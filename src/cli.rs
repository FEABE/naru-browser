use crate::protocol::{self, Command, Request, Response};
use serde_json::json;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const USAGE: &str = "Usage: lightbrowser <command>\n\nCommands:\n  open URL                 Open an HTTP(S) page in a new tab\n  tabs                     List tabs\n  read TAB                 Read page text and interactive element refs\n  click TAB REF            Click a reference returned by read\n  fill TAB REF TEXT        Replace an input value\n  press TAB REF KEY        Send a key to the referenced element\n  close TAB                Close a tab\n  screenshot TAB PATH      Save a screenshot to PATH\n  status                   Show browser status\n  help                     Show this help\n\nTAB is a positive integer. Quote arguments containing spaces.\nEvery command writes one JSON response to stdout. Errors also use stderr.\n";

/// `args` excludes the executable name. The caller supplies the configured socket.
pub fn run(args: &[String], socket: &Path) -> i32 {
    let id = request_id();
    if args.is_empty() || (args.len() == 1 && matches!(args[0].as_str(), "help" | "--help" | "-h"))
    {
        return emit(&Response::success(id, json!({ "usage": USAGE })), 0);
    }
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return emit(&Response::error(id, "INVALID_ARGUMENTS", message), 2);
        }
    };
    let request = Request {
        id: id.clone(),
        command,
    };
    match protocol::call(socket, &request) {
        Ok(response) => {
            let exit_code = if response.ok { 0 } else { 1 };
            if let Some(error) = &response.error {
                eprintln!("{}: {}", error.code, error.message);
            }
            emit(&response, exit_code)
        }
        Err(error) => {
            eprintln!("{error}");
            let response = Response::error(id, error.code, error.message)
                .with_details(json!({ "outcomeUnknown": error.outcome_unknown }));
            emit(&response, 1)
        }
    }
}

fn emit(response: &Response, exit_code: i32) -> i32 {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = serde_json::to_writer(&mut output, response)
        .map_err(io::Error::other)
        .and_then(|_| output.write_all(b"\n"))
        .and_then(|_| output.flush());
    if let Err(error) = result {
        eprintln!("OUTPUT_ERROR: {error}");
        return 1;
    }
    exit_code
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{timestamp}", std::process::id())
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let values: Vec<&str> = args.iter().map(String::as_str).collect();
    match values.as_slice() {
        ["open", url] => {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("open accepts only an http:// or https:// URL.".into());
            }
            Ok(Command::Open { url: (*url).into() })
        }
        ["tabs"] => Ok(Command::Tabs),
        ["read", tab] => Ok(Command::Read {
            tab: parse_tab(tab)?,
        }),
        ["click", tab, reference] => Ok(Command::Click {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
        }),
        ["fill", tab, reference, text] => Ok(Command::Fill {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            text: (*text).into(),
        }),
        ["press", tab, reference, key] => {
            if key.is_empty() {
                return Err("KEY must not be empty.".into());
            }
            Ok(Command::Press {
                tab: parse_tab(tab)?,
                reference: parse_ref(reference)?,
                key: (*key).into(),
            })
        }
        ["close", tab] => Ok(Command::Close {
            tab: parse_tab(tab)?,
        }),
        ["screenshot", tab, path] => {
            if path.is_empty() {
                return Err("PATH must not be empty.".into());
            }
            // Resolve against the CLI caller's directory, not the GUI app's directory.
            let absolute = if Path::new(path).is_absolute() {
                Path::new(path).to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("Cannot resolve screenshot path: {e}"))?
                    .join(path)
            };
            let path = absolute
                .to_str()
                .ok_or("Screenshot path must be valid UTF-8.")?
                .to_owned();
            Ok(Command::Screenshot {
                tab: parse_tab(tab)?,
                path,
            })
        }
        ["status"] => Ok(Command::Status),
        [command, ..]
            if matches!(
                *command,
                "open"
                    | "tabs"
                    | "read"
                    | "click"
                    | "fill"
                    | "press"
                    | "close"
                    | "screenshot"
                    | "status"
            ) =>
        {
            Err(format!("Wrong number of arguments for {command}."))
        }
        [command, ..] => Err(format!("Unknown command: {command}")),
        [] => Err("A command is required.".into()),
    }
}

fn parse_tab(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .ok()
        .filter(|tab| *tab > 0)
        .ok_or_else(|| "TAB must be a positive 32-bit integer.".into())
}

fn parse_ref(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 256 {
        return Err("REF must contain 1 to 256 bytes.".into());
    }
    Ok(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).into()).collect()
    }

    #[test]
    fn rejects_dangerous_scheme_and_extra_arguments() {
        assert!(parse_command(&args(&["open", "javascript:alert(1)"])).is_err());
        assert!(parse_command(&args(&["tabs", "unexpected"])).is_err());
        assert!(parse_command(&args(&["close", "0"])).is_err());
    }

    #[test]
    fn fill_preserves_empty_text_and_unicode() {
        assert_eq!(
            parse_command(&args(&["fill", "1", "2:a", ""])).unwrap(),
            Command::Fill {
                tab: 1,
                reference: "2:a".into(),
                text: "".into()
            }
        );
        assert_eq!(
            parse_command(&args(&["fill", "1", "2:a", "한글 text"])).unwrap(),
            Command::Fill {
                tab: 1,
                reference: "2:a".into(),
                text: "한글 text".into()
            }
        );
    }
}
