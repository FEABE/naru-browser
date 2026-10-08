use crate::protocol::{self, Command, Request, Response};
use base64::Engine;
use cef::*;
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Write,
    os::unix::net::UnixListener,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const DOM_BRIDGE: &str = include_str!("dom_bridge.js");
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct Reply {
    id: String,
    sender: Sender<Response>,
}
impl Reply {
    fn ok(&self, result: Json) {
        let _ = self.sender.send(Response::success(&self.id, result));
    }
    fn error(&self, code: &str, message: &str) {
        let _ = self.sender.send(Response::error(&self.id, code, message));
    }
}

struct Tab {
    browser: Browser,
    title: String,
    _devtools: Option<Registration>,
    opening: Option<Reply>,
}
enum Step {
    Frame(String),
    World(String),
    Evaluate,
    Screenshot(String),
}
struct Pending {
    tab: i32,
    reply: Reply,
    step: Step,
}
#[derive(Default)]
struct State {
    tabs: BTreeMap<i32, Tab>,
    pending: BTreeMap<i32, Pending>,
    next_message: i32,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

pub fn clear() {
    STATE.with(|state| *state.borrow_mut() = State::default());
}

pub fn close_all() {
    let browsers = STATE.with(|state| {
        state
            .borrow()
            .tabs
            .values()
            .map(|tab| tab.browser.clone())
            .collect::<Vec<_>>()
    });
    for browser in browsers {
        if let Some(host) = browser.host() {
            host.close_browser(0);
        }
    }
}

fn tab_info(tab: &Tab) -> Json {
    json!({"tab":tab.browser.identifier(),"title":tab.title,
        "url":tab.browser.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default(),
        "loading":tab.browser.is_loading()!=0})
}

pub fn start_server(listener: UnixListener, stopped: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // A single request queue deliberately serializes all v0 clients.
        for connection in listener.incoming() {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let Ok(mut stream) = connection else {
                continue;
            };
            let request = match protocol::read_request(&mut stream) {
                Ok(request) => request,
                Err(error) => {
                    let _ = protocol::write_response(
                        &mut stream,
                        &Response::error("invalid", error.code, error.message),
                    );
                    continue;
                }
            };
            let id = request.id.clone();
            let (sender, receiver) = mpsc::channel();
            let mut task = CommandTask::new(
                request,
                Reply {
                    id: id.clone(),
                    sender,
                },
            );
            if post_task(ThreadId::UI, Some(&mut task)) == 0 {
                let _ = protocol::write_response(
                    &mut stream,
                    &Response::error(id, "APP_NOT_READY", "The browser event loop is not ready."),
                );
                continue;
            }
            let deadline = Instant::now() + ACTION_TIMEOUT;
            let response = loop {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(response) => break response,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Response::error(
                            &id,
                            "APP_STOPPING",
                            "The browser stopped handling this request.",
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if stopped.load(Ordering::Acquire) {
                    break Response::error(&id, "APP_STOPPING", "The browser is shutting down.")
                        .with_details(json!({"outcomeUnknown":true}));
                }
                if Instant::now() < deadline {
                    continue;
                }
                let mut cancel = CancelTask::new(id.clone());
                post_task(ThreadId::UI, Some(&mut cancel));
                break Response::error(
                    &id,
                    "TIMEOUT",
                    "No completion was received; do not automatically repeat an action.",
                )
                .with_details(json!({"outcomeUnknown":true}));
            };
            let _ = protocol::write_response(&mut stream, &response);
        }
    })
}

wrap_task! {
    struct CommandTask { request: Request, reply: Reply }
    impl Task { fn execute(&self) { handle(&self.request.command, self.reply.clone()); } }
}
wrap_task! {
    struct CancelTask { id: String }
    impl Task {
        fn execute(&self) {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.pending.retain(|_, pending| pending.reply.id != self.id);
                for tab in state.tabs.values_mut() {
                    if tab.opening.as_ref().is_some_and(|r| r.id == self.id) { tab.opening = None; }
                }
            });
        }
    }
}

