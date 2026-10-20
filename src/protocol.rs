//! One JSON request and one JSON response per local socket connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, DirBuilder};
use std::io::{self, Read, Write};
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SELECT_VALUE_UTF16: usize = 1024;
/// Longest ref accepted by element commands.
pub const MAX_REF_BYTES: usize = 256;
/// Longest `text=`, `css=` or `role=` selector accepted where a ref is accepted.
pub const MAX_SELECTOR_BYTES: usize = 1024;
/// Largest absolute wheel delta, in CSS pixels.
pub const MAX_WHEEL_DELTA: i32 = 10000;
pub const MAX_POINTER_COORDINATE: i32 = 1_000_000;
pub const MAX_CLICK_COUNT: u8 = 3;
/// Response id the server uses when it rejects a request before reading its id.
pub const INVALID_REQUEST_ID: &str = "invalid";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const RESPONSE_READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskScope {
    pub task: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitUntil {
    Dom,
    Load,
    /// Main-frame URL (native, not page JS) starts with `prefix`.
    Url {
        prefix: String,
    },
    /// Visible text of the main document contains `substring` (case-sensitive).
    Text {
        substring: String,
    },
}

pub const WAIT_NEEDLE_MAX_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
}

impl MouseButton {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

fn default_click_count() -> u8 {
    1
}

/// True when a target string is a selector (`text=`, `css=` or `role=`), not a ref.
/// Refs start with a navigation epoch digit or `frame/`, so the forms never collide.
pub fn is_selector(value: &str) -> bool {
    value.starts_with("text=") || value.starts_with("css=") || value.starts_with("role=")
}

/// Validate a REF or selector target. Refs keep their 1..256 byte rule; selectors
/// allow at most 1024 bytes in total, a non-empty body and no NUL.
pub fn validate_target(value: &str) -> Result<(), ProtocolError> {
    let invalid = |message| ProtocolError::new("INVALID_REQUEST", message);
    if is_selector(value) {
        let body = &value[value.find('=').map_or(0, |index| index + 1)..];
        if value.len() > MAX_SELECTOR_BYTES || value.contains('\0') || body.is_empty() {
            return Err(invalid(
                "Selector must be text=, css= or role= with a non-empty value, at most 1024 bytes in total and no NUL.",
            ));
        }
        return Ok(());
    }
    if value.is_empty() || value.len() > MAX_REF_BYTES {
        return Err(invalid("REF must contain 1 to 256 bytes."));
    }
    Ok(())
}

fn validate_coordinates(x: i32, y: i32) -> Result<(), ProtocolError> {
    if !(0..=MAX_POINTER_COORDINATE).contains(&x) || !(0..=MAX_POINTER_COORDINATE).contains(&y) {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "X and Y must be integer viewport CSS pixels from 0 to 1000000.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DialogAction {
    /// `text` is the prompt answer; only a prompt dialog accepts it.
    Accept {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Dismiss {},
}

impl WaitUntil {
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Dom => "dom",
            Self::Load => "load",
            Self::Url { .. } => "url",
            Self::Text { .. } => "text",
        }
    }

    pub fn expected(&self) -> Option<&str> {
        match self {
            Self::Url { prefix } => Some(prefix),
            Self::Text { substring } => Some(substring),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        let Some(value) = self.expected() else {
            return Ok(());
        };
        // JS `\s` also matches U+FEFF; a text needle that collapses to nothing can never be searched.
        let blank_text = matches!(self, Self::Text { .. })
            && value.chars().all(|c| c.is_whitespace() || c == '\u{FEFF}');
        if value.is_empty()
            || value.len() > WAIT_NEEDLE_MAX_BYTES
            || value.contains('\0')
            || blank_text
        {
            return Err(ProtocolError::new(
                "INVALID_REQUEST",
                "Wait url/text value must be 1 to 4096 bytes, contain no NUL, and text must have a non-whitespace character.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    TaskCreate {
        name: String,
    },
    TaskList,
    TaskInfo {
        task: String,
    },
    TaskHandoff {
        task: String,
        revision: u64,
    },
    TaskResume {
        task: String,
        revision: u64,
    },
    /// Block until the task's state or revision changes (no revision needed:
    /// the baseline is the task state when the request is admitted).
    TaskWait {
        task: String,
        timeout_ms: u64,
        /// Last revision the caller knows; a different current revision
        /// returns immediately instead of waiting for another change.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
    },
    TaskAdopt {
        task: String,
        revision: u64,
        tab: i32,
    },
    TaskRelease {
        task: String,
        revision: u64,
        tab: i32,
    },
    TaskFinish {
        task: String,
        revision: u64,
        keep: Vec<i32>,
    },
    TaskRun {
        task: String,
        revision: u64,
        command: Box<Command>,
    },
    Open {
        url: String,
    },
    Navigate {
        tab: i32,
        url: String,
    },
    Back {
        tab: i32,
    },
    Forward {
        tab: i32,
    },
    Reload {
        tab: i32,
    },
    Stop {
        tab: i32,
    },
    Tabs,
    Spaces,
    SpaceShow {
        space: String,
    },
    SpaceOverview,
    Wait {
        tab: i32,
        until: WaitUntil,
        timeout_ms: u64,
    },
    Read {
        tab: i32,
    },
    ReadPage {
        tab: i32,
    },
    Frames {
        tab: i32,
    },
    ReadFrame {
        tab: i32,
        frame: String,
    },
    Scroll {
        tab: i32,
        x: i32,
        y: i32,
    },
    ScrollElement {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        x: i32,
        y: i32,
    },
    Click {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
    },
    Hover {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Chromium double click (two press/release pairs, clickCount 1 then 2).
    #[serde(rename = "dblclick")]
    DblClick {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Chromium right button press/release at a verified element point.
    RightClick {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Chromium mouseWheel at a verified element point (distinct from `scroll`).
    Wheel {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        dx: i32,
        dy: i32,
    },
    /// Press at `from`, move through intermediate points, release at `to`.
    Drag {
        tab: i32,
        from: String,
        to: String,
    },
    /// Click at a viewport CSS-pixel coordinate.
    ClickAt {
        tab: i32,
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default = "default_click_count")]
        count: u8,
    },
    /// Move the pointer to a viewport CSS-pixel coordinate.
    MoveAt {
        tab: i32,
        x: i32,
        y: i32,
    },
    Check {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        checked: bool,
    },
    Fill {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        text: String,
    },
    Insert {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        text: String,
    },
    Select {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        value: String,
    },
    Press {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        key: String,
    },
    Close {
        tab: i32,
    },
    /// Answer the JavaScript dialog queued for an AI-owned tab (task run only).
    Dialog {
        tab: i32,
        action: DialogAction,
    },
    Screenshot {
        tab: i32,
        path: String,
    },
    Status,
    Panel,
}

impl Command {
    pub fn tab_id(&self) -> Option<i32> {
        match self {
            Self::Navigate { tab, .. }
            | Self::Back { tab }
            | Self::Forward { tab }
            | Self::Reload { tab }
            | Self::Stop { tab }
            | Self::Wait { tab, .. }
            | Self::Read { tab }
            | Self::ReadPage { tab }
            | Self::Frames { tab }
            | Self::ReadFrame { tab, .. }
            | Self::Scroll { tab, .. }
            | Self::ScrollElement { tab, .. }
            | Self::Click { tab, .. }
            | Self::Hover { tab, .. }
            | Self::DblClick { tab, .. }
            | Self::RightClick { tab, .. }
            | Self::Wheel { tab, .. }
            | Self::Drag { tab, .. }
            | Self::ClickAt { tab, .. }
            | Self::MoveAt { tab, .. }
            | Self::Check { tab, .. }
            | Self::Fill { tab, .. }
            | Self::Insert { tab, .. }
            | Self::Select { tab, .. }
            | Self::Press { tab, .. }
            | Self::Close { tab }
            | Self::Dialog { tab, .. }
            | Self::Screenshot { tab, .. }
            | Self::TaskAdopt { tab, .. }
            | Self::TaskRelease { tab, .. } => Some(*tab),
            Self::TaskRun { command, .. } => command.tab_id(),
            _ => None,
        }
    }

    pub fn is_task_command(&self) -> bool {
        matches!(
            self,
            Self::TaskCreate { .. }
                | Self::TaskList
                | Self::TaskInfo { .. }
                | Self::TaskHandoff { .. }
                | Self::TaskResume { .. }
                | Self::TaskWait { .. }
                | Self::TaskAdopt { .. }
                | Self::TaskRelease { .. }
                | Self::TaskFinish { .. }
                | Self::TaskRun { .. }
        )
    }

    /// Validate command parameters independently of the local socket transport.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let invalid = |message| ProtocolError::new("INVALID_REQUEST", message);
        // Reject a nested command before traversing any of its parameters.
        if let Self::TaskRun { command, .. } = self {
            if command.is_task_command() {
                return Err(invalid(
                    "task run accepts only a browser command, not a task command.",
                ));
            }
        }
        if self.tab_id().is_some_and(|tab| tab <= 0) {
            return Err(invalid("TAB must be a positive 32-bit integer."));
        }
        match self {
            Self::ReadFrame { frame, .. }
                if frame.is_empty()
                    || frame.len() > 256
                    || !frame.is_ascii()
                    || frame.bytes().any(|byte| byte.is_ascii_control()) =>
            {
                return Err(invalid(
                    "FRAME must be a current CDP frame identifier of 1 to 256 ASCII bytes without control characters.",
                ));
            }
            Self::TaskCreate { name } => {
                if name.trim().is_empty()
                    || name.trim().chars().count() > 120
                    || name.chars().any(char::is_control)
                {
                    return Err(invalid(
                        "Task name must contain 1 to 120 characters after trimming, with no control characters.",
                    ));
                }
            }
            Self::SpaceShow { space } => {
                if space.len() > 120 || validate_task_id(space).is_err() {
                    return Err(invalid(
                        "Space must be user or a task ID of 1 to 120 ASCII letters, digits, hyphens or underscores.",
                    ));
                }
            }
            Self::TaskInfo { task } => validate_task_id(task)?,
            Self::TaskWait {
                task,
                timeout_ms,
                since,
            } => {
                validate_task_id(task)?;
                if *since == Some(0) {
                    return Err(invalid("REV must be a positive 64-bit integer."));
                }
                if !(1..=30000).contains(timeout_ms) {
                    return Err(invalid("Task wait timeout_ms must be between 1 and 30000."));
                }
            }
            Self::TaskHandoff { task, revision }
            | Self::TaskResume { task, revision }
            | Self::TaskAdopt { task, revision, .. }
            | Self::TaskRelease { task, revision, .. }
            | Self::TaskFinish { task, revision, .. }
            | Self::TaskRun { task, revision, .. } => {
                validate_task_id(task)?;
                if *revision == 0 {
                    return Err(invalid("REV must be a positive 64-bit integer."));
                }
            }
            Self::Wait { timeout_ms, .. } if !(1..=30000).contains(timeout_ms) => {
                return Err(invalid("Wait timeout_ms must be between 1 and 30000."));
            }
            Self::Wait { until, .. } => until.validate()?,
            Self::Dialog {
                action: DialogAction::Accept { text: Some(text) },
                ..
            } if !crate::dialogs::text_is_valid(text) => {
                return Err(invalid(
                    "Dialog TEXT must be at most 4096 bytes and contain no NUL.",
                ));
            }
            Self::Click { reference, .. }
            | Self::Hover { reference, .. }
            | Self::DblClick { reference, .. }
            | Self::RightClick { reference, .. }
            | Self::Check { reference, .. }
            | Self::Fill { reference, .. }
            | Self::Insert { reference, .. }
            | Self::Press { reference, .. } => validate_target(reference)?,
            Self::Wheel {
                reference, dx, dy, ..
            } => {
                validate_target(reference)?;
                if !(-MAX_WHEEL_DELTA..=MAX_WHEEL_DELTA).contains(dx)
                    || !(-MAX_WHEEL_DELTA..=MAX_WHEEL_DELTA).contains(dy)
                    || (*dx == 0 && *dy == 0)
                {
                    return Err(invalid(
                        "Wheel dx and dy must be integers between -10000 and 10000 and not both zero.",
                    ));
                }
            }
            Self::Drag { from, to, .. } => {
                validate_target(from)?;
                validate_target(to)?;
                if from == to {
                    return Err(invalid("Drag FROM and TO must differ."));
                }
                // A selector re-reads the page and renumbers every ref, so a ref
                // mixed with a selector would be stale before it is used.
                if is_selector(from) != is_selector(to) {
                    return Err(invalid(
                        "Drag FROM and TO must both be refs or both be selectors.",
                    ));
                }
            }
            Self::ClickAt { x, y, count, .. } => {
                validate_coordinates(*x, *y)?;
                if !(1..=MAX_CLICK_COUNT).contains(count) {
                    return Err(invalid("Click count must be 1 to 3."));
                }
            }
            Self::MoveAt { x, y, .. } => validate_coordinates(*x, *y)?,
            Self::ScrollElement { reference, .. } => {
                validate_target(reference)?;
                if let Self::ScrollElement { x, y, .. } = self
                    && (!(-10000..=10000).contains(x) || !(-10000..=10000).contains(y))
                {
                    return Err(invalid(
                        "Scroll x and y must be integers between -10000 and 10000 CSS pixels.",
                    ));
                }
            }
            Self::Select {
                reference, value, ..
            } => {
                validate_target(reference)?;
                if value
                    .encode_utf16()
                    .take(MAX_SELECT_VALUE_UTF16 + 1)
                    .count()
                    > MAX_SELECT_VALUE_UTF16
                {
                    return Err(invalid(
                        "Select value must contain at most 1024 UTF-16 code units.",
                    ));
                }
            }
            Self::Scroll { x, y, .. }
                if !(-10000..=10000).contains(x) || !(-10000..=10000).contains(y) =>
            {
                return Err(invalid(
                    "Scroll x and y must be integers between -10000 and 10000 CSS pixels.",
                ));
            }
            _ => {}
        }
        if let Self::TaskFinish { keep, .. } = self {
            let mut unique = BTreeSet::new();
            if keep.iter().any(|tab| *tab <= 0 || !unique.insert(*tab)) {
                return Err(invalid(
                    "Keep tabs must be distinct positive 32-bit integers.",
                ));
            }
        }
        if let Self::TaskRun { command, .. } = self {
            command.validate()?;
        }
        Ok(())
    }
}

fn validate_task_id(task: &str) -> Result<(), ProtocolError> {
    if task.is_empty()
        || task.len() > 128
        || !task
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "Task ID must contain 1 to 128 ASCII letters, digits, hyphens or underscores.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(
        default,
        deserialize_with = "present_json_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

fn present_json_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl Response {
    pub fn success(id: impl Into<String>, result: Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
                details: None,
            }),
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        if let Some(error) = &mut self.error {
            error.details = Some(details);
        }
        self
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if (self.ok && (self.result.is_none() || self.error.is_some()))
            || (!self.ok && (self.result.is_some() || self.error.is_none()))
        {
            return Err(ProtocolError::new(
                "INVALID_RESPONSE",
                "Response must contain either a success result or an error.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    /// A failed response does not establish whether the browser performed the action.
    pub outcome_unknown: bool,
}

impl ProtocolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            outcome_unknown: false,
        }
    }

    fn after_send(mut self) -> Self {
        self.outcome_unknown = true;
        self
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

fn io_error(phase: &'static str, error: io::Error) -> ProtocolError {
    let code = if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        "TIMEOUT"
    } else {
        "SOCKET_IO_ERROR"
    };
    ProtocolError::new(code, format!("{phase}: {error}"))
}

fn current_uid() -> libc::uid_t {
    // geteuid has no arguments or failure result.
    unsafe { libc::geteuid() }
}

fn verify_private_parent(socket: &Path, create: bool) -> Result<(), ProtocolError> {
    let parent = socket
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            ProtocolError::new(
                "INVALID_SOCKET_PATH",
                "Socket must have a parent directory.",
            )
        })?;
    if create && !parent.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| io_error("create socket directory", e))?;
    }
    let metadata = fs::symlink_metadata(parent).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ProtocolError::new(
                "APP_NOT_RUNNING",
                "Browser socket directory does not exist. Start the browser app first.",
            )
        } else {
            io_error("inspect socket directory", e)
        }
    })?;
    if !metadata.is_dir() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(ProtocolError::new(
            "INSECURE_SOCKET_DIRECTORY",
            "Socket parent must be a real directory owned by the current user with no group or other access.",
        ));
    }
    Ok(())
}

