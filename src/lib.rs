pub mod address;
pub mod bookmarks;
pub mod cli;
pub mod closed_pages;
pub mod config;
pub mod download_permissions;
pub mod downloads;
pub mod file_selection;
pub mod history;
pub mod keyboard;
pub mod page_find;
pub mod page_recovery;
pub mod protocol;
pub mod resources;
pub mod site_permissions;
pub mod spaces;
pub mod start_page;
pub mod tab_search;
pub mod tasks;

#[cfg(feature = "browser")]
pub mod bookmark_runtime;
#[cfg(feature = "browser")]
pub mod browser;
#[cfg(feature = "browser")]
pub mod capture;
#[cfg(all(feature = "browser", target_os = "macos"))]
pub mod control_panel;
#[cfg(feature = "browser")]
mod devtools_wire;
#[cfg(feature = "browser")]
pub mod download_permission_runtime;
#[cfg(feature = "browser")]
pub mod download_runtime;
#[cfg(all(feature = "browser", target_os = "macos"))]
pub mod file_selection_native;
#[cfg(feature = "browser")]
pub mod file_selection_runtime;
#[cfg(feature = "browser")]
mod frame_pointer;
#[cfg(feature = "browser")]
mod frame_sessions;
#[cfg(feature = "browser")]
pub mod history_runtime;
#[cfg(all(feature = "browser", target_os = "macos"))]
pub mod mac;
#[cfg(feature = "browser")]
mod page_read;
#[cfg(feature = "browser")]
pub mod site_permission_runtime;

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
