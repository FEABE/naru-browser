use crate::capture::{MAX_CAPTURE_RESULT_BYTES, save_png_base64};
use crate::control_panel::{ShellAction, ShellSnapshot, ShellTab};
use crate::page_find::{Request as FindRequest, State as FindState, Target as FindTarget};
use crate::protocol::{self, Command, Request, Response, TaskScope, WaitUntil};
use crate::resources::{MAX_AUTOMATION_TABS, ResourceBudget};
use crate::spaces::{Spaces, USER_SPACE};
use crate::tasks::{Origin, Registry, Status, TaskError};
use base64::{Engine, engine::general_purpose::STANDARD};
use cef::*;
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const DOM_BRIDGE: &str = include_str!("dom_bridge.js");
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_CONNECTIONS: usize = 16;

#[derive(Clone)]
struct Reply {
    id: String,
    token: u64,
    active: Arc<AtomicBool>,
    sender: Sender<Response>,
    scope: Option<TaskScope>,
}
impl Reply {
    fn send(&self, response: Response) {
        if self.active.swap(false, Ordering::AcqRel) {
            let _ = self.sender.send(response);
        }
    }
    fn ok(&self, result: Json) {
        self.send(Response::success(&self.id, result));
    }
    fn error(&self, code: &str, message: &str) {
        self.send(Response::error(&self.id, code, message));
    }
    fn error_details(&self, code: &str, message: &str, details: Json) {
        self.send(Response::error(&self.id, code, message).with_details(details));
    }
}

#[derive(Default)]
struct Navigation {
    epoch: u64,
    // Main-document identity excludes provisional requests that become downloads.
    document_epoch: u64,
    download_permission_provenance: crate::download_permissions::Provenance,
    requested_url: String,
    provisional: bool,
    committed: bool,
    document_available: bool,
    main_load_complete: bool,
    intent_pending: bool,
    stopped: bool,
    stop_pending: bool,
    incomplete: bool,
    error: Option<Json>,
    renderer_exit: Option<Json>,
    recovering: bool,
}
impl Navigation {
    fn preview_ready(&self) -> bool {
        self.committed
            && self.document_available
            && self.main_load_complete
            && !self.provisional
            && !self.intent_pending
            && !self.stopped
            && !self.stop_pending
            && !self.incomplete
            && self.error.is_none()
            && self.renderer_exit.is_none()
    }

    fn can_reuse_bookmark(&self, url: &str) -> bool {
        self.requested_url == url
            && self.renderer_exit.is_none()
            && self.error.is_none()
            && !self.stopped
            && !self.incomplete
    }

    fn begin(&mut self, url: Option<&str>) {
        self.epoch += 1;
        self.provisional = true;
        self.intent_pending = true;
        self.stopped = false;
        self.stop_pending = false;
        self.incomplete = false;
        self.error = None;
        self.recovering = self.renderer_exit.is_some();
        if let Some(url) = url {
            self.requested_url = url.to_owned();
        }
    }
    fn finish_loading(&mut self) {
        if self.renderer_exit.is_some() {
            // A recovery attempt can end without committing (204, stop, or
            // network failure). Keep the failure, but allow another attempt.
            self.provisional = false;
            self.intent_pending = false;
            self.stop_pending = false;
            self.recovering = false;
            return;
        }
        self.provisional = false;
        self.incomplete |= self.intent_pending && !self.stopped && self.error.is_none();
        self.intent_pending = false;
        self.stop_pending = false;
    }
    fn renderer_failed(&mut self, reason: Json) {
        self.epoch += 1;
        self.renderer_exit = Some(reason);
        self.recovering = false;
        self.committed = false;
        self.document_available = false;
        self.main_load_complete = false;
        self.provisional = false;
        self.intent_pending = false;
        self.stopped = false;
        self.stop_pending = false;
        self.incomplete = false;
        self.error = None;
    }
    fn renderer_status(&self) -> &'static str {
        if self.renderer_exit.is_none() {
            "running"
        } else if self.recovering {
            "recovering"
        } else {
            "terminated"
        }
    }
    fn commit(&mut self, url: &str) {
        if self.renderer_exit.is_some() && !self.recovering {
            return;
        }
        self.renderer_exit = None;
        self.recovering = false;
        if !self.stop_pending {
            self.stopped = false;
        }
        if self
            .error
            .as_ref()
            .is_some_and(|error| error["failedUrl"].as_str() != Some(url))
        {
            self.error = None;
        }
        if !self.provisional {
            self.requested_url = url.to_owned();
        }
        self.epoch += 1;
        self.provisional = false;
        self.intent_pending = false;
        self.incomplete = false;
        self.document_epoch += 1;
        self.download_permission_provenance = Default::default();
        self.committed = true;
        self.document_available = false;
        self.main_load_complete = false;
    }
}
struct Tab {
    browser: Browser,
    title: String,
    rejected_creation: bool,
    _devtools: Option<Registration>,
    navigation: Navigation,
}
#[derive(Clone, Copy)]
enum Evaluation {
    Page { mutation: bool },
    ReadyState { token: u64 },
}
enum Step {
    Frame(String),
    World(String),
    Evaluate,
    Screenshot(String),
}
struct Pending {
    tab: i32,
    epoch: u64,
    evaluation: Evaluation,
    reply: Reply,
    step: Step,
}
struct Waiter {
    tab: i32,
    until: WaitUntil,
    timeout_ms: u64,
    started: Instant,
    probe_in_flight: bool,
    reply: Reply,
}

struct PreviewRequest {
    space: String,
    tab: i32,
    epoch: u64,
    generation: u64,
}

/// `ready_epoch` comes from a fresh readiness check after the CEF loading query.
fn reserve_preview(
    state: &mut State,
    key: &(String, i32, u64),
    generation: u64,
    ready_epoch: Option<u64>,
) -> Option<i32> {
    if ready_epoch != Some(key.2)
        || !state.spaces.overview()
        || state.preview_generation != generation
        || state.spaces.tab_space(key.1) != Some(key.0.as_str())
        || !state.preview_candidates.contains(key)
        || state.preview_attempted.contains(key)
        || !state.captures.is_empty()
    {
        return None;
    }
    state.preview_attempted.insert(key.clone());
    state.next_message += 1;
    let id = state.next_message;
    state.captures.insert(id, key.1);
    state.previews.insert(
        id,
        PreviewRequest {
            space: key.0.clone(),
            tab: key.1,
            epoch: key.2,
            generation,
        },
    );
    Some(id)
}

/// Native overview only. No recurring timer, disk file, or additional renderer.
/// This shares the single capture permit with CLI screenshots.
pub fn request_space_previews() {
    #[cfg(test)]
    return;
    #[cfg(not(test))]
    {
        let candidates = crate::control_panel::preview_candidates();
        let pending = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let candidates = candidates
                .into_iter()
                .filter_map(|(space, id)| {
                    let tab = state.tabs.get(&id)?;
                    if tab.rejected_creation
                        || tab._devtools.is_none()
                        || !tab.navigation.preview_ready()
                        || state.spaces.tab_space(id) != Some(space.as_str())
                    {
                        return None;
                    }
                    Some((space, id, tab.navigation.epoch))
                })
                .take(8)
                .collect::<Vec<_>>();
            if candidates != state.preview_candidates {
                state.preview_generation = state.preview_generation.wrapping_add(1);
                state
                    .preview_attempted
                    .retain(|key| candidates.contains(key));
                state.preview_candidates = candidates.clone();
            }
            if !state.captures.is_empty() {
                return Vec::new();
            }
            candidates
                .into_iter()
                .filter(|key| !state.preview_attempted.contains(key))
                .filter_map(|key| {
                    let browser = state.tabs.get(&key.1)?.browser.clone();
                    Some((key, state.preview_generation, browser))
                })
                .collect::<Vec<_>>()
        });
        let next = pending.into_iter().find_map(|(key, generation, browser)| {
            // CEF may reenter native callbacks. Do not hold STATE while asking
            // about loading, and revalidate before consuming the capture slot.
            if browser.is_loading() != 0 {
                return None;
            }
            let id = STATE.with(|state| {
                let mut state = state.borrow_mut();
                let ready_epoch = state
                    .tabs
                    .get(&key.1)
                    .filter(|tab| {
                        !tab.rejected_creation
                            && tab._devtools.is_some()
                            && tab.navigation.preview_ready()
                    })
                    .map(|tab| tab.navigation.epoch);
                reserve_preview(&mut state, &key, generation, ready_epoch)
            })?;
            Some((id, browser))
        });
        let Some((id, browser)) = next else {
            return;
        };
        let bounds = crate::control_panel::browser_bounds();
        // The CEF view is in points. Leave room for Retina pixels within the
        // image decoder's 640x480 hard limit; no full-size image is cached.
        let scale = (240.0 / f64::from(bounds.width.max(1)))
            .min(150.0 / f64::from(bounds.height.max(1)))
            .min(1.0);
        let params = json!({"format":"png","captureBeyondViewport":false,
            "clip":{"x":0,"y":0,"width":bounds.width,"height":bounds.height,"scale":scale}});
        let mut dictionary = parse_json(
            Some(&CefString::from(params.to_string().as_str())),
            JsonParserOptions::default(),
        )
        .and_then(|value| value.dictionary());
        let accepted = browser.host().is_some_and(|host| {
            host.execute_dev_tools_method(
                id,
                Some(&CefString::from("Page.captureScreenshot")),
                dictionary.as_mut(),
            ) != 0
        });
        if !accepted {
            complete_preview(id, false, b"{}");
        }
    }
}

fn complete_preview(id: i32, success: bool, bytes: &[u8]) -> bool {
    let request = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let request = state.previews.remove(&id)?;
        state.captures.remove(&id);
        if state.preview_generation != request.generation {
            // Another card may have changed while this capture was in flight.
            // Do not permanently suppress an unchanged card's next attempt.
            state
                .preview_attempted
                .remove(&(request.space.clone(), request.tab, request.epoch));
        }
        Some(request)
    });
    let Some(request) = request else {
        return false;
    };
    let current = STATE.with(|state| {
        let state = state.borrow();
        state.spaces.overview()
            && state.preview_generation == request.generation
            && state.spaces.tab_space(request.tab) == Some(request.space.as_str())
            && state.tabs.get(&request.tab).is_some_and(|tab| {
                tab.navigation.epoch == request.epoch && !tab.navigation.provisional
            })
    });
    if current && success && bytes.len() <= 1_400_000 {
        if let Ok(result) = serde_json::from_slice::<Json>(bytes) {
            if let Some(encoded) = result["data"]
                .as_str()
                .filter(|data| data.len() <= 1_398_104)
            {
                if let Ok(png) = STANDARD.decode(encoded) {
                    if png.len() <= 1024 * 1024 {
                        #[cfg(not(test))]
                        crate::control_panel::set_preview(&request.space, request.tab, &png);
                    }
                }
            }
        }
    }
    notify_panel();
    true
}

#[derive(Clone)]
struct LinkCreation {
    opener: i32,
    owner: Option<(TaskScope, Status)>,
    foreground: bool,
}

impl LinkCreation {
    fn scope(&self) -> Option<TaskScope> {
        self.owner
            .as_ref()
            .and_then(|(scope, status)| (*status != Status::Paused).then(|| scope.clone()))
    }

    fn matches(&self, opener_exists: bool, tasks: &Registry) -> bool {
        opener_exists && self.owner == link_owner(tasks, self.opener)
    }
}

fn link_owner(tasks: &Registry, opener: i32) -> Option<(TaskScope, Status)> {
    let (task, _) = tasks.owner(opener)?;
    let task = tasks.info(&task).ok()?;
    Some((
        TaskScope {
            task: task.id,
            revision: task.revision,
        },
        task.status,
    ))
}

fn link_foreground(disposition: WindowOpenDisposition) -> Option<bool> {
    match disposition {
        WindowOpenDisposition::NEW_FOREGROUND_TAB
        | WindowOpenDisposition::NEW_WINDOW
        | WindowOpenDisposition::NEW_POPUP => Some(true),
        WindowOpenDisposition::NEW_BACKGROUND_TAB => Some(false),
        _ => None,
    }
}

fn popup_foreground(disposition: WindowOpenDisposition, automated: bool) -> bool {
    !automated && link_foreground(disposition).unwrap_or(true)
}

fn link_is_current(state: &State, link: &LinkCreation) -> bool {
    link.matches(
        state
            .tabs
            .get(&link.opener)
            .is_some_and(|tab| !tab.rejected_creation),
        &state.tasks,
    )
}

#[derive(Default)]
struct State {
    closed_pages: crate::closed_pages::ClosedPages,
    page_find: FindState,
    previews: BTreeMap<i32, PreviewRequest>,
    preview_attempted: BTreeSet<(String, i32, u64)>,
    preview_candidates: Vec<(String, i32, u64)>,
    preview_generation: u64,
    tabs: BTreeMap<i32, Tab>,
    tasks: Registry,
    spaces: Spaces,
    pending: BTreeMap<i32, Pending>,
    // Native captures can outlive a timed-out or revoked CLI request.
    captures: BTreeMap<i32, i32>,
    creations: ResourceBudget,
    waits: BTreeMap<u64, Waiter>,
    next_message: i32,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

pub fn clear() {
    crate::download_permission_runtime::clear();
    crate::download_runtime::clear();
    STATE.with(|state| *state.borrow_mut() = State::default());
}

pub(crate) struct DownloadContext {
    pub scope: Option<TaskScope>,
    pub source: String,
}

pub(crate) struct DownloadPermissionContext {
    pub target: crate::download_permissions::Target,
    pub source: String,
}

pub(crate) fn download_permission_context(tab: i32) -> Result<DownloadPermissionContext, String> {
    let download = download_context(tab)?;
    let target = STATE.with(|state| {
        let state = state.borrow();
        let page = state
            .tabs
            .get(&tab)
            .filter(|page| !page.rejected_creation && page.navigation.renderer_exit.is_none())
            .ok_or_else(|| "다운로드를 요청한 페이지를 사용할 수 없습니다.".to_owned())?;
        let target = crate::download_permissions::Target {
            tab,
            epoch: page.navigation.document_epoch,
            owner: link_owner(&state.tasks, tab),
        };
        if !page
            .navigation
            .download_permission_provenance
            .permits(&target)
        {
            return Err(
                "페이지의 제어권이 변경되어 이전 다운로드 권한 요청을 취소했습니다.".to_owned(),
            );
        }
        Ok::<_, String>(target)
    })?;
    Ok(DownloadPermissionContext {
        target,
        source: download.source,
    })
}

fn record_download_permission_attempt(tab: i32) -> Result<(), String> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let owner = link_owner(&state.tasks, tab);
        let page = state
            .tabs
            .get_mut(&tab)
            .filter(|page| !page.rejected_creation && page.navigation.renderer_exit.is_none())
            .ok_or_else(|| "다운로드를 요청한 페이지를 사용할 수 없습니다.".to_owned())?;
        page.navigation.download_permission_provenance.record(
            crate::download_permissions::Target {
                tab,
                epoch: page.navigation.document_epoch,
                owner,
            },
        );
        Ok(())
    })
}

fn revoke_download_permission_provenance(tab: i32) {
    STATE.with(|state| {
        if let Some(page) = state.borrow_mut().tabs.get_mut(&tab) {
            page.navigation.download_permission_provenance.revoke();
        }
    });
}

pub(crate) fn validate_download_permission(
    target: &crate::download_permissions::Target,
) -> Result<(), String> {
    let current = download_permission_context(target.tab)?;
    if &current.target != target {
        return Err("페이지나 탭의 제어권이 변경되어 다운로드 권한 요청이 만료되었습니다.".into());
    }
    Ok(())
}

/// Capture ownership without retaining a browser or the source URL.
pub(crate) fn download_context(tab: i32) -> Result<DownloadContext, String> {
    STATE.with(|state| {
        let state = state.borrow();
        if !state
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
        {
            return Err("다운로드를 요청한 탭이 닫혔습니다.".into());
        }
        let task = state
            .tasks
            .owner(tab)
            .map(|(owner, _)| state.tasks.info(&owner).map_err(|error| error.to_string()))
            .transpose()?;
        match task {
            Some(task) if task.status == Status::Active => Ok(DownloadContext {
                scope: Some(TaskScope {
                    task: task.id,
                    revision: task.revision,
                }),
                source: format!("{} · 탭 {}", task.name, tab),
            }),
            Some(task) if task.status != Status::Paused => {
                Err("마무리 중인 작업에서는 다운로드를 받을 수 없습니다.".into())
            }
            _ => Ok(DownloadContext {
                scope: None,
                source: format!("사용자 · 탭 {tab}"),
            }),
        }
    })
}

pub(crate) fn validate_download(tab: i32, scope: Option<&TaskScope>) -> Result<(), String> {
    STATE.with(|state| {
        let state = state.borrow();
        if !state
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
        {
            return Err("다운로드를 요청한 탭이 닫혔습니다.".into());
        }
        if let Some(scope) = scope {
            state
                .tasks
                .check_tab(Some(scope), tab)
                .map_err(|_| "탭의 제어권이 변경되어 다운로드 요청이 만료되었습니다.".to_owned())?;
        }
        // A user download accepted while paused stays unscoped on later resume.
        Ok(())
    })
}