fn verify_socket_file(socket: &Path) -> Result<(), ProtocolError> {
    let metadata = fs::symlink_metadata(socket).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ProtocolError::new(
                "APP_NOT_RUNNING",
                "Browser socket does not exist. Start the browser app first.",
            )
        } else {
            io_error("inspect socket", e)
        }
    })?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(ProtocolError::new(
            "INSECURE_SOCKET",
            "Socket must be owned by the current user with no group or other access.",
        ));
    }
    Ok(())
}

/// Creates only a socket inside a private directory; never replaces a live socket.
pub fn bind_listener(socket: &Path) -> Result<UnixListener, ProtocolError> {
    socket_address(socket.as_os_str())?;
    verify_private_parent(socket, true)?;
    match fs::symlink_metadata(socket) {
        Ok(_) => {
            verify_socket_file(socket)?;
            match connect_with_timeout(socket, CONNECT_TIMEOUT) {
                Ok(_) => {
                    return Err(ProtocolError::new(
                        "SOCKET_IN_USE",
                        "A browser service is already listening on this socket.",
                    ));
                }
                Err(error) if error.code == "APP_NOT_RUNNING" => {
                    fs::remove_file(socket).map_err(|e| io_error("remove stale socket", e))?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect socket", error)),
    }
    let listener = UnixListener::bind(socket).map_err(|e| io_error("bind socket", e))?;
    if let Err(error) = fs::set_permissions(socket, fs::Permissions::from_mode(0o600)) {
        drop(listener);
        let _ = fs::remove_file(socket);
        return Err(io_error("set socket permissions", error));
    }
    Ok(listener)
}

fn socket_address(path: &OsStr) -> Result<libc::sockaddr_un, ProtocolError> {
    let bytes = path.as_bytes();
    // All-zero padding and a terminating NUL are required by sockaddr_un.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(ProtocolError::new(
            "INVALID_SOCKET_PATH",
            format!(
                "Socket path must be nonempty, contain no NUL, and be shorter than {} bytes.",
                address.sun_path.len()
            ),
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        address.sun_len = mem::size_of::<libc::sockaddr_un>() as u8;
    }
    for (destination, source) in address.sun_path.iter_mut().zip(bytes) {
        *destination = *source as libc::c_char;
    }
    Ok(address)
}

fn connect_with_timeout(socket: &Path, timeout: Duration) -> Result<UnixStream, ProtocolError> {
    let address = socket_address(socket.as_os_str())?;
    // The returned descriptor is immediately owned by UnixStream on success.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io_error("create socket", io::Error::last_os_error()));
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // Do not allow a browser subprocess to inherit this local control connection.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io_error("set close-on-exec", io::Error::last_os_error()));
    }
    stream
        .set_nonblocking(true)
        .map_err(|e| io_error("set nonblocking socket", e))?;
    let result = unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::EINPROGRESS) | Some(libc::EAGAIN)
        ) {
            return Err(connect_error(error));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ProtocolError::new(
                    "TIMEOUT",
                    "Timed out connecting to the browser socket.",
                ));
            }
            let mut descriptor = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            let wait_ms = remaining.as_millis().max(1).min(i32::MAX as u128) as i32;
            let ready = unsafe { libc::poll(&mut descriptor, 1, wait_ms) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io_error("wait for socket connection", error));
            }
            if ready == 0 {
                continue;
            }
            let mut socket_error: libc::c_int = 0;
            let mut length = mem::size_of_val(&socket_error) as libc::socklen_t;
            if unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    &mut socket_error as *mut _ as *mut libc::c_void,
                    &mut length,
                )
            } < 0
            {
                return Err(io_error(
                    "inspect socket connection",
                    io::Error::last_os_error(),
                ));
            }
            if socket_error != 0 {
                return Err(connect_error(io::Error::from_raw_os_error(socket_error)));
            }
            stream.peer_addr().map_err(connect_error)?;
            break;
        }
    }
    stream
        .set_nonblocking(false)
        .map_err(|e| io_error("restore blocking socket", e))?;
    verify_peer_uid(&stream)?;
    Ok(stream)
}

