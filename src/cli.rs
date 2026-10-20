use crate::keyboard::{KeyPress, UNSUPPORTED_KEY_MESSAGE};
use crate::protocol::{self, Command, Request, Response, WaitUntil};
use serde_json::json;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const USAGE: &str = "Usage: lightbrowser <command>\n\nCommands:\n  open URL                 Return a created tab immediately; navigation may still be loading\n  navigate TAB URL         Request navigation in an existing tab\n  back TAB                 Request backward navigation\n  forward TAB              Request forward navigation\n  reload TAB               Request a reload of the current page\n  stop TAB                 Request loading to stop; does not wait for load completion\n  tabs                     List tabs\n  wait TAB [dom|load] [TIMEOUT_MS]\n                           Wait for readiness (defaults: dom, 10000 ms; range: 1..30000)\n  read TAB                 Read page text and interactive element refs\n  read-page TAB            Read discovered frame documents under a shared budget\n  frames TAB               Inspect the tab frame tree, including discovered child sessions\n                           Inventory only; does not read or control child-frame content\n  read-frame TAB FRAME     Read one current frame, including cross-origin content\n                           Refs support click, hover, fill, insert, press, check, select and targeted scroll\n  scroll TAB DX DY          Scroll the page by relative integer CSS pixels (-10000..10000)\n  scroll TAB REF DX DY      Scroll a referenced list or document by the same CSS pixel deltas\n                           Use a scrollable ref from read, read-frame or read-page\n  click TAB REF            Dispatch a Chromium pointer click to a reference\n  hover TAB REF            Move the Chromium pointer to a reference without clicking\n  check TAB REF true|false Set a native checkbox/radio checked state\n  fill TAB REF TEXT        Replace an input, textarea or editable host with text\n  insert TAB REF TEXT      Insert text at the existing caret or selection\n  select TAB REF VALUE     Select one native HTML option by exact value\n  press TAB REF KEY        Dispatch a Chromium key pair from a focused reference\n  close TAB                Close a tab\n  screenshot TAB PATH      Save a screenshot to PATH\n  status                   Show browser status\n  panel                    Bring the browser window forward (compatibility alias)\n  help                     Show this help\n\nSpaces:\n  spaces                   List user and task spaces (also available in task run)\n  space show user|TASK_ID  Show a space without handing off control\n  space overview           Show the space overview\n                           show and overview are global UI controls; task run rejects them\n\nTasks:\n  task create NAME         Create a named task\n  task list                List tasks\n  task info ID             Inspect a task and its current revision\n  task handoff ID REV       Hand control to the user\n  task resume ID REV        Resume agent control\n  task adopt ID REV TAB     Attach an existing tab to a task\n  task release ID REV TAB   Release a tab from a task\n  task finish ID REV [--keep TAB ...]\n                           Finish a task, retaining only the listed tabs\n  task run ID REV COMMAND  Run an existing browser command in a task\n\nKeys:\n  Enter Tab Escape ArrowLeft ArrowUp ArrowRight ArrowDown\n  Home End PageUp PageDown Backspace Delete Space\n  Shift+ only with Enter, Tab, arrows, Home, End, PageUp or PageDown.\n  Meta+A Meta+Z Meta+Shift+Z Meta+Backspace\n  Meta+arrows Meta+Shift+arrows\n  Alt+ArrowLeft Alt+ArrowRight Alt+Shift+ArrowLeft Alt+Shift+ArrowRight\n  Alt+Backspace Alt+Delete\n  Key names are case-sensitive; only the listed combinations are supported.\n\nID uses 1..128 ASCII letters, digits, hyphens or underscores.\nSpace IDs use user or an existing task ID, at most 120 ASCII characters.\nREV is the positive revision returned by task info or a task update.\nNAME is trimmed, contains 1..120 characters, and has no control characters.\ntask run cannot contain task commands.\n\nNavigation commands return when the request is accepted; use wait for readiness.\nwait dom: document.readyState is interactive or complete.\nwait load: the browser's overall loading has ended.\nSpecify dom or load before TIMEOUT_MS. open and navigate accept HTTP(S) URLs.\nTAB is a positive integer. Quote arguments containing spaces.\nEvery command writes one JSON response to stdout. Errors also use stderr.\n";

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
    let mut command = match values.as_slice() {
        ["task", task_args @ ..] => parse_task_command(task_args),
        ["open", url] => Ok(Command::Open {
            url: parse_url("open", url)?,
        }),
        ["navigate", tab, url] => Ok(Command::Navigate {
            tab: parse_tab(tab)?,
            url: parse_url("navigate", url)?,
        }),
        ["back", tab] => Ok(Command::Back {
            tab: parse_tab(tab)?,
        }),
        ["forward", tab] => Ok(Command::Forward {
            tab: parse_tab(tab)?,
        }),
        ["reload", tab] => Ok(Command::Reload {
            tab: parse_tab(tab)?,
        }),
        ["stop", tab] => Ok(Command::Stop {
            tab: parse_tab(tab)?,
        }),
        ["tabs"] => Ok(Command::Tabs),
        ["spaces"] => Ok(Command::Spaces),
        ["space", "show", space] => Ok(Command::SpaceShow {
            space: (*space).into(),
        }),
        ["space", "overview"] => Ok(Command::SpaceOverview),
        ["panel"] => Ok(Command::Panel),
        ["wait", tab, options @ ..] if options.len() <= 2 => {
            let until = match options.first().copied().unwrap_or("dom") {
                "dom" => WaitUntil::Dom,
                "load" => WaitUntil::Load,
                _ => return Err("Wait mode must be dom or load.".into()),
            };
            let timeout_ms = options
                .get(1)
                .copied()
                .unwrap_or("10000")
                .parse::<u64>()
                .ok()
                .filter(|timeout| (1..=30000).contains(timeout))
                .ok_or("TIMEOUT_MS must be an integer between 1 and 30000.")?;
            Ok(Command::Wait {
                tab: parse_tab(tab)?,
                until,
                timeout_ms,
            })
        }
        ["read", tab] => Ok(Command::Read {
            tab: parse_tab(tab)?,
        }),
        ["read-page", tab] => Ok(Command::ReadPage {
            tab: parse_tab(tab)?,
        }),
        ["frames", tab] => Ok(Command::Frames {
            tab: parse_tab(tab)?,
        }),
        ["read-frame", tab, frame] => Ok(Command::ReadFrame {
            tab: parse_tab(tab)?,
            frame: (*frame).into(),
        }),
        ["scroll", tab, x, y] => Ok(Command::Scroll {
            tab: parse_tab(tab)?,
            x: parse_scroll_delta(x)?,
            y: parse_scroll_delta(y)?,
        }),
        ["scroll", tab, reference, x, y] => Ok(Command::ScrollElement {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            x: parse_scroll_delta(x)?,
            y: parse_scroll_delta(y)?,
        }),
        ["click", tab, reference] => Ok(Command::Click {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
        }),
        ["hover", tab, reference] => Ok(Command::Hover {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
        }),
        ["check", tab, reference, checked] => Ok(Command::Check {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            checked: match *checked {
                "true" => true,
                "false" => false,
                _ => return Err("CHECKED must be exactly true or false.".into()),
            },
        }),
        ["fill", tab, reference, text] => Ok(Command::Fill {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            text: (*text).into(),
        }),
        ["insert", tab, reference, text] => Ok(Command::Insert {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            text: (*text).into(),
        }),
        ["select", tab, reference, value] => Ok(Command::Select {
            tab: parse_tab(tab)?,
            reference: parse_ref(reference)?,
            value: (*value).into(),
        }),
        ["press", tab, reference, key] => {
            if KeyPress::parse(key).is_none() {
                return Err(UNSUPPORTED_KEY_MESSAGE.into());
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
                    | "navigate"
                    | "back"
                    | "forward"
                    | "reload"
                    | "stop"
                    | "tabs"
                    | "spaces"
                    | "space"
                    | "wait"
                    | "read"
                    | "read-page"
                    | "frames"
                    | "read-frame"
                    | "scroll"
                    | "click"
                    | "hover"
                    | "check"
                    | "fill"
                    | "insert"
                    | "select"
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
    }?;
    command.validate().map_err(|error| error.message)?;
    if let Command::TaskCreate { name } = &mut command {
        *name = name.trim().to_owned();
    }
    Ok(command)
}

fn parse_task_command(args: &[&str]) -> Result<Command, String> {
    match args {
        ["create", name] => Ok(Command::TaskCreate {
            name: (*name).into(),
        }),
        ["list"] => Ok(Command::TaskList),
        ["info", task] => Ok(Command::TaskInfo {
            task: (*task).into(),
        }),
        ["handoff", task, revision] => Ok(Command::TaskHandoff {
            task: (*task).into(),
            revision: parse_revision(revision)?,
        }),
        ["resume", task, revision] => Ok(Command::TaskResume {
            task: (*task).into(),
            revision: parse_revision(revision)?,
        }),
        ["adopt", task, revision, tab] => Ok(Command::TaskAdopt {
            task: (*task).into(),
            revision: parse_revision(revision)?,
            tab: parse_tab(tab)?,
        }),
        ["release", task, revision, tab] => Ok(Command::TaskRelease {
            task: (*task).into(),
            revision: parse_revision(revision)?,
            tab: parse_tab(tab)?,
        }),
        ["finish", task, revision, options @ ..] => {
            let keep = match options {
                [] => Vec::new(),
                ["--keep", tabs @ ..] if !tabs.is_empty() => tabs
                    .iter()
                    .map(|tab| parse_tab(tab))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(
                    "Use task finish ID REV [--keep TAB ...], with at least one tab after --keep."
                        .into(),
                ),
            };
            Ok(Command::TaskFinish {
                task: (*task).into(),
                revision: parse_revision(revision)?,
                keep,
            })
        }
        ["run", task, revision, inner @ ..] if !inner.is_empty() => {
            if inner[0] == "task" {
                return Err("task run accepts only a browser command, not a task command.".into());
            }
            let inner_args = inner
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>();
            let command = parse_command(&inner_args)?;
            if command.is_task_command() {
                return Err("task run accepts only a browser command, not a task command.".into());
            }
            Ok(Command::TaskRun {
                task: (*task).into(),
                revision: parse_revision(revision)?,
                command: Box::new(command),
            })
        }
        [name, ..]
            if matches!(
                *name,
                "create"
                    | "list"
                    | "info"
                    | "handoff"
                    | "resume"
                    | "adopt"
                    | "release"
                    | "finish"
                    | "run"
            ) =>
        {
            Err(format!("Wrong number of arguments for task {name}."))
        }
        [name, ..] => Err(format!("Unknown task command: {name}")),
        [] => Err("A task command is required.".into()),
    }
}

fn parse_revision(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| "REV must be a positive 64-bit integer.".into())
}

