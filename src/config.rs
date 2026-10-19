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

/// 검증 전용 mock keychain 여부. 실제 프로필에는 mock 키를 쓰지 않도록 데이터 경로 지정을 요구한다.
pub fn mock_keychain() -> io::Result<bool> {
    mock_keychain_from(
        env::var_os("NARU_MOCK_KEYCHAIN").as_deref(),
        env::var_os("LIGHT_BROWSER_DATA_DIR").is_some(),
    )
}

fn mock_keychain_from(flag: Option<&std::ffi::OsStr>, has_data_dir: bool) -> io::Result<bool> {
    if flag != Some(std::ffi::OsStr::new("1")) {
        return Ok(false);
    }
    if !has_data_dir {
        return Err(io::Error::other(
            "NARU_MOCK_KEYCHAIN=1 requires LIGHT_BROWSER_DATA_DIR so the real profile is not opened with a mock key.",
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::mock_keychain_from;
    use std::ffi::OsStr;

    #[test]
    fn mock_keychain_requires_separate_data_dir() {
        let on = Some(OsStr::new("1"));
        assert!(!mock_keychain_from(None, false).unwrap());
        assert!(!mock_keychain_from(Some(OsStr::new("0")), false).unwrap());
        assert!(mock_keychain_from(on, false).is_err());
        assert!(mock_keychain_from(on, true).unwrap());
    }
}