fn connect_error(error: io::Error) -> ProtocolError {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    ) {
        ProtocolError::new(
            "APP_NOT_RUNNING",
            "Browser is not listening on its socket. Start the browser app first.",
        )
    } else {
        io_error("connect socket", error)
    }
}

fn verify_peer_uid(stream: &UnixStream) -> Result<(), ProtocolError> {
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    let peer_uid = {
        let mut uid = 0;
        let mut gid = 0;
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io_error("inspect socket peer", io::Error::last_os_error()));
        }
        uid
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let peer_uid = {
        let mut credentials: libc::ucred = unsafe { mem::zeroed() };
        let mut length = mem::size_of_val(&credentials) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut length,
            )
        } != 0
        {
            return Err(io_error("inspect socket peer", io::Error::last_os_error()));
        }
        credentials.uid
    };
    if peer_uid != current_uid() {
        return Err(ProtocolError::new(
            "PEER_NOT_ALLOWED",
            "Socket peer is not running as the current user.",
        ));
    }
    Ok(())
}

fn wait_ready(
    stream: &UnixStream,
    events: libc::c_short,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ProtocolError::new(
                "TIMEOUT",
                "Timed out transferring the complete JSON frame.",
            ));
        }
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        let wait_ms = remaining.as_millis().max(1).min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut descriptor, 1, wait_ms) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io_error("wait for JSON frame", error));
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(ProtocolError::new(
                "SOCKET_IO_ERROR",
                "Socket descriptor is no longer valid.",
            ));
        }
        // HUP still permits consuming bytes already buffered before the peer closed.
        return Ok(());
    }
}

fn read_frame(
    stream: &mut UnixStream,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ProtocolError> {
    let mut frame = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 8192];
    stream
        .set_nonblocking(true)
        .map_err(|e| io_error("set frame read mode", e))?;
    loop {
        wait_ready(stream, libc::POLLIN, deadline)?;
        // Leave room for a newline, but never allocate beyond the payload limit.
        let capacity = buffer.len().min(limit + 1 - frame.len());
        let count = match stream.read(&mut buffer[..capacity]) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(io_error("read JSON frame", error)),
        };
        if count == 0 {
            return Err(ProtocolError::new(
                "INCOMPLETE_FRAME",
                "Expected one JSON line ending in a newline.",
            ));
        }
        if let Some(end) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            frame.extend_from_slice(&buffer[..end]);
            return Ok(frame);
        }
        if frame.len() + count > limit {
            return Err(ProtocolError::new(
                "FRAME_TOO_LARGE",
                "JSON frame exceeds the size limit.",
            ));
        }
        frame.extend_from_slice(&buffer[..count]);
    }
}

struct BoundedFrame {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedFrame {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedFrame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.exceeded || bytes.len() > self.limit - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON frame exceeds the size limit.",
            ));
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            // Vec's implicit geometric growth may exceed a non-power-of-two
            // limit. Reserve a clamped capacity before extending the buffer.
            let capacity = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(4096)
                .max(needed)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_frame<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    limit: usize,
) -> Result<(), ProtocolError> {
    let mut frame = BoundedFrame::new(limit);
    serde_json::to_writer(&mut frame, value).map_err(|error| {
        if frame.exceeded {
            ProtocolError::new("FRAME_TOO_LARGE", "JSON frame exceeds the size limit.")
        } else {
            ProtocolError::new("INVALID_JSON", error.to_string())
        }
    })?;
    let deadline = Instant::now() + WRITE_TIMEOUT;
    stream
        .set_nonblocking(true)
        .map_err(|e| io_error("set frame write mode", e))?;
    // The newline is outside the payload limit. Keep it outside the allocation
    // too, including when serialization fills the buffer exactly to its limit.
    for mut pending in [frame.bytes.as_slice(), b"\n"] {
        while !pending.is_empty() {
            wait_ready(stream, libc::POLLOUT, deadline)?;
            match stream.write(pending) {
                Ok(0) => {
                    return Err(ProtocolError::new(
                        "SOCKET_IO_ERROR",
                        "Socket stopped accepting the JSON frame.",
                    ));
                }
                Ok(count) => pending = &pending[count..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(io_error("write JSON frame", error)),
            }
        }
    }
    Ok(())
}

fn validate_command_fields(value: &Value, allow_task: bool) -> Result<(), ProtocolError> {
    let Some(command) = value.as_object() else {
        return Ok(());
    };
    let command_type = command.get("type").and_then(Value::as_str);
    if !allow_task
        && !matches!(
            command_type,
            Some(
                "open"
                    | "navigate"
                    | "back"
                    | "forward"
                    | "reload"
                    | "stop"
                    | "tabs"
                    | "spaces"
                    | "space_show"
                    | "space_overview"
                    | "wait"
                    | "read"
                    | "frames"
                    | "read_page"
                    | "read_frame"
                    | "scroll"
                    | "scroll_element"
                    | "click"
                    | "hover"
                    | "dblclick"
                    | "right_click"
                    | "wheel"
                    | "drag"
                    | "click_at"
                    | "move_at"
                    | "check"
                    | "fill"
                    | "insert"
                    | "select"
                    | "press"
                    | "close"
                    | "dialog"
                    | "screenshot"
                    | "status"
                    | "panel"
            )
        )
    {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "task run accepts only a browser command, not a task command.",
        ));
    }
    // Serde ignores extra fields on internally tagged unit variants, even with deny_unknown_fields.
    if matches!(
        command_type,
        Some("tabs" | "spaces" | "space_overview" | "status" | "panel" | "task_list")
    ) && command.len() != 1
    {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "This command does not accept additional fields.",
        ));
    }
    if command_type == Some("task_run") {
        validate_command_fields(&value["command"], false)?;
    }
    Ok(())
}

pub fn read_request(stream: &mut UnixStream) -> Result<Request, ProtocolError> {
    verify_peer_uid(stream)?;
    let frame = read_frame(stream, MAX_REQUEST_BYTES, REQUEST_READ_TIMEOUT)?;
    let value: Value = serde_json::from_slice(&frame)
        .map_err(|e| ProtocolError::new("INVALID_REQUEST", e.to_string()))?;
    validate_command_fields(&value["command"], true)?;
    let mut request: Request = serde_json::from_value(value)
        .map_err(|e| ProtocolError::new("INVALID_REQUEST", e.to_string()))?;
    if request.id.is_empty() || request.id.len() > 128 {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "Request id must contain 1 to 128 bytes.",
        ));
    }
    request.command.validate()?;
    if let Command::TaskCreate { name } = &mut request.command {
        *name = name.trim().to_owned();
    }
    Ok(request)
}