pub(crate) fn download_host(tab: i32, scope: Option<&TaskScope>) -> Result<BrowserHost, String> {
    validate_download(tab, scope)?;
    let browser = STATE.with(|state| state.borrow().tabs.get(&tab).map(|tab| tab.browser.clone()));
    browser
        .and_then(|browser| browser.host())
        .ok_or_else(|| "다운로드를 요청한 탭을 사용할 수 없습니다.".into())
}

pub fn shell_snapshot() -> ShellSnapshot {
    STATE.with(|state| {
        let state = state.borrow();
        ShellSnapshot {
            spaces: state.spaces.views(),
            active_space: state.spaces.active_space().to_owned(),
            active_tab: state.spaces.active_tab(),
            overview: state.spaces.overview(),
            tabs: state
                .tabs
                .iter()
                .filter(|(_, tab)| !tab.rejected_creation)
                .map(|(id, tab)| {
                    let task = state
                        .tasks
                        .owner(*id)
                        .and_then(|(task, _)| state.tasks.info(&task).ok());
                    let task_status = task
                        .as_ref()
                        .map(|task| {
                            format!(
                                "{}, {}",
                                task.name,
                                match task.status {
                                    Status::Active => "AI 제어 중",
                                    Status::Paused => "중지됨, 직접 사용 가능",
                                    Status::Finishing => "작업 마무리 중",
                                    Status::Finished => "직접 사용 중",
                                }
                            )
                        })
                        .unwrap_or_else(|| "직접 사용 중".into());
                    let stop_scope =
                        task.filter(|task| task.status == Status::Active)
                            .map(|task| TaskScope {
                                task: task.id,
                                revision: task.revision,
                            });
                    ShellTab {
                        id: *id,
                        navigation_epoch: tab.navigation.epoch,
                        space: state.spaces.tab_space(*id).unwrap_or(USER_SPACE).to_owned(),
                        title: tab.title.chars().take(120).collect(),
                        url: tab.navigation.requested_url.clone(),
                        can_go_back: tab.browser.can_go_back() != 0,
                        can_go_forward: tab.browser.can_go_forward() != 0,
                        loading: tab.browser.is_loading() != 0,
                        crashed: tab.navigation.renderer_exit.is_some(),
                        task_status,
                        stop_scope,
                    }
                })
                .collect(),
        }
    })
}

fn native_reply() -> (Reply, mpsc::Receiver<Response>) {
    let (sender, receiver) = mpsc::channel();
    (
        Reply {
            id: "native-browser".into(),
            token: 0,
            active: Arc::new(AtomicBool::new(true)),
            sender,
            scope: None,
        },
        receiver,
    )
}

fn native_result(receiver: mpsc::Receiver<Response>) -> Result<(), String> {
    let response = receiver
        .try_recv()
        .map_err(|_| "The requested operation did not complete.".to_owned())?;
    match response.error {
        Some(error) => Err(format!("{}: {}", error.code, error.message)),
        None => Ok(()),
    }
}

fn update_space_tasks() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let tasks = state.tasks.list();
        state.spaces.update_tasks(&tasks);
    });
}

fn spaces_json(scope: Option<&TaskScope>) -> Json {
    STATE.with(|state| {
        let state = state.borrow();
        let spaces = state
            .spaces
            .views()
            .into_iter()
            .filter(|space| scope.is_none_or(|scope| scope.task == space.id))
            .collect::<Vec<_>>();
        json!({"spaces":spaces,"activeSpace":state.spaces.active_space(),
            "activeTab":state.spaces.active_tab(),"overview":state.spaces.overview()})
    })
}

fn handoff_native(scope: TaskScope) -> Result<(), String> {
    let (reply, receiver) = native_reply();
    handle(
        &Command::TaskHandoff {
            task: scope.task,
            revision: scope.revision,
        },
        reply,
    );
    native_result(receiver)
}

fn handoff_space_if_active(space: &str) -> Result<(), String> {
    let scope = STATE.with(|state| {
        state
            .borrow()
            .tasks
            .info(space)
            .ok()
            .filter(|task| task.status == Status::Active)
            .map(|task| TaskScope {
                task: task.id,
                revision: task.revision,
            })
    });
    if let Some(scope) = scope {
        handoff_native(scope)?;
    }
    Ok(())
}

/// Called synchronously before AppKit delivers intentional input to a page.
/// Resolve the current owner instead of using the shell's last rendered scope.
pub fn handoff_page_input(tab: i32) -> Result<bool, String> {
    let scope = STATE.with(|state| {
        let state = state.borrow();
        if state.spaces.overview() || state.spaces.active_tab() != Some(tab) {
            return None;
        }
        let (owner, _) = state.tasks.owner(tab)?;
        let task = state.tasks.info(&owner).ok()?;
        (task.status == Status::Active).then_some(TaskScope {
            task: task.id,
            revision: task.revision,
        })
    });
    let Some(scope) = scope else { return Ok(false) };
    shell_action(ShellAction::Stop(tab, scope))?;
    Ok(true)
}

fn sync_shell() {
    reconcile_page_find();
    #[cfg(not(test))]
    crate::control_panel::sync();
}

fn page_find_browser(target: FindTarget) -> Result<Browser, String> {
    if crate::control_panel::search_is_open() {
        return Err("탭·북마크 화면을 닫은 뒤 페이지에서 찾아 주세요.".into());
    }
    STATE.with(|state| {
        let state = state.borrow();
        if state.spaces.overview() || state.spaces.active_tab() != Some(target.tab) {
            return Err("현재 화면에 표시된 페이지에서만 찾을 수 있습니다.".into());
        }
        state
            .tabs
            .get(&target.tab)
            .filter(|tab| {
                !tab.rejected_creation
                    && tab.navigation.epoch == target.epoch
                    && tab.navigation.document_available
                    && !tab.navigation.provisional
                    && tab.navigation.renderer_exit.is_none()
            })
            .map(|tab| tab.browser.clone())
            .ok_or_else(|| {
                "페이지가 바뀌었거나 아직 준비되지 않았습니다. 찾기를 다시 열어 주세요.".into()
            })
    })
}

/// Opening native find is intentional page control. Handoff invalidates refs,
/// so return the resulting epoch instead of keeping the shell's old snapshot.
pub(crate) fn begin_page_find(tab: i32, epoch: u64) -> Result<u64, String> {
    page_find_browser(FindTarget { tab, epoch })?;
    let handed_off = handoff_page_input(tab)?;
    let epoch = epoch
        .checked_add(u64::from(handed_off))
        .ok_or_else(|| "페이지 상태를 확인할 수 없습니다.".to_owned())?;
    page_find_browser(FindTarget { tab, epoch })?;
    Ok(epoch)
}

pub(crate) fn find_in_page(
    tab: i32,
    epoch: u64,
    token: u64,
    query: &str,
    forward: bool,
    find_next: bool,
) -> Result<(), String> {
    let target = FindTarget { tab, epoch };
    page_find_browser(target)?;
    // An agent may have resumed since the bar opened. Honor human control, but
    // never run an old request against the newly invalidated navigation epoch.
    if handoff_page_input(tab)? {
        return Err("AI 작업을 중지했습니다. 찾기를 다시 열어 주세요.".into());
    }
    let browser = page_find_browser(target)?;
    browser.host().ok_or("페이지 찾기를 시작할 수 없습니다.")?;
    let plan = STATE
        .with(|state| {
            state.borrow_mut().page_find.submit(FindRequest {
                target,
                token,
                query: query.to_owned(),
                forward,
                find_next,
            })
        })
        .map_err(str::to_owned)?;
    if let Some(tab) = plan.stop {
        stop_find_host(tab);
    }
    if let Some(request) = plan.issue {
        issue_page_find(request)?;
    }
    Ok(())
}

pub(crate) fn stop_page_find(tab: i32, token: u64) {
    if STATE.with(|state| state.borrow_mut().page_find.stop(tab, token)) {
        stop_find_host(tab);
    }
}

fn stop_find_host(tab: i32) {
    let browser = STATE.with(|state| state.borrow().tabs.get(&tab).map(|tab| tab.browser.clone()));
    if let Some(host) = browser.and_then(|browser| browser.host()) {
        host.stop_finding(1);
    }
}

fn invalidate_page_find(tab: i32) {
    if STATE.with(|state| state.borrow_mut().page_find.invalidate(tab)) {
        stop_find_host(tab);
    }
}

fn reconcile_page_find() {
    let target = STATE.with(|state| state.borrow().page_find.target());
    if let Some(target) = target {
        if page_find_browser(target).is_err() {
            invalidate_page_find(target.tab);
        }
    }
}

fn issue_page_find(request: FindRequest) -> Result<(), String> {
    if !STATE.with(|state| state.borrow().page_find.is_current(&request)) {
        return Ok(());
    }
    let result = (|| {
        let browser = page_find_browser(request.target)?;
        let host = browser.host().ok_or("페이지에서 찾을 수 없습니다.")?;
        page_find_browser(request.target)?;
        if !STATE.with(|state| state.borrow().page_find.is_current(&request)) {
            return Ok(());
        }
        if !request.find_next {
            // CEF156 calls this argument findNext, but forwards it as
            // Chromium's find_match. Reset establishes a fresh session even
            // for identical text; find_match=true also selects the first hit.
            host.stop_finding(1);
        }
        // Host access can dispatch callbacks: check both the request and
        // visible document immediately before CEF, without holding STATE.
        page_find_browser(request.target)?;
        if !STATE.with(|state| state.borrow_mut().page_find.mark_issued(&request)) {
            return Ok(());
        }
        host.find(
            Some(&CefString::from(request.query.as_str())),
            i32::from(request.forward),
            0,
            1,
        );
        Ok(())
    })();
    if result.is_err() {
        let cancelled = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.page_find.is_current(&request) && state.page_find.invalidate(request.target.tab)
        });
        if cancelled {
            stop_find_host(request.target.tab);
        }
    }
    result
}

fn page_find_reply(tab: i32, identifier: i32, count: i32, ordinal: i32, final_update: bool) {
    let target = STATE.with(|state| state.borrow().page_find.target());
    let Some(target) = target.filter(|target| target.tab == tab) else {
        return;
    };
    if page_find_browser(target).is_err() {
        invalidate_page_find(tab);
        return;
    }
    let plan = STATE.with(|state| {
        state
            .borrow_mut()
            .page_find
            .result(target, identifier, count, ordinal, final_update)
    });
    // AppKit and CEF may both reenter. No state borrow crosses either call.
    #[cfg(not(test))]
    if let Some(update) = plan.update {
        crate::control_panel::page_find_result(
            update.target.tab,
            update.target.epoch,
            update.token,
            update.count,
            update.ordinal,
            update.final_update,
        );
    }
    if let Some(request) = plan.issue {
        if issue_page_find(request).is_err() {
            notify_panel();
        }
    }
}

/// Capture the current main-frame address, never a pending address-bar edit.
pub(crate) fn bookmark_page(tab: i32) -> Result<(String, String), String> {
    let (browser, title) = STATE
        .with(|state| {
            state
                .borrow()
                .tabs
                .get(&tab)
                .filter(|tab| !tab.rejected_creation)
                .map(|tab| (tab.browser.clone(), tab.title.clone()))
        })
        .ok_or_else(|| "현재 탭이 닫혔습니다.".to_owned())?;
    // CEF calls stay outside the state borrow: a native callback may reenter.
    let frame = browser
        .main_frame()
        .ok_or_else(|| "페이지 주소를 읽을 수 없습니다.".to_owned())?;
    let url = CefString::from(&frame.url()).to_string();
    let url = crate::address::normalize_url(&url).map_err(|error| error.to_string())?;
    Ok((url, title))
}

fn is_unowned_user_tab(state: &State, tab: i32) -> bool {
    state.spaces.tab_space(tab) == Some(USER_SPACE) && state.tasks.owner(tab).is_none()
}

fn remember_closed_page(state: &mut State, tab: i32, url: &str) {
    if is_unowned_user_tab(state, tab)
        && state
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
    {
        state.closed_pages.record(url);
    }
}

pub(crate) fn can_reopen_closed_page() -> bool {
    STATE.with(|state| {
        let state = state.borrow();
        state.spaces.active_space() == USER_SPACE
            && !state.spaces.overview()
            && state.closed_pages.can_reopen()
    })
}

/// Reopen a URL in a new user tab; this does not restore a document or session.
pub(crate) fn reopen_closed_page() -> Result<(), String> {
    if crate::control_panel::search_is_open() || !can_reopen_closed_page() {
        return Err("내 공간에서 최근 닫은 탭을 다시 열 수 있습니다.".into());
    }
    let entry = STATE
        .with(|state| state.borrow_mut().closed_pages.begin_reopen())
        .ok_or_else(|| "다시 열 수 있는 탭이 없습니다.".to_owned())?;
    let result =
        create_browser_with_context(&entry.url, None, None, None, Some(USER_SPACE.into()), true)
            .ok_or_else(|| "닫은 탭을 다시 만들 수 없습니다.".to_owned())
            .and_then(|browser| {
                let tab = browser.identifier();
                if !STATE.with(|state| {
                    state
                        .borrow()
                        .tabs
                        .get(&tab)
                        .is_some_and(|tab| !tab.rejected_creation)
                }) {
                    return Err("새 탭을 브라우저 창에 연결할 수 없습니다.".into());
                }
                select_bookmark_tab(tab)
            });
    STATE.with(|state| {
        state
            .borrow_mut()
            .closed_pages
            .finish_reopen(entry.token, result.is_ok())
    });
    notify_panel();
    result
}

fn prefer_bookmark_tab(active: Option<i32>, candidates: impl Iterator<Item = i32>) -> Option<i32> {
    let mut first = None;
    for tab in candidates {
        if Some(tab) == active {
            return Some(tab);
        }
        first.get_or_insert(tab);
    }
    first
}

fn select_bookmark_tab(tab: i32) -> Result<(), String> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !is_unowned_user_tab(&state, tab) {
            return Err("북마크를 열 사용자 탭을 찾을 수 없습니다.".to_owned());
        }
        state.spaces.show(USER_SPACE);
        state.spaces.select_tab(tab);
        Ok(())
    })?;
    sync_shell();
    focus_tab(tab);
    Ok(())
}

/// Native bookmark input opens the user's space without handing off an AI task.
pub(crate) fn open_bookmark_url(url: &str) -> Result<(), String> {
    let url = crate::address::normalize_url(url).map_err(|error| error.to_string())?;
    let existing = STATE.with(|state| {
        let state = state.borrow();
        // BTreeMap iteration supplies the stable first match. A failed renderer
        // or navigation cannot provide a usable existing bookmark destination.
        prefer_bookmark_tab(
            state.spaces.active_tab(),
            state.tabs.iter().filter_map(|(&id, tab)| {
                (is_unowned_user_tab(&state, id)
                    && !tab.rejected_creation
                    && tab.navigation.can_reuse_bookmark(&url))
                .then_some(id)
            }),
        )
    });
    if let Some(tab) = existing {
        return select_bookmark_tab(tab);
    }
    let browser =
        create_browser_with_context(&url, None, None, None, Some(USER_SPACE.into()), true)
            .ok_or_else(|| "북마크를 열 새 탭을 만들 수 없습니다.".to_owned())?;
    let tab = browser.identifier();
    if !STATE.with(|state| {
        state
            .borrow()
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
    }) {
        return Err("새 탭을 브라우저 창에 연결할 수 없습니다.".into());
    }
    select_bookmark_tab(tab)
}