fn handle(command: &Command, reply: Reply) {
    match command {
        Command::Status => reply.ok(STATE.with(|state| json!({"app":"Light Browser","engine":"CEF/Chromium","cef":String::from_utf8_lossy(&sys::CEF_VERSION[..sys::CEF_VERSION.len()-1]),"cefApi":sys::CEF_API_VERSION_LAST,"version":env!("CARGO_PKG_VERSION"),"tabs":state.borrow().tabs.len(),"sandbox":true,"externalDebugPort":false}))),
        Command::Tabs => reply.ok(STATE.with(|state| json!({"tabs":state.borrow().tabs.values().map(tab_info).collect::<Vec<_>>()}))),
        Command::Open { url } => {
            if !valid_url(url) { reply.error("INVALID_URL", "Only an absolute HTTP(S) URL without control characters is accepted."); return; }
            if create_browser(url, Some(reply.clone())).is_none() {
                reply.error("OPEN_FAILED", "Chromium could not create a browser window.");
            }
        }
        other => {
            let tab_id = match other {
                Command::Read { tab } | Command::Click { tab, .. } | Command::Fill { tab, .. } |
                Command::Press { tab, .. } | Command::Close { tab } | Command::Screenshot { tab, .. } => *tab,
                _ => unreachable!(),
            };
            let browser = STATE.with(|state| state.borrow().tabs.get(&tab_id).map(|tab| tab.browser.clone()));
            let Some(browser) = browser else { reply.error("TAB_NOT_FOUND", "No browser tab has that ID."); return; };
            match other {
                Command::Close { .. } => {
                    reply.ok(json!({"tab":tab_id,"closeRequested":true}));
                    if let Some(host) = browser.host() { host.close_browser(0); }
                }
                Command::Screenshot { path, .. } => {
                    if !Path::new(path).is_absolute() || Path::new(path).exists() {
                        reply.error("INVALID_PATH", "Use an absolute path to a new PNG file."); return;
                    }
                    dispatch(&browser, "Page.captureScreenshot", json!({"format":"png","captureBeyondViewport":false}), Pending { tab:tab_id, reply, step:Step::Screenshot(path.clone()) });
                }
                _ => {
                    if browser.is_loading() != 0 { reply.error("PAGE_LOADING", "The page is navigating. Read it after loading finishes."); return; }
                    let invocation = match other {
                        Command::Read { .. } => "window.__lightBrowserBridge.read()".to_owned(),
                        Command::Click { reference, .. } => action_expression("click", reference, None, &reply),
                        Command::Fill { reference, text, .. } => action_expression("fill", reference, Some(("text",text)), &reply),
                        Command::Press { reference, key, .. } => {
                            if key != "Enter" { reply.error("UNSUPPORTED_KEY", "The first version supports Enter only."); return; }
                            action_expression("press", reference, Some(("key",key)), &reply)
                        }
                        _ => unreachable!(),
                    };
                    if invocation.is_empty() { return; }
                    let expression = format!("(()=>{{{DOM_BRIDGE}\nreturn {invocation};}})()");
                    dispatch(&browser, "Page.getFrameTree", json!({}), Pending { tab:tab_id, reply, step:Step::Frame(expression) });
                }
            }
        }
    }
}

fn action_expression(
    action: &str,
    reference: &str,
    value: Option<(&str, &String)>,
    reply: &Reply,
) -> String {
    if reference.is_empty() || reference.len() > 256 || reference.chars().any(char::is_control) {
        reply.error(
            "INVALID_REF",
            "Use an element reference from the latest read response.",
        );
        return String::new();
    }
    let mut args = json!({"action":action,"ref":reference});
    if let Some((key, value)) = value {
        args[key] = json!(value);
    }
    format!("window.__lightBrowserBridge.act({args})")
}

