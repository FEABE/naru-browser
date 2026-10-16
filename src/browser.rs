use crate::capture::{MAX_CAPTURE_RESULT_BYTES, save_png_base64};
use crate::control_panel::{ShellAction, ShellSnapshot, ShellTab};
use crate::page_find::{Request as FindRequest, State as FindState, Target as FindTarget};
use crate::page_recovery::{PageFailure, PageRecoveryTarget};
use crate::protocol::{self, Command, Request, Response, TaskScope, WaitUntil};
use crate::resources::{MAX_AUTOMATION_TABS, ResourceBudget};
use crate::spaces::{Spaces, USER_SPACE};
use crate::start_page::StartPageTarget;
use crate::tasks::{Origin, Registry, Status, TaskError};
use base64::{Engine, engine::general_purpose::STANDARD};
use cef::*;
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    rc::Rc as Shared,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const DOM_BRIDGE: &str = include_str!("dom_bridge.js");
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_CONNECTIONS: usize = 16;

struct CommandPermit {
    outstanding: Arc<AtomicUsize>,
}

impl CommandPermit {
    fn acquire(outstanding: &Arc<AtomicUsize>, id: &str) -> Result<Self, Response> {
        outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_CONNECTIONS).then_some(count + 1)
            })
            .map_err(|_| {
                Response::error(
                    id,
                    "APP_BUSY",
                    "The browser command queue is full. Try again later.",
                )
                .with_details(json!({"outcomeUnknown":false}))
            })?;
        Ok(Self {
            outstanding: outstanding.clone(),
        })
    }
}

impl Drop for CommandPermit {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }
}

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

#[derive(Clone, Copy)]
struct HistoryCreation {
    generation: Option<u64>,
    staged: bool,
}

impl HistoryCreation {
    fn new(staged: bool) -> Self {
        Self {
            generation: crate::history_runtime::generation(),
            staged,
        }
    }
}

#[derive(Default)]
struct HistoryObservation {
    // Some(None) retains an ineligible/revoked navigation across redirects.
    attempt: Option<Option<u64>>,
    document: Option<HistoryDocument>,
    queued: bool,
    staged: bool,
    revoked: bool,
}

struct HistoryDocument {
    generation: u64,
    epoch: u64,
    successful_http: bool,
    address: String,
    title: Option<String>,
    recorded: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryAction {
    generation: u64,
    document_epoch: u64,
    url: String,
    title: String,
    update_title: bool,
}

impl HistoryObservation {
    fn begin(&mut self, generation: Option<u64>, explicit: bool) {
        if explicit || self.attempt.is_none() {
            self.attempt = Some(generation);
            self.revoked = false;
        }
        if explicit {
            self.document = None;
        }
    }

    fn revoke(&mut self) {
        self.revoked = true;
        if let Some(attempt) = &mut self.attempt {
            *attempt = None;
        }
        self.document = None;
    }

    fn commit(&mut self, epoch: u64, url: &str, fallback: Option<u64>) {
        // A late commit without a new navigation start cannot reclassify an
        // AI document after release, even after loading=false cleared attempt.
        let generation = self
            .attempt
            .unwrap_or(if self.revoked { None } else { fallback });
        let address = crate::address::normalize_url(url).ok();
        self.document = generation
            .zip(address)
            .map(|(generation, address)| HistoryDocument {
                generation,
                epoch,
                successful_http: false,
                address,
                title: None,
                recorded: None,
            });
    }

    fn title_changed(&mut self, value: &str) {
        if let Some(document) = &mut self.document {
            let title: String = value
                .chars()
                .filter(|ch| !ch.is_control())
                .take(256)
                .collect();
            let title = title.trim();
            document.title = (!title.is_empty()).then(|| title.to_owned());
        }
    }

    fn address_changed(&mut self, url: &str) {
        // The old document's title cannot label a new URL. A later title event
        // can update the accepted visit without counting another visit.
        if let Some(document) = &mut self.document {
            if let Ok(url) = crate::address::normalize_url(url) {
                if document.address != url {
                    document.title = None;
                    document.address = url;
                }
            }
        }
    }

    fn action(&self, url: String) -> Option<HistoryAction> {
        if self.staged {
            return None;
        }
        let document = self.document.as_ref()?;
        if !document.successful_http {
            return None;
        }
        let title = document
            .title
            .clone()
            .unwrap_or_else(|| url.chars().take(256).collect());
        let update_title = document
            .recorded
            .as_ref()
            .is_some_and(|recorded| recorded.0 == url);
        if document
            .recorded
            .as_ref()
            .is_some_and(|recorded| recorded.0 == url && recorded.1 == title)
        {
            return None;
        }
        Some(HistoryAction {
            generation: document.generation,
            document_epoch: document.epoch,
            url,
            title,
            update_title,
        })
    }

    fn accepted(&mut self, action: &HistoryAction) {
        if let Some(document) = &mut self.document {
            if document.generation == action.generation && document.epoch == action.document_epoch {
                document.recorded = Some((action.url.clone(), action.title.clone()));
            }
        }
    }
}

#[derive(Default)]
struct Navigation {
    history: HistoryObservation,
    epoch: u64,
    // Main-document identity excludes provisional requests that become downloads.
    document_epoch: u64,
    // Explicit navigation/control changes invalidate a file's source link;
    // provisional attachment requests must not hide earlier source links.
    download_source_generation: u64,
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
    // One moved error for a same-URL automatic retry; never exposed while the
    // retry is pending. CEF does not identify its internal error-page requests.
    automatic_retry_error: Option<Json>,
    renderer_exit: Option<Json>,
    recovering: bool,
}
impl Navigation {
    fn concrete_error_matches(error: &Json, url: &str) -> bool {
        !url.is_empty()
            && error
                .get("code")
                .and_then(Json::as_i64)
                .and_then(|code| i32::try_from(code).ok())
                .is_some_and(|code| code != Errorcode::ABORTED.get_raw())
            && error.get("failedUrl").and_then(Json::as_str) == Some(url)
    }

    fn load_failed(&mut self, code: i32, text: &str, failed_url: &str) {
        self.history.revoke();
        if self.renderer_exit.is_some() && !self.recovering {
            return;
        }
        self.provisional = false;
        let retry_error = self.automatic_retry_error.take();
        if code == Errorcode::ABORTED.get_raw() && self.stopped {
            return;
        }
        if code == Errorcode::ABORTED.get_raw()
            && self.requested_url == failed_url
            && retry_error
                .as_ref()
                .is_some_and(|error| Self::concrete_error_matches(error, failed_url))
        {
            self.error = retry_error;
            return;
        }
        if code == Errorcode::ABORTED.get_raw()
            && self
                .error
                .as_ref()
                .is_some_and(|error| Self::concrete_error_matches(error, failed_url))
        {
            // CEF can deliver errors from both DidFinishNavigation and
            // DidFailLoad. A trailing abort must not replace a concrete error
            // for the same URL; a new begin still clears the previous failure.
            return;
        }
        self.error = Some(json!({"code": code, "text": text, "failedUrl": failed_url}));
    }

    fn trace_load_event(
        &self,
        event: &str,
        tab: i32,
        code: Option<i32>,
        url: &str,
        browse: Option<(bool, bool, u32)>,
    ) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !ENABLED.get_or_init(|| {
            std::env::var_os("NARU_NAVIGATION_DIAGNOSTICS").is_some_and(|value| value == "1")
        }) {
            return;
        }
        let previous_code = self
            .error
            .as_ref()
            .and_then(|error| error.get("code"))
            .and_then(Json::as_i64);
        let previous_url_matches = self
            .error
            .as_ref()
            .and_then(|error| error.get("failedUrl"))
            .and_then(Json::as_str)
            == Some(url);
        let candidate_code = self
            .automatic_retry_error
            .as_ref()
            .and_then(|error| error.get("code"))
            .and_then(Json::as_i64);
        let candidate_url_matches = self
            .automatic_retry_error
            .as_ref()
            .and_then(|error| error.get("failedUrl"))
            .and_then(Json::as_str)
            == Some(url);
        // Diagnostics deliberately contain no page URL or error text.
        eprintln!(
            "navigation event={event} tab={tab} epoch={} code={code:?} previous_code={previous_code:?} candidate_code={candidate_code:?} requested_url_match={} previous_failed_url_match={previous_url_matches} candidate_failed_url_match={candidate_url_matches} user_gesture={:?} is_redirect={:?} transition={:?}",
            self.epoch,
            self.requested_url == url,
            browse.map(|value| value.0),
            browse.map(|value| value.1),
            browse.map(|value| value.2),
        );
    }

    fn page_failure(&self, loading: bool) -> Option<PageFailure> {
        if self.recovering || self.provisional {
            return None;
        }
        // A failed renderer recovery can finish through stop/204/no commit.
        // Keep recovery available even if the old renderer's loading flag lags.
        if self.renderer_exit.is_some() {
            return Some(PageFailure::Renderer);
        }
        if self.stopped || self.stop_pending || self.intent_pending || loading {
            return None;
        }
        let code = self.error.as_ref()?.get("code")?.as_i64()?;
        let code = i32::try_from(code).ok()?;
        (code != Errorcode::ABORTED.get_raw()).then_some(PageFailure::Load { code })
    }

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
        self.automatic_retry_error = None;
        self.recovering = self.renderer_exit.is_some();
        if let Some(url) = url {
            self.requested_url = url.to_owned();
        }
    }
    fn begin_browse(&mut self, url: &str, user_gesture: bool, is_redirect: bool, transition: u32) {
        // An explicit native/CLI begin has already cleared both errors. For
        // otherwise indistinguishable same-URL automatic retries, keep only a
        // private candidate until a matching abort. A successful commit or any
        // known new navigation boundary discards it.
        let candidate = self.error.take().or(self.automatic_retry_error.take());
        let preserve = !user_gesture
            && !is_redirect
            && transition
                & (TransitionType::IS_REDIRECT_MASK.get_raw()
                    | TransitionType::FORWARD_BACK_FLAG.get_raw())
                == 0
            && !self.stopped
            && !self.stop_pending
            && self.renderer_exit.is_none()
            && self.requested_url == url
            && candidate
                .as_ref()
                .is_some_and(|error| Self::concrete_error_matches(error, url));
        self.begin(Some(url));
        if preserve {
            self.automatic_retry_error = candidate;
        }
    }
    fn finish_loading(&mut self) {
        self.automatic_retry_error = None;
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
        self.history.revoke();
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
        self.automatic_retry_error = None;
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
        // CEF only emits OnLoadStart for a successful new-document commit.
        // A later error callback will record a failure after that commit.
        self.error = None;
        self.automatic_retry_error = None;
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
    // Only startup/native NewTab may cover a blank document with native UI.
    // Popup and CLI documents retain their normal CEF surface.
    native_start_page: bool,
    // Last confirmed native mute setting, not evidence of audible playback.
    audio_muted: Option<bool>,
    context_menu: Option<ContextMenuTarget>,
    context_menu_generation: u64,
    _devtools: Option<Registration>,
    navigation: Navigation,
}
impl Tab {
    fn start_page_ready(&self, loading: bool) -> bool {
        self.native_start_page
            && !self.rejected_creation
            && !loading
            && self.navigation.requested_url == "about:blank"
            && !self.navigation.recovering
            && self.navigation.preview_ready()
    }

    fn observe_start_page_url(&mut self, url: &str) {
        // Once used as a web page, returning to about:blank must not cover
        // page-generated content or revive the discarded native input draft.
        self.native_start_page &= url == "about:blank";
    }
}
#[derive(Clone, Copy)]
enum ActionEvidence {
    Check,
    Fill,
    Press,
}

impl ActionEvidence {
    fn for_command(command: &Command) -> Option<Self> {
        match command {
            Command::Check { .. } => Some(Self::Check),
            Command::Fill { .. } => Some(Self::Fill),
            Command::Press { .. } => Some(Self::Press),
            _ => None,
        }
    }

    fn fields(self, value: &Json) -> &'static [&'static str] {
        match self {
            Self::Check if value["action"] == "check" => {
                &["attempted", "trusted", "checked", "indeterminate"]
            }
            Self::Fill if value["action"] == "fill" => &["attempted", "trusted"],
            Self::Press if value["action"] == "press" => &["attempted"],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy)]
enum Evaluation {
    Page {
        mutation: bool,
        evidence: Option<ActionEvidence>,
    },
    ReadyState {
        token: u64,
    },
}
#[derive(Clone)]
enum Step {
    Frame(String),
    World(String),
    Evaluate,
    Screenshot(String),
    Press {
        batch: Shared<PressBatch>,
        part: usize,
    },
}
#[derive(Clone)]
struct Pending {
    tab: i32,
    epoch: u64,
    evaluation: Evaluation,
    reply: Reply,
    step: Step,
}
impl Pending {
    fn mutation_dispatched(&self) -> bool {
        matches!(self.step, Step::Evaluate | Step::Press { .. })
            && matches!(self.evaluation, Evaluation::Page { mutation: true, .. })
    }

    fn press_preparation(&self) -> bool {
        matches!(self.step, Step::Evaluate)
            && matches!(
                self.evaluation,
                Evaluation::Page {
                    evidence: Some(ActionEvidence::Press),
                    ..
                }
            )
    }

    fn outcome_details(&self) -> Json {
        let mut details = json!({"outcomeUnknown":self.mutation_dispatched()});
        if let Step::Press { batch, .. } = &self.step {
            details["attempted"] =
                json!(batch.progress.borrow().attempted.iter().any(|value| *value));
        } else if self.press_preparation() {
            details["attempted"] = json!(false);
        }
        details
    }
}

#[derive(Clone, Copy, Debug)]
struct PressFailure {
    code: &'static str,
    message: &'static str,
}

struct PressProgress {
    enqueuing: bool,
    attempted: [bool; 2],
    accepted: [Option<bool>; 2],
    acknowledged: [bool; 2],
    failure: Option<PressFailure>,
}

struct PressBatch {
    ids: [i32; 2],
    tab: i32,
    epoch: u64,
    // Retain the original browser until both parts finish; raw identity cannot
    // be recycled while a late callback is being checked.
    browser: Browser,
    reply: Reply,
    progress: RefCell<PressProgress>,
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
    // Explicit context-menu intents stay in their accepted source space.
    // Existing renderer navigation/popup paths retain their original policy.
    expected_space: Option<String>,
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
    ) && link
        .expected_space
        .as_ref()
        .is_none_or(|space| state.spaces.tab_space(link.opener) == Some(space.as_str()))
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
    pub source_target: crate::downloads::SourceTarget,
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
            page.navigation.download_source_generation += 1;
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
        let page = state
            .tabs
            .get(&tab)
            .filter(|tab| !tab.rejected_creation)
            .ok_or_else(|| "다운로드를 요청한 탭이 닫혔습니다.".to_owned())?;
        let source_target = crate::downloads::SourceTarget {
            tab,
            generation: page.navigation.download_source_generation,
            document_epoch: page.navigation.document_epoch,
            owner: link_owner(&state.tasks, tab),
        };
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
                source: format!(
                    "{}, 탭 {}",
                    task.name.chars().take(48).collect::<String>(),
                    tab
                ),
                source_target,
            }),
            Some(task) if task.status == Status::Paused => Ok(DownloadContext {
                scope: None,
                source: format!(
                    "{}, 사용자 제어, 탭 {}",
                    task.name.chars().take(48).collect::<String>(),
                    tab
                ),
                source_target,
            }),
            Some(task) if task.status != Status::Paused => {
                Err("마무리 중인 작업에서는 다운로드를 받을 수 없습니다.".into())
            }
            _ => Ok(DownloadContext {
                scope: None,
                source: format!("사용자 · 탭 {tab}"),
                source_target,
            }),
        }
    })
}

/// A source link is independent of the file's lifetime and permission prompt.
/// It never substitutes a newer document or controller for the captured one.
pub(crate) fn validate_download_source(
    target: &crate::downloads::SourceTarget,
) -> Result<(), String> {
    STATE.with(|state| {
        let state = state.borrow();
        let current = state.spaces.tab_space(target.tab).is_some()
            && link_owner(&state.tasks, target.tab) == target.owner
            && state.tabs.get(&target.tab).is_some_and(|tab| {
                !tab.rejected_creation
                    && tab.navigation.renderer_exit.is_none()
                    && tab.navigation.document_epoch == target.document_epoch
                    && tab.navigation.download_source_generation == target.generation
            });
        if current {
            Ok(())
        } else {
            Err("다운로드를 요청한 탭이나 페이지의 제어권이 변경되었습니다.".into())
        }
    })
}

pub(crate) fn reveal_download_source(
    target: &crate::downloads::SourceTarget,
) -> Result<i32, String> {
    validate_download_source(target)?;
    // No CEF or AppKit call separates validation from the existing selector's
    // live space lookup. Selection alone never hands off the source AI task.
    shell_action(ShellAction::SelectSearchTab(target.tab))?;
    Ok(target.tab)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabAudioTarget {
    pub tab: i32,
    pub space: String,
    pub navigation_epoch: u64,
    owner: Option<(TaskScope, Status)>,
}

fn validate_tab_audio_state(state: &State, target: &TabAudioTarget) -> Result<(), String> {
    let current = !state.spaces.overview()
        && state.spaces.active_tab() == Some(target.tab)
        && state.spaces.active_space() == target.space
        && state.spaces.tab_space(target.tab) == Some(target.space.as_str())
        && link_owner(&state.tasks, target.tab) == target.owner
        && state.tabs.get(&target.tab).is_some_and(|tab| {
            !tab.rejected_creation
                && tab.navigation.renderer_exit.is_none()
                && tab.navigation.epoch == target.navigation_epoch
        });
    if current {
        Ok(())
    } else {
        Err("음소거할 탭이나 페이지의 제어권이 바뀌었습니다. 메뉴를 다시 열어 주세요.".into())
    }
}

pub(crate) fn capture_tab_audio_target(
    tab: i32,
    space: &str,
    navigation_epoch: u64,
) -> Result<TabAudioTarget, String> {
    STATE.with(|slot| {
        let state = slot.borrow();
        let target = TabAudioTarget {
            tab,
            space: space.to_owned(),
            navigation_epoch,
            owner: link_owner(&state.tasks, tab),
        };
        validate_tab_audio_state(&state, &target)?;
        Ok(target)
    })
}

fn validate_tab_audio_target(target: &TabAudioTarget) -> Result<(), String> {
    STATE.with(|slot| validate_tab_audio_state(&slot.borrow(), target))
}

fn tab_audio_host(target: &TabAudioTarget) -> Result<BrowserHost, String> {
    let browser = STATE.with(|slot| {
        let state = slot.borrow();
        validate_tab_audio_state(&state, target)?;
        Ok::<_, String>(state.tabs[&target.tab].browser.clone())
    })?;
    // Both the host getter and audio getter can call CEF. Neither crosses STATE.
    let host = browser
        .host()
        .ok_or_else(|| "이 탭의 음소거 설정을 읽을 수 없습니다.".to_owned())?;
    validate_tab_audio_target(target)?;
    Ok(host)
}

fn record_tab_audio(target: &TabAudioTarget, muted: bool) -> Result<(), String> {
    let changed = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        validate_tab_audio_state(&state, target)?;
        let tab = state.tabs.get_mut(&target.tab).unwrap();
        let changed = tab.audio_muted != Some(muted);
        tab.audio_muted = Some(muted);
        Ok::<_, String>(changed)
    })?;
    if changed {
        notify_panel();
    }
    Ok(())
}

pub(crate) fn read_tab_audio(target: &TabAudioTarget) -> Result<bool, String> {
    let host = tab_audio_host(target)?;
    let muted = host.is_audio_muted() != 0;
    record_tab_audio(target, muted)?;
    Ok(muted)
}

pub(crate) fn set_tab_audio_muted(
    target: &TabAudioTarget,
    expected: bool,
    muted: bool,
    still_current: impl Fn() -> bool,
) -> Result<bool, String> {
    let ui_changed = || "음소거 메뉴의 대상이 바뀌었습니다. 메뉴를 다시 열어 주세요.".to_owned();
    if !still_current() {
        return Err(ui_changed());
    }
    let host = tab_audio_host(target)?;
    let before = host.is_audio_muted() != 0;
    record_tab_audio(target, before)?;
    if before != expected {
        return Err("탭의 음소거 상태가 바뀌었습니다. 메뉴를 다시 열어 주세요.".into());
    }
    if !still_current() {
        return Err(ui_changed());
    }
    validate_tab_audio_target(target)?;
    // Muting this WebContents is browser chrome, not page input or AI handoff.
    host.set_audio_muted(i32::from(muted));
    // A native callback can close the tab during the setter. Do not query the
    // retained host after OnBeforeClose has removed its live tab.
    validate_tab_audio_target(target).map_err(|_| {
        "음소거 변경을 요청했지만 탭이 바뀌어 결과를 확인하지 못했습니다.".to_owned()
    })?;
    let actual = host.is_audio_muted() != 0;
    if record_tab_audio(target, actual).is_err() {
        return Err("음소거 변경을 요청했지만 탭이 바뀌어 결과를 확인하지 못했습니다.".into());
    }
    if actual != muted {
        return Err("음소거 변경을 요청했지만 설정에 반영되지 않았습니다.".into());
    }
    // Chromium's audio device update is asynchronous. This confirms its native
    // requested setting, not when the physical speaker reaches silence.
    Ok(actual)
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
        let spaces = state.spaces.views();
        ShellSnapshot {
            active_space: state.spaces.active_space().to_owned(),
            active_tab: state.spaces.active_tab(),
            overview: state.spaces.overview(),
            tabs: spaces
                .iter()
                .flat_map(|space| space.tabs.iter())
                .filter_map(|id| state.tabs.get(id).map(|tab| (id, tab)))
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
                    let loading = tab.browser.is_loading() != 0;
                    ShellTab {
                        id: *id,
                        navigation_epoch: tab.navigation.epoch,
                        space: state.spaces.tab_space(*id).unwrap_or(USER_SPACE).to_owned(),
                        title: tab.title.chars().take(120).collect(),
                        url: tab.navigation.requested_url.clone(),
                        can_go_back: tab.browser.can_go_back() != 0,
                        can_go_forward: tab.browser.can_go_forward() != 0,
                        loading,
                        crashed: tab.navigation.renderer_exit.is_some(),
                        audio_muted: tab.audio_muted,
                        failure: tab.navigation.page_failure(loading),
                        start_page: tab.start_page_ready(loading),
                        task_status,
                        stop_scope,
                    }
                })
                .collect(),
            spaces,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextMenuTarget {
    tab: i32,
    space: String,
    frame_id: String,
    navigation_epoch: u64,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
    link_url: Option<String>,
}

fn context_menu_target(state: &State, tab: i32) -> Option<ContextMenuTarget> {
    let page = state.tabs.get(&tab)?;
    let target = ContextMenuTarget {
        tab,
        space: state.spaces.tab_space(tab)?.to_owned(),
        frame_id: String::new(),
        navigation_epoch: page.navigation.epoch,
        document_epoch: page.navigation.document_epoch,
        owner: link_owner(&state.tasks, tab),
        link_url: None,
    };
    context_menu_target_matches(state, &target).then_some(target)
}

