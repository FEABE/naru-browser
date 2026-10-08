use std::{env, io, path::PathBuf};

pub fn data_dir() -> io::Result<PathBuf> {
    let path = if let Some(path) = env::var_os("LIGHT_BROWSER_DATA_DIR") {
        PathBuf::from(path)
    } else {
        let home = env::var_os("HOME")
            .ok_or_else(|| io::Error::other("Set LIGHT_BROWSER_DATA_DIR when HOME is absent."))?;
        PathBuf::from(home).join("Library/Application Support/Light Browser")
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

pub fn socket_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("control.sock"))
}

pub fn log_file() -> io::Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::other("HOME is required for the macOS application log directory.")
    })?;
    Ok(PathBuf::from(home).join("Library/Logs/Light Browser/chromium.log"))
}
