pub mod cli;
pub mod config;
pub mod protocol;

#[cfg(feature = "browser")]
pub mod browser;
#[cfg(all(feature = "browser", target_os = "macos"))]
pub mod mac;

#[cfg(feature = "browser")]
pub fn initialize_cef_api() -> bool {
    let hash = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    if hash.is_null() {
        return false;
    }
    // Official macOS CEF API 15600 hash; the loaded framework must match our headers.
    (unsafe { std::ffi::CStr::from_ptr(hash).to_bytes() })
        == b"e01e9208f887490aa249d57f56761ab9c0c3f7ac"
}