pub fn write_response(stream: &mut UnixStream, response: &Response) -> Result<(), ProtocolError> {
    verify_peer_uid(stream)?;
    response.validate()?;
    write_frame(stream, response, MAX_RESPONSE_BYTES)
}

pub fn call(socket: &Path, request: &Request) -> Result<Response, ProtocolError> {
    verify_private_parent(socket, false)?;
    verify_socket_file(socket)?;
    let mut stream = connect_with_timeout(socket, CONNECT_TIMEOUT)?;
    write_frame(&mut stream, request, MAX_REQUEST_BYTES).map_err(|error| {
        if error.code == "FRAME_TOO_LARGE" || error.code == "INVALID_JSON" {
            error
        } else {
            error.after_send()
        }
    })?;
    let frame = read_frame(&mut stream, MAX_RESPONSE_BYTES, RESPONSE_READ_TIMEOUT)
        .map_err(ProtocolError::after_send)?;
    let response: Response = serde_json::from_slice(&frame)
        .map_err(|e| ProtocolError::new("INVALID_RESPONSE", e.to_string()).after_send())?;
    response.validate().map_err(ProtocolError::after_send)?;
    if response.id != request.id {
        // The server answers requests it rejects before execution with this id.
        if response.id == INVALID_REQUEST_ID {
            if let Some(error) = &response.error {
                return Err(ProtocolError::new(
                    error.code.clone(),
                    error.message.clone(),
                ));
            }
        }
        return Err(ProtocolError::new(
            "RESPONSE_ID_MISMATCH",
            "Response id does not match the request; the action outcome is unknown.",
        )
        .after_send());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "light-ipc-test-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }

        fn socket(&self) -> PathBuf {
            self.0.join("browser.sock")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn wait_command_round_trip_and_invalid_mode() {
        for (until, serialized) in [(WaitUntil::Dom, "dom"), (WaitUntil::Load, "load")] {
            let command = Command::Wait {
                tab: 3,
                until,
                timeout_ms: 10000,
            };
            let value = json!({"type":"wait","tab":3,"until":serialized,"timeout_ms":10000});
            assert_eq!(serde_json::to_value(&command).unwrap(), value);
            assert_eq!(serde_json::from_value::<Command>(value).unwrap(), command);
        }
        assert!(
            serde_json::from_value::<Command>(
                json!({"type":"wait","tab":3,"until":"networkidle","timeout_ms":10000})
            )
            .is_err()
        );
    }

    #[test]
    fn dialog_command_round_trips_and_validates_text() {
        for (action, value) in [
            (
                DialogAction::Accept { text: None },
                json!({"type":"accept"}),
            ),
            (
                DialogAction::Accept {
                    text: Some("값".into()),
                },
                json!({"type":"accept","text":"값"}),
            ),
            (DialogAction::Dismiss {}, json!({"type":"dismiss"})),
        ] {
            let command = Command::Dialog { tab: 3, action };
            let wire = json!({"type":"dialog","tab":3,"action":value});
            assert_eq!(serde_json::to_value(&command).unwrap(), wire);
            assert_eq!(serde_json::from_value::<Command>(wire).unwrap(), command);
            assert!(command.validate().is_ok());
            assert_eq!(command.tab_id(), Some(3));
            assert!(!command.is_task_command());
        }
        let accept = |text: String| Command::Dialog {
            tab: 3,
            action: DialogAction::Accept { text: Some(text) },
        };
        assert!(accept("a".repeat(4096)).validate().is_ok());
        assert!(accept("a".repeat(4097)).validate().is_err());
        assert!(accept("a\0b".into()).validate().is_err());
        assert!(
            Command::Dialog {
                tab: 0,
                action: DialogAction::Dismiss {}
            }
            .validate()
            .is_err()
        );
        let scoped = Command::TaskRun {
            task: "t".into(),
            revision: 1,
            command: Box::new(accept("a\0".into())),
        };
        assert!(scoped.validate().is_err());
        assert!(
            serde_json::from_value::<Command>(
                json!({"type":"dialog","tab":3,"action":{"type":"dismiss","text":"x"}})
            )
            .is_err()
        );
    }

    #[test]
    fn task_run_field_allowlist_accepts_dialog() {
        let wire = json!({"type":"task_run","task":"t","revision":1,
            "command":{"type":"dialog","tab":3,"action":{"type":"accept","text":"x"}}});
        assert!(validate_command_fields(&wire, true).is_ok());
        let command: Command = serde_json::from_value(wire).unwrap();
        assert!(command.validate().is_ok());
        let nested = json!({"type":"task_run","task":"t","revision":1,
            "command":{"type":"task_run","task":"t","revision":1,"command":{"type":"dialog","tab":3,"action":{"type":"dismiss"}}}});
        assert!(validate_command_fields(&nested, true).is_err());
    }

    #[test]
    fn url_and_text_waits_round_trip_and_validate_value() {
        for (until, value) in [
            (
                WaitUntil::Url {
                    prefix: "https://a.test/".into(),
                },
                json!({"url":{"prefix":"https://a.test/"}}),
            ),
            (
                WaitUntil::Text {
                    substring: "완료".into(),
                },
                json!({"text":{"substring":"완료"}}),
            ),
        ] {
            let command = Command::Wait {
                tab: 3,
                until,
                timeout_ms: 500,
            };
            let wire = json!({"type":"wait","tab":3,"until":value,"timeout_ms":500});
            assert_eq!(serde_json::to_value(&command).unwrap(), wire);
            assert_eq!(serde_json::from_value::<Command>(wire).unwrap(), command);
            assert!(command.validate().is_ok());
        }
        let edge = "a".repeat(WAIT_NEEDLE_MAX_BYTES);
        let long = "a".repeat(WAIT_NEEDLE_MAX_BYTES + 1);
        for (value, ok) in [
            ("", false),
            ("a\0b", false),
            (long.as_str(), false),
            (edge.as_str(), true),
        ] {
            for until in [
                WaitUntil::Url {
                    prefix: value.into(),
                },
                WaitUntil::Text {
                    substring: value.into(),
                },
            ] {
                let command = Command::Wait {
                    tab: 1,
                    until,
                    timeout_ms: 10,
                };
                assert_eq!(command.validate().is_ok(), ok, "{}", value.len());
                if !ok {
                    assert_eq!(command.validate().unwrap_err().code, "INVALID_REQUEST");
                }
            }
        }
        for blank in [" ", "\n\t", "\u{FEFF}", "\u{3000} "] {
            let text = Command::Wait {
                tab: 1,
                until: WaitUntil::Text {
                    substring: blank.into(),
                },
                timeout_ms: 10,
            };
            assert_eq!(
                text.validate().unwrap_err().code,
                "INVALID_REQUEST",
                "{blank:?}"
            );
        }
        // A url prefix is compared literally, so whitespace alone is not rejected here.
        assert!(
            Command::Wait {
                tab: 1,
                until: WaitUntil::Text {
                    substring: " a ".into()
                },
                timeout_ms: 10,
            }
            .validate()
            .is_ok()
        );
        // Multi-byte values are bounded by bytes, not characters.
        let wide = Command::Wait {
            tab: 1,
            until: WaitUntil::Text {
                substring: "가".repeat(1366),
            },
            timeout_ms: 10,
        };
        assert!(wide.validate().is_err());
        for bad in [
            json!({"url":{}}),
            json!({"url":{"prefix":"x","extra":1}}),
            json!({"text":"x"}),
            json!({"selector":{"css":"x"}}),
        ] {
            assert!(
                serde_json::from_value::<Command>(
                    json!({"type":"wait","tab":3,"until":bad,"timeout_ms":10})
                )
                .is_err()
            );
        }
        // Nested task commands are validated too.
        let nested = Command::TaskRun {
            task: "task-1".into(),
            revision: 1,
            command: Box::new(Command::Wait {
                tab: 1,
                until: WaitUntil::Url {
                    prefix: String::new(),
                },
                timeout_ms: 10,
            }),
        };
        assert!(nested.validate().is_err());
    }

    #[test]
    fn navigation_and_scroll_commands_round_trip() {
        for (command, value) in [
            (
                Command::Navigate {
                    tab: 3,
                    url: "https://example.test/검색?q=한글".into(),
                },
                json!({"type":"navigate","tab":3,"url":"https://example.test/검색?q=한글"}),
            ),
            (Command::Back { tab: 3 }, json!({"type":"back","tab":3})),
            (
                Command::Forward { tab: 3 },
                json!({"type":"forward","tab":3}),
            ),
            (Command::Reload { tab: 3 }, json!({"type":"reload","tab":3})),
            (Command::Stop { tab: 3 }, json!({"type":"stop","tab":3})),
            (Command::Frames { tab: 3 }, json!({"type":"frames","tab":3})),
            (
                Command::ReadFrame {
                    tab: 3,
                    frame: "frame-A1".into(),
                },
                json!({"type":"read_frame","tab":3,"frame":"frame-A1"}),
            ),
            (
                Command::Scroll {
                    tab: 3,
                    x: -10000,
                    y: 10000,
                },
                json!({"type":"scroll","tab":3,"x":-10000,"y":10000}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&command).unwrap(), value);
            assert_eq!(serde_json::from_value::<Command>(value).unwrap(), command);
        }
    }

    fn request_from_command_value(command: Value) -> Result<Request, ProtocolError> {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let mut frame =
            serde_json::to_vec(&json!({"id":"navigation-check","command":command})).unwrap();
        frame.push(b'\n');
        writer.write_all(&frame).unwrap();
        read_request(&mut reader)
    }

    #[test]
    fn check_boolean_round_trips_through_global_and_task_requests() {
        for checked in [false, true] {
            let command = Command::Check {
                tab: 3,
                reference: "9/doc:1:1".into(),
                checked,
            };
            let value = json!({"type":"check","tab":3,"ref":"9/doc:1:1","checked":checked});
            assert_eq!(serde_json::to_value(&command).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<Command>(value.clone()).unwrap(),
                command
            );
            assert_eq!(
                request_from_command_value(value.clone()).unwrap().command,
                command
            );
            assert_eq!(command.tab_id(), Some(3));
            assert!(!command.is_task_command());
            let nested = json!({"type":"task_run","task":"task-1","revision":2,"command":value});
            let request = request_from_command_value(nested.clone()).unwrap();
            assert_eq!(request.command.tab_id(), Some(3));
            assert_eq!(serde_json::to_value(request.command).unwrap(), nested);
        }
    }

    #[test]
    fn check_wire_requires_a_boolean_and_rejects_invalid_fields_in_both_scopes() {
        let base = json!({"type":"check","tab":3,"ref":"9/doc:1:1","checked":true});
        let mut invalid = Vec::new();
        for checked in [
            json!("true"),
            json!("false"),
            json!(0),
            json!(1),
            json!(1.0),
            Value::Null,
            json!([]),
            json!({}),
        ] {
            let mut command = base.clone();
            command["checked"] = checked;
            invalid.push(command);
        }
        invalid.extend([
            json!({"type":"check","tab":3,"ref":"9/doc:1:1"}),
            json!({"type":"check","tab":3,"checked":true}),
            json!({"type":"check","tab":0,"ref":"9/doc:1:1","checked":false}),
            json!({"type":"check","tab":-1,"ref":"9/doc:1:1","checked":true}),
            json!({"type":"check","tab":"3","ref":"9/doc:1:1","checked":true}),
            json!({"type":"check","tab":3,"ref":"9/doc:1:1","checked":true,"value":true}),
        ]);
        for command in invalid {
            for value in [
                command.clone(),
                json!({"type":"task_run","task":"task-1","revision":2,"command":command}),
            ] {
                assert_eq!(
                    request_from_command_value(value).unwrap_err().code,
                    "INVALID_REQUEST"
                );
            }
        }
    }

    #[test]
    fn select_wire_preserves_exact_values_and_enforces_utf16_limit_in_task_scope() {
        for value in [String::new(), "  국내 배송  ".into(), "🧭".repeat(512)] {
            let command = json!({"type":"select","tab":3,"ref":"9/doc:1:1","value":value});
            let direct = request_from_command_value(command.clone()).unwrap().command;
            assert_eq!(direct.tab_id(), Some(3));
            assert_eq!(serde_json::to_value(direct).unwrap(), command);
            let nested = json!({"type":"task_run","task":"task-1","revision":2,"command":command});
            assert_eq!(
                serde_json::to_value(request_from_command_value(nested.clone()).unwrap().command)
                    .unwrap(),
                nested
            );
        }
        for value in ["x".repeat(1025), "🧭".repeat(513)] {
            let command = json!({"type":"select","tab":3,"ref":"9/doc:1:1","value":value});
            assert_eq!(
                request_from_command_value(command.clone())
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST"
            );
            let nested = json!({"type":"task_run","task":"task-1","revision":2,"command":command});
            assert_eq!(
                request_from_command_value(nested).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        for invalid in [
            json!({"type":"select","tab":0,"ref":"9/doc:1:1","value":"a"}),
            json!({"type":"select","tab":3,"ref":"9/doc:1:1","value":["a"]}),
            json!({"type":"select","tab":3,"ref":"9/doc:1:1","value":"a","index":1}),
        ] {
            assert!(request_from_command_value(invalid).is_err());
        }
    }

    #[test]
    fn space_commands_round_trip_and_reject_unknown_fields() {
        for (command, value) in [
            (Command::Spaces, json!({"type":"spaces"})),
            (Command::SpaceOverview, json!({"type":"space_overview"})),
            (
                Command::SpaceShow {
                    space: "user".into(),
                },
                json!({"type":"space_show","space":"user"}),
            ),
            (
                Command::SpaceShow {
                    space: "task-Abc_123".into(),
                },
                json!({"type":"space_show","space":"task-Abc_123"}),
            ),
        ] {
            assert!(!command.is_task_command());
            assert_eq!(command.tab_id(), None);
            assert_eq!(serde_json::to_value(&command).unwrap(), value);
            assert_eq!(
                request_from_command_value(value.clone()).unwrap().command,
                command
            );
            // UI permissions are enforced by the browser after transport validation.
            let scoped = json!({"type":"task_run","task":"task-1","revision":1,"command":value});
            assert_eq!(
                request_from_command_value(scoped.clone()).unwrap().command,
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 1,
                    command: Box::new(command)
                }
            );
            let mut extra = value;
            extra["unexpected"] = json!(true);
            assert_eq!(
                request_from_command_value(extra.clone()).unwrap_err().code,
                "INVALID_REQUEST"
            );
            let mut scoped_extra = scoped;
            scoped_extra["command"] = extra;
            assert_eq!(
                request_from_command_value(scoped_extra).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn space_socket_validation_enforces_identifier_bounds() {
        assert!(
            request_from_command_value(json!({"type":"space_show","space":"a".repeat(120)}))
                .is_ok()
        );
        for space in [
            "",
            " ",
            "user\n",
            "task\t1",
            "task/1",
            "task id",
            "한글",
            &"a".repeat(121),
        ] {
            let command = Command::SpaceShow {
                space: space.into(),
            };
            assert_eq!(command.validate().unwrap_err().code, "INVALID_REQUEST");
            assert_eq!(
                request_from_command_value(json!({"type":"space_show","space":space}))
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST"
            );
        }
        for value in [
            json!({"type":"space_show"}),
            json!({"type":"space_show","space":null}),
            json!({"type":"space_show","space":1}),
        ] {
            assert_eq!(
                request_from_command_value(value).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn task_wait_validates_timeout_and_task_id_and_cannot_nest() {
        let wait = |task: &str, timeout_ms| Command::TaskWait {
            task: task.into(),
            timeout_ms,
            since: None,
        };
        let since = |value| Command::TaskWait {
            task: "task-1".into(),
            timeout_ms: 10,
            since: Some(value),
        };
        assert!(since(1).validate().is_ok());
        assert!(since(u64::MAX).validate().is_ok());
        assert_eq!(since(0).validate().unwrap_err().code, "INVALID_REQUEST");
        assert_eq!(
            serde_json::to_value(since(3)).unwrap(),
            json!({"type":"task_wait","task":"task-1","timeout_ms":10,"since":3})
        );
        assert_eq!(
            serde_json::from_value::<Command>(
                json!({"type":"task_wait","task":"task-1","timeout_ms":10})
            )
            .unwrap(),
            wait("task-1", 10)
        );
        assert!(wait("task-1", 1).validate().is_ok());
        assert!(wait("task-1", 30000).validate().is_ok());
        for invalid in [wait("task-1", 0), wait("task-1", 30001), wait("bad id", 10)] {
            assert_eq!(invalid.validate().unwrap_err().code, "INVALID_REQUEST");
        }
        let nested = Command::TaskRun {
            task: "task-1".into(),
            revision: 1,
            command: Box::new(wait("task-1", 10)),
        };
        assert_eq!(nested.validate().unwrap_err().code, "INVALID_REQUEST");
    }

    #[test]
    fn task_commands_and_scope_round_trip() {
        let task = "task-abc_123";
        for (command, value) in [
            (
                Command::TaskCreate {
                    name: "검색 작업".into(),
                },
                json!({"type":"task_create","name":"검색 작업"}),
            ),
            (Command::TaskList, json!({"type":"task_list"})),
            (
                Command::TaskInfo { task: task.into() },
                json!({"type":"task_info","task":task}),
            ),
            (
                Command::TaskHandoff {
                    task: task.into(),
                    revision: 1,
                },
                json!({"type":"task_handoff","task":task,"revision":1}),
            ),
            (
                Command::TaskResume {
                    task: task.into(),
                    revision: 2,
                },
                json!({"type":"task_resume","task":task,"revision":2}),
            ),
            (
                Command::TaskWait {
                    task: task.into(),
                    timeout_ms: 2500,
                    since: None,
                },
                json!({"type":"task_wait","task":task,"timeout_ms":2500}),
            ),
            (
                Command::TaskAdopt {
                    task: task.into(),
                    revision: 3,
                    tab: 4,
                },
                json!({"type":"task_adopt","task":task,"revision":3,"tab":4}),
            ),
            (
                Command::TaskRelease {
                    task: task.into(),
                    revision: 4,
                    tab: 5,
                },
                json!({"type":"task_release","task":task,"revision":4,"tab":5}),
            ),
            (
                Command::TaskFinish {
                    task: task.into(),
                    revision: 5,
                    keep: vec![2, 4],
                },
                json!({"type":"task_finish","task":task,"revision":5,"keep":[2,4]}),
            ),
            (
                Command::TaskRun {
                    task: task.into(),
                    revision: 6,
                    command: Box::new(Command::Read { tab: 3 }),
                },
                json!({"type":"task_run","task":task,"revision":6,"command":{"type":"read","tab":3}}),
            ),
        ] {
            assert!(command.is_task_command());
            assert_eq!(serde_json::to_value(&command).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<Command>(value.clone()).unwrap(),
                command
            );
            assert_eq!(
                request_from_command_value(value.clone()).unwrap().command,
                command
            );
            let mut extra_field = value;
            extra_field["unexpected"] = json!(true);
            assert_eq!(
                request_from_command_value(extra_field).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        let scope = TaskScope {
            task: task.into(),
            revision: u64::MAX,
        };
        let serialized = json!({"task":task,"revision":u64::MAX});
        assert_eq!(serde_json::to_value(&scope).unwrap(), serialized);
        assert_eq!(
            serde_json::from_value::<TaskScope>(serialized).unwrap(),
            scope
        );
        assert!(
            serde_json::from_value::<TaskScope>(json!({"task":task,"revision":1,"extra":true}))
                .is_err()
        );
    }

    #[test]
    fn task_socket_validation_trims_names_and_enforces_parameter_bounds() {
        let name = "한".repeat(120);
        let request =
            request_from_command_value(json!({"type":"task_create","name":format!("  {name}  ")}))
                .unwrap();
        assert_eq!(request.command, Command::TaskCreate { name });
        let task = "a".repeat(128);
        assert!(request_from_command_value(json!({"type":"task_info","task":task})).is_ok());
        for command in [
            json!({"type":"task_create","name":"   "}),
            json!({"type":"task_create","name":"name\n"}),
            json!({"type":"task_create","name":"한".repeat(121)}),
            json!({"type":"task_info","task":""}),
            json!({"type":"task_info","task":"a".repeat(129)}),
            json!({"type":"task_info","task":"한글"}),
            json!({"type":"task_info","task":"task/1"}),
            json!({"type":"task_adopt","task":"task-1","revision":1,"tab":0}),
            json!({"type":"task_release","task":"task-1","revision":1,"tab":-1}),
            json!({"type":"task_finish","task":"task-1","revision":1,"keep":[0]}),
            json!({"type":"task_finish","task":"task-1","revision":1,"keep":[1,1]}),
            json!({"type":"task_finish","task":"task-1","revision":1,"keep":[1.5]}),
            json!({"type":"task_finish","task":"task-1","revision":1}),
        ] {
            assert_eq!(
                request_from_command_value(command).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        for kind in [
            "task_handoff",
            "task_resume",
            "task_adopt",
            "task_release",
            "task_finish",
            "task_run",
        ] {
            let mut command = json!({"type":kind,"task":"task-1","revision":0});
            if matches!(kind, "task_adopt" | "task_release") {
                command["tab"] = json!(1);
            }
            if kind == "task_finish" {
                command["keep"] = json!([]);
            }
            if kind == "task_run" {
                command["command"] = json!({"type":"tabs"});
            }
            assert_eq!(
                request_from_command_value(command).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn task_run_socket_validation_rejects_nested_and_invalid_inner_commands() {
        for inner in [
            json!({"type":"task_create","name":"nested"}),
            json!({"type":"task_list"}),
            json!({"type":"task_run","task":"task-2","revision":1,"command":{"type":"tabs"}}),
            json!({"type":"tabs","extra":true}),
            json!({"type":"status","extra":true}),
            json!({"type":"read","tab":0}),
            json!({"type":"wait","tab":1,"until":"dom","timeout_ms":0}),
            json!({"type":"scroll","tab":1,"x":0,"y":10001}),
            json!({"type":"read","tab":1,"extra":true}),
            json!({"type":"frames","tab":0}),
            json!({"type":"frames","tab":3,"session":"untrusted"}),
            json!({"type":"read_page","tab":0}),
        ] {
            let command = json!({"type":"task_run","task":"task-1","revision":1,"command":inner});
            assert_eq!(
                request_from_command_value(command).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        let read_page = json!({"type":"task_run","task":"task-1","revision":1,"command":{"type":"read_page","tab":3}});
        assert!(request_from_command_value(read_page).is_ok());
        // Programmatic callers cannot bypass the same nested-command restriction.
        assert_eq!(
            Command::TaskRun {
                task: "task-1".into(),
                revision: 1,
                command: Box::new(Command::TaskList),
            }
            .validate()
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
    }

    #[test]
    fn task_helpers_identify_tab_targets_without_claiming_multi_tab_commands() {
        for inner in [
            Command::Read { tab: 3 },
            Command::Frames { tab: 3 },
            Command::ReadFrame {
                tab: 3,
                frame: "frame-A1".into(),
            },
            Command::ReadFrame {
                tab: 3,
                frame: "F".repeat(256),
            },
        ] {
            assert!(!inner.is_task_command());
            assert_eq!(inner.tab_id(), Some(3));
            assert_eq!(
                request_from_command_value(serde_json::to_value(&inner).unwrap())
                    .unwrap()
                    .command,
                inner,
            );
            let run = Command::TaskRun {
                task: "task-1".into(),
                revision: 1,
                command: Box::new(inner),
            };
            assert!(run.is_task_command());
            assert_eq!(run.tab_id(), Some(3));
            let wire = serde_json::to_value(&run).unwrap();
            assert_eq!(request_from_command_value(wire).unwrap().command, run);
        }
        assert_eq!(
            Command::TaskAdopt {
                task: "task-1".into(),
                revision: 1,
                tab: 4
            }
            .tab_id(),
            Some(4)
        );
        assert_eq!(
            Command::TaskRelease {
                task: "task-1".into(),
                revision: 1,
                tab: 5
            }
            .tab_id(),
            Some(5)
        );
        assert_eq!(
            Command::TaskFinish {
                task: "task-1".into(),
                revision: 1,
                keep: vec![3]
            }
            .tab_id(),
            None
        );
        assert_eq!(
            Command::Open {
                url: "https://example.test".into()
            }
            .tab_id(),
            None
        );
        assert_eq!(Command::Tabs.tab_id(), None);
        assert_eq!(Command::Status.tab_id(), None);
    }

    #[test]
    fn scroll_element_wire_round_trips_refs_axes_and_scope() {
        for reference in ["9/doc:1:1".to_owned(), "r".repeat(256), "가".repeat(85)] {
            for (x, y) in [(-10000, 10000), (10000, -10000), (0, 0)] {
                let command = Command::ScrollElement {
                    tab: 3,
                    reference: reference.clone(),
                    x,
                    y,
                };
                let wire = json!({"type":"scroll_element","tab":3,"ref":reference,"x":x,"y":y});
                assert_eq!(serde_json::to_value(&command).unwrap(), wire);
                assert_eq!(
                    request_from_command_value(wire.clone()).unwrap().command,
                    command
                );
                assert_eq!(command.tab_id(), Some(3));
                assert!(!command.is_task_command());
                let nested = Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(command),
                };
                assert_eq!(nested.tab_id(), Some(3));
                assert_eq!(
                    request_from_command_value(json!({
                        "type":"task_run","task":"task-1","revision":2,"command":wire
                    }))
                    .unwrap()
                    .command,
                    nested
                );
            }
        }
    }

    #[test]
    fn scroll_element_wire_rejects_invalid_refs_axes_and_fields_in_each_scope() {
        let base = json!({"type":"scroll_element","tab":3,"ref":"9/doc:1:1","x":0,"y":0});
        let mut invalid = Vec::new();
        for reference in [
            json!(""),
            json!("r".repeat(257)),
            json!("가".repeat(86)),
            Value::Null,
            json!(3),
        ] {
            let mut command = base.clone();
            command["ref"] = reference;
            invalid.push(command);
        }
        for delta in [
            json!(-10001),
            json!(10001),
            json!(2147483648_i64),
            json!(1.5),
            json!("1"),
            Value::Null,
        ] {
            for axis in ["x", "y"] {
                let mut command = base.clone();
                command[axis] = delta.clone();
                invalid.push(command);
            }
        }
        for field in ["tab", "ref", "x", "y"] {
            let mut command = base.clone();
            command.as_object_mut().unwrap().remove(field);
            invalid.push(command);
        }
        let mut extra = base.clone();
        extra["frame"] = json!("untrusted-frame");
        invalid.push(extra);
        let mut invalid_tab = base.clone();
        invalid_tab["tab"] = json!(0);
        invalid.push(invalid_tab);
        for command in invalid {
            assert_eq!(
                request_from_command_value(command.clone())
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST",
                "{command}"
            );
            assert_eq!(
                request_from_command_value(json!({
                    "type":"task_run","task":"task-1","revision":2,"command":command
                }))
                .unwrap_err()
                .code,
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn scroll_socket_validation_accepts_signed_bounds_and_rejects_invalid_deltas() {
        for (x, y) in [(-10000, 10000), (10000, -10000), (0, 0)] {
            assert_eq!(
                request_from_command_value(json!({"type":"scroll","tab":3,"x":x,"y":y}))
                    .unwrap()
                    .command,
                Command::Scroll { tab: 3, x, y },
            );
        }
        for invalid in [
            json!(-10001),
            json!(10001),
            json!(i32::MIN),
            json!(i32::MAX),
            json!(1.5),
            json!("100"),
            Value::Null,
        ] {
            for axis in ["x", "y"] {
                let mut command = json!({"type":"scroll","tab":3,"x":0,"y":0});
                command[axis] = invalid.clone();
                assert_eq!(
                    request_from_command_value(command).unwrap_err().code,
                    "INVALID_REQUEST",
                    "{axis}={invalid}",
                );
            }
        }
    }

    #[test]
    fn navigation_socket_validation_rejects_missing_and_extra_fields() {
        assert_eq!(
            request_from_command_value(json!({"type":"panel"}))
                .unwrap()
                .command,
            Command::Panel
        );
        for command in [
            json!({"type":"panel","extra":true}),
            json!({"type":"navigate","tab":3}),
            json!({"type":"navigate","tab":3,"url":"https://example.test","extra":true}),
            json!({"type":"navigate","tab":"3","url":"https://example.test"}),
            json!({"type":"scroll","tab":3,"x":0}),
            json!({"type":"scroll","tab":3,"x":0,"y":0,"extra":true}),
        ] {
            assert_eq!(
                request_from_command_value(command).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
        for name in ["back", "forward", "reload", "stop", "frames"] {
            for command in [
                json!({"type":name}),
                json!({"type":name,"tab":3,"url":"https://example.test"}),
                json!({"type":name,"tab":"3"}),
                json!({"type":name,"tab":0}),
                json!({"type":name,"tab":2147483648_i64}),
            ] {
                assert_eq!(
                    request_from_command_value(command).unwrap_err().code,
                    "INVALID_REQUEST"
                );
            }
        }
        let mut invalid_frames = vec![
            json!({"type":"read_frame","tab":3}),
            json!({"type":"read_frame","frame":"frame-A1"}),
            json!({"type":"read_frame","tab":0,"frame":"frame-A1"}),
            json!({"type":"read_frame","tab":"3","frame":"frame-A1"}),
            json!({"type":"read_frame","tab":2147483648_i64,"frame":"frame-A1"}),
            json!({"type":"read_frame","tab":3,"frame":null}),
            json!({"type":"read_frame","tab":3,"frame":42}),
            json!({"type":"read_frame","tab":3,"frame":"frame-A1","extra":true}),
            json!({"type":"read_frame","tab":3,"frame":"frame-A1","session":"untrusted"}),
            json!({"type":"read_frame","tab":3,"frame":"frame-A1","sessionId":"untrusted"}),
        ];
        for frame in [
            "".to_owned(),
            "프레임".into(),
            "frame\n1".into(),
            "frame\u{7f}".into(),
            "F".repeat(257),
        ] {
            invalid_frames.push(json!({"type":"read_frame","tab":3,"frame":frame}));
        }
        for inner in invalid_frames {
            for command in [
                inner.clone(),
                json!({"type":"task_run","task":"task-1","revision":1,"command":inner}),
            ] {
                assert_eq!(
                    request_from_command_value(command.clone())
                        .unwrap_err()
                        .code,
                    "INVALID_REQUEST",
                    "{command}",
                );
            }
        }
    }

    #[test]
    fn rejects_out_of_range_wait_timeout_from_socket() {
        for timeout_ms in [0, 30001, u64::MAX] {
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            let mut frame = serde_json::to_vec(&Request {
                id: "wait-bounds".into(),
                command: Command::Wait {
                    tab: 1,
                    until: WaitUntil::Dom,
                    timeout_ms,
                },
            })
            .unwrap();
            frame.push(b'\n');
            writer.write_all(&frame).unwrap();
            assert_eq!(
                read_request(&mut reader).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn round_trip_preserves_request_id_and_null_result() {
        let directory = TestDirectory::new();
        let socket = directory.socket();
        let listener = bind_listener(&socket).unwrap();
        assert_eq!(fs::metadata(&socket).unwrap().mode() & 0o777, 0o600);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).unwrap();
            assert_eq!(request.command, Command::Status);
            write_response(&mut stream, &Response::success(request.id, Value::Null)).unwrap();
        });
        let response = call(
            &socket,
            &Request {
                id: "check".into(),
                command: Command::Status,
            },
        )
        .unwrap();
        assert_eq!(response.id, "check");
        assert!(response.ok);
        assert_eq!(response.result, Some(Value::Null));
        server.join().unwrap();
    }

    #[test]
    fn rejects_mismatched_response_and_marks_unknown_outcome() {
        let directory = TestDirectory::new();
        let socket = directory.socket();
        let listener = bind_listener(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream).unwrap();
            write_response(&mut stream, &Response::success("other", json!({}))).unwrap();
        });
        let error = call(
            &socket,
            &Request {
                id: "check".into(),
                command: Command::Status,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "RESPONSE_ID_MISMATCH");
        assert!(error.outcome_unknown);
        server.join().unwrap();
    }

    #[test]
    fn surfaces_server_rejection_of_invalid_request_as_known_outcome() {
        let directory = TestDirectory::new();
        let socket = directory.socket();
        let listener = bind_listener(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream).unwrap();
            write_response(
                &mut stream,
                &Response::error(INVALID_REQUEST_ID, "INVALID_REQUEST", "bad frame"),
            )
            .unwrap();
        });
        let error = call(
            &socket,
            &Request {
                id: "check".into(),
                command: Command::Status,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "INVALID_REQUEST");
        assert_eq!(error.message, "bad frame");
        assert!(!error.outcome_unknown);
        server.join().unwrap();
    }

    #[test]
    fn rejects_oversized_request_before_json_parse() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let sender = thread::spawn(move || {
            let _ = writer.write_all(&vec![b'x'; MAX_REQUEST_BYTES + 2]);
        });
        let error = read_request(&mut reader).unwrap_err();
        assert_eq!(error.code, "FRAME_TOO_LARGE", "{error}");
        drop(reader);
        sender.join().unwrap();
    }

    #[test]
    fn frame_limit_excludes_newline_and_incomplete_peer_times_out() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"abc\n").unwrap();
        assert_eq!(
            read_frame(&mut reader, 3, Duration::from_secs(1)).unwrap(),
            b"abc"
        );

        for text in ["a", "\n", "가"] {
            let expected = serde_json::to_vec(text).unwrap();
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            write_frame(&mut writer, &text, expected.len()).unwrap();
            assert_eq!(
                read_frame(&mut reader, expected.len(), Duration::from_secs(1)).unwrap(),
                expected
            );
        }

        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"a").unwrap();
        assert_eq!(
            read_frame(&mut reader, 3, Duration::from_millis(20))
                .unwrap_err()
                .code,
            "TIMEOUT"
        );
    }

    #[test]
    fn rejects_incomplete_line_and_unknown_command_fields() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer
            .write_all(b"{\"id\":\"x\",\"command\":{\"type\":\"status\"}}")
            .unwrap();
        drop(writer);
        let error = read_request(&mut reader).unwrap_err();
        assert_eq!(error.code, "INCOMPLETE_FRAME", "{error}");

        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer
            .write_all(b"{\"id\":\"x\",\"command\":{\"type\":\"status\",\"script\":\"ignored\"}}\n")
            .unwrap();
        assert_eq!(
            read_request(&mut reader).unwrap_err().code,
            "INVALID_REQUEST"
        );
    }

    #[test]
    fn rejects_oversized_response_without_writing() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        let response = Response::success("x", Value::String("x".repeat(MAX_RESPONSE_BYTES)));
        assert_eq!(
            write_response(&mut writer, &response).unwrap_err().code,
            "FRAME_TOO_LARGE"
        );
        reader.set_nonblocking(true).unwrap();
        assert_eq!(
            reader.read(&mut [0u8; 1]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn generated_json_string_stops_at_the_buffer_limit_without_capacity_overshoot() {
        struct GeneratedString(usize);
        impl fmt::Display for GeneratedString {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                const BLOCK: &str =
                    "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
                for _ in 0..self.0 / BLOCK.len() {
                    formatter.write_str(BLOCK)?;
                }
                formatter.write_str(&BLOCK[..self.0 % BLOCK.len()])
            }
        }
        impl Serialize for GeneratedString {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        // Include non-power-of-two limits so implicit Vec growth would overshoot.
        for limit in [31, 4099, MAX_REQUEST_BYTES + 3, MAX_RESPONSE_BYTES] {
            let mut frame = BoundedFrame::new(limit);
            let error =
                serde_json::to_writer(&mut frame, &GeneratedString(limit + 32)).unwrap_err();
            assert!(error.is_io());
            assert!(frame.exceeded);
            assert!(frame.bytes.len() <= limit);
            assert!(frame.bytes.capacity() <= limit);
        }

        let mut small = BoundedFrame::new(MAX_RESPONSE_BYTES);
        serde_json::to_writer(&mut small, &json!({"ok":true})).unwrap();
        assert_eq!(small.bytes, br#"{"ok":true}"#);
        assert!(small.bytes.capacity() <= 4096);
    }

    #[test]
    fn failed_serializer_preserves_invalid_json_error_and_writes_nothing() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom(
                    "intentional serialization failure",
                ))
            }
        }
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        let error = write_frame(&mut writer, &(1, Invalid), MAX_RESPONSE_BYTES).unwrap_err();
        assert_eq!(error.code, "INVALID_JSON");
        assert!(error.message.contains("intentional serialization failure"));
        reader.set_nonblocking(true).unwrap();
        assert_eq!(
            reader.read(&mut [0u8; 1]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn preserves_live_socket_and_recovers_stale_socket() {
        let directory = TestDirectory::new();
        let socket = directory.socket();
        let listener = bind_listener(&socket).unwrap();
        assert_eq!(bind_listener(&socket).unwrap_err().code, "SOCKET_IN_USE");
        drop(listener);
        let recovered = bind_listener(&socket).unwrap();
        drop(recovered);
        let error = call(
            &socket,
            &Request {
                id: "x".into(),
                command: Command::Tabs,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "APP_NOT_RUNNING");
        assert!(!error.outcome_unknown);
    }

    #[test]
    fn refuses_insecure_directory_and_existing_regular_file() {
        let directory = TestDirectory::new();
        let socket = directory.socket();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            bind_listener(&socket).unwrap_err().code,
            "INSECURE_SOCKET_DIRECTORY"
        );
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&socket, "preserve me").unwrap();
        assert_eq!(bind_listener(&socket).unwrap_err().code, "INSECURE_SOCKET");
        assert_eq!(fs::read_to_string(&socket).unwrap(), "preserve me");
    }

    #[test]
    fn pointer_expansion_commands_round_trip_in_both_scopes() {
        let wires = [
            json!({"type":"dblclick","tab":3,"ref":"9/doc:1:1"}),
            json!({"type":"right_click","tab":3,"ref":"text=Save"}),
            json!({"type":"wheel","tab":3,"ref":"css=#list","dx":-10000,"dy":10000}),
            json!({"type":"drag","tab":3,"from":"9/doc:1:1","to":"9/doc:1:2"}),
            json!({"type":"drag","tab":3,"from":"css=#a","to":"role=button[name=Drop]"}),
            json!({"type":"click_at","tab":3,"x":0,"y":1000000,"button":"right","count":3}),
            json!({"type":"move_at","tab":3,"x":12,"y":34}),
        ];
        for wire in wires {
            let command = request_from_command_value(wire.clone()).unwrap().command;
            assert_eq!(command.tab_id(), Some(3));
            assert!(!command.is_task_command());
            assert_eq!(serde_json::to_value(&command).unwrap(), wire);
            let nested = request_from_command_value(json!({
                "type":"task_run","task":"task-1","revision":2,"command":wire
            }))
            .unwrap()
            .command;
            assert_eq!(nested.tab_id(), Some(3));
        }
        // click_at defaults: left button, single click.
        let command = request_from_command_value(json!({"type":"click_at","tab":3,"x":1,"y":2}))
            .unwrap()
            .command;
        assert_eq!(
            command,
            Command::ClickAt {
                tab: 3,
                x: 1,
                y: 2,
                button: MouseButton::Left,
                count: 1
            }
        );
    }

    #[test]
    fn pointer_expansion_commands_reject_invalid_fields() {
        let invalid = [
            json!({"type":"dblclick","tab":0,"ref":"r"}),
            json!({"type":"dblclick","tab":3,"ref":""}),
            json!({"type":"dblclick","tab":3,"ref":"r".repeat(257)}),
            json!({"type":"right_click","tab":3,"ref":"text="}),
            json!({"type":"right_click","tab":3,"ref":"css=a\u{0}b"}),
            json!({"type":"right_click","tab":3,"ref":format!("css={}", "a".repeat(1022))}),
            json!({"type":"wheel","tab":3,"ref":"r","dx":0,"dy":0}),
            json!({"type":"wheel","tab":3,"ref":"r","dx":10001,"dy":0}),
            json!({"type":"wheel","tab":3,"ref":"r","dx":0,"dy":-10001}),
            json!({"type":"wheel","tab":3,"ref":"r","dx":1.5,"dy":1}),
            json!({"type":"wheel","tab":3,"ref":"r","dx":1}),
            json!({"type":"drag","tab":3,"from":"r","to":"r"}),
            json!({"type":"drag","tab":3,"from":"css=a","to":"9/doc:1:2"}),
            json!({"type":"drag","tab":3,"from":"9/doc:1:2","to":"css=a"}),
            json!({"type":"drag","tab":3,"from":"","to":"r"}),
            json!({"type":"drag","tab":3,"from":"r"}),
            json!({"type":"click_at","tab":3,"x":-1,"y":0}),
            json!({"type":"click_at","tab":3,"x":0,"y":1000001}),
            json!({"type":"click_at","tab":3,"x":1.5,"y":0}),
            json!({"type":"click_at","tab":3,"x":1,"y":2,"count":0}),
            json!({"type":"click_at","tab":3,"x":1,"y":2,"count":4}),
            json!({"type":"click_at","tab":3,"x":1,"y":2,"button":"middle"}),
            json!({"type":"click_at","tab":3,"x":1,"y":2,"extra":true}),
            json!({"type":"move_at","tab":3,"x":1}),
            json!({"type":"move_at","tab":3,"x":1,"y":2,"button":"left"}),
        ];
        for command in invalid {
            assert_eq!(
                request_from_command_value(command.clone())
                    .unwrap_err()
                    .code,
                "INVALID_REQUEST",
                "{command}"
            );
        }
    }

    #[test]
    fn selectors_are_accepted_wherever_a_ref_is_and_bounded() {
        let long_ok = format!("css={}", "a".repeat(MAX_SELECTOR_BYTES - 4));
        let long_bad = format!("css={}", "a".repeat(MAX_SELECTOR_BYTES - 3));
        let multibyte_ok = format!("text={}", "가".repeat(339)); // 5 + 1017 bytes
        let multibyte_bad = format!("text={}", "가".repeat(342)); // 5 + 1026 bytes
        for selector in [
            "text=Sign in",
            "css=button.primary",
            "role=button[name=OK]",
            &long_ok,
            &multibyte_ok,
        ] {
            for wire in [
                json!({"type":"click","tab":3,"ref":selector}),
                json!({"type":"hover","tab":3,"ref":selector}),
                json!({"type":"fill","tab":3,"ref":selector,"text":"x"}),
                json!({"type":"insert","tab":3,"ref":selector,"text":"x"}),
                json!({"type":"press","tab":3,"ref":selector,"key":"Enter"}),
                json!({"type":"check","tab":3,"ref":selector,"checked":true}),
                json!({"type":"select","tab":3,"ref":selector,"value":"a"}),
                json!({"type":"scroll_element","tab":3,"ref":selector,"x":0,"y":5}),
            ] {
                assert!(request_from_command_value(wire.clone()).is_ok(), "{wire}");
            }
        }
        for selector in [&long_bad, &multibyte_bad, "css=", "role=", "text="] {
            for wire in [
                json!({"type":"click","tab":3,"ref":selector}),
                json!({"type":"fill","tab":3,"ref":selector,"text":"x"}),
                json!({"type":"scroll_element","tab":3,"ref":selector,"x":0,"y":5}),
            ] {
                assert_eq!(
                    request_from_command_value(wire.clone()).unwrap_err().code,
                    "INVALID_REQUEST",
                    "{wire}"
                );
            }
        }
        // Refs keep their own 256-byte limit even when they look like a selector's neighbors.
        assert!(is_selector("text=a") && is_selector("css=a") && is_selector("role=a"));
        assert!(
            !is_selector("9/doc:1:1")
                && !is_selector("frame/1/2/doc:1:1")
                && !is_selector("Text=a")
        );
        assert!(
            request_from_command_value(json!({"type":"click","tab":3,"ref":"r".repeat(257)}))
                .is_err()
        );
    }
}