fn context_menu_target_matches(state: &State, target: &ContextMenuTarget) -> bool {
    !state.spaces.overview()
        && state.spaces.active_tab() == Some(target.tab)
        && state.spaces.active_space() == target.space
        && state.spaces.tab_space(target.tab) == Some(target.space.as_str())
        && link_owner(&state.tasks, target.tab) == target.owner
        && target
            .owner
            .as_ref()
            .is_none_or(|(_, status)| matches!(status, Status::Active | Status::Paused))
        && state.tabs.get(&target.tab).is_some_and(|page| {
            !page.rejected_creation
                && page.navigation.renderer_exit.is_none()
                && page.navigation.epoch == target.navigation_epoch
                && page.navigation.document_epoch == target.document_epoch
        })
}

fn dismiss_context_menu(tab: i32) {
    STATE.with(|slot| {
        if let Some(page) = slot.borrow_mut().tabs.get_mut(&tab) {
            page.context_menu = None;
            page.context_menu_generation = page.context_menu_generation.saturating_add(1);
        }
    });
}

fn capture_context_menu(
    tab: i32,
    callback_frame: impl FnOnce() -> Option<String>,
    mut current_frame: impl FnMut(i32) -> Option<String>,
) -> bool {
    let candidate = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let page = state.tabs.get_mut(&tab)?;
        page.context_menu = None;
        let generation = page.context_menu_generation.saturating_add(1);
        page.context_menu_generation = generation;
        if generation == u64::MAX {
            return None;
        }
        context_menu_target(&state, tab).map(|target| (target, generation))
    });
    let Some((mut target, generation)) = candidate else {
        return false;
    };
    // Capture only the callback frame's ID, after invalidating the older menu.
    // Frame getters and native visibility checks may both reenter.
    let Some(frame_id) = callback_frame().filter(|id| !id.is_empty()) else {
        return false;
    };
    target.frame_id = frame_id;
    if !context_menu_command_current(&target, generation)
        || current_frame(tab).as_deref() != Some(target.frame_id.as_str())
        || !context_menu_command_current(&target, generation)
    {
        return false;
    }
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if context_menu_target_matches(&state, &target)
            && state.tabs[&tab].context_menu_generation == generation
        {
            state.tabs.get_mut(&tab).unwrap().context_menu = Some(target);
            true
        } else {
            false
        }
    })
}

fn context_menu_command_current(target: &ContextMenuTarget, generation: u64) -> bool {
    STATE.with(|slot| {
        let state = slot.borrow();
        context_menu_target_matches(&state, target)
            && state.tabs[&target.tab].context_menu_generation == generation
            && state.tabs[&target.tab].context_menu.is_none()
    })
}

fn take_context_menu_command(
    tab: i32,
    callback_frame: impl FnOnce() -> Option<String>,
    mut current_frame: impl FnMut(i32) -> Option<String>,
    require_link: bool,
) -> Option<ContextMenuTarget> {
    let captured = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let page = state.tabs.get_mut(&tab)?;
        page.context_menu
            .take()
            .map(|target| (target, page.context_menu_generation))
    });
    let (mut target, generation) = captured?;
    if (require_link && target.link_url.is_none())
        || !context_menu_command_current(&target, generation)
        || callback_frame().as_deref() != Some(target.frame_id.as_str())
        || !context_menu_command_current(&target, generation)
        || current_frame(tab).as_deref() != Some(target.frame_id.as_str())
        || !context_menu_command_current(&target, generation)
    {
        return None;
    }
    if let Some((scope, Status::Active)) = target.owner.as_mut() {
        let (Some(epoch), Some(revision)) = (
            target.navigation_epoch.checked_add(1),
            scope.revision.checked_add(1),
        ) else {
            return None;
        };
        scope.revision = revision;
        target.navigation_epoch = epoch;
        target.owner.as_mut().unwrap().1 = Status::Paused;
        if !matches!(handoff_page_input(tab), Ok(true)) {
            return None;
        }
        // Our handoff expires refs once. It must not permit a different commit,
        // owner, resumed task, space, selection or nested menu to execute.
        if !context_menu_command_current(&target, generation)
            || current_frame(tab).as_deref() != Some(target.frame_id.as_str())
            || !context_menu_command_current(&target, generation)
        {
            return None;
        }
    }
    Some(target)
}

// CEF's return value is 1 to consume/reject, 0 to execute the default command.
fn execute_context_menu(
    tab: i32,
    callback_frame: impl FnOnce() -> Option<String>,
    current_frame: impl FnMut(i32) -> Option<String>,
) -> i32 {
    i32::from(take_context_menu_command(tab, callback_frame, current_frame, false).is_none())
}

// CEF reserves 26500..=28500 for application menu commands.
const CONTEXT_OPEN_LINK: i32 = 26500;

fn stored_context_menu_current(target: &ContextMenuTarget, generation: u64) -> bool {
    STATE.with(|slot| {
        let state = slot.borrow();
        context_menu_target_matches(&state, target)
            && state.tabs[&target.tab].context_menu_generation == generation
            && state.tabs[&target.tab].context_menu.as_ref() == Some(target)
    })
}

fn add_context_link(
    tab: i32,
    link_url: impl FnOnce() -> Option<String>,
    mut current_frame: impl FnMut(i32) -> Option<String>,
    add_item: impl FnOnce() -> bool,
) {
    let captured = STATE.with(|slot| {
        let state = slot.borrow();
        let page = state.tabs.get(&tab)?;
        Some((page.context_menu.clone()?, page.context_menu_generation))
    });
    let Some((target, generation)) = captured else {
        return;
    };
    if target.link_url.is_some() {
        return;
    }
    let mut current = || {
        stored_context_menu_current(&target, generation)
            && current_frame(tab).as_deref() == Some(target.frame_id.as_str())
            && stored_context_menu_current(&target, generation)
    };
    if !current() {
        return;
    }
    let Some(url) = link_url().and_then(|url| crate::address::normalize_url(&url).ok()) else {
        return;
    };
    if !current() || !add_item() || !current() {
        return;
    }
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        // No native call intervenes between the final check and this update.
        state
            .tabs
            .get_mut(&tab)
            .unwrap()
            .context_menu
            .as_mut()
            .unwrap()
            .link_url = Some(url);
    });
}

fn context_link_url(params: Option<&ContextMenuParams>) -> Option<String> {
    let value = params?.link_url();
    let raw: Option<&sys::_cef_string_utf16_t> = (&value).into();
    let raw = raw?;
    if raw.str_.is_null() || raw.length == 0 || raw.length > 8192 {
        return None;
    }
    // CEF owns this valid UTF-16 buffer for the lifetime of `value`. Bound the
    // copy before conversion; normalize_url also bounds the resulting UTF-8 URL.
    String::from_utf16(unsafe { std::slice::from_raw_parts(raw.str_, raw.length) }).ok()
}

#[derive(Debug, PartialEq, Eq)]
enum ContextLinkResult {
    Rejected,
    Queued,
    Failed,
}

fn execute_context_link(
    tab: i32,
    callback_frame: impl FnOnce() -> Option<String>,
    current_frame: impl FnMut(i32) -> Option<String>,
    post: impl FnOnce(String, LinkCreation, Option<u64>) -> bool,
) -> ContextLinkResult {
    let Some(target) = take_context_menu_command(tab, callback_frame, current_frame, true) else {
        return ContextLinkResult::Rejected;
    };
    let link = LinkCreation {
        opener: tab,
        owner: target.owner,
        foreground: false,
        expected_space: Some(target.space),
    };
    // The accepted URL is an intent, independent of later menu dismissal or
    // page selection. The opener's lifetime, ownership and space remain bound.
    if !STATE.with(|slot| link_is_current(&slot.borrow(), &link)) {
        return ContextLinkResult::Failed;
    }
    let Ok(reservation) = reserve_link_creation(&link) else {
        return ContextLinkResult::Failed;
    };
    if post(target.link_url.unwrap(), link, reservation) {
        ContextLinkResult::Queued
    } else {
        if let Some(ticket) = reservation {
            STATE.with(|slot| {
                slot.borrow_mut().creations.release(ticket);
            });
        }
        ContextLinkResult::Failed
    }
}

fn context_link_creation_failed(link: &LinkCreation) {
    #[cfg(not(test))]
    if link.expected_space.is_some() {
        crate::control_panel::context_link_failed(link.opener);
    }
    #[cfg(test)]
    let _ = link;
}

fn context_menu_frame_id(frame: Option<&Frame>) -> Option<String> {
    let frame = frame.filter(|frame| frame.is_valid() != 0)?;
    let id = CefString::from(&frame.identifier()).to_string();
    (!id.is_empty()).then_some(id)
}

fn context_menu_current_frame(tab: i32) -> Option<String> {
    #[cfg(not(test))]
    {
        if !crate::control_panel::context_menu_target_is_visible(tab) {
            return None;
        }
        // Visibility getters may have closed the tab. Obtain its live browser
        // afterwards, and make every CEF call outside the STATE borrow.
        let browser = STATE.with(|slot| {
            slot.borrow()
                .tabs
                .get(&tab)
                .filter(|page| !page.rejected_creation && page.navigation.renderer_exit.is_none())
                .map(|page| page.browser.clone())
        })?;
        let frame = browser.focused_frame()?;
        context_menu_frame_id(Some(&frame))
    }
    #[cfg(test)]
    {
        let _ = tab;
        None
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CurrentPageTarget {
    pub tab: i32,
    pub space: String,
    pub navigation_epoch: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DownloadSiteTarget {
    pub page: CurrentPageTarget,
    pub document_epoch: u64,
    pub owner: Option<(TaskScope, Status)>,
}

fn validate_current_page_state(state: &State, target: &CurrentPageTarget) -> Result<(), String> {
    crate::address::normalize_url(&target.url).map_err(|error| error.to_string())?;
    let valid = !state.spaces.overview()
        && state.spaces.active_space() == target.space
        && state.spaces.active_tab() == Some(target.tab)
        && state.spaces.tab_space(target.tab) == Some(target.space.as_str())
        && state.tabs.get(&target.tab).is_some_and(|tab| {
            let navigation = &tab.navigation;
            !tab.rejected_creation
                && navigation.epoch == target.navigation_epoch
                && navigation.requested_url == target.url
                && navigation.committed
                && navigation.document_available
                && !navigation.provisional
                && !navigation.intent_pending
                && !navigation.recovering
                && navigation.error.is_none()
                && navigation.renderer_exit.is_none()
        });
    if !valid {
        return Err("현재 페이지가 바뀌었습니다. 페이지를 확인한 뒤 다시 시도하세요.".into());
    }
    Ok(())
}

/// Native menu validation reads state only; it never queries CEF or changes AI control.
pub(crate) fn validate_current_page_target(target: &CurrentPageTarget) -> Result<(), String> {
    STATE.with(|state| validate_current_page_state(&state.borrow(), target))
}

fn validate_current_page_source(
    state: &State,
    target: &CurrentPageTarget,
    owner: &Option<(TaskScope, Status)>,
    ui_current: bool,
) -> Result<(), String> {
    if !ui_current {
        return Err("현재 페이지 화면이 바뀌었습니다. 다시 시도하세요.".into());
    }
    validate_current_page_state(state, target)?;
    if link_owner(&state.tasks, target.tab) != *owner {
        return Err("현재 페이지의 제어권이 바뀌었습니다. 다시 시도하세요.".into());
    }
    Ok(())
}

fn current_page_address(target: &CurrentPageTarget, frame_url: &str) -> Result<String, String> {
    let expected = crate::address::normalize_url(&target.url).map_err(|error| error.to_string())?;
    let actual = crate::address::normalize_url(frame_url).map_err(|error| error.to_string())?;
    if actual != expected {
        return Err("페이지 주소가 바뀌었습니다. 페이지를 확인한 뒤 다시 시도하세요.".into());
    }
    Ok(actual)
}

fn read_current_page_address(
    browser: &Browser,
    target: &CurrentPageTarget,
) -> Result<String, String> {
    let frame = browser
        .main_frame()
        .ok_or_else(|| "페이지 주소를 읽을 수 없습니다.".to_owned())?;
    current_page_address(target, &CefString::from(&frame.url()).to_string())
}

/// Capture native setting identity without querying CEF or changing AI control.
pub(crate) fn capture_download_site_target(
    page: &CurrentPageTarget,
) -> Result<DownloadSiteTarget, String> {
    STATE.with(|state| {
        let state = state.borrow();
        validate_current_page_state(&state, page)?;
        Ok(DownloadSiteTarget {
            page: page.clone(),
            document_epoch: state.tabs.get(&page.tab).unwrap().navigation.document_epoch,
            owner: link_owner(&state.tasks, page.tab),
        })
    })
}

fn validate_download_site_state(state: &State, target: &DownloadSiteTarget) -> Result<(), String> {
    // Native overlay visibility is checked by the caller. Reuse the same live
    // page and ownership checks without imposing the menu's no-overlay rule.
    validate_current_page_source(state, &target.page, &target.owner, true)?;
    if state
        .tabs
        .get(&target.page.tab)
        .unwrap()
        .navigation
        .document_epoch
        != target.document_epoch
    {
        return Err("현재 페이지의 문서가 바뀌었습니다. 다시 시도하세요.".into());
    }
    Ok(())
}

pub(crate) fn validate_download_site_target(target: &DownloadSiteTarget) -> Result<(), String> {
    STATE.with(|state| validate_download_site_state(&state.borrow(), target))
}

/// Read the actual main-frame URL, then reject any reentrant document or owner
/// change. Callers must revalidate again after their own CEF/UI calls.
pub(crate) fn read_download_site_page(
    target: &DownloadSiteTarget,
) -> Result<(Browser, String), String> {
    let browser = STATE.with(|state| {
        let state = state.borrow();
        validate_download_site_state(&state, target)?;
        Ok::<_, String>(state.tabs.get(&target.page.tab).unwrap().browser.clone())
    })?;
    let url = read_current_page_address(&browser, &target.page)?;
    validate_download_site_target(target)?;
    Ok((browser, url))
}

fn can_discard_user_staging(state: &State, tab: i32, source: Option<i32>) -> bool {
    Some(tab) != source && state.tabs.contains_key(&tab) && is_unowned_user_tab(state, tab)
}

fn discard_user_staging(browser: &Browser, tab: i32, source: Option<i32>) {
    if !STATE.with(|state| can_discard_user_staging(&state.borrow(), tab, source)) {
        return;
    }
    let Some(host) = browser.host() else {
        return;
    };
    // Even host lookup may reenter. An adopted or moved destination now belongs
    // to its new owner; only our still-unowned staged user tab may be closed.
    let close = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !can_discard_user_staging(&state, tab, source) {
            return false;
        }
        // A never-accepted destination must not enter recent closed-tab history.
        state.tabs.get_mut(&tab).unwrap().rejected_creation = true;
        true
    });
    if close {
        host.close_browser(1);
    }
}

/// Open only the current HTTP(S) address in a new user tab, without AI handoff.
pub(crate) fn open_current_page_in_user_tab(target: &CurrentPageTarget) -> Result<(), String> {
    let (browser, owner) = STATE.with(|state| {
        let state = state.borrow();
        validate_current_page_state(&state, target)?;
        Ok::<_, String>((
            state.tabs.get(&target.tab).unwrap().browser.clone(),
            link_owner(&state.tasks, target.tab),
        ))
    })?;
    // CEF calls stay outside every STATE borrow. The address editor's draft and
    // the previously requested URL cannot substitute for this live frame URL.
    let url = read_current_page_address(&browser, target)?;
    let ui_current = crate::control_panel::current_page_target_is_current(target);
    STATE
        .with(|state| validate_current_page_source(&state.borrow(), target, &owner, ui_current))?;
    // Stage hidden: attachment failure or a reentrant source change must not
    // select a half-created tab or disturb the source space's saved selection.
    let created = create_user_staging(&url)
        .ok_or_else(|| "주소를 열 새 탭을 만들 수 없습니다.".to_owned())?;
    let tab = created.identifier();
    let result = read_current_page_address(&browser, target)
        .and_then(|_| {
            // A CEF call can open a native overlay or IME composition without
            // changing navigation state. Query UI outside STATE, then recheck
            // source identity immediately before selecting the new user tab.
            let ui_current = crate::control_panel::current_page_target_is_current(target);
            STATE.with(|state| {
                let state = state.borrow();
                validate_current_page_source(&state, target, &owner, ui_current)?;
                if tab == target.tab
                    || !is_unowned_user_tab(&state, tab)
                    || !state
                        .tabs
                        .get(&tab)
                        .is_some_and(|tab| !tab.rejected_creation)
                {
                    return Err("새 탭을 브라우저 창에 연결할 수 없습니다.".to_owned());
                }
                Ok(())
            })
        })
        .and_then(|()| select_user_tab(tab));
    if result.is_ok() {
        accept_user_staging(tab);
    }
    if result.is_err() {
        discard_user_staging(&created, tab, Some(target.tab));
    }
    result
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

fn accept_user_staging(id: i32) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if is_unowned_user_tab(&state, id) {
            if let Some(tab) = state.tabs.get_mut(&id).filter(|tab| !tab.rejected_creation) {
                tab.navigation.history.staged = false;
            }
        }
    });
    queue_history_observation(id);
}

fn history_page_ready(state: &State, id: i32, epoch: u64, document_epoch: u64) -> bool {
    is_unowned_user_tab(state, id)
        && state.tabs.get(&id).is_some_and(|tab| {
            !tab.rejected_creation
                && !tab.navigation.history.staged
                && tab.navigation.epoch == epoch
                && tab.navigation.document_epoch == document_epoch
                && tab.navigation.preview_ready()
                && !tab.navigation.recovering
                && tab
                    .navigation
                    .history
                    .document
                    .as_ref()
                    .is_some_and(|document| document.epoch == document_epoch)
        })
}

fn queue_history_observation(id: i32) {
    let queued = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tabs.get_mut(&id) else {
            return false;
        };
        let history = &mut tab.navigation.history;
        if history.queued || history.document.is_none() {
            return false;
        }
        history.queued = true;
        true
    });
    if !queued {
        return;
    }
    #[cfg(not(test))]
    {
        let mut task = HistoryObservationTask::new(id);
        if post_task(ThreadId::UI, Some(&mut task)) != 0 {
            return;
        }
    }
    STATE.with(|state| {
        if let Some(tab) = state.borrow_mut().tabs.get_mut(&id) {
            tab.navigation.history.queued = false;
        }
    });
}

fn observe_history(id: i32) {
    let source = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let tab = state.tabs.get_mut(&id)?;
        tab.navigation.history.queued = false;
        let epoch = tab.navigation.epoch;
        let document_epoch = tab.navigation.document_epoch;
        if !history_page_ready(&state, id, epoch, document_epoch) {
            return None;
        }
        let tab = &state.tabs[&id];
        Some((tab.browser.clone(), epoch, document_epoch))
    });
    let Some((browser, epoch, document_epoch)) = source else {
        return;
    };
    // A same-document address notification can precede a real commit in the
    // same CEF turn. Defer and verify both identities and the live frame URL.
    if browser.is_loading() != 0 {
        return;
    }
    let Some(frame) = browser.main_frame() else {
        return;
    };
    let Ok(url) = crate::address::normalize_url(&CefString::from(&frame.url()).to_string()) else {
        return;
    };
    let action = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !history_page_ready(&state, id, epoch, document_epoch) {
            return None;
        }
        let tab = state.tabs.get_mut(&id)?;
        if crate::address::normalize_url(&tab.navigation.requested_url)
            .ok()
            .as_deref()
            != Some(url.as_str())
        {
            return None;
        }
        // A settled same-document navigation has no LoadEnd/loading callback.
        tab.navigation.history.attempt = None;
        tab.navigation.history.action(url)
    });
    let Some(action) = action else {
        return;
    };
    // Runtime writes/notifications may reenter. Never retain STATE across them.
    let accepted = if action.update_title {
        crate::history_runtime::update_title(
            action.generation,
            action.url.clone(),
            action.title.clone(),
        )
    } else {
        crate::history_runtime::record(action.generation, action.url.clone(), action.title.clone())
    };
    if accepted {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if history_page_ready(&state, id, epoch, document_epoch) {
                state
                    .tabs
                    .get_mut(&id)
                    .unwrap()
                    .navigation
                    .history
                    .accepted(&action);
            }
        });
    }
}