fn parse_url(command: &str, value: &str) -> Result<String, String> {
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err(format!(
            "{command} accepts only an http:// or https:// URL."
        ));
    }
    Ok(value.into())
}

fn parse_tab(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .ok()
        .filter(|tab| *tab > 0)
        .ok_or_else(|| "TAB must be a positive 32-bit integer.".into())
}

fn parse_scroll_delta(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .ok()
        .filter(|delta| (-10000..=10000).contains(delta))
        .ok_or_else(|| "DX and DY must be integers between -10000 and 10000 CSS pixels.".into())
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
    fn space_commands_parse_without_changing_task_or_panel_syntax() {
        for (input, expected) in [
            (vec!["spaces"], Command::Spaces),
            (vec!["space", "overview"], Command::SpaceOverview),
            (
                vec!["space", "show", "user"],
                Command::SpaceShow {
                    space: "user".into(),
                },
            ),
            (
                vec!["space", "show", "task-Abc_123"],
                Command::SpaceShow {
                    space: "task-Abc_123".into(),
                },
            ),
            (vec!["panel"], Command::Panel),
            (
                vec!["task", "run", "task-1", "2", "spaces"],
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(Command::Spaces),
                },
            ),
        ] {
            assert_eq!(parse_command(&args(&input)).unwrap(), expected);
        }
    }

    #[test]
    fn space_commands_reject_bad_ids_and_wrong_arguments() {
        let longest = "a".repeat(120);
        assert_eq!(
            parse_command(&args(&["space", "show", &longest])).unwrap(),
            Command::SpaceShow { space: longest }
        );
        for invalid in [
            "",
            " ",
            "user\n",
            "task\t1",
            "task/1",
            "task id",
            "한글",
            &"a".repeat(121),
        ] {
            assert!(
                parse_command(&args(&["space", "show", invalid])).is_err(),
                "{invalid:?}"
            );
        }
        for invalid in [
            vec!["spaces", "extra"],
            vec!["space"],
            vec!["space", "show"],
            vec!["space", "show", "user", "extra"],
            vec!["space", "overview", "extra"],
            vec!["space", "unknown"],
            vec!["task", "run", "task-1", "2", "spaces", "extra"],
        ] {
            assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn task_commands_parse_with_names_and_revisions() {
        let task = "task-Abc_123";
        for (input, expected) in [
            (
                vec!["task", "create", "  검색 작업  "],
                Command::TaskCreate {
                    name: "검색 작업".into(),
                },
            ),
            (vec!["task", "list"], Command::TaskList),
            (
                vec!["task", "info", task],
                Command::TaskInfo { task: task.into() },
            ),
            (
                vec!["task", "handoff", task, "1"],
                Command::TaskHandoff {
                    task: task.into(),
                    revision: 1,
                },
            ),
            (
                vec!["task", "resume", task, "2"],
                Command::TaskResume {
                    task: task.into(),
                    revision: 2,
                },
            ),
            (
                vec!["task", "adopt", task, "3", "4"],
                Command::TaskAdopt {
                    task: task.into(),
                    revision: 3,
                    tab: 4,
                },
            ),
            (
                vec!["task", "release", task, "4", "5"],
                Command::TaskRelease {
                    task: task.into(),
                    revision: 4,
                    tab: 5,
                },
            ),
            (
                vec!["task", "finish", task, "5"],
                Command::TaskFinish {
                    task: task.into(),
                    revision: 5,
                    keep: vec![],
                },
            ),
            (
                vec!["task", "finish", task, "5", "--keep", "2", "4"],
                Command::TaskFinish {
                    task: task.into(),
                    revision: 5,
                    keep: vec![2, 4],
                },
            ),
        ] {
            assert_eq!(parse_command(&args(&input)).unwrap(), expected);
        }
        assert_eq!(
            parse_command(&args(&["task", "handoff", task, &u64::MAX.to_string()])).unwrap(),
            Command::TaskHandoff {
                task: task.into(),
                revision: u64::MAX
            },
        );
    }

    #[test]
    fn task_name_and_id_bounds_use_characters_and_ascii_ids() {
        let name = "한".repeat(120);
        assert_eq!(
            parse_command(&args(&["task", "create", &name])).unwrap(),
            Command::TaskCreate { name }
        );
        for invalid in [
            "".to_owned(),
            "   ".into(),
            "한".repeat(121),
            "name\n".into(),
            "na\tme".into(),
        ] {
            assert!(parse_command(&args(&["task", "create", &invalid])).is_err());
        }
        let id = "A".repeat(128);
        assert_eq!(
            parse_command(&args(&["task", "info", &id])).unwrap(),
            Command::TaskInfo { task: id }
        );
        for invalid in [
            "".to_owned(),
            "a".repeat(129),
            "한글".into(),
            "task id".into(),
            "task/id".into(),
            "task.id".into(),
        ] {
            assert!(parse_command(&args(&["task", "info", &invalid])).is_err());
        }
    }

    #[test]
    fn task_run_wraps_existing_parser_and_rejects_task_commands() {
        assert_eq!(
            parse_command(&args(&[
                "task",
                "run",
                "task-1",
                "2",
                "fill",
                "3",
                "ref",
                "한글 text"
            ]))
            .unwrap(),
            Command::TaskRun {
                task: "task-1".into(),
                revision: 2,
                command: Box::new(Command::Fill {
                    tab: 3,
                    reference: "ref".into(),
                    text: "한글 text".into()
                }),
            },
        );
        assert_eq!(
            parse_command(&args(&["task", "run", "task-1", "2", "wait", "3"])).unwrap(),
            Command::TaskRun {
                task: "task-1".into(),
                revision: 2,
                command: Box::new(Command::Wait {
                    tab: 3,
                    until: WaitUntil::Dom,
                    timeout_ms: 10000
                }),
            },
        );
        assert_eq!(
            parse_command(&args(&["task", "run", "task-1", "2", "frames", "3"])).unwrap(),
            Command::TaskRun {
                task: "task-1".into(),
                revision: 2,
                command: Box::new(Command::Frames { tab: 3 }),
            },
        );
        for frame in ["frame-A1".to_owned(), "F".repeat(256)] {
            assert_eq!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "read-frame",
                    "3",
                    &frame,
                ]))
                .unwrap(),
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(Command::ReadFrame { tab: 3, frame }),
                },
            );
        }
        for invalid in [
            vec!["task", "run", "task-1", "2"],
            vec!["task", "run", "task-1", "2", "task", "list"],
            vec![
                "task", "run", "task-1", "2", "task", "run", "task-2", "1", "tabs",
            ],
            vec!["task", "run", "task-1", "2", "read", "0"],
            vec!["task", "run", "task-1", "2", "frames", "0"],
            vec!["task", "run", "task-1", "2", "frames", "3", "extra"],
            vec!["task", "run", "task-1", "2", "read-frame", "0", "frame-A1"],
            vec!["task", "run", "task-1", "2", "read-frame", "3"],
            vec![
                "task",
                "run",
                "task-1",
                "2",
                "read-frame",
                "3",
                "frame-A1",
                "extra",
            ],
            vec!["task", "run", "task-1", "2", "scroll", "3", "0", "10001"],
            vec!["task", "run", "task-1", "2", "tabs", "extra"],
        ] {
            assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn task_commands_reject_zero_revision_bad_keep_and_wrong_arguments() {
        for revision in ["0", "-1", "1.5", "next", "18446744073709551616"] {
            for name in ["handoff", "resume", "adopt", "release", "finish", "run"] {
                let mut input = vec!["task", name, "task-1", revision];
                if matches!(name, "adopt" | "release") {
                    input.push("1");
                }
                if name == "run" {
                    input.push("tabs");
                }
                assert!(parse_command(&args(&input)).is_err(), "{input:?}");
            }
        }
        for invalid in [
            vec!["task"],
            vec!["task", "unknown"],
            vec!["task", "create"],
            vec!["task", "create", "name", "extra"],
            vec!["task", "list", "extra"],
            vec!["task", "info"],
            vec!["task", "info", "task-1", "extra"],
            vec!["task", "handoff", "task-1", "1", "extra"],
            vec!["task", "resume", "task-1"],
            vec!["task", "adopt", "task-1", "1", "0"],
            vec!["task", "release", "task-1", "1", "-1"],
            vec!["task", "finish", "task-1", "1", "--keep"],
            vec!["task", "finish", "task-1", "1", "2"],
            vec!["task", "finish", "task-1", "1", "--keep", "0"],
            vec!["task", "finish", "task-1", "1", "--keep", "1", "1"],
            vec!["task", "finish", "task-1", "1", "--keep", "2147483648"],
            vec![
                "task", "finish", "task-1", "1", "--keep", "1", "--keep", "2",
            ],
        ] {
            assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn rejects_dangerous_scheme_and_extra_arguments() {
        assert!(parse_command(&args(&["open", "javascript:alert(1)"])).is_err());
        assert!(parse_command(&args(&["tabs", "unexpected"])).is_err());
        assert!(parse_command(&args(&["close", "0"])).is_err());
    }

    #[test]
    fn navigation_commands_preserve_tab_and_unicode_url() {
        for url in [
            "https://example.test/검색?q=한글",
            "http://127.0.0.1:8080/a#part",
        ] {
            assert_eq!(
                parse_command(&args(&["navigate", "3", url])).unwrap(),
                Command::Navigate {
                    tab: 3,
                    url: url.into(),
                }
            );
        }
        for (name, command) in [
            ("back", Command::Back { tab: 3 }),
            ("forward", Command::Forward { tab: 3 }),
            ("reload", Command::Reload { tab: 3 }),
            ("stop", Command::Stop { tab: 3 }),
            ("frames", Command::Frames { tab: 3 }),
        ] {
            assert_eq!(parse_command(&args(&[name, "3"])).unwrap(), command);
        }
        for frame in ["frame-A1".to_owned(), "F".repeat(256)] {
            assert_eq!(
                parse_command(&args(&["read-frame", "3", &frame])).unwrap(),
                Command::ReadFrame { tab: 3, frame },
            );
        }
    }

    #[test]
    fn navigation_rejects_bad_scheme_tab_and_argument_count() {
        for url in [
            "",
            "example.test",
            "file:///tmp/page.html",
            "javascript:alert(1)",
        ] {
            assert!(
                parse_command(&args(&["navigate", "3", url])).is_err(),
                "{url}"
            );
        }
        for name in [
            "navigate",
            "back",
            "forward",
            "reload",
            "stop",
            "frames",
            "read-frame",
        ] {
            let mut valid = vec![name, "3"];
            if name == "navigate" {
                valid.push("https://example.test");
            } else if name == "read-frame" {
                valid.push("frame-A1");
            }
            let mut extra = valid.clone();
            extra.push("extra");
            assert!(parse_command(&args(&extra)).is_err(), "{extra:?}");
            for count in 1..valid.len() {
                assert!(parse_command(&args(&valid[..count])).is_err(), "{name}");
            }
            for tab in ["0", "-1", "1.5", "tab", "2147483648"] {
                let mut invalid = valid.clone();
                invalid[1] = tab;
                assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
            }
        }
        for frame in [
            "".to_owned(),
            "프레임".into(),
            "frame\n1".into(),
            "frame\u{7f}".into(),
            "F".repeat(257),
        ] {
            assert!(
                parse_command(&args(&["read-frame", "3", &frame])).is_err(),
                "{frame:?}"
            );
            assert!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "read-frame",
                    "3",
                    &frame,
                ]))
                .is_err(),
                "{frame:?}"
            );
        }
    }

    #[test]
    fn scroll_element_cli_preserves_references_and_task_scope() {
        for reference in ["9/doc:1:1".to_owned(), "r".repeat(256), "가".repeat(85)] {
            let command = Command::ScrollElement {
                tab: 3,
                reference: reference.clone(),
                x: -10000,
                y: 10000,
            };
            assert_eq!(
                parse_command(&args(&["scroll", "3", &reference, "-10000", "10000"])).unwrap(),
                command
            );
            assert_eq!(
                parse_command(&args(&[
                    "task", "run", "task-1", "2", "scroll", "3", &reference, "-10000", "10000"
                ]))
                .unwrap(),
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(command),
                }
            );
        }
        assert_eq!(
            parse_command(&args(&["scroll", "3", "-10000", "10000"])).unwrap(),
            Command::Scroll {
                tab: 3,
                x: -10000,
                y: 10000
            }
        );
    }

    #[test]
    fn scroll_element_cli_rejects_invalid_refs_axes_and_arity_in_each_scope() {
        let mut invalid = vec![
            args(&["scroll", "0", "9/doc:1:1", "0", "1"]),
            args(&["scroll", "3", "9/doc:1:1", "0"]),
            args(&["scroll", "3", "9/doc:1:1", "0", "1", "extra"]),
        ];
        for reference in ["".to_owned(), "r".repeat(257), "가".repeat(86)] {
            invalid.push(args(&["scroll", "3", &reference, "0", "1"]));
        }
        for value in ["-10001", "10001", "2147483648", "1.5", "NaN", ""] {
            invalid.push(args(&["scroll", "3", "9/doc:1:1", value, "0"]));
            invalid.push(args(&["scroll", "3", "9/doc:1:1", "0", value]));
        }
        for command in invalid {
            assert!(parse_command(&command).is_err(), "{command:?}");
            let mut nested = args(&["task", "run", "task-1", "2"]);
            nested.extend(command);
            assert!(parse_command(&nested).is_err(), "{nested:?}");
        }
    }

    #[test]
    fn scroll_preserves_axes_signs_and_inclusive_bounds() {
        for (x, y) in [(0, 0), (-10000, 10000), (10000, -10000), (-250, 400)] {
            assert_eq!(
                parse_command(&args(&["scroll", "3", &x.to_string(), &y.to_string()])).unwrap(),
                Command::Scroll { tab: 3, x, y }
            );
        }
    }

    #[test]
    fn scroll_requires_two_bounded_integer_deltas_and_valid_tab() {
        for value in [
            "-10001",
            "10001",
            "2147483648",
            "-2147483649",
            "1.5",
            "NaN",
            "inf",
            "",
        ] {
            assert!(
                parse_command(&args(&["scroll", "3", value, "0"])).is_err(),
                "x={value}"
            );
            assert!(
                parse_command(&args(&["scroll", "3", "0", value])).is_err(),
                "y={value}"
            );
        }
        for invalid in [
            vec!["scroll"],
            vec!["scroll", "3"],
            vec!["scroll", "3", "0"],
            vec!["scroll", "3", "0", "0", "extra"],
            vec!["scroll", "0", "0", "0"],
            vec!["scroll", "-1", "0", "0"],
        ] {
            assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn wait_defaults_and_explicit_modes() {
        assert_eq!(
            parse_command(&args(&["wait", "3"])).unwrap(),
            Command::Wait {
                tab: 3,
                until: WaitUntil::Dom,
                timeout_ms: 10000,
            }
        );
        for (mode, until) in [("dom", WaitUntil::Dom), ("load", WaitUntil::Load)] {
            assert_eq!(
                parse_command(&args(&["wait", "3", mode])).unwrap(),
                Command::Wait {
                    tab: 3,
                    until,
                    timeout_ms: 10000,
                }
            );
        }
    }

    #[test]
    fn wait_timeout_bounds() {
        for timeout_ms in [1, 30000] {
            assert_eq!(
                parse_command(&args(&["wait", "3", "load", &timeout_ms.to_string()])).unwrap(),
                Command::Wait {
                    tab: 3,
                    until: WaitUntil::Load,
                    timeout_ms,
                }
            );
        }
        for value in ["0", "30001", "-1", "1.5", "later", "18446744073709551616"] {
            assert!(
                parse_command(&args(&["wait", "3", "dom", value])).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn wait_rejects_bad_mode_tab_and_argument_count() {
        for invalid in [
            vec!["wait"],
            vec!["wait", "0"],
            vec!["wait", "3", "networkidle"],
            vec!["wait", "3", "1000"],
            vec!["wait", "3", "dom", "1000", "extra"],
        ] {
            assert!(parse_command(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn check_accepts_exact_booleans_globally_and_in_tasks() {
        for (text, checked) in [("false", false), ("true", true)] {
            let expected = Command::Check {
                tab: 3,
                reference: "9/doc:1:1".into(),
                checked,
            };
            assert_eq!(
                parse_command(&args(&["check", "3", "9/doc:1:1", text])).unwrap(),
                expected
            );
            assert_eq!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "check",
                    "3",
                    "9/doc:1:1",
                    text
                ]))
                .unwrap(),
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(expected)
                }
            );
        }
    }

    #[test]
    fn check_rejects_non_boolean_values_and_malformed_arguments() {
        for checked in [
            "", "True", "FALSE", "1", "0", "yes", "null", " true", "false ",
        ] {
            assert!(parse_command(&args(&["check", "3", "9/doc:1:1", checked])).is_err());
            assert!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "check",
                    "3",
                    "9/doc:1:1",
                    checked
                ]))
                .is_err()
            );
        }
        let long_ref = "x".repeat(257);
        for input in [
            vec!["check"],
            vec!["check", "3", "9/doc:1:1"],
            vec!["check", "3", "9/doc:1:1", "true", "extra"],
            vec!["check", "0", "9/doc:1:1", "true"],
            vec!["check", "-1", "9/doc:1:1", "true"],
            vec!["check", "tab", "9/doc:1:1", "true"],
            vec!["check", "3", "", "false"],
            vec!["check", "3", &long_ref, "true"],
        ] {
            assert!(parse_command(&args(&input)).is_err(), "{input:?}");
        }
    }

    #[test]
    fn press_named_keys_keep_wire_strings_and_task_scopes() {
        for key in [
            "Enter",
            "Tab",
            "Escape",
            "ArrowLeft",
            "ArrowUp",
            "ArrowRight",
            "ArrowDown",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Backspace",
            "Delete",
            "Space",
            "Shift+Enter",
            "Shift+Tab",
            "Shift+ArrowLeft",
            "Shift+ArrowUp",
            "Shift+ArrowRight",
            "Shift+ArrowDown",
            "Shift+Home",
            "Shift+End",
            "Shift+PageUp",
            "Shift+PageDown",
            "Meta+A",
            "Meta+Z",
            "Meta+Shift+Z",
            "Meta+ArrowLeft",
            "Meta+ArrowUp",
            "Meta+ArrowRight",
            "Meta+ArrowDown",
            "Meta+Shift+ArrowLeft",
            "Meta+Shift+ArrowUp",
            "Meta+Shift+ArrowRight",
            "Meta+Shift+ArrowDown",
            "Meta+Backspace",
            "Alt+ArrowLeft",
            "Alt+ArrowRight",
            "Alt+Shift+ArrowLeft",
            "Alt+Shift+ArrowRight",
            "Alt+Backspace",
            "Alt+Delete",
        ] {
            let command = parse_command(&args(&["press", "7", "4/doc:2:3", key])).unwrap();
            assert_eq!(
                command,
                Command::Press {
                    tab: 7,
                    reference: "4/doc:2:3".into(),
                    key: key.into()
                }
            );
            assert_eq!(serde_json::to_value(&command).unwrap()["key"], key);
            assert_eq!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "press",
                    "7",
                    "4/doc:2:3",
                    key
                ]))
                .unwrap(),
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(command)
                }
            );
        }
    }

    #[test]
    fn press_rejects_aliases_unsupported_modifiers_and_malformed_arguments() {
        for key in [
            "",
            "enter",
            "Esc",
            "Return",
            " ",
            "a",
            "Enter ",
            " Shift+Tab",
            "shift+Tab",
            "Shift+Shift+Tab",
            "Control+Tab",
            "Ctrl+Enter",
            "Alt+ArrowUp",
            "Meta+Delete",
            "Shift+Escape",
            "Shift+Backspace",
            "Shift+Delete",
            "Shift+Space",
            "Tab\n",
        ] {
            assert!(
                parse_command(&args(&["press", "7", "4/doc:2:3", key])).is_err(),
                "{key:?}"
            );
            assert!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "press",
                    "7",
                    "4/doc:2:3",
                    key
                ]))
                .is_err(),
                "{key:?}"
            );
        }
        for input in [
            vec!["press", "7", "4/doc:2:3"],
            vec!["press", "7", "4/doc:2:3", "Tab", "extra"],
            vec!["press", "0", "4/doc:2:3", "Tab"],
            vec!["press", "7", "", "Tab"],
        ] {
            assert!(parse_command(&args(&input)).is_err(), "{input:?}");
        }
    }

    #[test]
    fn fill_and_insert_preserve_empty_text_and_unicode() {
        for text in ["", "한글 text"] {
            for (name, expected) in [
                (
                    "fill",
                    Command::Fill {
                        tab: 1,
                        reference: "2:a".into(),
                        text: text.into(),
                    },
                ),
                (
                    "insert",
                    Command::Insert {
                        tab: 1,
                        reference: "2:a".into(),
                        text: text.into(),
                    },
                ),
            ] {
                assert_eq!(
                    parse_command(&args(&[name, "1", "2:a", text])).unwrap(),
                    expected
                );
                assert_eq!(
                    serde_json::to_value(&expected).unwrap(),
                    serde_json::json!({"type":name,"tab":1,"ref":"2:a","text":text})
                );
                assert_eq!(
                    parse_command(&args(&[
                        "task", "run", "task-1", "2", name, "1", "2:a", text
                    ]))
                    .unwrap(),
                    Command::TaskRun {
                        task: "task-1".into(),
                        revision: 2,
                        command: Box::new(expected),
                    }
                );
            }
        }
        for name in ["fill", "insert"] {
            for input in [
                vec![name, "1", "2:a"],
                vec![name, "1", "2:a", "text", "extra"],
                vec![name, "0", "2:a", "text"],
                vec![name, "1", "", "text"],
            ] {
                assert!(parse_command(&args(&input)).is_err(), "{input:?}");
            }
        }
    }

    #[test]
    fn select_preserves_empty_whitespace_and_unicode_values_in_task_commands() {
        for value in [String::new(), "  한글 배송  ".into(), "🧭".repeat(512)] {
            let expected = Command::Select {
                tab: 3,
                reference: "9/doc:1:1".into(),
                value: value.clone(),
            };
            assert_eq!(
                parse_command(&args(&["select", "3", "9/doc:1:1", &value])).unwrap(),
                expected
            );
            assert_eq!(
                parse_command(&args(&[
                    "task",
                    "run",
                    "task-1",
                    "2",
                    "select",
                    "3",
                    "9/doc:1:1",
                    &value
                ]))
                .unwrap(),
                Command::TaskRun {
                    task: "task-1".into(),
                    revision: 2,
                    command: Box::new(expected)
                }
            );
        }
    }

    #[test]
    fn select_rejects_missing_or_extra_args_bad_refs_and_oversized_values() {
        let too_long = "🧭".repeat(513);
        for input in [
            vec!["select", "3", "9/doc:1:1"],
            vec!["select", "3", "9/doc:1:1", "a", "extra"],
            vec!["select", "0", "9/doc:1:1", "a"],
            vec!["select", "3", "", "a"],
            vec!["select", "3", "9/doc:1:1", &too_long],
            vec![
                "task",
                "run",
                "task-1",
                "2",
                "select",
                "3",
                "9/doc:1:1",
                &too_long,
            ],
        ] {
            assert!(parse_command(&args(&input)).is_err(), "{input:?}");
        }
    }
}