pub fn shell_action(action: ShellAction) -> Result<(), String> {
    match &action {
        ShellAction::SelectSearchTab(tab) => {
            // Search rows can outlive a move or close. Resolve the live mapping
            // before changing either the displayed space or its selected tab.
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                let space = state
                    .spaces
                    .tab_space(*tab)
                    .ok_or_else(|| "The tab has closed.".to_owned())?
                    .to_owned();
                if !state.spaces.show(&space) || !state.spaces.select_tab(*tab) {
                    return Err("The tab is no longer available.".to_owned());
                }
                Ok(())
            })?;
            sync_shell();
            focus_tab(*tab);
            return Ok(());
        }
        ShellAction::SelectSpace(space) => {
            if !STATE.with(|state| state.borrow_mut().spaces.show(space)) {
                return Err("The space is no longer available.".into());
            }
            sync_shell();
            if let Some(tab) = STATE.with(|state| state.borrow().spaces.active_tab()) {
                focus_tab(tab);
            }
            return Ok(());
        }
        ShellAction::SelectTab(tab) => {
            if !STATE.with(|state| state.borrow_mut().spaces.select_tab(*tab)) {
                return Err("The tab is not in the current space.".into());
            }
            sync_shell();
            focus_tab(*tab);
            return Ok(());
        }
        ShellAction::ShowOverview(overview) => {
            STATE.with(|state| state.borrow_mut().spaces.set_overview(*overview));
            sync_shell();
            return Ok(());
        }
        ShellAction::StopSpace(scope) => return handoff_native(scope.clone()),
        ShellAction::Stop(tab, scope) => {
            STATE
                .with(|state| state.borrow().tasks.check_tab(Some(scope), *tab))
                .map_err(|error| error.to_string())?;
            return handoff_native(scope.clone());
        }
        ShellAction::NewTab => {
            let space = STATE.with(|state| state.borrow().spaces.active_space().to_owned());
            // Explicit native chrome input takes control before creating a human tab.
            handoff_space_if_active(&space)?;
            STATE.with(|state| state.borrow_mut().spaces.set_overview(false));
            create_browser_with_context("about:blank", None, None, None, Some(space), true)
                .ok_or_else(|| "Cannot create a tab.".to_owned())?;
            crate::control_panel::focus_address();
            return Ok(());
        }
        _ => {}
    }
    let command = match action {
        ShellAction::Back(tab) => Command::Back { tab },
        ShellAction::Forward(tab) => Command::Forward { tab },
        ShellAction::Reload(tab) => Command::Reload { tab },
        ShellAction::Navigate(tab, url) => Command::Navigate { tab, url },
        ShellAction::CloseTab(tab) => Command::Close { tab },
        _ => unreachable!(),
    };
    let id = match &command {
        Command::Back { tab }
        | Command::Forward { tab }
        | Command::Reload { tab }
        | Command::Navigate { tab, .. }
        | Command::Close { tab } => *tab,
        _ => unreachable!(),
    };
    let owner = STATE.with(|state| state.borrow().tasks.owner(id).map(|(task, _)| task));
    if let Some(owner) = owner {
        handoff_space_if_active(&owner)?;
    }
    let browser = STATE
        .with(|state| {
            state
                .borrow()
                .tabs
                .get(&id)
                .filter(|tab| !tab.rejected_creation)
                .map(|tab| tab.browser.clone())
        })
        .ok_or("The tab has closed.")?;
    if matches!(command, Command::Reload { .. }) && browser.is_loading() != 0 {
        stop_loading(&browser);
        notify_panel();
        return Ok(());
    }
    // These are explicit human actions in trusted native chrome. A paused task
    // must not prevent its user from browsing. CLI authorization stays in handle.
    if matches!(command, Command::Close { .. }) {
        browser
            .host()
            .ok_or("The tab has no host.")?
            .close_browser(0);
        return Ok(());
    }
    let (reply, receiver) = native_reply();
    navigate(&browser, &command, reply);
    notify_panel();
    native_result(receiver)
}

#[cfg(not(test))]
static PANEL_REFRESH_QUEUED: AtomicBool = AtomicBool::new(false);

#[cfg(not(test))]
wrap_task! {
    struct PanelRefreshTask;
    impl Task {
        fn execute(&self) {
            PANEL_REFRESH_QUEUED.store(false,Ordering::Release);
            crate::control_panel::refresh();
        }
    }
}

pub(crate) fn notify_panel() {
    #[cfg(not(test))]
    if !PANEL_REFRESH_QUEUED.swap(true, Ordering::AcqRel) {
        let mut task = PanelRefreshTask::new();
        // Coalesce event bursts; this is not a recurring polling timer.
        if post_delayed_task(ThreadId::UI, Some(&mut task), 100) == 0 {
            PANEL_REFRESH_QUEUED.store(false, Ordering::Release);
        }
    }
}

struct PanelRefresh(bool);
impl Drop for PanelRefresh {
    fn drop(&mut self) {
        if self.0 {
            notify_panel();
        }
    }
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

pub fn focus_tab(id: i32) {
    let browser = STATE.with(|state| state.borrow().tabs.get(&id).map(|tab| tab.browser.clone()));
    if let Some(host) = browser.and_then(|browser| browser.host()) {
        host.set_focus(1);
    }
}

fn tab_info(tab: &Tab) -> Json {
    json!({"tab":tab.browser.identifier(),"title":tab.title,
    "url":tab.browser.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default(),
    "loading":tab.browser.is_loading()!=0,
    "canGoBack":tab.browser.can_go_back()!=0,"canGoForward":tab.browser.can_go_forward()!=0,
    "renderer":{"status":tab.navigation.renderer_status(),"error":tab.navigation.renderer_exit},
    "navigation": {
        "epoch":tab.navigation.epoch,"requestedUrl":tab.navigation.requested_url,
        "provisional":tab.navigation.provisional,"committed":tab.navigation.committed,
        "documentAvailable":tab.navigation.document_available,
        "mainLoadComplete":tab.navigation.main_load_complete,"error":tab.navigation.error,
        "stopped":tab.navigation.stopped,"incomplete":tab.navigation.incomplete
    }})
}

pub fn start_server(listener: UnixListener, stopped: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
        let mut token = 0u64;
        for connection in listener.incoming() {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let Ok(stream) = connection else {
                continue;
            };
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    let _ = workers.swap_remove(index).join();
                } else {
                    index += 1;
                }
            }
            // Bound local client resources. Excess connections are disconnected;
            // requests already accepted still have independent deadlines.
            if workers.len() >= MAX_CONNECTIONS {
                continue;
            }
            token += 1;
            let stopped = stopped.clone();
            workers.push(thread::spawn(move || {
                serve_connection(stream, stopped, token)
            }));
        }
        for worker in workers {
            let _ = worker.join();
        }
    })
}

fn serve_connection(mut stream: UnixStream, stopped: Arc<AtomicBool>, token: u64) {
    let request = match protocol::read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = protocol::write_response(
                &mut stream,
                &Response::error("invalid", error.code, error.message),
            );
            return;
        }
    };
    let id = request.id.clone();
    let active = Arc::new(AtomicBool::new(true));
    let (sender, receiver) = mpsc::channel();
    let mut task = CommandTask::new(
        request,
        Reply {
            id: id.clone(),
            token,
            active: active.clone(),
            sender,
            scope: None,
        },
    );
    if stopped.load(Ordering::Acquire) || post_task(ThreadId::UI, Some(&mut task)) == 0 {
        let _ = protocol::write_response(
            &mut stream,
            &Response::error(id, "APP_NOT_READY", "The browser event loop is not ready."),
        );
        return;
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
            active.store(false, Ordering::Release);
            break Response::error(&id, "APP_STOPPING", "The browser is shutting down.")
                .with_details(json!({"outcomeUnknown":true}));
        }
        if Instant::now() < deadline {
            continue;
        }
        active.store(false, Ordering::Release);
        let mut cancel = CancelTask::new(token);
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

wrap_task! {
    struct CommandTask { request: Request, reply: Reply }
    impl Task {
        fn execute(&self) {
            if self.reply.active.load(Ordering::Acquire) {
                handle(&self.request.command, self.reply.clone());
            }
        }
    }
}
wrap_task! {
    struct CancelTask { token: u64 }
    impl Task {
        fn execute(&self) { remove_wait(self.token); }
    }
}
wrap_task! {
    struct WaitTask { token: u64 }
    impl Task { fn execute(&self) { poll_wait(self.token); } }
}

wrap_task! {
    struct OpenLinkTask { url: String, link: LinkCreation, reservation: Option<u64> }
    impl Task {
        fn execute(&self) {
            let current = STATE.with(|state| {
                let state = state.borrow();
                self.reservation.is_none_or(|ticket| state.creations.contains(ticket)) && link_is_current(&state, &self.link)
            });
            if !current || create_browser_with_link(&self.url, self.link.scope(), self.reservation, Some(self.link.clone())).is_none() {
                if let Some(ticket) = self.reservation {
                    STATE.with(|state| { state.borrow_mut().creations.release(ticket); });
                }
            }
            notify_panel();
        }
    }
}

fn remove_wait(token: u64) -> Option<Waiter> {
    let waiter = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state
            .pending
            .retain(|_, pending| pending.reply.token != token);
        state.waits.remove(&token)
    });
    if waiter.is_some() {
        notify_panel();
    }
    waiter
}

fn poll_wait(token: u64) {
    let probe = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let waiter = state.waits.get(&token)?;
        if let Err(error) = state.tasks.check_tab(waiter.reply.scope.as_ref(), waiter.tab) {
            waiter.reply.error(error.code, error.message);
            return None;
        }
        let Some(tab) = state.tabs.get(&waiter.tab) else {
            waiter.reply.error("TAB_NOT_FOUND", "No browser tab has that ID.");
            return None;
        };
        let info = tab_info(tab);
        if tab.navigation.renderer_exit.is_some() && !tab.navigation.recovering {
            waiter.reply.error_details("RENDERER_TERMINATED", "The renderer stopped. Reload the tab to recover.",
                json!({"tab":waiter.tab,"state":info,"outcomeUnknown":false}));
            return None;
        }
        if tab.navigation.stopped || tab.navigation.incomplete {
            let code = if tab.navigation.stopped { "NAVIGATION_STOPPED" } else { "NAVIGATION_INCOMPLETE" };
            waiter.reply.error_details(code, "Navigation ended without reaching the requested page readiness.",
                json!({"tab":waiter.tab,"state":info}));
            return None;
        }
        if tab.navigation.error.is_some() {
            waiter.reply.error_details("NAVIGATION_FAILED", "The main-frame navigation failed.",
                json!({"tab":waiter.tab,"state":info}));
            return None;
        }
        if waiter.started.elapsed() >= Duration::from_millis(waiter.timeout_ms) {
            waiter.reply.error_details("WAIT_TIMEOUT", "The requested page readiness was not reached.",
                json!({"tab":waiter.tab,"until":waiter.until,"timeoutMs":waiter.timeout_ms,
                    "elapsedMs":waiter.started.elapsed().as_millis(),"state":info,"navigationContinues":true}));
            return None;
        }
        let usable = tab.navigation.committed && tab.navigation.document_available && !tab.navigation.provisional;
        if usable && waiter.until == WaitUntil::Load && tab.browser.is_loading() == 0 {
            waiter.reply.ok(info);
            return None;
        }
        if usable && waiter.until == WaitUntil::Dom && !waiter.probe_in_flight {
            let browser = tab.browser.clone();
            let pending = Pending { tab:waiter.tab, epoch:tab.navigation.epoch,
                evaluation:Evaluation::ReadyState {token}, reply:waiter.reply.clone(),
                step:Step::Frame("document.readyState".into()) };
            state.waits.get_mut(&token).unwrap().probe_in_flight = true;
            Some((browser, pending))
        } else { None }
    });
    let active = STATE.with(|state| {
        state
            .borrow()
            .waits
            .get(&token)
            .is_some_and(|waiter| waiter.reply.active.load(Ordering::Acquire))
    });
    if !active {
        remove_wait(token);
        return;
    }
    if let Some((browser, pending)) = probe {
        dispatch(&browser, "Page.getFrameTree", json!({}), pending);
    }
    // A separate deadline tick can finish even if the renderer never answers.
    // At most one CDP probe is in flight for each wait.
    let mut task = WaitTask::new(token);
    if post_delayed_task(ThreadId::UI, Some(&mut task), 50) == 0 {
        if let Some(waiter) = remove_wait(token) {
            waiter
                .reply
                .error("APP_STOPPING", "The browser cannot continue waiting.");
        }
    }
}

fn pending_error(pending: &Pending, code: &str, message: &str) {
    if let Evaluation::ReadyState { token } = pending.evaluation {
        STATE.with(|state| {
            if let Some(waiter) = state.borrow_mut().waits.get_mut(&token) {
                waiter.probe_in_flight = false;
            }
        });
    } else {
        pending.reply.error(code, message);
    }
}

fn control_detached(tab: i32) {
    let pending = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.captures.retain(|_, capture_tab| *capture_tab != tab);
        state.previews.retain(|_, request| request.tab != tab);
        let ids = state
            .pending
            .iter()
            .filter_map(|(id, request)| (request.tab == tab).then_some(*id))
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>()
    });
    for request in pending {
        if matches!(request.step, Step::Evaluate)
            && matches!(request.evaluation, Evaluation::Page { mutation: true })
        {
            request.reply.error_details(
                "CONTROL_DISCONNECTED",
                "The page disconnected during an action. Inspect before repeating it.",
                json!({"tab":tab,"outcomeUnknown":true}),
            );
        } else {
            // Readiness probes can resume; page actions are never repeated.
            pending_error(
                &request,
                "CONTROL_DISCONNECTED",
                "The page control connection was lost.",
            );
        }
    }
    notify_panel();
}

fn renderer_terminated(tab: i32, reason: Json) {
    crate::download_permission_runtime::cancel_tab(tab);
    let (pending, waits) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.page_find.invalidate(tab);
        if let Some(tab) = state.tabs.get_mut(&tab) {
            tab.navigation.renderer_failed(reason.clone());
        }
        // A renderer failure cancels the caller, not necessarily the browser's
        // native capture. Release that slot on its result, detach, or close.
        let pending_ids = state
            .pending
            .iter()
            .filter_map(|(id, request)| (request.tab == tab).then_some(*id))
            .collect::<Vec<_>>();
        let wait_ids = state
            .waits
            .iter()
            .filter_map(|(id, waiter)| (waiter.tab == tab).then_some(*id))
            .collect::<Vec<_>>();
        let pending = pending_ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        let waits = wait_ids
            .into_iter()
            .filter_map(|id| state.waits.remove(&id))
            .collect::<Vec<_>>();
        (pending, waits)
    });
    for request in pending {
        let unknown = matches!(request.step, Step::Evaluate)
            && matches!(request.evaluation, Evaluation::Page { mutation: true });
        request.reply.error_details(
            "RENDERER_TERMINATED",
            "The renderer stopped. Inspect the tab before retrying an action.",
            json!({"tab":tab,"renderer":reason,"outcomeUnknown":unknown}),
        );
    }
    for waiter in waits {
        waiter.reply.error_details(
            "RENDERER_TERMINATED",
            "The renderer stopped while waiting. Reload to recover.",
            json!({"tab":tab,"renderer":reason,"outcomeUnknown":false}),
        );
    }
    notify_panel();
}

fn task_result<T: serde::Serialize>(result: Result<T, TaskError>, reply: &Reply) {
    match result {
        Ok(task) => reply.ok(json!({"task":task})),
        Err(error) => reply.error(error.code, error.message),
    }
}

// Revoke work on the UI thread before allowing another controller to resume.
// A dispatched mutation cannot be undone, so its reply records an unknown outcome.
fn revoke_task_requests(task: &str, only_tab: Option<i32>) {
    crate::download_permission_runtime::cancel_task(task, only_tab);
    crate::download_runtime::cancel_task(task, only_tab);
    let matches = |reply: &Reply, tab: i32| {
        reply.scope.as_ref().is_some_and(|scope| scope.task == task)
            && only_tab.is_none_or(|id| id == tab)
    };
    let (pending, waits) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let pending_ids = state
            .pending
            .iter()
            .filter_map(|(id, pending)| matches(&pending.reply, pending.tab).then_some(*id))
            .collect::<Vec<_>>();
        let wait_ids = state
            .waits
            .iter()
            .filter_map(|(id, waiter)| matches(&waiter.reply, waiter.tab).then_some(*id))
            .collect::<Vec<_>>();
        let pending = pending_ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        let waits = wait_ids
            .into_iter()
            .filter_map(|id| state.waits.remove(&id))
            .collect::<Vec<_>>();
        (pending, waits)
    });
    for request in pending {
        let unknown = matches!(request.step, Step::Evaluate)
            && matches!(request.evaluation, Evaluation::Page { mutation: true });
        request.reply.error_details(
            "TASK_CONTROL_LOST",
            "Task control changed. Inspect the page before continuing.",
            json!({"task":task,"tab":request.tab,"outcomeUnknown":unknown}),
        );
    }
    for waiter in waits {
        waiter.reply.error_details(
            "TASK_CONTROL_LOST",
            "Task control changed while waiting.",
            json!({"task":task,"tab":waiter.tab,"outcomeUnknown":false}),
        );
    }
}

fn expire_tab_refs(tabs: impl IntoIterator<Item = i32>) {
    let tabs = tabs.into_iter().collect::<Vec<_>>();
    let stopped = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let mut stopped = None;
        for &id in &tabs {
            if state.page_find.invalidate(id) {
                stopped = state.tabs.get(&id).map(|tab| tab.browser.clone());
            }
            if let Some(tab) = state.tabs.get_mut(&id) {
                tab.navigation.epoch += 1;
                tab.navigation.download_permission_provenance.revoke();
            }
        }
        stopped
    });
    for id in tabs {
        crate::download_permission_runtime::cancel_tab(id);
    }
    if let Some(host) = stopped.and_then(|browser| browser.host()) {
        host.stop_finding(1);
    }
}