fn valid_url(url: &str) -> bool {
    let remainder = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    remainder.is_some_and(|value| {
        !value.is_empty()
            && !value.starts_with('/')
            && !value.starts_with('?')
            && !value.starts_with('#')
    }) && url.len() <= 8192
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn create_browser(url: &str, opening: Option<Reply>) -> Option<Browser> {
    // CEF can deliver loading callbacks before create_browser_sync returns.
    // Install the completion receiver in OnAfterCreated, before those callbacks.
    let mut client = BrowserClient::new(Arc::new(Mutex::new(opening)));
    browser_host_create_browser_sync(
        Some(&WindowInfo {
            runtime_style: RuntimeStyle::CHROME,
            ..Default::default()
        }),
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&BrowserSettings::default()),
        None,
        None,
    )
}

fn dispatch(browser: &Browser, method: &str, params: Json, pending: Pending) {
    let Some(host) = browser.host() else {
        pending
            .reply
            .error("TAB_CLOSED", "Browser host is unavailable.");
        return;
    };
    let mut dictionary = parse_json(
        Some(&CefString::from(params.to_string().as_str())),
        JsonParserOptions::default(),
    )
    .and_then(|value| value.dictionary());
    let id = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.next_message += 1;
        let id = state.next_message;
        state.pending.insert(id, pending);
        id
    });
    if host.execute_dev_tools_method(id, Some(&CefString::from(method)), dictionary.as_mut()) == 0 {
        if let Some(pending) = STATE.with(|state| state.borrow_mut().pending.remove(&id)) {
            pending.reply.error(
                "CONTROL_UNAVAILABLE",
                "Chromium rejected the control request.",
            );
        }
    }
}

fn devtools_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) {
    let Some(pending) = STATE.with(|state| state.borrow_mut().pending.remove(&id)) else {
        return;
    };
    let Ok(result) = serde_json::from_slice::<Json>(bytes) else {
        pending
            .reply
            .error("INVALID_RESULT", "Chromium returned invalid JSON.");
        return;
    };
    if !success {
        pending.reply.error(
            "CONTROL_ERROR",
            result["message"]
                .as_str()
                .unwrap_or("Chromium control failed."),
        );
        return;
    }
    let Pending { tab, reply, step } = pending;
    match step {
        Step::Frame(expression) => {
            let Some(frame) = result["frameTree"]["frame"]["id"].as_str() else {
                reply.error("FRAME_UNAVAILABLE", "Main frame is unavailable.");
                return;
            };
            dispatch(
                browser,
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"LightBrowserControl","grantUniveralAccess":false}),
                Pending {
                    tab,
                    reply,
                    step: Step::World(expression),
                },
            );
        }
        Step::World(expression) => {
            let Some(context) = result["executionContextId"].as_i64() else {
                reply.error("CONTEXT_UNAVAILABLE", "Page context is unavailable.");
                return;
            };
            dispatch(
                browser,
                "Runtime.evaluate",
                json!({"expression":expression,"contextId":context,"returnByValue":true,"awaitPromise":false,"timeout":5000}),
                Pending {
                    tab,
                    reply,
                    step: Step::Evaluate,
                },
            );
        }
        Step::Evaluate => {
            if result.get("exceptionDetails").is_some() {
                reply.error(
                    "PAGE_SCRIPT_ERROR",
                    "The page changed or could not be inspected. Read again.",
                );
                return;
            }
            let value = &result["result"]["value"];
            if !value.is_object() {
                reply.error(
                    "INVALID_PAGE_RESULT",
                    "The page did not return a structured result. Read again.",
                );
                return;
            }
            if value["ok"] == false {
                let code = value["error"].as_str().unwrap_or("ACTION_FAILED");
                let message = value["detail"]
                    .as_str()
                    .unwrap_or("The page action did not complete.");
                reply.error(code, message);
            } else {
                reply.ok(json!({"tab":tab,"page":value}));
            }
        }
        Step::Screenshot(path) => {
            let Some(data) = result["data"]
                .as_str()
                .filter(|data| data.len() <= 32 * 1024 * 1024)
            else {
                reply.error("SCREENSHOT_FAILED", "No bounded screenshot was returned.");
                return;
            };
            let saved = (|| -> Result<usize, Box<dyn std::error::Error>> {
                let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                file.write_all(&bytes)?;
                Ok(bytes.len())
            })();
            match saved {
                Ok(bytes) => reply.ok(json!({"tab":tab,"path":path,"bytes":bytes})),
                Err(_) => reply.error(
                    "SCREENSHOT_FAILED",
                    "Could not save screenshot to a new file.",
                ),
            }
        }
    }
}