wrap_task! {
    struct HistoryObservationTask { tab: i32 }
    impl Task {
        fn execute(&self) { observe_history(self.tab); }
    }
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
    let result = create_browser_with_context(
        &entry.url,
        None,
        None,
        None,
        Some(USER_SPACE.into()),
        true,
        false,
    )
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
        select_user_tab(tab)
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

fn select_user_tab(tab: i32) -> Result<(), String> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !is_unowned_user_tab(&state, tab) {
            return Err("열려는 사용자 탭을 찾을 수 없습니다.".to_owned());
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
    open_saved_user_url(url, || true)
}

pub(crate) fn open_history_url(url: &str, still_current: impl Fn() -> bool) -> Result<(), String> {
    open_saved_user_url(url, still_current)
}

/// Explicit native search submission shares user-tab reuse and staging. The
/// caller guards the query revision; merely typing never reaches this path.
pub(crate) fn open_search_url(url: &str, still_current: impl Fn() -> bool) -> Result<(), String> {
    open_saved_user_url(url, still_current)
}

fn user_open_selection(state: &State) -> (String, Option<i32>, bool) {
    (
        state.spaces.active_space().to_owned(),
        state.spaces.active_tab(),
        state.spaces.overview(),
    )
}

fn validate_saved_user_destination(
    state: &State,
    tab: i32,
    selection: &(String, Option<i32>, bool),
    still_current: bool,
) -> Result<(), String> {
    if !still_current
        || user_open_selection(state) != *selection
        || !is_unowned_user_tab(state, tab)
        || !state
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
    {
        return Err("열려는 항목이나 현재 화면이 바뀌었습니다. 다시 시도하세요.".into());
    }
    Ok(())
}

fn open_saved_user_url(url: &str, still_current: impl Fn() -> bool) -> Result<(), String> {
    let url = crate::address::normalize_url(url).map_err(|error| error.to_string())?;
    if !still_current() {
        return Err("열려는 항목이 더 이상 없습니다.".into());
    }
    let (selection, existing) = STATE.with(|state| {
        let state = state.borrow();
        let existing = prefer_bookmark_tab(
            state.spaces.active_tab(),
            state.tabs.iter().filter_map(|(&id, tab)| {
                (is_unowned_user_tab(&state, id)
                    && !tab.rejected_creation
                    && tab.navigation.can_reuse_bookmark(&url))
                .then_some(id)
            }),
        );
        (user_open_selection(&state), existing)
    });
    if let Some(tab) = existing {
        let current = still_current();
        STATE.with(|state| -> Result<(), String> {
            let state = state.borrow();
            validate_saved_user_destination(&state, tab, &selection, current)?;
            if !state.tabs[&tab].navigation.can_reuse_bookmark(&url) {
                return Err("열려는 페이지가 바뀌었습니다. 다시 시도하세요.".into());
            }
            Ok(())
        })?;
        return select_user_tab(tab);
    }
    if !still_current() {
        return Err("열려는 항목이 더 이상 없습니다.".into());
    }
    // Saved URLs and explicit search submissions stage in the background, so
    // a failed attachment or changed source preserves the original selection.
    let browser = create_user_staging(&url)
        .ok_or_else(|| "주소를 열 새 탭을 만들 수 없습니다.".to_owned())?;
    let tab = browser.identifier();
    let current = still_current();
    let result = STATE
        .with(|state| validate_saved_user_destination(&state.borrow(), tab, &selection, current))
        .and_then(|()| select_user_tab(tab));
    if result.is_ok() {
        accept_user_staging(tab);
    }
    if result.is_err() {
        discard_user_staging(&browser, tab, selection.1);
    }
    result
}

fn validate_sidebar_close(state: &State, tab: i32, expected_space: &str) -> Result<(), String> {
    if state.spaces.overview()
        || state.spaces.active_space() != expected_space
        || state.spaces.tab_space(tab) != Some(expected_space)
        || !state
            .tabs
            .get(&tab)
            .is_some_and(|tab| !tab.rejected_creation)
    {
        return Err("The tab is no longer in the displayed space.".into());
    }
    Ok(())
}

pub(crate) fn close_tab_from_sidebar(tab: i32, expected_space: &str) -> Result<(), String> {
    // The native row can outlive an adoption/release until the coalesced UI
    // refresh. Validate live membership before handing control away from AI.
    // Both steps run on the UI thread; no CEF call crosses this STATE borrow.
    STATE.with(|state| validate_sidebar_close(&state.borrow(), tab, expected_space))?;
    shell_action(ShellAction::CloseTab(tab))
}

pub(crate) fn select_tab_from_sidebar(tab: i32, expected_space: &str) -> Result<(), String> {
    // A coalesced native snapshot can retain a moved or closed tab. Validate
    // live membership before any selection, focus, or shell synchronization.
    // Overview is allowed: selecting a live tab exits it through SelectTab.
    STATE.with(|state| {
        let state = state.borrow();
        if state.spaces.active_space() != expected_space
            || state.spaces.tab_space(tab) != Some(expected_space)
            || !state
                .tabs
                .get(&tab)
                .is_some_and(|tab| !tab.rejected_creation)
        {
            return Err("The tab is no longer in the displayed space.".to_owned());
        }
        Ok(())
    })?;
    // Both steps run synchronously on the UI thread without intervening CEF
    // or AppKit calls. Reuse selection's existing no-handoff path.
    shell_action(ShellAction::SelectTab(tab))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabOrderTarget {
    pub tab: i32,
    pub space: String,
    pub neighbor: i32,
    pub forward: bool,
}

fn validate_tab_order_state(state: &State, target: &TabOrderTarget) -> Result<(), String> {
    let order = state.spaces.ordered_tabs(&target.space);
    let index = order.iter().position(|id| *id == target.tab);
    let adjacent = index.and_then(|index| {
        if target.forward {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        }
    });
    if state.spaces.overview()
        || state.spaces.active_space() != target.space
        || state.spaces.active_tab() != Some(target.tab)
        || adjacent.and_then(|index| order.get(index)) != Some(&target.neighbor)
        || [target.tab, target.neighbor].into_iter().any(|id| {
            state.spaces.tab_space(id) != Some(target.space.as_str())
                || !state
                    .tabs
                    .get(&id)
                    .is_some_and(|tab| !tab.rejected_creation)
        })
    {
        return Err("탭 순서나 선택이 바뀌었습니다. 메뉴를 다시 열어 주세요.".into());
    }
    Ok(())
}

pub(crate) fn validate_tab_order_target(target: &TabOrderTarget) -> Result<(), String> {
    STATE.with(|state| validate_tab_order_state(&state.borrow(), target))
}

pub(crate) fn reorder_tab_from_sidebar(target: &TabOrderTarget) -> Result<(), String> {
    // Presentation metadata only: no CEF/AppKit call, selection, navigation or
    // AI handoff occurs between validation and the adjacent swap.
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        validate_tab_order_state(&state, target)?;
        if !state.spaces.reorder_adjacent(
            target.tab,
            &target.space,
            target.neighbor,
            target.forward,
        ) {
            return Err("현재 탭을 이동할 수 없습니다.".into());
        }
        Ok(())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabDragTarget {
    pub tab: i32,
    pub space: String,
    pub selected: Option<i32>,
    pub order: Vec<i32>,
}

fn validate_tab_drag_state(state: &State, target: &TabDragTarget) -> Result<(), String> {
    if state.spaces.overview()
        || state.spaces.active_space() != target.space
        || state.spaces.active_tab() != target.selected
        || state.spaces.ordered_tabs(&target.space) != target.order.as_slice()
        || !target.order.contains(&target.tab)
        || target.order.iter().any(|id| {
            state.spaces.tab_space(*id) != Some(target.space.as_str())
                || !state.tabs.get(id).is_some_and(|tab| !tab.rejected_creation)
        })
    {
        return Err("탭 목록이나 선택이 바뀌었습니다. 다시 드래그해 주세요.".into());
    }
    Ok(())
}

pub(crate) fn validate_tab_drag_target(target: &TabDragTarget) -> Result<(), String> {
    STATE.with(|slot| validate_tab_drag_state(&slot.borrow(), target))
}

pub(crate) fn reorder_tab_by_drag(
    target: &TabDragTarget,
    before: Option<i32>,
) -> Result<bool, String> {
    // Reordering a background row must not select/focus it or hand off its AI
    // owner. Validate and mutate presentation metadata within one STATE borrow.
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        validate_tab_drag_state(&state, target)?;
        if before == Some(target.tab) {
            return Ok(false);
        }
        state
            .spaces
            .reorder_before(target.tab, &target.space, before, &target.order)
            .ok_or_else(|| "탭을 놓을 위치가 바뀌었습니다. 다시 드래그해 주세요.".into())
    })
}

fn validate_recovery_state(
    state: &State,
    target: &PageRecoveryTarget,
    loading: bool,
) -> Result<(), String> {
    let current = !state.spaces.overview()
        && state.spaces.active_tab() == Some(target.tab)
        && state.spaces.active_space() == target.space
        && state.spaces.tab_space(target.tab) == Some(target.space.as_str())
        && state.tabs.get(&target.tab).is_some_and(|tab| {
            !tab.rejected_creation
                && tab.navigation.epoch == target.navigation_epoch
                && tab.navigation.page_failure(loading) == Some(target.failure)
        });
    if !current {
        return Err("페이지 상태가 바뀌었습니다. 현재 페이지를 확인해 주세요.".into());
    }
    Ok(())
}

fn recovery_loading(tab: i32) -> Result<bool, String> {
    let browser = STATE.with(|state| {
        state
            .borrow()
            .tabs
            .get(&tab)
            .filter(|tab| !tab.rejected_creation)
            .map(|tab| tab.browser.clone())
    });
    // Even a read-only CEF call stays outside the STATE borrow.
    browser
        .map(|browser| browser.is_loading() != 0)
        .ok_or_else(|| "페이지가 닫혔습니다.".into())
}

pub(crate) fn validate_failed_page_target(target: &PageRecoveryTarget) -> Result<(), String> {
    let loading = recovery_loading(target.tab)?;
    STATE.with(|state| validate_recovery_state(&state.borrow(), target, loading))
}

pub(crate) fn retry_failed_page(target: &PageRecoveryTarget) -> Result<(), String> {
    validate_failed_page_target(target)?;
    // Address correction remains available for a terminated renderer whose
    // loading flag lags. Retry must not enter Reload's loading-stop toggle.
    if recovery_loading(target.tab)? {
        return Err("페이지가 아직 로딩 중입니다. 잠시 후 다시 시도해 주세요.".into());
    }
    // The native Reload path hands off only this tab's current AI owner and
    // preserves browser-level renderer recovery without requiring a live frame.
    shell_action(ShellAction::Reload(target.tab))
}

fn validate_start_page_state(
    state: &State,
    target: &StartPageTarget,
    loading: bool,
    main_frame_is_blank: bool,
) -> Result<(), String> {
    let current = main_frame_is_blank
        && !state.spaces.overview()
        && state.spaces.active_tab() == Some(target.tab)
        && state.spaces.active_space() == target.space
        && state.spaces.tab_space(target.tab) == Some(target.space.as_str())
        && state.tabs.get(&target.tab).is_some_and(|tab| {
            tab.navigation.epoch == target.navigation_epoch && tab.start_page_ready(loading)
        });
    if !current {
        return Err("탭 상태가 바뀌었습니다. 현재 탭을 확인해 주세요.".into());
    }
    Ok(())
}

pub(crate) fn validate_start_page_target(target: &StartPageTarget) -> Result<(), String> {
    let browser = STATE.with(|state| {
        state
            .borrow()
            .tabs
            .get(&target.tab)
            .filter(|tab| !tab.rejected_creation)
            .map(|tab| tab.browser.clone())
    });
    let browser = browser.ok_or_else(|| "탭이 닫혔습니다.".to_owned())?;
    // Query CEF outside the STATE borrow, then revalidate the current identity.
    let loading = browser.is_loading() != 0;
    let main_frame_is_blank = browser
        .main_frame()
        .is_some_and(|frame| CefString::from(&frame.url()).to_string() == "about:blank");
    STATE.with(|state| {
        validate_start_page_state(&state.borrow(), target, loading, main_frame_is_blank)
    })
}

pub(crate) fn navigate_from_start_page(
    target: &StartPageTarget,
    input: &str,
) -> Result<(), String> {
    // Empty/invalid input and stale controls must never hand off an AI task.
    let Some(url) = crate::address::resolve(input).map_err(|error| error.to_string())? else {
        return Ok(());
    };
    validate_start_page_target(target)?;
    shell_action(ShellAction::Navigate(target.tab, url))
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
            create_browser_with_context("about:blank", None, None, None, Some(space), true, true)
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
    if crate::control_panel::focus_start_page_if_visible(id) {
        return;
    }
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
        // Unlike worker slots, queued commands retain their permit after the
        // connection times out while the UI thread is not draining its queue.
        let outstanding_commands = Arc::new(AtomicUsize::new(0));
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
            let outstanding_commands = outstanding_commands.clone();
            workers.push(thread::spawn(move || {
                serve_connection(stream, stopped, token, outstanding_commands)
            }));
        }
        for worker in workers {
            let _ = worker.join();
        }
    })
}