fn handle_task(command: &Command, reply: Reply) {
    match command {
        Command::TaskCreate { name } => task_result(
            STATE.with(|state| state.borrow_mut().tasks.create(name)),
            &reply,
        ),
        Command::TaskList => {
            reply.ok(STATE.with(|state| json!({"tasks":state.borrow().tasks.list()})))
        }
        Command::TaskInfo { task } => {
            task_result(STATE.with(|state| state.borrow().tasks.info(task)), &reply)
        }
        Command::TaskRun {
            task,
            revision,
            command,
        } => {
            if command.is_task_command() {
                reply.error("INVALID_ARGUMENTS", "Task commands cannot be nested.");
                return;
            }
            handle(
                command,
                Reply {
                    scope: Some(TaskScope {
                        task: task.clone(),
                        revision: *revision,
                    }),
                    ..reply
                },
            );
        }
        Command::TaskHandoff { task, revision } => {
            let scope = TaskScope {
                task: task.clone(),
                revision: *revision,
            };
            let result = STATE.with(|state| state.borrow_mut().tasks.handoff(&scope));
            if let Ok(view) = &result {
                revoke_task_requests(task, None);
                expire_tab_refs(view.tabs.iter().map(|tab| tab.tab));
            }
            task_result(result, &reply);
        }
        Command::TaskResume { task, revision } => {
            let scope = TaskScope {
                task: task.clone(),
                revision: *revision,
            };
            let result = STATE.with(|state| state.borrow_mut().tasks.resume(&scope));
            if let Ok(view) = &result {
                // Human find results and queued steps must not survive the
                // transfer back to AI control. The epoch also closes the UI.
                expire_tab_refs(view.tabs.iter().map(|tab| tab.tab));
            }
            task_result(result, &reply);
        }
        Command::TaskAdopt {
            task,
            revision,
            tab,
        } => {
            let scope = TaskScope {
                task: task.clone(),
                revision: *revision,
            };
            let result = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if !state.tabs.contains_key(tab) {
                    return Err(TaskError {
                        code: "TAB_NOT_FOUND",
                        message: "No browser tab has that ID.",
                    });
                }
                if state.tabs[tab].rejected_creation {
                    return Err(TaskError {
                        code: "TAB_CLOSING",
                        message: "This task window was rejected and is closing.",
                    });
                }
                // Do not transfer a tab while an unscoped CLI request could still act on it.
                if state.pending.values().any(|pending| pending.tab == *tab)
                    || state.waits.values().any(|waiter| waiter.tab == *tab)
                {
                    return Err(TaskError {
                        code: "TAB_BUSY",
                        message: "Wait for existing requests before adopting a tab.",
                    });
                }
                state.tasks.check_capacity(&scope)?;
                if state.tasks.info(&scope.task)?.tabs.len() + state.creations.task_reserved(&scope.task)
                    >= crate::tasks::MAX_TASK_TABS {
                    return Err(TaskError { code:"TASK_TAB_LIMIT", message:"Wait for pending windows or release a task tab before adopting another tab." });
                }
                state.tasks.attach(&scope, *tab, Origin::Borrowed)
            });
            if result.is_ok() {
                STATE.with(|state| state.borrow_mut().spaces.adopt(*tab, task));
                expire_tab_refs([*tab]);
            }
            task_result(result, &reply);
        }
        Command::TaskRelease {
            task,
            revision,
            tab,
        } => {
            let scope = TaskScope {
                task: task.clone(),
                revision: *revision,
            };
            let result = STATE.with(|state| state.borrow_mut().tasks.release(&scope, *tab));
            if result.is_ok() {
                STATE.with(|state| state.borrow_mut().spaces.release(*tab));
                revoke_task_requests(task, Some(*tab));
                expire_tab_refs([*tab]);
            }
            task_result(result, &reply);
        }
        Command::TaskFinish {
            task,
            revision,
            keep,
        } => {
            let scope = TaskScope {
                task: task.clone(),
                revision: *revision,
            };
            let tabs = STATE.with(|state| state.borrow().tasks.info(task).map(|view| view.tabs));
            let plan = STATE.with(|state| state.borrow_mut().tasks.finish(&scope, keep));
            match plan {
                Err(error) => reply.error(error.code, error.message),
                Ok(plan) => {
                    revoke_task_requests(task, None);
                    if let Ok(tabs) = tabs {
                        STATE.with(|state| {
                            state.borrow_mut().spaces.finish(
                                &tabs
                                    .iter()
                                    .map(|tab| (tab.tab, tab.origin))
                                    .collect::<Vec<_>>(),
                                keep,
                            )
                        });
                        expire_tab_refs(tabs.into_iter().map(|tab| tab.tab));
                    }
                    let browsers = STATE.with(|state| {
                        let state = state.borrow();
                        plan.close
                            .iter()
                            .filter_map(|id| state.tabs.get(id).map(|tab| tab.browser.clone()))
                            .collect::<Vec<_>>()
                    });
                    // A beforeunload prompt can postpone closing. Only on_before_close
                    // marks the tab closed and the task finished; never force a close.
                    reply.ok(json!({"task":plan.task,"closeRequested":plan.close}));
                    for browser in browsers {
                        if let Some(host) = browser.host() {
                            host.close_browser(0);
                        }
                    }
                }
            }
        }
        _ => reply.error("INVALID_ARGUMENTS", "Expected a task command."),
    }
}

fn status_json() -> Json {
    let (active_downloads, awaiting_save) = crate::download_runtime::activity();
    let awaiting_permission = crate::download_permission_runtime::pending_count();
    STATE.with(|state| {
        let state = state.borrow();
        json!({"app":"Light Browser","engine":"CEF/Chromium","cef":String::from_utf8_lossy(&sys::CEF_VERSION[..sys::CEF_VERSION.len()-1]),"cefApi":sys::CEF_API_VERSION_LAST,"version":env!("CARGO_PKG_VERSION"),"tabs":state.tabs.len(),"pendingWaits":state.waits.len(),"sandbox":true,"externalDebugPort":false,"controlPanel":"embedded","windowMode":"single","activeTab":state.spaces.active_tab(),"activeSpace":state.spaces.active_space(),"overview":state.spaces.overview(),
            "downloads":{"active":active_downloads,"awaitingSave":awaiting_save,"awaitingPermission":awaiting_permission},
            "resources":{"automationTabLimit":MAX_AUTOMATION_TABS,"taskTabLimit":crate::tasks::MAX_TASK_TABS,"pendingTabCreations":state.creations.len(),"captureLimit":1,"capturesInFlight":state.captures.len()}})
    })
}

fn handle(command: &Command, reply: Reply) {
    let observed = match command {
        Command::TaskRun { command, .. } => command.as_ref(),
        other => other,
    };
    let _refresh = PanelRefresh(!matches!(
        observed,
        Command::Status
            | Command::Spaces
            | Command::Tabs
            | Command::Read { .. }
            | Command::TaskList
            | Command::TaskInfo { .. }
    ));
    if command.is_task_command() {
        handle_task(command, reply);
        update_space_tasks();
        sync_shell();
        return;
    }
    let access = STATE.with(|state| {
        let state = state.borrow();
        if let Some(scope) = &reply.scope {
            state.tasks.check(scope)?;
        }
        if let Some(tab) = command.tab_id() {
            if state
                .tabs
                .get(&tab)
                .is_some_and(|tab| tab.rejected_creation)
            {
                return Err(TaskError {
                    code: "TAB_CLOSING",
                    message: "This task window was rejected and is closing.",
                });
            }
            state.tasks.check_tab(reply.scope.as_ref(), tab)?;
        }
        Ok::<(), TaskError>(())
    });
    if let Err(error) = access {
        reply.error(error.code, error.message);
        return;
    }
    match command {
        Command::Spaces => reply.ok(spaces_json(reply.scope.as_ref())),
        Command::SpaceShow { space } => {
            if reply.scope.is_some() {
                reply.error(
                    "SPACE_UI_REQUIRES_UNSCOPED",
                    "Task commands cannot switch the user's visible space.",
                );
                return;
            }
            if !STATE.with(|state| state.borrow().spaces.contains(space)) {
                reply.error("SPACE_NOT_FOUND", "No space has that ID.");
                return;
            }
            if let Err(error) = shell_action(ShellAction::SelectSpace(space.clone())) {
                reply.error("SPACE_UNAVAILABLE", &error);
                return;
            }
            crate::control_panel::show();
            reply.ok(spaces_json(None));
        }
        Command::SpaceOverview => {
            if reply.scope.is_some() {
                reply.error(
                    "SPACE_UI_REQUIRES_UNSCOPED",
                    "Task commands cannot switch the user's visible space.",
                );
                return;
            }
            let _ = shell_action(ShellAction::ShowOverview(true));
            crate::control_panel::show();
            reply.ok(spaces_json(None));
        }
        Command::Panel => {
            crate::control_panel::show();
            reply.ok(json!({"panel":"embedded","shown":true,"windowMode":"single"}));
        }
        Command::Status => reply.ok(status_json()),
        Command::Tabs => reply.ok(STATE.with(|state| {
            let state = state.borrow();
            let tabs =
                state
                    .tabs
                    .values()
                    .filter_map(|tab| {
                        if tab.rejected_creation {
                            return None;
                        }
                        let owner = state.tasks.owner(tab.browser.identifier());
                        if reply.scope.as_ref().is_some_and(|scope| {
                            owner.as_ref().is_none_or(|(id, _)| *id != scope.task)
                        }) {
                            return None;
                        }
                        let mut info = tab_info(tab);
                        info["active"] =
                            json!(state.spaces.active_tab() == Some(tab.browser.identifier()));
                        info["space"] = json!(
                            state
                                .spaces
                                .tab_space(tab.browser.identifier())
                                .unwrap_or(USER_SPACE)
                        );
                        info["task"] = owner
                            .map(|(id, origin)| json!({"id":id,"origin":origin}))
                            .unwrap_or(Json::Null);
                        Some(info)
                    })
                    .collect::<Vec<_>>();
            json!({"tabs":tabs})
        })),
        Command::Open { url } => {
            if !valid_url(url) {
                reply.error(
                    "INVALID_URL",
                    "Only an absolute HTTP(S) URL without control characters is accepted.",
                );
                return;
            }
            let reservation = match reserve_creation(reply.scope.clone(), None) {
                Ok(ticket) => ticket,
                Err(error) => {
                    reply.error(error.code, error.message);
                    return;
                }
            };
            if let Some(browser) = create_browser(url, reply.scope.clone(), Some(reservation)) {
                let info = STATE.with(|state| {
                    state
                        .borrow()
                        .tabs
                        .get(&browser.identifier())
                        .filter(|tab| !tab.rejected_creation)
                        .map(tab_info)
                });
                if let Some(info) = info {
                    reply.ok(info);
                } else {
                    reply.error("OPEN_FAILED", "The created tab is unavailable.");
                }
            } else {
                STATE.with(|state| {
                    state.borrow_mut().creations.release(reservation);
                });
                reply.error("OPEN_FAILED", "Chromium could not create a browser tab.");
            }
        }
        other => {
            let tab_id = match other {
                Command::Read { tab }
                | Command::Click { tab, .. }
                | Command::Fill { tab, .. }
                | Command::Select { tab, .. }
                | Command::Press { tab, .. }
                | Command::Close { tab }
                | Command::Screenshot { tab, .. }
                | Command::Wait { tab, .. }
                | Command::Navigate { tab, .. }
                | Command::Back { tab }
                | Command::Forward { tab }
                | Command::Reload { tab }
                | Command::Stop { tab }
                | Command::Scroll { tab, .. } => *tab,
                _ => unreachable!(),
            };
            let browser = STATE.with(|state| {
                state
                    .borrow()
                    .tabs
                    .get(&tab_id)
                    .map(|tab| tab.browser.clone())
            });
            let Some(browser) = browser else {
                reply.error("TAB_NOT_FOUND", "No browser tab has that ID.");
                return;
            };
            if !matches!(
                other,
                Command::Navigate { .. }
                    | Command::Back { .. }
                    | Command::Forward { .. }
                    | Command::Reload { .. }
                    | Command::Close { .. }
                    | Command::Stop { .. }
                    | Command::Wait { .. }
            ) {
                let unavailable = STATE.with(|state| {
                    let state = state.borrow();
                    let tab = &state.tabs[&tab_id];
                    tab.navigation.renderer_exit.as_ref().map(|_| tab_info(tab))
                });
                if let Some(info) = unavailable {
                    reply.error_details(
                        "RENDERER_TERMINATED",
                        "The renderer has not recovered. Reload, then wait for the new document.",
                        json!({"tab":tab_id,"state":info,"outcomeUnknown":false}),
                    );
                    return;
                }
            }
            if reply.scope.is_some()
                && !matches!(other, Command::Wait { .. } | Command::Screenshot { .. })
            {
                let busy = STATE.with(|state| {
                    state.borrow().pending.values().any(|pending| {
                        pending.tab == tab_id
                            && matches!(pending.evaluation, Evaluation::Page { .. })
                    })
                });
                if busy {
                    reply.error("TAB_BUSY", "A page operation is still in flight. Inspect its result before continuing.");
                    return;
                }
            }
            match other {
                Command::Navigate { .. }
                | Command::Back { .. }
                | Command::Forward { .. }
                | Command::Reload { .. } => {
                    navigate(&browser, other, reply);
                }
                Command::Stop { .. } => {
                    stop_loading(&browser);
                    reply.ok(json!({"tab":tab_id,"stopRequested":true}));
                }
                Command::Wait {
                    until, timeout_ms, ..
                } => {
                    if !(1..=30_000).contains(timeout_ms) {
                        reply.error(
                            "INVALID_ARGUMENTS",
                            "Wait timeout must be 1 to 30000 milliseconds.",
                        );
                        return;
                    }
                    let token = reply.token;
                    STATE.with(|state| {
                        state.borrow_mut().waits.insert(
                            token,
                            Waiter {
                                tab: tab_id,
                                until: *until,
                                timeout_ms: *timeout_ms,
                                started: Instant::now(),
                                probe_in_flight: false,
                                reply,
                            },
                        );
                    });
                    poll_wait(token);
                }
                Command::Close { .. } => {
                    reply.ok(json!({"tab":tab_id,"closeRequested":true}));
                    if let Some(host) = browser.host() {
                        host.close_browser(0);
                    }
                }
                Command::Screenshot { path, .. } => {
                    if !Path::new(path).is_absolute() || Path::new(path).exists() {
                        reply.error("INVALID_PATH", "Use an absolute path to a new PNG file.");
                        return;
                    }
                    dispatch(
                        &browser,
                        "Page.captureScreenshot",
                        json!({"format":"png","captureBeyondViewport":false}),
                        Pending {
                            tab: tab_id,
                            epoch: 0,
                            evaluation: Evaluation::Page { mutation: false },
                            reply,
                            step: Step::Screenshot(path.clone()),
                        },
                    );
                }
                _ => {
                    let epoch = STATE.with(|state| {
                        let state = state.borrow();
                        let tab = state.tabs.get(&tab_id)?;
                        if tab.navigation.provisional
                            || !tab.navigation.committed
                            || !tab.navigation.document_available
                        {
                            reply.error_details(
                                "PAGE_LOADING",
                                "The current main document is not available yet.",
                                json!({"tab":tab_id,"state":tab_info(tab)}),
                            );
                            None
                        } else {
                            Some(tab.navigation.epoch)
                        }
                    });
                    let Some(epoch) = epoch else {
                        return;
                    };
                    let invocation = match other {
                        Command::Read { .. } => "window.__lightBrowserBridge.read()".to_owned(),
                        Command::Click { reference, .. } => {
                            action_expression("click", reference, None, epoch, &reply)
                        }
                        Command::Fill {
                            reference, text, ..
                        } => action_expression(
                            "fill",
                            reference,
                            Some(("text", text)),
                            epoch,
                            &reply,
                        ),
                        Command::Select {
                            reference, value, ..
                        } => action_expression(
                            "select",
                            reference,
                            Some(("value", value)),
                            epoch,
                            &reply,
                        ),
                        Command::Press { reference, key, .. } => {
                            if key != "Enter" {
                                reply.error(
                                    "UNSUPPORTED_KEY",
                                    "The first version supports Enter only.",
                                );
                                return;
                            }
                            action_expression("press", reference, Some(("key", key)), epoch, &reply)
                        }
                        Command::Scroll { x, y, .. } => {
                            if !(-10000..=10000).contains(x) || !(-10000..=10000).contains(y) {
                                reply.error(
                                    "INVALID_SCROLL",
                                    "Scroll offsets must be between -10000 and 10000 CSS pixels.",
                                );
                                return;
                            }
                            format!(
                                "window.__lightBrowserBridge.act({})",
                                json!({"action":"scroll","x":x,"y":y})
                            )
                        }
                        _ => unreachable!(),
                    };
                    if invocation.is_empty() {
                        return;
                    }
                    let expression = format!("(()=>{{{DOM_BRIDGE}\nreturn {invocation};}})()");
                    dispatch(
                        &browser,
                        "Page.getFrameTree",
                        json!({}),
                        Pending {
                            tab: tab_id,
                            epoch,
                            evaluation: Evaluation::Page {
                                mutation: !matches!(other, Command::Read { .. }),
                            },
                            reply,
                            step: Step::Frame(expression),
                        },
                    );
                }
            }
        }
    }
}

fn stop_loading(browser: &Browser) {
    revoke_download_permission_provenance(browser.identifier());
    crate::download_permission_runtime::cancel_tab(browser.identifier());
    invalidate_page_find(browser.identifier());
    STATE.with(|state| {
        if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
            if tab.navigation.provisional || browser.is_loading() != 0 {
                tab.navigation.epoch += 1;
                tab.navigation.stopped = true;
                tab.navigation.stop_pending = true;
            }
        }
    });
    browser.stop_load();
}

