use cef::{args::Args, *};
use light_browser::{browser, config, mac, protocol};
use std::{
    os::unix::fs::DirBuilderExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    config::mock_keychain()?;
    let data_dir = config::data_dir()?;
    let socket_path = config::socket_path()?;
    let listener = protocol::bind_listener(&socket_path)?;
    // The original prototype used Chromium's shared Keychain identity. Keep
    // that profile intact; its encrypted data cannot use our app-specific key.
    let profile = data_dir.join("profile-v1");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&profile)?;
    // CEF opens this file in sandboxed helpers as well. Keep application logs
    // in the standard macOS location, independently of a user-selected profile.
    let log_file = config::log_file()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(log_file.parent().expect("Application log directory"))?;

    let loader = library_loader::LibraryLoader::new(&std::env::current_exe()?, false);
    if !loader.load() {
        return Err(
            "Launch the bundled app using scripts/dev.sh; the CEF framework is required.".into(),
        );
    }
    if !light_browser::initialize_cef_api() {
        return Err("CEF runtime does not match the bundled API 15600 bindings.".into());
    }
    mac::setup();
    let args = Args::new();
    let result = execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
        std::ptr::null_mut(),
    );
    if result >= 0 {
        return Ok(());
    }

    let settings = Settings {
        no_sandbox: 0,
        command_line_args_disabled: 1,
        root_cache_path: profile.to_string_lossy().as_ref().into(),
        cache_path: profile.join("default").to_string_lossy().as_ref().into(),
        persist_session_cookies: 1,
        keychain_service_name: "Light Browser Safe Storage".into(),
        keychain_account_name: "local.lightbrowser.dev".into(),
        log_file: log_file.to_string_lossy().as_ref().into(),
        log_severity: LogSeverity::WARNING,
        remote_debugging_port: 0,
        ..Default::default()
    };
    let mut app = browser::BrowserApp::new();
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err("CEF initialization failed".into());
    }
    mac::quit_on_signals();
    light_browser::bookmark_runtime::initialize(data_dir.clone());
    light_browser::history_runtime::initialize(data_dir);
    let stopped = Arc::new(AtomicBool::new(false));
    let server = browser::start_server(listener, stopped.clone());
    eprintln!("Light Browser ready: {}", socket_path.display());
    run_message_loop();
    stopped.store(true, Ordering::Release);
    // Wake accept() so the local server can finish before CEF shuts down.
    let _ = std::os::unix::net::UnixStream::connect(&socket_path);
    let _ = server.join();
    light_browser::bookmark_runtime::shutdown();
    light_browser::history_runtime::shutdown();
    light_browser::control_panel::shutdown();
    browser::clear();
    shutdown();
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}