fn serve_connection(
    mut stream: UnixStream,
    stopped: Arc<AtomicBool>,
    token: u64,
    outstanding_commands: Arc<AtomicUsize>,
) {
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
    let permit = match CommandPermit::acquire(&outstanding_commands, &id) {
        Ok(permit) => permit,
        Err(response) => {
            let _ = protocol::write_response(&mut stream, &response);
            return;
        }
    };
    let active = Arc::new(AtomicBool::new(true));
    let (sender, receiver) = mpsc::channel();
    let mut task = CommandTask::new(Arc::new(Mutex::new(Some(QueuedCommand {
        request,
        reply: Reply {
            id: id.clone(),
            token,
            active: active.clone(),
            sender,
            scope: None,
        },
        _permit: permit,
    }))));
    let posted = !stopped.load(Ordering::Acquire) && post_task(ThreadId::UI, Some(&mut task)) != 0;
    // CEF's accepted post owns a reference. Do not retain the task payload in
    // the worker until the response deadline, or after a rejected post.
    drop(task);
    if !posted {
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

struct QueuedCommand {
    request: Request,
    reply: Reply,
    _permit: CommandPermit,
}

wrap_task! {
    // The generated wrapper requires Clone. Share one payload and permit rather
    // than cloning either; a discarded native task releases both on last drop.
    struct CommandTask { work: Arc<Mutex<Option<QueuedCommand>>> }
    impl Task {
        fn execute(&self) {
            let work = self.work.lock().unwrap_or_else(|error| error.into_inner()).take();
            if let Some(work) = work {
                // Neither the mutex nor a queued payload is retained by an
                // asynchronous Pending request. The permit drops after handle.
                if work.reply.active.load(Ordering::Acquire) {
                    handle(&work.request.command, work.reply.clone());
                }
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
                context_link_creation_failed(&self.link);
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
        if pending.mutation_dispatched() {
            pending
                .reply
                .error_details(code, message, pending.outcome_details());
        } else {
            pending.reply.error(code, message);
        }
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
        if request.mutation_dispatched() {
            let mut details = request.outcome_details();
            details["tab"] = json!(tab);
            request.reply.error_details(
                "CONTROL_DISCONNECTED",
                "The page disconnected during an action. Inspect before repeating it.",
                details,
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
        let mut details = request.outcome_details();
        details["tab"] = json!(tab);
        details["renderer"] = reason.clone();
        request.reply.error_details(
            "RENDERER_TERMINATED",
            "The renderer stopped. Inspect the tab before retrying an action.",
            details,
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
        let mut details = request.outcome_details();
        details["task"] = json!(task);
        details["tab"] = json!(request.tab);
        request.reply.error_details(
            "TASK_CONTROL_LOST",
            "Task control changed. Inspect the page before continuing.",
            details,
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
                tab.navigation.download_source_generation += 1;
                tab.navigation.download_permission_provenance.revoke();
                tab.navigation.history.revoke();
                tab.navigation.history.staged = false;
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
                | Command::Check { tab, .. }
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
                            evaluation: Evaluation::Page {
                                mutation: false,
                                evidence: None,
                            },
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
                        Command::Check {
                            reference, checked, ..
                        } => action_expression(
                            "check",
                            reference,
                            Some(("checked", ActionValue::Bool(*checked))),
                            epoch,
                            &reply,
                        ),
                        Command::Fill {
                            reference, text, ..
                        } => action_expression(
                            "fill",
                            reference,
                            Some(("text", ActionValue::String(text))),
                            epoch,
                            &reply,
                        ),
                        Command::Select {
                            reference, value, ..
                        } => action_expression(
                            "select",
                            reference,
                            Some(("value", ActionValue::String(value))),
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
                            action_expression(
                                "press",
                                reference,
                                Some(("key", ActionValue::String(key))),
                                epoch,
                                &reply,
                            )
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
                                evidence: ActionEvidence::for_command(other),
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
            tab.navigation.automatic_retry_error = None;
            tab.navigation.history.revoke();
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
    let history_generation = crate::history_runtime::generation();
    let started = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let history_generation = history_generation.filter(|_| is_unowned_user_tab(&state, tab_id));
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
        if let Some(url) = url {
            tab.observe_start_page_url(url);
        }
        tab.navigation.begin(url);
        tab.navigation.history.begin(history_generation, true);
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

enum ActionValue<'a> {
    String(&'a str),
    Bool(bool),
}

fn action_expression(
    action: &str,
    reference: &str,
    value: Option<(&str, ActionValue<'_>)>,
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
        args[key] = match value {
            ActionValue::String(value) => json!(value),
            ActionValue::Bool(value) => json!(value),
        };
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
    let space = link.as_ref().and_then(|link| link.expected_space.clone());
    create_browser_with_context(url, scope, reservation, link, space, foreground, false)
}

fn create_browser_with_context(
    url: &str,
    scope: Option<TaskScope>,
    reservation: Option<u64>,
    link: Option<LinkCreation>,
    space: Option<String>,
    foreground: bool,
    native_start_page: bool,
) -> Option<Browser> {
    create_browser_with_history(
        url,
        scope,
        reservation,
        link,
        space,
        foreground,
        native_start_page,
        HistoryCreation::new(false),
    )
}

fn create_user_staging(url: &str) -> Option<Browser> {
    create_browser_with_history(
        url,
        None,
        None,
        None,
        Some(USER_SPACE.into()),
        false,
        false,
        HistoryCreation::new(true),
    )
}

fn create_browser_with_history(
    url: &str,
    scope: Option<TaskScope>,
    reservation: Option<u64>,
    link: Option<LinkCreation>,
    space: Option<String>,
    foreground: bool,
    native_start_page: bool,
    history: HistoryCreation,
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
    let mut client = BrowserClient::new(
        url.to_owned(),
        scope,
        reservation,
        link,
        space,
        foreground,
        native_start_page && url == "about:blank",
        history,
    );
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

fn press_target_failure(
    state: &State,
    browser: &Browser,
    tab: i32,
    epoch: u64,
    reply: &Reply,
    allow_navigation: bool,
) -> Option<PressFailure> {
    if !reply.active.load(Ordering::Acquire) {
        return Some(PressFailure {
            code: "COMMAND_CANCELED",
            message: "The Enter request is no longer active.",
        });
    }
    if let Err(error) = state.tasks.check_tab(reply.scope.as_ref(), tab) {
        return Some(PressFailure {
            code: error.code,
            message: error.message,
        });
    }
    let Some(tab) = state.tabs.get(&tab) else {
        return Some(PressFailure {
            code: "TAB_CLOSED",
            message: "The tab closed during Enter preparation.",
        });
    };
    if tab.rejected_creation || tab.browser.get_raw() != browser.get_raw() {
        return Some(PressFailure {
            code: "TAB_CLOSED",
            message: "The original browser is no longer available.",
        });
    }
    if tab.navigation.renderer_exit.is_some() {
        return Some(PressFailure {
            code: "RENDERER_TERMINATED",
            message: "The renderer stopped during Enter. Inspect before retrying.",
        });
    }
    if !allow_navigation
        && (tab.navigation.epoch != epoch
            || tab.navigation.provisional
            || !tab.navigation.committed
            || !tab.navigation.document_available)
    {
        return Some(PressFailure {
            code: "PAGE_CHANGED",
            message: "The document changed before Enter finished dispatching. Read again; do not automatically repeat Enter.",
        });
    }
    None
}

fn press_registered(state: &State, batch: &Shared<PressBatch>) -> bool {
    batch.ids.iter().all(|id| state.pending.get(id).is_some_and(|pending| {
        matches!(&pending.step, Step::Press { batch: current, .. } if Shared::ptr_eq(current, batch))
    }))
}

fn remove_press_batch(batch: &Shared<PressBatch>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        for id in batch.ids {
            if state.pending.get(&id).is_some_and(|pending| {
                matches!(&pending.step, Step::Press { batch: current, .. } if Shared::ptr_eq(current, batch))
            }) {
                state.pending.remove(&id);
            }
        }
    });
}

fn finish_press_batch(batch: &Shared<PressBatch>) {
    let (enqueuing, failure, acknowledged, attempted) = {
        let progress = batch.progress.borrow();
        (
            progress.enqueuing,
            progress.failure,
            progress.accepted == [Some(true); 2] && progress.acknowledged == [true; 2],
            progress.attempted.iter().any(|value| *value),
        )
    };
    if enqueuing {
        return;
    }
    let current = STATE.with(|state| {
        let state = state.borrow();
        press_registered(&state, batch).then(|| {
            press_target_failure(
                &state,
                &batch.browser,
                batch.tab,
                batch.epoch,
                &batch.reply,
                true,
            )
        })
    });
    let Some(current_failure) = current else {
        return;
    };
    let failure = failure.or(current_failure);
    if failure.is_none() && !acknowledged {
        return;
    }
    remove_press_batch(batch);
    if let Some(failure) = failure {
        batch.reply.error_details(
            failure.code,
            failure.message,
            json!({"tab":batch.tab,"outcomeUnknown":true,"attempted":attempted}),
        );
    } else {
        batch.reply.ok(json!({"tab":batch.tab,"page":{
            "ok":true,"action":"press","key":"Enter","input":"chromium",
            "attempted":true,"dispatchAcknowledged":true,
            "detail":"Chromium acknowledged Enter keyDown and keyUp; read again to verify effects."
        }}));
    }
}

// CEF documents asynchronous results. Keep both entries even if a transport
// re-enters its observer before returning; a callback alone cannot acknowledge
// dispatch. These adjacent UI submissions are not an atomic renderer operation.
fn enqueue_press_batch(
    batch: &Shared<PressBatch>,
    mut native_available: impl FnMut() -> bool,
    mut submit: impl FnMut(usize, i32) -> i32,
) {
    for part in 0..2 {
        if !STATE.with(|state| press_registered(&state.borrow(), batch)) {
            return;
        }
        if batch.progress.borrow().failure.is_some() {
            break;
        }
        let available = native_available();
        // Native getters can re-enter cancellation, handoff, navigation or close.
        let checked = STATE.with(|state| {
            let state = state.borrow();
            press_registered(&state, batch).then(|| {
                press_target_failure(
                    &state,
                    &batch.browser,
                    batch.tab,
                    batch.epoch,
                    &batch.reply,
                    false,
                )
            })
        });
        let Some(failure) = checked else {
            return;
        };
        if batch.progress.borrow().failure.is_some() {
            break;
        }
        let failure = failure.or_else(|| {
            (!available).then_some(PressFailure {
                code: "TAB_CLOSED",
                message: "The browser is unavailable during Enter dispatch.",
            })
        });
        if let Some(failure) = failure {
            batch.progress.borrow_mut().failure = Some(failure);
            break;
        }
        batch.progress.borrow_mut().attempted[part] = true;
        let receipt = submit(part, batch.ids[part]);
        if !STATE.with(|state| press_registered(&state.borrow(), batch)) {
            return;
        }
        let mut progress = batch.progress.borrow_mut();
        progress.accepted[part] = Some(receipt == batch.ids[part]);
        if receipt != batch.ids[part] {
            progress.failure = Some(PressFailure {
                code: "CONTROL_UNAVAILABLE",
                message: "Chromium rejected an Enter event. Inspect before retrying.",
            });
            break;
        }
    }
    batch.progress.borrow_mut().enqueuing = false;
    finish_press_batch(batch);
}

fn press_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) -> bool {
    let part = STATE.with(|state| {
        let state = state.borrow();
        match &state.pending.get(&id)?.step {
            Step::Press { batch, part } => Some((batch.clone(), *part)),
            _ => None,
        }
    });
    let Some((batch, part)) = part else {
        return false;
    };
    // Input results are empty objects in this protocol. Bound error payloads,
    // and never retain page or Chromium text in the shared completion state.
    let valid_json = (success && bytes.is_empty())
        || (bytes.len() <= 64 * 1024
            && serde_json::from_slice::<Json>(bytes).is_ok_and(|value| value.is_object()));
    let current = STATE.with(|state| {
        let state = state.borrow();
        press_target_failure(&state, browser, batch.tab, batch.epoch, &batch.reply, true)
    });
    {
        let mut progress = batch.progress.borrow_mut();
        if progress.acknowledged[part] {
            return true;
        }
        if !progress.attempted[part] || !valid_json {
            progress.failure = Some(PressFailure {
                code: "INVALID_RESULT",
                message: "Chromium returned an invalid Enter acknowledgement. Inspect before retrying.",
            });
        } else if !success {
            progress.failure = Some(PressFailure {
                code: "CONTROL_ERROR",
                message: "Chromium could not dispatch Enter. Inspect before retrying.",
            });
        } else if let Some(failure) = current {
            progress.failure = Some(failure);
        }
        progress.acknowledged[part] = true;
    }
    finish_press_batch(&batch);
    true
}

fn reserve_press_batch(
    id: i32,
    pending: &Pending,
    browser: &Browser,
) -> Result<Shared<PressBatch>, PressFailure> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(failure) = press_target_failure(
            &state,
            browser,
            pending.tab,
            pending.epoch,
            &pending.reply,
            false,
        ) {
            return Err(failure);
        }
        if !state.pending.get(&id).is_some_and(|current| {
            current.reply.token == pending.reply.token && current.press_preparation()
        }) {
            return Err(PressFailure {
                code: "COMMAND_CANCELED",
                message: "The Enter preparation is no longer current.",
            });
        }
        let Some(last) = state.next_message.checked_add(2) else {
            return Err(PressFailure {
                code: "CONTROL_UNAVAILABLE",
                message: "The control message identifiers are exhausted.",
            });
        };
        let batch = Shared::new(PressBatch {
            ids: [last - 1, last],
            tab: pending.tab,
            epoch: pending.epoch,
            browser: browser.clone(),
            reply: pending.reply.clone(),
            progress: RefCell::new(PressProgress {
                enqueuing: true,
                attempted: [false; 2],
                accepted: [None; 2],
                acknowledged: [false; 2],
                failure: None,
            }),
        });
        state.next_message = last;
        state.pending.remove(&id);
        for (part, id) in batch.ids.iter().enumerate() {
            state.pending.insert(
                *id,
                Pending {
                    step: Step::Press {
                        batch: batch.clone(),
                        part,
                    },
                    ..pending.clone()
                },
            );
        }
        Ok(batch)
    })
}

fn dispatch_press(browser: &Browser, id: i32, pending: &Pending) {
    let host = browser.host();
    let mut dictionaries = ["keyDown", "keyUp"].map(|kind| {
        let mut params = json!({"type":kind,"key":"Enter","code":"Enter","windowsVirtualKeyCode":13,
            "nativeVirtualKeyCode":0});
        if kind == "keyDown" {
            params["text"] = json!("\r");
            params["unmodifiedText"] = json!("\r");
        }
        parse_json(
            Some(&CefString::from(params.to_string().as_str())),
            JsonParserOptions::default(),
        )
        .and_then(|value| value.dictionary())
    });
    // Native code zero prevents Chromium's fallback into native browser key
    // handling. No set_focus, frontmost-window or current-space mutation.
    let available = browser.is_valid() != 0 && browser.has_document() != 0;
    if host.is_none() || dictionaries.iter().any(Option::is_none) || !available {
        finish_press_preparation(
            id,
            pending,
            "CONTROL_UNAVAILABLE",
            "Chromium is unavailable for Enter.",
        );
        return;
    }
    let batch = match reserve_press_batch(id, pending, browser) {
        Ok(batch) => batch,
        Err(failure) => {
            finish_press_preparation(id, pending, failure.code, failure.message);
            return;
        }
    };
    let host = host.unwrap();
    let method = CefString::from("Input.dispatchKeyEvent");
    enqueue_press_batch(
        &batch,
        || browser.is_valid() != 0 && browser.has_document() != 0,
        |part, message| {
            host.execute_dev_tools_method(message, Some(&method), dictionaries[part].as_mut())
        },
    );
}

fn finish_press_preparation(id: i32, pending: &Pending, code: &str, message: &str) {
    let removed = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.pending.get(&id).is_some_and(|current| {
            current.reply.token == pending.reply.token && current.press_preparation()
        }) {
            state.pending.remove(&id);
            true
        } else {
            false
        }
    });
    if !removed {
        return;
    }
    // Preparation can focus a page element, but has dispatched no native keys.
    pending
        .reply
        .error_details(code, message, pending.outcome_details());
}

fn press_preparation_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) -> bool {
    let pending = STATE.with(|state| {
        state
            .borrow()
            .pending
            .get(&id)
            .filter(|pending| pending.press_preparation())
            .cloned()
    });
    let Some(pending) = pending else {
        return false;
    };
    let failure = STATE.with(|state| {
        press_target_failure(
            &state.borrow(),
            browser,
            pending.tab,
            pending.epoch,
            &pending.reply,
            false,
        )
    });
    if let Some(failure) = failure {
        finish_press_preparation(id, &pending, failure.code, failure.message);
        return true;
    }
    let result = (bytes.len() <= 64 * 1024)
        .then(|| serde_json::from_slice::<Json>(bytes).ok())
        .flatten();
    let Some(result) = result else {
        finish_press_preparation(
            id,
            &pending,
            "INVALID_RESULT",
            "Chromium returned an invalid Enter preparation.",
        );
        return true;
    };
    let value = &result["result"]["value"];
    if !success || result.get("exceptionDetails").is_some() {
        finish_press_preparation(
            id,
            &pending,
            "PAGE_SCRIPT_ERROR",
            "Enter preparation failed. Read again before repeating it.",
        );
    } else if value["ok"] == false && value["action"] == "press" {
        finish_press_preparation(
            id,
            &pending,
            value["error"].as_str().unwrap_or("ACTION_FAILED"),
            value["detail"]
                .as_str()
                .unwrap_or("Enter preparation did not complete."),
        );
    } else if value["ok"] == true
        && value["action"] == "press"
        && value["key"] == "Enter"
        && value["prepared"] == true
        && value["attempted"] == false
    {
        dispatch_press(browser, id, &pending);
    } else {
        finish_press_preparation(
            id,
            &pending,
            "INVALID_PAGE_RESULT",
            "The page did not confirm Enter preparation. Read again.",
        );
    }
    true
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
    if matches!(
        pending.evaluation,
        Evaluation::Page {
            evidence: Some(ActionEvidence::Press),
            ..
        }
    ) {
        let available =
            browser.is_valid() != 0 && browser.has_document() != 0 && dictionary.is_some();
        let failure = STATE.with(|state| {
            press_target_failure(
                &state.borrow(),
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                false,
            )
        });
        if let Some(failure) = failure {
            pending_error(&pending, failure.code, failure.message);
            return;
        }
        if !available {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "The browser is unavailable for Enter preparation.",
            );
            return;
        }
    }
    let (id, start_page_revoked) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.next_message += 1;
        let id = state.next_message;
        if capture {
            state.captures.insert(id, pending.tab);
        }
        // Reads, screenshots and action preparation leave native input alone.
        // An actual page mutation gives the CEF document its visible surface.
        let start_page_revoked = pending.mutation_dispatched()
            && state
                .tabs
                .get_mut(&pending.tab)
                .is_some_and(|tab| std::mem::replace(&mut tab.native_start_page, false));
        state.pending.insert(id, pending);
        (id, start_page_revoked)
    });
    let _refresh = PanelRefresh(start_page_revoked);
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
    if press_result(browser, id, success, bytes)
        || press_preparation_result(browser, id, success, bytes)
    {
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
    let already_dispatched_action = pending.mutation_dispatched();
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
                    // For ReadyState, pending_error only releases this completed
                    // probe. The timer still owns the failure and state snapshot;
                    // a navigation that clears the error can issue a fresh probe.
                    pending_error(&pending, "PAGE_LOADING", "The DOM is not ready.");
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
                let mut details = serde_json::Map::new();
                if let Evaluation::Page {
                    evidence: Some(evidence),
                    ..
                } = pending.evaluation
                {
                    // A failed action may already have changed the page. Keep
                    // only bounded boolean evidence for its original request kind.
                    for &key in evidence.fields(&value) {
                        if let Some(value) = value[key].as_bool() {
                            details.insert(key.into(), Json::Bool(value));
                        }
                    }
                }
                if details.is_empty() {
                    reply.error(code, message);
                } else {
                    reply.error_details(code, message, Json::Object(details));
                }
            } else {
                let mut page = value.clone();
                if matches!(
                    pending.evaluation,
                    Evaluation::Page {
                        mutation: false,
                        ..
                    }
                ) {
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
        Step::Press { .. } => {
            unreachable!("Enter results are aggregated before completing a pending request")
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
                if create_browser_with_context("about:blank", None, None, None, None, true, true).is_none() { quit_message_loop(); }
                else { crate::control_panel::show(); }
            });
        }
        fn default_client(&self) -> Option<Client> { Some(BrowserClient::new(String::new(), None, None, None, USER_SPACE.into(), true, false, HistoryCreation::new(false))) }
    }
}
wrap_client! {
    struct BrowserClient { requested_url: String, scope: Option<TaskScope>, reservation: Option<u64>, link: Option<LinkCreation>, space: String, foreground: bool, native_start_page: bool, history: HistoryCreation }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(LifeHandler::new(self.requested_url.clone(), self.scope.clone(), self.reservation, self.link.clone(), self.space.clone(), self.foreground, self.native_start_page, self.history)) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(TitleHandler::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(PageLoadHandler::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(NavigationHandler::new()) }
        fn focus_handler(&self) -> Option<FocusHandler> { Some(TabFocusHandler::new()) }
        fn find_handler(&self) -> Option<FindHandler> { Some(PageFindHandler::new()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(FileDownloadHandler::new()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(FilePermissionHandler::new()) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(PageContextMenuHandler::new()) }
    }
}
wrap_context_menu_handler! {
    struct PageContextMenuHandler;
    impl ContextMenuHandler {
        fn on_before_context_menu(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>,
            params:Option<&mut ContextMenuParams>, model:Option<&mut MenuModel>) {
            if let Some(browser) = browser {
                let tab = browser.identifier();
                if capture_context_menu(tab, ||context_menu_frame_id(frame.as_deref()), context_menu_current_frame) {
                    add_context_link(tab, ||context_link_url(params.as_deref()), context_menu_current_frame,
                        || model.is_some_and(|model| model.insert_item_at(0, CONTEXT_OPEN_LINK, Some(&CefString::from("링크 주소를 새 탭에서 열기"))) != 0));
                }
            }
        }
        fn on_context_menu_command(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>,
            _params:Option<&mut ContextMenuParams>, command_id:i32, _event_flags:EventFlags) -> i32 {
            let Some(browser) = browser else { return 1; };
            let tab = browser.identifier();
            if command_id == CONTEXT_OPEN_LINK {
                let result = execute_context_link(tab, ||context_menu_frame_id(frame.as_deref()), context_menu_current_frame,
                    |url, link, reservation| {
                        let mut task = OpenLinkTask::new(url, link, reservation);
                        post_task(ThreadId::UI, Some(&mut task)) != 0
                    });
                #[cfg(not(test))]
                match result {
                    ContextLinkResult::Rejected => crate::control_panel::context_menu_rejected(tab),
                    ContextLinkResult::Failed => crate::control_panel::context_link_failed(tab),
                    ContextLinkResult::Queued => {},
                }
                #[cfg(test)]
                let _ = result;
                return 1;
            }
            let handled = execute_context_menu(tab, ||context_menu_frame_id(frame.as_deref()), context_menu_current_frame);
            #[cfg(not(test))]
            if handled != 0 { crate::control_panel::context_menu_rejected(tab); }
            handled
        }
        fn on_context_menu_dismissed(&self, browser:Option<&mut Browser>, _frame:Option<&mut Frame>) {
            if let Some(browser) = browser { dismiss_context_menu(browser.identifier()); }
        }
    }
}
wrap_life_span_handler! {
    struct LifeHandler { requested_url: String, scope: Option<TaskScope>, reservation: Option<u64>, link: Option<LinkCreation>, space: String, foreground: bool, native_start_page: bool, history: HistoryCreation }
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
                LinkCreation { opener, owner:link_owner(&state.tasks,opener), foreground, expected_space:None }
            }));
            let space = opener.and_then(|opener|STATE.with(|state|state.borrow().spaces.tab_space(opener).map(str::to_owned))).unwrap_or_else(||USER_SPACE.into());
            window_info.hidden = i32::from(!foreground || STATE.with(|state| {
                let state = state.borrow(); state.spaces.overview() || state.spaces.active_space() != space
            }));
            *client = Some(BrowserClient::new(target_url.map(ToString::to_string).unwrap_or_default(),scope,reservation,link,space,foreground,false,HistoryCreation::new(false)));
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
            if let Some(client) = client { *client = Some(BrowserClient::new(String::new(),None,None,None,USER_SPACE.into(),false,false,HistoryCreation::new(false))); }
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
                    state.tabs.insert(browser.identifier(),Tab {browser:browser.clone(),title:String::new(),rejected_creation:!attached,native_start_page:self.native_start_page,audio_muted:None,context_menu:None,context_menu_generation:0,_devtools:None,
                        navigation:Navigation { requested_url:self.requested_url.clone(), history: HistoryObservation {
                            attempt: Some(self.history.generation.filter(|_| self.space == USER_SPACE && self.scope.is_none())), staged: self.history.staged, ..Default::default()
                        }, ..Default::default() } });
                    attached
                });
                if !accepted {
                        // Creation may finish after a handoff or finish. This is
                        // a newly created task window, never an adopted user tab.
                        if let Some(host) = browser.host() { host.close_browser(0); }
                        if let Some(link) = &self.link { context_link_creation_failed(link); }
                        return;
                }
                if let Some(host) = browser.host() {
                    let parent = crate::control_panel::create_parent().expect("The browser shell exists");
                    if let Err(error) = unsafe { crate::control_panel::attach(browser.identifier(), parent, host.window_handle().cast(), self.foreground) } {
                        eprintln!("Cannot attach browser tab: {error}");
                        STATE.with(|state| { if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) { tab.rejected_creation = true; } });
                        host.close_browser(1);
                        if let Some(link) = &self.link { context_link_creation_failed(link); }
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
                state.pending.retain(|_,pending| {if pending.tab==id {
                    if pending.mutation_dispatched() {
                        pending.reply.error_details("TAB_CLOSED","The tab closed during the request. Inspect before retrying.",pending.outcome_details());
                    } else {
                        pending.reply.error("TAB_CLOSED","The tab closed during the request.");
                    }
                    false
                }else{true}});
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
        fn on_set_focus(&self, browser: Option<&mut Browser>, source: FocusSource) -> i32 {
            // Reading or loading a hidden tab must not steal native keyboard focus.
            let tab = browser.map(|browser| browser.identifier());
            let deny = crate::control_panel::search_is_open() || crate::control_panel::page_find_is_editing() || tab.is_some_and(crate::control_panel::start_page_is_visible) || tab.is_some_and(|tab| STATE.with(|state| {
                let state = state.borrow();
                state.spaces.overview() || state.spaces.active_tab() != Some(tab)
            }));
            crate::control_panel::trace_focus("cef-set", tab, Some(source.get_raw()), Some(deny));
            i32::from(deny)
        }
        fn on_got_focus(&self, browser: Option<&mut Browser>) {
            crate::control_panel::trace_focus("cef-got", browser.map(|browser| browser.identifier()), None, None);
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
                    let url = url.map(ToString::to_string).unwrap_or_default();
                    tab.observe_start_page_url(&url);
                    let nav = &mut tab.navigation;
                    if !nav.stop_pending { nav.stopped = false; }
                    nav.epoch += 1;
                    nav.history.address_changed(&url);
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
            }); queue_history_observation(browser.identifier()); notify_panel(); }
        }
        fn on_title_change(&self,browser:Option<&mut Browser>,title:Option<&CefString>) {
            if let Some(browser)=browser {STATE.with(|state| {if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                tab.title=title.map(|title|title.to_string().chars().take(512).collect()).unwrap_or_default();
                if !tab.navigation.provisional && !tab.navigation.intent_pending {
                    tab.navigation.history.title_changed(&tab.title);
                }
            }});queue_history_observation(browser.identifier());notify_panel();}
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
                    .map(|_| LinkCreation { opener, owner:link_owner(&state.tasks, opener), foreground, expected_space:None })
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
        fn on_before_browse(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,request:Option<&mut cef::Request>,user_gesture:i32,is_redirect:i32) -> i32 {
            // A subframe can replace its document without changing the main
            // navigation epoch. Cancel the whole tab's menu conservatively.
            if let Some(browser) = browser.as_ref() { dismiss_context_menu(browser.identifier()); }
            if frame.is_some_and(|frame| frame.is_main()!=0) {
                if let Some(browser) = browser {
                    invalidate_page_find(browser.identifier());
                    let (url, transition) = request.map(|request| (CefString::from(&request.url()).to_string(), request.transition_type().get_raw())).unwrap_or_default();
                    let generation = crate::history_runtime::generation();
                    STATE.with(|state| {
                        let mut state = state.borrow_mut();
                        let generation = generation.filter(|_| is_unowned_user_tab(&state, browser.identifier()));
                        if let Some(tab) = state.tabs.get_mut(&browser.identifier()) {
                            tab.observe_start_page_url(&url);
                            let nav = &mut tab.navigation;
                            nav.trace_load_event("browse-begin", browser.identifier(), None, &url, Some((user_gesture != 0, is_redirect != 0, transition)));
                            nav.begin_browse(&url, user_gesture != 0, is_redirect != 0, transition);
                            nav.history.begin(generation, false);
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
                    tab.navigation.history.attempt = None;
                }
            });queue_history_observation(browser.identifier());notify_panel();}
        }
        fn on_load_start(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,_transition_type:TransitionType) {
            // Restored documents may commit without OnBeforeBrowse.
            if let Some(browser) = browser.as_ref() { dismiss_context_menu(browser.identifier()); }
            let Some(frame) = frame.filter(|frame| frame.is_main()!=0) else { return; };
            let url = CefString::from(&frame.url()).to_string();
            let generation = crate::history_runtime::generation();
            if let Some(browser) = browser { crate::download_permission_runtime::cancel_tab(browser.identifier()); invalidate_page_find(browser.identifier()); STATE.with(|state| {
                let mut state = state.borrow_mut();
                let generation = generation.filter(|_| is_unowned_user_tab(&state, browser.identifier()));
                if let Some(tab) = state.tabs.get_mut(&browser.identifier()) {
                    tab.observe_start_page_url(&url);
                    // A restored document may commit without OnBeforeBrowse.
                    tab.navigation.trace_load_event("load-start", browser.identifier(), None, &url, None);
                    tab.navigation.commit(&url);
                    tab.navigation.history.commit(tab.navigation.document_epoch, &url, generation);
                }
            }); notify_panel(); }
        }
        fn on_load_end(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,http_status_code:i32) {
            if frame.is_none_or(|frame| frame.is_main()==0) { return; }
            if let Some(browser) = browser { STATE.with(|state| {
                if let Some(tab) = state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    if tab.navigation.committed && !tab.navigation.provisional {
                        tab.navigation.main_load_complete = true;
                        // Includes restored documents that have no new-document callback.
                        tab.navigation.document_available = true;
                        if let Some(document) = &mut tab.navigation.history.document {
                            document.successful_http = (200..=599).contains(&http_status_code) && tab.navigation.error.is_none();
                        }
                    }
                }
            }); queue_history_observation(browser.identifier()); notify_panel(); }
        }
        fn on_load_error(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,error_code:Errorcode,error_text:Option<&CefString>,failed_url:Option<&CefString>) {
            if frame.is_none_or(|frame| frame.is_main()==0) { return; }
            if let Some(browser)=browser {STATE.with(|state| {
                if let Some(tab)=state.borrow_mut().tabs.get_mut(&browser.identifier()) {
                    let failed_url = failed_url.map(ToString::to_string).unwrap_or_default();
                    let error_text = error_text.map(ToString::to_string).unwrap_or_default();
                    tab.navigation.trace_load_event("load-error", browser.identifier(), Some(error_code.get_raw()), &failed_url, None);
                    tab.navigation.load_failed(error_code.get_raw(), &error_text, &failed_url);
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

    fn native_blank_tab() -> Tab {
        // Inert refcounted CEF handles keep these state guards independent of
        // Chromium and AppKit. Successful navigation is a native smoke check.
        let raw = cef::rc::RcImpl::new(unsafe { std::mem::zeroed::<sys::_cef_browser_t>() }, ());
        Tab {
            browser: cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_browser_t>()),
            title: String::new(),
            rejected_creation: false,
            native_start_page: true,
            audio_muted: None,
            context_menu: None,
            context_menu_generation: 0,
            _devtools: None,
            navigation: Navigation {
                epoch: 4,
                requested_url: "about:blank".into(),
                committed: true,
                document_available: true,
                main_load_complete: true,
                ..Default::default()
            },
        }
    }

    fn context_menu_fixture(owned: bool) -> Option<TaskScope> {
        clear();
        STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            for id in [7, 8] {
                let mut tab = native_blank_tab();
                tab.native_start_page = false;
                tab.navigation.requested_url = "https://example.test/menu".into();
                tab.navigation.document_epoch = 2;
                state.tabs.insert(id, tab);
                state.spaces.insert(id, USER_SPACE, id == 7);
            }
            if !owned {
                return None;
            }
            let task = state.tasks.create("menu owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.adopt(7, &scope.task);
            state.spaces.show(&scope.task);
            Some(scope)
        })
    }

    fn menu_frame_id() -> Option<String> {
        Some("frame-1".into())
    }

    #[test]
    fn context_menu_user_and_paused_commands_run_once_without_handoff() {
        for owned in [false, true] {
            let paused = context_menu_fixture(owned).map(|scope| {
                handoff_native(scope.clone()).unwrap();
                TaskScope {
                    revision: scope.revision + 1,
                    ..scope
                }
            });
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                0
            );
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                1
            );
            if let Some(scope) = paused {
                STATE.with(|slot| {
                    let task = slot.borrow().tasks.info(&scope.task).unwrap();
                    assert_eq!(
                        (task.status, task.revision),
                        (Status::Paused, scope.revision)
                    );
                });
            }
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            dismiss_context_menu(7);
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                1
            );
        }
        clear();
    }

    #[test]
    fn context_menu_active_owner_handoffs_only_itself_before_default_command() {
        let scope = context_menu_fixture(true).unwrap();
        let other = STATE.with(|slot| slot.borrow_mut().tasks.create("other menu owner").unwrap());
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        let mut native_checks = 0;
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| {
                native_checks += 1;
                menu_frame_id()
            }),
            0
        );
        assert_eq!(native_checks, 2);
        STATE.with(|slot| {
            let state = slot.borrow();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Paused, scope.revision + 1)
            );
            let other_now = state.tasks.info(&other.id).unwrap();
            assert_eq!(
                (other_now.status, other_now.revision),
                (Status::Active, other.revision)
            );
            assert_eq!(state.tabs[&7].navigation.epoch, 5);
            assert_eq!(state.tabs[&7].navigation.document_epoch, 2);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert_eq!(state.spaces.active_space(), scope.task);
        });
        clear();
    }

    #[test]
    fn context_menu_resume_or_owner_change_rejects_without_stopping_new_owner() {
        let scope = context_menu_fixture(true).unwrap();
        handoff_native(scope.clone()).unwrap();
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        let (reply, result) = test_reply(1);
        handle(
            &Command::TaskResume {
                task: scope.task.clone(),
                revision: scope.revision + 1,
            },
            reply,
        );
        assert!(result.try_recv().unwrap().ok);
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
            1
        );
        STATE.with(|slot| {
            let task = slot.borrow().tasks.info(&scope.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Active, scope.revision + 2)
            );
        });

        context_menu_fixture(false);
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        let new_owner = STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            let task = state.tasks.create("adopt while menu open").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            scope
        });
        // Even before the presentation moves, authorization changed.
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
            1
        );
        STATE.with(|slot| {
            let task = slot.borrow().tasks.info(&new_owner.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Active, new_owner.revision)
            );
        });
        clear();
    }

    #[test]
    fn context_menu_rejects_stale_navigation_selection_space_and_closed_tabs() {
        let changes: [fn(&mut State); 8] = [
            |state| state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
            |state| state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
            |state| {
                state.spaces.select_tab(8);
            },
            |state| {
                let task = state.tasks.create("other visible space").unwrap();
                let tasks = state.tasks.list();
                state.spaces.update_tasks(&tasks);
                state.spaces.show(&task.id);
            },
            |state| state.spaces.set_overview(true),
            |state| {
                state.tabs.remove(&7);
            },
            |state| state.tabs.get_mut(&7).unwrap().rejected_creation = true,
            |state| state.tabs.get_mut(&7).unwrap().navigation.renderer_exit = Some(json!({})),
        ];
        for change in changes {
            context_menu_fixture(false);
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            STATE.with(|slot| change(&mut slot.borrow_mut()));
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                1
            );
        }
        context_menu_fixture(false);
        capture_context_menu(7, menu_frame_id, |_| None);
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
            1
        );
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        assert_eq!(execute_context_menu(7, menu_frame_id, |_| None), 1);
        clear();
    }

    #[test]
    fn context_menu_native_reentry_preserves_newer_menu_and_rechecks_document() {
        context_menu_fixture(false);
        capture_context_menu(7, menu_frame_id, |_| {
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            menu_frame_id()
        });
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| {
                capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
                menu_frame_id()
            }),
            1
        );
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
            0
        );
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| {
                capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
                dismiss_context_menu(7);
                menu_frame_id()
            }),
            1
        );

        let scope = context_menu_fixture(true).unwrap();
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        let mut checks = 0;
        assert_eq!(
            execute_context_menu(7, menu_frame_id, |_| {
                checks += 1;
                if checks == 2 {
                    STATE.with(|slot| {
                        slot.borrow_mut()
                            .tabs
                            .get_mut(&7)
                            .unwrap()
                            .navigation
                            .document_epoch += 1
                    });
                }
                menu_frame_id()
            }),
            1
        );
        STATE.with(|slot| {
            let task = slot.borrow().tasks.info(&scope.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Paused, scope.revision + 1)
            );
        });
        clear();
    }

    #[test]
    fn context_menu_rejects_missing_or_changed_callback_and_focused_frames() {
        for invalid in [None, Some(String::new()), Some("frame-2".into())] {
            context_menu_fixture(false);
            capture_context_menu(7, || invalid.clone(), |_| menu_frame_id());
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                1
            );

            let scope = context_menu_fixture(true).unwrap();
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            assert_eq!(
                execute_context_menu(7, || invalid.clone(), |_| menu_frame_id()),
                1
            );
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| invalid.clone()),
                1
            );
            STATE.with(|slot| {
                let task = slot.borrow().tasks.info(&scope.task).unwrap();
                assert_eq!(
                    (task.status, task.revision),
                    (Status::Active, scope.revision)
                );
            });
        }
        clear();
    }

    #[test]
    fn context_menu_rechecks_frame_after_handoff_and_callback_getter_reentry() {
        for changed in [None, Some("frame-2".into())] {
            let scope = context_menu_fixture(true).unwrap();
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            let mut checks = 0;
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| {
                    checks += 1;
                    if checks == 1 {
                        menu_frame_id()
                    } else {
                        changed.clone()
                    }
                }),
                1
            );
            STATE.with(|slot| {
                let task = slot.borrow().tasks.info(&scope.task).unwrap();
                assert_eq!(
                    (task.status, task.revision),
                    (Status::Paused, scope.revision + 1)
                );
            });
        }
        context_menu_fixture(false);
        capture_context_menu(
            7,
            || {
                capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
                menu_frame_id()
            },
            |_| panic!("stale outer capture must not query native UI"),
        );
        assert_eq!(
            execute_context_menu(
                7,
                || {
                    dismiss_context_menu(7);
                    menu_frame_id()
                },
                |_| panic!("dismissed command must not query native UI")
            ),
            1
        );
        clear();
    }

    #[test]
    fn subframe_browse_and_commit_invalidate_menu_without_main_epoch_change() {
        // Invoke the actual callbacks with inert CEF handles. No native string
        // conversion or Chromium/AppKit initialization is involved.
        unsafe extern "C" fn identifier(_: *mut sys::_cef_browser_t) -> i32 {
            7
        }
        let mut raw_browser = unsafe { std::mem::zeroed::<sys::_cef_browser_t>() };
        raw_browser.get_identifier = Some(identifier);
        let raw_browser = cef::rc::RcImpl::new(raw_browser, ());
        let mut browser =
            cef::rc::ConvertReturnValue::wrap_result(raw_browser.cast::<sys::_cef_browser_t>());
        let raw_frame =
            cef::rc::RcImpl::new(unsafe { std::mem::zeroed::<sys::_cef_frame_t>() }, ());
        let mut subframe =
            cef::rc::ConvertReturnValue::wrap_result(raw_frame.cast::<sys::_cef_frame_t>());
        for commit in [false, true] {
            context_menu_fixture(false);
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            if commit {
                PageLoadHandler::new().on_load_start(
                    Some(&mut browser),
                    Some(&mut subframe),
                    TransitionType::default(),
                );
            } else {
                assert_eq!(
                    NavigationHandler::new().on_before_browse(
                        Some(&mut browser),
                        Some(&mut subframe),
                        None,
                        0,
                        0
                    ),
                    0
                );
            }
            STATE.with(|slot| {
                let state = slot.borrow();
                assert_eq!(state.tabs[&7].navigation.epoch, 4);
                assert_eq!(state.tabs[&7].navigation.document_epoch, 2);
            });
            assert_eq!(
                execute_context_menu(7, menu_frame_id, |_| menu_frame_id()),
                1
            );
        }
        clear();
    }

    fn prepare_context_link(url: &str) {
        assert!(capture_context_menu(7, menu_frame_id, |_| menu_frame_id()));
        add_context_link(7, || Some(url.into()), |_| menu_frame_id(), || true);
    }

    #[test]
    fn context_link_item_requires_a_bounded_normalized_http_url() {
        let rejected = [
            String::new(),
            "javascript:alert(1)".into(),
            "about:blank".into(),
            "https://user:password@example.test/".into(),
            "https://example.test/\nnext".into(),
            format!("https://example.test/{}", "x".repeat(8192)),
            // Under the input byte cap, over the normalized output byte cap.
            format!("https://example.test/{}", "가".repeat(1000)),
        ];
        for url in rejected {
            context_menu_fixture(false);
            capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
            add_context_link(
                7,
                || Some(url),
                |_| menu_frame_id(),
                || panic!("invalid URL must not add a menu item"),
            );
            STATE.with(|slot| {
                assert!(
                    slot.borrow().tabs[&7]
                        .context_menu
                        .as_ref()
                        .unwrap()
                        .link_url
                        .is_none()
                )
            });
        }
        context_menu_fixture(false);
        prepare_context_link("HTTPS://example.test/a b?q=한글");
        let mut queued = None;
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |url, link, ticket| {
                    queued = Some((url, link, ticket));
                    true
                }
            ),
            ContextLinkResult::Queued
        );
        let (url, link, ticket) = queued.unwrap();
        assert_eq!(url, "https://example.test/a%20b?q=%ED%95%9C%EA%B8%80");
        assert_eq!(link.expected_space.as_deref(), Some(USER_SPACE));
        assert!(!link.foreground);
        assert!(link.owner.is_none());
        assert!(ticket.is_none());
        clear();
    }

    #[test]
    fn context_link_preparation_rechecks_getters_and_model_calls() {
        context_menu_fixture(false);
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        add_context_link(
            7,
            || {
                capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
                Some("https://example.test/old-menu".into())
            },
            |_| menu_frame_id(),
            || panic!("stale URL getter must not mutate a new menu"),
        );
        STATE.with(|slot| {
            assert!(
                slot.borrow().tabs[&7]
                    .context_menu
                    .as_ref()
                    .unwrap()
                    .link_url
                    .is_none()
            )
        });

        add_context_link(
            7,
            || Some("https://example.test/old-menu".into()),
            |_| menu_frame_id(),
            || {
                prepare_context_link("https://example.test/new-menu");
                true
            },
        );
        STATE.with(|slot| {
            assert_eq!(
                slot.borrow().tabs[&7]
                    .context_menu
                    .as_ref()
                    .unwrap()
                    .link_url
                    .as_deref(),
                Some("https://example.test/new-menu")
            )
        });

        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        add_context_link(
            7,
            || Some("https://example.test/failed-item".into()),
            |_| menu_frame_id(),
            || false,
        );
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |_, _, _| panic!("failed item cannot queue")
            ),
            ContextLinkResult::Rejected
        );

        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        let mut checks = 0;
        add_context_link(
            7,
            || Some("https://example.test/changed-frame".into()),
            |_| {
                checks += 1;
                if checks == 3 {
                    Some("other-frame".into())
                } else {
                    menu_frame_id()
                }
            },
            || true,
        );
        assert_eq!(checks, 3);
        STATE.with(|slot| {
            assert!(
                slot.borrow().tabs[&7]
                    .context_menu
                    .as_ref()
                    .unwrap()
                    .link_url
                    .is_none()
            )
        });
        clear();
    }

    #[test]
    fn context_link_captures_post_handoff_owner_and_survives_normal_dismissal() {
        let scope = context_menu_fixture(true).unwrap();
        prepare_context_link("https://example.test/chosen");
        let mut queued = None;
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |url, link, ticket| {
                    queued = Some((url, link, ticket));
                    true
                }
            ),
            ContextLinkResult::Queued
        );
        let (url, link, ticket) = queued.unwrap();
        assert_eq!(url, "https://example.test/chosen");
        assert_eq!(
            link.owner,
            Some((
                TaskScope {
                    task: scope.task.clone(),
                    revision: scope.revision + 1
                },
                Status::Paused
            ))
        );
        assert_eq!(link.expected_space.as_deref(), Some(scope.task.as_str()));
        assert!(!link.foreground);
        assert!(link.scope().is_none());
        assert!(ticket.is_none());
        STATE.with(|slot| {
            let state = slot.borrow();
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(link_is_current(&state, &link));
        });
        dismiss_context_menu(7);
        STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            assert!(state.spaces.show(USER_SPACE));
            assert!(state.spaces.select_tab(8));
            let navigation = &mut state.tabs.get_mut(&7).unwrap().navigation;
            navigation.epoch += 1;
            navigation.document_epoch += 1;
            navigation.requested_url = "https://example.test/later-document".into();
            assert!(link_is_current(&state, &link));
            assert_eq!(state.spaces.active_tab(), Some(8));
            state
                .tasks
                .resume(&TaskScope {
                    task: scope.task,
                    revision: scope.revision + 1,
                })
                .unwrap();
            assert!(!link_is_current(&state, &link));
        });
        clear();
    }

    #[test]
    fn context_link_queue_rejects_moved_closed_or_newly_owned_openers() {
        for change in 0..3 {
            context_menu_fixture(false);
            prepare_context_link("https://example.test/chosen");
            let mut queued = None;
            assert_eq!(
                execute_context_link(
                    7,
                    menu_frame_id,
                    |_| menu_frame_id(),
                    |_, link, _| {
                        queued = Some(link);
                        true
                    }
                ),
                ContextLinkResult::Queued
            );
            let link = queued.unwrap();
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                match change {
                    0 => {
                        let task = state.tasks.create("different space").unwrap();
                        let tasks = state.tasks.list();
                        state.spaces.update_tasks(&tasks);
                        state.spaces.insert(7, &task.id, false);
                        // Old renderer links keep their existing space policy.
                        let legacy = LinkCreation {
                            expected_space: None,
                            ..link.clone()
                        };
                        assert!(link_is_current(&state, &legacy));
                    }
                    1 => {
                        state.tabs.remove(&7);
                    }
                    _ => {
                        let task = state.tasks.create("new owner").unwrap();
                        state
                            .tasks
                            .attach(
                                &TaskScope {
                                    task: task.id,
                                    revision: task.revision,
                                },
                                7,
                                Origin::Borrowed,
                            )
                            .unwrap();
                    }
                }
                assert!(!link_is_current(&state, &link));
            });
        }
        clear();
    }

    #[test]
    fn context_link_queue_failure_consumes_intent_once_and_missing_link_does_not_handoff() {
        let scope = context_menu_fixture(true).unwrap();
        capture_context_menu(7, menu_frame_id, |_| menu_frame_id());
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |_, _, _| panic!("no menu link")
            ),
            ContextLinkResult::Rejected
        );
        STATE.with(|slot| {
            assert_eq!(
                slot.borrow().tasks.info(&scope.task).unwrap().revision,
                scope.revision
            )
        });

        prepare_context_link("https://example.test/chosen");
        let mut queued = 0;
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |_, _, _| {
                    queued += 1;
                    false
                }
            ),
            ContextLinkResult::Failed
        );
        assert_eq!(
            execute_context_link(
                7,
                menu_frame_id,
                |_| menu_frame_id(),
                |_, _, _| {
                    queued += 1;
                    true
                }
            ),
            ContextLinkResult::Rejected
        );
        assert_eq!(queued, 1);
        STATE.with(|slot| {
            let state = slot.borrow();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Paused, scope.revision + 1)
            );
            assert_eq!(state.creations.len(), 0);
        });
        clear();
    }

    #[derive(Default)]
    struct AudioFixture {
        muted: std::cell::Cell<bool>,
        writes: RefCell<Vec<bool>>,
        reads: std::cell::Cell<usize>,
        ignore_write: std::cell::Cell<bool>,
        on_read: std::cell::Cell<Option<fn()>>,
        on_write: std::cell::Cell<Option<fn()>>,
    }

    fn audio_tab(fixture: &std::rc::Rc<AudioFixture>) -> Tab {
        type Shared = std::rc::Rc<AudioFixture>;
        unsafe extern "C" fn read(raw: *mut sys::_cef_browser_host_t) -> i32 {
            let fixture = cef::rc::RcImpl::<sys::_cef_browser_host_t, Shared>::get(raw)
                .interface
                .clone();
            fixture.reads.set(fixture.reads.get() + 1);
            if let Some(callback) = fixture.on_read.take() {
                callback();
            }
            i32::from(fixture.muted.get())
        }
        unsafe extern "C" fn write(raw: *mut sys::_cef_browser_host_t, muted: i32) {
            let fixture = cef::rc::RcImpl::<sys::_cef_browser_host_t, Shared>::get(raw)
                .interface
                .clone();
            fixture.writes.borrow_mut().push(muted != 0);
            if !fixture.ignore_write.get() {
                fixture.muted.set(muted != 0);
            }
            if let Some(callback) = fixture.on_write.take() {
                callback();
            }
        }
        unsafe extern "C" fn host(raw: *mut sys::_cef_browser_t) -> *mut sys::_cef_browser_host_t {
            let host = cef::rc::RcImpl::<sys::_cef_browser_t, BrowserHost>::get(raw)
                .interface
                .clone();
            host.into()
        }
        let mut raw_host = unsafe { std::mem::zeroed::<sys::_cef_browser_host_t>() };
        raw_host.is_audio_muted = Some(read);
        raw_host.set_audio_muted = Some(write);
        let raw_host = cef::rc::RcImpl::new(raw_host, fixture.clone());
        let host_value: BrowserHost =
            cef::rc::ConvertReturnValue::wrap_result(raw_host.cast::<sys::_cef_browser_host_t>());
        let mut raw_browser = unsafe { std::mem::zeroed::<sys::_cef_browser_t>() };
        raw_browser.get_host = Some(host);
        let raw_browser = cef::rc::RcImpl::new(raw_browser, host_value);
        let mut tab = native_blank_tab();
        tab.browser =
            cef::rc::ConvertReturnValue::wrap_result(raw_browser.cast::<sys::_cef_browser_t>());
        tab
    }

    fn setup_audio() -> (std::rc::Rc<AudioFixture>, TabAudioTarget) {
        clear();
        let fixture = std::rc::Rc::new(AudioFixture::default());
        STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            state.tabs.insert(7, audio_tab(&fixture));
            state.spaces.insert(7, USER_SPACE, true);
        });
        let target = capture_tab_audio_target(7, USER_SPACE, 4).unwrap();
        (fixture, target)
    }

    #[test]
    fn tab_audio_changes_only_selected_webcontents_without_ai_handoff() {
        let (fixture, _) = setup_audio();
        let other = std::rc::Rc::new(AudioFixture::default());
        let space = STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            let task = state.tasks.create("audio").unwrap();
            let scope = TaskScope {
                task: task.id.clone(),
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.adopt(7, &scope.task);
            state.spaces.show(&scope.task);
            state.tabs.insert(8, audio_tab(&other));
            state.spaces.insert(8, USER_SPACE, false);
            task.id
        });
        let snapshot = || {
            STATE.with(|slot| {
                let state = slot.borrow();
                json!({"tasks": state.tasks.list(), "spaces": state.spaces.views(),
                "active": state.spaces.active_space(), "selected": state.spaces.active_tab(),
                "epoch": state.tabs[&7].navigation.epoch,
                "url": state.tabs[&7].navigation.requested_url})
            })
        };
        let before = snapshot();
        let target = capture_tab_audio_target(7, &space, 4).unwrap();
        assert!(!read_tab_audio(&target).unwrap());
        assert!(set_tab_audio_muted(&target, false, true, || true).unwrap());
        assert!(!set_tab_audio_muted(&target, true, false, || true).unwrap());
        assert_eq!(*fixture.writes.borrow(), [true, false]);
        assert!(other.writes.borrow().is_empty());
        assert_eq!(other.reads.get(), 0);
        assert_eq!(snapshot(), before);
        STATE.with(|slot| assert_eq!(slot.borrow().tabs[&7].audio_muted, Some(false)));
        clear();
    }

    #[test]
    fn tab_audio_rejects_stale_selection_document_owner_and_failed_tabs() {
        let changes: [fn(&mut State); 7] = [
            |state| {
                state.spaces.set_overview(true);
            },
            |state| {
                state.spaces.insert(8, USER_SPACE, true);
            },
            |state| {
                state.tabs.get_mut(&7).unwrap().navigation.epoch += 1;
            },
            |state| {
                state.tabs.get_mut(&7).unwrap().rejected_creation = true;
            },
            |state| {
                state.tabs.get_mut(&7).unwrap().navigation.renderer_exit =
                    Some(json!({"status":"crashed"}));
            },
            |state| {
                state.tabs.remove(&7);
            },
            |state| {
                let task = state.tasks.create("new owner").unwrap();
                state
                    .tasks
                    .attach(
                        &TaskScope {
                            task: task.id,
                            revision: task.revision,
                        },
                        7,
                        Origin::Borrowed,
                    )
                    .unwrap();
            },
        ];
        for change in changes {
            let (fixture, target) = setup_audio();
            STATE.with(|slot| change(&mut slot.borrow_mut()));
            assert!(read_tab_audio(&target).is_err());
            assert!(set_tab_audio_muted(&target, false, true, || true).is_err());
            assert_eq!(fixture.reads.get(), 0);
            assert!(fixture.writes.borrow().is_empty());
        }
        let (_, target) = setup_audio();
        STATE.with(|slot| {
            slot.borrow_mut().tabs.insert(7, native_blank_tab());
        });
        assert!(
            read_tab_audio(&target)
                .unwrap_err()
                .contains("읽을 수 없습니다")
        );
        clear();
    }

    #[test]
    fn tab_audio_revalidates_after_native_getter_and_ui_changes() {
        let (fixture, target) = setup_audio();
        fixture.on_read.set(Some(|| {
            STATE.with(|slot| {
                slot.borrow_mut().tabs.get_mut(&7).unwrap().navigation.epoch += 1;
            })
        }));
        assert!(set_tab_audio_muted(&target, false, true, || true).is_err());
        assert!(fixture.writes.borrow().is_empty());
        let (fixture, target) = setup_audio();
        let checks = std::cell::Cell::new(0);
        assert!(
            set_tab_audio_muted(&target, false, true, || {
                checks.set(checks.get() + 1);
                checks.get() == 1
            })
            .is_err()
        );
        assert_eq!(checks.get(), 2);
        assert!(fixture.writes.borrow().is_empty());
        fixture.muted.set(true);
        assert!(set_tab_audio_muted(&target, false, false, || true).is_err());
        assert!(fixture.writes.borrow().is_empty());
        STATE.with(|slot| assert_eq!(slot.borrow().tabs[&7].audio_muted, Some(true)));
        clear();
    }

    #[test]
    fn tab_audio_reports_unconfirmed_write_and_never_reads_closed_host() {
        let (fixture, target) = setup_audio();
        fixture.ignore_write.set(true);
        let error = set_tab_audio_muted(&target, false, true, || true).unwrap_err();
        assert!(error.contains("요청했지만 설정에 반영되지"));
        assert_eq!(*fixture.writes.borrow(), [true]);
        STATE.with(|slot| assert_eq!(slot.borrow().tabs[&7].audio_muted, Some(false)));
        let (fixture, target) = setup_audio();
        fixture.on_write.set(Some(|| {
            STATE.with(|slot| {
                slot.borrow_mut().tabs.remove(&7);
            })
        }));
        let error = set_tab_audio_muted(&target, false, true, || true).unwrap_err();
        assert!(error.contains("요청했지만 탭이 바뀌어"));
        assert_eq!(*fixture.writes.borrow(), [true]);
        assert_eq!(fixture.reads.get(), 1);
        clear();
    }

    #[test]
    fn start_page_requires_native_provenance_and_a_settled_blank_document() {
        assert!(native_blank_tab().start_page_ready(false));
        assert!(!native_blank_tab().start_page_ready(true));
        let changes: [fn(&mut Tab); 14] = [
            |tab| tab.native_start_page = false,
            |tab| tab.rejected_creation = true,
            |tab| tab.navigation.requested_url = "about:blank#content".into(),
            |tab| tab.navigation.committed = false,
            |tab| tab.navigation.document_available = false,
            |tab| tab.navigation.main_load_complete = false,
            |tab| tab.navigation.provisional = true,
            |tab| tab.navigation.intent_pending = true,
            |tab| tab.navigation.stopped = true,
            |tab| tab.navigation.stop_pending = true,
            |tab| tab.navigation.incomplete = true,
            |tab| tab.navigation.error = Some(json!({"code": -3})),
            |tab| tab.navigation.renderer_exit = Some(json!({"status": "crashed"})),
            |tab| tab.navigation.recovering = true,
        ];
        for change in changes {
            let mut tab = native_blank_tab();
            change(&mut tab);
            assert!(!tab.start_page_ready(false));
        }
        let mut tab = native_blank_tab();
        tab.observe_start_page_url("https://example.test/");
        tab.observe_start_page_url("about:blank");
        assert!(!tab.start_page_ready(false));
    }

    #[test]
    fn start_page_revalidates_selection_document_and_creation_before_handoff() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("start page owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(7, &scope.task, false);
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.show(&scope.task);
            state.tabs.insert(7, native_blank_tab());
            scope
        });
        let target = StartPageTarget {
            tab: 7,
            space: scope.task.clone(),
            navigation_epoch: 4,
        };
        let validate = |target: &StartPageTarget, loading, blank| {
            STATE.with(|state| validate_start_page_state(&state.borrow(), target, loading, blank))
        };
        assert!(validate(&target, false, true).is_ok());
        assert!(validate(&target, true, true).is_err());
        assert!(validate(&target, false, false).is_err());
        for stale in [
            StartPageTarget {
                tab: 8,
                ..target.clone()
            },
            StartPageTarget {
                space: USER_SPACE.into(),
                ..target.clone()
            },
            StartPageTarget {
                navigation_epoch: 3,
                ..target.clone()
            },
        ] {
            assert!(validate(&stale, false, true).is_err());
        }
        STATE.with(|state| state.borrow_mut().spaces.set_overview(true));
        assert!(validate(&target, false, true).is_err());
        STATE.with(|state| state.borrow_mut().spaces.show(USER_SPACE));
        assert!(validate(&target, false, true).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.show(&scope.task);
            state.tabs.get_mut(&7).unwrap().rejected_creation = true;
        });
        assert!(validate(&target, false, true).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let tab = state.tabs.get_mut(&7).unwrap();
            tab.rejected_creation = false;
            // A native/CLI blank reload preserves provenance but invalidates
            // the old form, including after the same blank document settles.
            tab.navigation.begin(None);
            assert!(tab.native_start_page);
            tab.navigation.commit("about:blank");
            tab.navigation.document_available = true;
            tab.navigation.main_load_complete = true;
        });
        assert!(validate(&target, false, true).is_err());
        let current = STATE.with(|state| StartPageTarget {
            navigation_epoch: state.borrow().tabs[&7].navigation.epoch,
            ..target.clone()
        });
        assert!(validate(&current, false, true).is_ok());
        for input in [
            "javascript:alert(1)",
            "https://user:secret@example.test/",
            "http://",
        ] {
            assert!(navigate_from_start_page(&current, input).is_err());
        }
        assert!(navigate_from_start_page(&current, "  ").is_ok());
        // The inert browser has no actual main frame; no owner may be handed
        // off merely because the cached/tracked document still looks blank.
        assert!(navigate_from_start_page(&current, "example.test").is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            state.spaces.release(7);
        });
        assert!(validate(&current, false, true).is_err());
        clear();
        assert!(validate_start_page_target(&current).is_err());
    }

    #[test]
    fn page_failure_only_projects_settled_concrete_load_errors() {
        let failed = || Navigation {
            error: Some(
                json!({"code": -105, "text": "raw detail", "failedUrl": "https://example.test/"}),
            ),
            ..Default::default()
        };
        assert_eq!(
            failed().page_failure(false),
            Some(PageFailure::Load { code: -105 })
        );
        assert_eq!(failed().page_failure(true), None);
        for change in [
            |nav: &mut Navigation| nav.provisional = true,
            |nav: &mut Navigation| nav.recovering = true,
            |nav: &mut Navigation| nav.intent_pending = true,
            |nav: &mut Navigation| nav.stop_pending = true,
            |nav: &mut Navigation| nav.stopped = true,
        ] {
            let mut nav = failed();
            change(&mut nav);
            assert_eq!(nav.page_failure(false), None);
        }
        for code in [
            json!(Errorcode::ABORTED.get_raw()),
            json!("-105"),
            json!(-105.5),
            json!(i64::MAX),
            Json::Null,
        ] {
            let mut nav = failed();
            nav.error = Some(json!({"code": code}));
            assert_eq!(nav.page_failure(false), None);
        }
        let mut nav = Navigation::default();
        assert_eq!(nav.page_failure(false), None);
        nav.begin(None);
        nav.finish_loading();
        assert!(nav.incomplete);
        assert_eq!(nav.page_failure(false), None);
    }

    #[test]
    fn renderer_failure_remains_recoverable_after_an_uncommitted_stopped_retry() {
        let mut nav = Navigation::default();
        nav.renderer_failed(json!({"status": "crashed"}));
        assert_eq!(nav.page_failure(false), Some(PageFailure::Renderer));
        assert_eq!(nav.page_failure(true), Some(PageFailure::Renderer));
        nav.begin(None);
        assert_eq!(nav.page_failure(false), None);
        nav.stopped = true;
        nav.finish_loading();
        assert_eq!(nav.page_failure(false), Some(PageFailure::Renderer));
        nav.begin(None);
        nav.commit("https://example.test/recovered");
        nav.finish_loading();
        assert_eq!(nav.page_failure(false), None);
    }

    #[test]
    fn trailing_abort_preserves_only_the_same_urls_concrete_failure() {
        let url = "https://example.test/failed";
        let other = "https://example.test/other";
        let mut nav = Navigation::default();
        nav.begin(Some(url));
        nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
        nav.finish_loading();
        assert_eq!(
            nav.page_failure(false),
            Some(PageFailure::Load { code: -324 })
        );
        assert_eq!(nav.error.as_ref().unwrap()["text"], "ERR_EMPTY_RESPONSE");
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", other);
        assert_eq!(nav.page_failure(false), None);
        nav.begin(Some(url));
        nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
        nav.begin(Some(url));
        assert!(nav.error.is_none());
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
        nav.finish_loading();
        assert_eq!(nav.page_failure(false), None);
        nav.begin(Some(url));
        nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
        nav.commit(other);
        assert!(nav.error.is_none());
        nav.stopped = true;
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", other);
        assert!(nav.error.is_none());
    }

    #[test]
    fn automatic_same_url_browse_hides_candidate_until_a_matching_abort() {
        let url = "https://example.test/failed";
        let mut nav = Navigation::default();
        nav.begin(Some(url));
        nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
        let failed_epoch = nav.epoch;
        for _ in 0..2 {
            nav.begin_browse(url, false, false, TransitionType::RELOAD.get_raw());
            // poll_wait only examines public error, so an ongoing retry does
            // not immediately report the previous NAVIGATION_FAILED result.
            assert!(nav.error.is_none());
            assert!(nav.automatic_retry_error.is_some());
            assert!(nav.provisional && nav.intent_pending);
            assert_eq!(nav.page_failure(false), None);
        }
        assert!(nav.epoch > failed_epoch);
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
        nav.finish_loading();
        assert!(nav.automatic_retry_error.is_none());
        assert_eq!(
            nav.page_failure(false),
            Some(PageFailure::Load { code: -324 })
        );
        assert_eq!(nav.error.as_ref().unwrap()["text"], "ERR_EMPTY_RESPONSE");
    }

    #[test]
    fn new_navigation_boundaries_discard_an_automatic_retry_candidate() {
        let url = "https://example.test/failed";
        let other = "https://example.test/other";
        for (next_url, gesture, redirect, transition) in [
            (url, true, false, 0),
            (url, false, true, 0),
            (other, false, false, 0),
            (
                url,
                false,
                false,
                TransitionType::CLIENT_REDIRECT_FLAG.get_raw(),
            ),
            (
                url,
                false,
                false,
                TransitionType::SERVER_REDIRECT_FLAG.get_raw(),
            ),
            (
                url,
                false,
                false,
                TransitionType::FORWARD_BACK_FLAG.get_raw(),
            ),
        ] {
            let mut nav = Navigation::default();
            nav.begin(Some(url));
            nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
            nav.begin_browse(url, false, false, 0);
            nav.begin_browse(next_url, gesture, redirect, transition);
            assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
            nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", next_url);
            nav.finish_loading();
            assert_eq!(nav.page_failure(false), None);
        }
        for next in [None, Some(url), Some(other)] {
            let mut nav = Navigation::default();
            nav.begin(Some(url));
            nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
            nav.begin_browse(url, false, false, 0);
            nav.begin(next);
            assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
        }
    }

    #[test]
    fn successful_commit_clears_both_visible_and_pending_load_failures() {
        let url = "https://example.test/failed";
        for automatic in [false, true] {
            let mut nav = Navigation::default();
            nav.begin(Some(url));
            nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
            if automatic {
                nav.begin_browse(url, false, false, 0);
            }
            nav.commit(url);
            nav.finish_loading();
            assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
            assert_eq!(nav.page_failure(false), None);
            nav.begin_browse(url, false, false, 0);
            nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
            nav.finish_loading();
            assert_eq!(nav.page_failure(false), None);
        }
    }

    #[test]
    fn automatic_retry_candidate_does_not_survive_other_terminal_events() {
        let url = "https://example.test/failed";
        let pending = || {
            let mut nav = Navigation::default();
            nav.begin(Some(url));
            nav.load_failed(-324, "ERR_EMPTY_RESPONSE", url);
            nav.begin_browse(url, false, false, 0);
            nav
        };
        let mut nav = pending();
        nav.load_failed(-105, "ERR_NAME_NOT_RESOLVED", url);
        nav.finish_loading();
        assert!(nav.automatic_retry_error.is_none());
        assert_eq!(
            nav.page_failure(false),
            Some(PageFailure::Load { code: -105 })
        );
        let mut nav = pending();
        nav.load_failed(
            Errorcode::ABORTED.get_raw(),
            "ERR_ABORTED",
            "https://example.test/other",
        );
        nav.finish_loading();
        assert!(nav.automatic_retry_error.is_none());
        assert_eq!(nav.page_failure(false), None);
        let mut nav = pending();
        nav.finish_loading();
        assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
        assert!(nav.incomplete);
        let mut nav = pending();
        nav.stopped = true;
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
        nav.finish_loading();
        assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
        let mut nav = pending();
        nav.renderer_failed(json!({"status":"crashed"}));
        assert!(nav.error.is_none() && nav.automatic_retry_error.is_none());
        nav.begin(None);
        nav.load_failed(Errorcode::ABORTED.get_raw(), "ERR_ABORTED", url);
        nav.finish_loading();
        assert_eq!(nav.page_failure(false), Some(PageFailure::Renderer));
    }

    #[test]
    fn stale_recovery_targets_and_loading_renderer_retry_do_not_handoff() {
        clear();
        let browser = |loading| {
            unsafe extern "C" fn still_loading(_: *mut sys::_cef_browser_t) -> i32 {
                1
            }
            let mut raw = unsafe { std::mem::zeroed::<sys::_cef_browser_t>() };
            if loading {
                raw.is_loading = Some(still_loading);
            }
            let raw = cef::rc::RcImpl::new(raw, ());
            cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_browser_t>())
        };
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("recovery owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(7, &scope.task, false);
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.show(&scope.task);
            state.tabs.insert(
                7,
                Tab {
                    browser: browser(false),
                    title: String::new(),
                    rejected_creation: false,
                    native_start_page: false,
                    audio_muted: None,
                    context_menu: None,
                    context_menu_generation: 0,
                    _devtools: None,
                    navigation: Navigation {
                        epoch: 4,
                        error: Some(json!({"code": -105})),
                        ..Default::default()
                    },
                },
            );
            scope
        });
        let target = PageRecoveryTarget {
            tab: 7,
            navigation_epoch: 4,
            space: scope.task.clone(),
            failure: PageFailure::Load { code: -105 },
        };
        assert!(validate_failed_page_target(&target).is_ok());
        for stale in [
            PageRecoveryTarget {
                tab: 8,
                ..target.clone()
            },
            PageRecoveryTarget {
                navigation_epoch: 3,
                ..target.clone()
            },
            PageRecoveryTarget {
                space: USER_SPACE.into(),
                ..target.clone()
            },
            PageRecoveryTarget {
                failure: PageFailure::Renderer,
                ..target.clone()
            },
        ] {
            assert!(retry_failed_page(&stale).is_err());
        }
        STATE.with(|state| state.borrow_mut().spaces.set_overview(true));
        assert!(retry_failed_page(&target).is_err());
        STATE.with(|state| state.borrow_mut().spaces.show(USER_SPACE));
        assert!(retry_failed_page(&target).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.show(&scope.task);
            state.tabs.get_mut(&7).unwrap().rejected_creation = true;
        });
        assert!(retry_failed_page(&target).is_err());
        let renderer = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let tab = state.tabs.get_mut(&7).unwrap();
            tab.rejected_creation = false;
            tab.browser = browser(true);
            tab.navigation.renderer_failed(json!({"status": "crashed"}));
            PageRecoveryTarget {
                navigation_epoch: tab.navigation.epoch,
                failure: PageFailure::Renderer,
                ..target.clone()
            }
        });
        assert!(validate_failed_page_target(&renderer).is_ok());
        assert!(retry_failed_page(&renderer).is_err());
        STATE.with(|state| {
            let state = state.borrow();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
        });
        clear();
        assert!(validate_failed_page_target(&target).is_err());
    }

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
                    native_start_page: false,
                    audio_muted: None,
                    context_menu: None,
                    context_menu_generation: 0,
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
    fn download_source_reveal_preserves_control_and_survives_attachment_intents() {
        clear();
        let page = || {
            let mut tab = native_blank_tab();
            tab.navigation.document_epoch = 7;
            tab
        };
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create(&"긴".repeat(64)).unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.tabs.insert(7, page());
            state.tabs.insert(8, page());
            state.spaces.insert(7, &scope.task, false);
            state.spaces.insert(8, USER_SPACE, true);
            state.tasks.attach(&scope, 7, Origin::Created).unwrap();
            scope
        });
        let snapshot = || {
            STATE.with(|state| {
                let state = state.borrow();
                json!({"spaces":state.spaces.views(), "active":state.spaces.active_space(),
                    "overview":state.spaces.overview(), "tasks":state.tasks.list()})
            })
        };
        let context = download_context(7).unwrap();
        assert_eq!(context.scope, Some(scope.clone()));
        assert!(context.source.chars().count() <= 80);
        assert!(context.source.ends_with("탭 7"));
        let target = context.source_target;
        // Source viewing does not require a CanDownload permission attempt.
        assert!(download_permission_context(7).is_err());
        assert!(validate_download_source(&target).is_ok());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
            nav.begin_browse("https://example.test/file-one", false, false, 0);
            nav.begin_browse("https://example.test/file-two", false, false, 0);
            assert!(nav.provisional && nav.intent_pending);
            assert!(nav.epoch > 4);
            state.spaces.set_overview(true);
        });
        assert!(validate_download_source(&target).is_ok());
        let tasks = STATE.with(|state| state.borrow().tasks.list());
        assert_eq!(reveal_download_source(&target), Ok(7));
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(!state.spaces.overview());
            assert_eq!(state.tasks.list(), tasks);
        });
        STATE.with(|state| state.borrow_mut().spaces.show(USER_SPACE));
        let reject_unchanged = |target: &crate::downloads::SourceTarget| {
            let before = snapshot();
            assert!(validate_download_source(target).is_err());
            assert!(reveal_download_source(target).is_err());
            assert_eq!(snapshot(), before);
        };
        for change in [
            (|tab: &mut Tab| tab.navigation.document_epoch += 1) as fn(&mut Tab),
            |tab| tab.navigation.renderer_exit = Some(json!({"status":"crashed"})),
            |tab| tab.rejected_creation = true,
        ] {
            STATE.with(|state| change(state.borrow_mut().tabs.get_mut(&7).unwrap()));
            reject_unchanged(&target);
            STATE.with(|state| state.borrow_mut().tabs.insert(7, page()));
        }
        STATE.with(|state| state.borrow_mut().spaces.remove(7));
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.insert(7, &scope.task, false);
            state.tabs.remove(&7);
        });
        reject_unchanged(&target);
        STATE.with(|state| state.borrow_mut().tabs.insert(7, page()));
        // This shared boundary is called by explicit native/CLI navigation,
        // stop, and main-frame address changes, unlike attachment BeforeBrowse.
        revoke_download_permission_provenance(7);
        reject_unchanged(&target);
        let current = download_context(7).unwrap().source_target;
        assert!(validate_download_source(&current).is_ok());
        let paused = STATE.with(|state| state.borrow_mut().tasks.handoff(&scope).unwrap());
        reject_unchanged(&current);
        expire_tab_refs([7]);
        let paused_context = download_context(7).unwrap();
        assert_eq!(paused_context.scope, None);
        assert!(paused_context.source.contains("사용자 제어"));
        assert!(paused_context.source.ends_with("탭 7"));
        assert!(paused_context.source.chars().count() <= 80);
        let paused_target = paused_context.source_target;
        assert_eq!(paused_target.owner.as_ref().unwrap().1, Status::Paused);
        assert!(validate_download_source(&paused_target).is_ok());
        let resumed = STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .resume(&TaskScope {
                    task: paused.id,
                    revision: paused.revision,
                })
                .unwrap()
        });
        reject_unchanged(&paused_target);
        // File Save As scope deliberately remains independent of this link.
        assert!(validate_download(7, paused_context.scope.as_ref()).is_ok());
        let resumed_scope = TaskScope {
            task: resumed.id,
            revision: resumed.revision,
        };
        let user_target = download_context(8).unwrap().source_target;
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .attach(&resumed_scope, 8, Origin::Borrowed)
                .unwrap();
        });
        reject_unchanged(&user_target);
        expire_tab_refs([8]);
        STATE.with(|state| {
            state.borrow_mut().tasks.release(&resumed_scope, 8).unwrap();
        });
        expire_tab_refs([8]);
        reject_unchanged(&user_target);
        let fresh_user = download_context(8).unwrap().source_target;
        assert!(validate_download_source(&fresh_user).is_ok());
        clear();
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
            expected_space: None,
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
            expected_space: None,
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

    fn admitted_test_command(
        outstanding: &Arc<AtomicUsize>,
        token: u64,
    ) -> (Task, mpsc::Receiver<Response>, Arc<AtomicBool>) {
        let (reply, receiver) = test_reply(token);
        let active = reply.active.clone();
        let permit = CommandPermit::acquire(outstanding, &reply.id).ok().unwrap();
        let task = CommandTask::new(Arc::new(Mutex::new(Some(QueuedCommand {
            request: Request {
                id: reply.id.clone(),
                command: Command::TaskInfo {
                    task: "missing-task".into(),
                },
            },
            reply,
            _permit: permit,
        }))));
        (task, receiver, active)
    }

    #[test]
    fn stalled_command_queue_stays_bounded_after_timeouts_and_reconnects() {
        let outstanding = Arc::new(AtomicUsize::new(0));
        let mut queued = Vec::new();
        for token in 1..=MAX_CONNECTIONS {
            let (worker, receiver, active) = admitted_test_command(&outstanding, token as u64);
            // Model CEF retaining the accepted post after its worker times out.
            queued.push(worker.clone());
            active.store(false, Ordering::Release);
            drop(worker);
            drop(receiver);
        }
        for _ in 0..3 * MAX_CONNECTIONS {
            let response = match CommandPermit::acquire(&outstanding, "reconnect") {
                Ok(_) => panic!("A timed-out worker must not free its queued command slot"),
                Err(response) => response,
            };
            assert_eq!(response.id, "reconnect");
            let error = response.error.unwrap();
            assert_eq!(error.code, "APP_BUSY");
            assert_eq!(error.details.unwrap()["outcomeUnknown"], false);
        }
        assert_eq!(outstanding.load(Ordering::Acquire), MAX_CONNECTIONS);
        // Draining one expired command does no work but frees exactly one slot.
        queued[0].execute();
        assert_eq!(outstanding.load(Ordering::Acquire), MAX_CONNECTIONS - 1);
        let replacement = CommandPermit::acquire(&outstanding, "replacement")
            .ok()
            .unwrap();
        assert_eq!(outstanding.load(Ordering::Acquire), MAX_CONNECTIONS);
        queued[0].execute();
        assert_eq!(outstanding.load(Ordering::Acquire), MAX_CONNECTIONS);
        drop(replacement);
        drop(queued);
        assert_eq!(outstanding.load(Ordering::Acquire), 0);
    }

    #[test]
    fn command_execution_releases_payload_even_while_native_handles_remain() {
        clear();
        let outstanding = Arc::new(AtomicUsize::new(0));
        let (task, receiver, _) = admitted_test_command(&outstanding, 1);
        let native_handle = task.clone();
        task.execute();
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "TASK_NOT_FOUND"
        );
        assert_eq!(outstanding.load(Ordering::Acquire), 0);
        native_handle.execute();
        assert!(receiver.try_recv().is_err());
        drop(task);
        drop(native_handle);
        assert_eq!(outstanding.load(Ordering::Acquire), 0);
        clear();
    }

    #[test]
    fn rejected_posts_and_discarded_native_tasks_return_only_their_server_permit() {
        let first_server = Arc::new(AtomicUsize::new(0));
        let second_server = Arc::new(AtomicUsize::new(0));
        let (rejected, rejected_result, _) = admitted_test_command(&first_server, 1);
        let (queued, queued_result, _) = admitted_test_command(&first_server, 2);
        let (other, _, _) = admitted_test_command(&second_server, 3);
        // A failed native post releases its parameter reference before returning.
        let rejected_parameter = rejected.clone();
        drop(rejected_parameter);
        drop(rejected);
        assert_eq!(first_server.load(Ordering::Acquire), 1);
        assert_eq!(second_server.load(Ordering::Acquire), 1);
        assert!(rejected_result.try_recv().is_err());
        // An accepted task discarded without execute releases on its last handle.
        let native_queue = queued.clone();
        drop(queued);
        assert_eq!(first_server.load(Ordering::Acquire), 1);
        drop(native_queue);
        assert_eq!(first_server.load(Ordering::Acquire), 0);
        assert!(queued_result.try_recv().is_err());
        drop(other);
        assert_eq!(second_server.load(Ordering::Acquire), 0);
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
                        evaluation: Evaluation::Page {
                            mutation,
                            evidence: None,
                        },
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
        assert!(select_user_tab(7).is_err());
        select_user_tab(8).unwrap();
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
        assert!(select_user_tab(8).is_err());
        assert!(select_user_tab(99).is_err());
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
    fn history_observer_keeps_navigation_generation_and_fresh_titles() {
        let first = "https://example.test/first";
        let next = "https://example.test/next#section";
        let mut history = HistoryObservation::default();
        history.begin(Some(4), true);
        history.begin(Some(5), false); // redirect after a clear cannot upgrade it
        history.commit(2, first, Some(5));
        assert!(history.action(first.into()).is_none()); // no successful HTTP end
        history.document.as_mut().unwrap().successful_http = true;
        let initial = history.action(first.into()).unwrap();
        assert_eq!(initial.generation, 4);
        assert_eq!(initial.title, first);
        assert!(!initial.update_title);
        history.accepted(&initial);
        assert!(history.action(first.into()).is_none());
        history.title_changed(" Fresh\n title ");
        let title = history.action(first.into()).unwrap();
        assert_eq!(title.title, "Fresh title");
        assert!(title.update_title);
        history.accepted(&title);
        history.address_changed(next);
        let spa = history.action(next.into()).unwrap();
        assert_eq!(spa.generation, 4); // old document cannot revive a cleared generation
        assert_eq!(spa.document_epoch, 2);
        assert_eq!(spa.title, next);
        assert!(!spa.update_title);
        history.accepted(&spa);
        history.begin(Some(5), true);
        history.commit(3, next, Some(5));
        history.document.as_mut().unwrap().successful_http = true;
        assert_eq!(history.action(next.into()).unwrap().generation, 5);
        assert_eq!(history.action(next.into()).unwrap().title, next);
        history.staged = true;
        assert!(history.action(next.into()).is_none());
        history.staged = false;
        assert!(history.action(next.into()).is_some());
        history.revoke();
        history.commit(4, next, Some(9));
        assert!(history.document.is_none()); // release/late commit cannot promote revoked provenance
        history.attempt = None;
        history.revoke();
        history.commit(5, next, Some(9));
        assert!(history.document.is_none());
        history.begin(Some(9), false);
        history.commit(6, next, Some(9));
        assert!(history.document.is_some());
        history.begin(Some(9), true);
        history.commit(7, "about:blank", Some(9));
        assert!(history.document.is_none());
    }

    #[test]
    fn history_collection_requires_live_user_document_and_revokes_transfer() {
        clear();
        let url = "https://example.test/history";
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("history excluded AI").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            let mut tab = native_blank_tab();
            tab.navigation.requested_url = url.into();
            tab.navigation.history.begin(Some(8), true);
            tab.navigation.history.commit(0, url, Some(8));
            tab.navigation
                .history
                .document
                .as_mut()
                .unwrap()
                .successful_http = true;
            state.tabs.insert(7, tab);
            state.spaces.insert(7, USER_SPACE, true);
            // Collection does not depend on which space the user is viewing.
            state.spaces.show(&scope.task);
            assert!(history_page_ready(&state, 7, 4, 0));
            assert!(!history_page_ready(&state, 7, 5, 0));
            assert!(!history_page_ready(&state, 7, 4, 1));
            state.tabs.get_mut(&7).unwrap().navigation.history.staged = true;
            assert!(!history_page_ready(&state, 7, 4, 0));
            state.tabs.get_mut(&7).unwrap().navigation.history.staged = false;
            state.tabs.get_mut(&7).unwrap().rejected_creation = true;
            assert!(!history_page_ready(&state, 7, 4, 0));
            state.tabs.get_mut(&7).unwrap().rejected_creation = false;
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            assert!(!history_page_ready(&state, 7, 4, 0));
            state.spaces.adopt(7, &scope.task);
            scope
        });
        expire_tab_refs([7]);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tasks.release(&scope, 7).unwrap();
            state.spaces.release(7);
            let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
            assert!(nav.history.document.is_none());
            nav.history.commit(1, url, Some(9));
            assert!(nav.history.document.is_none());
            nav.history.begin(Some(9), true);
            nav.history.commit(1, url, Some(9));
            nav.history.document.as_mut().unwrap().successful_http = true;
            nav.load_failed(-324, "network failure", url);
            assert!(nav.history.document.is_none());
            nav.history.begin(Some(9), true);
            nav.history.commit(1, url, Some(9));
            nav.renderer_failed(json!({"status":"crashed"}));
            assert!(nav.history.document.is_none());
            state.tabs.remove(&7);
            assert!(!history_page_ready(&state, 7, 5, 0));
        });
        clear();
    }

    #[test]
    fn saved_history_open_rechecks_clear_and_preserves_ai_control() {
        clear();
        let url = "https://example.test/saved";
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("saved page source").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.tabs.insert(7, native_blank_tab());
            state.spaces.insert(7, &scope.task, false);
            state.tasks.attach(&scope, 7, Origin::Created).unwrap();
            let mut target = native_blank_tab();
            target.navigation.requested_url = url.into();
            state.tabs.insert(8, target);
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.show(&scope.task);
            scope
        });
        let before = STATE.with(|state| user_open_selection(&state.borrow()));
        assert!(open_history_url(url, || false).is_err());
        let calls = std::cell::Cell::new(0);
        assert!(
            open_history_url(url, || {
                calls.set(calls.get() + 1);
                calls.get() == 1
            })
            .is_err()
        );
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(user_open_selection(&state), before);
            assert!(validate_saved_user_destination(&state, 8, &before, true).is_ok());
            assert!(validate_saved_user_destination(&state, 8, &before, false).is_err());
        });
        open_history_url(url, || true).unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(8));
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
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
        clear();
    }

    #[test]
    fn current_page_address_requires_the_same_explicit_web_address() {
        let target = CurrentPageTarget {
            tab: 7,
            space: USER_SPACE.into(),
            navigation_epoch: 4,
            url: "HTTPS://example.test/current".into(),
        };
        assert_eq!(
            current_page_address(&target, "https://example.test/current").unwrap(),
            "https://example.test/current"
        );
        for url in [
            "https://example.test/other",
            "about:blank",
            "data:text/html,content",
            "https://user:password@example.test/current",
        ] {
            assert!(current_page_address(&target, url).is_err());
        }
        let mut unsupported = target;
        unsupported.url = "about:blank".into();
        assert!(current_page_address(&unsupported, "about:blank").is_err());
    }

    #[test]
    fn download_site_target_rechecks_document_and_owner_without_changing_selection() {
        clear();
        let page = || {
            let mut tab = native_blank_tab();
            tab.native_start_page = false;
            tab.navigation.requested_url = "https://example.test/current".into();
            tab.navigation.document_epoch = 11;
            tab
        };
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("download setting source").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.tabs.insert(7, page());
            state.spaces.insert(7, &scope.task, false);
            state.tasks.attach(&scope, 7, Origin::Created).unwrap();
            state.tabs.insert(8, page());
            state.spaces.insert(8, USER_SPACE, false);
            state.spaces.show(&scope.task);
            scope
        });
        let source = CurrentPageTarget {
            tab: 7,
            space: scope.task.clone(),
            navigation_epoch: 4,
            url: "https://example.test/current".into(),
        };
        let snapshot = || {
            STATE.with(|state| {
                let state = state.borrow();
                json!({"spaces":state.spaces.views(), "active":state.spaces.active_space(),
                    "overview":state.spaces.overview(), "tasks":state.tasks.list()})
            })
        };
        let before = snapshot();
        let target = capture_download_site_target(&source).unwrap();
        assert_eq!(target.document_epoch, 11);
        assert_eq!(target.owner, Some((scope.clone(), Status::Active)));
        assert!(validate_download_site_target(&target).is_ok());
        // The inert handle has no main frame. A read failure must not hand off
        // the task, navigate, or select another tab to obtain setting context.
        assert!(read_download_site_page(&target).is_err());
        assert_eq!(snapshot(), before);
        let reject_unchanged = |target: &DownloadSiteTarget| {
            let before = snapshot();
            assert!(validate_download_site_target(target).is_err());
            assert!(read_download_site_page(target).is_err());
            assert_eq!(snapshot(), before);
        };
        for change in [
            (|tab: &mut Tab| tab.navigation.document_epoch += 1) as fn(&mut Tab),
            |tab| tab.navigation.epoch += 1,
            |tab| tab.navigation.requested_url = "https://example.test/other".into(),
            |tab| tab.navigation.provisional = true,
            |tab| tab.rejected_creation = true,
        ] {
            STATE.with(|state| change(state.borrow_mut().tabs.get_mut(&7).unwrap()));
            reject_unchanged(&target);
            STATE.with(|state| state.borrow_mut().tabs.insert(7, page()));
        }
        STATE.with(|state| state.borrow_mut().spaces.set_overview(true));
        assert!(capture_download_site_target(&source).is_err());
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.set_overview(false);
            state.spaces.show(USER_SPACE);
        });
        reject_unchanged(&target);
        STATE.with(|state| state.borrow_mut().spaces.show(&scope.task));
        let paused = STATE.with(|state| state.borrow_mut().tasks.handoff(&scope).unwrap());
        reject_unchanged(&target);
        let paused_target = capture_download_site_target(&source).unwrap();
        assert_eq!(paused_target.owner.as_ref().unwrap().1, Status::Paused);
        assert!(validate_download_site_target(&paused_target).is_ok());
        let resumed = STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .resume(&TaskScope {
                    task: paused.id,
                    revision: paused.revision,
                })
                .unwrap()
        });
        reject_unchanged(&paused_target);
        reject_unchanged(&target);
        let resumed_scope = TaskScope {
            task: resumed.id,
            revision: resumed.revision,
        };
        STATE.with(|state| state.borrow_mut().spaces.show(USER_SPACE));
        let user_source = CurrentPageTarget {
            tab: 8,
            space: USER_SPACE.into(),
            ..source
        };
        let user_target = capture_download_site_target(&user_source).unwrap();
        assert_eq!(user_target.owner, None);
        STATE.with(|state| {
            state
                .borrow_mut()
                .tasks
                .attach(&resumed_scope, 8, Origin::Borrowed)
                .unwrap();
        });
        // Ownership changes invalidate even before the space mapping catches up.
        reject_unchanged(&user_target);
        expire_tab_refs([8]);
        STATE.with(|state| {
            state.borrow_mut().tasks.release(&resumed_scope, 8).unwrap();
        });
        expire_tab_refs([8]);
        // The existing transfer boundary also prevents a None -> owned -> None
        // round trip from reviving an old setting action on the same document.
        reject_unchanged(&user_target);
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.active_tab(), Some(8));
            assert_eq!(state.tasks.info(&resumed_scope.task).unwrap().revision, 3);
        });
        clear();
    }

    #[test]
    fn current_page_open_guards_preserve_source_control_and_staged_selection() {
        clear();
        let page = || {
            let mut tab = native_blank_tab();
            tab.native_start_page = false;
            tab.navigation.requested_url = "https://example.test/current".into();
            tab
        };
        let (scope_a, scope_b) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let a = state.tasks.create("current page source").unwrap();
            let b = state.tasks.create("unrelated background task").unwrap();
            let scope_a = TaskScope {
                task: a.id,
                revision: a.revision,
            };
            let scope_b = TaskScope {
                task: b.id,
                revision: b.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            for (id, space) in [
                (7, scope_a.task.as_str()),
                (8, USER_SPACE),
                (9, scope_b.task.as_str()),
            ] {
                state.tabs.insert(id, page());
                state.spaces.insert(id, space, false);
            }
            state.tasks.attach(&scope_a, 7, Origin::Created).unwrap();
            state.tasks.attach(&scope_b, 9, Origin::Created).unwrap();
            state.spaces.show(&scope_a.task);
            (scope_a, scope_b)
        });
        let target = CurrentPageTarget {
            tab: 7,
            space: scope_a.task.clone(),
            navigation_epoch: 4,
            url: "https://example.test/current".into(),
        };
        let snapshot = || {
            STATE.with(|state| {
                let state = state.borrow();
                json!({"spaces":state.spaces.views(), "active":state.spaces.active_space(),
                "overview":state.spaces.overview(), "tasks":state.tasks.list()})
            })
        };
        let reject_unchanged = |target: &CurrentPageTarget| {
            let before = snapshot();
            // Invalid state must fail before touching the inert CEF handle.
            assert!(open_current_page_in_user_tab(target).is_err());
            assert_eq!(snapshot(), before);
        };
        assert!(validate_current_page_target(&target).is_ok());
        for change in [
            (|target: &mut CurrentPageTarget| target.tab = 99) as fn(&mut CurrentPageTarget),
            |target| target.space = USER_SPACE.into(),
            |target| target.navigation_epoch += 1,
            |target| target.url = "https://example.test/other".into(),
            |target| target.url = "about:blank".into(),
        ] {
            let mut stale = target.clone();
            change(&mut stale);
            reject_unchanged(&stale);
        }
        let changes: [fn(&mut Tab); 8] = [
            |tab| tab.rejected_creation = true,
            |tab| tab.navigation.committed = false,
            |tab| tab.navigation.document_available = false,
            |tab| tab.navigation.provisional = true,
            |tab| tab.navigation.intent_pending = true,
            |tab| tab.navigation.recovering = true,
            |tab| tab.navigation.error = Some(json!({"code": -324})),
            |tab| tab.navigation.renderer_exit = Some(json!({"status": "crashed"})),
        ];
        for change in changes {
            STATE.with(|state| change(state.borrow_mut().tabs.get_mut(&7).unwrap()));
            reject_unchanged(&target);
            STATE.with(|state| {
                state.borrow_mut().tabs.insert(7, page());
            });
        }
        STATE.with(|state| state.borrow_mut().spaces.set_overview(true));
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.set_overview(false);
            state.spaces.show(USER_SPACE);
        });
        reject_unchanged(&target);
        STATE.with(|state| state.borrow_mut().spaces.show(&scope_a.task));
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.insert(11, page());
            state.spaces.insert(11, &scope_a.task, true);
        });
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.remove(&11);
            state.spaces.remove(11);
            state.spaces.remove(7);
        });
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.insert(7, &scope_a.task, true);
            state.tabs.remove(&7);
        });
        reject_unchanged(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.insert(7, page());
            state.spaces.show(USER_SPACE);
            let user_target = CurrentPageTarget {
                tab: 8,
                space: USER_SPACE.into(),
                ..target.clone()
            };
            assert!(validate_current_page_source(&state, &user_target, &None, true).is_ok());
            state.tasks.attach(&scope_b, 8, Origin::Borrowed).unwrap();
            assert!(validate_current_page_source(&state, &user_target, &None, true).is_err());
            state.tasks.release(&scope_b, 8).unwrap();
            state.spaces.show(&scope_a.task);
        });
        let owner = STATE.with(|state| link_owner(&state.borrow().tasks, 7));
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            // Even if selection/epoch stayed unchanged around a CEF call, a
            // revision change must invalidate the captured owner identity.
            let paused = state.tasks.handoff(&scope_a).unwrap();
            assert!(validate_current_page_source(&state, &target, &owner, true).is_err());
            let resumed = state
                .tasks
                .resume(&TaskScope {
                    task: paused.id,
                    revision: paused.revision,
                })
                .unwrap();
            assert!(validate_current_page_source(&state, &target, &owner, true).is_err());
            assert_eq!(resumed.status, Status::Active);
        });
        let current_owner = STATE.with(|state| link_owner(&state.borrow().tasks, 7));
        let before_ui_change = snapshot();
        STATE.with(|state| {
            assert!(
                validate_current_page_source(&state.borrow(), &target, &current_owner, false)
                    .is_err()
            );
        });
        assert_eq!(snapshot(), before_ui_change);
        let before = snapshot();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.insert(10, page());
            state.spaces.insert(10, USER_SPACE, false);
            assert!(validate_current_page_source(&state, &target, &current_owner, true).is_ok());
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(can_discard_user_staging(&state, 10, Some(7)));
            assert!(!can_discard_user_staging(&state, 7, Some(7)));
            assert!(!can_discard_user_staging(&state, 99, Some(7)));
            // Pending cleanup must leave an adopted destination to its owner,
            // even before the space mapping catches up with task ownership.
            state.tasks.attach(&scope_b, 10, Origin::Borrowed).unwrap();
            assert!(!can_discard_user_staging(&state, 10, Some(7)));
            state.tasks.release(&scope_b, 10).unwrap();
            state.spaces.adopt(10, &scope_b.task);
            assert!(!can_discard_user_staging(&state, 10, Some(7)));
            state.spaces.release(10);
            state.tabs.get_mut(&10).unwrap().rejected_creation = true;
            assert!(can_discard_user_staging(&state, 10, Some(7)));
            remember_closed_page(&mut state, 10, &target.url);
            assert!(!state.closed_pages.can_reopen());
            state.tabs.get_mut(&10).unwrap().rejected_creation = false;
        });
        // Successful selection uses the actual user selector, without handoff.
        let tasks_before = STATE.with(|state| state.borrow().tasks.list());
        select_user_tab(10).unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.tasks.list(), tasks_before);
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(10));
            assert_eq!(
                state
                    .spaces
                    .views()
                    .iter()
                    .find(|space| space.id == scope_a.task)
                    .unwrap()
                    .selected_tab,
                Some(7)
            );
            assert_eq!(
                state.tasks.info(&scope_b.task).unwrap().revision,
                scope_b.revision
            );
            assert_eq!(
                before["tasks"][0]["revision"],
                state.tasks.info(&scope_a.task).unwrap().revision
            );
        });
        clear();
    }

    #[test]
    fn sidebar_close_revalidates_moves_and_visibility_before_handoff() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("sidebar close owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            // Inert refcounted CEF values test the real state guard without
            // loading Chromium or reaching a native close operation.
            for id in [7, 8] {
                let raw =
                    cef::rc::RcImpl::new(unsafe { std::mem::zeroed::<sys::_cef_browser_t>() }, ());
                let browser =
                    cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_browser_t>());
                state.tabs.insert(
                    id,
                    Tab {
                        browser,
                        title: String::new(),
                        rejected_creation: false,
                        native_start_page: false,
                        audio_muted: None,
                        context_menu: None,
                        context_menu_generation: 0,
                        _devtools: None,
                        navigation: Navigation::default(),
                    },
                );
                state.spaces.insert(id, USER_SPACE, id == 7);
            }
            // Selected and non-selected rows are both closeable without first
            // selecting the latter. The guard itself must not alter selection.
            assert!(validate_sidebar_close(&state, 7, USER_SPACE).is_ok());
            assert!(validate_sidebar_close(&state, 8, USER_SPACE).is_ok());
            assert_eq!(state.spaces.active_tab(), Some(7));
            state.tasks.attach(&scope, 8, Origin::Borrowed).unwrap();
            state.spaces.adopt(8, &scope.task);
            scope
        });
        // A row still rendered in user space must not hand off the new owner.
        assert!(close_tab_from_sidebar(8, USER_SPACE).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(validate_sidebar_close(&state, 8, &scope.task).is_err());
            state.spaces.show(&scope.task);
            assert!(validate_sidebar_close(&state, 8, &scope.task).is_ok());
            state.spaces.set_overview(true);
        });
        assert!(close_tab_from_sidebar(8, &scope.task).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.set_overview(false);
            state.tasks.release(&scope, 8).unwrap();
            state.spaces.release(8);
        });
        // A stale task-space row must not close the now-released user tab.
        assert!(close_tab_from_sidebar(8, &scope.task).is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(
                state.tasks.info(&scope.task).unwrap().revision,
                scope.revision
            );
            state.spaces.show(USER_SPACE);
            assert!(validate_sidebar_close(&state, 8, USER_SPACE).is_ok());
            state.tabs.get_mut(&8).unwrap().rejected_creation = true;
        });
        assert!(close_tab_from_sidebar(8, USER_SPACE).is_err());
        STATE.with(|state| {
            state.borrow_mut().tabs.remove(&8);
        });
        assert!(close_tab_from_sidebar(8, USER_SPACE).is_err());
        clear();
    }

    #[test]
    fn sidebar_reordering_preserves_selection_navigation_and_ai_ownership() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("ordered task").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            for id in [7, 3, 9, 12, 11] {
                state.tabs.insert(id, native_blank_tab());
                state.spaces.insert(id, USER_SPACE, id == 3);
            }
            for id in [12, 11] {
                state.tasks.attach(&scope, id, Origin::Borrowed).unwrap();
                state.spaces.adopt(id, &scope.task);
            }
            scope
        });
        let before_tasks =
            STATE.with(|state| serde_json::to_value(state.borrow().tasks.list()).unwrap());
        reorder_tab_from_sidebar(&TabOrderTarget {
            tab: 3,
            space: USER_SPACE.into(),
            neighbor: 7,
            forward: false,
        })
        .unwrap();
        let snapshot = shell_snapshot();
        assert_eq!(snapshot.active_tab, Some(3));
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .filter(|tab| tab.space == USER_SPACE)
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            [3, 7, 9]
        );
        assert_eq!(
            snapshot
                .spaces
                .iter()
                .find(|space| space.id == USER_SPACE)
                .unwrap()
                .tabs,
            [3, 7, 9]
        );
        STATE.with(|state| {
            state.borrow_mut().spaces.show(&scope.task);
        });
        reorder_tab_from_sidebar(&TabOrderTarget {
            tab: 12,
            space: scope.task.clone(),
            neighbor: 11,
            forward: true,
        })
        .unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.ordered_tabs(&scope.task), [11, 12]);
            assert_eq!(state.spaces.active_tab(), Some(12));
            assert_eq!(state.spaces.ordered_tabs(USER_SPACE), [3, 7, 9]);
            assert_eq!(
                serde_json::to_value(state.tasks.list()).unwrap(),
                before_tasks
            );
            assert_eq!(state.tasks.owner(12).unwrap().0, scope.task);
            for tab in state.tabs.values() {
                assert_eq!(tab.navigation.epoch, 4);
                assert_eq!(tab.navigation.requested_url, "about:blank");
            }
        });
        clear();
    }

    #[test]
    fn sidebar_reordering_rejects_stale_selection_neighbors_direction_and_lifetime() {
        clear();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            for id in [7, 3, 9] {
                state.tabs.insert(id, native_blank_tab());
                state.spaces.insert(id, USER_SPACE, id == 3);
            }
        });
        let target = TabOrderTarget {
            tab: 3,
            space: USER_SPACE.into(),
            neighbor: 9,
            forward: true,
        };
        let reject = |target: &TabOrderTarget| {
            let before =
                STATE.with(|state| serde_json::to_value(state.borrow().spaces.views()).unwrap());
            assert!(reorder_tab_from_sidebar(target).is_err());
            assert_eq!(
                STATE.with(|state| serde_json::to_value(state.borrow().spaces.views()).unwrap()),
                before
            );
        };
        reject(&TabOrderTarget {
            neighbor: 7,
            ..target.clone()
        });
        reject(&TabOrderTarget {
            space: "other".into(),
            ..target.clone()
        });
        STATE.with(|state| {
            state.borrow_mut().spaces.select_tab(7);
        });
        reject(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.select_tab(3);
            state.spaces.set_overview(true);
        });
        reject(&target);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.set_overview(false);
            state.tabs.get_mut(&9).unwrap().rejected_creation = true;
        });
        reject(&target);
        STATE.with(|state| {
            state
                .borrow_mut()
                .tabs
                .get_mut(&9)
                .unwrap()
                .rejected_creation = false;
        });
        reorder_tab_from_sidebar(&target).unwrap();
        reject(&target); // the same pair now has the opposite direction
        let reverse = TabOrderTarget {
            forward: false,
            ..target
        };
        STATE.with(|state| {
            state.borrow_mut().tabs.remove(&9);
        });
        reject(&reverse); // stale presentation membership is insufficient
        clear();
    }

    fn tab_drag_fixture() -> (TabDragTarget, TaskScope) {
        clear();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("drag owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            for id in [7, 8, 9] {
                let mut tab = native_blank_tab();
                tab.navigation.document_epoch = 2;
                state.tabs.insert(id, tab);
                state.spaces.insert(id, USER_SPACE, id == 7);
                state.tasks.attach(&scope, id, Origin::Borrowed).unwrap();
                state.spaces.adopt(id, &scope.task);
            }
            assert!(state.spaces.show(&scope.task));
            assert!(state.spaces.select_tab(7));
            (
                TabDragTarget {
                    tab: 8,
                    space: scope.task.clone(),
                    selected: Some(7),
                    order: vec![7, 8, 9],
                },
                scope,
            )
        })
    }

    #[test]
    fn sidebar_drag_moves_background_tab_without_selection_or_ai_handoff() {
        let (mut target, scope) = tab_drag_fixture();
        let before_tasks =
            STATE.with(|state| serde_json::to_value(state.borrow().tasks.list()).unwrap());
        assert!(validate_tab_drag_target(&target).is_ok());
        assert_eq!(reorder_tab_by_drag(&target, Some(7)), Ok(true));
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.ordered_tabs(&scope.task), [8, 7, 9]);
            assert_eq!(state.spaces.active_tab(), Some(7));
            target.order = state.spaces.ordered_tabs(&scope.task).to_vec();
        });
        assert_eq!(reorder_tab_by_drag(&target, None), Ok(true));
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.spaces.ordered_tabs(&scope.task), [7, 9, 8]);
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(7));
            assert!(!state.spaces.overview());
            assert_eq!(
                serde_json::to_value(state.tasks.list()).unwrap(),
                before_tasks
            );
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.revision, scope.revision);
            assert_eq!(task.status, Status::Active);
            for id in [7, 8, 9] {
                let owner = state.tasks.owner(id).unwrap();
                assert_eq!(owner.0, scope.task);
                assert_eq!(owner.1, Origin::Borrowed);
                let tab = &state.tabs[&id];
                assert_eq!(tab.navigation.epoch, 4);
                assert_eq!(tab.navigation.document_epoch, 2);
                assert_eq!(tab.navigation.requested_url, "about:blank");
            }
            target.order = state.spaces.ordered_tabs(&scope.task).to_vec();
        });
        assert_eq!(reorder_tab_by_drag(&target, None), Ok(false));
        clear();
    }

    #[test]
    fn sidebar_drag_rejects_stale_order_selection_space_and_live_membership() {
        let cases: &[(&str, fn(&mut State))] = &[
            ("order", |state| {
                let space = state.spaces.active_space().to_owned();
                assert!(state.spaces.reorder_adjacent(7, &space, 8, true));
            }),
            ("selection", |state| {
                assert!(state.spaces.select_tab(8));
            }),
            ("overview", |state| state.spaces.set_overview(true)),
            ("active space", |state| {
                assert!(state.spaces.show(USER_SPACE));
            }),
            ("closed source", |state| {
                state.tabs.remove(&8);
            }),
            ("closed listed tab", |state| {
                state.tabs.remove(&9);
            }),
            ("rejected source", |state| {
                state.tabs.get_mut(&8).unwrap().rejected_creation = true;
            }),
            ("rejected listed tab", |state| {
                state.tabs.get_mut(&9).unwrap().rejected_creation = true;
            }),
            ("moved listed tab", |state| {
                state.spaces.insert(9, USER_SPACE, false);
            }),
        ];
        for (name, change) in cases {
            let (target, _) = tab_drag_fixture();
            let before = STATE.with(|state| {
                let mut state = state.borrow_mut();
                change(&mut state);
                json!({"spaces": state.spaces.views(), "tasks": state.tasks.list()})
            });
            assert!(validate_tab_drag_target(&target).is_err(), "{name}");
            assert!(reorder_tab_by_drag(&target, Some(7)).is_err(), "{name}");
            // Dropping onto the source is a no-op only for a current drag.
            assert!(reorder_tab_by_drag(&target, Some(8)).is_err(), "{name}");
            STATE.with(|state| {
                let state = state.borrow();
                assert_eq!(
                    json!({"spaces": state.spaces.views(), "tasks": state.tasks.list()}),
                    before,
                    "{name}"
                );
            });
        }
        clear();
    }

    #[test]
    fn sidebar_drag_noops_and_invalid_drop_targets_preserve_metadata() {
        let (target, _) = tab_drag_fixture();
        let before = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.insert(11, native_blank_tab());
            state.spaces.insert(11, USER_SPACE, false);
            json!({"spaces": state.spaces.views(), "tasks": state.tasks.list()})
        });
        assert_eq!(reorder_tab_by_drag(&target, Some(8)), Ok(false));
        assert_eq!(reorder_tab_by_drag(&target, Some(9)), Ok(false));
        assert!(reorder_tab_by_drag(&target, Some(11)).is_err());
        assert!(reorder_tab_by_drag(&target, Some(999)).is_err());
        let missing_source = TabDragTarget { tab: 999, ..target };
        assert!(validate_tab_drag_target(&missing_source).is_err());
        assert!(reorder_tab_by_drag(&missing_source, Some(999)).is_err());
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(
                json!({"spaces": state.spaces.views(), "tasks": state.tasks.list()}),
                before
            );
        });
        clear();
    }

    #[test]
    fn sidebar_selection_revalidates_live_membership_without_handoff() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("sidebar selection owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            for id in [7, 8, 9] {
                state.tabs.insert(id, native_blank_tab());
                state.spaces.insert(id, USER_SPACE, id == 7);
            }
            state.tasks.attach(&scope, 9, Origin::Borrowed).unwrap();
            state.spaces.adopt(9, &scope.task);
            scope
        });
        let snapshot = || {
            STATE.with(|state| {
                let state = state.borrow();
                json!({
                    "spaces": state.spaces.views(),
                    "overview": state.spaces.overview(),
                    "tasks": state.tasks.list(),
                })
            })
        };
        let reject_unchanged = |tab, expected_space: &str| {
            let before = snapshot();
            assert!(select_tab_from_sidebar(tab, expected_space).is_err());
            assert_eq!(snapshot(), before);
        };
        reject_unchanged(99, USER_SPACE);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tasks.attach(&scope, 8, Origin::Borrowed).unwrap();
            state.spaces.adopt(8, &scope.task);
        });
        // An adopted user row cannot select its new owner or change AI control.
        reject_unchanged(8, USER_SPACE);
        // Correct membership alone is insufficient when the viewed space changed.
        reject_unchanged(8, &scope.task);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.show(&scope.task);
            state.spaces.set_overview(true);
            assert_eq!(state.spaces.active_tab(), Some(9));
        });
        // Valid selection reuses the real SelectTab path, exits overview, and
        // preserves the active owner even when selecting another AI tab.
        select_tab_from_sidebar(8, &scope.task).unwrap();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(state.spaces.active_space(), scope.task);
            assert_eq!(state.spaces.active_tab(), Some(8));
            assert!(!state.spaces.overview());
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(task.status, Status::Active);
            assert_eq!(task.revision, scope.revision);
            assert_eq!(state.tasks.owner(8).unwrap().0, scope.task);
            state.tasks.release(&scope, 8).unwrap();
            state.spaces.release(8);
        });
        // A released task row must not select the returned user tab.
        reject_unchanged(8, &scope.task);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.show(USER_SPACE);
            state.tabs.get_mut(&8).unwrap().rejected_creation = true;
        });
        reject_unchanged(9, &scope.task);
        reject_unchanged(8, USER_SPACE);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.get_mut(&8).unwrap().rejected_creation = false;
            state.spaces.remove(8);
        });
        // A live CEF handle without current presentation membership is stale.
        reject_unchanged(8, USER_SPACE);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.spaces.insert(8, USER_SPACE, false);
            state.tabs.remove(&8);
        });
        // Conversely, a retained space mapping does not make a closed tab live.
        reject_unchanged(8, USER_SPACE);
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
                        evaluation: Evaluation::Page {
                            mutation: true,
                            evidence: None,
                        },
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
    fn string_actions_use_epoch_guard_and_serialize_exact_values_as_data() {
        let value = "  \"});window.unexpected=true;//\n🧭  ".to_owned();
        for (action, key) in [("fill", "text"), ("select", "value")] {
            let (reply, receiver) = test_reply(1);
            assert!(
                action_expression(
                    action,
                    "8/document:1:1",
                    Some((key, ActionValue::String(&value))),
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
                action,
                "9/document:1:1",
                Some((key, ActionValue::String(&value))),
                9,
                &reply,
            );
            let args = expression
                .strip_prefix("window.__lightBrowserBridge.act(")
                .unwrap()
                .strip_suffix(')')
                .unwrap();
            let mut expected = json!({"action":action,"ref":"document:1:1"});
            expected[key] = json!(value);
            assert_eq!(serde_json::from_str::<Json>(args).unwrap(), expected);
        }
    }

    #[test]
    fn check_serializes_boolean_data_and_rejects_stale_or_invalid_refs() {
        for checked in [false, true] {
            let (reply, _) = test_reply(1);
            let expression = action_expression(
                "check",
                "9/document:1:1",
                Some(("checked", ActionValue::Bool(checked))),
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
                json!({"action":"check","ref":"document:1:1","checked":checked})
            );
            for (reference, code) in [
                ("8/document:1:1", "STALE_REF"),
                ("9/", "STALE_REF"),
                ("", "INVALID_REF"),
                ("9/doc\n:1", "INVALID_REF"),
            ] {
                let (reply, receiver) = test_reply(2);
                assert!(
                    action_expression(
                        "check",
                        reference,
                        Some(("checked", ActionValue::Bool(checked))),
                        9,
                        &reply
                    )
                    .is_empty()
                );
                assert_eq!(receiver.try_recv().unwrap().error.unwrap().code, code);
            }
        }
    }

    #[test]
    fn check_preserves_task_access_and_epoch_guards_before_dispatch() {
        clear();
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("check owner").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Borrowed).unwrap();
            state.tabs.insert(7, native_blank_tab());
            scope
        });
        let command = Command::Check {
            tab: 7,
            reference: "3/document:1:1".into(),
            checked: false,
        };
        let (reply, receiver) = test_reply(1);
        handle(&command, reply);
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "TASK_REQUIRED"
        );
        let scoped = Command::TaskRun {
            task: scope.task.clone(),
            revision: scope.revision,
            command: Box::new(command),
        };
        let (reply, receiver) = test_reply(2);
        handle(&scoped, reply);
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "STALE_REF"
        );
        STATE.with(|state| {
            state.borrow_mut().tasks.handoff(&scope).unwrap();
        });
        let (reply, receiver) = test_reply(3);
        handle(&scoped, reply);
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "TASK_STALE_REVISION"
        );
        STATE.with(|state| {
            let state = state.borrow();
            assert!(state.pending.is_empty());
            assert_eq!(state.next_message, 0);
        });
        clear();
    }

    #[test]
    fn action_failures_forward_only_boolean_details_for_the_requested_action() {
        for (requested_evidence, action, fields, expected) in [
            (
                Some(ActionEvidence::Check),
                "check",
                json!({"attempted":true,"trusted":false,"checked":false,"indeterminate":true,"pageText":"excluded"}),
                Some(
                    json!({"attempted":true,"trusted":false,"checked":false,"indeterminate":true}),
                ),
            ),
            (
                Some(ActionEvidence::Check),
                "check",
                json!({"attempted":false,"trusted":false,"checked":true,"indeterminate":false}),
                Some(
                    json!({"attempted":false,"trusted":false,"checked":true,"indeterminate":false}),
                ),
            ),
            (
                Some(ActionEvidence::Check),
                "check",
                json!({"attempted":true,"trusted":"false","checked":null,"indeterminate":[]} ),
                Some(json!({"attempted":true})),
            ),
            (
                Some(ActionEvidence::Check),
                "check",
                json!({"attempted":1,"trusted":"false","checked":null,"indeterminate":{}}),
                None,
            ),
            (
                Some(ActionEvidence::Check),
                "click",
                json!({"attempted":true}),
                None,
            ),
            (None, "check", json!({"attempted":true}), None),
            (
                Some(ActionEvidence::Fill),
                "fill",
                json!({"attempted":true,"trusted":false,"checked":false,"changed":true,"text":"excluded","unknown":{"body":"excluded"}}),
                Some(json!({"attempted":true,"trusted":false})),
            ),
            (
                Some(ActionEvidence::Fill),
                "fill",
                json!({"attempted":false,"trusted":false}),
                Some(json!({"attempted":false,"trusted":false})),
            ),
            (
                Some(ActionEvidence::Fill),
                "fill",
                json!({"attempted":"true","trusted":1,"text":"excluded"}),
                None,
            ),
            (
                Some(ActionEvidence::Fill),
                "check",
                json!({"attempted":true}),
                None,
            ),
            (
                Some(ActionEvidence::Check),
                "fill",
                json!({"attempted":true}),
                None,
            ),
            (None, "fill", json!({"attempted":true}), None),
        ] {
            clear();
            let tab = native_blank_tab();
            let browser = tab.browser.clone();
            let (reply, receiver) = test_reply(1);
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.pending.insert(
                    42,
                    Pending {
                        tab: 7,
                        epoch: tab.navigation.epoch,
                        evaluation: Evaluation::Page {
                            mutation: true,
                            evidence: requested_evidence,
                        },
                        reply,
                        step: Step::Evaluate,
                    },
                );
                state.tabs.insert(7, tab);
            });
            let mut value = fields;
            value["ok"] = json!(false);
            value["action"] = json!(action);
            let code = if action == "fill" {
                "FILL_STATE_UNCONFIRMED"
            } else {
                "CHECK_STATE_NOT_REACHED"
            };
            value["error"] = json!(code);
            value["detail"] = json!("Read again before retrying.");
            let bytes = serde_json::to_vec(&json!({"result":{"value":value}})).unwrap();
            devtools_result(&browser, 42, true, &bytes);
            let response = receiver.try_recv().unwrap();
            let wire = serde_json::to_value(response).unwrap();
            assert_eq!(wire["error"]["code"], code);
            assert_eq!(wire["error"].get("details"), expected.as_ref());
            // A repeated native callback cannot send a second action result.
            devtools_result(&browser, 42, true, &bytes);
            assert!(receiver.try_recv().is_err());
            clear();
        }
    }

    #[test]
    fn canceled_actions_keep_unknown_outcome_and_ignore_late_boolean_details() {
        for (evidence, action) in [
            (ActionEvidence::Check, "check"),
            (ActionEvidence::Fill, "fill"),
        ] {
            clear();
            let tab = native_blank_tab();
            let browser = tab.browser.clone();
            let (mut reply, receiver) = test_reply(1);
            reply.scope = Some(TaskScope {
                task: "check-task".into(),
                revision: 1,
            });
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.pending.insert(
                    42,
                    Pending {
                        tab: 7,
                        epoch: tab.navigation.epoch,
                        evaluation: Evaluation::Page {
                            mutation: true,
                            evidence: Some(evidence),
                        },
                        reply,
                        step: Step::Evaluate,
                    },
                );
                state.tabs.insert(7, tab);
            });
            revoke_task_requests("check-task", None);
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "TASK_CONTROL_LOST");
            assert_eq!(error.details.unwrap()["outcomeUnknown"], true);
            devtools_result(
            &browser,
            42,
            true,
            &serde_json::to_vec(&json!({"result":{"value":{"ok":false,"action":action,"attempted":true,"trusted":false}}})).unwrap(),
        );
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
            clear();
        }
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

    fn test_press_preparation() -> (Pending, Browser, mpsc::Receiver<Response>) {
        clear();
        let tab = native_blank_tab();
        let browser = tab.browser.clone();
        let (mut reply, receiver) = test_reply(701);
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("Enter test").unwrap();
            let scope = TaskScope {
                task: task.id,
                revision: task.revision,
            };
            state.tasks.attach(&scope, 7, Origin::Created).unwrap();
            let tasks = state.tasks.list();
            state.spaces.update_tasks(&tasks);
            state.spaces.insert(8, USER_SPACE, true);
            state.spaces.insert(7, &scope.task, false);
            state.tabs.insert(8, native_blank_tab());
            state.tabs.insert(7, tab);
            scope
        });
        reply.scope = Some(scope);
        let pending = Pending {
            tab: 7,
            epoch: 4,
            reply,
            evaluation: Evaluation::Page {
                mutation: true,
                evidence: Some(ActionEvidence::Press),
            },
            step: Step::Evaluate,
        };
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.next_message = 42;
            state.pending.insert(42, pending.clone());
        });
        (pending, browser, receiver)
    }

    fn test_press_batch() -> (Shared<PressBatch>, mpsc::Receiver<Response>) {
        let (pending, browser, receiver) = test_press_preparation();
        (
            reserve_press_batch(42, &pending, &browser).unwrap(),
            receiver,
        )
    }

    #[test]
    fn enter_synchronous_acknowledgements_keep_the_lock_until_both_receipts() {
        let (batch, receiver) = test_press_batch();
        let mut sent = Vec::new();
        enqueue_press_batch(
            &batch,
            || true,
            |part, id| {
                sent.push(part);
                devtools_result(&batch.browser, id, true, b"{}");
                assert!(receiver.try_recv().is_err());
                STATE.with(|state| assert!(press_registered(&state.borrow(), &batch)));
                // The normal command admission path must still see TAB_BUSY even
                // when both callbacks arrived before ExecuteDevToolsMethod returned.
                let (mut reply, blocked) = test_reply(702);
                reply.scope = batch.reply.scope.clone();
                handle(&Command::Read { tab: 7 }, reply);
                assert_eq!(blocked.try_recv().unwrap().error.unwrap().code, "TAB_BUSY");
                id
            },
        );
        assert_eq!(sent, [0, 1]);
        let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
        assert_eq!(wire["result"]["page"]["dispatchAcknowledged"], true);
        assert_eq!(wire["result"]["page"]["input"], "chromium");
        assert!(wire["result"]["page"].get("trusted").is_none());
        STATE.with(|state| {
            let state = state.borrow();
            assert!(state.pending.is_empty());
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(8));
        });
        for id in batch.ids {
            devtools_result(&batch.browser, id, true, b"{}");
        }
        assert!(receiver.try_recv().is_err());
        clear();
    }

    #[test]
    fn enter_async_reversed_results_wait_for_both_and_allow_own_navigation() {
        let (batch, receiver) = test_press_batch();
        enqueue_press_batch(&batch, || true, |_, id| id);
        assert!(receiver.try_recv().is_err());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
            nav.epoch += 1;
            nav.provisional = true;
        });
        devtools_result(&batch.browser, batch.ids[1], true, b"");
        assert!(receiver.try_recv().is_err());
        STATE.with(|state| assert_eq!(state.borrow().pending.len(), 2));
        devtools_result(&batch.browser, batch.ids[0], true, b"{}");
        assert!(receiver.try_recv().unwrap().error.is_none());
        assert!(receiver.try_recv().is_err());
        STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        clear();
    }

    #[test]
    fn enter_rejected_receipt_cannot_succeed_after_a_synchronous_ack() {
        for rejected in 0..2 {
            let (batch, receiver) = test_press_batch();
            let mut sent = Vec::new();
            enqueue_press_batch(
                &batch,
                || true,
                |part, id| {
                    sent.push(part);
                    devtools_result(&batch.browser, id, true, b"{}");
                    assert!(receiver.try_recv().is_err());
                    if part == rejected { 0 } else { id }
                },
            );
            assert_eq!(sent.len(), rejected + 1);
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "CONTROL_UNAVAILABLE");
            assert_eq!(
                error.details.unwrap(),
                json!({"tab":7,"outcomeUnknown":true,"attempted":true})
            );
            for id in batch.ids {
                devtools_result(&batch.browser, id, true, b"{}");
            }
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    #[test]
    fn enter_navigation_during_keydown_never_sends_keyup_to_the_new_document() {
        let (batch, receiver) = test_press_batch();
        let mut sent = Vec::new();
        enqueue_press_batch(
            &batch,
            || true,
            |part, id| {
                sent.push(part);
                devtools_result(&batch.browser, id, true, b"{}");
                STATE.with(|state| {
                    state
                        .borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .epoch += 1
                });
                id
            },
        );
        assert_eq!(sent, [0]);
        let error = receiver.try_recv().unwrap().error.unwrap();
        assert_eq!(error.code, "PAGE_CHANGED");
        assert_eq!(error.details.unwrap()["attempted"], true);
        STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        clear();
    }

    #[test]
    fn enter_rechecks_after_native_getters_and_does_not_revive_canceled_batches() {
        for cancel in [false, true] {
            let (batch, receiver) = test_press_batch();
            let mut sent = 0;
            enqueue_press_batch(
                &batch,
                || {
                    if cancel {
                        batch.reply.active.store(false, Ordering::Release);
                        remove_wait(batch.reply.token);
                    } else {
                        STATE.with(|state| state.borrow_mut().tabs.insert(7, native_blank_tab()));
                    }
                    true
                },
                |_, id| {
                    sent += 1;
                    id
                },
            );
            assert_eq!(sent, 0);
            if cancel {
                assert!(receiver.try_recv().is_err());
            } else {
                let error = receiver.try_recv().unwrap().error.unwrap();
                assert_eq!(error.code, "TAB_CLOSED");
                assert_eq!(error.details.unwrap()["attempted"], false);
            }
            for id in batch.ids {
                devtools_result(&batch.browser, id, true, b"{}");
            }
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    #[test]
    fn enter_handoff_detach_and_renderer_exit_terminate_both_parts_once() {
        for cause in 0..3 {
            let (batch, receiver) = test_press_batch();
            let mut sent = Vec::new();
            enqueue_press_batch(
                &batch,
                || true,
                |part, id| {
                    sent.push(part);
                    devtools_result(&batch.browser, id, true, b"{}");
                    match cause {
                        0 => {
                            revoke_task_requests(&batch.reply.scope.as_ref().unwrap().task, Some(7))
                        }
                        1 => control_detached(7),
                        _ => renderer_terminated(7, json!({"status":"crashed"})),
                    }
                    id
                },
            );
            assert_eq!(sent, [0]);
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                [
                    "TASK_CONTROL_LOST",
                    "CONTROL_DISCONNECTED",
                    "RENDERER_TERMINATED"
                ][cause]
            );
            let details = error.details.unwrap();
            assert_eq!(details["outcomeUnknown"], true);
            assert_eq!(details["attempted"], true);
            for id in batch.ids {
                devtools_result(&batch.browser, id, true, b"{}");
            }
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    #[test]
    fn enter_protocol_error_stops_dispatch_and_malformed_late_results_never_succeed() {
        for immediate in [false, true] {
            let (batch, receiver) = test_press_batch();
            let mut sent = Vec::new();
            enqueue_press_batch(
                &batch,
                || true,
                |part, id| {
                    sent.push(part);
                    if immediate {
                        devtools_result(&batch.browser, id, false, br#"{"message":"failed"}"#);
                    }
                    id
                },
            );
            if !immediate {
                assert!(receiver.try_recv().is_err());
                devtools_result(&batch.browser, batch.ids[0], true, b"invalid");
            }
            assert_eq!(sent.len(), if immediate { 1 } else { 2 });
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                if immediate {
                    "CONTROL_ERROR"
                } else {
                    "INVALID_RESULT"
                }
            );
            assert_eq!(error.details.unwrap()["outcomeUnknown"], true);
            devtools_result(&batch.browser, batch.ids[1], true, b"{}");
            assert!(receiver.try_recv().is_err());
        }
        clear();
    }

    #[test]
    fn enter_preparation_rejects_legacy_success_and_stale_documents_without_keys() {
        for stale in [false, true] {
            let (_pending, browser, receiver) = test_press_preparation();
            if stale {
                STATE.with(|state| {
                    state
                        .borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .epoch += 1
                });
            }
            devtools_result(&browser, 42, true, br#"{"result":{"value":{"ok":true,"action":"press","key":"Enter","trusted":false}}}"#);
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                if stale {
                    "PAGE_CHANGED"
                } else {
                    "INVALID_PAGE_RESULT"
                }
            );
            assert_eq!(
                error.details.unwrap(),
                json!({"outcomeUnknown":true,"attempted":false})
            );
            STATE.with(|state| {
                assert!(state.borrow().pending.is_empty());
                assert_eq!(state.borrow().next_message, 42);
            });
        }
        clear();
    }

    #[test]
    fn completed_dom_probe_is_released_when_wait_cannot_finish() {
        for expired in [false, true] {
            clear();
            let (reply, receiver) = test_reply(17);
            let active = reply.active.clone();
            let mut tab = native_blank_tab();
            if !expired {
                tab.navigation.load_failed(
                    -324,
                    "ERR_EMPTY_RESPONSE",
                    "https://example.test/partial",
                );
            }
            let browser = tab.browser.clone();
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.pending.insert(
                    42,
                    Pending {
                        tab: 7,
                        epoch: tab.navigation.epoch,
                        evaluation: Evaluation::ReadyState { token: 17 },
                        reply: reply.clone(),
                        step: Step::Evaluate,
                    },
                );
                state.tabs.insert(7, tab);
                state.waits.insert(
                    17,
                    Waiter {
                        tab: 7,
                        until: WaitUntil::Dom,
                        timeout_ms: 1_000,
                        started: Instant::now() - Duration::from_secs(if expired { 2 } else { 0 }),
                        probe_in_flight: true,
                        reply,
                    },
                );
            });
            // A completed response cannot remain an in-flight probe, even
            // when the timer must still return navigation failure or timeout.
            for _ in 0..2 {
                devtools_result(&browser, 42, true, br#"{"result":{"value":"complete"}}"#);
                STATE.with(|state| {
                    let state = state.borrow();
                    assert!(state.pending.is_empty());
                    assert!(!state.waits[&17].probe_in_flight, "expired={expired}");
                });
                assert!(active.load(Ordering::Acquire));
                assert!(matches!(
                    receiver.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
            }
            clear();
        }
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
                        evaluation: Evaluation::Page {
                            mutation: false,
                            evidence: None,
                        },
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
                        evaluation: Evaluation::Page {
                            mutation: true,
                            evidence: None,
                        },
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
                        evaluation: Evaluation::Page {
                            mutation: false,
                            evidence: None,
                        },
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
                    evaluation: Evaluation::Page {
                        mutation: true,
                        evidence: None,
                    },
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