fn navigate(browser: &Browser, command: &Command, reply: Reply) {
    let tab_id = browser.identifier();
    let (action, url) = match command {
        Command::Navigate { url, .. } => {
            if !valid_url(url) {
                reply.error(
                    "INVALID_URL",
                    "Only an absolute HTTP(S) URL without control characters is accepted.",
                );
                return;
            }
            ("navigate", Some(url.as_str()))
        }
        Command::Back { .. } => {
            if browser.can_go_back() == 0 {
                reply.error("NO_HISTORY", "No previous history entry.");
                return;
            }
            ("back", None)
        }
        Command::Forward { .. } => {
            if browser.can_go_forward() == 0 {
                reply.error("NO_HISTORY", "No next history entry.");
                return;
            }
            ("forward", None)
        }
        Command::Reload { .. } => ("reload", None),
        _ => unreachable!(),
    };
    // History/reload are browser-level recovery operations and must remain
    // available while a terminated renderer has no usable main frame.
    let frame = browser.main_frame();
    if url.is_some() && frame.is_none() {
        reply.error("FRAME_UNAVAILABLE", "Main frame is unavailable.");
        return;
    }
    let started = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tabs.get_mut(&tab_id) else {
            return false;
        };
        // A single navigation intent avoids attributing a superseded request's
        // late ERR_ABORTED callback to the next request. Stop and wait first.
        let recovering = tab.navigation.renderer_exit.is_some() && !tab.navigation.recovering;
        if !recovering && (tab.navigation.provisional || browser.is_loading() != 0) {
            reply.error_details(
                "NAVIGATION_BUSY",
                "The tab is still loading. Stop it or wait for load before navigating.",
                json!({"tab":tab_id,"state":tab_info(tab)}),
            );
            return false;
        }
        // Set the barrier before CEF can queue any asynchronous callbacks.
        tab.navigation.begin(url);
        true
    });
    if !started {
        return;
    }
    revoke_download_permission_provenance(tab_id);
    crate::download_permission_runtime::cancel_tab(tab_id);
    invalidate_page_find(tab_id);
    match command {
        Command::Navigate { url, .. } => frame
            .unwrap()
            .load_url(Some(&CefString::from(url.as_str()))),
        Command::Back { .. } => browser.go_back(),
        Command::Forward { .. } => browser.go_forward(),
        Command::Reload { .. } => browser.reload(),
        _ => unreachable!(),
    }
    reply.ok(json!({"tab":tab_id,"action":action,"navigationRequested":true}));
}

fn action_expression(
    action: &str,
    reference: &str,
    value: Option<(&str, &String)>,
    epoch: u64,
    reply: &Reply,
) -> String {
    if reference.is_empty() || reference.len() > 256 || reference.chars().any(char::is_control) {
        reply.error(
            "INVALID_REF",
            "Use an element reference from the latest read response.",
        );
        return String::new();
    }
    let prefix = format!("{epoch}/");
    let Some(reference) = reference
        .strip_prefix(&prefix)
        .filter(|value| !value.is_empty())
    else {
        reply.error(
            "STALE_REF",
            "Read the current page to obtain a new element ref.",
        );
        return String::new();
    };
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

fn reserve_creation(
    scope: Option<TaskScope>,
    opener: Option<(i32, i32)>,
) -> Result<u64, TaskError> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let task_tabs = match &scope {
            Some(scope) => {
                state.tasks.check_capacity(scope)?;
                state.tasks.info(&scope.task)?.tabs.len()
            }
            None => 0,
        };
        let existing_tabs = state.tabs.len();
        state
            .creations
            .reserve(existing_tabs, task_tabs, scope, opener)
    })
}

fn create_browser(
    url: &str,
    scope: Option<TaskScope>,
    reservation: Option<u64>,
) -> Option<Browser> {
    create_browser_with_link(url, scope, reservation, None)
}

fn reserve_link_creation(link: &LinkCreation) -> Result<Option<u64>, TaskError> {
    // Match native popups: only task-owned creations consume automation slots.
    link.scope()
        .map(|scope| reserve_creation(Some(scope), None))
        .transpose()
}

fn create_browser_with_link(
    url: &str,
    scope: Option<TaskScope>,
    reservation: Option<u64>,
    link: Option<LinkCreation>,
) -> Option<Browser> {
    let foreground = scope.is_none() && link.as_ref().is_none_or(|link| link.foreground);
    create_browser_with_context(url, scope, reservation, link, None, foreground)
}

fn create_browser_with_context(
    url: &str,
    scope: Option<TaskScope>,
    reservation: Option<u64>,
    link: Option<LinkCreation>,
    space: Option<String>,
    foreground: bool,
) -> Option<Browser> {
    let parent = crate::control_panel::create_parent().ok()?;
    let space = space
        .or_else(|| scope.as_ref().map(|scope| scope.task.clone()))
        .or_else(|| {
            link.as_ref().and_then(|link| {
                STATE.with(|state| {
                    state
                        .borrow()
                        .spaces
                        .tab_space(link.opener)
                        .map(str::to_owned)
                })
            })
        })
        .unwrap_or_else(|| USER_SPACE.into());
    let hidden = i32::from(STATE.with(|state| {
        let state = state.borrow();
        state.spaces.overview() || state.spaces.active_space() != space || !foreground
    }));
    let mut client =
        BrowserClient::new(url.to_owned(), scope, reservation, link, space, foreground);
    browser_host_create_browser_sync(
        Some(&WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            parent_view: parent.cast(),
            bounds: crate::control_panel::browser_bounds(),
            hidden,
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
    if !pending.reply.active.load(Ordering::Acquire) {
        return;
    }
    let access = STATE.with(|state| {
        state
            .borrow()
            .tasks
            .check_tab(pending.reply.scope.as_ref(), pending.tab)
    });
    if let Err(error) = access {
        pending.reply.error(error.code, error.message);
        return;
    }
    let Some(host) = browser.host() else {
        pending_error(&pending, "TAB_CLOSED", "Browser host is unavailable.");
        return;
    };
    let capture = matches!(pending.step, Step::Screenshot(_));
    if capture && STATE.with(|state| !state.borrow().captures.is_empty()) {
        pending.reply.error(
            "CAPTURE_BUSY",
            "Wait for the current screenshot capture to finish.",
        );
        return;
    }
    let mut dictionary = parse_json(
        Some(&CefString::from(params.to_string().as_str())),
        JsonParserOptions::default(),
    )
    .and_then(|value| value.dictionary());
    let id = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.next_message += 1;
        let id = state.next_message;
        if capture {
            state.captures.insert(id, pending.tab);
        }
        state.pending.insert(id, pending);
        id
    });
    if host.execute_dev_tools_method(id, Some(&CefString::from(method)), dictionary.as_mut()) == 0 {
        if let Some(pending) = complete_control_request(id) {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "Chromium rejected the control request.",
            );
        }
    }
}

fn complete_control_request(id: i32) -> Option<Pending> {
    let (pending, capture) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let capture = state.captures.remove(&id).is_some();
        (state.pending.remove(&id), capture)
    });
    if capture {
        notify_panel();
    }
    pending
}

