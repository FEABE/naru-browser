//! One JSON request and one JSON response per local socket connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;
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
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Open {
        url: String,
    },
    Tabs,
    Read {
        tab: i32,
    },
    Click {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
    },
    Fill {
        tab: i32,
        #[serde(rename = "ref")]
        reference: String,
        text: String,
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
    Screenshot {
        tab: i32,
        path: String,
    },
    Status,
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
    pub code: &'static str,
    pub message: String,
    /// A failed response does not establish whether the browser performed the action.
    pub outcome_unknown: bool,
}

impl ProtocolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
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

fn write_frame<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    limit: usize,
) -> Result<(), ProtocolError> {
    let mut frame =
        serde_json::to_vec(value).map_err(|e| ProtocolError::new("INVALID_JSON", e.to_string()))?;
    if frame.len() > limit {
        return Err(ProtocolError::new(
            "FRAME_TOO_LARGE",
            "JSON frame exceeds the size limit.",
        ));
    }
    frame.push(b'\n');
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut pending = frame.as_slice();
    stream
        .set_nonblocking(true)
        .map_err(|e| io_error("set frame write mode", e))?;
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
    Ok(())
}

pub fn read_request(stream: &mut UnixStream) -> Result<Request, ProtocolError> {
    verify_peer_uid(stream)?;
    let frame = read_frame(stream, MAX_REQUEST_BYTES, REQUEST_READ_TIMEOUT)?;
    let value: Value = serde_json::from_slice(&frame)
        .map_err(|e| ProtocolError::new("INVALID_REQUEST", e.to_string()))?;
    // Serde ignores extra fields on internally tagged unit variants, even with deny_unknown_fields.
    if let Some(command) = value.get("command").and_then(Value::as_object) {
        if matches!(
            command.get("type").and_then(Value::as_str),
            Some("tabs" | "status")
        ) && command.len() != 1
        {
            return Err(ProtocolError::new(
                "INVALID_REQUEST",
                "This command does not accept additional fields.",
            ));
        }
    }
    let request: Request = serde_json::from_value(value)
        .map_err(|e| ProtocolError::new("INVALID_REQUEST", e.to_string()))?;
    if request.id.is_empty() || request.id.len() > 128 {
        return Err(ProtocolError::new(
            "INVALID_REQUEST",
            "Request id must contain 1 to 128 bytes.",
        ));
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
        let (mut writer, _reader) = UnixStream::pair().unwrap();
        let response = Response::success("x", Value::String("x".repeat(MAX_RESPONSE_BYTES)));
        assert_eq!(
            write_response(&mut writer, &response).unwrap_err().code,
            "FRAME_TOO_LARGE"
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
}
