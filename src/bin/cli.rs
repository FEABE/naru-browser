fn main() {
    let socket = match light_browser::config::socket_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    std::process::exit(light_browser::cli::run(&args, &socket));
}