fn devtools_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) {
    if complete_preview(id, success, bytes) {
        return;
    }
    let Some(pending) = complete_control_request(id) else {
        return;
    };
    if !pending.reply.active.load(Ordering::Acquire) {
        return;
    }
    let access = STATE.with(|state| {
        state
            .borrow()
            .tasks
            .check_tab(pending.reply.scope.as_ref(), pending.tab)
    });
    if let Err(error) = access {
        pending.reply.error(error.code, error.message);
        return;
    }
    // Check every preparation step. An action's own navigation is allowed
    // after evaluation has been dispatched, and is not retried.
    let already_dispatched_action = matches!(pending.step, Step::Evaluate)
        && matches!(pending.evaluation, Evaluation::Page { mutation: true });
    if !matches!(pending.step, Step::Screenshot(_)) && !already_dispatched_action {
        let current = STATE.with(|state| {
            state.borrow().tabs.get(&pending.tab).is_some_and(|tab| {
                tab.navigation.epoch == pending.epoch && !tab.navigation.provisional
            })
        });
        if !current {
            pending_error(
                &pending,
                "PAGE_CHANGED",
                "The page navigated during inspection. Read again.",
            );
            return;
        }
    }
    if matches!(pending.step, Step::Screenshot(_)) && bytes.len() > MAX_CAPTURE_RESULT_BYTES {
        pending.reply.error(
            "SCREENSHOT_FAILED",
            "The screenshot result exceeded the size limit.",
        );
        return;
    }
    let Ok(result) = serde_json::from_slice::<Json>(bytes) else {
        pending_error(
            &pending,
            "INVALID_RESULT",
            "Chromium returned invalid JSON.",
        );
        return;
    };
    if !success {
        pending_error(
            &pending,
            "CONTROL_ERROR",
            result["message"]
                .as_str()
                .unwrap_or("Chromium control failed."),
        );
        return;
    }
    match &pending.step {
        Step::Frame(expression) => {
            let Some(frame) = result["frameTree"]["frame"]["id"].as_str() else {
                pending_error(&pending, "FRAME_UNAVAILABLE", "Main frame is unavailable.");
                return;
            };
            dispatch(
                browser,
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"LightBrowserControl","grantUniveralAccess":false}),
                Pending {
                    step: Step::World(expression.clone()),
                    ..pending
                },
            );
        }
        Step::World(expression) => {
            let Some(context) = result["executionContextId"].as_i64() else {
                pending_error(
                    &pending,
                    "CONTEXT_UNAVAILABLE",
                    "Page context is unavailable.",
                );
                return;
            };
            dispatch(
                browser,
                "Runtime.evaluate",
                json!({"expression":expression,"contextId":context,"returnByValue":true,"awaitPromise":false,"timeout":5000}),
                Pending {
                    step: Step::Evaluate,
                    ..pending
                },
            );
        }
        Step::Evaluate => {
            if result.get("exceptionDetails").is_some() {
                pending_error(
                    &pending,
                    "PAGE_SCRIPT_ERROR",
                    "The page changed or could not be inspected. Read again.",
                );
                return;
            }
            let value = &result["result"]["value"];
            if let Evaluation::ReadyState { token } = pending.evaluation {
                let within_deadline = STATE.with(|state| {
                    let state = state.borrow();
                    state.waits.get(&token).is_some_and(|waiter| {
                        waiter.started.elapsed() < Duration::from_millis(waiter.timeout_ms)
                            && state
                                .tabs
                                .get(&waiter.tab)
                                .is_some_and(|tab| tab.navigation.error.is_none())
                    })
                });
                if !within_deadline {
                    // The independent timer returns the error and current tab state.
                    return;
                }
                let ready = matches!(value.as_str(), Some("interactive" | "complete"));
                if ready {
                    if let Some(waiter) = remove_wait(token) {
                        let info =
                            STATE.with(|state| state.borrow().tabs.get(&waiter.tab).map(tab_info));
                        if let Some(mut info) = info {
                            info["readyState"] = value.clone();
                            waiter.reply.ok(info);
                        }
                    }
                } else {
                    pending_error(&pending, "PAGE_LOADING", "The DOM is still parsing.");
                }
                return;
            }
            let reply = &pending.reply;
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
                let mut page = value.clone();
                if matches!(pending.evaluation, Evaluation::Page { mutation: false }) {
                    if let Some(elements) = page["interactive"].as_array_mut() {
                        for element in elements {
                            if let Some(reference) = element["ref"].as_str() {
                                element["ref"] = json!(format!("{}/{}", pending.epoch, reference));
                            }
                        }
                    }
                }
                reply.ok(json!({"tab":pending.tab,"page":page}));
            }
        }
        Step::Screenshot(path) => {
            let reply = &pending.reply;
            let Some(data) = result["data"].as_str() else {
                reply.error("SCREENSHOT_FAILED", "No bounded screenshot was returned.");
                return;
            };
            let saved = save_png_base64(Path::new(path), data);
            match saved {
                Ok(bytes) => reply.ok(json!({"tab":pending.tab,"path":path,"bytes":bytes})),
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
        fn on_context_initialized(&self) {
            // The shell retains its native views before startup temporaries drain.
            objc2::rc::autoreleasepool(|_| {
                if create_browser("about:blank", None, None).is_none() { quit_message_loop(); }
                else { crate::control_panel::show(); }
            });
        }
        fn default_client(&self) -> Option<Client> { Some(BrowserClient::new(String::new(), None, None, None, USER_SPACE.into(), true)) }
    }
}
wrap_client! {
    struct BrowserClient { requested_url: String, scope: Option<TaskScope>, reservation: Option<u64>, link: Option<LinkCreation>, space: String, foreground: bool }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(LifeHandler::new(self.requested_url.clone(), self.scope.clone(), self.reservation, self.link.clone(), self.space.clone(), self.foreground)) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(TitleHandler::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(PageLoadHandler::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(NavigationHandler::new()) }
        fn focus_handler(&self) -> Option<FocusHandler> { Some(TabFocusHandler::new()) }
        fn find_handler(&self) -> Option<FindHandler> { Some(PageFindHandler::new()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(FileDownloadHandler::new()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(FilePermissionHandler::new()) }
    }
}
wrap_life_span_handler! {
    struct LifeHandler { requested_url: String, scope: Option<TaskScope>, reservation: Option<u64>, link: Option<LinkCreation>, space: String, foreground: bool }
    impl LifeSpanHandler {
        fn on_before_popup(&self,browser:Option<&mut Browser>,_frame:Option<&mut Frame>,popup_id:i32,
            target_url:Option<&CefString>,_target_frame_name:Option<&CefString>,target_disposition:WindowOpenDisposition,
            _user_gesture:i32,_popup_features:Option<&PopupFeatures>,window_info:Option<&mut WindowInfo>,
            client:Option<&mut Option<Client>>,_settings:Option<&mut BrowserSettings>,
            _extra_info:Option<&mut Option<DictionaryValue>>,_no_javascript_access:Option<&mut i32>) -> i32 {
            let Some(window_info) = window_info else { return 1; };
            let Ok(parent) = crate::control_panel::create_parent() else { return 1; };
            window_info.parent_view = parent.cast();
            window_info.runtime_style = RuntimeStyle::ALLOY;
            window_info.bounds = crate::control_panel::browser_bounds();
            let opener = browser.as_ref().map(|browser|browser.identifier());
            let scope = STATE.with(|state| {
                let state = state.borrow();
                let owner = opener.and_then(|id| state.tasks.owner(id));
                owner.and_then(|(task,_)| state.tasks.info(&task).ok()).and_then(|task| {
                    // During a handoff, popups belong to the user. Finishing tasks
                    // cannot create extra work after their close plan was fixed.
                    match task.status {
                        Status::Paused => None,
                        _ => Some(TaskScope {task:task.id,revision:task.revision}),
                    }
                })
            });
            let Some(client) = client else { return i32::from(scope.is_some()); };
            let reservation = if scope.is_some() {
                match reserve_creation(scope.clone(), opener.map(|id|(id,popup_id))) {
                    Ok(ticket) => Some(ticket),
                    Err(_) => return 1,
                }
            } else { None };
            let foreground = popup_foreground(target_disposition, scope.is_some());
            let link = opener.map(|opener| STATE.with(|state| {
                let state = state.borrow();
                LinkCreation { opener, owner:link_owner(&state.tasks,opener), foreground }
            }));
            let space = opener.and_then(|opener|STATE.with(|state|state.borrow().spaces.tab_space(opener).map(str::to_owned))).unwrap_or_else(||USER_SPACE.into());
            window_info.hidden = i32::from(!foreground || STATE.with(|state| {
                let state = state.borrow(); state.spaces.overview() || state.spaces.active_space() != space
            }));
            *client = Some(BrowserClient::new(target_url.map(ToString::to_string).unwrap_or_default(),scope,reservation,link,space,foreground));
            0
        }
        fn on_before_popup_aborted(&self, browser:Option<&mut Browser>, popup_id:i32) {
            if let Some(browser) = browser {
                STATE.with(|state| { state.borrow_mut().creations.abort_popup(browser.identifier(),popup_id); });
            }
        }
        fn on_before_dev_tools_popup(&self, _browser:Option<&mut Browser>, window_info:Option<&mut WindowInfo>,
            client:Option<&mut Option<Client>>, _settings:Option<&mut BrowserSettings>,
            _extra_info:Option<&mut Option<DictionaryValue>>, _use_default_window:Option<&mut i32>) {
            // CEF otherwise inherits the opener's already-consumed reservation.
            // Developer tools opened by the user are not task-created windows.
            if let Some(client) = client { *client = Some(BrowserClient::new(String::new(),None,None,None,USER_SPACE.into(),false)); }
            if let (Some(info), Ok(parent)) = (window_info, crate::control_panel::create_parent()) {
                info.parent_view = parent.cast();
                info.runtime_style = RuntimeStyle::ALLOY;
                info.bounds = crate::control_panel::browser_bounds();
            }
        }
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser)=browser {
                let accepted = STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let current = self.link.as_ref().is_none_or(|link| link_is_current(&state, link));
                    let reserved = self.reservation.is_none_or(|ticket|state.creations.release(ticket));
                    let attached = current && reserved && self.scope.as_ref().is_none_or(|scope|state.tasks.attach(scope,browser.identifier(),Origin::Created).is_ok());
                    if attached { state.spaces.insert(browser.identifier(), &self.space, self.foreground); }
                    state.tabs.insert(browser.identifier(),Tab {browser:browser.clone(),title:String::new(),rejected_creation:!attached,_devtools:None,
                        navigation:Navigation { requested_url:self.requested_url.clone(),..Default::default() } });
                    attached
                });
                if !accepted {
                        // Creation may finish after a handoff or finish. This is
                        // a newly created task window, never an adopted user tab.
                        if let Some(host) = browser.host() { host.close_browser(0); }
                        return;
                }
                if let Some(host) = browser.host() {
                    let parent = crate::control_panel::create_parent().expect("The browser shell exists");
                    if let Err(error) = unsafe { crate::control_panel::attach(browser.identifier(), parent, host.window_handle().cast(), self.foreground) } {
                        eprintln!("Cannot attach browser tab: {error}");
                        STATE.with(|state| { if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) { tab.rejected_creation = true; } });
                        host.close_browser(1);
                        return;
                    }
                }
                let mut observer=ControlObserver::new();
                let registration=browser.host().and_then(|host|host.add_dev_tools_message_observer(Some(&mut observer)));
                STATE.with(|state| {if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {tab._devtools=registration;}});
                notify_panel();
            }
        }
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            if let Some(browser) = browser {
                crate::download_permission_runtime::cancel_tab(browser.identifier());
                crate::download_runtime::cancel_tab(browser.identifier());
                invalidate_page_find(browser.identifier());
                // Unload approval has completed. Tear down only this native
                // child view; never send performClose to the shared NSWindow.
                if STATE.with(|state| state.borrow().tabs.len() == 1) {
                    // The final approved tab also releases the native window's
                    // responder hierarchy. OnBeforeClose still owns app exit.
                    crate::control_panel::shutdown();
                    return 1;
                }
                if !crate::control_panel::detach_view(browser.identifier()) {
                    if let Some(host) = browser.host() {
                        unsafe { crate::control_panel::detach_native_view(host.window_handle().cast()); }
                    }
                }
            }
            1
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser)=browser else{return;};
            let id=browser.identifier();
            crate::download_permission_runtime::cancel_tab(id);
            crate::download_runtime::cancel_tab(id);
            // The main frame is valid until this callback returns (CEF 156).
            // Read before the STATE borrow; never retain CEF objects as history.
            let url=browser.main_frame().map(|frame| CefString::from(&frame.url()).to_string());
            let empty=STATE.with(|state| {
                let mut state=state.borrow_mut();
                if let Some(url)=url.as_deref() { remember_closed_page(&mut state,id,url); }
                state.page_find.invalidate(id);
                state.tabs.remove(&id);
                state.creations.close_opener(id);
                state.captures.retain(|_,tab|*tab!=id);
                state.previews.retain(|_,request|request.tab!=id);
                state.tasks.tab_closed(id);
                state.spaces.remove(id);
                let tasks = state.tasks.list();
                state.spaces.update_tasks(&tasks);
                state.waits.retain(|_,waiter| {if waiter.tab==id {waiter.reply.error("TAB_CLOSED","The tab closed while waiting.");false}else{true}});
                state.pending.retain(|_,pending| {if pending.tab==id {pending.reply.error("TAB_CLOSED","The tab closed during the request.");false}else{true}});
                state.tabs.is_empty()
            });
            crate::control_panel::remove(id);
            notify_panel();
            if empty {quit_message_loop();}
        }
    }
}
wrap_download_handler! {
    struct FileDownloadHandler;
    impl DownloadHandler {
        fn can_download(&self, browser:Option<&mut Browser>, _url:Option<&CefString>, _request_method:Option<&CefString>) -> i32 {
            i32::from(browser.is_some_and(|browser| {
                let tab = browser.identifier();
                crate::download_runtime::can_accept(tab) && record_download_permission_attempt(tab).is_ok()
            }))
        }
        fn on_before_download(&self, browser:Option<&mut Browser>, download_item:Option<&mut DownloadItem>, suggested_name:Option<&CefString>, callback:Option<&mut BeforeDownloadCallback>) -> i32 {
            let (Some(browser),Some(item),Some(callback))=(browser,download_item,callback) else{return 0;};
            i32::from(crate::download_runtime::before_download(browser.identifier(),item,suggested_name,callback))
        }
        fn on_download_updated(&self, browser:Option<&mut Browser>, download_item:Option<&mut DownloadItem>, callback:Option<&mut DownloadItemCallback>) {
            if let Some(item)=download_item {
                crate::download_runtime::updated(browser.map(|browser|browser.identifier()),item,callback.as_deref());
            }
        }
    }
}
wrap_permission_handler! {
    struct FilePermissionHandler;
    impl PermissionHandler {
        fn on_show_permission_prompt(&self, browser:Option<&mut Browser>, prompt_id:u64,
            requesting_origin:Option<&CefString>, requested_permissions:u32,
            callback:Option<&mut PermissionPromptCallback>) -> i32 {
            let tab = browser.map(|browser| browser.identifier());
            eprintln!("download permission=show tab={tab:?} prompt={prompt_id} mask={requested_permissions}");
            i32::from(crate::download_permission_runtime::show(tab, prompt_id,
                requesting_origin, requested_permissions, callback.as_deref()))
        }
        fn on_dismiss_permission_prompt(&self, browser:Option<&mut Browser>, prompt_id:u64,
            _result:PermissionRequestResult) {
            crate::download_permission_runtime::dismissed(browser.map(|browser| browser.identifier()), prompt_id);
        }
    }
}
wrap_focus_handler! {
    struct TabFocusHandler;
    impl FocusHandler {
        fn on_set_focus(&self, browser: Option<&mut Browser>, _source: FocusSource) -> i32 {
            // Reading or loading a hidden tab must not steal native keyboard focus.
            i32::from(crate::control_panel::search_is_open() || crate::control_panel::page_find_is_editing() || browser.is_some_and(|browser| STATE.with(|state| {
                let state = state.borrow();
                state.spaces.overview() || state.spaces.active_tab() != Some(browser.identifier())
            })))
        }
    }
}
wrap_find_handler! {
    struct PageFindHandler;
    impl FindHandler {
        fn on_find_result(&self, browser:Option<&mut Browser>, identifier:i32,
            count:i32, _selection_rect:Option<&Rect>, active_match_ordinal:i32, final_update:i32) {
            if let Some(browser) = browser {
                page_find_reply(browser.identifier(), identifier, count, active_match_ordinal, final_update != 0);
            }
        }
    }
}
wrap_display_handler! {
    struct TitleHandler;
    impl DisplayHandler {
        fn on_address_change(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,url:Option<&CefString>) {
            if frame.is_none_or(|frame| frame.is_main()==0) { return; }
            if let Some(browser) = browser { revoke_download_permission_provenance(browser.identifier()); crate::download_permission_runtime::cancel_tab(browser.identifier()); invalidate_page_find(browser.identifier()); STATE.with(|state| {
                if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    let nav = &mut tab.navigation;
                    let url = url.map(ToString::to_string).unwrap_or_default();
                    if !nav.stop_pending { nav.stopped = false; }
                    nav.epoch += 1;
                    nav.requested_url = url;
                    // Only a new main-frame commit clears renderer failure.
                    if nav.renderer_exit.is_some() { return; }
                    // Same-document navigation has no OnLoadStart/OnLoadEnd.
                    // Keep its document, while a cross-document OnLoadStart
                    // clears document availability in the same CEF event turn.
                    if !nav.stopped && nav.error.is_none() {
                        nav.provisional = false;
                        nav.intent_pending = false;
                        nav.incomplete = false;
                    }
                }
            }); notify_panel(); }
        }
        fn on_title_change(&self,browser:Option<&mut Browser>,title:Option<&CefString>) {
            if let Some(browser)=browser {STATE.with(|state| {if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {tab.title=title.map(|title|title.to_string().chars().take(512).collect()).unwrap_or_default();}});notify_panel();}
        }
    }
}
wrap_request_handler! {
    struct NavigationHandler;
    impl RequestHandler {
        fn on_open_urlfrom_tab(&self, browser:Option<&mut Browser>, _frame:Option<&mut Frame>, target_url:Option<&CefString>, target_disposition:WindowOpenDisposition, _user_gesture:i32) -> i32 {
            let Some(foreground) = link_foreground(target_disposition) else { return 0; };
            // Once a new-tab request is recognized, failure must never replace
            // the source document via CEF's default same-tab navigation.
            let (Some(browser), Some(url)) = (browser, target_url) else { return 1; };
            let url = url.to_string();
            if !valid_url(&url) { return 1; }
            let link = STATE.with(|state| {
                let state = state.borrow();
                let opener = browser.identifier();
                state.tabs.get(&opener).filter(|tab| !tab.rejected_creation)
                    .map(|_| LinkCreation { opener, owner:link_owner(&state.tasks, opener), foreground })
            });
            let Some(link) = link else { return 1; };
            let reservation = match reserve_link_creation(&link) {
                Ok(ticket) => ticket,
                Err(_) => return 1,
            };
            // Defer creation outside navigation callbacks. Task-owned links
            // retain their budget slot until accepted or canceled.
            let mut task = OpenLinkTask::new(url, link, reservation);
            if post_task(ThreadId::UI, Some(&mut task)) == 0 {
                if let Some(ticket) = reservation {
                    STATE.with(|state| { state.borrow_mut().creations.release(ticket); });
                }
            }
            1
        }
        fn on_render_process_terminated(&self,browser:Option<&mut Browser>,status:TerminationStatus,error_code:i32,error_string:Option<&CefString>) {
            if let Some(browser) = browser {
                let name = match status {
                    TerminationStatus::ABNORMAL_TERMINATION => "abnormal",
                    TerminationStatus::PROCESS_WAS_KILLED => "killed",
                    TerminationStatus::PROCESS_CRASHED => "crashed",
                    TerminationStatus::PROCESS_OOM => "out_of_memory",
                    TerminationStatus::LAUNCH_FAILED => "launch_failed",
                    TerminationStatus::INTEGRITY_FAILURE => "integrity_failure",
                    _ => "unknown",
                };
                renderer_terminated(browser.identifier(),json!({"status":name,"rawStatus":status.get_raw(),"errorCode":error_code,
                    "message":error_string.map(|value|value.to_string().chars().take(240).collect::<String>()).unwrap_or_default()}));
            }
        }
        fn on_before_browse(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,request:Option<&mut cef::Request>,_user_gesture:i32,_is_redirect:i32) -> i32 {
            if frame.is_some_and(|frame| frame.is_main()!=0) {
                if let Some(browser) = browser {
                    invalidate_page_find(browser.identifier());
                    STATE.with(|state| {
                        if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                            let nav = &mut tab.navigation;
                            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
                            nav.begin(Some(&url));
                            // Preserve the current document until a new one commits.
                            // An aborted request, download or 204 may keep that document.
                        }
                    });
                }
            }
            0
        }
        fn on_document_available_in_main_frame(&self,browser:Option<&mut Browser>) {
            if let Some(browser) = browser {
                STATE.with(|state| {
                    if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                        if tab.navigation.committed && !tab.navigation.provisional {
                            tab.navigation.document_available = true;
                        }
                    }
                });
                notify_panel();
            }
        }
    }
}
wrap_load_handler! {
    struct PageLoadHandler;
    impl LoadHandler {
        fn on_loading_state_change(&self,browser:Option<&mut Browser>,is_loading:i32,_can_go_back:i32,_can_go_forward:i32) {
            if is_loading != 0 { notify_panel(); return; }
            if let Some(browser)=browser {STATE.with(|state| {
                if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    tab.navigation.finish_loading();
                }
            });notify_panel();}
        }
        fn on_load_start(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,_transition_type:TransitionType) {
            let Some(frame) = frame.filter(|frame| frame.is_main()!=0) else { return; };
            let url = CefString::from(&frame.url()).to_string();
            if let Some(browser) = browser { crate::download_permission_runtime::cancel_tab(browser.identifier()); invalidate_page_find(browser.identifier()); STATE.with(|state| {
                if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    // A restored document may commit without OnBeforeBrowse.
                    tab.navigation.commit(&url);
                }
            }); notify_panel(); }
        }
        fn on_load_end(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,_http_status_code:i32) {
            if frame.is_none_or(|frame| frame.is_main()==0) { return; }
            if let Some(browser) = browser { STATE.with(|state| {
                if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    if tab.navigation.committed && !tab.navigation.provisional {
                        tab.navigation.main_load_complete = true;
                        // Includes restored documents that have no new-document callback.
                        tab.navigation.document_available = true;
                    }
                }
            }); notify_panel(); }
        }
        fn on_load_error(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,error_code:Errorcode,error_text:Option<&CefString>,failed_url:Option<&CefString>) {
            if frame.is_none_or(|frame| frame.is_main()==0) { return; }
            if let Some(browser)=browser {STATE.with(|state| {
                if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    if tab.navigation.renderer_exit.is_some() && !tab.navigation.recovering { return; }
                    tab.navigation.provisional = false;
                    if !(tab.navigation.stopped && sys::cef_errorcode_t::from(error_code)==sys::cef_errorcode_t::ERR_ABORTED) {
                        tab.navigation.error = Some(json!({"code":error_code.get_raw(),
                            "text":error_text.map(ToString::to_string).unwrap_or_default(),
                            "failedUrl":failed_url.map(ToString::to_string).unwrap_or_default()}));
                    }
                }
            });notify_panel();}
        }
    }
}
wrap_dev_tools_message_observer! {
    struct ControlObserver;
    impl DevToolsMessageObserver {
        fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>) {
            if let Some(browser) = browser { control_detached(browser.identifier()); }
        }
        fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>) {
            if let Some(browser)=browser {devtools_result(browser,message_id,success!=0,result.unwrap_or(b"{}"));}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_permission_expires_on_ownership_revision_document_and_tab_failure() {
        clear();
        // A refcounted inert CEF browser lets this test exercise actual STATE
        // validation without loading Chromium or allocating native views.
        let raw = cef::rc::RcImpl::new(unsafe { std::mem::zeroed::<sys::_cef_browser_t>() }, ());
        let browser = cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_browser_t>());
        STATE.with(|state| {
            state.borrow_mut().tabs.insert(
                7,
                Tab {
                    browser,
                    title: String::new(),
                    rejected_creation: false,
                    _devtools: None,
                    navigation: Navigation {
                        epoch: 1,
                        ..Default::default()
                    },
                },
            );
        });
        let next_document = || {
            STATE.with(|state| {
                state
                    .borrow_mut()
                    .tabs
                    .get_mut(&7)
                    .unwrap()
                    .navigation
                    .commit("https://example.test/new")
            });
            record_download_permission_attempt(7).unwrap();
            download_permission_context(7).unwrap().target
        };
        assert!(download_permission_context(7).is_err());
        record_download_permission_attempt(7).unwrap();
        let user = download_permission_context(7).unwrap().target;
        assert!(validate_download_permission(&user).is_ok());
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("download permission").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            scope
        });
        // Even without an epoch change, an unscoped approval cannot follow adoption.
        assert!(validate_download_permission(&user).is_err());
        assert!(download_permission_context(7).is_err());
        record_download_permission_attempt(7).unwrap();
        assert!(download_permission_context(7).is_err());
        let active = next_document();
        assert!(validate_download_permission(&active).is_ok());
        let paused = STATE.with(|state| state.borrow_mut().tasks.handoff(&scope).unwrap());
        assert!(validate_download_permission(&active).is_err());
        expire_tab_refs([7]);
        assert!(download_permission_context(7).is_err());
        let human = next_document();
        assert!(validate_download_permission(&human).is_ok());
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .resume(&TaskScope {
                    task: paused.id,
                    revision: paused.revision,
                })
                .unwrap()
        });
        assert!(validate_download_permission(&human).is_err());
        expire_tab_refs([7]);
        assert!(download_permission_context(7).is_err());
        let current = next_document();
        STATE.with(|state| {
            state
                .borrow_mut()
                .tabs
                .get_mut(&7)
                .unwrap()
                .navigation
                .begin(None)
        });
        // A provisional navigation may become an attachment or 204. It does
        // not invalidate the same document's permission prompt by itself.
        assert!(validate_download_permission(&current).is_ok());
        STATE.with(|state| {
            state
                .borrow_mut()
                .tabs
                .get_mut(&7)
                .unwrap()
                .navigation
                .commit("https://example.test/new")
        });
        assert!(validate_download_permission(&current).is_err());
        assert!(download_permission_context(7).is_err());
        record_download_permission_attempt(7).unwrap();
        let loading = download_permission_context(7).unwrap().target;
        assert!(validate_download_permission(&loading).is_ok());
        STATE.with(|state| {
            state
                .borrow_mut()
                .tabs
                .get_mut(&7)
                .unwrap()
                .navigation
                .renderer_failed(json!({"status":"crashed"}))
        });
        assert!(validate_download_permission(&loading).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let page = state.tabs.get_mut(&7).unwrap();
            page.navigation.renderer_exit = None;
            page.rejected_creation = true;
        });
        assert!(download_permission_context(7).is_err());
        clear();
        assert!(validate_download_permission(&loading).is_err());
    }

    #[test]
    fn reopen_is_user_space_only_and_does_not_transfer_ai_control() {
        clear();
        assert!(!can_reopen_closed_page());
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.closed_pages.record("https://example.test/reopen");
            let task = state.tasks.create("other AI space").unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            TaskScope {
                task: task.id,
                revision: task.revision,
            }
        });
        assert!(can_reopen_closed_page()); // A currently empty user space is valid.
        STATE.with(|state| state.borrow_mut().spaces.set_overview(true));
        assert!(!can_reopen_closed_page());
        STATE.with(|state| state.borrow_mut().spaces.show(&scope.task));
        assert!(!can_reopen_closed_page());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tasks.handoff(&scope).unwrap();
        });
        assert!(!can_reopen_closed_page());
        STATE.with(|state| state.borrow_mut().spaces.show(USER_SPACE));
        assert!(can_reopen_closed_page());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Paused);
            assert_eq!(task.revision, scope.revision + 1);
            state.closed_pages.begin_reopen().unwrap();
        });
        assert!(!can_reopen_closed_page());
        clear();
        assert!(!can_reopen_closed_page());
    }

    #[test]
    fn link_dispositions_only_redirect_new_browser_requests() {
        for disposition in [
            WindowOpenDisposition::NEW_FOREGROUND_TAB,
            WindowOpenDisposition::NEW_WINDOW,
            WindowOpenDisposition::NEW_POPUP,
        ] {
            assert_eq!(link_foreground(disposition), Some(true));
        }
        assert_eq!(
            link_foreground(WindowOpenDisposition::NEW_BACKGROUND_TAB),
            Some(false)
        );
        for disposition in [
            WindowOpenDisposition::CURRENT_TAB,
            WindowOpenDisposition::UNKNOWN,
            WindowOpenDisposition::SAVE_TO_DISK,
            WindowOpenDisposition::OFF_THE_RECORD,
            WindowOpenDisposition::SWITCH_TO_TAB,
        ] {
            assert_eq!(link_foreground(disposition), None);
        }
    }

    #[test]
    fn queued_link_expires_after_opener_close_handoff_or_ownership_transfer() {
        let mut tasks = Registry::default();
        let first = tasks.create("first owner").unwrap();
        let first = TaskScope {
            task: first.id,
            revision: first.revision,
        };
        let second = tasks.create("second owner").unwrap();
        let second = TaskScope {
            task: second.id,
            revision: second.revision,
        };
        let user_link = LinkCreation {
            opener: 7,
            owner: None,
            foreground: false,
        };
        assert!(user_link.matches(true, &tasks));
        assert!(!user_link.matches(false, &tasks));
        tasks.attach(&first, 7, Origin::Borrowed).unwrap();
        assert!(!user_link.matches(true, &tasks));
        let owned = LinkCreation {
            owner: link_owner(&tasks, 7),
            ..user_link.clone()
        };
        assert_eq!(owned.scope(), Some(first.clone()));
        assert!(owned.matches(true, &tasks));
        tasks.release(&first, 7).unwrap();
        tasks.attach(&second, 7, Origin::Borrowed).unwrap();
        tasks.check(&first).unwrap(); // Transfer need not change the old revision.
        assert!(!owned.matches(true, &tasks));
        let owned = LinkCreation {
            owner: link_owner(&tasks, 7),
            ..owned
        };
        let paused = tasks.handoff(&second).unwrap();
        assert!(!owned.matches(true, &tasks));
        let paused_link = LinkCreation {
            owner: link_owner(&tasks, 7),
            ..owned
        };
        assert_eq!(paused_link.scope(), None);
        assert!(paused_link.matches(true, &tasks));
        tasks
            .resume(&TaskScope {
                task: paused.id,
                revision: paused.revision,
            })
            .unwrap();
        assert!(!paused_link.matches(true, &tasks));
    }

    #[test]
    fn only_task_owned_links_reserve_automation_capacity() {
        clear();
        let owner = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("link owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            scope
        });
        let owned = LinkCreation {
            opener: 7,
            owner: Some((owner.clone(), Status::Active)),
            foreground: true,
        };
        let ticket = reserve_link_creation(&owned).unwrap().unwrap();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(state.creations.task_reserved(&owner.task), 1);
            assert!(state.creations.release(ticket));
            assert!(!state.creations.contains(ticket));
            for _ in 0..MAX_AUTOMATION_TABS {
                state.creations.reserve(0, 0, None, None).unwrap();
            }
        });
        assert_eq!(reserve_link_creation(&owned).unwrap_err().code, "TAB_LIMIT");
        let user_link = LinkCreation {
            owner: None,
            ..owned.clone()
        };
        assert_eq!(reserve_link_creation(&user_link).unwrap(), None);
        let paused = STATE.with(|state| state.borrow_mut().tasks.handoff(&owner).unwrap());
        let paused_link = LinkCreation {
            owner: Some((
                TaskScope {
                    task: paused.id,
                    revision: paused.revision,
                },
                Status::Paused,
            )),
            ..owned
        };
        assert_eq!(reserve_link_creation(&paused_link).unwrap(), None);
        assert_eq!(
            STATE.with(|state| state.borrow().creations.len()),
            MAX_AUTOMATION_TABS
        );
        clear();
    }

    #[test]
    fn popup_selection_preserves_background_intent_and_automated_views() {
        assert!(!popup_foreground(
            WindowOpenDisposition::NEW_BACKGROUND_TAB,
            false
        ));
        assert!(!popup_foreground(
            WindowOpenDisposition::NEW_FOREGROUND_TAB,
            true
        ));
        assert!(popup_foreground(
            WindowOpenDisposition::NEW_FOREGROUND_TAB,
            false
        ));
        assert!(popup_foreground(WindowOpenDisposition::NEW_POPUP, false));
    }

    #[test]
    fn previews_wait_for_the_current_document_to_finish() {
        let mut nav = Navigation::default();
        assert!(!nav.preview_ready());
        nav.commit("https://example.test/page");
        assert!(!nav.preview_ready());
        nav.document_available = true;
        assert!(!nav.preview_ready());
        nav.main_load_complete = true;
        nav.finish_loading();
        assert!(nav.preview_ready());

        // begin intentionally retains the old document, but its flags must
        // never authorize a capture of a new, still-pending navigation.
        nav.begin(Some("https://example.test/next"));
        assert!(nav.committed && nav.document_available && nav.main_load_complete);
        assert!(!nav.preview_ready());
        nav.finish_loading();
        assert!(nav.incomplete);
        assert!(!nav.preview_ready());

        // Restores may commit without begin; OnLoadEnd supplies both flags.
        nav.commit("https://example.test/restored");
        assert!(!nav.preview_ready());
        nav.document_available = true;
        nav.main_load_complete = true;
        nav.finish_loading();
        assert!(nav.preview_ready());
        // Same-document address updates keep the usable document flags.
        nav.epoch += 1;
        nav.requested_url.push_str("#section");
        assert!(nav.preview_ready());
        for block in [
            |nav: &mut Navigation| nav.provisional = true,
            |nav: &mut Navigation| nav.intent_pending = true,
            |nav: &mut Navigation| nav.stopped = true,
            |nav: &mut Navigation| nav.stop_pending = true,
            |nav: &mut Navigation| nav.incomplete = true,
            |nav: &mut Navigation| nav.error = Some(json!({"code": -1})),
            |nav: &mut Navigation| nav.renderer_exit = Some(json!({"status": "crashed"})),
        ] {
            let mut blocked = Navigation {
                committed: true,
                document_available: true,
                main_load_complete: true,
                ..Default::default()
            };
            block(&mut blocked);
            assert!(!blocked.preview_ready());
        }
    }

    #[test]
    fn unready_or_stale_preview_does_not_consume_attempt_or_capture_slot() {
        let mut state = State::default();
        let key = (USER_SPACE.to_owned(), 7, 3);
        state.spaces.insert(7, USER_SPACE, true);
        state.spaces.set_overview(true);
        state.preview_generation = 2;
        state.preview_candidates = vec![key.clone()];
        for ready_epoch in [None, Some(2), Some(4)] {
            assert!(reserve_preview(&mut state, &key, 2, ready_epoch).is_none());
            assert!(state.preview_attempted.is_empty());
            assert!(state.captures.is_empty());
            assert!(state.previews.is_empty());
            assert_eq!(state.next_message, 0);
        }
        assert!(reserve_preview(&mut state, &key, 1, Some(3)).is_none());
        assert!(state.preview_attempted.is_empty());
        assert!(state.captures.is_empty());
        let id = reserve_preview(&mut state, &key, 2, Some(3)).unwrap();
        assert_eq!(state.captures.get(&id), Some(&7));
        assert!(state.preview_attempted.contains(&key));
        assert!(reserve_preview(&mut state, &key, 2, Some(3)).is_none());
        assert_eq!(state.captures.len(), 1);
        assert_eq!(state.previews.len(), 1);
    }

    #[test]
    fn superseded_preview_releases_permit_and_allows_unchanged_card_retry() {
        clear();
        let key = ("task-a".to_owned(), 7, 3);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.preview_generation = 2;
            state.preview_candidates = vec![key.clone()];
            state.preview_attempted.insert(key.clone());
            state.captures.insert(42, 7);
            state.previews.insert(
                42,
                PreviewRequest {
                    space: key.0.clone(),
                    tab: 7,
                    epoch: 3,
                    generation: 1,
                },
            );
        });
        assert!(complete_preview(42, false, b"{}"));
        STATE.with(|state| {
            let state = state.borrow();
            assert!(state.captures.is_empty());
            assert!(state.previews.is_empty());
            assert!(!state.preview_attempted.contains(&key));
            assert_eq!(state.preview_candidates, vec![key]);
        });
        assert!(!complete_preview(42, false, b"{}"));
        clear();
    }

    fn test_reply(token: u64) -> (Reply, mpsc::Receiver<Response>) {
        let (sender, receiver) = mpsc::channel();
        (
            Reply {
                id: "same-wire-id".into(),
                token,
                active: Arc::new(AtomicBool::new(true)),
                sender,
                scope: None,
            },
            receiver,
        )
    }

    #[test]
    fn renderer_failure_requires_a_new_navigation_and_commit_before_readiness() {
        let mut nav = Navigation {
            epoch: 5,
            committed: true,
            document_available: true,
            main_load_complete: true,
            ..Default::default()
        };
        nav.renderer_failed(json!({"status":"crashed","errorCode":11}));
        assert_eq!(nav.epoch, 6);
        assert_eq!(nav.renderer_status(), "terminated");
        assert!(!nav.committed && !nav.document_available && !nav.main_load_complete);
        nav.finish_loading();
        nav.commit("https://example.test/old");
        assert_eq!(nav.renderer_status(), "terminated");
        assert!(!nav.committed && !nav.incomplete);
        nav.begin(Some("https://example.test/recovery"));
        assert_eq!(nav.renderer_status(), "recovering");
        assert!(!nav.document_available);
        nav.commit("https://example.test/recovery");
        assert_eq!(nav.renderer_status(), "running");
        assert!(nav.committed && !nav.document_available && !nav.main_load_complete);
    }

    #[test]
    fn uncommitted_renderer_recovery_can_be_attempted_again() {
        for stopped in [false, true] {
            let mut nav = Navigation::default();
            nav.renderer_failed(json!({"status":"crashed"}));
            nav.begin(Some("https://example.test/no-content"));
            nav.stopped = stopped;
            nav.stop_pending = stopped;
            nav.finish_loading();
            assert_eq!(nav.renderer_status(), "terminated");
            assert!(!nav.provisional && !nav.intent_pending && !nav.stop_pending);
            assert!(!nav.committed && !nav.document_available);
            nav.begin(None);
            assert_eq!(nav.renderer_status(), "recovering");
            assert!(!nav.stopped);
            nav.commit("https://example.test/recovered");
            assert_eq!(nav.renderer_status(), "running");
        }
    }

    #[test]
    fn renderer_exit_cancels_only_its_requests_without_releasing_task_ownership() {
        clear();
        let task = STATE.with(|state| state.borrow_mut().tasks.create("crash test").unwrap());
        let scope = TaskScope {
            task: task.id.clone(),
            revision: task.revision,
        };
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .attach(&scope, 7, Origin::Created)
                .unwrap()
        });
        let mut responses = Vec::new();
        for (id, tab, step, mutation) in [
            (1, 7, Step::World("act".into()), true),
            (2, 7, Step::Evaluate, true),
            (3, 7, Step::Screenshot("unused".into()), false),
            (4, 8, Step::Evaluate, false),
        ] {
            let (reply, receiver) = test_reply(id as u64);
            responses.push(receiver);
            STATE.with(|state| {
                state.borrow_mut().pending.insert(
                    id,
                    Pending {
                        tab,
                        epoch: 0,
                        reply,
                        step,
                        evaluation: Evaluation::Page { mutation },
                    },
                )
            });
        }
        let (waiting, wait_response) = test_reply(5);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.captures.insert(3, 7);
            state.waits.insert(
                5,
                Waiter {
                    tab: 7,
                    until: WaitUntil::Load,
                    timeout_ms: 1000,
                    started: Instant::now(),
                    probe_in_flight: false,
                    reply: waiting,
                },
            );
        });
        renderer_terminated(7, json!({"status":"crashed"}));
        for (receiver, unknown) in responses.iter().take(3).zip([false, true, false]) {
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "RENDERER_TERMINATED");
            assert_eq!(error.details.unwrap()["outcomeUnknown"], unknown);
        }
        assert_eq!(
            wait_response.try_recv().unwrap().error.unwrap().code,
            "RENDERER_TERMINATED"
        );
        assert!(responses[3].try_recv().is_err());
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.tasks.owner(7).unwrap().0, scope.task);
            assert_eq!(
                state.tasks.info(&scope.task).unwrap().revision,
                scope.revision
            );
            assert_eq!(state.captures.get(&3), Some(&7));
            assert!(state.waits.is_empty());
            assert_eq!(state.pending.len(), 1);
        });
        renderer_terminated(7, json!({"status":"crashed"}));
        for receiver in responses.iter().take(3) {
            assert!(receiver.try_recv().is_err());
        }
        assert!(complete_control_request(3).is_none());
        assert!(STATE.with(|state| state.borrow().captures.is_empty()));
        clear();
    }

    #[test]
    fn bookmark_reuse_excludes_stopped_or_uncommitted_destinations() {
        let url = "https://example.test/bookmark";
        let mut nav = Navigation::default();
        nav.begin(Some(url));
        assert!(nav.can_reuse_bookmark(url));
        assert!(!nav.can_reuse_bookmark("https://example.test/other"));
        nav.stopped = true;
        nav.finish_loading();
        assert!(!nav.can_reuse_bookmark(url));
        nav.begin(Some(url));
        nav.finish_loading();
        assert!(nav.incomplete);
        assert!(!nav.can_reuse_bookmark(url));
        nav.begin(Some(url));
        nav.commit(url);
        nav.finish_loading();
        assert!(nav.can_reuse_bookmark(url));
        nav.renderer_failed(json!({"status":"crashed"}));
        assert!(!nav.can_reuse_bookmark(url));
    }

    #[test]
    fn bookmark_selection_preserves_ai_control_and_saved_selection() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("bookmark background task").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(7, &scope.task, false);
            state.tasks.attach(&scope, 7, Origin::Created).unwrap();
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.show(&scope.task);
            scope
        });
        assert!(select_bookmark_tab(7).is_err());
        select_bookmark_tab(8).unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(8));
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.tasks.owner(7).unwrap().0, scope.task);
            assert_eq!(
                state
                    .spaces
                    .views()
                    .iter()
                    .find(|space| space.id == scope.task)
                    .unwrap()
                    .selected_tab,
                Some(7)
            );
        });
        // Ownership is checked independently of the visible space mapping.
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .attach(&scope, 8, Origin::Borrowed)
                .unwrap()
        });
        assert!(select_bookmark_tab(8).is_err());
        assert!(select_bookmark_tab(99).is_err());
        assert_eq!(
            prefer_bookmark_tab(Some(8), [4, 8, 12].into_iter()),
            Some(8)
        );
        assert_eq!(
            prefer_bookmark_tab(Some(7), [4, 8, 12].into_iter()),
            Some(4)
        );
        assert_eq!(prefer_bookmark_tab(Some(7), [].into_iter()), None);
        clear();
    }

    #[test]
    fn search_selection_uses_current_space_without_changing_task_control() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("search destination").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(7, USER_SPACE, true);
            state.spaces.insert(8, USER_SPACE, false);
            // The search result originally belonged to the user space. An
            // adoption before selection must resolve to the new task space.
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            state.spaces.adopt(7, &scope.task);
            state.spaces.set_overview(true);
            scope
        });
        shell_action(ShellAction::SelectSearchTab(7)).unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(!state.spaces.overview());
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.tasks.owner(7).unwrap().0, scope.task);
            assert_eq!(state.spaces.tab_space(8), Some(USER_SPACE));
        });
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tasks.tab_closed(7);
            state.spaces.remove(7);
            state.spaces.show(USER_SPACE);
            state.spaces.set_overview(true);
        });
        assert!(shell_action(ShellAction::SelectSearchTab(7)).is_err());
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(8));
            assert!(state.spaces.overview());
        });
        clear();
    }

    #[test]
    fn page_input_handoffs_once_and_revokes_only_its_task_requests() {
        clear();
        let (scope, other) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("page input owner").unwrap();
            let other = state.tasks.create("background task").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(7, &scope.task, false);
            state.spaces.show(&scope.task);
            (scope, other)
        });
        let mut results = Vec::new();
        for (id, step, owned) in [
            (1, Step::World("click".into()), true),
            (2, Step::Evaluate, true),
            (3, Step::Evaluate, false),
        ] {
            let (mut reply, receiver) = test_reply(id as u64);
            if owned {
                reply.scope = Some(scope.clone());
            }
            results.push(receiver);
            STATE.with(|state| {
                state.borrow_mut().pending.insert(
                    id,
                    Pending {
                        tab: 7,
                        epoch: 0,
                        reply,
                        step,
                        evaluation: Evaluation::Page { mutation: true },
                    },
                )
            });
        }
        let (mut reply, wait_result) = test_reply(4);
        reply.scope = Some(scope.clone());
        STATE.with(|state| {
            state.borrow_mut().waits.insert(
                4,
                Waiter {
                    tab: 7,
                    until: WaitUntil::Load,
                    timeout_ms: 1000,
                    started: Instant::now(),
                    probe_in_flight: false,
                    reply,
                },
            )
        });

        assert!(handoff_page_input(7).unwrap());
        assert!(!handoff_page_input(7).unwrap());
        for (receiver, unknown) in [
            (&results[0], false),
            (&results[1], true),
            (&wait_result, false),
        ] {
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "TASK_CONTROL_LOST");
            assert_eq!(error.details.unwrap()["outcomeUnknown"], unknown);
            assert!(receiver.try_recv().is_err());
        }
        assert!(results[2].try_recv().is_err());
        STATE.with(|state| {
            let state = state.borrow();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Paused);
            assert_eq!(task.revision, scope.revision + 1);
            assert_eq!(state.tasks.info(&other.id).unwrap().status, Status::Active);
            assert_eq!(state.tasks.owner(7).unwrap().0, scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert_eq!(state.spaces.active_space(), scope.task);
            assert!(state.waits.is_empty());
            assert_eq!(state.pending.len(), 1);
        });
        clear();
    }

    #[test]
    fn page_input_uses_current_owner_and_ignores_overview_or_other_tabs() {
        clear();
        let (previous, current) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let first = state.tasks.create("previous owner").unwrap();
            let second = state.tasks.create("current owner").unwrap();
            let first = TaskScope {
                task: first.id,
                revision: first.revision,
            };
            let second = TaskScope {
                task: second.id,
                revision: second.revision,
            };
            state.tasks.attach(&first, 7, Origin::Borrowed).unwrap();
            state.tasks.release(&first, 7).unwrap();
            state.tasks.attach(&second, 7, Origin::Borrowed).unwrap();
            state.spaces.insert(7, USER_SPACE, true);
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.set_overview(true);
            (first, second)
        });
        assert!(!handoff_page_input(7).unwrap());
        STATE.with(|state| state.borrow_mut().spaces.set_overview(false));
        assert!(!handoff_page_input(8).unwrap());
        assert!(!handoff_page_input(99).unwrap());
        assert!(handoff_page_input(7).unwrap());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(
                state.tasks.info(&previous.task).unwrap().status,
                Status::Active
            );
            assert_eq!(
                state.tasks.info(&previous.task).unwrap().revision,
                previous.revision
            );
            assert_eq!(
                state.tasks.info(&current.task).unwrap().status,
                Status::Paused
            );
            assert_eq!(
                state.tasks.info(&current.task).unwrap().revision,
                current.revision + 1
            );
            state.spaces.select_tab(8);
        });
        assert!(!handoff_page_input(8).unwrap());
        clear();
    }

    #[test]
    fn task_resume_invalidates_only_its_find_session_after_success() {
        clear();
        let (owner, other, find) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let [owner, other] = [("find owner", 7), ("other task", 8)].map(|(name, tab)| {
                let task = state.tasks.create(name).unwrap();
                let scope = TaskScope {
                    task: task.id,
                    revision: task.revision,
                };
                state.tasks.attach(&scope, tab, Origin::Borrowed).unwrap();
                let task = state.tasks.handoff(&scope).unwrap();
                TaskScope {
                    task: task.id,
                    revision: task.revision,
                }
            });
            // No native Browser handles are needed to exercise the ownership
            // boundary and cancellation of both current and queued find work.
            let find = FindRequest {
                target: FindTarget { tab: 7, epoch: 3 },
                token: 1,
                query: "needle".into(),
                forward: true,
                find_next: false,
            };
            state.page_find.submit(find.clone()).unwrap();
            assert!(state.page_find.mark_issued(&find));
            state
                .page_find
                .submit(FindRequest {
                    token: 2,
                    find_next: true,
                    ..find.clone()
                })
                .unwrap();
            (owner, other, find)
        });

        for (scope, revision, succeeds) in [
            (&owner, owner.revision - 1, false),
            (&other, other.revision, true),
        ] {
            let (reply, receiver) = test_reply(1);
            handle_task(
                &Command::TaskResume {
                    task: scope.task.clone(),
                    revision,
                },
                reply,
            );
            let response = receiver.try_recv().unwrap();
            if succeeds {
                assert!(response.error.is_none());
            } else {
                assert_eq!(response.error.unwrap().code, "TASK_STALE_REVISION");
            }
            STATE.with(|state| assert!(state.borrow().page_find.is_current(&find)));
        }

        let (reply, receiver) = test_reply(2);
        handle_task(
            &Command::TaskResume {
                task: owner.task.clone(),
                revision: owner.revision,
            },
            reply,
        );
        assert!(receiver.try_recv().unwrap().error.is_none());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert!(state.page_find.target().is_none());
            let late = state.page_find.result(find.target, 17, 8, 1, true);
            assert!(late.update.is_none() && late.issue.is_none());
            for scope in [&owner, &other] {
                let task = state.tasks.info(&scope.task).unwrap();
                assert_eq!(task.status, Status::Active);
                assert_eq!(task.revision, scope.revision + 1);
            }
        });
        clear();
    }

    #[test]
    fn native_stop_uses_displayed_revision_and_preserves_other_tasks() {
        clear();
        let task = STATE.with(|state| state.borrow_mut().tasks.create("current task").unwrap());
        let other = STATE.with(|state| state.borrow_mut().tasks.create("other task").unwrap());
        let scope = TaskScope {
            task: task.id.clone(),
            revision: task.revision,
        };
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .attach(&scope, 7, Origin::Borrowed)
                .unwrap()
        });
        shell_action(ShellAction::Stop(7, scope.clone())).unwrap();
        let error = shell_action(ShellAction::Stop(7, scope.clone())).unwrap_err();
        assert!(error.starts_with("TASK_STALE_REVISION"));
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(
                state.tasks.info(&scope.task).unwrap().status,
                Status::Paused
            );
            assert_eq!(state.tasks.info(&other.id).unwrap().status, Status::Active);
            assert_eq!(
                state.tasks.owner(7).unwrap(),
                (scope.task.clone(), Origin::Borrowed)
            );
        });
        clear();
    }

    #[test]
    fn native_stop_rejects_a_tab_transferred_after_the_displayed_snapshot() {
        clear();
        let (previous, current) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let previous = state.tasks.create("previous owner").unwrap();
            let current = state.tasks.create("current owner").unwrap();
            let previous = TaskScope {
                task: previous.id,
                revision: previous.revision,
            };
            let current = TaskScope {
                task: current.id,
                revision: current.revision,
            };
            state.tasks.attach(&previous, 7, Origin::Borrowed).unwrap();
            state.tasks.release(&previous, 7).unwrap();
            state.tasks.attach(&current, 7, Origin::Borrowed).unwrap();
            // Ownership can change while the displayed revision remains valid.
            state.tasks.check(&previous).unwrap();
            (previous, current)
        });

        let error = shell_action(ShellAction::Stop(7, previous.clone())).unwrap_err();
        assert!(error.starts_with("TASK_TAB_MISMATCH"));
        STATE.with(|state| {
            let state = state.borrow();
            for scope in [&previous, &current] {
                let task = state.tasks.info(&scope.task).unwrap();
                assert_eq!(task.status, Status::Active);
                assert_eq!(task.revision, scope.revision);
            }
            assert_eq!(
                state.tasks.owner(7).unwrap(),
                (current.task.clone(), Origin::Borrowed)
            );
        });
        shell_action(ShellAction::Stop(7, current.clone())).unwrap();
        STATE.with(|state| {
            assert_eq!(
                state.borrow().tasks.info(&current.task).unwrap().status,
                Status::Paused
            );
        });
        clear();
    }

    #[test]
    fn navigation_barrier_preserves_document_but_resets_previous_outcome() {
        let mut nav = Navigation {
            epoch: 3,
            committed: true,
            document_available: true,
            main_load_complete: true,
            stopped: true,
            incomplete: true,
            error: Some(json!({"code":-1})),
            ..Default::default()
        };
        nav.begin(Some("https://example.test/next"));
        assert_eq!(nav.epoch, 4);
        assert!(nav.provisional && nav.intent_pending);
        assert!(nav.committed && nav.document_available);
        assert!(!nav.stopped && !nav.incomplete && nav.error.is_none());
        nav.finish_loading();
        assert!(nav.incomplete);
        assert!(!nav.provisional && !nav.intent_pending);
        nav.finish_loading(); // A later subframe load must not erase the outcome.
        assert!(nav.incomplete);
    }

    #[test]
    fn stopped_navigation_is_distinct_from_incomplete_or_committed_navigation() {
        let mut nav = Navigation::default();
        nav.begin(None);
        nav.stopped = true;
        nav.stop_pending = true;
        nav.finish_loading();
        assert!(nav.stopped && !nav.incomplete && !nav.provisional);
        assert!(!nav.stop_pending);
        nav.begin(None);
        nav.intent_pending = false; // A commit or same-document address callback.
        nav.finish_loading();
        assert!(!nav.stopped && !nav.incomplete && !nav.provisional);
    }

    #[test]
    fn native_epoch_rejects_references_from_a_restored_document() {
        let (reply, receiver) = test_reply(1);
        assert!(action_expression("click", "8/document:1:1", None, 9, &reply).is_empty());
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "STALE_REF"
        );
        let (reply, _) = test_reply(2);
        let expression = action_expression("click", "9/document:1:1", None, 9, &reply);
        assert!(expression.contains("\"ref\":\"document:1:1\""));
        assert!(!expression.contains("9/"));
    }

    #[test]
    fn select_uses_epoch_guard_and_serializes_option_values_as_data() {
        let value = "  \"});window.unexpected=true;//\n🧭  ".to_owned();
        let (reply, receiver) = test_reply(1);
        assert!(
            action_expression(
                "select",
                "8/document:1:1",
                Some(("value", &value)),
                9,
                &reply
            )
            .is_empty()
        );
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "STALE_REF"
        );
        let (reply, _) = test_reply(2);
        let expression = action_expression(
            "select",
            "9/document:1:1",
            Some(("value", &value)),
            9,
            &reply,
        );
        let args = expression
            .strip_prefix("window.__lightBrowserBridge.act(")
            .unwrap()
            .strip_suffix(')')
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Json>(args).unwrap(),
            json!({"action":"select","ref":"document:1:1","value":value})
        );
    }

    #[test]
    fn concurrent_completion_replies_only_once() {
        let (reply, receiver) = test_reply(1);
        let second = reply.clone();
        let worker = thread::spawn(move || second.error("TIMEOUT", "expired"));
        reply.ok(json!({"done":true}));
        worker.join().unwrap();
        assert_eq!(receiver.try_iter().count(), 1);
        assert!(!reply.active.load(Ordering::Acquire));
    }

    #[test]
    fn capture_permit_outlives_timeout_and_handoff_until_native_completion() {
        clear();
        for (id, token) in [(10, 1), (11, 2)] {
            let (mut reply, _) = test_reply(token);
            reply.scope = Some(TaskScope {
                task: "capture-task".into(),
                revision: 1,
            });
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.captures.insert(id, 7);
                state.pending.insert(
                    id,
                    Pending {
                        tab: 7,
                        epoch: 0,
                        evaluation: Evaluation::Page { mutation: false },
                        reply,
                        step: Step::Screenshot("unused.png".into()),
                    },
                );
            });
        }
        remove_wait(1);
        revoke_task_requests("capture-task", None);
        STATE.with(|state| {
            assert!(state.borrow().pending.is_empty());
            assert_eq!(state.borrow().captures.len(), 2);
        });
        assert!(complete_control_request(10).is_none());
        STATE.with(|state| assert_eq!(state.borrow().captures.len(), 1));
        control_detached(7);
        STATE.with(|state| assert!(state.borrow().captures.is_empty()));
        clear();
    }

    #[test]
    fn task_handoff_cancels_preparation_waits_and_reports_dispatched_effects() {
        clear();
        let scope = TaskScope {
            task: "task-test".into(),
            revision: 1,
        };
        let (mut prepared, prepared_result) = test_reply(1);
        let (mut dispatched, dispatched_result) = test_reply(2);
        let (mut waiting, wait_result) = test_reply(3);
        let (other, other_result) = test_reply(4);
        prepared.scope = Some(scope.clone());
        dispatched.scope = Some(scope.clone());
        waiting.scope = Some(scope.clone());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            for (id, reply, step) in [
                (1, prepared, Step::World("click".into())),
                (2, dispatched, Step::Evaluate),
                (4, other, Step::Evaluate),
            ] {
                state.pending.insert(
                    id,
                    Pending {
                        tab: 7,
                        epoch: 1,
                        reply,
                        evaluation: Evaluation::Page { mutation: true },
                        step,
                    },
                );
            }
            state.waits.insert(
                3,
                Waiter {
                    tab: 7,
                    until: WaitUntil::Load,
                    timeout_ms: 1000,
                    started: Instant::now(),
                    probe_in_flight: true,
                    reply: waiting.clone(),
                },
            );
            state.pending.insert(
                3,
                Pending {
                    tab: 7,
                    epoch: 1,
                    reply: waiting,
                    evaluation: Evaluation::ReadyState { token: 3 },
                    step: Step::Evaluate,
                },
            );
        });
        revoke_task_requests(&scope.task, None);
        for (receiver, unknown) in [
            (prepared_result, false),
            (dispatched_result, true),
            (wait_result, false),
        ] {
            let response = receiver.try_recv().unwrap();
            let error = response.error.unwrap();
            assert_eq!(error.code, "TASK_CONTROL_LOST");
            assert_eq!(error.details.unwrap()["outcomeUnknown"], unknown);
            assert!(receiver.try_recv().is_err());
        }
        assert!(other_result.try_recv().is_err());
        STATE.with(|state| {
            assert!(state.borrow().waits.is_empty());
            assert_eq!(
                state.borrow().pending.keys().copied().collect::<Vec<_>>(),
                [4]
            );
        });
        clear();
    }

    #[test]
    fn queued_task_request_cannot_run_after_handoff_and_resume() {
        clear();
        let task = STATE.with(|state| state.borrow_mut().tasks.create("queue test").unwrap());
        let original = TaskScope {
            task: task.id.clone(),
            revision: task.revision,
        };
        let paused = STATE.with(|state| state.borrow_mut().tasks.handoff(&original).unwrap());
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .resume(&TaskScope {
                    task: task.id.clone(),
                    revision: paused.revision,
                })
                .unwrap()
        });
        let (reply, receiver) = test_reply(1);
        // Rejected before looking up a browser or dispatching a CEF action.
        handle(
            &Command::TaskRun {
                task: original.task,
                revision: original.revision,
                command: Box::new(Command::Read { tab: 7 }),
            },
            reply,
        );
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "TASK_STALE_REVISION"
        );
        clear();
    }

    #[test]
    fn cancellation_does_not_match_another_connections_wire_id() {
        clear();
        let (first, _) = test_reply(1);
        let (second, _) = test_reply(2);
        STATE.with(|state| {
            for (id, reply) in [(1, first), (2, second)] {
                state.borrow_mut().pending.insert(
                    id,
                    Pending {
                        tab: 1,
                        epoch: 0,
                        reply,
                        evaluation: Evaluation::Page { mutation: false },
                        step: Step::Evaluate,
                    },
                );
            }
        });
        remove_wait(1);
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.pending.len(), 1);
            assert_eq!(state.pending[&2].reply.token, 2);
        });
        clear();
    }

    #[test]
    fn detach_resumes_readiness_but_does_not_repeat_a_dispatched_action() {
        clear();
        let (wait_reply, wait_result) = test_reply(1);
        let (action_reply, action_result) = test_reply(2);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.waits.insert(
                1,
                Waiter {
                    tab: 7,
                    until: WaitUntil::Dom,
                    timeout_ms: 1000,
                    started: Instant::now(),
                    probe_in_flight: true,
                    reply: wait_reply.clone(),
                },
            );
            state.pending.insert(
                1,
                Pending {
                    tab: 7,
                    epoch: 1,
                    reply: wait_reply,
                    evaluation: Evaluation::ReadyState { token: 1 },
                    step: Step::Evaluate,
                },
            );
            state.pending.insert(
                2,
                Pending {
                    tab: 7,
                    epoch: 1,
                    reply: action_reply,
                    evaluation: Evaluation::Page { mutation: true },
                    step: Step::Evaluate,
                },
            );
        });
        control_detached(7);
        STATE.with(|state| {
            let state = state.borrow();
            assert!(state.pending.is_empty());
            assert!(!state.waits[&1].probe_in_flight);
        });
        assert!(wait_result.try_recv().is_err());
        let error = action_result.try_recv().unwrap().error.unwrap();
        assert_eq!(error.code, "CONTROL_DISCONNECTED");
        assert_eq!(error.details.unwrap()["outcomeUnknown"], true);
        clear();
    }

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