wrap_app! {
    pub struct BrowserApp;
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(ProcessHandler::new()) }
    }
}
wrap_browser_process_handler! {
    struct ProcessHandler;
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) { if create_browser("about:blank", None).is_none() { quit_message_loop(); } }
        fn default_client(&self) -> Option<Client> { Some(BrowserClient::new(Arc::new(Mutex::new(None)))) }
    }
}
wrap_client! {
    struct BrowserClient { opening: Arc<Mutex<Option<Reply>>> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(LifeHandler::new(self.opening.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(TitleHandler::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(PageLoadHandler::new()) }
    }
}
wrap_life_span_handler! {
    struct LifeHandler { opening: Arc<Mutex<Option<Reply>>> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser)=browser {
                let opening = self.opening.lock().expect("Opening state lock").take();
                STATE.with(|state| {state.borrow_mut().tabs.insert(browser.identifier(),Tab {browser:browser.clone(),title:String::new(),_devtools:None,opening});});
                let mut observer=ControlObserver::new();
                let registration=browser.host().and_then(|host|host.add_dev_tools_message_observer(Some(&mut observer)));
                STATE.with(|state| {if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {tab._devtools=registration;}});
            }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser)=browser else{return;};
            let id=browser.identifier();
            let empty=STATE.with(|state| {
                let mut state=state.borrow_mut();
                if let Some(tab)=state.tabs.remove(&id) { if let Some(reply)=tab.opening {reply.error("TAB_CLOSED","The tab closed while loading.");} }
                state.pending.retain(|_,pending| {if pending.tab==id {pending.reply.error("TAB_CLOSED","The tab closed during the request.");false}else{true}});
                state.tabs.is_empty()
            });
            if empty {quit_message_loop();}
        }
    }
}
wrap_display_handler! {
    struct TitleHandler;
    impl DisplayHandler {
        fn on_title_change(&self,browser:Option<&mut Browser>,title:Option<&CefString>) {
            if let Some(browser)=browser {STATE.with(|state| {if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {tab.title=title.map(ToString::to_string).unwrap_or_default();}});}
        }
    }
}
wrap_load_handler! {
    struct PageLoadHandler;
    impl LoadHandler {
        fn on_loading_state_change(&self,browser:Option<&mut Browser>,is_loading:i32,_can_go_back:i32,_can_go_forward:i32) {
            if is_loading!=0{return;}
            if let Some(browser)=browser {STATE.with(|state| {
                if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {if let Some(reply)=tab.opening.take(){reply.ok(tab_info(tab));}}
            });}
        }
        fn on_load_error(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,error_code:Errorcode,_error_text:Option<&CefString>,_failed_url:Option<&CefString>) {
            if frame.is_none_or(|f|f.is_main()==0) || sys::cef_errorcode_t::from(error_code)==sys::cef_errorcode_t::ERR_ABORTED{return;}
            if let Some(browser)=browser {STATE.with(|state|{if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()){if let Some(reply)=tab.opening.take(){reply.error("NAVIGATION_FAILED","Chromium could not load the requested page.");}}});}
        }
    }
}
wrap_dev_tools_message_observer! {
    struct ControlObserver;
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>) {
            if let Some(browser)=browser {devtools_result(browser,message_id,success!=0,result.unwrap_or(b"{}"));}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::valid_url;
    #[test]
    fn rejects_non_web_urls_and_ambiguous_authorities() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "https:///host",
            "https://?x",
            "https://host\n",
        ] {
            assert!(!valid_url(url));
        }
        assert!(valid_url("https://www.naver.com/"));
        assert!(valid_url("http://127.0.0.1:8123/"));
    }
}
