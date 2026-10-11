use base64::{engine::general_purpose::STANDARD, read::DecoderReader};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::MetadataExt,
    path::Path,
};

pub const MAX_ENCODED_PNG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CAPTURE_RESULT_BYTES: usize = MAX_ENCODED_PNG_BYTES + 64 * 1024;

#[derive(Debug)]
pub enum CaptureError {
    TooLarge,
    Io(io::Error),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(
                formatter,
                "encoded PNG exceeds {MAX_ENCODED_PNG_BYTES} bytes"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TooLarge => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// Decode directly into a new file, without allocating a decoded image buffer.
pub fn save_png_base64(path: &Path, data: &str) -> Result<u64, CaptureError> {
    if data.len() > MAX_ENCODED_PNG_BYTES {
        return Err(CaptureError::TooLarge);
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(CaptureError::Io)?;
    let mut decoded = DecoderReader::new(data.as_bytes(), &STANDARD);
    match io::copy(&mut decoded, &mut output) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            remove_partial_if_owned(path, &output);
            Err(CaptureError::Io(error))
        }
    }
}

fn remove_partial_if_owned(path: &Path, output: &File) {
    let (Ok(created), Ok(current)) = (output.metadata(), fs::symlink_metadata(path)) else {
        return;
    };
    // A replaced path or symlink is not this invocation's partial output.
    if current.file_type().is_file()
        && created.dev() == current.dev()
        && created.ino() == current.ino()
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    const ENCODED_PNG_SIGNATURE: &str = "iVBORw0KGgo=";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            loop {
                let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "light-browser-capture-{}-{suffix}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create capture test directory: {error}"),
                }
            }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn streams_decoded_bytes_and_returns_written_size() {
        let directory = TestDirectory::new();
        let path = directory.file("capture.png");

        let written = save_png_base64(&path, ENCODED_PNG_SIGNATURE).unwrap();

        assert_eq!(written, 8);
        assert_eq!(fs::read(path).unwrap(), b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn never_overwrites_an_existing_file() {
        let directory = TestDirectory::new();
        let path = directory.file("existing.png");
        fs::write(&path, b"keep existing data").unwrap();

        let error = save_png_base64(&path, ENCODED_PNG_SIGNATURE).unwrap_err();

        assert!(matches!(
            error,
            CaptureError::Io(error) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(path).unwrap(), b"keep existing data");
    }

    #[test]
    fn malformed_base64_removes_its_partial_output() {
        let directory = TestDirectory::new();
        let path = directory.file("invalid.png");
        let malformed = "YWJj".repeat(4096) + "!!!!";

        assert!(matches!(
            save_png_base64(&path, &malformed),
            Err(CaptureError::Io(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn rejects_oversized_input_before_creating_a_file() {
        let directory = TestDirectory::new();
        let path = directory.file("oversized.png");
        let oversized = "A".repeat(MAX_ENCODED_PNG_BYTES + 1);

        assert!(matches!(
            save_png_base64(&path, &oversized),
            Err(CaptureError::TooLarge)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_preserves_a_replacement_file() {
        let directory = TestDirectory::new();
        let path = directory.file("capture.png");
        let original = directory.file("original.png");
        let output = File::create(&path).unwrap();
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"replacement").unwrap();

        remove_partial_if_owned(&path, &output);

        assert_eq!(fs::read(path).unwrap(), b"replacement");
        assert!(original.exists());
    }

    #[test]
    fn cleanup_preserves_a_replacement_symlink() {
        let directory = TestDirectory::new();
        let path = directory.file("capture.png");
        let original = directory.file("original.png");
        let output = File::create(&path).unwrap();
        fs::rename(&path, &original).unwrap();
        std::os::unix::fs::symlink(&original, &path).unwrap();

        remove_partial_if_owned(&path, &output);

        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        assert!(original.exists());
    }
}
