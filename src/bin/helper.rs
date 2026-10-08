use cef::{args::Args, *};

fn main() {
    let args = Args::new();
    let mut sandbox = cef::sandbox::Sandbox::new();
    sandbox.initialize(args.as_main_args());
    let loader = library_loader::LibraryLoader::new(&std::env::current_exe().unwrap(), true);
    assert!(loader.load(), "CEF framework could not be loaded");
    assert!(light_browser::initialize_cef_api(), "CEF API mismatch");
    let code = execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
        std::ptr::null_mut(),
    );
    if code != 0 {
        std::process::exit(code);
    }
}
