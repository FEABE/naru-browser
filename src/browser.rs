use crate::capture::{MAX_CAPTURE_RESULT_BYTES, save_png_base64};
use crate::control_panel::{ShellAction, ShellSnapshot, ShellTab};
use crate::keyboard::{KeyPress, UNSUPPORTED_KEY_MESSAGE};
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
    cell::{Cell, RefCell},
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
const MAX_FOCUS_LEASES: usize = 16;
const FOCUS_ACK_TIMEOUT_MS: i64 = 5000;

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
    focus_completion: Option<i32>,
    frame_completion: Option<i32>,
}
impl Reply {
    fn send(&self, response: Response) {
        if self.focus_completion.is_some() && defer_focus_reply(self, response.clone()) {
            return;
        }
        self.send_after_focus(response);
    }
    fn send_after_focus(&self, response: Response) {
        if let Some(tab) = self.frame_completion {
            finish_frame_action_reply(tab, self, response);
            return;
        }
        self.send_final(response);
    }
    fn send_final(&self, response: Response) {
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
    // A loading callback can change native chrome flags without navigating.
    loading_event_revision: u64,
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
#[derive(Clone)]
enum ActionEvidence {
    Check,
    Fill(Shared<FillTarget>),
    Press(Shared<PressTarget>),
    Click(Shared<ClickTarget>),
}

impl ActionEvidence {
    fn for_command(command: &Command) -> Option<Self> {
        match command {
            Command::Check { .. } => Some(Self::Check),
            Command::Fill {
                reference, text, ..
            }
            | Command::Insert {
                reference, text, ..
            } => Some(Self::Fill(Shared::new(FillTarget {
                reference: reference.clone(),
                text: text.clone(),
                insert: matches!(command, Command::Insert { .. }),
                classification: RefCell::new(None),
                observation: RefCell::new(None),
                context: Cell::new(None),
                kind: Cell::new(None),
                attempted: Cell::new(false),
                native: Cell::new(matches!(command, Command::Insert { .. })),
                binding: RefCell::new(None),
            }))),
            Command::Press { reference, key, .. } => KeyPress::parse(key)
                .map(|key| Self::Press(Shared::new(PressTarget::new(reference.clone(), key)))),
            Command::Click { reference, .. } | Command::Hover { reference, .. } => {
                Some(Self::Click(Shared::new(ClickTarget {
                    reference: reference.clone(),
                    hover: matches!(command, Command::Hover { .. }),
                    context: Cell::new(None),
                    point: Cell::new(None),
                    local_point: Cell::new(None),
                    attempted: Cell::new(false),
                })))
            }
            _ => None,
        }
    }

    fn fields(&self, value: &Json) -> &'static [&'static str] {
        match self {
            Self::Check if value["action"] == "check" => {
                &["attempted", "trusted", "checked", "indeterminate"]
            }
            Self::Fill(target) if value["action"] == target.action() => &["attempted", "trusted"],
            Self::Press(_) if value["action"] == "press" => &["attempted"],
            _ => &[],
        }
    }
}

#[derive(Clone)]
enum Evaluation {
    Page {
        mutation: bool,
        evidence: Option<ActionEvidence>,
    },
    ReadyState {
        token: u64,
        started: Instant,
    },
}
#[derive(Clone)]
enum Step {
    FrameInventory {
        inventory: Shared<FrameInventory>,
        session: Option<String>,
        phase: FramePhase,
    },
    Frame(String),
    World(String),
    Evaluate,
    FillPreflight,
    ClickVerify,
    FillVerify,
    PressVerify,
    Screenshot(String),
    Input {
        batch: Shared<InputBatch>,
        part: usize,
    },
}
#[derive(Clone)]
struct Pending {
    tab: i32,
    epoch: u64,
    frame: Option<Shared<FrameSnapshot>>,
    evaluation: Evaluation,
    reply: Reply,
    step: Step,
}
impl Pending {
    fn session(&self) -> Option<&str> {
        if matches!(&self.step, Step::Input { batch, .. } if batch.kind.is_pointer()) {
            return None;
        }
        if let Some(frame) = &self.frame {
            return frame.session_id();
        }
        match &self.step {
            Step::FrameInventory { session, .. } => session.as_deref(),
            _ => None,
        }
    }

    fn mutation_dispatched(&self) -> bool {
        if let Step::FrameInventory { inventory, .. } = &self.step {
            if (inventory.action.is_some() && inventory.response.borrow().is_some())
                || inventory
                    .pointer
                    .as_ref()
                    .is_some_and(|pointer| pointer.target.attempted.get())
            {
                return true;
            }
        }
        matches!(
            self.step,
            Step::Evaluate
                | Step::ClickVerify
                | Step::FillVerify
                | Step::PressVerify
                | Step::Input { .. }
        ) && matches!(self.evaluation, Evaluation::Page { mutation: true, .. })
    }

    fn input_preparation(&self) -> bool {
        matches!(
            self.step,
            Step::Evaluate | Step::ClickVerify | Step::FillVerify | Step::PressVerify
        ) && matches!(
            self.evaluation,
            Evaluation::Page {
                evidence: Some(
                    ActionEvidence::Press(_) | ActionEvidence::Click(_) | ActionEvidence::Fill(_)
                ),
                ..
            }
        )
    }

    fn outcome_details(&self) -> Json {
        let mut details = json!({"outcomeUnknown":self.mutation_dispatched()});
        if let Step::Input { batch, .. } = &self.step {
            details["attempted"] = json!(
                batch.kind.prior_attempted()
                    || batch.progress.borrow().attempted.iter().any(|value| *value)
            );
        } else if let Some(target) = self.click_target() {
            details["attempted"] = json!(target.attempted.get());
        } else if let Some(target) = self.fill_target() {
            // Before the bridge identifies its route, a legacy DOM setter may
            // already have run. Preserve its previous unknown-outcome contract.
            if target.native.get() {
                details["attempted"] = json!(target.attempted.get());
            }
        } else if let Some(target) = self.press_target() {
            details["attempted"] = json!(target.attempted.get());
        } else if self.input_preparation() {
            details["attempted"] = json!(false);
        }
        details
    }

    fn fill_target(&self) -> Option<&Shared<FillTarget>> {
        match &self.evaluation {
            Evaluation::Page {
                evidence: Some(ActionEvidence::Fill(target)),
                ..
            } => Some(target),
            _ => None,
        }
    }

    fn press_target(&self) -> Option<&Shared<PressTarget>> {
        match &self.evaluation {
            Evaluation::Page {
                evidence: Some(ActionEvidence::Press(target)),
                ..
            } => Some(target),
            _ => None,
        }
    }

    fn click_target(&self) -> Option<&Shared<ClickTarget>> {
        if let Step::FrameInventory { inventory, .. } = &self.step {
            return inventory.pointer.as_ref().map(|pointer| &pointer.target);
        }
        match &self.evaluation {
            Evaluation::Page {
                evidence: Some(ActionEvidence::Click(target)),
                ..
            } => Some(target),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FramePhase {
    /// Child sessions only: browser-side Inspector.enable reports an already
    /// crashed renderer as Inspector.targetCrashed before its ACK.
    CrashProbe,
    Enable,
    Attach,
    Tree,
    RuntimeReset,
    RuntimeEnable,
    World,
    Read,
    RuntimeDisable,
    ParentReset,
    ParentEnable,
    ParentWorld,
    ParentOwner,
    ParentResolve,
    ParentInspect,
    ParentRelease,
    ParentDisable,
    Action,
    Pointer,
}

struct FrameRead {
    frame: crate::frame_sessions::FrameRecord,
    context: crate::frame_sessions::ContextTracker,
    page: Option<Json>,
    unavailable: Option<&'static str>,
    parents: Vec<FrameParent>,
    parent_index: usize,
}

struct CollectedFrame {
    snapshot: Shared<FrameSnapshot>,
    context: crate::frame_sessions::ContextTracker,
}

#[derive(Default)]
struct PageRead {
    remaining: crate::page_read::Budget,
    queue: Vec<crate::frame_sessions::FrameRecord>,
    next: usize,
    documents: Vec<Json>,
    unread: Vec<Json>,
    collected: Vec<CollectedFrame>,
    document_bytes: usize,
    truncated: bool,
}

struct FrameParent {
    child: String,
    frame: crate::frame_sessions::FrameRecord,
    context: crate::frame_sessions::ContextTracker,
    backend_node: Option<u64>,
    object: Option<String>,
    object_group: Option<String>,
    geometry: Option<crate::frame_pointer::Geometry>,
}

struct FrameSnapshot {
    browser: Browser,
    epoch: u64,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
    frame: crate::frame_sessions::FrameRecord,
    unique_context: String,
    prefix: String,
    document_id: String,
    generation: u64,
    references: BTreeSet<String>,
    invalidated: Cell<bool>,
}

impl FrameSnapshot {
    fn session_id(&self) -> Option<&str> {
        self.frame.session_id.as_deref()
    }
    fn unique_context_id(&self) -> &str {
        &self.unique_context
    }
    fn ref_prefix(&self) -> &str {
        &self.prefix
    }
    fn binding(&self) -> Json {
        json!({"documentId":self.document_id,"generation":self.generation})
    }
}

struct FrameAction {
    command: Command,
    snapshot: Shared<FrameSnapshot>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FramePointerStage {
    Geometry,
    ParentGeometry,
    Candidates,
    ParentPoints,
    Move,
    ParentVerify,
    Finish,
}

struct FramePointer {
    target: Shared<ClickTarget>,
    stage: Cell<FramePointerStage>,
    scrolled: Cell<bool>,
    rect: Cell<Option<crate::frame_pointer::Rect>>,
    viewport: Cell<Option<crate::frame_pointer::Rect>>,
    clip: Cell<Option<crate::frame_pointer::Rect>>,
    candidates: RefCell<Vec<crate::frame_pointer::Candidate>>,
    local_candidates: RefCell<Vec<crate::frame_pointer::Candidate>>,
}

impl FramePointer {
    fn new(command: &Command) -> Option<Self> {
        let ActionEvidence::Click(target) = ActionEvidence::for_command(command)? else {
            return None;
        };
        Some(Self {
            target,
            stage: Cell::new(FramePointerStage::Geometry),
            scrolled: Cell::new(false),
            rect: Cell::new(None),
            viewport: Cell::new(None),
            clip: Cell::new(None),
            candidates: RefCell::new(Vec::new()),
            local_candidates: RefCell::new(Vec::new()),
        })
    }
}

struct FrameInventory {
    browser: Browser,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
    requested_frame: Option<String>,
    page_read: Option<RefCell<PageRead>>,
    action: Option<FrameAction>,
    pointer: Option<FramePointer>,
    response: RefCell<Option<Response>>,
    read: RefCell<Option<FrameRead>>,
    current: RefCell<(Option<String>, FramePhase)>,
    invalidated: Cell<bool>,
    visited: RefCell<BTreeSet<Option<String>>>,
    frames: RefCell<Vec<crate::frame_sessions::FrameRecord>>,
    truncated: Cell<bool>,
}

#[derive(Default)]
struct FrameControl {
    enabled: bool,
    root_ready: bool,
    active_token: Option<u64>,
    active_inventory: Option<Shared<FrameInventory>>,
    sessions: crate::frame_sessions::Registry,
    cleanup: Option<i32>,
    runtime_cleanup: Option<i32>,
    object_cleanup: Option<i32>,
    deferred_object_group: Option<String>,
    root_runtime_enabled: bool,
    cleanup_failed: bool,
    snapshots: Vec<Shared<FrameSnapshot>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClickPoint {
    x: f64,
    y: f64,
}

impl ClickPoint {
    fn from_json(value: &Json) -> Option<Self> {
        let point = Self {
            x: value["x"].as_f64()?,
            y: value["y"].as_f64()?,
        };
        [point.x, point.y]
            .iter()
            .all(|value| value.is_finite() && (0.0..=1_000_000.0).contains(value))
            .then_some(point)
    }
}

struct ClickTarget {
    reference: String,
    hover: bool,
    context: Cell<Option<i64>>,
    point: Cell<Option<ClickPoint>>,
    local_point: Cell<Option<ClickPoint>>,
    attempted: Cell<bool>,
}

impl ClickTarget {
    fn action(&self) -> &'static str {
        if self.hover { "hover" } else { "click" }
    }
}

struct FillBinding {
    browser: Browser,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
}

struct FillTarget {
    reference: String,
    text: String,
    insert: bool,
    classification: RefCell<Option<(String, String)>>,
    observation: RefCell<Option<String>>,
    context: Cell<Option<i64>>,
    kind: Cell<Option<&'static str>>,
    attempted: Cell<bool>,
    native: Cell<bool>,
    binding: RefCell<Option<FillBinding>>,
}

impl FillTarget {
    fn action(&self) -> &'static str {
        if self.insert { "insert" } else { "fill" }
    }
}

struct PressTarget {
    reference: String,
    key: KeyPress,
    context: Cell<Option<i64>>,
    observation: RefCell<Option<String>>,
    attempted: Cell<bool>,
}

impl PressTarget {
    fn new(reference: String, key: KeyPress) -> Self {
        Self {
            reference,
            key,
            context: Cell::new(None),
            observation: RefCell::new(None),
            attempted: Cell::new(false),
        }
    }
}

fn native_fill_kind(value: &Json) -> Option<&'static str> {
    match value.as_str()? {
        "text" => Some("text"),
        "search" => Some("search"),
        "tel" => Some("tel"),
        "url" => Some("url"),
        "password" => Some("password"),
        "textarea" => Some("textarea"),
        "contenteditable" => Some("contenteditable"),
        _ => None,
    }
}

enum NativeInput {
    Key(Shared<PressTarget>),
    Fill(Shared<FillTarget>),
    MouseMove(Shared<ClickTarget>),
    Click(ClickPoint),
}

impl NativeInput {
    fn is_pointer(&self) -> bool {
        matches!(self, Self::MouseMove(_) | Self::Click(_))
    }
    fn count(&self) -> usize {
        if matches!(self, Self::MouseMove(_) | Self::Fill(_)) {
            1
        } else {
            2
        }
    }
    fn prior_attempted(&self) -> bool {
        matches!(self, Self::Click(_))
            || matches!(self, Self::Fill(target) if target.attempted.get())
    }
    fn verifies_after_input(&self) -> bool {
        matches!(self, Self::MouseMove(_) | Self::Fill(_) | Self::Key(_))
    }
    fn allows_navigation(&self) -> bool {
        matches!(self, Self::Key(_) | Self::Click(_))
    }
    fn method(&self) -> &'static str {
        match self {
            Self::Key(_) => "Input.dispatchKeyEvent",
            Self::Fill(_) => "Input.insertText",
            _ => "Input.dispatchMouseEvent",
        }
    }
    fn params(&self, part: usize) -> Json {
        match self {
            Self::Fill(target) => json!({"text":target.text}),
            Self::Key(target) => {
                let key = target.key;
                let text = key.text();
                let down = if text.is_some() {
                    "keyDown"
                } else {
                    "rawKeyDown"
                };
                let mut params = json!({"type":if part == 0 {down} else {"keyUp"},
                    "key":key.dom_key(),"code":key.code(),"windowsVirtualKeyCode":key.windows_virtual_key_code(),
                    "nativeVirtualKeyCode":0,"modifiers":key.modifiers(),"autoRepeat":false,"isKeypad":false});
                if part == 0
                    && let Some(text) = text
                {
                    params["text"] = json!(text);
                    params["unmodifiedText"] = json!(text);
                }
                if part == 0 && !key.editing_commands().is_empty() {
                    params["commands"] = json!(key.editing_commands());
                }
                params
            }
            Self::MouseMove(target) => {
                let point = target
                    .point
                    .get()
                    .expect("mouse move follows point preparation");
                json!({"type":"mouseMoved","x":point.x,"y":point.y,"button":"none","buttons":0,"clickCount":0,"modifiers":0,"pointerType":"mouse"})
            }
            Self::Click(point) => {
                json!({"type":if part == 0 {"mousePressed"} else {"mouseReleased"},
                "x":point.x,"y":point.y,"button":"left","buttons":if part == 0 {1} else {0},"clickCount":1,"modifiers":0,"pointerType":"mouse"})
            }
        }
    }
    fn success_page(&self) -> Json {
        match self {
            Self::Key(target) => {
                json!({"ok":true,"action":"press","key":target.key.canonical(),"input":"chromium",
                "attempted":true,"dispatchAcknowledged":true,"keydownTarget":"unconfirmed",
                "detail":"Chromium acknowledged the keys, but the target observation is unavailable. Read again to verify effects; do not automatically repeat the action."})
            }
            Self::Click(_) => json!({"ok":true,"action":"click","input":"chromium",
                "attempted":true,"dispatchAcknowledged":true,
                "detail":"Chromium acknowledged mousePressed and mouseReleased; read again to verify effects."}),
            Self::MouseMove(_) | Self::Fill(_) => {
                unreachable!("intermediate input must be verified before completing the action")
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct InputFailure {
    code: &'static str,
    message: &'static str,
}

struct InputProgress {
    enqueuing: bool,
    attempted: [bool; 2],
    accepted: [Option<bool>; 2],
    acknowledged: [bool; 2],
    failure: Option<InputFailure>,
}

struct InputBatch {
    ids: [i32; 2],
    kind: NativeInput,
    tab: i32,
    epoch: u64,
    frame: Option<Shared<FrameSnapshot>>,
    // Retain the original browser until both parts finish; raw identity cannot
    // be recycled while a late callback is being checked.
    browser: Browser,
    reply: Reply,
    progress: RefCell<InputProgress>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusPhase {
    Enabling,
    Active,
    Disabling,
    Failed,
}

struct FocusMessage {
    id: i32,
    started: bool,
    sending: bool,
    accepted: Option<bool>,
    acknowledged: Option<bool>,
}

impl FocusMessage {
    fn new(id: i32) -> Self {
        Self {
            id,
            started: false,
            sending: true,
            accepted: None,
            acknowledged: None,
        }
    }
    fn finished(&self) -> Option<bool> {
        if self.sending {
            return None;
        }
        if self.accepted == Some(false) {
            return Some(false);
        }
        (self.accepted == Some(true))
            .then_some(self.acknowledged)
            .flatten()
    }
}

enum FocusContinuation {
    Dispatch {
        method: String,
        params: Json,
        pending: Pending,
    },
    Verify(Shared<InputBatch>),
    Respond {
        reply: Reply,
        response: Response,
    },
}

struct FocusLease {
    browser: Browser,
    session: Option<String>,
    token: u64,
    reply: Reply,
    evidence: Option<ActionEvidence>,
    attempted: bool,
    phase: FocusPhase,
    enable: FocusMessage,
    disable: Option<FocusMessage>,
    disable_after_enable: bool,
    initial: Option<FocusContinuation>,
    after: Option<FocusContinuation>,
    stop_frames: bool,
    started: Instant,
}

impl FocusLease {
    fn attempted(&self) -> bool {
        self.attempted
            || match &self.evidence {
                Some(ActionEvidence::Press(target)) => target.attempted.get(),
                Some(ActionEvidence::Fill(target)) => target.attempted.get(),
                Some(ActionEvidence::Click(target)) => target.attempted.get(),
                _ => false,
            }
    }
    fn failure(&self, code: &str, message: &str, cleanup: &str) -> Response {
        Response::error(&self.reply.id, code, message).with_details(json!({
            "attempted":self.attempted(),"outcomeUnknown":self.attempted(),
            "focusCleanup":cleanup
        }))
    }
}
#[derive(Clone)]
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
    let id = state.next_control_message()?;
    state.preview_attempted.insert(key.clone());
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
        let accepted = browser.host().is_some_and(|host| {
            send_root_command(&host, id, "Page.captureScreenshot", &params) == id
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
    // A lease survives removal of Pending/Reply ownership. At most one per tab;
    // failed cleanup remains bounded and blocks further page actions until an
    // exact late disable ACK or destruction of the control connection.
    focus_leases: BTreeMap<i32, FocusLease>,
    focus_watchdog_queued: bool,
    frame_controls: BTreeMap<i32, FrameControl>,
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
impl State {
    fn next_control_message(&mut self) -> Option<i32> {
        let id = self.next_message.checked_add(1)?;
        self.next_message = id;
        Some(id)
    }
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

thread_local! { static SHELL_SNAPSHOT_READING: Cell<bool> = const { Cell::new(false) }; }

struct ShellSnapshotRead;
impl ShellSnapshotRead {
    fn acquire() -> Option<Self> {
        SHELL_SNAPSHOT_READING.with(|reading| {
            if reading.replace(true) {
                None
            } else {
                Some(Self)
            }
        })
    }
}
impl Drop for ShellSnapshotRead {
    fn drop(&mut self) {
        SHELL_SNAPSHOT_READING.with(|reading| reading.set(false));
    }
}

#[derive(Clone, Copy)]
enum ShellProbe {
    Loading,
    Back,
    Forward,
}

struct ShellObservation {
    tab: i32,
    browser: Browser,
    epoch: u64,
    document_epoch: u64,
    loading_revision: u64,
    loading: bool,
    back: bool,
    forward: bool,
}

/// CEF wraps the same browser in a new C struct each time it crosses the API
/// (create_browser_sync, callbacks), so raw pointers differ for one browser.
/// Equal pointers are always the same browser and need no CEF call.
fn same_browser(a: &Browser, b: &Browser) -> bool {
    a.get_raw() == b.get_raw() || a.is_same(Some(&mut b.clone())) != 0
}

fn shell_observation_matches(state: &State, sample: &ShellObservation) -> bool {
    state.tabs.get(&sample.tab).is_some_and(|tab| {
        !tab.rejected_creation
            && same_browser(&tab.browser, &sample.browser)
            && tab.navigation.epoch == sample.epoch
            && tab.navigation.document_epoch == sample.document_epoch
            && tab.navigation.loading_event_revision == sample.loading_revision
    })
}

fn shell_observation_current(sample: &ShellObservation) -> bool {
    STATE.with(|slot| {
        slot.try_borrow()
            .is_ok_and(|state| shell_observation_matches(&state, sample))
    })
}

fn shell_snapshot_with(
    mut probe: impl FnMut(&Browser, ShellProbe) -> bool,
) -> Option<ShellSnapshot> {
    let mut read = || -> Option<ShellSnapshot> {
        let _reading = ShellSnapshotRead::acquire()?;
        let mut samples = STATE.with(|slot| {
            let state = slot.try_borrow().ok()?;
            Some(
                state
                    .tabs
                    .iter()
                    .filter(|(_, tab)| !tab.rejected_creation)
                    .map(|(id, tab)| {
                        (
                            *id,
                            ShellObservation {
                                tab: *id,
                                browser: tab.browser.clone(),
                                epoch: tab.navigation.epoch,
                                document_epoch: tab.navigation.document_epoch,
                                loading_revision: tab.navigation.loading_event_revision,
                                loading: false,
                                back: false,
                                forward: false,
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            )
        })?;
        for sample in samples.values_mut() {
            for kind in [ShellProbe::Loading, ShellProbe::Back, ShellProbe::Forward] {
                if !shell_observation_current(sample) {
                    return None;
                }
                // Even read-only CEF getters can reenter. No STATE borrow is
                // retained, and an invalidated sample never reaches the next getter.
                let value = probe(&sample.browser, kind);
                if !shell_observation_current(sample) {
                    return None;
                }
                match kind {
                    ShellProbe::Loading => sample.loading = value,
                    ShellProbe::Back => sample.back = value,
                    ShellProbe::Forward => sample.forward = value,
                }
            }
        }
        STATE.with(|slot| {
            let state = slot.try_borrow().ok()?;
            // A later tab's getter can invalidate an earlier observation or
            // create a tab. Never publish a partial or mixed-generation list.
            if state
                .tabs
                .values()
                .filter(|tab| !tab.rejected_creation)
                .count()
                != samples.len()
                || samples
                    .values()
                    .any(|sample| !shell_observation_matches(&state, sample))
            {
                return None;
            }
            let spaces = state.spaces.views();
            Some(ShellSnapshot {
                active_space: state.spaces.active_space().to_owned(),
                active_tab: state.spaces.active_tab(),
                overview: state.spaces.overview(),
                tabs: spaces
                    .iter()
                    .flat_map(|space| space.tabs.iter())
                    .filter_map(|id| state.tabs.get(id).map(|tab| (id, tab)))
                    .filter(|(_, tab)| !tab.rejected_creation)
                    .map(|(id, tab)| {
                        let sample = &samples[id];
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
                        let loading = sample.loading
                            || tab.navigation.provisional
                            || tab.navigation.intent_pending;
                        ShellTab {
                            id: *id,
                            navigation_epoch: tab.navigation.epoch,
                            space: state.spaces.tab_space(*id).unwrap_or(USER_SPACE).to_owned(),
                            title: tab.title.chars().take(120).collect(),
                            url: tab.navigation.requested_url.clone(),
                            can_go_back: sample.back,
                            can_go_forward: sample.forward,
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
            })
        })
    };
    let snapshot = read();
    if snapshot.is_none() {
        notify_panel();
    }
    snapshot
}

pub fn shell_snapshot() -> Option<ShellSnapshot> {
    shell_snapshot_with(|browser, kind| match kind {
        ShellProbe::Loading => browser.is_loading() != 0,
        ShellProbe::Back => browser.can_go_back() != 0,
        ShellProbe::Forward => browser.can_go_forward() != 0,
    })
}

fn observe_loading_state(tab: i32, loading: bool) {
    STATE.with(|slot| {
        if let Some(tab) = slot.borrow_mut().tabs.get_mut(&tab) {
            tab.navigation.loading_event_revision =
                tab.navigation.loading_event_revision.wrapping_add(1);
            if !loading {
                tab.navigation.finish_loading();
                tab.navigation.history.attempt = None;
            }
        }
    });
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
            focus_completion: None,
            frame_completion: None,
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
    control_navigation(
        &NavigationControlTarget {
            tab: target.tab,
            space: target.space.clone(),
            navigation_epoch: target.navigation_epoch,
            stop: false,
        },
        || true,
        || true,
    )
    .map(|_| ())
    .map_err(|error| error.message)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavigationControlTarget {
    pub tab: i32,
    pub space: String,
    pub navigation_epoch: u64,
    pub stop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavigationControlOutcome {
    pub navigation_epoch: u64,
    pub preserve_address: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavigationControlError {
    pub message: String,
    pub preserved_address: Option<NavigationControlOutcome>,
}

struct NavigationControlGuard {
    tab: i32,
    browser: Browser,
    epoch: u64,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
    // CLI stop can target a background tab; native controls must stay selected.
    space: Option<String>,
}

const NAVIGATION_CONTROL_CHANGED: &str = "페이지 상태가 바뀌었습니다. 현재 페이지를 확인해 주세요.";

fn navigation_control_guard(
    state: &State,
    tab: i32,
    target: Option<&NavigationControlTarget>,
) -> Result<NavigationControlGuard, String> {
    let page = state
        .tabs
        .get(&tab)
        .filter(|page| !page.rejected_creation)
        .ok_or_else(|| NAVIGATION_CONTROL_CHANGED.to_owned())?;
    let guard = NavigationControlGuard {
        tab,
        browser: page.browser.clone(),
        epoch: target.map_or(page.navigation.epoch, |target| target.navigation_epoch),
        document_epoch: page.navigation.document_epoch,
        owner: link_owner(&state.tasks, tab),
        space: target.map(|target| target.space.clone()),
    };
    if navigation_control_matches(state, &guard) {
        Ok(guard)
    } else {
        Err(NAVIGATION_CONTROL_CHANGED.into())
    }
}

fn navigation_control_matches(state: &State, guard: &NavigationControlGuard) -> bool {
    (guard.space.is_none() || !state.spaces.overview()) && navigation_address_matches(state, guard)
}

// Losing chrome focus or opening an overlay blocks a command, but does not
// change the document to which an existing address draft belongs.
fn navigation_address_matches(state: &State, guard: &NavigationControlGuard) -> bool {
    guard.space.as_ref().is_none_or(|space| {
        state.spaces.active_tab() == Some(guard.tab)
            && state.spaces.active_space() == space
            && state.spaces.tab_space(guard.tab) == Some(space.as_str())
    }) && link_owner(&state.tasks, guard.tab) == guard.owner
        && state.tabs.get(&guard.tab).is_some_and(|page| {
            !page.rejected_creation
                && same_browser(&page.browser, &guard.browser)
                && page.navigation.epoch == guard.epoch
                && page.navigation.document_epoch == guard.document_epoch
        })
}

// Short-lived proof spanning the shell's optional snapshot refresh. Keep this
// opaque so UI code cannot manufacture a new document/owner identity.
pub(crate) struct NavigationAddressReceipt {
    guard: NavigationControlGuard,
    url: String,
}

pub(crate) fn capture_navigation_address_receipt(
    target: &NavigationControlTarget,
    url: &str,
    outcome: &NavigationControlOutcome,
) -> Option<NavigationAddressReceipt> {
    if !outcome.preserve_address {
        return None;
    }
    STATE.with(|slot| {
        let state = slot.try_borrow().ok()?;
        let mut guard = navigation_control_guard(&state, target.tab, None).ok()?;
        guard.space = Some(target.space.clone());
        guard.epoch = outcome.navigation_epoch;
        if !navigation_address_matches(&state, &guard)
            || state.tabs[&target.tab].navigation.requested_url != url
        {
            return None;
        }
        Some(NavigationAddressReceipt {
            guard,
            url: url.to_owned(),
        })
    })
}

pub(crate) fn navigation_address_receipt_current(receipt: &NavigationAddressReceipt) -> bool {
    STATE.with(|slot| {
        slot.try_borrow().is_ok_and(|state| {
            navigation_address_matches(&state, &receipt.guard)
                && state.tabs[&receipt.guard.tab].navigation.requested_url == receipt.url
        })
    })
}

fn navigation_control_current(
    guard: &NavigationControlGuard,
    still_current: &impl Fn() -> bool,
) -> Result<(), String> {
    // This predicate is pure UI state. Native window/IME getters run only in
    // native_ready, before the final CEF loading sample.
    if still_current() && STATE.with(|slot| navigation_control_matches(&slot.borrow(), guard)) {
        Ok(())
    } else {
        Err(NAVIGATION_CONTROL_CHANGED.into())
    }
}

fn navigation_control_needed(
    guard: &NavigationControlGuard,
    stop: bool,
    loading: bool,
) -> Result<bool, String> {
    STATE.with(|slot| {
        let state = slot.borrow();
        if !navigation_control_matches(&state, guard) {
            return Err(NAVIGATION_CONTROL_CHANGED.into());
        }
        let nav = &state.tabs[&guard.tab].navigation;
        let busy = loading || nav.provisional || nav.intent_pending;
        if stop {
            // `stopped` can outlive this load. A later live resource load must
            // still be stoppable; only an outstanding stop is idempotent.
            Ok(busy && !nav.stop_pending)
        } else if busy || nav.stop_pending || nav.recovering {
            Err("페이지가 아직 로딩 중입니다. 중지하거나 로딩이 끝난 뒤 다시 시도해 주세요.".into())
        } else {
            Ok(true)
        }
    })
}

fn navigation_control_inspect(
    guard: &NavigationControlGuard,
    native_ready: &impl Fn() -> bool,
    still_current: &impl Fn() -> bool,
    inspect: &mut impl FnMut(&Browser) -> (bool, bool),
) -> Result<bool, String> {
    navigation_control_current(guard, still_current)?;
    if !native_ready() {
        return Err(NAVIGATION_CONTROL_CHANGED.into());
    }
    navigation_control_current(guard, still_current)?;
    let (valid, loading) = inspect(&guard.browser);
    navigation_control_current(guard, still_current)?;
    if !valid {
        return Err(NAVIGATION_CONTROL_CHANGED.into());
    }
    Ok(loading)
}

fn clear_navigation_controls(
    guard: &NavigationControlGuard,
    still_current: &impl Fn() -> bool,
) -> Result<(), String> {
    navigation_control_current(guard, still_current)?;
    abort_focus_lease(
        guard.tab,
        "PAGE_CHANGED",
        "Navigation control changed during input; inspect before retrying.",
        true,
    );
    navigation_control_current(guard, still_current)?;
    revoke_download_permission_provenance(guard.tab);
    crate::download_permission_runtime::cancel_tab(guard.tab);
    navigation_control_current(guard, still_current)?;
    invalidate_page_find(guard.tab);
    navigation_control_current(guard, still_current)
}

// Inspection, cleanup and dispatch are injected only at the native boundary so
// state/ownership transitions can be exercised without starting Chromium.
fn execute_navigation_control(
    mut guard: NavigationControlGuard,
    stop: bool,
    native_ready: impl Fn() -> bool,
    still_current: impl Fn() -> bool,
    mut inspect: impl FnMut(&Browser) -> (bool, bool),
    mut cleanup: impl FnMut(&NavigationControlGuard) -> Result<(), String>,
    send: impl FnOnce(&Browser, bool),
) -> Result<NavigationControlOutcome, NavigationControlError> {
    let operation = (|| -> Result<NavigationControlOutcome, String> {
        let loading =
            navigation_control_inspect(&guard, &native_ready, &still_current, &mut inspect)?;
        if !navigation_control_needed(&guard, stop, loading)? {
            return Ok(NavigationControlOutcome {
                navigation_epoch: guard.epoch,
                preserve_address: stop,
            });
        }
        if let Some((scope, Status::Active)) = guard.owner.clone() {
            let epoch = guard
                .epoch
                .checked_add(1)
                .ok_or(NAVIGATION_CONTROL_CHANGED)?;
            let revision = scope
                .revision
                .checked_add(1)
                .ok_or(NAVIGATION_CONTROL_CHANGED)?;
            handoff_native(scope.clone())?;
            guard.epoch = epoch;
            guard.owner = Some((TaskScope { revision, ..scope }, Status::Paused));
            navigation_control_current(&guard, &still_current)?;
        }
        // Handoff and native cleanup can synchronously finish, replace or navigate
        // a page. Re-read loading and validate the original identity after each.
        let loading =
            navigation_control_inspect(&guard, &native_ready, &still_current, &mut inspect)?;
        if !navigation_control_needed(&guard, stop, loading)? {
            return Ok(NavigationControlOutcome {
                navigation_epoch: guard.epoch,
                preserve_address: stop,
            });
        }
        cleanup(&guard)?;
        let loading =
            navigation_control_inspect(&guard, &native_ready, &still_current, &mut inspect)?;
        if !navigation_control_needed(&guard, stop, loading)? {
            return Ok(NavigationControlOutcome {
                navigation_epoch: guard.epoch,
                preserve_address: stop,
            });
        }
        let next_epoch = guard
            .epoch
            .checked_add(1)
            .ok_or(NAVIGATION_CONTROL_CHANGED)?;
        let history_generation = crate::history_runtime::generation();
        STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            if !navigation_control_matches(&state, &guard) {
                return Err(NAVIGATION_CONTROL_CHANGED.to_owned());
            }
            let history_generation =
                history_generation.filter(|_| is_unowned_user_tab(&state, guard.tab));
            let nav = &mut state.tabs.get_mut(&guard.tab).unwrap().navigation;
            if stop {
                nav.epoch = next_epoch;
                nav.automatic_retry_error = None;
                nav.history.revoke();
                nav.stopped = true;
                nav.stop_pending = true;
            } else {
                nav.begin(None);
                nav.history.begin(history_generation, true);
            }
            Ok(())
        })?;
        guard.epoch = next_epoch;
        // No native getter or UI callback between reservation and transmission.
        send(&guard.browser, stop);
        // The request was sent even if its synchronous callback navigated or closed
        // the page. Such a callback must not authorize rebasing an old address draft.
        Ok(NavigationControlOutcome {
            navigation_epoch: guard.epoch,
            preserve_address: false,
        })
    })();
    // Only the exact original page and our own expected epochs authorize a
    // draft rebase. This is separate from whether chrome still permits input.
    let preserve_address = (stop || operation.is_err())
        && STATE.with(|slot| navigation_address_matches(&slot.borrow(), &guard));
    match operation {
        Ok(mut outcome) => {
            outcome.preserve_address = preserve_address;
            Ok(outcome)
        }
        Err(message) => Err(NavigationControlError {
            message,
            preserved_address: preserve_address.then_some(NavigationControlOutcome {
                navigation_epoch: guard.epoch,
                preserve_address: true,
            }),
        }),
    }
}

fn native_navigation_control(
    guard: NavigationControlGuard,
    stop: bool,
    native_ready: impl Fn() -> bool,
    still_current: impl Fn() -> bool,
) -> Result<NavigationControlOutcome, NavigationControlError> {
    execute_navigation_control(
        guard,
        stop,
        native_ready,
        &still_current,
        |browser| (browser.is_valid() != 0, browser.is_loading() != 0),
        |guard| clear_navigation_controls(guard, &still_current),
        |browser, stop| {
            if stop {
                browser.stop_load()
            } else {
                browser.reload()
            }
        },
    )
}

pub(crate) fn control_navigation(
    target: &NavigationControlTarget,
    native_ready: impl Fn() -> bool,
    still_current: impl Fn() -> bool,
) -> Result<NavigationControlOutcome, NavigationControlError> {
    let guard = STATE
        .with(|slot| navigation_control_guard(&slot.borrow(), target.tab, Some(target)))
        .map_err(|message| NavigationControlError {
            message,
            preserved_address: None,
        })?;
    let result = native_navigation_control(guard, target.stop, native_ready, still_current);
    notify_panel();
    result
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
    navigate(&browser, &command, reply, ObservationAccess::Native);
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
    // OnBeforeClose of the last tab quits; without tabs nothing else will.
    if browsers.is_empty() {
        quit_message_loop();
    }
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

#[derive(Clone, Copy)]
enum ObservationAccess {
    Tab,
    List,
    Native,
}

struct TabObservation {
    sample: ShellObservation,
    owner: Option<(TaskScope, Status)>,
    access: ObservationAccess,
    native_space: Option<String>,
    frame: Option<Frame>,
    url: String,
}

const OBSERVATION_CHANGED: TaskError = TaskError {
    code: "PAGE_CHANGED",
    message: "The page changed while its state was being read. Inspect again.",
};

fn observation_access(
    state: &State,
    reply: &Reply,
    tab: i32,
    access: ObservationAccess,
) -> Result<(), TaskError> {
    if !reply.active.load(Ordering::Acquire) {
        return Err(TaskError {
            code: "COMMAND_CANCELED",
            message: "The request is no longer active.",
        });
    }
    match access {
        ObservationAccess::Native => Ok(()),
        ObservationAccess::List if reply.scope.is_none() => Ok(()),
        _ => state.tasks.check_tab(reply.scope.as_ref(), tab),
    }
}

fn observation_matches(
    state: &State,
    reply: &Reply,
    observation: &TabObservation,
) -> Result<(), TaskError> {
    observation_access(state, reply, observation.sample.tab, observation.access)?;
    if !shell_observation_matches(state, &observation.sample)
        || link_owner(&state.tasks, observation.sample.tab) != observation.owner
        || observation.native_space.as_ref().is_some_and(|space| {
            state.spaces.overview()
                || state.spaces.active_tab() != Some(observation.sample.tab)
                || state.spaces.active_space() != space
                || state.spaces.tab_space(observation.sample.tab) != Some(space.as_str())
        })
    {
        return Err(OBSERVATION_CHANGED);
    }
    Ok(())
}

fn observation_current(reply: &Reply, observation: &TabObservation) -> Result<(), TaskError> {
    STATE.with(|slot| observation_matches(&slot.borrow(), reply, observation))
}

fn observe_tab_with(
    tab: i32,
    reply: &Reply,
    access: ObservationAccess,
    mut frame: impl FnMut(&Browser) -> Option<Frame>,
    mut url: impl FnMut(&Frame) -> String,
    mut flag: impl FnMut(&Browser, ShellProbe) -> bool,
) -> Result<TabObservation, TaskError> {
    let mut observation = STATE.with(|slot| {
        let state = slot.borrow();
        observation_access(&state, reply, tab, access)?;
        let page = state
            .tabs
            .get(&tab)
            .filter(|page| !page.rejected_creation)
            .ok_or(TaskError {
                code: "TAB_NOT_FOUND",
                message: "No available browser tab has that ID.",
            })?;
        let native_space = if matches!(access, ObservationAccess::Native) {
            if state.spaces.overview() || state.spaces.active_tab() != Some(tab) {
                return Err(OBSERVATION_CHANGED);
            }
            let space = state.spaces.tab_space(tab).ok_or(OBSERVATION_CHANGED)?;
            if state.spaces.active_space() != space {
                return Err(OBSERVATION_CHANGED);
            }
            Some(space.to_owned())
        } else {
            None
        };
        Ok(TabObservation {
            sample: ShellObservation {
                tab,
                browser: page.browser.clone(),
                epoch: page.navigation.epoch,
                document_epoch: page.navigation.document_epoch,
                loading_revision: page.navigation.loading_event_revision,
                loading: false,
                back: false,
                forward: false,
            },
            owner: link_owner(&state.tasks, tab),
            access,
            native_space,
            frame: None,
            url: String::new(),
        })
    })?;
    observation_current(reply, &observation)?;
    observation.frame = frame(&observation.sample.browser);
    observation_current(reply, &observation)?;
    if let Some(frame) = &observation.frame {
        observation.url = url(frame);
        observation_current(reply, &observation)?;
    }
    for kind in [ShellProbe::Loading, ShellProbe::Back, ShellProbe::Forward] {
        observation_current(reply, &observation)?;
        let value = flag(&observation.sample.browser, kind);
        observation_current(reply, &observation)?;
        match kind {
            ShellProbe::Loading => observation.sample.loading = value,
            ShellProbe::Back => observation.sample.back = value,
            ShellProbe::Forward => observation.sample.forward = value,
        }
    }
    Ok(observation)
}

fn observe_tab(
    tab: i32,
    reply: &Reply,
    access: ObservationAccess,
) -> Result<TabObservation, TaskError> {
    observe_tab_with(
        tab,
        reply,
        access,
        |browser| browser.main_frame(),
        |frame| CefString::from(&frame.url()).to_string(),
        |browser, kind| match kind {
            ShellProbe::Loading => browser.is_loading() != 0,
            ShellProbe::Back => browser.can_go_back() != 0,
            ShellProbe::Forward => browser.can_go_forward() != 0,
        },
    )
}

fn tab_info(id: i32, tab: &Tab, observed: &TabObservation) -> Json {
    json!({"tab":id,"title":tab.title,
    "url":observed.url,"loading":observed.sample.loading,
    "canGoBack":observed.sample.back,"canGoForward":observed.sample.forward,
    "renderer":{"status":tab.navigation.renderer_status(),"error":tab.navigation.renderer_exit},
    "navigation": {
        "epoch":tab.navigation.epoch,"requestedUrl":tab.navigation.requested_url,
        "provisional":tab.navigation.provisional,"committed":tab.navigation.committed,
        "documentAvailable":tab.navigation.document_available,
        "mainLoadComplete":tab.navigation.main_load_complete,"error":tab.navigation.error,
        "stopped":tab.navigation.stopped,"incomplete":tab.navigation.incomplete
    }})
}

fn observed_tab_info(reply: &Reply, observation: &TabObservation) -> Result<Json, TaskError> {
    STATE.with(|slot| {
        let state = slot.borrow();
        observation_matches(&state, reply, observation)?;
        Ok(tab_info(
            observation.sample.tab,
            &state.tabs[&observation.sample.tab],
            observation,
        ))
    })
}

fn list_tab_ids(state: &State, reply: &Reply) -> Result<Vec<i32>, TaskError> {
    if !reply.active.load(Ordering::Acquire) {
        return Err(OBSERVATION_CHANGED);
    }
    if let Some(scope) = &reply.scope {
        state.tasks.check(scope)?;
    }
    Ok(state
        .tabs
        .iter()
        .filter(|(id, tab)| {
            !tab.rejected_creation
                && reply.scope.as_ref().is_none_or(|scope| {
                    state
                        .tasks
                        .owner(**id)
                        .is_some_and(|(owner, _)| owner == scope.task)
                })
        })
        .map(|(id, _)| *id)
        .collect())
}

fn tabs_info_with(
    reply: &Reply,
    mut observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
) -> Result<Json, TaskError> {
    let ids = STATE.with(|slot| list_tab_ids(&slot.borrow(), reply))?;
    let observations = ids
        .iter()
        .map(|id| observe(*id, reply, ObservationAccess::List))
        .collect::<Result<Vec<_>, _>>()?;
    STATE.with(|slot| {
        let state = slot.borrow();
        if list_tab_ids(&state, reply)? != ids {
            return Err(OBSERVATION_CHANGED);
        }
        let mut tabs = Vec::with_capacity(observations.len());
        for observation in &observations {
            observation_matches(&state, reply, observation)?;
            let id = observation.sample.tab;
            let mut info = tab_info(id, &state.tabs[&id], observation);
            info["active"] = json!(state.spaces.active_tab() == Some(id));
            info["space"] = json!(state.spaces.tab_space(id).unwrap_or(USER_SPACE));
            info["task"] = state
                .tasks
                .owner(id)
                .map(|(id, origin)| json!({"id":id,"origin":origin}))
                .unwrap_or(Json::Null);
            tabs.push(info);
        }
        Ok(json!({"tabs":tabs}))
    })
}

fn diagnostic_tab_info(
    tab: i32,
    browser: &Browser,
    reply: &Reply,
    unavailable: impl Fn(&Tab) -> bool,
) -> Result<Option<Json>, TaskError> {
    let needed = STATE.with(|slot| slot.borrow().tabs.get(&tab).is_some_and(&unavailable));
    if !needed {
        return Ok(None);
    }
    let observed = observe_tab(tab, reply, ObservationAccess::Tab)?;
    if !same_browser(&observed.sample.browser, &browser) {
        return Err(OBSERVATION_CHANGED);
    }
    STATE.with(|slot| {
        let state = slot.borrow();
        observation_matches(&state, reply, &observed)?;
        let page = &state.tabs[&tab];
        Ok(unavailable(page).then(|| tab_info(tab, page, &observed)))
    })
}

fn reply_created_tab_with(
    tab: i32,
    browser: &Browser,
    reply: &Reply,
    mut observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
) {
    let info = observe(tab, reply, ObservationAccess::Tab).and_then(|observed| {
        if !same_browser(&observed.sample.browser, &browser) {
            return Err(OBSERVATION_CHANGED);
        }
        observed_tab_info(reply, &observed)
    });
    match info {
        Ok(info) => reply.ok(info),
        Err(error) => reply.error_details(
            error.code,
            error.message,
            json!({"tab":tab,"tabCreated":true,"stateUnavailable":true,"outcomeUnknown":false}),
        ),
    }
}

fn observation_error(reply: &Reply, error: TaskError) {
    reply.error_details(error.code, error.message, json!({"outcomeUnknown":false}));
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
                &Response::error(protocol::INVALID_REQUEST_ID, error.code, error.message),
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
            focus_completion: None,
            frame_completion: None,
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
    struct WaitTask { token: u64, tab: i32, started: Instant, active: Arc<AtomicBool> }
    impl Task {
        fn execute(&self) {
            if wait_tick_current(self.token, self.tab, self.started, &self.active) { poll_wait(self.token); }
        }
    }
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
    let (waiter, frame_tabs) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let frame_tabs = state
            .frame_controls
            .iter()
            .filter_map(|(tab, control)| (control.active_token == Some(token)).then_some(*tab))
            .collect::<Vec<_>>();
        state
            .pending
            .retain(|_, pending| pending.reply.token != token);
        (state.waits.remove(&token), frame_tabs)
    });
    let focus_tabs = STATE.with(|slot| {
        slot.borrow()
            .focus_leases
            .iter()
            .filter_map(|(tab, lease)| (lease.token == token).then_some(*tab))
            .collect::<Vec<_>>()
    });
    for tab in focus_tabs {
        abort_focus_lease(
            tab,
            "TIMEOUT",
            "The input request was canceled or timed out; do not repeat it automatically.",
            true,
        );
    }
    for tab in frame_tabs {
        stop_frame_tracking(tab, Some(token));
    }
    if waiter.is_some() {
        notify_panel();
    }
    waiter
}

fn wait_tick_current(token: u64, tab: i32, started: Instant, active: &Arc<AtomicBool>) -> bool {
    STATE.with(|slot| {
        slot.borrow().waits.get(&token).is_some_and(|waiter| {
            waiter.tab == tab
                && waiter.started == started
                && Arc::ptr_eq(&waiter.reply.active, active)
        })
    })
}

fn same_waiter(current: &Waiter, captured: &Waiter) -> bool {
    current.tab == captured.tab
        && current.started == captured.started
        && Arc::ptr_eq(&current.reply.active, &captured.reply.active)
}

fn poll_wait_with(
    token: u64,
    mut observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
) -> Option<(Browser, Pending)> {
    let captured = STATE.with(|slot| slot.borrow().waits.get(&token).cloned())?;
    if !captured.reply.active.load(Ordering::Acquire) {
        return None;
    }
    let access = STATE.with(|slot| {
        slot.borrow()
            .tasks
            .check_tab(captured.reply.scope.as_ref(), captured.tab)
    });
    if let Err(error) = access {
        captured.reply.error(error.code, error.message);
        return None;
    }
    let observed = observe(captured.tab, &captured.reply, ObservationAccess::Tab);
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let waiter = state.waits.get(&token)?;
        if !same_waiter(waiter, &captured) || !waiter.reply.active.load(Ordering::Acquire) {
            return None;
        }
        if let Err(error) = state
            .tasks
            .check_tab(waiter.reply.scope.as_ref(), waiter.tab)
        {
            waiter.reply.error(error.code, error.message);
            return None;
        }
        let Some(tab) = state
            .tabs
            .get(&waiter.tab)
            .filter(|tab| !tab.rejected_creation)
        else {
            waiter
                .reply
                .error("TAB_NOT_FOUND", "No browser tab has that ID.");
            return None;
        };
        let stable = observed
            .as_ref()
            .ok()
            .filter(|observation| observation_matches(&state, &waiter.reply, observation).is_ok());
        let info = stable.map(|observation| tab_info(waiter.tab, tab, observation));
        let mut details = json!({"tab":waiter.tab,"state":info});
        if stable.is_none() {
            details["stateUnavailable"] = json!(true);
        }
        if tab.navigation.renderer_exit.is_some() && !tab.navigation.recovering {
            details["outcomeUnknown"] = json!(false);
            waiter.reply.error_details(
                "RENDERER_TERMINATED",
                "The renderer stopped. Reload the tab to recover.",
                details,
            );
            return None;
        }
        if tab.navigation.stopped || tab.navigation.incomplete {
            let code = if tab.navigation.stopped {
                "NAVIGATION_STOPPED"
            } else {
                "NAVIGATION_INCOMPLETE"
            };
            waiter.reply.error_details(
                code,
                "Navigation ended without reaching the requested page readiness.",
                details,
            );
            return None;
        }
        if tab.navigation.error.is_some() {
            waiter.reply.error_details(
                "NAVIGATION_FAILED",
                "The main-frame navigation failed.",
                details,
            );
            return None;
        }
        if waiter.started.elapsed() >= Duration::from_millis(waiter.timeout_ms) {
            details["until"] = json!(waiter.until);
            details["timeoutMs"] = json!(waiter.timeout_ms);
            details["elapsedMs"] = json!(waiter.started.elapsed().as_millis());
            details["navigationContinues"] = json!(true);
            waiter.reply.error_details(
                "WAIT_TIMEOUT",
                "The requested page readiness was not reached.",
                details,
            );
            return None;
        }
        let observation = stable?;
        let usable = tab.navigation.committed
            && tab.navigation.document_available
            && !tab.navigation.provisional;
        if usable && waiter.until == WaitUntil::Load && !observation.sample.loading {
            waiter.reply.ok(info.unwrap());
            return None;
        }
        if usable && waiter.until == WaitUntil::Dom && !waiter.probe_in_flight {
            let browser = observation.sample.browser.clone();
            let pending = Pending {
                frame: None,
                tab: waiter.tab,
                epoch: tab.navigation.epoch,
                evaluation: Evaluation::ReadyState {
                    token,
                    started: waiter.started,
                },
                reply: waiter.reply.clone(),
                step: Step::Frame("document.readyState".into()),
            };
            state.waits.get_mut(&token).unwrap().probe_in_flight = true;
            Some((browser, pending))
        } else {
            None
        }
    })
}

fn remove_matching_wait(token: u64, captured: &Waiter) -> Option<Waiter> {
    let removed = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if !state
            .waits
            .get(&token)
            .is_some_and(|waiter| same_waiter(waiter, captured))
        {
            return None;
        }
        state
            .pending
            .retain(|_, pending| !pending_matches_waiter(pending, captured));
        state.waits.remove(&token)
    });
    if removed.is_some() {
        notify_panel();
    }
    removed
}

fn wait_continues(token: u64, captured: &Waiter) -> bool {
    let current = STATE.with(|slot| {
        slot.borrow()
            .waits
            .get(&token)
            .is_some_and(|waiter| same_waiter(waiter, captured))
    });
    if !current {
        return false;
    }
    if captured.reply.active.load(Ordering::Acquire) {
        return true;
    }
    remove_matching_wait(token, captured);
    false
}

fn poll_wait_cycle_with(
    token: u64,
    observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
    dispatch_probe: impl FnOnce(&Browser, Pending),
    schedule: impl FnOnce(&Waiter) -> bool,
) {
    let Some(captured) = STATE.with(|slot| slot.borrow().waits.get(&token).cloned()) else {
        return;
    };
    let probe = poll_wait_with(token, observe);
    if !wait_continues(token, &captured) {
        return;
    }
    if let Some((browser, pending)) = probe {
        dispatch_probe(&browser, pending);
    }
    if !wait_continues(token, &captured) {
        return;
    }
    // A separate deadline tick can finish even if Chromium never answers.
    // Never schedule or remove a replacement waiter from this old callback.
    if !schedule(&captured) {
        if let Some(waiter) = remove_matching_wait(token, &captured) {
            waiter
                .reply
                .error("APP_STOPPING", "The browser cannot continue waiting.");
        }
    }
}

fn poll_wait(token: u64) {
    poll_wait_cycle_with(
        token,
        observe_tab,
        |browser, pending| dispatch(browser, "Page.getFrameTree", json!({}), pending),
        |waiter| {
            let mut task = WaitTask::new(
                token,
                waiter.tab,
                waiter.started,
                waiter.reply.active.clone(),
            );
            post_delayed_task(ThreadId::UI, Some(&mut task), 50) != 0
        },
    );
}

fn pending_matches_waiter(pending: &Pending, waiter: &Waiter) -> bool {
    matches!(pending.evaluation, Evaluation::ReadyState { token, started }
        if token == waiter.reply.token && started == waiter.started)
        && pending.tab == waiter.tab
        && Arc::ptr_eq(&pending.reply.active, &waiter.reply.active)
}

fn finish_dom_wait_with(
    browser: &Browser,
    pending: &Pending,
    token: u64,
    ready: &Json,
    mut observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
) {
    let captured = STATE.with(|slot| slot.borrow().waits.get(&token).cloned());
    let Some(captured) = captured.filter(|waiter| pending_matches_waiter(pending, waiter)) else {
        return;
    };
    let original_browser = STATE.with(|slot| {
        slot.borrow()
            .tabs
            .get(&captured.tab)
            .is_some_and(|tab| !tab.rejected_creation && same_browser(&tab.browser, &browser))
    });
    if !original_browser {
        pending_error(
            pending,
            "PAGE_CHANGED",
            "The wait callback belongs to an older browser.",
        );
        return;
    }
    let observed = observe(captured.tab, &captured.reply, ObservationAccess::Tab);
    let completed = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let waiter = state.waits.get(&token)?;
        if !same_waiter(waiter, &captured) || !waiter.reply.active.load(Ordering::Acquire) {
            return None;
        }
        let valid = observed.as_ref().ok().filter(|observation| {
            same_browser(&observation.sample.browser, &browser)
                && observation.sample.epoch == pending.epoch
                && observation_matches(&state, &waiter.reply, observation).is_ok()
                && waiter.started.elapsed() < Duration::from_millis(waiter.timeout_ms)
                && state.tabs.get(&waiter.tab).is_some_and(|tab| {
                    tab.navigation.error.is_none()
                        && !tab.navigation.stopped
                        && !tab.navigation.incomplete
                        && !tab.navigation.provisional
                        && tab.navigation.renderer_exit.is_none()
                        && tab.navigation.document_available
                })
        });
        let Some(observation) = valid else {
            // The absolute-deadline tick still owns this waiter. Release only
            // the completed probe; do not lose it on an unstable observation.
            state.waits.get_mut(&token).unwrap().probe_in_flight = false;
            return None;
        };
        let mut info = tab_info(waiter.tab, &state.tabs[&waiter.tab], observation);
        info["readyState"] = ready.clone();
        let waiter = state.waits.remove(&token).unwrap();
        state
            .pending
            .retain(|_, pending| !pending_matches_waiter(pending, &waiter));
        Some((waiter.reply, info))
    });
    if let Some((reply, info)) = completed {
        reply.ok(info);
        notify_panel();
    }
}

fn pending_error(pending: &Pending, code: &str, message: &str) {
    if matches!(pending.step, Step::FrameInventory { .. }) {
        stop_frame_tracking(pending.tab, Some(pending.reply.token));
    }
    if let Evaluation::ReadyState { token, .. } = pending.evaluation {
        STATE.with(|state| {
            if let Some(waiter) = state
                .borrow_mut()
                .waits
                .get_mut(&token)
                .filter(|waiter| pending_matches_waiter(pending, waiter))
            {
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
    let (pending, focus) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.frame_controls.remove(&tab);
        let focus = state.focus_leases.remove(&tab);
        state.captures.retain(|_, capture_tab| *capture_tab != tab);
        state.previews.retain(|_, request| request.tab != tab);
        let ids = state
            .pending
            .iter()
            .filter_map(|(id, request)| (request.tab == tab).then_some(*id))
            .collect::<Vec<_>>();
        let pending = ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        (pending, focus)
    });
    if let Some(lease) = focus {
        lease.reply.send_final(lease.failure(
            "CONTROL_DISCONNECTED",
            "The input control session disconnected; inspect before retrying.",
            "session-destroyed",
        ));
    }
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
    abort_focus_lease(
        tab,
        "RENDERER_TERMINATED",
        "The renderer stopped during input; inspect before retrying.",
        true,
    );
    stop_frame_tracking(tab, None);
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

wrap_task! {
    // OnBeforeClose answers a closed tab; an ID reused by another browser is
    // not the renderer that ended.
    struct RendererTerminatedTask { browser: Browser, reason: Json }
    impl Task {
        fn execute(&self) {
            let id = self.browser.identifier();
            if STATE.with(|state| state.borrow().tabs.get(&id).is_some_and(|tab| same_browser(&tab.browser, &self.browser))) {
                renderer_terminated(id, self.reason.clone());
            }
        }
    }
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
    let focus_tabs = STATE.with(|slot| {
        slot.borrow()
            .focus_leases
            .iter()
            .filter_map(|(tab, lease)| matches(&lease.reply, *tab).then_some(*tab))
            .collect::<Vec<_>>()
    });
    for tab in focus_tabs {
        abort_focus_lease(
            tab,
            "TASK_CONTROL_LOST",
            "Task control changed during input; inspect before continuing.",
            true,
        );
    }
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
        if matches!(request.step, Step::FrameInventory { .. }) {
            stop_frame_tracking(request.tab, Some(request.reply.token));
        }
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
    for &tab in &tabs {
        stop_frame_tracking(tab, None);
    }
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
            "resources":{"automationTabLimit":MAX_AUTOMATION_TABS,"taskTabLimit":crate::tasks::MAX_TASK_TABS,"pendingTabCreations":state.creations.len(),"captureLimit":1,"capturesInFlight":state.captures.len(),"focusLeases":state.focus_leases.len(),"focusCleanupFailed":state.focus_leases.values().filter(|lease| lease.phase == FocusPhase::Failed).count()}})
    })
}

fn handle(command: &Command, reply: Reply) {
    let observed = match command {
        Command::TaskRun { command, .. } => command.as_ref(),
        other => other,
    };
    // Reject raw protocol requests as early as CLI input, before native getters
    // or page preparation. Task-run uses the same bounded key grammar.
    if let Command::Press { key, .. } = observed
        && KeyPress::parse(key).is_none()
    {
        reply.error("UNSUPPORTED_KEY", UNSUPPORTED_KEY_MESSAGE);
        return;
    }
    let _refresh = PanelRefresh(!matches!(
        observed,
        Command::Status
            | Command::Spaces
            | Command::Tabs
            | Command::Read { .. }
            | Command::Frames { .. }
            | Command::ReadFrame { .. }
            | Command::ReadPage { .. }
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
        Command::Tabs => match tabs_info_with(&reply, observe_tab) {
            Ok(info) => reply.ok(info),
            Err(error) => observation_error(&reply, error),
        },
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
                let tab = browser.identifier();
                reply_created_tab_with(tab, &browser, &reply, observe_tab);
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
                | Command::Frames { tab }
                | Command::ReadFrame { tab, .. }
                | Command::ReadPage { tab }
                | Command::Click { tab, .. }
                | Command::Hover { tab, .. }
                | Command::Check { tab, .. }
                | Command::Fill { tab, .. }
                | Command::Insert { tab, .. }
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
            let focus_pending = STATE.with(|slot| slot.borrow().focus_leases.contains_key(&tab_id));
            if !matches!(
                other,
                Command::Close { .. } | Command::Stop { .. } | Command::Wait { .. }
            ) && focus_pending
            {
                reply.error("TAB_BUSY", "Temporary input focus is active or awaiting acknowledged cleanup. Inspect its result before continuing.");
                return;
            }
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
                let unavailable = match diagnostic_tab_info(tab_id, &browser, &reply, |tab| {
                    tab.navigation.renderer_exit.is_some()
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        observation_error(&reply, error);
                        return;
                    }
                };
                if let Some(info) = unavailable {
                    reply.error_details(
                        "RENDERER_TERMINATED",
                        "The renderer has not recovered. Reload, then wait for the new document.",
                        json!({"tab":tab_id,"state":info,"outcomeUnknown":false}),
                    );
                    return;
                }
            }
            let collecting_frames = STATE.with(|slot| {
                slot.borrow()
                    .frame_controls
                    .get(&tab_id)
                    .is_some_and(|control| control.active_token.is_some())
            });
            if (reply.scope.is_some()
                || matches!(
                    other,
                    Command::Frames { .. } | Command::ReadFrame { .. } | Command::ReadPage { .. }
                )
                || command_reference(other)
                    .is_some_and(|reference| reference.starts_with("frame/"))
                || collecting_frames)
                && !matches!(other, Command::Wait { .. } | Command::Screenshot { .. })
                && !(focus_pending && matches!(other, Command::Close { .. } | Command::Stop { .. }))
            {
                let busy = STATE.with(|state| {
                    collecting_frames
                        || state.borrow().pending.values().any(|pending| {
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
                Command::Frames { .. } => {
                    start_frame_inventory(&browser, tab_id, None, None, false, reply);
                }
                Command::ReadPage { .. } => {
                    start_frame_inventory(&browser, tab_id, None, None, true, reply);
                }
                Command::ReadFrame { frame, .. } => {
                    start_frame_inventory(
                        &browser,
                        tab_id,
                        Some(frame.clone()),
                        None,
                        false,
                        reply,
                    );
                }
                Command::Navigate { .. }
                | Command::Back { .. }
                | Command::Forward { .. }
                | Command::Reload { .. } => {
                    navigate(&browser, other, reply, ObservationAccess::Tab);
                }
                Command::Stop { .. } => {
                    match stop_loading(&browser, tab_id, || reply.active.load(Ordering::Acquire)) {
                        Ok(()) => reply.ok(json!({"tab":tab_id,"stopRequested":true})),
                        Err(error) => reply.error("PAGE_CHANGED", &error),
                    }
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
                            frame: None,
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
                    if command_reference(other)
                        .is_some_and(|reference| reference.starts_with("frame/"))
                    {
                        start_frame_action(&browser, tab_id, other, reply);
                        return;
                    }
                    match diagnostic_tab_info(tab_id, &browser, &reply, |tab| {
                        tab.navigation.provisional
                            || !tab.navigation.committed
                            || !tab.navigation.document_available
                    }) {
                        Ok(Some(info)) => {
                            reply.error_details(
                                "PAGE_LOADING",
                                "The current main document is not available yet.",
                                json!({"tab":tab_id,"state":info}),
                            );
                            return;
                        }
                        Err(error) => {
                            observation_error(&reply, error);
                            return;
                        }
                        Ok(None) => {}
                    }
                    let epoch = STATE.with(|slot| {
                        let state = slot.borrow();
                        let tab = state.tabs.get(&tab_id)?;
                        (same_browser(&tab.browser, &browser) && !tab.rejected_creation)
                            .then_some(tab.navigation.epoch)
                    });
                    let Some(epoch) = epoch else {
                        observation_error(&reply, OBSERVATION_CHANGED);
                        return;
                    };
                    let invocation = match other {
                        Command::Read { .. } => "window.__lightBrowserBridge.read()".to_owned(),
                        Command::Click { reference, .. } => {
                            action_expression("click", reference, None, epoch, &reply)
                        }
                        Command::Hover { reference, .. } => {
                            action_expression("hover", reference, None, epoch, &reply)
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
                        Command::Insert {
                            reference, text, ..
                        } => action_expression(
                            "insert",
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
                        Command::Press { reference, key, .. } => action_expression(
                            "press",
                            reference,
                            Some(("key", ActionValue::String(key))),
                            epoch,
                            &reply,
                        ),
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
                            frame: None,
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

fn execute_cli_stop(
    mut guard: NavigationControlGuard,
    still_current: impl Fn() -> bool,
    mut inspect: impl FnMut(&Browser) -> (bool, bool),
    mut cleanup: impl FnMut(&NavigationControlGuard) -> Result<(), String>,
    send: impl FnOnce(&Browser),
) -> Result<(), String> {
    navigation_control_inspect(&guard, &|| true, &still_current, &mut inspect)?;
    cleanup(&guard)?;
    let loading = navigation_control_inspect(&guard, &|| true, &still_current, &mut inspect)?;
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if !navigation_control_matches(&state, &guard) {
            return Err(NAVIGATION_CONTROL_CHANGED.to_owned());
        }
        let nav = &mut state.tabs.get_mut(&guard.tab).unwrap().navigation;
        if loading || nav.provisional || nav.intent_pending {
            guard.epoch = guard
                .epoch
                .checked_add(1)
                .ok_or(NAVIGATION_CONTROL_CHANGED)?;
            nav.epoch = guard.epoch;
            nav.stopped = true;
            nav.stop_pending = true;
        }
        nav.automatic_retry_error = None;
        nav.history.revoke();
        Ok(())
    })?;
    // CLI stop retains its accepted-request contract, including cleanup and
    // stop_load on an idle page. It does not use native chrome's no-op policy.
    send(&guard.browser);
    Ok(())
}

fn stop_loading(
    browser: &Browser,
    tab: i32,
    still_current: impl Fn() -> bool,
) -> Result<(), String> {
    let guard = STATE.with(|slot| navigation_control_guard(&slot.borrow(), tab, None))?;
    if !same_browser(&guard.browser, &browser) {
        return Err(NAVIGATION_CONTROL_CHANGED.into());
    }
    execute_cli_stop(
        guard,
        &still_current,
        |browser| (browser.is_valid() != 0, browser.is_loading() != 0),
        |guard| clear_navigation_controls(guard, &still_current),
        |browser| browser.stop_load(),
    )
}

fn navigation_request(command: &Command) -> Result<(&'static str, Option<&str>), TaskError> {
    match command {
        Command::Navigate { url, .. } if !valid_url(url) => Err(TaskError {
            code: "INVALID_URL",
            message: "Only an absolute HTTP(S) URL without control characters is accepted.",
        }),
        Command::Navigate { url, .. } => Ok(("navigate", Some(url.as_str()))),
        Command::Back { .. } => Ok(("back", None)),
        Command::Forward { .. } => Ok(("forward", None)),
        Command::Reload { .. } => Ok(("reload", None)),
        _ => unreachable!(),
    }
}

fn navigation_available(reply: &Reply, command: &Command, observed: &TabObservation) -> bool {
    let failure = match command {
        Command::Back { .. } if !observed.sample.back => {
            Some(("NO_HISTORY", "No previous history entry."))
        }
        Command::Forward { .. } if !observed.sample.forward => {
            Some(("NO_HISTORY", "No next history entry."))
        }
        Command::Navigate { .. } if observed.frame.is_none() => {
            Some(("FRAME_UNAVAILABLE", "Main frame is unavailable."))
        }
        _ => None,
    };
    if let Some((code, message)) = failure {
        reply.error(code, message);
        return false;
    }
    STATE.with(|slot| {
        let state = slot.borrow();
        if let Err(error) = observation_matches(&state, reply, observed) {
            observation_error(reply, error); return false;
        }
        let tab = &state.tabs[&observed.sample.tab];
        // History/reload remain recovery operations without a live main frame.
        let recovering = tab.navigation.renderer_exit.is_some() && !tab.navigation.recovering;
        if !recovering && (tab.navigation.provisional || tab.navigation.intent_pending || observed.sample.loading) {
            reply.error_details("NAVIGATION_BUSY",
                "The tab is still loading. Stop it or wait for load before navigating.",
                json!({"tab":observed.sample.tab,"state":tab_info(observed.sample.tab, tab, observed)}));
            return false;
        }
        true
    })
}

fn navigate_with(
    browser: &Browser,
    command: &Command,
    reply: &Reply,
    access: ObservationAccess,
    mut observe: impl FnMut(i32, &Reply, ObservationAccess) -> Result<TabObservation, TaskError>,
    cleanup: impl FnOnce(&TabObservation) -> Result<(), TaskError>,
    send: impl FnOnce(&TabObservation, &Command),
) {
    let (action, url) = match navigation_request(command) {
        Ok(request) => request,
        Err(error) => {
            observation_error(reply, error);
            return;
        }
    };
    let tab_id = command.tab_id().expect("navigation command has a tab");
    let first = match observe(tab_id, reply, access) {
        Ok(observed) if same_browser(&observed.sample.browser, &browser) => observed,
        Ok(_) => {
            observation_error(reply, OBSERVATION_CHANGED);
            return;
        }
        Err(error) => {
            observation_error(reply, error);
            return;
        }
    };
    if !navigation_available(reply, command, &first) {
        return;
    }
    if let Err(error) = cleanup(&first).and_then(|()| observation_current(reply, &first)) {
        observation_error(reply, error);
        return;
    }
    let observed = match observe(tab_id, reply, access) {
        Ok(observed) => observed,
        Err(error) => {
            observation_error(reply, error);
            return;
        }
    };
    // A native getter or cleanup can reenter. Never reserve an intent for the
    // replacement page/owner, nor redirect native navigation to another tab.
    if let Err(error) = observation_current(reply, &first) {
        observation_error(reply, error);
        return;
    }
    if !navigation_available(reply, command, &observed) {
        return;
    }
    let history_generation = crate::history_runtime::generation();
    let reserved = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        observation_matches(&state, reply, &first)?;
        observation_matches(&state, reply, &observed)?;
        let generation = history_generation.filter(|_| is_unowned_user_tab(&state, tab_id));
        let tab = state.tabs.get_mut(&tab_id).ok_or(OBSERVATION_CHANGED)?;
        if let Some(url) = url {
            tab.observe_start_page_url(url);
        }
        tab.navigation.begin(url);
        tab.navigation.history.begin(generation, true);
        Ok::<_, TaskError>(())
    });
    if let Err(error) = reserved {
        observation_error(reply, error);
        return;
    }
    // No native getters or UI callbacks between reservation and this send.
    send(&observed, command);
    reply.ok(json!({"tab":tab_id,"action":action,"navigationRequested":true}));
}

fn navigate(browser: &Browser, command: &Command, reply: Reply, access: ObservationAccess) {
    // Native browsing of a paused task remains available; only CLI requests
    // require a task scope. Both paths retain the captured owner during reads.
    navigate_with(
        browser,
        command,
        &reply,
        access,
        observe_tab,
        |observed| {
            observation_current(&reply, observed)?;
            revoke_download_permission_provenance(observed.sample.tab);
            crate::download_permission_runtime::cancel_tab(observed.sample.tab);
            observation_current(&reply, observed)?;
            invalidate_page_find(observed.sample.tab);
            observation_current(&reply, observed)
        },
        |observed, command| match command {
            Command::Navigate { url, .. } => observed
                .frame
                .as_ref()
                .unwrap()
                .load_url(Some(&CefString::from(url.as_str()))),
            Command::Back { .. } => browser.go_back(),
            Command::Forward { .. } => browser.go_forward(),
            Command::Reload { .. } => browser.reload(),
            _ => unreachable!(),
        },
    );
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

fn input_target_failure(
    state: &State,
    browser: &Browser,
    tab: i32,
    epoch: u64,
    reply: &Reply,
    allow_navigation: bool,
) -> Option<InputFailure> {
    if !reply.active.load(Ordering::Acquire) {
        return Some(InputFailure {
            code: "COMMAND_CANCELED",
            message: "The input request is no longer active.",
        });
    }
    if let Err(error) = state.tasks.check_tab(reply.scope.as_ref(), tab) {
        return Some(InputFailure {
            code: error.code,
            message: error.message,
        });
    }
    let Some(tab) = state.tabs.get(&tab) else {
        return Some(InputFailure {
            code: "TAB_CLOSED",
            message: "The tab closed during input preparation.",
        });
    };
    if tab.rejected_creation || !same_browser(&tab.browser, &browser) {
        return Some(InputFailure {
            code: "TAB_CLOSED",
            message: "The original browser is no longer available.",
        });
    }
    if tab.navigation.renderer_exit.is_some() {
        return Some(InputFailure {
            code: "RENDERER_TERMINATED",
            message: "The renderer stopped during input. Inspect before retrying.",
        });
    }
    if !allow_navigation
        && (tab.navigation.epoch != epoch
            || tab.navigation.provisional
            || !tab.navigation.committed
            || !tab.navigation.document_available)
    {
        return Some(InputFailure {
            code: "PAGE_CHANGED",
            message: "The document changed before input finished dispatching. Read again; do not automatically repeat the action.",
        });
    }
    None
}

fn fill_binding_failure(
    state: &State,
    browser: &Browser,
    tab: i32,
    target: &FillTarget,
) -> Option<InputFailure> {
    let binding = target.binding.borrow();
    let current = binding.as_ref().is_some_and(|binding| {
        same_browser(&binding.browser, &browser)
            && state
                .tabs
                .get(&tab)
                .is_some_and(|tab| tab.navigation.document_epoch == binding.document_epoch)
            && link_owner(&state.tasks, tab) == binding.owner
    });
    (!current).then_some(InputFailure { code: "PAGE_CHANGED", message: "The original fill target changed. Read again; do not repeat the input automatically." })
}

fn pending_input_failure(
    state: &State,
    browser: &Browser,
    pending: &Pending,
) -> Option<InputFailure> {
    pending
        .frame
        .as_ref()
        .and_then(|frame| {
            frame_snapshot_failure(
                state,
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                frame,
            )
        })
        .or_else(|| {
            input_target_failure(
                state,
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                false,
            )
        })
        .or_else(|| {
            pending
                .fill_target()
                .and_then(|target| fill_binding_failure(state, browser, pending.tab, target))
        })
}

fn bind_fill_target(browser: &Browser, pending: &Pending) -> Option<InputFailure> {
    let target = pending.fill_target()?;
    STATE.with(|slot| {
        let state = slot.borrow();
        if let Some(failure) = input_target_failure(
            &state,
            browser,
            pending.tab,
            pending.epoch,
            &pending.reply,
            false,
        ) {
            return Some(failure);
        }
        if target.binding.borrow().is_none() {
            *target.binding.borrow_mut() = Some(FillBinding {
                browser: browser.clone(),
                document_epoch: state.tabs[&pending.tab].navigation.document_epoch,
                owner: link_owner(&state.tasks, pending.tab),
            });
        }
        fill_binding_failure(&state, browser, pending.tab, target)
    })
}

fn batch_input_failure(
    state: &State,
    browser: &Browser,
    batch: &InputBatch,
    allow_navigation: bool,
) -> Option<InputFailure> {
    batch
        .frame
        .as_ref()
        .and_then(|frame| {
            frame_snapshot_failure(state, browser, batch.tab, batch.epoch, &batch.reply, frame)
        })
        .or_else(|| {
            input_target_failure(
                state,
                browser,
                batch.tab,
                batch.epoch,
                &batch.reply,
                allow_navigation,
            )
        })
        .or_else(|| match &batch.kind {
            NativeInput::Fill(target) => fill_binding_failure(state, browser, batch.tab, target),
            _ => None,
        })
}

fn input_registered(state: &State, batch: &Shared<InputBatch>) -> bool {
    batch.ids.iter().take(batch.kind.count()).all(|id| state.pending.get(id).is_some_and(|pending| {
        matches!(&pending.step, Step::Input { batch: current, .. } if Shared::ptr_eq(current, batch))
    }))
}

fn remove_input_batch(batch: &Shared<InputBatch>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        for id in batch.ids.iter().take(batch.kind.count()) {
            if state.pending.get(id).is_some_and(|pending| {
                matches!(&pending.step, Step::Input { batch: current, .. } if Shared::ptr_eq(current, batch))
            }) {
                state.pending.remove(id);
            }
        }
    });
}

// True advances intermediate input to DOM verification. Keep its pending
// entry until that continuation replaces it, so callers cannot observe a gap.
fn finish_input_batch(batch: &Shared<InputBatch>) -> bool {
    let (enqueuing, failure, acknowledged, attempted) = {
        let progress = batch.progress.borrow();
        (
            progress.enqueuing,
            progress.failure,
            progress.accepted[..batch.kind.count()]
                .iter()
                .all(|value| *value == Some(true))
                && progress.acknowledged[..batch.kind.count()]
                    .iter()
                    .all(|value| *value),
            batch.kind.prior_attempted() || progress.attempted.iter().any(|value| *value),
        )
    };
    if enqueuing {
        return false;
    }
    let current = STATE.with(|state| {
        let state = state.borrow();
        input_registered(&state, batch).then(|| {
            batch_input_failure(
                &state,
                &batch.browser,
                batch,
                batch.kind.allows_navigation(),
            )
        })
    });
    let Some(current_failure) = current else {
        return false;
    };
    let failure = failure.or(current_failure);
    if failure.is_none() && !acknowledged {
        return false;
    }
    if failure.is_none() && batch.kind.verifies_after_input() {
        // Enter may have navigated successfully. Its old document observation
        // is then unavailable, not proof that the original target received it.
        let can_observe = !matches!(batch.kind, NativeInput::Key(_))
            || STATE.with(|slot| {
                slot.borrow().tabs.get(&batch.tab).is_some_and(|tab| {
                    tab.navigation.epoch == batch.epoch
                        && !tab.navigation.provisional
                        && tab.navigation.committed
                        && tab.navigation.document_available
                })
            });
        if can_observe {
            return true;
        }
    }
    remove_input_batch(batch);
    if let Some(failure) = failure {
        batch.reply.error_details(
            failure.code,
            failure.message,
            json!({"tab":batch.tab,"outcomeUnknown":true,"attempted":attempted}),
        );
    } else {
        batch
            .reply
            .ok(json!({"tab":batch.tab,"page":batch.kind.success_page()}));
    }
    false
}

// CEF documents asynchronous results. Keep both entries even if a transport
// re-enters its observer before returning; a callback alone cannot acknowledge
// dispatch. These adjacent UI submissions are not an atomic renderer operation.
fn enqueue_input_batch(
    batch: &Shared<InputBatch>,
    mut native_available: impl FnMut() -> bool,
    mut submit: impl FnMut(usize, i32) -> i32,
) -> bool {
    for part in 0..batch.kind.count() {
        if !STATE.with(|state| input_registered(&state.borrow(), batch)) {
            return false;
        }
        if batch.progress.borrow().failure.is_some() {
            break;
        }
        let available = native_available();
        // Native getters can re-enter cancellation, handoff, navigation or close.
        let checked = STATE.with(|state| {
            let state = state.borrow();
            input_registered(&state, batch)
                .then(|| batch_input_failure(&state, &batch.browser, batch, false).or_else(|| {
                    (!focus_input_active(&state, &batch.browser, batch.tab, &batch.reply,
                        batch.frame.as_ref().and_then(|frame| frame.session_id())))
                        .then_some(InputFailure { code:"FOCUS_PREPARATION_FAILED", message:"Temporary input focus is no longer active; no remaining input is dispatched." })
                }))
        });
        let Some(failure) = checked else {
            return false;
        };
        if batch.progress.borrow().failure.is_some() {
            break;
        }
        let failure = failure.or_else(|| {
            (!available).then_some(InputFailure {
                code: "TAB_CLOSED",
                message: "The browser is unavailable during input dispatch.",
            })
        });
        if let Some(failure) = failure {
            batch.progress.borrow_mut().failure = Some(failure);
            break;
        }
        batch.progress.borrow_mut().attempted[part] = true;
        match &batch.kind {
            NativeInput::MouseMove(target) => target.attempted.set(true),
            NativeInput::Fill(target) => target.attempted.set(true),
            NativeInput::Key(target) => target.attempted.set(true),
            _ => {}
        }
        let receipt = submit(part, batch.ids[part]);
        if !STATE.with(|state| input_registered(&state.borrow(), batch)) {
            return false;
        }
        let mut progress = batch.progress.borrow_mut();
        progress.accepted[part] = Some(receipt == batch.ids[part]);
        if receipt != batch.ids[part] {
            progress.failure = Some(InputFailure {
                code: "CONTROL_UNAVAILABLE",
                message: "Chromium rejected an input event. Inspect before retrying.",
            });
            break;
        }
    }
    batch.progress.borrow_mut().enqueuing = false;
    finish_input_batch(batch)
}

fn input_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) -> bool {
    let part = STATE.with(|state| {
        let state = state.borrow();
        match &state.pending.get(&id)?.step {
            Step::Input { batch, part } => Some((batch.clone(), *part)),
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
        batch_input_failure(&state, browser, &batch, batch.kind.allows_navigation())
    });
    {
        let mut progress = batch.progress.borrow_mut();
        if progress.acknowledged[part] {
            return true;
        }
        if !progress.attempted[part] || !valid_json {
            progress.failure = Some(InputFailure {
                code: "INVALID_RESULT",
                message: "Chromium returned an invalid input acknowledgement. Inspect before retrying.",
            });
        } else if !success {
            progress.failure = Some(InputFailure {
                code: "CONTROL_ERROR",
                message: "Chromium could not dispatch input. Inspect before retrying.",
            });
        } else if let Some(failure) = current {
            progress.failure = Some(failure);
        }
        progress.acknowledged[part] = true;
    }
    if finish_input_batch(&batch) {
        dispatch_input_verification(&batch);
    }
    true
}

fn reserve_input_batch(
    id: i32,
    pending: &Pending,
    browser: &Browser,
    kind: NativeInput,
) -> Result<Shared<InputBatch>, InputFailure> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(failure) = pending_input_failure(&state, browser, pending) {
            return Err(failure);
        }
        if !state.pending.get(&id).is_some_and(|current| {
            current.reply.token == pending.reply.token
                && Arc::ptr_eq(&current.reply.active, &pending.reply.active)
                && current.input_preparation()
        }) {
            return Err(InputFailure {
                code: "COMMAND_CANCELED",
                message: "The input preparation is no longer current.",
            });
        }
        let count = kind.count();
        let Some(last) = state.next_message.checked_add(count as i32) else {
            return Err(InputFailure {
                code: "CONTROL_UNAVAILABLE",
                message: "The control message identifiers are exhausted.",
            });
        };
        let batch = Shared::new(InputBatch {
            ids: if count == 1 {
                [last, 0]
            } else {
                [last - 1, last]
            },
            kind,
            tab: pending.tab,
            epoch: pending.epoch,
            frame: pending.frame.clone(),
            browser: browser.clone(),
            reply: pending.reply.clone(),
            progress: RefCell::new(InputProgress {
                enqueuing: true,
                attempted: [false; 2],
                accepted: [None; 2],
                acknowledged: [false; 2],
                failure: None,
            }),
        });
        state.next_message = last;
        state.pending.remove(&id);
        for (part, id) in batch.ids.iter().take(count).enumerate() {
            state.pending.insert(
                *id,
                Pending {
                    step: Step::Input {
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

// Focus emulation is a temporary renderer/session setting, not native window
// activation and not an atomic binding between a DOM ref and an Input command.
// Its owner is deliberately separate from Pending: cancellation must still
// disable the original session after the action's pending entries disappear.
fn focus_preparation(pending: &Pending) -> bool {
    let initial = matches!(pending.step, Step::Frame(_))
        || (pending.frame.is_some() && matches!(pending.step, Step::Evaluate));
    match &pending.evaluation {
        // A page loaded while hidden drops pointer down/up until it paints.
        // Focus emulation keeps it visible across the whole pointer action.
        Evaluation::Page {
            evidence: Some(ActionEvidence::Press(_) | ActionEvidence::Click(_)),
            ..
        } => initial,
        Evaluation::Page {
            evidence: Some(ActionEvidence::Fill(target)),
            ..
        } => {
            if target.insert {
                initial
            } else {
                matches!(pending.step, Step::Evaluate)
                    && target
                        .classification
                        .borrow()
                        .as_ref()
                        .is_some_and(|(input, _)| input == "chromium")
            }
        }
        _ => false,
    }
}

fn fill_dispatch_params(pending: &Pending, inspect: bool) -> Option<Json> {
    let target = pending.fill_target()?;
    let root_prefix = format!("{}/", pending.epoch);
    let prefix = pending
        .frame
        .as_ref()
        .map_or(root_prefix.as_str(), |frame| frame.ref_prefix());
    let reference = target
        .reference
        .strip_prefix(prefix)
        .filter(|reference| !reference.is_empty())?;
    let mut request = json!({"action":"fill","ref":reference,"text":target.text});
    if inspect {
        request["inspectOnly"] = json!(true);
    } else {
        let classification = target.classification.borrow();
        let (input, kind) = classification.as_ref()?;
        request["expectedInput"] = json!(input);
        request["expectedKind"] = json!(kind);
    }
    if let Some(frame) = &pending.frame {
        Some(
            json!({"expression":format!("window.__lightBrowserBridge.frameAct({request},{})", frame.binding()),
            "uniqueContextId":frame.unique_context_id(),"returnByValue":true,"awaitPromise":false,"timeout":5000}),
        )
    } else {
        Some(
            json!({"expression":format!("(()=>{{{DOM_BRIDGE}\nreturn window.__lightBrowserBridge.act({request});}})()"),
            "contextId":target.context.get()?,"returnByValue":true,"awaitPromise":false,"timeout":5000}),
        )
    }
}

fn start_focus_lease(browser: &Browser, method: &str, params: Json, mut pending: Pending) {
    let tab = pending.tab;
    let token = pending.reply.token;
    pending.reply.focus_completion = Some(tab);
    let session = pending.session().map(str::to_owned);
    let reserved = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.focus_leases.contains_key(&tab) || state.focus_leases.len() >= MAX_FOCUS_LEASES {
            return Err((
                "TAB_BUSY",
                "Temporary input focus is still in use or awaiting cleanup.",
            ));
        }
        if let Some(failure) = pending_input_failure(&state, browser, &pending) {
            return Err((failure.code, failure.message));
        }
        let id = state.next_control_message().ok_or((
            "CONTROL_UNAVAILABLE",
            "The control message identifiers are exhausted.",
        ))?;
        let evidence = match &pending.evaluation {
            Evaluation::Page { evidence, .. } => evidence.clone(),
            _ => None,
        };
        state.focus_leases.insert(
            tab,
            FocusLease {
                browser: browser.clone(),
                session,
                token,
                reply: pending.reply.clone(),
                evidence,
                attempted: false,
                phase: FocusPhase::Enabling,
                enable: FocusMessage::new(id),
                disable: None,
                disable_after_enable: false,
                initial: Some(FocusContinuation::Dispatch {
                    method: method.to_owned(),
                    params,
                    pending: pending.clone(),
                }),
                after: None,
                stop_frames: false,
                started: Instant::now(),
            },
        );
        Ok(())
    });
    if let Err((code, message)) = reserved {
        pending
            .reply
            .send_after_focus(Response::error(&pending.reply.id, code, message));
        return;
    }
    send_focus_message(tab, token, true);
    schedule_focus_watchdog();
}

fn focus_input_active(
    state: &State,
    browser: &Browser,
    tab: i32,
    reply: &Reply,
    session: Option<&str>,
) -> bool {
    reply.focus_completion.is_none()
        || state.focus_leases.get(&tab).is_some_and(|lease| {
            lease.token == reply.token
                && lease.phase == FocusPhase::Active
                && same_browser(&lease.browser, &browser)
                && lease.session.as_deref() == session
        })
}

fn schedule_focus_watchdog() {
    let schedule = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.focus_watchdog_queued
            || !state
                .focus_leases
                .values()
                .any(|lease| lease.phase != FocusPhase::Failed)
        {
            return false;
        }
        state.focus_watchdog_queued = true;
        true
    });
    if !schedule {
        return;
    }
    let mut task = FocusWatchdogTask::new();
    if post_delayed_task(ThreadId::UI, Some(&mut task), 1000) == 0 {
        let tabs = STATE.with(|slot| {
            let mut state = slot.borrow_mut();
            state.focus_watchdog_queued = false;
            state.focus_leases.keys().copied().collect::<Vec<_>>()
        });
        for tab in tabs {
            abort_focus_lease(
                tab,
                "CONTROL_UNAVAILABLE",
                "Input cleanup monitoring is unavailable; no input is retried.",
                true,
            );
        }
    }
}

wrap_task! {
    struct FocusWatchdogTask;
    impl Task {
        fn execute(&self) {
            let tabs = STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                state.focus_watchdog_queued = false;
                state.focus_leases.iter().filter_map(|(tab, lease)|
                    (lease.phase != FocusPhase::Failed && lease.started.elapsed() >= ACTION_TIMEOUT).then_some(*tab)).collect::<Vec<_>>()
            });
            for tab in tabs { abort_focus_lease(tab, "TIMEOUT", "The input operation exceeded its lifetime; do not repeat it automatically.", true); }
            schedule_focus_watchdog();
        }
    }
}

fn focus_navigation_changed(tab: i32) {
    let preserve_key_ack = STATE.with(|slot| {
        let state = slot.borrow();
        state
            .focus_leases
            .get(&tab)
            .is_some_and(|lease| root_key_dispatched(lease) || pointer_press_dispatched(&state, lease))
    });
    // Root keys and dispatched pointer presses can navigate as their intended
    // effect. Their ACK path still finishes the lease, or cancellation/watchdog
    // does so if no ACK comes.
    if !preserve_key_ack {
        abort_focus_lease(
            tab,
            "PAGE_CHANGED",
            "The document changed during input; inspect before retrying.",
            true,
        );
    }
}

/// Root keys were sent and the lease is still active or disabling before its
/// key verification. Their verification already accepts a navigated document.
fn root_key_dispatched(lease: &FocusLease) -> bool {
    lease.session.is_none()
        && matches!(&lease.evidence, Some(ActionEvidence::Press(target)) if target.attempted.get())
        && match lease.phase {
            FocusPhase::Active => true,
            FocusPhase::Disabling => matches!(&lease.after, Some(FocusContinuation::Verify(_))),
            _ => false,
        }
}

/// Press/release is in flight, or acknowledged with only the disable pending.
fn pointer_press_dispatched(state: &State, lease: &FocusLease) -> bool {
    matches!(&lease.evidence, Some(ActionEvidence::Click(_)))
        && match lease.phase {
            FocusPhase::Active => state.pending.values().any(|pending| {
                pending.reply.token == lease.token
                    && matches!(&pending.step, Step::Input { batch, .. }
                        if matches!(batch.kind, NativeInput::Click(_)))
            }),
            FocusPhase::Disabling => matches!(&lease.after,
                Some(FocusContinuation::Respond { response, .. }) if response.ok),
            _ => false,
        }
}

fn focus_message_current(
    state: &State,
    tab: i32,
    token: u64,
    id: i32,
    enabled: bool,
    browser: &Browser,
) -> bool {
    state
        .tabs
        .get(&tab)
        .is_some_and(|page| same_browser(&page.browser, &browser))
        && state.focus_leases.get(&tab).is_some_and(|lease| {
            lease.token == token
                && same_browser(&lease.browser, &browser)
                && if enabled {
                    lease.phase == FocusPhase::Enabling && lease.enable.id == id
                } else {
                    lease.phase == FocusPhase::Disabling
                        && lease
                            .disable
                            .as_ref()
                            .is_some_and(|message| message.id == id)
                }
        })
}

fn send_focus_message(tab: i32, token: u64, enabled: bool) {
    let captured = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let lease = state
            .focus_leases
            .get_mut(&tab)
            .filter(|lease| lease.token == token)?;
        if enabled && lease.phase != FocusPhase::Enabling {
            return None;
        }
        if !enabled
            && (lease.phase != FocusPhase::Disabling
                || (lease.enable.started && lease.enable.sending))
        {
            return None;
        }
        if !enabled {
            lease.disable_after_enable = !lease.enable.started || lease.enable.finished().is_some();
        }
        let message = if enabled {
            &mut lease.enable
        } else {
            lease.disable.as_mut()?
        };
        if message.started {
            return None;
        }
        message.started = true;
        let id = message.id;
        Some((lease.browser.clone(), lease.session.clone(), id))
    });
    let Some((browser, session, id)) = captured else {
        return;
    };
    let host = browser.host();
    // No epoch/task/active-Reply guard here: disable must also reach a surviving
    // session after navigation, cancellation or a task handoff.
    let current =
        STATE.with(|slot| focus_message_current(&slot.borrow(), tab, token, id, enabled, &browser));
    let accepted = current
        && host.is_some_and(|host| {
            send_session_command(
                &host,
                id,
                "Emulation.setFocusEmulationEnabled",
                &json!({"enabled":enabled}),
                session.as_deref(),
            ) == id
        });
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if let Some(lease) = state
            .focus_leases
            .get_mut(&tab)
            .filter(|lease| lease.token == token)
        {
            let message = if enabled {
                Some(&mut lease.enable)
            } else {
                lease.disable.as_mut()
            };
            if let Some(message) = message.filter(|message| message.id == id) {
                message.sending = false;
                message.accepted = Some(accepted);
            }
        }
    });
    advance_focus_lease(tab, token);
    // Cancellation while enable's native sender was on the stack defers the
    // disable send until enable's acceptance is recorded above, so the lease
    // never sees disable complete before the earlier enable was accepted.
    if enabled {
        send_focus_message(tab, token, false);
    }
    let waiting = STATE.with(|slot| {
        let state = slot.borrow();
        state.focus_leases.get(&tab).is_some_and(|lease| {
            lease.token == token
                && if enabled {
                    lease.phase == FocusPhase::Enabling
                } else {
                    lease.phase == FocusPhase::Disabling
                }
        })
    });
    if waiting {
        let mut task = FocusTimeoutTask::new(tab, token, id, enabled);
        if post_delayed_task(ThreadId::UI, Some(&mut task), FOCUS_ACK_TIMEOUT_MS) == 0 {
            focus_timeout(tab, token, id, enabled);
        }
    }
}

fn begin_focus_cleanup(
    tab: i32,
    token: u64,
    after: Option<FocusContinuation>,
    stop_frames: bool,
) -> bool {
    let mut retired = Vec::with_capacity(2);
    let reserved = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let Some(lease) = state
            .focus_leases
            .get(&tab)
            .filter(|lease| lease.token == token)
        else {
            return None;
        };
        let allocate = !matches!(lease.phase, FocusPhase::Disabling | FocusPhase::Failed);
        let id = allocate.then(|| state.next_control_message()).flatten();
        let lease = state.focus_leases.get_mut(&tab).unwrap();
        lease.stop_frames |= stop_frames;
        if let Some(initial) = lease.initial.take() {
            retired.push(initial);
        }
        if !matches!(lease.phase, FocusPhase::Failed) {
            if let Some(after) = after {
                let error =
                    matches!(&after, FocusContinuation::Respond { response, .. } if !response.ok);
                if lease.after.is_none() || error {
                    if let Some(previous) = lease.after.replace(after) {
                        retired.push(previous);
                    }
                } else {
                    retired.push(after);
                }
            }
        } else if let Some(after) = after {
            retired.push(after);
        }
        if !allocate {
            return Some(None);
        }
        lease.phase = FocusPhase::Disabling;
        lease.disable = id.map(FocusMessage::new);
        Some(Some(id))
    });
    drop(retired);
    match reserved {
        None => false,
        Some(None) => true,
        Some(Some(Some(_))) => {
            send_focus_message(tab, token, false);
            true
        }
        Some(Some(None)) => {
            fail_focus_cleanup(tab, token);
            true
        }
    }
}

fn defer_focus_reply(reply: &Reply, response: Response) -> bool {
    let Some(tab) = reply.focus_completion else {
        return false;
    };
    begin_focus_cleanup(
        tab,
        reply.token,
        Some(FocusContinuation::Respond {
            reply: reply.clone(),
            response,
        }),
        false,
    )
}

fn defer_focus_verification(batch: &Shared<InputBatch>) -> bool {
    let Some(tab) = batch.reply.focus_completion else {
        return false;
    };
    begin_focus_cleanup(
        tab,
        batch.reply.token,
        Some(FocusContinuation::Verify(batch.clone())),
        false,
    )
}

fn abort_focus_lease(tab: i32, code: &str, message: &str, stop_frames: bool) {
    let captured = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let lease = state.focus_leases.get(&tab)?;
        let token = lease.token;
        let after = FocusContinuation::Respond {
            reply: lease.reply.clone(),
            response: lease.failure(code, message, "pending"),
        };
        let ids = state
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.reply.token == token).then_some(*id))
            .collect::<Vec<_>>();
        let retired = ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        Some((token, after, retired))
    });
    if let Some((token, after, retired)) = captured {
        drop(retired);
        begin_focus_cleanup(tab, token, Some(after), stop_frames);
    }
}

fn fail_focus_cleanup(tab: i32, token: u64) {
    let failure = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let lease = state.focus_leases.get_mut(&tab).filter(|lease| lease.token == token)?;
        if lease.phase == FocusPhase::Failed { return None; }
        lease.phase = FocusPhase::Failed;
        lease.attempted = lease.attempted();
        let evidence = lease.evidence.take();
        let initial = lease.initial.take();
        let after = lease.after.take();
        lease.stop_frames = true;
        let result = (lease.reply.clone(), lease.failure("FOCUS_CLEANUP_FAILED",
            "Temporary input focus cleanup was not acknowledged. Do not repeat input; close this tab to reset its control session.", "failed"));
        let ids = state.pending.iter().filter_map(|(id, pending)| (pending.reply.token == token).then_some(*id)).collect::<Vec<_>>();
        let retired = ids.into_iter().filter_map(|id| state.pending.remove(&id)).collect::<Vec<_>>();
        let frame_payload = state.frame_controls.get_mut(&tab)
            .filter(|control| control.active_token == Some(token))
            .map(|control| {
                let inventory = control.active_inventory.take();
                if let Some(inventory) = &inventory {
                    inventory.invalidated.set(true);
                    // Retain only the remote cleanup obligation, not the
                    // FrameAction's input text while this tab is quarantined.
                    let read = inventory.read.borrow();
                    if let Some(parent) = read.as_ref().and_then(|read| read.parents.get(read.parent_index)) {
                        if parent.frame.session_id.is_none() {
                            control.deferred_object_group = parent.object_group.clone();
                        }
                    }
                }
                let snapshots = std::mem::take(&mut control.snapshots);
                for snapshot in &snapshots { snapshot.invalidated.set(true); }
                (inventory, snapshots)
            });
        Some((result, evidence, initial, after, retired, frame_payload))
    });
    if let Some(((reply, response), evidence, initial, after, retired, frame_payload)) = failure {
        drop((evidence, initial, after, retired, frame_payload));
        // Frame detachment is intentionally held while the session's setting is
        // uncertain. A late successful disable ACK can release this quarantine.
        reply.send_final(response);
        notify_panel();
    }
}

fn focus_session_detached(browser: &Browser, tab: i32, removed: &[String]) {
    let retired = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let lease = state.focus_leases.get(&tab)?;
        if !same_browser(&lease.browser, &browser)
            || !lease
                .session
                .as_ref()
                .is_some_and(|session| removed.contains(session))
        {
            return None;
        }
        let lease = state.focus_leases.remove(&tab).unwrap();
        let ids = state
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.reply.token == lease.token).then_some(*id))
            .collect::<Vec<_>>();
        let pending = ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        Some((lease, pending))
    });
    if let Some((lease, pending)) = retired {
        let response = lease.failure(
            "FRAME_CHANGED",
            "The controlled frame session ended during input. Inspect before retrying.",
            "session-destroyed",
        );
        let reply = lease.reply.clone();
        drop((lease, pending));
        // Exact parent/child detachment transfers cleanup to engine disposal.
        // This is an error, not a disable ACK or proof of renderer reset timing.
        reply.send_final(response);
    }
}

fn run_focus_continuation(continuation: FocusContinuation) {
    match continuation {
        FocusContinuation::Dispatch {
            method,
            params,
            pending,
        } => {
            let browser = STATE.with(|slot| {
                slot.borrow()
                    .focus_leases
                    .get(&pending.tab)
                    .filter(|lease| {
                        lease.token == pending.reply.token && lease.phase == FocusPhase::Active
                    })
                    .map(|lease| lease.browser.clone())
            });
            if let Some(browser) = browser {
                dispatch(&browser, &method, params, pending);
            }
        }
        FocusContinuation::Verify(batch) => dispatch_input_verification(&batch),
        FocusContinuation::Respond { reply, response } => reply.send_after_focus(response),
    }
}

fn advance_focus_lease(tab: i32, token: u64) {
    enum Next {
        Resume(FocusContinuation),
        Reject,
        Cleaned(FocusLease),
        DisableAgain,
        Fail,
    }
    let next = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let lease = state
            .focus_leases
            .get_mut(&tab)
            .filter(|lease| lease.token == token)?;
        match lease.phase {
            FocusPhase::Enabling => match lease.enable.finished()? {
                true if lease.reply.active.load(Ordering::Acquire) => {
                    lease.phase = FocusPhase::Active;
                    lease.initial.take().map(Next::Resume)
                }
                _ => Some(Next::Reject),
            },
            FocusPhase::Disabling | FocusPhase::Failed => {
                if lease.enable.started && lease.enable.sending {
                    return None;
                }
                let succeeded = lease.disable.as_ref()?.finished()?;
                if !succeeded {
                    return (lease.phase != FocusPhase::Failed).then_some(Next::Fail);
                }
                if lease.enable.started && lease.enable.finished().is_none() {
                    return None;
                }
                if !lease.disable_after_enable {
                    // An early cancellation can send disable before enable's
                    // ACK. Once enable settles, one final disable establishes
                    // ordering instead of assuming cross-domain reply order.
                    let id = state.next_control_message();
                    let lease = state.focus_leases.get_mut(&tab).unwrap();
                    lease.phase = FocusPhase::Disabling;
                    lease.disable = id.map(FocusMessage::new);
                    return Some(if id.is_some() {
                        Next::DisableAgain
                    } else {
                        Next::Fail
                    });
                }
                let lease = state.focus_leases.remove(&tab).unwrap();
                Some(Next::Cleaned(lease))
            }
            FocusPhase::Active => None,
        }
    });
    match next {
        Some(Next::Resume(continuation)) => run_focus_continuation(continuation),
        Some(Next::Reject) => abort_focus_lease(
            tab,
            "FOCUS_PREPARATION_FAILED",
            "Chromium did not acknowledge temporary input focus preparation. No input is retried.",
            false,
        ),
        Some(Next::Fail) => fail_focus_cleanup(tab, token),
        Some(Next::DisableAgain) => send_focus_message(tab, token, false),
        Some(Next::Cleaned(mut lease)) => {
            if lease.stop_frames {
                stop_frame_tracking(tab, Some(token));
            }
            if let Some(after) = lease.after.take() {
                run_focus_continuation(after);
            }
            notify_panel();
        }
        None => {}
    }
}

fn complete_focus_message(browser: &Browser, id: i32, success: bool, bytes: &[u8]) -> bool {
    let completed = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let (tab, lease) = state.focus_leases.iter_mut().find(|(_, lease)| {
            same_browser(&lease.browser, &browser)
                && (lease.enable.id == id
                    || lease
                        .disable
                        .as_ref()
                        .is_some_and(|message| message.id == id))
        })?;
        let message = if lease.enable.id == id {
            &mut lease.enable
        } else {
            lease.disable.as_mut().unwrap()
        };
        if message.acknowledged.is_none() {
            message.acknowledged = Some(
                success
                    && bytes.len() <= 4096
                    && serde_json::from_slice::<Json>(bytes).is_ok_and(|value| value.is_object()),
            );
        }
        Some((*tab, lease.token))
    });
    let Some((tab, token)) = completed else {
        return false;
    };
    advance_focus_lease(tab, token);
    true
}

fn focus_timeout(tab: i32, token: u64, id: i32, enabled: bool) {
    let current = STATE.with(|slot| {
        slot.borrow().focus_leases.get(&tab).is_some_and(|lease| {
            lease.token == token
                && if enabled {
                    lease.phase == FocusPhase::Enabling && lease.enable.id == id
                } else {
                    lease.phase == FocusPhase::Disabling
                        && lease
                            .disable
                            .as_ref()
                            .is_some_and(|message| message.id == id)
                }
        })
    });
    if !current {
        return;
    }
    if enabled {
        abort_focus_lease(
            tab,
            "FOCUS_PREPARATION_TIMEOUT",
            "Temporary input focus preparation timed out; no input is retried.",
            false,
        );
    } else {
        fail_focus_cleanup(tab, token);
    }
}

wrap_task! {
    struct FocusTimeoutTask { tab: i32, token: u64, id: i32, enabled: bool }
    impl Task {
        fn execute(&self) { focus_timeout(self.tab, self.token, self.id, self.enabled); }
    }
}

fn dispatch_input(browser: &Browser, id: i32, pending: &Pending, kind: NativeInput) {
    let host = browser.host();
    let count = kind.count();
    let params: [Option<Json>; 2] =
        std::array::from_fn(|part| (part < count).then(|| kind.params(part)));
    // Native code zero prevents native browser key fallback. This dispatch code
    // does not call native focus or selected-space methods.
    let available = browser.is_valid() != 0 && browser.has_document() != 0;
    if !STATE.with(|slot| {
        focus_input_active(
            &slot.borrow(),
            browser,
            pending.tab,
            &pending.reply,
            pending.session(),
        )
    }) {
        finish_input_preparation(
            id,
            pending,
            "FOCUS_PREPARATION_FAILED",
            "Temporary input focus is no longer active; no input was dispatched.",
        );
        return;
    }
    if let Some(failure) = STATE.with(|slot| {
        pending.frame.as_ref().and_then(|frame| {
            frame_snapshot_failure(
                &slot.borrow(),
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                frame,
            )
        })
    }) {
        finish_input_preparation(id, pending, failure.code, failure.message);
        return;
    }
    if host.is_none() || !available {
        finish_input_preparation(
            id,
            pending,
            "CONTROL_UNAVAILABLE",
            "Chromium is unavailable for input.",
        );
        return;
    }
    let batch = match reserve_input_batch(id, pending, browser, kind) {
        Ok(batch) => batch,
        Err(failure) => {
            finish_input_preparation(id, pending, failure.code, failure.message);
            return;
        }
    };
    let host = host.unwrap();
    if enqueue_input_batch(
        &batch,
        || browser.is_valid() != 0 && browser.has_document() != 0,
        |part, message| {
            send_session_command(
                &host,
                message,
                batch.kind.method(),
                params[part].as_ref().unwrap(),
                if batch.kind.is_pointer() {
                    None
                } else {
                    batch.frame.as_ref().and_then(|frame| frame.session_id())
                },
            )
        },
    ) {
        dispatch_input_verification(&batch);
    }
}

fn input_verification_params(batch: &InputBatch) -> Option<Json> {
    let (reference, context, mut request) = match &batch.kind {
        NativeInput::MouseMove(target) => {
            let point = if batch.frame.is_some() {
                target.local_point.get()?
            } else {
                target.point.get()?
            };
            (
                &target.reference,
                target.context.get(),
                json!({"action":target.action(),"point":{"x":point.x,"y":point.y},"verifyOnly":true}),
            )
        }
        NativeInput::Fill(target) => {
            let mut request = json!({"action":target.action(),"text":target.text,
                "kind":target.kind.get()?,"verifyOnly":true});
            if target.insert || target.kind.get() != Some("contenteditable") {
                request["observation"] = json!(target.observation.borrow().as_ref()?);
            }
            (&target.reference, target.context.get(), request)
        }
        NativeInput::Key(target) => (
            &target.reference,
            target.context.get(),
            json!({"action":"press","key":target.key.canonical(),
                "observation":target.observation.borrow().as_ref()?,"verifyOnly":true}),
        ),
        _ => return None,
    };
    let root_prefix = format!("{}/", batch.epoch);
    let prefix = batch
        .frame
        .as_ref()
        .map_or(root_prefix.as_str(), |frame| frame.ref_prefix());
    request["ref"] = json!(
        reference
            .strip_prefix(prefix)
            .filter(|value| !value.is_empty())?
    );
    if let Some(frame) = &batch.frame {
        let binding = frame.binding();
        Some(
            json!({"expression":format!("window.__lightBrowserBridge.frameAct({request},{binding})"),
            "uniqueContextId":frame.unique_context_id(),"returnByValue":true,"awaitPromise":false,"timeout":5000}),
        )
    } else {
        Some(
            json!({"expression":format!("window.__lightBrowserBridge.act({request})"),"contextId":context?,
            "returnByValue":true,"awaitPromise":false,"timeout":5000}),
        )
    }
}

fn reserve_input_verification(
    batch: &Shared<InputBatch>,
) -> Result<Option<(i32, Pending)>, InputFailure> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !input_registered(&state, batch) {
            return Ok(None);
        }
        if let Some(failure) = batch_input_failure(&state, &batch.browser, batch, false) {
            return Err(failure);
        }
        let Some(id) = state.next_message.checked_add(1) else {
            return Err(InputFailure {
                code: "CONTROL_UNAVAILABLE",
                message: "The control message identifiers are exhausted.",
            });
        };
        let pending = Pending {
            step: match &batch.kind {
                NativeInput::Fill(_) => Step::FillVerify,
                NativeInput::Key(_) => Step::PressVerify,
                NativeInput::MouseMove(_) => Step::ClickVerify,
                _ => unreachable!("terminal input has no verification"),
            },
            ..state.pending[&batch.ids[0]].clone()
        };
        state.next_message = id;
        for previous in batch.ids.iter().take(batch.kind.count()) {
            state.pending.remove(previous);
        }
        state.pending.insert(id, pending.clone());
        Ok(Some((id, pending)))
    })
}

fn dispatch_input_verification(batch: &Shared<InputBatch>) {
    // Pointer verification precedes press/release, which still need the lease;
    // the final pointer reply releases it.
    if !matches!(batch.kind, NativeInput::MouseMove(_)) && defer_focus_verification(batch) {
        return;
    }
    let params = input_verification_params(batch);
    let host = batch.browser.host();
    let available = batch.browser.is_valid() != 0 && batch.browser.has_document() != 0;
    if let Some(failure) = STATE.with(|slot| {
        batch.frame.as_ref().and_then(|frame| {
            frame_snapshot_failure(
                &slot.borrow(),
                &batch.browser,
                batch.tab,
                batch.epoch,
                &batch.reply,
                frame,
            )
        })
    }) {
        batch.progress.borrow_mut().failure = Some(failure);
        finish_input_batch(batch);
        return;
    }
    if batch.frame.is_none()
        && matches!(batch.kind, NativeInput::Key(_))
        && STATE.with(|slot| {
            let state = slot.borrow();
            batch_input_failure(&state, &batch.browser, batch, true).is_none()
                && batch_input_failure(&state, &batch.browser, batch, false).is_some()
        })
    {
        finish_input_batch(batch);
        return;
    }
    if host.is_none() || params.is_none() || !available {
        batch.progress.borrow_mut().failure = Some(InputFailure {
            code: "CONTROL_UNAVAILABLE",
            message: "Chromium is unavailable for input verification. Inspect before retrying.",
        });
        finish_input_batch(batch);
        return;
    }
    // Native getters above can re-enter. Keep the input pending until this
    // checked replacement; no fresh read, ref or document context is used.
    let (id, _) = match reserve_input_verification(batch) {
        Ok(Some(reserved)) => reserved,
        Ok(None) => return,
        Err(failure) => {
            if batch.frame.is_none()
                && matches!(batch.kind, NativeInput::Key(_))
                && failure.code == "PAGE_CHANGED"
            {
                finish_input_batch(batch);
                return;
            }
            batch.progress.borrow_mut().failure = Some(failure);
            finish_input_batch(batch);
            return;
        }
    };
    if send_session_command(
        &host.unwrap(),
        id,
        "Runtime.evaluate",
        &params.unwrap(),
        batch.frame.as_ref().and_then(|frame| frame.session_id()),
    ) != id
    {
        if let Some(pending) = complete_control_request(id) {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "Chromium rejected input verification. Inspect before retrying.",
            );
        }
    }
}

fn take_input_preparation(id: i32, pending: &Pending) -> bool {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.pending.get(&id).is_some_and(|current| {
            current.reply.token == pending.reply.token
                && Arc::ptr_eq(&current.reply.active, &pending.reply.active)
                && current.input_preparation()
        }) {
            state.pending.remove(&id);
            true
        } else {
            false
        }
    })
}

fn finish_input_preparation(id: i32, pending: &Pending, code: &str, message: &str) {
    if !take_input_preparation(id, pending) {
        return;
    }
    // Preparation may focus, select or scroll. Evidence from an engine input
    // survives rejection of its later DOM verification.
    pending
        .reply
        .error_details(code, message, pending.outcome_details());
}

fn fill_error(value: &Json) -> (&'static str, &'static str) {
    // Do not forward exception text, input values or arbitrary page-provided
    // messages from either native verification or the legacy DOM path.
    match value["error"].as_str() {
        Some("STALE_REF") => (
            "STALE_REF",
            "Read the page again to get a current element ref.",
        ),
        Some("ELEMENT_UNAVAILABLE") => (
            "ELEMENT_UNAVAILABLE",
            "The fill target changed or became unavailable; read again.",
        ),
        Some("READ_ONLY") => ("READ_ONLY", "The fill target is read-only."),
        Some("INVALID_TEXT") => ("INVALID_TEXT", "Text must be a string."),
        Some("UNSUPPORTED_ELEMENT") => {
            ("UNSUPPORTED_ELEMENT", "This element does not support fill.")
        }
        Some("FOCUS_NOT_REACHED") => (
            "FOCUS_NOT_REACHED",
            "The target did not retain focus; no text was inserted.",
        ),
        Some("SELECTION_NOT_REACHED") => (
            "SELECTION_NOT_REACHED",
            "The target did not retain the full selection; no text was inserted.",
        ),
        Some("INPUT_CANCELED") => (
            "INPUT_CANCELED",
            "The page canceled input; inspect the page before retrying.",
        ),
        Some("FILL_STATE_UNCONFIRMED") => (
            "FILL_STATE_UNCONFIRMED",
            "Input was attempted, but the original target could not be verified. Read again before retrying.",
        ),
        Some("FILL_STATE_NOT_REACHED") => (
            "FILL_STATE_NOT_REACHED",
            "Input was attempted, but the final value differs from the requested text. Read again before retrying.",
        ),
        Some("INSERT_STATE_UNCONFIRMED") => (
            "INSERT_STATE_UNCONFIRMED",
            "Text insertion was attempted, but its original target or input observation could not be confirmed. Read again before retrying.",
        ),
        Some("INSERT_STATE_NOT_REACHED") => (
            "INSERT_STATE_NOT_REACHED",
            "Text insertion was attempted, but the original control does not contain the expected result. Read again before retrying.",
        ),
        Some("INPUT_TARGET_MISMATCH") => (
            "INPUT_TARGET_MISMATCH",
            "A matching text input reached a different target. Inspect the page; do not automatically repeat the input.",
        ),
        _ => (
            "ACTION_FAILED",
            "The fill action did not complete; read again before retrying.",
        ),
    }
}

fn finish_legacy_fill(id: i32, pending: &Pending, value: &Json) {
    if !take_input_preparation(id, pending) {
        return;
    }
    if value["ok"] == false {
        let (code, message) = fill_error(value);
        let mut details = serde_json::Map::new();
        if let Evaluation::Page {
            evidence: Some(evidence),
            ..
        } = &pending.evaluation
        {
            for &key in evidence.fields(value) {
                if let Some(value) = value[key].as_bool() {
                    details.insert(key.into(), Json::Bool(value));
                }
            }
        }
        if details.is_empty() {
            pending.reply.error(code, message);
        } else {
            pending
                .reply
                .error_details(code, message, Json::Object(details));
        }
    } else {
        let mut page = json!({"ok":true,"action":"fill","trusted":false,
            "detail":"Value set and input/change events dispatched; the native value matched the requested text at the final synchronous check."});
        if let Some(changed) = value["changed"].as_bool() {
            page["changed"] = json!(changed);
            page["detail"] = json!(if changed {
                "Plain text replaced the editing host contents and synthetic beforeinput/input were dispatched. Formatting and native editor undo are not preserved."
            } else {
                "The editing host already contains the requested plain text; no input events were dispatched."
            });
        }
        pending.reply.ok(json!({"tab":pending.tab,"page":page}));
    }
}

fn fill_preparation_result(
    browser: &Browser,
    id: i32,
    pending: &Pending,
    success: bool,
    bytes: &[u8],
) {
    let failure = STATE.with(|slot| pending_input_failure(&slot.borrow(), browser, pending));
    if let Some(failure) = failure {
        finish_input_preparation(id, pending, failure.code, failure.message);
        return;
    }
    let parsed = (bytes.len() <= 64 * 1024)
        .then(|| serde_json::from_slice::<Json>(bytes).ok())
        .flatten();
    let Some(result) = parsed else {
        finish_input_preparation(
            id,
            pending,
            "INVALID_RESULT",
            "Chromium returned an invalid text input result.",
        );
        return;
    };
    if !success || result.get("exceptionDetails").is_some() {
        finish_input_preparation(
            id,
            pending,
            "PAGE_SCRIPT_ERROR",
            "Text input preparation or verification failed. Read again before retrying.",
        );
        return;
    }
    let value = &result["result"]["value"];
    let target = pending
        .fill_target()
        .expect("fill handler has a fill target");
    let verify = matches!(pending.step, Step::FillVerify);
    if verify {
        let native = value["action"] == target.action()
            && value["input"] == "chromium"
            && value["attempted"] == true
            && value.get("prepared").is_none()
            && value.get("trusted").is_none()
            && target.attempted.get()
            && (pending.frame.is_some() || target.context.get().is_some())
            && target.kind.get().is_some();
        if target.insert {
            let observed = value["observation"].as_str() == target.observation.borrow().as_deref()
                && target.observation.borrow().is_some();
            let verification = if target.kind.get() == Some("contenteditable") {
                "input-event"
            } else {
                "value"
            };
            if native
                && observed
                && value["ok"] == true
                && value["verification"] == verification
                && value.get("verified").is_none()
            {
                if take_input_preparation(id, pending) {
                    let detail = if verification == "value" {
                        "Chromium acknowledged insertion and the original control matched the expected value. Read again to verify later effects."
                    } else {
                        "Chromium acknowledged insertion and a matching trusted input event was observed on the original editing host. This does not verify the complete document, formatting or editor model; read again."
                    };
                    pending.reply.ok(json!({"tab":pending.tab,"page":{
                        "ok":true,"action":"insert","input":"chromium","attempted":true,
                        "dispatchAcknowledged":true,"verification":verification,"detail":detail
                    }}));
                }
            } else if native && observed && value["ok"] == false {
                let (code, message) = fill_error(value);
                finish_input_preparation(id, pending, code, message);
            } else {
                finish_input_preparation(
                    id,
                    pending,
                    "INVALID_PAGE_RESULT",
                    "The page did not confirm the original insertion result. Read again.",
                );
            }
            return;
        }
        let observed_fill = target.kind.get() == Some("contenteditable")
            || (target.observation.borrow().is_some()
                && matches!(
                    value["inputTarget"].as_str(),
                    Some("matched" | "unconfirmed")
                ));
        if native && observed_fill && value["ok"] == true && value["verified"] == true {
            if take_input_preparation(id, pending) {
                let mut page = json!({"ok":true,"action":"fill","input":"chromium",
                    "attempted":true,"dispatchAcknowledged":true,"verified":true,
                    "detail":"Chromium acknowledged text insertion and the original target matched the requested text; read again to verify later effects."});
                if target.kind.get() != Some("contenteditable") {
                    page["inputTarget"] = value["inputTarget"].clone();
                }
                pending.reply.ok(json!({"tab":pending.tab,"page":page}));
            }
        } else if native && value["ok"] == false && value.get("verified").is_none() {
            let (code, message) = fill_error(value);
            finish_input_preparation(id, pending, code, message);
        } else {
            finish_input_preparation(
                id,
                pending,
                "INVALID_PAGE_RESULT",
                "The page did not confirm the original fill result. Read again.",
            );
        }
        return;
    }
    if value.get("prepared").is_some() {
        if let Some(kind) = prepared_input_kind(pending, value) {
            dispatch_input(browser, id, pending, kind);
        } else {
            finish_input_preparation(
                id,
                pending,
                "INVALID_PAGE_RESULT",
                "The page did not confirm text input preparation. Read again.",
            );
        }
        return;
    }
    if value["input"] == "chromium"
        && value["ok"] == false
        && value["action"] == target.action()
        && value["attempted"] == false
        && value.get("trusted").is_none()
        && value.get("verified").is_none()
    {
        target.native.set(true);
        let (code, message) = fill_error(value);
        finish_input_preparation(id, pending, code, message);
    } else if !target.insert
        && value.get("input").is_none()
        && value.get("verified").is_none()
        && value["action"] == "fill"
        && (value["ok"] == false || (value["ok"] == true && value["trusted"] == false))
    {
        finish_legacy_fill(id, pending, value);
    } else {
        finish_input_preparation(
            id,
            pending,
            "INVALID_PAGE_RESULT",
            "The page did not return a supported fill result. Read again.",
        );
    }
}

fn press_verification_result(
    browser: &Browser,
    id: i32,
    pending: &Pending,
    success: bool,
    bytes: &[u8],
) {
    // The engine already acknowledged both events. Navigation can invalidate
    // this observation, but never relax cancellation, ownership or Browser checks.
    let failure = STATE.with(|slot| {
        let state = slot.borrow();
        if let Some(failure) = pending.frame.as_ref().and_then(|frame| {
            frame_snapshot_failure(
                &state,
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                frame,
            )
        }) {
            return Some(failure);
        }
        input_target_failure(
            &state,
            browser,
            pending.tab,
            pending.epoch,
            &pending.reply,
            true,
        )
    });
    if let Some(failure) = failure {
        finish_input_preparation(id, pending, failure.code, failure.message);
        return;
    }
    let current_failure =
        STATE.with(|slot| pending_input_failure(&slot.borrow(), browser, pending));
    if let Some(failure) = current_failure {
        if pending.frame.is_some() {
            finish_input_preparation(id, pending, failure.code, failure.message);
            return;
        }
        if take_input_preparation(id, pending) {
            let target = pending
                .press_target()
                .expect("key verification retains its target");
            pending.reply.ok(
                json!({"tab":pending.tab,"page":NativeInput::Key(target.clone()).success_page()}),
            );
        }
        return;
    }
    let parsed = (bytes.len() <= 64 * 1024)
        .then(|| serde_json::from_slice::<Json>(bytes).ok())
        .flatten();
    let Some(result) = parsed else {
        finish_input_preparation(
            id,
            pending,
            "INVALID_RESULT",
            "The key target observation was invalid. Inspect before retrying.",
        );
        return;
    };
    let target = pending
        .press_target()
        .expect("key verification retains its target");
    let value = &result["result"]["value"];
    let valid = success
        && result.get("exceptionDetails").is_none()
        && value["ok"] == true
        && value["action"] == "press"
        && value["input"] == "chromium"
        && value["attempted"] == true
        && value["key"] == target.key.canonical()
        && target.attempted.get()
        && value["observation"].as_str() == target.observation.borrow().as_deref()
        && matches!(
            value["keydownTarget"].as_str(),
            Some("matched" | "mismatched" | "unconfirmed")
        );
    if !valid {
        finish_input_preparation(
            id,
            pending,
            "INVALID_PAGE_RESULT",
            "The original key target observation could not be verified. Inspect before retrying.",
        );
        return;
    }
    if value["keydownTarget"] == "mismatched" {
        if take_input_preparation(id, pending) {
            pending.reply.error_details("KEY_TARGET_MISMATCH",
                "A matching keydown reached a different element. The input may already have changed the page; inspect it and do not automatically retry.",
                json!({"tab":pending.tab,"attempted":true,"outcomeUnknown":true,
                    "dispatchAcknowledged":true,"keydownTarget":"mismatched"}));
        }
        return;
    }
    if take_input_preparation(id, pending) {
        pending.reply.ok(json!({"tab":pending.tab,"page":{
            "ok":true,"action":"press","key":target.key.canonical(),"input":"chromium",
            "attempted":true,"dispatchAcknowledged":true,"keydownTarget":value["keydownTarget"],
            "detail":"The first matching trusted keydown target was observed when available. This cannot distinguish simultaneous human input or guarantee subsequent page effects. Read again before the next action."
        }}));
    }
}

fn prepared_input_kind(pending: &Pending, value: &Json) -> Option<NativeInput> {
    let action = if let Some(target) = pending.fill_target() {
        target.action()
    } else if let Some(target) = pending.click_target() {
        target.action()
    } else {
        "press"
    };
    if value["ok"] != true
        || value["action"] != action
        || value["prepared"] != true
        || value["attempted"] != false
    {
        return None;
    }
    if let Some(target) = pending.fill_target() {
        let kind = native_fill_kind(&value["kind"])?;
        if !matches!(pending.step, Step::Evaluate)
            || value["input"] != "chromium"
            || value.get("trusted").is_some()
            || value.get("verified").is_some()
            || (pending.frame.is_none() && target.context.get().is_none())
            || target.kind.get().is_some()
            || target.attempted.get()
        {
            return None;
        }
        if target.insert || kind != "contenteditable" {
            let observation = value["observation"].as_str()?;
            if observation.len() != 32
                || !observation.bytes().all(|byte| byte.is_ascii_hexdigit())
                || target.observation.borrow().is_some()
            {
                return None;
            }
            *target.observation.borrow_mut() = Some(observation.to_owned());
        }
        target.kind.set(Some(kind));
        target.native.set(true);
        return Some(NativeInput::Fill(target.clone()));
    }
    if let Some(target) = pending.click_target() {
        let point = ClickPoint::from_json(&value["point"])?;
        if matches!(pending.step, Step::ClickVerify) {
            (!target.hover && target.attempted.get() && target.point.get() == Some(point))
                .then_some(NativeInput::Click(point))
        } else if matches!(pending.step, Step::Evaluate)
            && !target.attempted.get()
            && (pending.frame.is_some() || target.context.get().is_some())
            && target.point.get().is_none_or(|existing| existing == point)
        {
            target.point.set(Some(point));
            Some(NativeInput::MouseMove(target.clone()))
        } else {
            None
        }
    } else if let Some(target) = pending.press_target() {
        let observation = value["observation"].as_str()?;
        if !matches!(pending.step, Step::Evaluate)
            || value["key"] != target.key.canonical()
            || value.get("trusted").is_some()
            || (pending.frame.is_none() && target.context.get().is_none())
            || observation.len() != 32
            || !observation.bytes().all(|byte| byte.is_ascii_hexdigit())
            || target.observation.borrow().is_some()
            || target.attempted.get()
        {
            return None;
        }
        *target.observation.borrow_mut() = Some(observation.to_owned());
        Some(NativeInput::Key(target.clone()))
    } else {
        None
    }
}

fn hover_verified(pending: &Pending, value: &Json) -> bool {
    pending.click_target().is_some_and(|target| {
        target.hover
            && matches!(pending.step, Step::ClickVerify)
            && target.attempted.get()
            && (pending.frame.is_some() || target.context.get().is_some())
            && target.point.get().is_some()
            && ClickPoint::from_json(&value["point"]) == target.point.get()
            && value["ok"] == true
            && value["action"] == target.action()
            && value["prepared"] == true
            && value["attempted"] == false
    })
}

fn input_preparation_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) -> bool {
    let pending = STATE.with(|state| {
        state
            .borrow()
            .pending
            .get(&id)
            .filter(|pending| pending.input_preparation())
            .cloned()
    });
    let Some(pending) = pending else {
        return false;
    };
    if pending.fill_target().is_some() {
        fill_preparation_result(browser, id, &pending, success, bytes);
        return true;
    }
    if matches!(pending.step, Step::PressVerify) {
        press_verification_result(browser, id, &pending, success, bytes);
        return true;
    }
    let action = if let Some(target) = pending.click_target() {
        target.action()
    } else {
        "press"
    };
    let failure = STATE.with(|state| pending_input_failure(&state.borrow(), browser, &pending));
    if let Some(failure) = failure {
        finish_input_preparation(id, &pending, failure.code, failure.message);
        return true;
    }
    let result = (bytes.len() <= 64 * 1024)
        .then(|| serde_json::from_slice::<Json>(bytes).ok())
        .flatten();
    let Some(result) = result else {
        finish_input_preparation(
            id,
            &pending,
            "INVALID_RESULT",
            "Chromium returned an invalid input preparation.",
        );
        return true;
    };
    let value = &result["result"]["value"];
    if !success || result.get("exceptionDetails").is_some() {
        finish_input_preparation(
            id,
            &pending,
            "PAGE_SCRIPT_ERROR",
            "Input preparation failed. Read again before repeating it.",
        );
    } else if value["ok"] == false && value["action"] == action {
        finish_input_preparation(
            id,
            &pending,
            value["error"].as_str().unwrap_or("ACTION_FAILED"),
            value["detail"]
                .as_str()
                .unwrap_or("Input preparation did not complete."),
        );
    } else if pending.frame.is_some() && pending.click_target().is_some() {
        frame_pointer_input_ready(browser, id, &pending, value);
    } else if hover_verified(&pending, value) {
        // Hover is terminal after one mouseMoved and its original-point check.
        // Never advance it to the click press/release pair.
        if take_input_preparation(id, &pending) {
            pending.reply.ok(json!({"tab":pending.tab,"page":{
                "ok":true,"action":"hover","input":"chromium","attempted":true,
                "dispatchAcknowledged":true,"pointVerified":true,
                "detail":"Chromium acknowledged mouseMoved and the original point still reached the reference. Read again to verify hover effects."
            }}));
        }
    } else if let Some(kind) = prepared_input_kind(&pending, value) {
        dispatch_input(browser, id, &pending, kind);
    } else {
        finish_input_preparation(
            id,
            &pending,
            "INVALID_PAGE_RESULT",
            "The page did not confirm input preparation. Read again.",
        );
    }
    true
}

struct ReadinessDispatchGuard {
    waiter: Waiter,
    browser: Browser,
    epoch: u64,
    document_epoch: u64,
    owner: Option<(TaskScope, Status)>,
}

fn readiness_dispatch_current(
    state: &State,
    pending: &Pending,
    guard: &ReadinessDispatchGuard,
) -> bool {
    let token = guard.waiter.reply.token;
    pending_matches_waiter(pending, &guard.waiter)
        && state.waits.get(&token).is_some_and(|waiter| {
            same_waiter(waiter, &guard.waiter)
                && waiter.probe_in_flight
                && waiter.started.elapsed() < Duration::from_millis(waiter.timeout_ms)
        })
        && pending.reply.active.load(Ordering::Acquire)
        && guard.waiter.started.elapsed() < Duration::from_millis(guard.waiter.timeout_ms)
        && state
            .tasks
            .check_tab(pending.reply.scope.as_ref(), pending.tab)
            .is_ok()
        && link_owner(&state.tasks, pending.tab) == guard.owner
        && state.tabs.get(&pending.tab).is_some_and(|tab| {
            !tab.rejected_creation
                && same_browser(&tab.browser, &guard.browser)
                && tab.navigation.epoch == pending.epoch
                && tab.navigation.epoch == guard.epoch
                && tab.navigation.document_epoch == guard.document_epoch
                && !tab.navigation.provisional
                && tab.navigation.document_available
                && tab.navigation.renderer_exit.is_none()
                && tab.navigation.error.is_none()
                && !tab.navigation.stopped
                && !tab.navigation.incomplete
        })
}

fn dispatch_readiness_with<H, P>(
    browser: &Browser,
    pending: Pending,
    host: impl FnOnce() -> Option<H>,
    params: impl FnOnce() -> Option<P>,
    send: impl FnOnce(H, P, i32) -> i32,
) {
    let guard = STATE.with(|slot| {
        let state = slot.borrow();
        let Evaluation::ReadyState { token, .. } = pending.evaluation else {
            return None;
        };
        let waiter = state.waits.get(&token)?.clone();
        let tab = state.tabs.get(&pending.tab)?;
        let guard = ReadinessDispatchGuard {
            waiter,
            browser: browser.clone(),
            epoch: pending.epoch,
            document_epoch: tab.navigation.document_epoch,
            owner: link_owner(&state.tasks, pending.tab),
        };
        readiness_dispatch_current(&state, &pending, &guard).then_some(guard)
    });
    let Some(guard) = guard else {
        pending_error(
            &pending,
            "PAGE_CHANGED",
            "The wait probe is no longer current.",
        );
        return;
    };
    let host = host();
    if !STATE.with(|slot| readiness_dispatch_current(&slot.borrow(), &pending, &guard)) {
        pending_error(
            &pending,
            "PAGE_CHANGED",
            "The page changed while preparing the wait probe.",
        );
        return;
    }
    let Some(host) = host else {
        pending_error(&pending, "TAB_CLOSED", "Browser host is unavailable.");
        return;
    };
    let params = params();
    let reserved = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        if params.is_none() || !readiness_dispatch_current(&state, &pending, &guard) {
            return Err((
                "PAGE_CHANGED",
                "The page changed while preparing the wait probe.",
            ));
        }
        let id = state.next_control_message().ok_or((
            "CONTROL_UNAVAILABLE",
            "The control message identifiers are exhausted.",
        ))?;
        state.pending.insert(id, pending.clone());
        Ok(id)
    });
    let id = match reserved {
        Ok(id) => id,
        Err((code, message)) => {
            pending_error(&pending, code, message);
            return;
        }
    };
    // All CEF getters and parameter preparation are complete. A stale probe
    // releases only its own waiter; the existing deadline tick remains active.
    if send(host, params.unwrap(), id) != id {
        if let Some(pending) = complete_control_request(id) {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "Chromium rejected the wait probe.",
            );
        }
    }
}

fn command_reference(command: &Command) -> Option<&str> {
    match command {
        Command::Click { reference, .. }
        | Command::Hover { reference, .. }
        | Command::Check { reference, .. }
        | Command::Fill { reference, .. }
        | Command::Insert { reference, .. }
        | Command::Select { reference, .. }
        | Command::Press { reference, .. } => Some(reference),
        _ => None,
    }
}

fn frame_action_arguments(command: &Command, snapshot: &FrameSnapshot) -> Option<Json> {
    let reference = command_reference(command)?;
    if !snapshot.references.contains(reference) {
        return None;
    }
    let reference = reference.strip_prefix(snapshot.ref_prefix())?;
    Some(match command {
        Command::Click { .. } => json!({"action":"click","ref":reference}),
        Command::Hover { .. } => json!({"action":"hover","ref":reference}),
        Command::Fill { text, .. } => json!({"action":"fill","ref":reference,"text":text}),
        Command::Insert { text, .. } => json!({"action":"insert","ref":reference,"text":text}),
        Command::Press { key, .. } => json!({"action":"press","ref":reference,"key":key}),
        Command::Check { checked, .. } => {
            json!({"action":"check","ref":reference,"checked":checked})
        }
        Command::Select { value, .. } => json!({"action":"select","ref":reference,"value":value}),
        _ => return None,
    })
}

fn start_frame_action(browser: &Browser, tab: i32, command: &Command, reply: Reply) {
    let snapshot = STATE.with(|slot| {
        slot.borrow().frame_controls.get(&tab).and_then(|control| {
            control
                .snapshots
                .iter()
                .find(|snapshot| {
                    command_reference(command)
                        .is_some_and(|reference| snapshot.references.contains(reference))
                })
                .cloned()
        })
    });
    let Some(snapshot) = snapshot.filter(|snapshot| {
        !snapshot.invalidated.get()
            && same_browser(&snapshot.browser, &browser)
            && command_reference(command)
                .is_some_and(|reference| snapshot.references.contains(reference))
    }) else {
        reply.error(
            "STALE_FRAME_SNAPSHOT",
            "Read the frame again to obtain a current reference.",
        );
        return;
    };
    start_frame_inventory(
        browser,
        tab,
        Some(snapshot.frame.frame_id.clone()),
        Some(FrameAction {
            command: command.clone(),
            snapshot,
        }),
        false,
        reply,
    );
}

fn frame_snapshot_failure(
    state: &State,
    browser: &Browser,
    tab: i32,
    epoch: u64,
    reply: &Reply,
    snapshot: &Shared<FrameSnapshot>,
) -> Option<InputFailure> {
    let current = !snapshot.invalidated.get()
        && same_browser(&snapshot.browser, &browser)
        && snapshot.epoch == epoch
        && snapshot.owner == link_owner(&state.tasks, tab)
        && state.tasks.check_tab(reply.scope.as_ref(), tab).is_ok()
        && reply.active.load(Ordering::Acquire)
        && state.tabs.get(&tab).is_some_and(|page| {
            !page.rejected_creation
                && same_browser(&page.browser, &browser)
                && page.navigation.epoch == epoch
                && page.navigation.document_epoch == snapshot.document_epoch
                && page.navigation.document_available
                && !page.navigation.provisional
                && page.navigation.renderer_exit.is_none()
        })
        && state.frame_controls.get(&tab).is_some_and(|control| {
            control.enabled
                && !control.cleanup_failed
                && control.cleanup.is_none()
                && control.runtime_cleanup.is_none()
                && control.object_cleanup.is_none()
                && control.active_token == Some(reply.token)
                && control
                    .snapshots
                    .iter()
                    .any(|stored| Shared::ptr_eq(stored, snapshot))
                && snapshot
                    .session_id()
                    .is_none_or(|session| control.sessions.contains(session))
                && control.active_inventory.as_ref().is_some_and(|inventory| {
                    !inventory.invalidated.get()
                        && inventory
                            .action
                            .as_ref()
                            .is_some_and(|action| Shared::ptr_eq(&action.snapshot, snapshot))
                        && inventory.read.borrow().as_ref().is_some_and(|read| {
                            read.frame.frame_id == snapshot.frame.frame_id
                                && read.frame.session_id == snapshot.frame.session_id
                                && read
                                    .parents
                                    .iter()
                                    .all(|parent| !parent.context.invalidated())
                                && read.context.context().is_some_and(|context| {
                                    context.unique_id == snapshot.unique_context
                                })
                        })
                })
        });
    (!current).then_some(InputFailure { code:"FRAME_CHANGED", message:"The original frame, document, context or control ownership changed. Read again; do not repeat input automatically." })
}

fn finish_frame_action_reply(tab: i32, reply: &Reply, response: Response) {
    if !reply.active.load(Ordering::Acquire) {
        return;
    }
    let inventory = STATE.with(|slot| {
        let state = slot.borrow();
        state
            .frame_controls
            .get(&tab)
            .filter(|control| control.active_token == Some(reply.token))
            .and_then(|control| control.active_inventory.clone())
    });
    let Some(inventory) = inventory else {
        // Cancellation/disconnection already revoked the route. Preserve an
        // actual error, but never turn a late successful action into success.
        reply.send_final(if response.ok {
            Response::error(
                &reply.id,
                "FRAME_CHANGED",
                "The frame changed before action completion.",
            )
            .with_details(json!({"outcomeUnknown":true}))
        } else {
            response
        });
        return;
    };
    if !response.ok {
        stop_frame_tracking(tab, Some(reply.token));
        reply.send_final(response);
        return;
    }
    let Some(action) = inventory.action.as_ref() else {
        reply.send_final(Response::error(
            &reply.id,
            "INVALID_RESULT",
            "The frame action is unavailable.",
        ));
        return;
    };
    let failure = STATE.with(|slot| {
        frame_snapshot_failure(
            &slot.borrow(),
            &inventory.browser,
            tab,
            action.snapshot.epoch,
            reply,
            &action.snapshot,
        )
    });
    if let Some(failure) = failure {
        stop_frame_tracking(tab, Some(reply.token));
        reply.send_final(
            Response::error(&reply.id, failure.code, failure.message)
                .with_details(json!({"outcomeUnknown":true})),
        );
        return;
    }
    if inventory.response.borrow().is_some() {
        return;
    }
    *inventory.response.borrow_mut() = Some(response);
    dispatch_frame_phase(Pending {
        tab,
        epoch: action.snapshot.epoch,
        evaluation: Evaluation::Page {
            mutation: true,
            evidence: None,
        },
        reply: Reply {
            frame_completion: None,
            ..reply.clone()
        },
        frame: None,
        step: Step::FrameInventory {
            inventory: inventory.clone(),
            session: action.snapshot.frame.session_id.clone(),
            phase: FramePhase::RuntimeDisable,
        },
    });
}

fn frame_inventory_current(state: &State, browser: &Browser, pending: &Pending) -> bool {
    let Step::FrameInventory {
        inventory, session, ..
    } = &pending.step
    else {
        return true;
    };
    pending.reply.active.load(Ordering::Acquire)
        && !inventory.invalidated.get()
        && inventory.page_read.as_ref().is_none_or(|page_read| {
            page_read.borrow().collected.iter().all(|collected| {
                !collected.context.invalidated() && !collected.snapshot.invalidated.get()
            })
        })
        && inventory.read.borrow().as_ref().is_none_or(|read| {
            !read.context.invalidated()
                && read
                    .parents
                    .iter()
                    .all(|parent| !parent.context.invalidated())
        })
        && same_browser(&inventory.browser, &browser)
        && inventory.owner == link_owner(&state.tasks, pending.tab)
        && state
            .tasks
            .check_tab(pending.reply.scope.as_ref(), pending.tab)
            .is_ok()
        && state.tabs.get(&pending.tab).is_some_and(|tab| {
            same_browser(&tab.browser, &browser)
                && !tab.rejected_creation
                && tab.navigation.epoch == pending.epoch
                && tab.navigation.document_epoch == inventory.document_epoch
                && !tab.navigation.provisional
                && tab.navigation.document_available
                && tab.navigation.renderer_exit.is_none()
        })
        && state
            .frame_controls
            .get(&pending.tab)
            .is_some_and(|control| {
                control.enabled
                    && control.active_token == Some(pending.reply.token)
                    && control.cleanup.is_none()
                    && control.runtime_cleanup.is_none()
                    && control.object_cleanup.is_none()
                    && !control.cleanup_failed
                    && session
                        .as_deref()
                        .is_none_or(|session| control.sessions.contains(session))
            })
}

fn start_frame_inventory(
    browser: &Browser,
    tab: i32,
    requested_frame: Option<String>,
    action: Option<FrameAction>,
    read_page: bool,
    reply: Reply,
) {
    match diagnostic_tab_info(tab, browser, &reply, |tab| {
        tab.navigation.provisional
            || !tab.navigation.committed
            || !tab.navigation.document_available
    }) {
        Ok(Some(info)) => {
            reply.error_details(
                "PAGE_LOADING",
                "The current document is not available yet.",
                json!({"tab":tab,"state":info}),
            );
            return;
        }
        Err(error) => {
            observation_error(&reply, error);
            return;
        }
        Ok(None) => {}
    }
    let prepared = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let page = state.tabs.get(&tab).filter(|page| {
            !page.rejected_creation && same_browser(&page.browser, &browser)
        })?;
        let epoch = page.navigation.epoch;
        let inventory = Shared::new(FrameInventory {
            browser: browser.clone(),
            document_epoch: page.navigation.document_epoch,
            owner: link_owner(&state.tasks, tab),
            requested_frame,
            page_read: read_page.then(|| RefCell::new(PageRead::default())),
            pointer: action
                .as_ref()
                .and_then(|action| FramePointer::new(&action.command)),
            action,
            response: RefCell::new(None),
            read: RefCell::new(None),
            current: RefCell::new((None, FramePhase::Enable)),
            invalidated: Cell::new(false),
            visited: RefCell::new(BTreeSet::new()),
            frames: RefCell::new(Vec::new()),
            truncated: Cell::new(false),
        });
        let control = state.frame_controls.entry(tab).or_default();
        if control.cleanup.is_some()
            || control.runtime_cleanup.is_some()
            || control.object_cleanup.is_some()
            || control.cleanup_failed
            || control.active_token.is_some()
        {
            return None;
        }
        if (inventory.requested_frame.is_some() || read_page) && inventory.action.is_none() {
            for snapshot in control.snapshots.drain(..) {
                snapshot.invalidated.set(true);
            }
        }
        control.enabled = true;
        control.active_token = Some(reply.token);
        control.active_inventory = Some(inventory.clone());
        let phase = if control.root_ready {
            FramePhase::Tree
        } else {
            FramePhase::Enable
        };
        Some(Pending {
            frame: None,
            tab,
            epoch,
            reply: reply.clone(),
            evaluation: Evaluation::Page {
                mutation: false,
                evidence: None,
            },
            step: Step::FrameInventory {
                inventory,
                session: None,
                phase,
            },
        })
    });
    let Some(pending) = prepared else {
        reply.error("FRAME_CONTROL_UNAVAILABLE", "Frame discovery is busy, disconnecting, or unavailable. Inspect the tab before retrying.");
        return;
    };
    dispatch_frame_phase(pending);
}

fn dispatch_frame_phase(pending: Pending) {
    let Step::FrameInventory {
        inventory,
        phase,
        session,
    } = &pending.step
    else {
        return;
    };
    let browser = inventory.browser.clone();
    *inventory.current.borrow_mut() = (session.clone(), *phase);
    if matches!(phase, FramePhase::RuntimeEnable | FramePhase::ParentEnable) && session.is_none() {
        STATE.with(|slot| {
            if let Some(control) = slot
                .borrow_mut()
                .frame_controls
                .get_mut(&pending.tab)
                .filter(|control| control.active_token == Some(pending.reply.token))
            {
                // A failed/late enable still needs explicit root-domain cleanup.
                control.root_runtime_enabled = true;
            }
        });
    }
    let (method, params) = match phase {
        FramePhase::CrashProbe => ("Inspector.enable", json!({})),
        FramePhase::Enable => ("Page.enable", json!({})),
        FramePhase::Attach => (
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true,"filter":[{"type":"iframe"},{"exclude":true}]}),
        ),
        FramePhase::Tree => ("Page.getFrameTree", json!({})),
        FramePhase::RuntimeReset
        | FramePhase::RuntimeDisable
        | FramePhase::ParentReset
        | FramePhase::ParentDisable => ("Runtime.disable", json!({})),
        FramePhase::ParentEnable => ("Runtime.enable", json!({})),
        FramePhase::ParentWorld
        | FramePhase::ParentOwner
        | FramePhase::ParentResolve
        | FramePhase::ParentInspect
        | FramePhase::ParentRelease => {
            let prepared = frame_parent_command(inventory, *phase, pending.reply.token);
            let Some(command) = prepared else {
                pending_error(
                    &pending,
                    "FRAME_OWNER_UNAVAILABLE",
                    "The frame parent inspection target is unavailable.",
                );
                return;
            };
            command
        }
        FramePhase::Action => {
            dispatch_frame_action(&pending);
            return;
        }
        FramePhase::Pointer => {
            dispatch_frame_pointer_dom(&pending);
            return;
        }
        FramePhase::RuntimeEnable => ("Runtime.enable", json!({})),
        FramePhase::World => {
            let target = inventory.read.borrow().as_ref().map(|read| {
                (
                    read.context.frame_id().to_owned(),
                    read.context.world_name().to_owned(),
                )
            });
            let Some((frame, world)) = target else {
                pending_error(
                    &pending,
                    "FRAME_UNAVAILABLE",
                    "The selected frame is unavailable.",
                );
                return;
            };
            (
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":world,"grantUniveralAccess":false}),
            )
        }
        FramePhase::Read => {
            let context = inventory
                .read
                .borrow()
                .as_ref()
                .and_then(|read| read.context.context())
                .map(|context| context.unique_id.clone());
            let Some(context) = context else {
                pending_error(
                    &pending,
                    "CONTEXT_UNAVAILABLE",
                    "The original frame context is unavailable.",
                );
                return;
            };
            let limits = inventory
                .page_read
                .as_ref()
                .map_or_else(String::new, |page_read| {
                    page_read.borrow().remaining.to_json().to_string()
                });
            let expression = format!(
                "(()=>{{if(!document.body||!['interactive','complete'].includes(document.readyState))return {{ok:false,error:'PAGE_LOADING'}};{DOM_BRIDGE}\nreturn window.__lightBrowserBridge.readFrameActions({limits});}})()"
            );
            (
                "Runtime.evaluate",
                json!({"expression":expression,"uniqueContextId":context,"returnByValue":true,"awaitPromise":false,"timeout":5000}),
            )
        }
    };
    dispatch(&browser, method, params, pending);
}

/// Disable the root auto-attacher, which owns all descendant attachments.
/// Keep the cleanup ID until the ACK; send acceptance alone is not detachment.
fn stop_frame_tracking(tab: i32, token: Option<u64>) {
    let focus = STATE.with(|slot| {
        slot.borrow()
            .focus_leases
            .get(&tab)
            .filter(|lease| token.is_none_or(|token| token == lease.token))
            .map(|lease| lease.token)
    });
    if let Some(focus_token) = focus {
        // The original child session must survive until its focus-disable ACK.
        // Revoke the frame route now, while retaining its transport for cleanup.
        STATE.with(|slot| {
            if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&tab) {
                if let Some(inventory) = &control.active_inventory {
                    inventory.invalidated.set(true);
                }
                for snapshot in &control.snapshots {
                    snapshot.invalidated.set(true);
                }
            }
        });
        abort_focus_lease(
            tab,
            "FRAME_CHANGED",
            "The frame action was revoked during input; inspect before retrying.",
            true,
        );
        // The focus-disable ACK releases the lease and then runs this deferred
        // stop. Detaching its session here as well would detach it twice.
        let _ = focus_token;
        return;
    }
    let reservation = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let browser = state.tabs.get(&tab)?.browser.clone();
        let control = state.frame_controls.get_mut(&tab)?;
        if token.is_some_and(|token| control.active_token != Some(token)) || !control.enabled {
            return None;
        }
        let object = control.deferred_object_group.take().or_else(|| {
            control.active_inventory.as_ref().and_then(|inventory| {
                let read = inventory.read.borrow();
                let read = read.as_ref()?;
                let parent = read.parents.get(read.parent_index)?;
                parent
                    .frame
                    .session_id
                    .is_none()
                    .then(|| parent.object_group.clone())
                    .flatten()
            })
        });
        let runtime_enabled = control.root_runtime_enabled;
        for snapshot in control.snapshots.drain(..) {
            snapshot.invalidated.set(true);
        }
        control.enabled = false;
        control.root_ready = false;
        control.active_token = None;
        control.active_inventory = None;
        control.sessions.clear();
        let object_id = object.as_ref().and_then(|_| state.next_control_message());
        let runtime_id = runtime_enabled
            .then(|| state.next_control_message())
            .flatten();
        let detach_id = state.next_control_message();
        let control = state.frame_controls.get_mut(&tab).unwrap();
        control.object_cleanup = object_id;
        control.runtime_cleanup = runtime_id;
        control.cleanup = detach_id;
        control.cleanup_failed = detach_id.is_none()
            || (runtime_enabled && runtime_id.is_none())
            || (object.is_some() && object_id.is_none());
        Some((browser, runtime_id, detach_id, object_id, object))
    });
    let Some((browser, runtime_id, detach_id, object_id, object)) = reservation else {
        return;
    };
    // Disposing child sessions disables their Runtime domain. Root Runtime
    // remains on the Browser connection and needs its own acknowledged reset.
    for (id, method, params) in [
        (
            object_id,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup":object}),
        ),
        (runtime_id, "Runtime.disable", json!({})),
        (
            detach_id,
            "Target.setAutoAttach",
            json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}),
        ),
    ] {
        let Some(id) = id else {
            continue;
        };
        let host = browser.host();
        let current = STATE.with(|slot| {
            let state = slot.borrow();
            state
                .tabs
                .get(&tab)
                .is_some_and(|page| same_browser(&page.browser, &browser))
                && state.frame_controls.get(&tab).is_some_and(|control| {
                    control.cleanup == Some(id)
                        || control.runtime_cleanup == Some(id)
                        || control.object_cleanup == Some(id)
                })
        });
        if !current {
            continue;
        }
        let accepted = host.is_some_and(|host| send_root_command(&host, id, method, &params) == id);
        if !accepted {
            complete_frame_cleanup(&browser, id, false);
        }
    }
}

fn complete_frame_cleanup(browser: &Browser, id: i32, success: bool) -> bool {
    STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let Some(tab) = state.frame_controls.iter().find_map(|(tab, control)| {
            ((control.cleanup == Some(id)
                || control.runtime_cleanup == Some(id)
                || control.object_cleanup == Some(id))
                && state
                    .tabs
                    .get(tab)
                    .is_some_and(|page| same_browser(&page.browser, &browser)))
            .then_some(*tab)
        }) else {
            return false;
        };
        let control = state.frame_controls.get_mut(&tab).unwrap();
        if control.object_cleanup == Some(id) {
            control.object_cleanup = None;
        } else if control.runtime_cleanup == Some(id) {
            control.runtime_cleanup = None;
            if success {
                control.root_runtime_enabled = false;
            }
        } else {
            control.cleanup = None;
        }
        control.cleanup_failed |= !success;
        true
    })
}

fn fail_frame_inventory(tab: i32, code: &str, message: &str) {
    let requests = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let ids = state
            .pending
            .iter()
            .filter_map(|(id, request)| {
                (request.tab == tab
                    && (matches!(request.step, Step::FrameInventory { .. })
                        || request.frame.is_some()))
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>()
    });
    for request in requests {
        pending_error(&request, code, message);
    }
}

fn frame_inventory_result(browser: &Browser, pending: &Pending, success: bool, bytes: &[u8]) {
    let Step::FrameInventory {
        inventory,
        session,
        phase,
    } = &pending.step
    else {
        return;
    };
    if !STATE.with(|slot| frame_inventory_current(&slot.borrow(), browser, pending)) {
        pending_error(
            pending,
            "FRAME_CHANGED",
            "The document, frame connection, or task changed during discovery.",
        );
        return;
    }
    let result_limit = if *phase == FramePhase::Read {
        2 * 1024 * 1024
    } else {
        256 * 1024
    };
    if !success || bytes.len() > result_limit {
        pending_error(
            pending,
            "FRAME_CONTROL_UNAVAILABLE",
            "Frame discovery failed or exceeded its response limit.",
        );
        return;
    }
    let Ok(result) = serde_json::from_slice::<Json>(bytes) else {
        pending_error(
            pending,
            "INVALID_RESULT",
            "Chromium returned an invalid frame result.",
        );
        return;
    };
    if *phase == FramePhase::Pointer {
        frame_pointer_result(pending, &result);
        return;
    }
    if matches!(
        phase,
        FramePhase::ParentReset
            | FramePhase::ParentEnable
            | FramePhase::ParentWorld
            | FramePhase::ParentOwner
            | FramePhase::ParentResolve
            | FramePhase::ParentInspect
            | FramePhase::ParentRelease
            | FramePhase::ParentDisable
    ) {
        frame_parent_result(pending, &result);
        return;
    }
    if matches!(
        phase,
        FramePhase::RuntimeReset
            | FramePhase::RuntimeEnable
            | FramePhase::World
            | FramePhase::Read
            | FramePhase::RuntimeDisable
    ) {
        frame_read_result(pending, &result);
        return;
    }
    let next_phase = match phase {
        // A crashed child never answers Page.enable; skip it below.
        FramePhase::CrashProbe => (!STATE.with(|slot| {
            slot.borrow()
                .frame_controls
                .get(&pending.tab)
                .zip(session.as_deref())
                .and_then(|(control, session)| control.sessions.get(session))
                .is_some_and(|entry| entry.crashed)
        }))
        .then_some(FramePhase::Enable),
        FramePhase::Enable => Some(FramePhase::Attach),
        FramePhase::Attach => {
            STATE.with(|slot| {
                if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&pending.tab) {
                    if let Some(session) = session {
                        control.sessions.mark_ready(session);
                    } else {
                        control.root_ready = true;
                    }
                }
            });
            Some(FramePhase::Tree)
        }
        FramePhase::Tree => None,
        _ => unreachable!("frame read phases handled above"),
    };
    if let Some(phase) = next_phase {
        dispatch_frame_phase(Pending {
            step: Step::FrameInventory {
                inventory: inventory.clone(),
                session: session.clone(),
                phase,
            },
            ..pending.clone()
        });
        return;
    }
    if *phase == FramePhase::Tree {
        let tree = match crate::frame_sessions::normalize_frame_tree(&result, session.as_deref()) {
            Ok(tree) => tree,
            Err(message) => {
                pending_error(pending, "INVALID_RESULT", message);
                return;
            }
        };
        inventory
            .truncated
            .set(inventory.truncated.get() || tree.truncated);
        {
            let mut frames = inventory.frames.borrow_mut();
            for frame in tree.frames {
                if frames
                    .iter()
                    .any(|previous| previous.frame_id == frame.frame_id)
                {
                    drop(frames);
                    pending_error(
                        pending,
                        "FRAME_CHANGED",
                        "A frame appeared in more than one observed tree. Inspect again after navigation settles.",
                    );
                    return;
                }
                if frames.len() >= crate::frame_sessions::MAX_FRAMES {
                    inventory.truncated.set(true);
                    break;
                }
                frames.push(frame);
            }
        }
        inventory.visited.borrow_mut().insert(session.clone());
    }
    let next = STATE.with(|slot| {
        let state = slot.borrow();
        let control = state.frame_controls.get(&pending.tab)?;
        // Chromium keeps a crashed child's renderer-bound commands queued
        // until a reload. Omit its frames and report the inventory incomplete.
        if control.sessions.has_crashed() {
            inventory.truncated.set(true);
        }
        control.sessions.next_live(&inventory.visited.borrow())
    });
    if let Some((session, ready)) = next {
        if inventory.visited.borrow().len() > crate::frame_sessions::MAX_CHILD_SESSIONS {
            pending_error(
                pending,
                "FRAME_LIMIT",
                "The frame discovery session budget was exhausted.",
            );
            return;
        }
        dispatch_frame_phase(Pending {
            step: Step::FrameInventory {
                inventory: inventory.clone(),
                session: Some(session),
                phase: if ready {
                    FramePhase::Tree
                } else {
                    FramePhase::CrashProbe
                },
            },
            ..pending.clone()
        });
    } else {
        if inventory.page_read.is_some() {
            start_page_read(pending);
            return;
        }
        if let Some(requested) = &inventory.requested_frame {
            let frame = inventory
                .frames
                .borrow()
                .iter()
                .find(|frame| &frame.frame_id == requested)
                .cloned();
            let Some(frame) = frame else {
                pending_error(
                    pending,
                    if inventory.truncated.get() {
                        "FRAME_LIMIT"
                    } else {
                        "FRAME_NOT_FOUND"
                    },
                    "The requested frame was not found in this bounded current-tab inventory.",
                );
                return;
            };
            let Some(context) = crate::frame_sessions::ContextTracker::new(
                &frame.frame_id,
                "LightBrowserFrameControl",
            ) else {
                pending_error(
                    pending,
                    "CONTEXT_UNAVAILABLE",
                    "The frame cannot be assigned an inspection context.",
                );
                return;
            };
            let parents = if let Some(action) = &inventory.action {
                if action.snapshot.invalidated.get()
                    || frame.frame_id != action.snapshot.frame.frame_id
                    || frame.session_id != action.snapshot.frame.session_id
                {
                    pending_error(
                        pending,
                        "FRAME_CHANGED",
                        "The frame's original connection changed.",
                    );
                    return;
                }
                match frame_parent_chain(&frame, &inventory.frames.borrow()) {
                    Some(parents) => parents,
                    None => {
                        pending_error(
                            pending,
                            "FRAME_LIMIT",
                            "The complete parent frame chain is unavailable.",
                        );
                        return;
                    }
                }
            } else {
                Vec::new()
            };
            let (session, phase) = match parents.first() {
                Some(parent) if inventory.pointer.is_none() => {
                    (parent.frame.session_id.clone(), FramePhase::ParentReset)
                }
                _ => (frame.session_id.clone(), FramePhase::RuntimeReset),
            };
            *inventory.read.borrow_mut() = Some(FrameRead {
                frame,
                context,
                page: None,
                unavailable: None,
                parents,
                parent_index: 0,
            });
            dispatch_frame_phase(Pending {
                step: Step::FrameInventory {
                    inventory: inventory.clone(),
                    session,
                    phase,
                },
                ..pending.clone()
            });
            return;
        }
        STATE.with(|slot| {
            if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&pending.tab) {
                inventory
                    .truncated
                    .set(inventory.truncated.get() || control.sessions.truncated());
                control.active_token = None;
                control.active_inventory = None;
            }
        });
        pending.reply.ok(json!({"tab":pending.tab,"epoch":pending.epoch,"documentEpoch":inventory.document_epoch,
            "frames":*inventory.frames.borrow(),"truncated":inventory.truncated.get(),"atomic":false,"contentIncluded":false}));
    }
}

fn start_page_read(pending: &Pending) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    let Some(page_read) = &inventory.page_read else {
        return;
    };
    let frames = inventory.frames.borrow();
    let roots = frames
        .iter()
        .filter(|frame| frame.parent_id.is_none())
        .collect::<Vec<_>>();
    if roots.len() != 1 || roots[0].session_id.is_some() {
        drop(frames);
        pending_error(
            pending,
            "FRAME_CHANGED",
            "The page inventory has no unique top document.",
        );
        return;
    }
    // Visit parents before children. Child sessions may have been discovered
    // after their same-process siblings; this is not a unified DOM order.
    let mut queue = vec![roots[0].clone()];
    let mut index = 0;
    while index < queue.len() && queue.len() <= crate::frame_sessions::MAX_FRAMES {
        let parent = queue[index].frame_id.clone();
        for frame in frames
            .iter()
            .filter(|frame| frame.parent_id.as_deref() == Some(&parent))
        {
            if queue.iter().any(|queued| queued.frame_id == frame.frame_id) {
                drop(frames);
                pending_error(
                    pending,
                    "FRAME_CHANGED",
                    "The page inventory contains a cyclic frame relation.",
                );
                return;
            }
            queue.push(frame.clone());
        }
        index += 1;
    }
    if queue.len() != frames.len() || queue.len() > crate::frame_sessions::MAX_FRAMES {
        drop(frames);
        pending_error(
            pending,
            "FRAME_LIMIT",
            "The complete parent relation is unavailable for page reading.",
        );
        return;
    }
    drop(frames);
    page_read.borrow_mut().queue = queue;
    STATE.with(|slot| {
        if let Some(control) = slot.borrow().frame_controls.get(&pending.tab) {
            inventory
                .truncated
                .set(inventory.truncated.get() || control.sessions.truncated());
        }
    });
    next_page_document(pending);
}

fn next_page_document(pending: &Pending) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    let Some(page_read) = &inventory.page_read else {
        return;
    };
    let frame = {
        let mut batch = page_read.borrow_mut();
        let reason = if batch.next >= 16 {
            Some("DOCUMENT_LIMIT")
        } else if batch.remaining.nodes == 0
            || (batch.remaining.text == 0
                && batch.remaining.elements == 0
                && batch.remaining.frames == 0)
        {
            Some("READ_BUDGET")
        } else {
            None
        };
        if let Some(reason) = reason {
            while batch.next < batch.queue.len() {
                let frame = batch.queue[batch.next].clone();
                batch.unread.push(json!({"frame":frame,"reason":reason}));
                batch.next += 1;
                batch.truncated = true;
            }
            None
        } else {
            let frame = batch.queue.get(batch.next).cloned();
            if frame.is_some() {
                batch.next += 1;
            }
            frame
        }
    };
    let Some(frame) = frame else {
        finish_page_read(pending);
        return;
    };
    let Some(context) =
        crate::frame_sessions::ContextTracker::new(&frame.frame_id, "LightBrowserFrameControl")
    else {
        pending_error(
            pending,
            "CONTEXT_UNAVAILABLE",
            "The frame cannot be assigned an inspection context.",
        );
        return;
    };
    let session = frame.session_id.clone();
    *inventory.read.borrow_mut() = Some(FrameRead {
        frame,
        context,
        page: None,
        unavailable: None,
        parents: Vec::new(),
        parent_index: 0,
    });
    dispatch_frame_phase(Pending {
        step: Step::FrameInventory {
            inventory: inventory.clone(),
            session,
            phase: FramePhase::RuntimeReset,
        },
        ..pending.clone()
    });
}

fn finish_page_read(pending: &Pending) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    let Some(page_read) = &inventory.page_read else {
        return;
    };
    if !STATE.with(|slot| frame_inventory_current(&slot.borrow(), &inventory.browser, pending)) {
        pending_error(
            pending,
            "FRAME_CHANGED",
            "A collected document changed before the page read completed.",
        );
        return;
    }
    let (documents, unread, remaining, truncated) = {
        let mut batch = page_read.borrow_mut();
        (
            std::mem::take(&mut batch.documents),
            std::mem::take(&mut batch.unread),
            batch.remaining,
            batch.truncated,
        )
    };
    let limit = crate::page_read::Budget::default().to_json();
    let remaining = remaining.to_json();
    let mut used = json!({});
    for key in ["text", "elements", "nodes", "roots", "options", "frames"] {
        used[key] = json!(limit[key].as_u64().unwrap() - remaining[key].as_u64().unwrap());
    }
    let response = Response::success(
        &pending.reply.id,
        json!({
            "tab":pending.tab,"epoch":pending.epoch,"documentEpoch":inventory.document_epoch,
            "documents":documents,"unread":unread,"frames":*inventory.frames.borrow(),
            "budget":{"limit":limit,"used":used,"remaining":remaining},
            "truncated":truncated || inventory.truncated.get(),"atomic":false,
            "supportedActions":["click","hover","fill","insert","press","check","select"],
            "pointerActionsSupported":true
        }),
    );
    if crate::page_read::serialized_size(&response).is_none() {
        pending_error(
            pending,
            "RESULT_LIMIT",
            "The combined page response exceeded its byte limit.",
        );
        return;
    }
    STATE.with(|slot| {
        if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&pending.tab) {
            control.snapshots = page_read
                .borrow()
                .collected
                .iter()
                .map(|collected| collected.snapshot.clone())
                .collect();
        }
    });
    finish_frame_inventory_state(pending.tab, pending.reply.token);
    pending.reply.send_final(response);
}

fn frame_parent_chain(
    frame: &crate::frame_sessions::FrameRecord,
    frames: &[crate::frame_sessions::FrameRecord],
) -> Option<Vec<FrameParent>> {
    let mut parents = Vec::new();
    let mut child = frame;
    let mut visited = BTreeSet::new();
    visited.insert(frame.frame_id.as_str());
    while let Some(parent_id) = &child.parent_id {
        if parents.len() >= crate::frame_sessions::MAX_FRAME_DEPTH
            || !visited.insert(parent_id.as_str())
        {
            return None;
        }
        let parent = frames
            .iter()
            .find(|candidate| &candidate.frame_id == parent_id)?;
        parents.push(FrameParent {
            child: child.frame_id.clone(),
            frame: parent.clone(),
            context: crate::frame_sessions::ContextTracker::new(
                parent_id,
                "LightBrowserFrameAncestors",
            )?,
            backend_node: None,
            object: None,
            object_group: None,
            geometry: None,
        });
        child = parent;
    }
    Some(parents)
}

fn frame_parent_command(
    inventory: &FrameInventory,
    phase: FramePhase,
    token: u64,
) -> Option<(&'static str, Json)> {
    let mut read = inventory.read.borrow_mut();
    let read = read.as_mut()?;
    let parent = read.parents.get_mut(read.parent_index)?;
    Some(match phase {
        FramePhase::ParentWorld => (
            "Page.createIsolatedWorld",
            json!({"frameId":parent.frame.frame_id,
            "worldName":parent.context.world_name(),"grantUniveralAccess":false}),
        ),
        FramePhase::ParentOwner => ("DOM.getFrameOwner", json!({"frameId":parent.child})),
        FramePhase::ParentResolve => {
            let group = format!("NaruFrameParent:{token}:{}", read.parent_index);
            parent.object_group = Some(group.clone());
            (
                "DOM.resolveNode",
                json!({"backendNodeId":parent.backend_node?,
                "executionContextId":parent.context.context()?.id,"objectGroup":group}),
            )
        }
        FramePhase::ParentInspect => {
            let request = inventory.pointer.as_ref().map(|pointer| match pointer.stage.get() {
                FramePointerStage::ParentGeometry => Some(json!({"action":pointer.target.action(),"phase":"geometry"})),
                FramePointerStage::ParentPoints | FramePointerStage::ParentVerify => Some(json!({
                    "action":pointer.target.action(),"phase":"points",
                    "points":pointer.candidates.borrow().iter().map(|point| point.to_json()).collect::<Vec<_>>()
                })),
                _ => None,
            });
            let (expression, arguments) = if let Some(request) = request {
                let request = request?;
                (
                    format!(
                        "function(owner,request){{{DOM_BRIDGE}\nreturn window.__lightBrowserBridge.inspectFramePointer(owner,request);}}"
                    ),
                    json!([{"objectId":parent.object.as_ref()?},{"value":request}]),
                )
            } else {
                (
                    format!(
                        "function(owner){{{DOM_BRIDGE}\nreturn window.__lightBrowserBridge.inspectFrameOwner(owner);}}"
                    ),
                    json!([{"objectId":parent.object.as_ref()?}]),
                )
            };
            (
                "Runtime.callFunctionOn",
                json!({"functionDeclaration":expression,
                "uniqueContextId":parent.context.context()?.unique_id,
                "arguments":arguments,
                "objectGroup":parent.object_group.as_ref()?,
                "returnByValue":true,"awaitPromise":false}),
            )
        }
        FramePhase::ParentRelease => (
            "Runtime.releaseObjectGroup",
            json!({"objectGroup":parent.object_group.as_ref()?}),
        ),
        _ => return None,
    })
}

fn frame_parent_result(pending: &Pending, result: &Json) {
    let Step::FrameInventory {
        inventory,
        session,
        phase,
    } = &pending.step
    else {
        return;
    };
    let next = (|| {
        let mut slot = inventory.read.borrow_mut();
        let read = slot.as_mut()?;
        let parent = read.parents.get_mut(read.parent_index)?;
        let next = match phase {
            FramePhase::ParentReset => FramePhase::ParentEnable,
            FramePhase::ParentEnable => FramePhase::ParentWorld,
            FramePhase::ParentWorld => {
                let id = result["executionContextId"]
                    .as_i64()
                    .and_then(|id| i32::try_from(id).ok())?;
                parent.context.confirm(id)?;
                FramePhase::ParentOwner
            }
            FramePhase::ParentOwner => {
                let backend_node = Some(
                    result["backendNodeId"]
                        .as_u64()
                        .filter(|id| *id > 0 && *id <= i32::MAX as u64)?,
                );
                if inventory.pointer.is_some()
                    && parent.backend_node.is_some()
                    && parent.backend_node != backend_node
                {
                    return None;
                }
                parent.backend_node = backend_node;
                FramePhase::ParentResolve
            }
            FramePhase::ParentResolve => {
                if result["object"]["type"] != "object" || result["object"]["subtype"] != "node" {
                    return None;
                }
                parent.object = Some(
                    result["object"]["objectId"]
                        .as_str()
                        .filter(|id| {
                            !id.is_empty()
                                && id.len() <= 512
                                && id.is_ascii()
                                && !id.bytes().any(|b| b.is_ascii_control())
                        })?
                        .to_owned(),
                );
                FramePhase::ParentInspect
            }
            FramePhase::ParentInspect => {
                let value = &result["result"]["value"];
                if result.get("exceptionDetails").is_some() || value["ok"] != true {
                    return None;
                }
                if let Some(pointer) = &inventory.pointer {
                    if value["action"] != pointer.target.action()
                        || value["prepared"] != true
                        || value["attempted"] != false
                    {
                        return None;
                    }
                    let geometry = crate::frame_pointer::Geometry::from_json(&value["geometry"])?;
                    match pointer.stage.get() {
                        FramePointerStage::ParentGeometry => parent.geometry = Some(geometry),
                        FramePointerStage::ParentPoints | FramePointerStage::ParentVerify => {
                            let filtered = crate::frame_pointer::candidate_filter(
                                &value["candidates"],
                                &pointer.candidates.borrow(),
                                &geometry,
                            )?;
                            *pointer.candidates.borrow_mut() = filtered;
                        }
                        _ => return None,
                    }
                } else if value["visible"] != true || value["disabled"] != false {
                    return None;
                }
                FramePhase::ParentRelease
            }
            FramePhase::ParentRelease => {
                parent.object = None;
                parent.object_group = None;
                FramePhase::ParentDisable
            }
            FramePhase::ParentDisable => {
                if session.is_none() {
                    STATE.with(|slot| {
                        if let Some(control) =
                            slot.borrow_mut().frame_controls.get_mut(&pending.tab)
                        {
                            control.root_runtime_enabled = false;
                        }
                    });
                }
                read.parent_index += 1;
                if inventory.pointer.as_ref().is_some_and(|pointer| {
                    matches!(
                        pointer.stage.get(),
                        FramePointerStage::ParentPoints | FramePointerStage::ParentVerify
                    ) && pointer.candidates.borrow().is_empty()
                }) {
                    read.parent_index = read.parents.len();
                }
                return Some(match read.parents.get(read.parent_index) {
                    Some(parent) => (parent.frame.session_id.clone(), FramePhase::ParentReset),
                    None if inventory.pointer.is_some() => {
                        (read.frame.session_id.clone(), FramePhase::Pointer)
                    }
                    None => (read.frame.session_id.clone(), FramePhase::RuntimeReset),
                });
            }
            _ => return None,
        };
        Some((session.clone(), next))
    })();
    let Some((session, phase)) = next else {
        pending_error(
            pending,
            "FRAME_OWNER_UNAVAILABLE",
            "The embedding frame is hidden, disabled, changed or unavailable. Inspect before retrying.",
        );
        return;
    };
    if phase == FramePhase::Pointer && inventory.pointer.is_some() {
        frame_pointer_parents_done(pending, inventory);
        return;
    }
    dispatch_frame_phase(Pending {
        step: Step::FrameInventory {
            inventory: inventory.clone(),
            session,
            phase,
        },
        ..pending.clone()
    });
}

fn dispatch_frame_action(pending: &Pending) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    if inventory.pointer.is_some() {
        let session = inventory
            .read
            .borrow()
            .as_ref()
            .map(|read| read.frame.session_id.clone());
        if let Some(session) = session {
            dispatch_frame_phase(Pending {
                step: Step::FrameInventory {
                    inventory: inventory.clone(),
                    session,
                    phase: FramePhase::Pointer,
                },
                ..pending.clone()
            });
        } else {
            pending_error(
                pending,
                "FRAME_CHANGED",
                "The pointer frame is unavailable.",
            );
        }
        return;
    }
    let Some(action) = &inventory.action else {
        return;
    };
    let Some(args) = frame_action_arguments(&action.command, &action.snapshot) else {
        pending_error(
            pending,
            "STALE_FRAME_SNAPSHOT",
            "The element reference does not belong to the current frame snapshot.",
        );
        return;
    };
    let expression = format!(
        "window.__lightBrowserBridge.frameAct({args},{})",
        action.snapshot.binding()
    );
    dispatch(
        &inventory.browser,
        "Runtime.evaluate",
        json!({"expression":expression,"uniqueContextId":action.snapshot.unique_context_id(),
            "returnByValue":true,"awaitPromise":false,"timeout":5000}),
        Pending {
            frame: Some(action.snapshot.clone()),
            evaluation: Evaluation::Page {
                mutation: true,
                evidence: ActionEvidence::for_command(&action.command),
            },
            reply: Reply {
                frame_completion: Some(pending.tab),
                ..pending.reply.clone()
            },
            step: Step::Evaluate,
            ..pending.clone()
        },
    );
}

fn dispatch_frame_pointer_dom(pending: &Pending) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    let (Some(pointer), Some(action)) = (&inventory.pointer, &inventory.action) else {
        return;
    };
    let Some(mut args) = frame_action_arguments(&action.command, &action.snapshot) else {
        pending_error(
            pending,
            "STALE_FRAME_SNAPSHOT",
            "Read the frame again before pointer input.",
        );
        return;
    };
    let verifying = match pointer.stage.get() {
        FramePointerStage::Geometry => {
            args["pointerPhase"] = json!("geometry");
            args["scroll"] = json!(pointer.scrolled.get());
            false
        }
        FramePointerStage::Candidates => {
            let Some(clip) = pointer.clip.get() else {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The clipped pointer area is unavailable.",
                );
                return;
            };
            args["pointerPhase"] = json!("candidates");
            args["clip"] = clip.to_json();
            false
        }
        FramePointerStage::Move | FramePointerStage::Finish => {
            let Some(point) = pointer.target.local_point.get() else {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The original frame point is unavailable.",
                );
                return;
            };
            args["verifyOnly"] = json!(true);
            args["point"] = json!({"x":point.x,"y":point.y});
            true
        }
        _ => {
            pending_error(
                pending,
                "INVALID_RESULT",
                "The pointer preparation phase is invalid.",
            );
            return;
        }
    };
    let expression = format!(
        "window.__lightBrowserBridge.frameAct({args},{})",
        action.snapshot.binding()
    );
    let next = if verifying {
        Pending {
            frame: Some(action.snapshot.clone()),
            evaluation: Evaluation::Page {
                mutation: true,
                evidence: Some(ActionEvidence::Click(pointer.target.clone())),
            },
            reply: Reply {
                frame_completion: Some(pending.tab),
                ..pending.reply.clone()
            },
            step: Step::Evaluate,
            ..pending.clone()
        }
    } else {
        pending.clone()
    };
    dispatch(
        &inventory.browser,
        "Runtime.evaluate",
        json!({"expression":expression,"uniqueContextId":action.snapshot.unique_context_id(),
            "returnByValue":true,"awaitPromise":false,"timeout":5000}),
        next,
    );
}

fn frame_pointer_target(pending: &Pending, inventory: &Shared<FrameInventory>) {
    let session = inventory
        .read
        .borrow()
        .as_ref()
        .map(|read| read.frame.session_id.clone());
    if let Some(session) = session {
        dispatch_frame_phase(Pending {
            frame: None,
            evaluation: Evaluation::Page {
                mutation: false,
                evidence: None,
            },
            step: Step::FrameInventory {
                inventory: inventory.clone(),
                session,
                phase: FramePhase::RuntimeReset,
            },
            ..pending.clone()
        });
    } else {
        pending_error(
            pending,
            "FRAME_CHANGED",
            "The pointer frame is unavailable.",
        );
    }
}

fn frame_pointer_parents(pending: &Pending, inventory: &Shared<FrameInventory>) {
    let first = inventory.read.borrow_mut().as_mut().and_then(|read| {
        read.parent_index = 0;
        read.parents
            .first()
            .map(|parent| parent.frame.session_id.clone())
    });
    if let Some(session) = first {
        dispatch_frame_phase(Pending {
            frame: None,
            evaluation: Evaluation::Page {
                mutation: false,
                evidence: None,
            },
            step: Step::FrameInventory {
                inventory: inventory.clone(),
                session,
                phase: FramePhase::ParentReset,
            },
            ..pending.clone()
        });
    } else {
        frame_pointer_parents_done(pending, inventory);
    }
}

fn frame_pointer_retry(pending: &Pending, inventory: &Shared<FrameInventory>) {
    let Some(pointer) = &inventory.pointer else {
        return;
    };
    if pointer.target.attempted.get() || pointer.scrolled.replace(true) {
        pending_error(
            pending,
            "CLICK_POINT_UNAVAILABLE",
            "No supported point reached the frame element. Inspect before retrying; no pointer input is repeated.",
        );
        return;
    }
    pointer.stage.set(FramePointerStage::Geometry);
    pointer.candidates.borrow_mut().clear();
    pointer.local_candidates.borrow_mut().clear();
    pointer.clip.set(None);
    if let Some(read) = inventory.read.borrow_mut().as_mut() {
        for parent in &mut read.parents {
            parent.geometry = None;
        }
    }
    frame_pointer_target(pending, inventory);
}

fn frame_pointer_parents_done(pending: &Pending, inventory: &Shared<FrameInventory>) {
    let Some(pointer) = &inventory.pointer else {
        return;
    };
    match pointer.stage.get() {
        FramePointerStage::ParentGeometry => {
            let geometry = inventory.read.borrow().as_ref().and_then(|read| {
                read.parents
                    .iter()
                    .map(|parent| parent.geometry)
                    .collect::<Option<Vec<_>>>()
            });
            let clip = geometry.and_then(|geometry| {
                Some(crate::frame_pointer::local_clip(
                    pointer.rect.get()?,
                    pointer.viewport.get()?,
                    &geometry,
                ))
            });
            match clip {
                Some(Ok(Some(clip))) => pointer.clip.set(Some(clip)),
                Some(Ok(None)) => {
                    frame_pointer_retry(pending, inventory);
                    return;
                }
                _ => {
                    pending_error(
                        pending,
                        "UNSUPPORTED_GEOMETRY",
                        "The frame clipping geometry is unsupported.",
                    );
                    return;
                }
            }
            pointer.stage.set(FramePointerStage::Candidates);
        }
        FramePointerStage::ParentPoints => {
            let candidate = pointer.candidates.borrow().first().copied();
            let Some(candidate) = candidate else {
                frame_pointer_retry(pending, inventory);
                return;
            };
            let local = pointer
                .local_candidates
                .borrow()
                .iter()
                .find(|local| local.index == candidate.index)
                .copied();
            let Some(local) = local else {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The frame point lost its original candidate.",
                );
                return;
            };
            pointer.target.local_point.set(Some(ClickPoint {
                x: local.x,
                y: local.y,
            }));
            pointer.target.point.set(Some(ClickPoint {
                x: candidate.x,
                y: candidate.y,
            }));
            pointer.stage.set(FramePointerStage::Move);
        }
        FramePointerStage::ParentVerify => {
            let valid = {
                let candidates = pointer.candidates.borrow();
                candidates.len() == 1
                    && pointer.target.point.get()
                        == Some(ClickPoint {
                            x: candidates[0].x,
                            y: candidates[0].y,
                        })
            };
            if !valid {
                pending_error(
                    pending,
                    "CLICK_POINT_UNAVAILABLE",
                    "The original pointer position no longer reaches the frame element. Inspect before retrying.",
                );
                return;
            }
            pointer.stage.set(FramePointerStage::Finish);
        }
        _ => {
            pending_error(
                pending,
                "INVALID_RESULT",
                "The pointer parent phase is invalid.",
            );
            return;
        }
    }
    frame_pointer_target(pending, inventory);
}

fn frame_pointer_result(pending: &Pending, result: &Json) {
    let Step::FrameInventory { inventory, .. } = &pending.step else {
        return;
    };
    let Some(pointer) = &inventory.pointer else {
        return;
    };
    let value = &result["result"]["value"];
    if result.get("exceptionDetails").is_some()
        || value["ok"] != true
        || value["prepared"] != true
        || value["attempted"] != false
        || value["action"] != pointer.target.action()
    {
        pending_error(
            pending,
            "FRAME_POINTER_UNAVAILABLE",
            "The frame could not prepare its pointer geometry. Inspect the page before retrying.",
        );
        return;
    }
    match pointer.stage.get() {
        FramePointerStage::Geometry => {
            let rect = crate::frame_pointer::Rect::from_json(&value["rect"]);
            let viewport = crate::frame_pointer::Rect::from_json(&value["viewport"]);
            if rect.is_none() || viewport.is_none() {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The frame returned invalid pointer geometry.",
                );
                return;
            }
            pointer.rect.set(rect);
            pointer.viewport.set(viewport);
            pointer.stage.set(FramePointerStage::ParentGeometry);
        }
        FramePointerStage::Candidates => {
            let Some(items) = value["candidates"]
                .as_array()
                .filter(|items| items.len() <= 9)
            else {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The frame returned invalid pointer candidates.",
                );
                return;
            };
            let candidates: Option<Vec<_>> = items
                .iter()
                .map(crate::frame_pointer::Candidate::from_json)
                .collect();
            let Some(candidates) = candidates else {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The frame returned an invalid candidate.",
                );
                return;
            };
            let unique: BTreeSet<_> = candidates.iter().map(|candidate| candidate.index).collect();
            if unique.len() != candidates.len() {
                pending_error(
                    pending,
                    "INVALID_POINT",
                    "The frame returned duplicate pointer candidates.",
                );
                return;
            }
            if candidates.is_empty() {
                frame_pointer_retry(pending, inventory);
                return;
            }
            *pointer.local_candidates.borrow_mut() = candidates.clone();
            *pointer.candidates.borrow_mut() = candidates;
            pointer.stage.set(FramePointerStage::ParentPoints);
        }
        _ => {
            pending_error(
                pending,
                "INVALID_RESULT",
                "The pointer result belongs to another phase.",
            );
            return;
        }
    }
    frame_pointer_parents(pending, inventory);
}

fn frame_pointer_input_ready(browser: &Browser, id: i32, pending: &Pending, value: &Json) {
    let inventory = STATE.with(|slot| {
        slot.borrow()
            .frame_controls
            .get(&pending.tab)
            .filter(|control| control.active_token == Some(pending.reply.token))
            .and_then(|control| control.active_inventory.clone())
    });
    let Some(inventory) = inventory else {
        finish_input_preparation(
            id,
            pending,
            "FRAME_CHANGED",
            "The original pointer operation is unavailable.",
        );
        return;
    };
    let Some(pointer) = &inventory.pointer else {
        finish_input_preparation(
            id,
            pending,
            "INVALID_RESULT",
            "The frame pointer operation is unavailable.",
        );
        return;
    };
    if value["ok"] != true
        || value["prepared"] != true
        || value["attempted"] != false
        || value["action"] != pointer.target.action()
        || pointer.target.local_point.get().is_none()
        || ClickPoint::from_json(&value["point"]) != pointer.target.local_point.get()
    {
        finish_input_preparation(
            id,
            pending,
            "CLICK_POINT_UNAVAILABLE",
            "The original frame point did not pass its hit test.",
        );
        return;
    }
    match (
        pointer.stage.get(),
        &pending.step,
        pointer.target.attempted.get(),
    ) {
        (FramePointerStage::Move, Step::Evaluate, false) => {
            dispatch_input(
                browser,
                id,
                pending,
                NativeInput::MouseMove(pointer.target.clone()),
            );
        }
        (FramePointerStage::Move, Step::ClickVerify, true) => {
            if !take_input_preparation(id, pending) {
                return;
            }
            let point = pointer.target.local_point.get().unwrap();
            *pointer.candidates.borrow_mut() = vec![crate::frame_pointer::Candidate {
                index: 0,
                x: point.x,
                y: point.y,
            }];
            pointer.stage.set(FramePointerStage::ParentVerify);
            let next = Pending {
                frame: None,
                evaluation: Evaluation::Page {
                    mutation: false,
                    evidence: None,
                },
                step: Step::FrameInventory {
                    inventory: inventory.clone(),
                    session: None,
                    phase: FramePhase::Pointer,
                },
                ..pending.clone()
            };
            frame_pointer_parents(&next, &inventory);
        }
        (FramePointerStage::Finish, Step::Evaluate, true) => {
            if pointer.target.hover {
                if take_input_preparation(id, pending) {
                    pending.reply.ok(json!({"tab":pending.tab,"page":{
                        "ok":true,"action":"hover","input":"chromium","attempted":true,
                        "dispatchAcknowledged":true,"pointVerified":true,
                        "detail":"Chromium acknowledged mouseMoved; the original frame point and embedding chain passed their subsequent hit tests. Read again to verify hover effects."
                    }}));
                }
            } else if let Some(point) = pointer.target.point.get() {
                dispatch_input(browser, id, pending, NativeInput::Click(point));
            } else {
                finish_input_preparation(
                    id,
                    pending,
                    "INVALID_POINT",
                    "The original root point is unavailable.",
                );
            }
        }
        _ => finish_input_preparation(
            id,
            pending,
            "INVALID_RESULT",
            "The pointer verification belongs to another phase.",
        ),
    }
}

fn finish_frame_inventory_state(tab: i32, token: u64) {
    STATE.with(|slot| {
        if let Some(control) = slot
            .borrow_mut()
            .frame_controls
            .get_mut(&tab)
            .filter(|control| control.active_token == Some(token))
        {
            control.active_token = None;
            control.active_inventory = None;
        }
    });
}

fn make_frame_snapshot(
    pending: &Pending,
    inventory: &FrameInventory,
    read: &FrameRead,
    page: &mut Json,
) -> Option<FrameSnapshot> {
    let document_id = page["documentId"]
        .as_str()
        .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))?
        .to_owned();
    let generation = page["generation"]
        .as_u64()
        .filter(|value| *value > 0 && *value <= 9_007_199_254_740_991)?;
    let prefix = match &inventory.page_read {
        Some(page_read) => format!(
            "frame/{}/{}/{}/",
            pending.epoch,
            pending.reply.token,
            page_read.borrow().next
        ),
        None => format!("frame/{}/{}/", pending.epoch, pending.reply.token),
    };
    let element_prefix = format!("{document_id}:{generation}:");
    let focused = page["focusedRef"].as_str().map(str::to_owned);
    let elements = page["interactive"].as_array_mut()?;
    if elements.len() > 120 {
        return None;
    }
    let mut references = BTreeSet::new();
    for item in elements {
        let reference = item["ref"].as_str()?;
        let ordinal = reference
            .strip_prefix(&element_prefix)?
            .parse::<usize>()
            .ok()?;
        if ordinal == 0 || ordinal > 120 || prefix.len() + reference.len() > 256 {
            return None;
        }
        let qualified = format!("{prefix}{reference}");
        if !references.insert(qualified.clone()) {
            return None;
        }
        item["ref"] = json!(qualified);
    }
    page["focusedRef"] = json!(
        focused
            .map(|reference| format!("{prefix}{reference}"))
            .filter(|reference| references.contains(reference))
    );
    page["supportedActions"] = json!([
        "click", "hover", "fill", "insert", "press", "check", "select"
    ]);
    page["pointerActionsSupported"] = Json::Bool(true);
    Some(FrameSnapshot {
        browser: inventory.browser.clone(),
        epoch: pending.epoch,
        document_epoch: inventory.document_epoch,
        owner: inventory.owner.clone(),
        frame: read.frame.clone(),
        unique_context: read.context.context()?.unique_id.clone(),
        prefix,
        document_id,
        generation,
        references,
        invalidated: Cell::new(false),
    })
}

fn frame_read_result(pending: &Pending, result: &Json) {
    let Step::FrameInventory {
        inventory,
        session,
        phase,
    } = &pending.step
    else {
        return;
    };
    let next_phase = match phase {
        FramePhase::RuntimeReset => FramePhase::RuntimeEnable,
        FramePhase::RuntimeEnable => FramePhase::World,
        FramePhase::World => {
            let context_id = result["executionContextId"]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok());
            let matched = context_id.is_some_and(|id| {
                inventory
                    .read
                    .borrow_mut()
                    .as_mut()
                    .is_some_and(|read| read.context.confirm(id).is_some())
            });
            if !matched {
                pending_error(
                    pending,
                    "CONTEXT_UNAVAILABLE",
                    "The isolated context did not match the frame's creation event.",
                );
                return;
            }
            if let Some(action) = &inventory.action {
                let current = inventory.read.borrow().as_ref().is_some_and(|read| {
                    read.context
                        .context()
                        .is_some_and(|context| context.unique_id == action.snapshot.unique_context)
                });
                if !current {
                    pending_error(
                        pending,
                        "STALE_FRAME_SNAPSHOT",
                        "The frame's original execution context no longer exists. Read again.",
                    );
                    return;
                }
                FramePhase::Action
            } else {
                FramePhase::Read
            }
        }
        FramePhase::Read => {
            let value = &result["result"]["value"];
            if result.get("exceptionDetails").is_some() || !value.is_object() {
                pending_error(
                    pending,
                    "PAGE_SCRIPT_ERROR",
                    "The selected frame could not be inspected.",
                );
                return;
            }
            if value["error"] == "PAGE_LOADING" && value["ok"] == false {
                if inventory.page_read.is_some() {
                    if let Some(read) = inventory.read.borrow_mut().as_mut() {
                        read.unavailable = Some("PAGE_LOADING");
                    }
                    dispatch_frame_phase(Pending {
                        step: Step::FrameInventory {
                            inventory: inventory.clone(),
                            session: session.clone(),
                            phase: FramePhase::RuntimeDisable,
                        },
                        ..pending.clone()
                    });
                    return;
                }
                pending_error(
                    pending,
                    "PAGE_LOADING",
                    "The selected frame document is not ready.",
                );
                return;
            }
            if value["scope"] != "local-frame"
                || !value["interactive"].is_array()
                || !matches!(
                    value["readyState"].as_str(),
                    Some("interactive" | "complete")
                )
            {
                pending_error(
                    pending,
                    "INVALID_PAGE_RESULT",
                    "The selected frame did not return a local document snapshot.",
                );
                return;
            }
            if let Some(read) = inventory.read.borrow_mut().as_mut() {
                read.page = Some(value.clone());
            }
            FramePhase::RuntimeDisable
        }
        FramePhase::RuntimeDisable => {
            if session.is_none() {
                STATE.with(|slot| {
                    if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&pending.tab) {
                        control.root_runtime_enabled = false;
                    }
                });
            }
            if let Some(response) = inventory.response.borrow_mut().take() {
                finish_frame_inventory_state(pending.tab, pending.reply.token);
                pending.reply.send_final(response);
                return;
            }
            let read = inventory.read.borrow_mut().take();
            let Some(mut read) = read else {
                pending_error(
                    pending,
                    "INVALID_PAGE_RESULT",
                    "The frame snapshot is unavailable.",
                );
                return;
            };
            if let Some(reason) = read.unavailable {
                if let Some(page_read) = &inventory.page_read {
                    let mut batch = page_read.borrow_mut();
                    batch
                        .unread
                        .push(json!({"frame":read.frame,"reason":reason}));
                    batch.truncated = true;
                    drop(batch);
                    next_page_document(pending);
                    return;
                }
            }
            let Some(mut page) = read.page.take() else {
                pending_error(
                    pending,
                    "INVALID_PAGE_RESULT",
                    "The frame snapshot is unavailable.",
                );
                return;
            };
            let Some(snapshot) = make_frame_snapshot(pending, inventory, &read, &mut page) else {
                pending_error(
                    pending,
                    "INVALID_PAGE_RESULT",
                    "The frame snapshot references are invalid.",
                );
                return;
            };
            if let Some(page_read) = &inventory.page_read {
                let mut batch = page_read.borrow_mut();
                if batch.remaining.consume(&page).is_none() {
                    drop(batch);
                    pending_error(
                        pending,
                        "INVALID_PAGE_BUDGET",
                        "The frame result did not match the remaining shared read budget.",
                    );
                    return;
                }
                batch.truncated |= page["truncated"]
                    .as_object()
                    .is_some_and(|flags| flags.values().any(|flag| flag == &Json::Bool(true)))
                    || page["interactive"].as_array().is_some_and(|items| {
                        items.iter().any(|item| item["optionsTruncated"] == true)
                    });
                let document = json!({"frame":read.frame,"page":page});
                let bytes = crate::page_read::serialized_size(&document)
                    .and_then(|bytes| batch.document_bytes.checked_add(bytes))
                    .filter(|bytes| *bytes <= crate::page_read::MAX_AGGREGATE_BYTES);
                let Some(bytes) = bytes else {
                    drop(batch);
                    pending_error(
                        pending,
                        "RESULT_LIMIT",
                        "The collected page documents exceeded their byte limit.",
                    );
                    return;
                };
                batch.document_bytes = bytes;
                batch.documents.push(document);
                batch.collected.push(CollectedFrame {
                    snapshot: Shared::new(snapshot),
                    context: read.context,
                });
                drop(batch);
                next_page_document(pending);
                return;
            }
            STATE.with(|slot| {
                if let Some(control) = slot.borrow_mut().frame_controls.get_mut(&pending.tab) {
                    control.snapshots = vec![Shared::new(snapshot)];
                }
            });
            finish_frame_inventory_state(pending.tab, pending.reply.token);
            pending.reply.ok(json!({"tab":pending.tab,"epoch":pending.epoch,"documentEpoch":inventory.document_epoch,
                "frame":read.frame,"page":page,"atomic":false,
                "supportedActions":["click","hover","fill","insert","press","check","select"],"pointerActionsSupported":true}));
            return;
        }
        _ => return,
    };
    dispatch_frame_phase(Pending {
        step: Step::FrameInventory {
            inventory: inventory.clone(),
            session: session.clone(),
            phase: next_phase,
        },
        ..pending.clone()
    });
}

fn frame_event(browser: &Browser, session: Option<&str>, method: &str, bytes: &[u8]) {
    if !matches!(
        method,
        "Target.attachedToTarget"
            | "Target.detachedFromTarget"
            | "Page.frameAttached"
            | "Page.frameDetached"
            | "Page.frameNavigated"
            | "Runtime.executionContextCreated"
            | "Runtime.executionContextDestroyed"
            | "Runtime.executionContextsCleared"
            | "Inspector.targetCrashed"
            | "Inspector.detached"
            | "Inspector.targetReloadedAfterCrash"
    ) || (method.starts_with("Inspector.") && session.is_none())
    {
        // A root renderer exit keeps its RENDERER_TERMINATED path.
        return;
    }
    let tab = STATE.with(|slot| {
        let state = slot.borrow();
        state.tabs.iter().find_map(|(id, tab)| {
            (!tab.rejected_creation
                && same_browser(&tab.browser, &browser)
                && state.frame_controls.get(id).is_some_and(|control| {
                    control.enabled
                        && session.is_none_or(|session| control.sessions.contains(session))
                }))
            .then_some(*id)
        })
    });
    let Some(tab) = tab else {
        return;
    };
    if bytes.len() > 32 * 1024 {
        fail_frame_inventory(
            tab,
            "FRAME_LIMIT",
            "The frame event exceeded its size limit.",
        );
        stop_frame_tracking(tab, None);
        return;
    }
    let Ok(params) = serde_json::from_slice::<Json>(bytes) else {
        fail_frame_inventory(
            tab,
            "INVALID_RESULT",
            "Chromium returned an invalid frame event.",
        );
        stop_frame_tracking(tab, None);
        return;
    };
    let (changed, rejected, removed) = STATE.with(|slot| {
        let mut state = slot.borrow_mut();
        let Some(control) = state.frame_controls.get_mut(&tab) else {
            return (false, false, Vec::new());
        };
        // Keep the collection guard between Pending stages too: CEF getters
        // can deliver events after the preceding request has been removed.
        let runtime_event = method.starts_with("Runtime.");
        let mut changed = !runtime_event
            && control.active_inventory.as_ref().is_some_and(|inventory| {
                let current = inventory.current.borrow();
                inventory
                    .visited
                    .borrow()
                    .contains(&session.map(str::to_owned))
                    || (current.1 == FramePhase::Tree && current.0.as_deref() == session)
            });
        let mut rejected = false;
        let mut removed = Vec::new();
        let mut renderer_changed = false;
        match method {
            "Runtime.executionContextCreated"
            | "Runtime.executionContextDestroyed"
            | "Runtime.executionContextsCleared" => {
                if let Some(inventory) = &control.active_inventory {
                    if let Some(page_read) = &inventory.page_read {
                        for collected in &mut page_read.borrow_mut().collected {
                            if collected.snapshot.session_id() == session {
                                collected.context.observe(method, &params);
                                if collected.context.invalidated() {
                                    collected.snapshot.invalidated.set(true);
                                    changed = true;
                                }
                            }
                        }
                    }
                    if let Some(read) = inventory.read.borrow_mut().as_mut() {
                        // A parent pass may enable the same session as the
                        // target or another already-observed ancestor. Keep
                        // all bound worlds current across those passes.
                        if read.frame.session_id.as_deref() == session {
                            read.context.observe(method, &params);
                            changed |= read.context.invalidated();
                        }
                        for parent in &mut read.parents {
                            if parent.frame.session_id.as_deref() == session {
                                parent.context.observe(method, &params);
                                changed |= parent.context.invalidated();
                            }
                        }
                    }
                }
            }
            "Target.attachedToTarget" => match control.sessions.attach(session, &params) {
                crate::frame_sessions::AttachOutcome::Duplicate => {
                    changed = false;
                }
                crate::frame_sessions::AttachOutcome::Reject { .. } => {
                    rejected = true;
                }
                crate::frame_sessions::AttachOutcome::Added { .. } => {}
            },
            "Target.detachedFromTarget" => {
                removed = params["sessionId"]
                    .as_str()
                    .map(|id| control.sessions.detach(session, id))
                    .unwrap_or_default();
                changed = !removed.is_empty();
            }
            "Inspector.targetCrashed"
            | "Inspector.detached"
            | "Inspector.targetReloadedAfterCrash" => {
                // Chromium keeps the crashed child attached and queues its
                // renderer-bound commands until a reload; no detach follows.
                let id = session.unwrap_or_default();
                let crashed = method != "Inspector.targetReloadedAfterCrash";
                control.sessions.set_crashed(id, crashed);
                let probing = control.active_inventory.as_ref().is_some_and(|inventory| {
                    *inventory.current.borrow() == (Some(id.to_owned()), FramePhase::CrashProbe)
                });
                if !probing {
                    renderer_changed = true;
                    changed = control.active_inventory.as_ref().is_some_and(|inventory| {
                        inventory.current.borrow().0.as_deref() == Some(id)
                            || inventory.visited.borrow().contains(&Some(id.to_owned()))
                    });
                    if crashed {
                        // A focus lease there can never receive its disable ACK.
                        removed = vec![id.to_owned()];
                    }
                }
            }
            _ => {}
        }
        let snapshot_changed = match method {
            "Page.frameAttached" | "Page.frameDetached" | "Page.frameNavigated" => true,
            "Target.detachedFromTarget" => changed,
            _ => renderer_changed,
        };
        if changed || rejected || snapshot_changed {
            for snapshot in &control.snapshots {
                snapshot.invalidated.set(true);
            }
        }
        if changed || rejected {
            if let Some(inventory) = &control.active_inventory {
                inventory.invalidated.set(true);
            }
        }
        (changed, rejected, removed)
    });
    if !removed.is_empty() {
        focus_session_detached(browser, tab, &removed);
    }
    if rejected {
        fail_frame_inventory(
            tab,
            "FRAME_LIMIT",
            "A child frame attachment was invalid or exceeded the session limits.",
        );
        stop_frame_tracking(tab, None);
    } else if changed {
        fail_frame_inventory(
            tab,
            "FRAME_CHANGED",
            "Frames changed during discovery. Inspect again after the page settles.",
        );
        stop_frame_tracking(tab, None);
    }
}

fn dispatch(browser: &Browser, method: &str, mut params: Json, mut pending: Pending) {
    if matches!(pending.step, Step::Evaluate)
        && pending.fill_target().is_some_and(|target| !target.insert)
    {
        let inspect = pending
            .fill_target()
            .unwrap()
            .classification
            .borrow()
            .is_none();
        let Some(fill_params) = fill_dispatch_params(&pending, inspect) else {
            pending_error(
                &pending,
                "STALE_REF",
                "The original fill reference or context is unavailable.",
            );
            return;
        };
        params = fill_params;
        if inspect {
            pending.step = Step::FillPreflight;
        }
    }
    // Bind before a focus lease: its reservation checks the fill binding, and
    // insert reaches the lease on its first dispatch.
    if let Some(failure) = bind_fill_target(browser, &pending) {
        pending_error(&pending, failure.code, failure.message);
        return;
    }
    if focus_preparation(&pending) && pending.reply.focus_completion.is_none() {
        start_focus_lease(browser, method, params, pending);
        return;
    }
    let frame_failure = STATE.with(|slot| {
        pending.frame.as_ref().and_then(|frame| {
            frame_snapshot_failure(
                &slot.borrow(),
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                frame,
            )
        })
    });
    if let Some(failure) = frame_failure {
        pending_error(&pending, failure.code, failure.message);
        return;
    }
    if matches!(
        pending.step,
        Step::Frame(_) | Step::World(_) | Step::Evaluate
    ) && !STATE.with(|slot| {
        focus_input_active(
            &slot.borrow(),
            browser,
            pending.tab,
            &pending.reply,
            pending.session(),
        )
    }) {
        pending_error(
            &pending,
            "FOCUS_PREPARATION_FAILED",
            "Temporary input focus is no longer active; no input was dispatched.",
        );
        return;
    }
    if matches!(pending.evaluation, Evaluation::ReadyState { .. }) {
        dispatch_readiness_with(
            browser,
            pending,
            || browser.host(),
            || Some(params),
            |host, params, id| send_root_command(&host, id, method, &params),
        );
        return;
    }
    if !pending.reply.active.load(Ordering::Acquire) {
        if pending.reply.focus_completion.is_some() {
            abort_focus_lease(
                pending.tab,
                "TIMEOUT",
                "The input request is no longer active; do not repeat it automatically.",
                false,
            );
        }
        if matches!(pending.step, Step::FrameInventory { .. }) {
            stop_frame_tracking(pending.tab, Some(pending.reply.token));
        }
        return;
    }
    let access = STATE.with(|state| {
        state
            .borrow()
            .tasks
            .check_tab(pending.reply.scope.as_ref(), pending.tab)
    });
    if let Err(error) = access {
        pending_error(&pending, error.code, error.message);
        return;
    }
    let host = browser.host();
    let frame_failure = STATE.with(|slot| {
        pending.frame.as_ref().and_then(|frame| {
            frame_snapshot_failure(
                &slot.borrow(),
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                frame,
            )
        })
    });
    if let Some(failure) = frame_failure {
        pending_error(&pending, failure.code, failure.message);
        return;
    }
    let Some(host) = host else {
        pending_error(&pending, "TAB_CLOSED", "Browser host is unavailable.");
        return;
    };
    if !STATE.with(|slot| frame_inventory_current(&slot.borrow(), browser, &pending)) {
        pending_error(
            &pending,
            "FRAME_CHANGED",
            "The frame discovery target changed before dispatch.",
        );
        return;
    }
    let session = pending.session().map(str::to_owned);
    let capture = matches!(pending.step, Step::Screenshot(_));
    if capture && STATE.with(|state| !state.borrow().captures.is_empty()) {
        pending.reply.error(
            "CAPTURE_BUSY",
            "Wait for the current screenshot capture to finish.",
        );
        return;
    }
    if matches!(
        pending.evaluation,
        Evaluation::Page {
            evidence: Some(
                ActionEvidence::Press(_) | ActionEvidence::Click(_) | ActionEvidence::Fill(_)
            ),
            ..
        }
    ) {
        let available = browser.is_valid() != 0 && browser.has_document() != 0;
        let failure = STATE.with(|state| pending_input_failure(&state.borrow(), browser, &pending));
        if let Some(failure) = failure {
            pending_error(&pending, failure.code, failure.message);
            return;
        }
        if !available {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "The browser is unavailable for input preparation.",
            );
            return;
        }
    }
    // Host/document getters above can re-enter cancellation while the disable
    // ACK is still pending. Do not start a fresh DOM focus/select preparation.
    if matches!(
        pending.step,
        Step::Frame(_) | Step::World(_) | Step::Evaluate
    ) && !STATE.with(|slot| {
        focus_input_active(
            &slot.borrow(),
            browser,
            pending.tab,
            &pending.reply,
            pending.session(),
        )
    }) {
        pending_error(
            &pending,
            "FOCUS_PREPARATION_FAILED",
            "Temporary input focus is no longer active; no input was dispatched.",
        );
        return;
    }
    let reserved = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(id) = state.next_control_message() else {
            return Err(pending);
        };
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
        Ok((id, start_page_revoked))
    });
    let (id, start_page_revoked) = match reserved {
        Ok(reserved) => reserved,
        Err(pending) => {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "The control message identifiers are exhausted.",
            );
            return;
        }
    };
    let _refresh = PanelRefresh(start_page_revoked);
    if send_session_command(&host, id, method, &params, session.as_deref()) != id {
        if let Some(pending) = complete_control_request(id) {
            pending_error(
                &pending,
                "CONTROL_UNAVAILABLE",
                "Chromium rejected the control request.",
            );
        }
    }
}

/// Keep our identifier in the envelope. The typed CEF sender can allocate an
/// identifier itself; root and flat child sessions share this raw transport.
fn send_root_command(host: &BrowserHost, id: i32, method: &str, params: &Json) -> i32 {
    send_session_command(host, id, method, params, None)
}

fn send_session_command(
    host: &BrowserHost,
    id: i32,
    method: &str,
    params: &Json,
    session: Option<&str>,
) -> i32 {
    let bytes = match session {
        Some(session) => {
            crate::devtools_wire::serialize_session_request(id, method, params, Some(session))
        }
        None => crate::devtools_wire::serialize_request(id, method, params),
    };
    let Some(bytes) = bytes else {
        return 0;
    };
    if host.send_dev_tools_message(Some(&bytes)) != 0 {
        id
    } else {
        0
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

fn response_matches(state: &State, browser: &Browser, id: i32, session: Option<&str>) -> bool {
    // No CEF getter or removal while correlating. An unrelated/late response
    // must not release another tab's capture, preview or pending command.
    let tab = state
        .pending
        .get(&id)
        .filter(|pending| pending.session() == session)
        .map(|pending| pending.tab)
        .or_else(|| {
            state.focus_leases.iter().find_map(|(tab, lease)| {
                (same_browser(&lease.browser, &browser)
                    && lease.session.as_deref() == session
                    && (lease.enable.id == id
                        || lease
                            .disable
                            .as_ref()
                            .is_some_and(|message| message.id == id)))
                .then_some(*tab)
            })
        })
        .or_else(|| {
            session
                .is_none()
                .then(|| state.previews.get(&id).map(|preview| preview.tab))
                .flatten()
        })
        .or_else(|| {
            session
                .is_none()
                .then(|| state.captures.get(&id).copied())
                .flatten()
        })
        .or_else(|| {
            session
                .is_none()
                .then(|| {
                    state.frame_controls.iter().find_map(|(tab, control)| {
                        (control.cleanup == Some(id)
                            || control.runtime_cleanup == Some(id)
                            || control.object_cleanup == Some(id))
                        .then_some(*tab)
                    })
                })
                .flatten()
        });
    tab.and_then(|tab| state.tabs.get(&tab))
        .is_some_and(|tab| !tab.rejected_creation && same_browser(&tab.browser, &browser))
}

fn prefix_read_references(page: &mut Json, epoch: u64) {
    if !page.is_object() {
        return;
    }
    let prefix = format!("{epoch}/");
    let focused = page["focusedRef"]
        .as_str()
        .filter(|reference| {
            !reference.is_empty()
                && prefix.len() + reference.len() <= 256
                && !reference.chars().any(char::is_control)
                && page["interactive"].as_array().is_some_and(|elements| {
                    elements
                        .iter()
                        .any(|element| element["ref"].as_str() == Some(*reference))
                })
        })
        .map(|reference| format!("{prefix}{reference}"));
    if let Some(elements) = page["interactive"].as_array_mut() {
        for element in elements {
            if let Some(reference) = element["ref"].as_str() {
                element["ref"] = json!(format!("{prefix}{reference}"));
            }
        }
    }
    // Focus is useful only as a reference from this exact read. Missing,
    // unsupported or malformed focus never becomes an actionable arbitrary ref.
    page["focusedRef"] = json!(focused);
}

fn devtools_result(browser: &Browser, id: i32, success: bool, bytes: &[u8]) {
    if complete_focus_message(browser, id, success, bytes) {
        return;
    }
    if complete_frame_cleanup(browser, id, success) {
        return;
    }
    if complete_preview(id, success, bytes) {
        return;
    }
    if input_result(browser, id, success, bytes)
        || input_preparation_result(browser, id, success, bytes)
    {
        return;
    }
    let Some(pending) = complete_control_request(id) else {
        return;
    };
    if matches!(pending.step, Step::FrameInventory { .. }) {
        frame_inventory_result(browser, &pending, success, bytes);
        return;
    }
    let frame_failure = STATE.with(|slot| {
        pending.frame.as_ref().and_then(|snapshot| {
            frame_snapshot_failure(
                &slot.borrow(),
                browser,
                pending.tab,
                pending.epoch,
                &pending.reply,
                snapshot,
            )
        })
    });
    if let Some(failure) = frame_failure {
        pending_error(&pending, failure.code, failure.message);
        return;
    }
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
    if matches!(pending.evaluation, Evaluation::ReadyState { .. }) {
        let current = STATE.with(|slot| {
            let state = slot.borrow();
            state
                .waits
                .get(&pending.reply.token)
                .is_some_and(|waiter| pending_matches_waiter(&pending, waiter))
                && state.tabs.get(&pending.tab).is_some_and(|tab| {
                    !tab.rejected_creation && same_browser(&tab.browser, &browser)
                })
        });
        if !current {
            pending_error(
                &pending,
                "PAGE_CHANGED",
                "The wait callback belongs to an older browser.",
            );
            return;
        }
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
    if matches!(pending.step, Step::FillPreflight) && bytes.len() > 64 * 1024 {
        pending_error(
            &pending,
            "INVALID_RESULT",
            "The fill classification exceeded its result limit.",
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
        Step::FrameInventory { .. } => {
            unreachable!("frame inventory handled before page evaluation")
        }
        Step::FillPreflight => {
            if result.get("exceptionDetails").is_some() {
                pending_error(
                    &pending,
                    "PAGE_SCRIPT_ERROR",
                    "The fill target could not be classified.",
                );
                return;
            }
            let value = &result["result"]["value"];
            if value["ok"] == false {
                // Classification is read-only and precedes focus or dispatch.
                let (code, message) = fill_error(value);
                pending.reply.error_details(
                    code,
                    message,
                    json!({"attempted":false,"outcomeUnknown":false}),
                );
                return;
            }
            let path = value["input"].as_str();
            let kind = value["kind"].as_str();
            let valid = value["ok"] == true
                && value["action"] == "fill"
                && match (path, kind) {
                    (
                        Some("chromium"),
                        Some(
                            "text" | "search" | "tel" | "url" | "password" | "textarea"
                            | "contenteditable",
                        ),
                    ) => true,
                    (
                        Some("dom"),
                        Some(
                            "email" | "number" | "date" | "datetime-local" | "month" | "week"
                            | "time" | "range" | "color",
                        ),
                    ) => true,
                    _ => false,
                };
            if !valid {
                pending_error(
                    &pending,
                    "INVALID_PAGE_RESULT",
                    "The fill target returned an unsupported classification.",
                );
                return;
            }
            *pending.fill_target().unwrap().classification.borrow_mut() =
                Some((path.unwrap().to_owned(), kind.unwrap().to_owned()));
            dispatch(
                browser,
                "Runtime.evaluate",
                json!({}),
                Pending {
                    step: Step::Evaluate,
                    ..pending
                },
            );
        }
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
            if let Some(target) = pending.click_target() {
                target.context.set(Some(context));
            }
            if let Some(target) = pending.press_target() {
                if context <= 0 {
                    pending_error(
                        &pending,
                        "CONTEXT_UNAVAILABLE",
                        "The key input context is unavailable.",
                    );
                    return;
                }
                target.context.set(Some(context));
            }
            if let Some(target) = pending.fill_target() {
                if context <= 0
                    || target
                        .context
                        .get()
                        .is_some_and(|original| original != context)
                {
                    pending_error(
                        &pending,
                        "CONTEXT_UNAVAILABLE",
                        "The original fill context is unavailable.",
                    );
                    return;
                }
                target.context.set(Some(context));
            }
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
            if let Evaluation::ReadyState { token, .. } = pending.evaluation {
                let within_deadline = STATE.with(|state| {
                    let state = state.borrow();
                    state.waits.get(&token).is_some_and(|waiter| {
                        pending_matches_waiter(&pending, waiter)
                            && waiter.started.elapsed() < Duration::from_millis(waiter.timeout_ms)
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
                    finish_dom_wait_with(browser, &pending, token, value, observe_tab);
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
                } = &pending.evaluation
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
                    prefix_read_references(&mut page, pending.epoch);
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
        Step::Input { .. } | Step::ClickVerify | Step::FillVerify | Step::PressVerify => {
            unreachable!("Native input results are handled before completing a pending request")
        }
    }
}

wrap_app! {
    pub struct BrowserApp;
    impl App {
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            // command_line_args_disabled blocks real arguments, so verification builds add this switch here.
            let browser_process = process_type.is_none_or(|kind| kind.to_string().is_empty());
            if browser_process && crate::config::mock_keychain().unwrap_or(false) {
                if let Some(command_line) = command_line { command_line.append_switch(Some(&"use-mock-keychain".into())); }
            }
        }
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
                // DevTools envelopes, including teardown's child-session
                // detaches, are handled in posted UI tasks. Stop observing now;
                // tasks still queued after OnBeforeClose removes the tab and
                // answers its pending requests fail re-validation and do nothing.
                let devtools = STATE.with(|state| state.borrow_mut().tabs.get_mut(&browser.identifier()).and_then(|tab| tab._devtools.take()));
                drop(devtools);
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
            let (empty, waits, pending, focus)=STATE.with(|state| {
                let mut state=state.borrow_mut();
                if let Some(url)=url.as_deref() { remember_closed_page(&mut state,id,url); }
                state.page_find.invalidate(id);
                state.tabs.remove(&id);
                state.frame_controls.remove(&id);
                let focus=state.focus_leases.remove(&id);
                state.creations.close_opener(id);
                state.captures.retain(|_,tab|*tab!=id);
                state.previews.retain(|_,request|request.tab!=id);
                state.tasks.tab_closed(id);
                state.spaces.remove(id);
                let tasks = state.tasks.list();
                state.spaces.update_tasks(&tasks);
                let wait_ids=state.waits.iter().filter_map(|(key,waiter)|(waiter.tab==id).then_some(*key)).collect::<Vec<_>>();
                let pending_ids=state.pending.iter().filter_map(|(key,pending)|(pending.tab==id).then_some(*key)).collect::<Vec<_>>();
                let waits=wait_ids.into_iter().filter_map(|key|state.waits.remove(&key)).collect::<Vec<_>>();
                let pending=pending_ids.into_iter().filter_map(|key|state.pending.remove(&key)).collect::<Vec<_>>();
                (state.tabs.is_empty(), waits, pending, focus)
            });
            // Destruction of the browser destroys its sessions. Reply only
            // outside STATE, and do not enter focus/frame cleanup recursively.
            if let Some(lease)=focus {
                lease.reply.send_final(lease.failure("TAB_CLOSED","The tab closed during input. Inspect before retrying.","session-destroyed"));
            }
            for waiter in waits {
                waiter.reply.send_final(Response::error(&waiter.reply.id,"TAB_CLOSED","The tab closed while waiting."));
            }
            for pending in pending {
                let mut response=Response::error(&pending.reply.id,"TAB_CLOSED","The tab closed during the request. Inspect before retrying.");
                if pending.mutation_dispatched() { response=response.with_details(pending.outcome_details()); }
                pending.reply.send_final(response);
            }
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
            }); focus_navigation_changed(browser.identifier()); queue_history_observation(browser.identifier()); notify_panel(); }
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
                // Cleanup sends DevTools commands into the dying agent; run it
                // after this callback, like ControlObserver envelopes.
                let mut task = RendererTerminatedTask::new(browser.clone(), json!({"status":name,"rawStatus":status.get_raw(),"errorCode":error_code,
                    "message":error_string.map(|value|value.to_string().chars().take(240).collect::<String>()).unwrap_or_default()}));
                post_task(ThreadId::UI, Some(&mut task));
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
                    focus_navigation_changed(browser.identifier());
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
            if let Some(browser) = browser {
                let tab = browser.identifier();
                observe_loading_state(tab, is_loading != 0);
                if is_loading == 0 { queue_history_observation(tab); }
            }
            notify_panel();
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
            }); focus_navigation_changed(browser.identifier()); notify_panel(); }
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
        // Chromium calls these while iterating DevTools observer lists, also
        // inside child-session teardown. A command sent from here re-enters
        // that iteration and can free the session still on the stack. Handle
        // each envelope in its own UI task; posting order keeps protocol order.
        fn on_dev_tools_message(&self,browser:Option<&mut Browser>,message:Option<&[u8]>) -> i32 {
            if let (Some(browser), Some(message)) = (browser, message) {
                post_control_event(browser, Some(message.to_vec()));
            }
            // Consume every envelope. Typed CEF parsing drops session identity
            // and assumes field order.
            1
        }
        fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>) {
            if let Some(browser) = browser {
                post_control_event(browser, None);
            }
        }
    }
}

wrap_task! {
    // `None` reports that the DevTools agent detached.
    struct ControlEventTask { browser: Browser, message: Option<Vec<u8>> }
    impl Task {
        fn execute(&self) {
            match &self.message {
                Some(message) => control_message(&self.browser, message),
                None => {
                    let id = self.browser.identifier();
                    if STATE.with(|state| state.borrow().tabs.get(&id).is_some_and(|tab| same_browser(&tab.browser, &self.browser))) {
                        control_detached(id);
                    }
                }
            }
        }
    }
}

fn post_control_event(browser: &Browser, message: Option<Vec<u8>>) {
    let mut task = ControlEventTask::new(browser.clone(), message);
    // A rejected post drops the envelope; handling it inline would re-enter.
    post_task(ThreadId::UI, Some(&mut task));
}

fn control_message(browser: &Browser, message: &[u8]) {
    let Some(message) = crate::devtools_wire::incoming_message(message) else {
        return;
    };
    match message {
        crate::devtools_wire::Incoming::Response {
            id,
            session,
            success,
            payload,
        } => {
            if STATE.with(|state| response_matches(&state.borrow(), browser, id, session)) {
                devtools_result(browser, id, success, payload);
            }
        }
        crate::devtools_wire::Incoming::Event {
            session,
            method,
            params,
        } => {
            frame_event(browser, session, method, params);
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

    fn inert_frame() -> Frame {
        let raw = cef::rc::RcImpl::new(unsafe { std::mem::zeroed::<sys::_cef_frame_t>() }, ());
        cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_frame_t>())
    }

    fn test_observation(
        tab: i32,
        reply: &Reply,
        access: ObservationAccess,
    ) -> Result<TabObservation, TaskError> {
        observe_tab_with(
            tab,
            reply,
            access,
            |_| None,
            |_| unreachable!(),
            |_, _| false,
        )
    }

    fn install_wait(reply: Reply, until: WaitUntil) -> Waiter {
        let waiter = Waiter {
            tab: 7,
            until,
            timeout_ms: 1_000,
            started: Instant::now(),
            probe_in_flight: false,
            reply,
        };
        STATE.with(|slot| {
            slot.borrow_mut()
                .waits
                .insert(waiter.reply.token, waiter.clone());
        });
        waiter
    }

    #[test]
    fn cli_observation_releases_state_and_keeps_actual_url_distinct_from_intent() {
        context_menu_fixture(false);
        let (reply, _) = test_reply(17);
        let calls = Cell::new(0);
        let observed = observe_tab_with(
            7,
            &reply,
            ObservationAccess::Tab,
            |_| {
                STATE.with(|slot| drop(slot.try_borrow_mut().unwrap()));
                Some(inert_frame())
            },
            |_| {
                STATE.with(|slot| drop(slot.try_borrow_mut().unwrap()));
                "https://example.test/actual".into()
            },
            |_, kind| {
                calls.set(calls.get() + 1);
                STATE.with(|slot| {
                    let mut state = slot.try_borrow_mut().unwrap();
                    state.tabs.get_mut(&7).unwrap().title = "new title".into();
                    state.spaces.select_tab(8);
                });
                matches!(kind, ShellProbe::Back)
            },
        )
        .unwrap();
        let info = observed_tab_info(&reply, &observed).unwrap();
        assert_eq!(calls.get(), 3);
        assert_eq!(info["url"], "https://example.test/actual");
        assert_eq!(
            info["navigation"]["requestedUrl"],
            "https://example.test/menu"
        );
        assert_eq!(info["title"], "new title");
        assert_eq!(info["canGoBack"], true);
        assert_eq!(info["loading"], false);
        clear();
    }

    #[test]
    fn cli_observation_stops_after_each_reentrant_native_boundary() {
        // main_frame, frame.url, loading, back and forward all permit callbacks.
        for boundary in 1..=5 {
            for change in 0..6 {
                context_menu_fixture(false);
                let (reply, _) = test_reply(17);
                let calls = Cell::new(0);
                let touch = || {
                    calls.set(calls.get() + 1);
                    STATE.with(|slot| {
                        let mut state = slot.try_borrow_mut().unwrap();
                        if calls.get() != boundary {
                            return;
                        }
                        match change {
                            0 => {
                                state.tabs.remove(&7);
                            }
                            1 => {
                                state.tabs.get_mut(&7).unwrap().browser =
                                    native_blank_tab().browser;
                            }
                            2 => state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
                            3 => state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
                            4 => {
                                state
                                    .tabs
                                    .get_mut(&7)
                                    .unwrap()
                                    .navigation
                                    .loading_event_revision += 1
                            }
                            _ => {
                                let task = state.tasks.create("changed owner").unwrap();
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
                    });
                };
                let result = observe_tab_with(
                    7,
                    &reply,
                    ObservationAccess::Tab,
                    |_| {
                        touch();
                        Some(inert_frame())
                    },
                    |_| {
                        touch();
                        "https://example.test/actual".into()
                    },
                    |_, _| {
                        touch();
                        false
                    },
                );
                assert!(result.is_err(), "boundary={boundary} change={change}");
                assert_eq!(calls.get(), boundary);
            }
        }
        clear();
    }

    #[test]
    fn cli_tabs_rechecks_scope_full_membership_and_earlier_samples() {
        let scope = context_menu_fixture(true).unwrap();
        let (mut scoped, _) = test_reply(17);
        scoped.scope = Some(scope.clone());
        let info = tabs_info_with(&scoped, test_observation).unwrap();
        assert_eq!(info["tabs"].as_array().unwrap().len(), 1);
        assert_eq!(info["tabs"][0]["tab"], 7);
        let (unscoped, _) = test_reply(18);
        assert_eq!(
            tabs_info_with(&unscoped, test_observation).unwrap()["tabs"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            tabs_info_with(&scoped, |id, reply, access| {
                let observed = test_observation(id, reply, access)?;
                STATE.with(|slot| {
                    slot.borrow_mut().tasks.handoff(&scope).unwrap();
                });
                Ok(observed)
            })
            .is_err()
        );
        for change in 0..3 {
            context_menu_fixture(false);
            let (reply, _) = test_reply(19);
            let result = tabs_info_with(&reply, |id, reply, access| {
                let observed = test_observation(id, reply, access)?;
                if id == 8 {
                    STATE.with(|slot| {
                        let mut state = slot.borrow_mut();
                        match change {
                            0 => state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
                            1 => {
                                state.tabs.insert(9, native_blank_tab());
                            }
                            _ => {
                                state.tabs.remove(&7);
                            }
                        }
                    });
                }
                Ok(observed)
            });
            assert_eq!(result.err().unwrap().code, "PAGE_CHANGED");
        }
        clear();
    }

    #[test]
    fn created_tab_observation_failure_reports_creation_without_recreating() {
        context_menu_fixture(false);
        let browser = STATE.with(|slot| slot.borrow().tabs[&7].browser.clone());
        let (reply, receiver) = test_reply(17);
        reply_created_tab_with(7, &browser, &reply, |id, reply, access| {
            observe_tab_with(
                id,
                reply,
                access,
                |_| {
                    STATE.with(|slot| {
                        slot.borrow_mut().tabs.get_mut(&7).unwrap().navigation.epoch += 1
                    });
                    None
                },
                |_| unreachable!(),
                |_, _| panic!("stale frame lookup must stop observation"),
            )
        });
        let error = receiver.try_recv().unwrap().error.unwrap();
        assert_eq!(error.code, "PAGE_CHANGED");
        assert_eq!(
            error.details.unwrap(),
            json!({"tab":7,"tabCreated":true,"stateUnavailable":true,"outcomeUnknown":false})
        );
        STATE.with(|slot| assert_eq!(slot.borrow().tabs.len(), 2));
        assert!(receiver.try_recv().is_err());
        clear();
    }

    #[test]
    fn wait_unstable_observation_keeps_deadline_and_terminal_diagnostics() {
        for terminal in 0..3 {
            context_menu_fixture(false);
            let (reply, receiver) = test_reply(17);
            let captured = install_wait(reply, WaitUntil::Load);
            assert!(poll_wait_with(17, |_, _, _| Err(OBSERVATION_CHANGED)).is_none());
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                assert_eq!(state.waits[&17].started, captured.started);
                assert!(!state.waits[&17].probe_in_flight);
                match terminal {
                    0 => state.waits.get_mut(&17).unwrap().timeout_ms = 0,
                    1 => {
                        state.tabs.get_mut(&7).unwrap().navigation.error =
                            Some(json!({"error":"failed"}))
                    }
                    _ => {
                        state.tabs.get_mut(&7).unwrap().navigation.renderer_exit =
                            Some(json!({"status":"crashed"}))
                    }
                }
            });
            assert!(receiver.try_recv().is_err());
            poll_wait_cycle_with(
                17,
                |_, _, _| Err(OBSERVATION_CHANGED),
                |_, _| panic!("no DOM probe"),
                |_| panic!("finished wait must not schedule"),
            );
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                ["WAIT_TIMEOUT", "NAVIGATION_FAILED", "RENDERER_TERMINATED"][terminal]
            );
            let details = error.details.unwrap();
            assert!(details["state"].is_null());
            assert_eq!(details["stateUnavailable"], true);
            STATE.with(|slot| assert!(!slot.borrow().waits.contains_key(&17)));
            assert!(receiver.try_recv().is_err());
        }
        context_menu_fixture(false);
        let (reply, receiver) = test_reply(17);
        install_wait(reply, WaitUntil::Load);
        poll_wait_cycle_with(
            17,
            test_observation,
            |_, _| panic!("load uses no CDP"),
            |_| panic!("completed"),
        );
        let info = receiver.try_recv().unwrap().result.unwrap();
        assert_eq!(info["navigation"]["epoch"], 4);
        assert!(info.get("stateUnavailable").is_none());
        clear();
    }

    #[test]
    fn wait_replacement_is_not_reset_removed_or_scheduled_by_old_cycle() {
        for boundary in 0..3 {
            context_menu_fixture(false);
            let (reply, receiver) = test_reply(17);
            let old = install_wait(reply, WaitUntil::Dom);
            let (replacement_reply, _) = test_reply(17);
            let replacement = Waiter {
                reply: replacement_reply,
                probe_in_flight: true,
                ..old.clone()
            };
            let replace = || {
                STATE.with(|slot| {
                    slot.borrow_mut().waits.insert(17, replacement.clone());
                });
            };
            let sent = Cell::new(0);
            let scheduled = Cell::new(0);
            poll_wait_cycle_with(
                17,
                |id, reply, access| {
                    let observed = test_observation(id, reply, access)?;
                    if boundary == 0 {
                        replace();
                    }
                    Ok(observed)
                },
                |_, pending| {
                    sent.set(sent.get() + 1);
                    if boundary == 1 {
                        replace();
                    }
                    pending_error(&pending, "PAGE_CHANGED", "old probe");
                },
                |_| {
                    scheduled.set(scheduled.get() + 1);
                    assert_eq!(boundary, 2);
                    replace();
                    false
                },
            );
            assert_eq!(sent.get(), usize::from(boundary != 0));
            assert_eq!(scheduled.get(), usize::from(boundary == 2));
            assert!(!wait_tick_current(
                17,
                old.tab,
                old.started,
                &old.reply.active
            ));
            assert!(remove_matching_wait(17, &old).is_none());
            STATE.with(|slot| {
                let state = slot.borrow();
                assert!(same_waiter(&state.waits[&17], &replacement));
                assert!(state.waits[&17].probe_in_flight);
            });
            assert!(receiver.try_recv().is_err());
        }
        clear();
    }

    #[test]
    fn dom_wait_completion_preserves_probe_deadline_and_browser_identity() {
        context_menu_fixture(false);
        let (reply, receiver) = test_reply(17);
        let old = install_wait(reply, WaitUntil::Dom);
        let (browser, pending) = poll_wait_with(17, test_observation).unwrap();
        assert!(
            poll_wait_with(17, test_observation).is_none(),
            "one probe only"
        );
        finish_dom_wait_with(&browser, &pending, 17, &json!("complete"), |_, _, _| {
            Err(OBSERVATION_CHANGED)
        });
        STATE.with(|slot| {
            let state = slot.borrow();
            assert_eq!(state.waits[&17].started, old.started);
            assert!(!state.waits[&17].probe_in_flight);
        });
        assert!(receiver.try_recv().is_err());
        let (_, pending) = poll_wait_with(17, test_observation).unwrap();
        // An old browser callback cannot certify a replacement with the same ID/epoch.
        STATE.with(|slot| {
            slot.borrow_mut().tabs.get_mut(&7).unwrap().browser = native_blank_tab().browser
        });
        finish_dom_wait_with(&browser, &pending, 17, &json!("complete"), test_observation);
        assert!(receiver.try_recv().is_err());
        let (browser, pending) = poll_wait_with(17, test_observation).unwrap();
        STATE.with(|slot| {
            slot.borrow_mut().pending.insert(42, pending.clone());
        });
        let replaced = native_blank_tab().browser;
        STATE.with(|slot| slot.borrow_mut().tabs.get_mut(&7).unwrap().browser = replaced.clone());
        devtools_result(&browser, 42, true, br#"{"result":{"value":"complete"}}"#);
        assert!(receiver.try_recv().is_err());
        let (_, pending) = poll_wait_with(17, test_observation).unwrap();
        STATE.with(|slot| slot.borrow_mut().waits.get_mut(&17).unwrap().timeout_ms = 0);
        finish_dom_wait_with(
            &replaced,
            &pending,
            17,
            &json!("complete"),
            test_observation,
        );
        assert!(receiver.try_recv().is_err());
        STATE.with(|slot| assert!(!slot.borrow().waits[&17].probe_in_flight));
        clear();
        context_menu_fixture(false);
        let (reply, receiver) = test_reply(18);
        install_wait(reply, WaitUntil::Dom);
        let (browser, pending) = poll_wait_with(18, test_observation).unwrap();
        finish_dom_wait_with(
            &browser,
            &pending,
            18,
            &json!("interactive"),
            test_observation,
        );
        assert_eq!(
            receiver.try_recv().unwrap().result.unwrap()["readyState"],
            "interactive"
        );
        finish_dom_wait_with(&browser, &pending, 18, &json!("complete"), |_, _, _| {
            panic!("completed waiter must not observe")
        });
        assert!(receiver.try_recv().is_err());
        clear();
    }

    #[test]
    fn readiness_dispatch_revalidates_native_preparation_and_releases_only_its_waiter() {
        for boundary in 0..2 {
            for change in 0..6 {
                context_menu_fixture(false);
                let (reply, receiver) = test_reply(17);
                let old = install_wait(reply, WaitUntil::Dom);
                let (browser, pending) = poll_wait_with(17, test_observation).unwrap();
                let (replacement_reply, _) = test_reply(17);
                let replacement = Waiter {
                    reply: replacement_reply,
                    probe_in_flight: true,
                    ..old.clone()
                };
                let mutate = || {
                    STATE.with(|slot| {
                        let mut state = slot.try_borrow_mut().unwrap();
                        match change {
                            0 => {
                                state.waits.insert(17, replacement.clone());
                            }
                            1 => {
                                state.tabs.get_mut(&7).unwrap().browser = native_blank_tab().browser
                            }
                            2 => state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
                            3 => {
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
                            4 => state.waits.get_mut(&17).unwrap().timeout_ms = 0,
                            _ => old.reply.active.store(false, Ordering::Release),
                        }
                    });
                };
                let params = Cell::new(0);
                dispatch_readiness_with(
                    &browser,
                    pending,
                    || {
                        if boundary == 0 {
                            mutate();
                        }
                        Some(())
                    },
                    || {
                        params.set(params.get() + 1);
                        if boundary == 1 {
                            mutate();
                        }
                        Some(())
                    },
                    |_, _, _| panic!("stale probe must not reach Chromium"),
                );
                assert_eq!(params.get(), boundary);
                STATE.with(|slot| {
                    let state = slot.borrow();
                    assert!(state.pending.is_empty());
                    assert_eq!(state.waits[&17].probe_in_flight, change == 0);
                });
                assert!(receiver.try_recv().is_err());
            }
        }
        for accepted in [false, true] {
            context_menu_fixture(false);
            let (reply, receiver) = test_reply(17);
            install_wait(reply, WaitUntil::Dom);
            let (browser, pending) = poll_wait_with(17, test_observation).unwrap();
            let sent = Cell::new(0);
            dispatch_readiness_with(
                &browser,
                pending,
                || Some(()),
                || Some(()),
                |_, _, id| {
                    STATE.with(|slot| {
                        assert!(slot.try_borrow_mut().unwrap().pending.contains_key(&id))
                    });
                    sent.set(sent.get() + 1);
                    if accepted { id } else { 0 }
                },
            );
            assert_eq!(sent.get(), 1);
            STATE.with(|slot| {
                let state = slot.borrow();
                assert_eq!(state.pending.len(), usize::from(accepted));
                assert_eq!(state.waits[&17].probe_in_flight, accepted);
            });
            assert!(
                receiver.try_recv().is_err(),
                "dispatch ACK is not DOM readiness"
            );
        }
        clear();
    }

    #[test]
    fn navigate_native_paused_owner_and_background_cli_keep_distinct_access() {
        for native in [false, true] {
            let scope = context_menu_fixture(true).unwrap();
            STATE.with(|slot| {
                slot.borrow_mut().tasks.handoff(&scope).unwrap();
            });
            let (reply, receiver) = test_reply(17);
            let browser = STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                let tab = state.tabs.get_mut(&7).unwrap();
                tab.navigation.renderer_exit = Some(json!({"status":"crashed"}));
                tab.navigation.document_available = false;
                tab.browser.clone()
            });
            let sent = Cell::new(0);
            let cleaned = Cell::new(0);
            navigate_with(
                &browser,
                &Command::Reload { tab: 7 },
                &reply,
                if native {
                    ObservationAccess::Native
                } else {
                    ObservationAccess::Tab
                },
                test_observation,
                |_| {
                    cleaned.set(cleaned.get() + 1);
                    Ok(())
                },
                |observed, _| {
                    assert!(
                        observed.frame.is_none(),
                        "reload recovery requires no frame"
                    );
                    STATE.with(|slot| {
                        let state = slot.try_borrow_mut().unwrap();
                        assert_eq!(state.tabs[&7].navigation.epoch, 5);
                        assert!(state.tabs[&7].navigation.intent_pending);
                    });
                    sent.set(sent.get() + 1);
                },
            );
            assert_eq!(sent.get(), usize::from(native));
            assert_eq!(cleaned.get(), usize::from(native));
            let response = receiver.try_recv().unwrap();
            if native {
                assert_eq!(response.result.unwrap()["navigationRequested"], true);
            } else {
                assert_eq!(response.error.unwrap().code, "TASK_REQUIRED");
            }
        }
        context_menu_fixture(false);
        let (reply, receiver) = test_reply(17);
        let browser = STATE.with(|slot| slot.borrow().tabs[&7].browser.clone());
        navigate_with(
            &browser,
            &Command::Reload { tab: 7 },
            &reply,
            ObservationAccess::Tab,
            test_observation,
            |_| {
                STATE.with(|slot| {
                    slot.borrow_mut().spaces.select_tab(8);
                });
                Ok(())
            },
            |_, _| {},
        );
        assert!(
            receiver.try_recv().unwrap().ok,
            "CLI may navigate a background tab"
        );
        clear();
    }

    #[test]
    fn navigate_rejects_before_cleanup_and_never_reserves_after_reentrant_change() {
        for rejected in 0..4 {
            context_menu_fixture(false);
            let (reply, receiver) = test_reply(17);
            let browser = STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                if rejected == 0 {
                    state.tabs.get_mut(&7).unwrap().navigation.intent_pending = true;
                }
                state.tabs[&7].browser.clone()
            });
            let command = match rejected {
                0 => Command::Reload { tab: 7 },
                1 => Command::Back { tab: 7 },
                2 => Command::Navigate {
                    tab: 7,
                    url: "invalid".into(),
                },
                _ => Command::Navigate {
                    tab: 7,
                    url: "https://example.test/next".into(),
                },
            };
            navigate_with(
                &browser,
                &command,
                &reply,
                ObservationAccess::Tab,
                test_observation,
                |_| panic!("invalid or busy request must not clean up"),
                |_, _| panic!("must not send"),
            );
            assert_eq!(
                receiver.try_recv().unwrap().error.unwrap().code,
                [
                    "NAVIGATION_BUSY",
                    "NO_HISTORY",
                    "INVALID_URL",
                    "FRAME_UNAVAILABLE"
                ][rejected]
            );
            STATE.with(|slot| assert_eq!(slot.borrow().tabs[&7].navigation.epoch, 4));
        }
        for change in 0..5 {
            context_menu_fixture(false);
            let (reply, receiver) = test_reply(17);
            let browser = STATE.with(|slot| slot.borrow().tabs[&7].browser.clone());
            navigate_with(
                &browser,
                &Command::Reload { tab: 7 },
                &reply,
                ObservationAccess::Native,
                test_observation,
                |_| {
                    STATE.with(|slot| {
                        let mut state = slot.try_borrow_mut().unwrap();
                        match change {
                            0 => state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
                            1 => {
                                state.spaces.select_tab(8);
                            }
                            2 => {
                                state.tabs.get_mut(&7).unwrap().browser = native_blank_tab().browser
                            }
                            3 => {
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
                            _ => {
                                state
                                    .tabs
                                    .get_mut(&7)
                                    .unwrap()
                                    .navigation
                                    .loading_event_revision += 1
                            }
                        }
                    });
                    Ok(())
                },
                |_, _| panic!("cleanup reentry must prevent transmission"),
            );
            assert!(receiver.try_recv().unwrap().error.is_some());
            STATE.with(|slot| {
                let state = slot.borrow();
                assert_eq!(state.tabs[&7].navigation.epoch, 4);
                assert!(!state.tabs[&7].navigation.intent_pending);
            });
        }
        clear();
    }

    #[test]
    fn shell_snapshot_releases_state_for_getters_and_projects_latest_selection_and_task() {
        context_menu_fixture(false);
        let calls = Cell::new(0);
        let snapshot = shell_snapshot_with(|_, kind| {
            calls.set(calls.get() + 1);
            STATE.with(|slot| {
                let mut state = slot
                    .try_borrow_mut()
                    .expect("native getter must not overlap STATE borrow");
                state.tabs.get_mut(&7).unwrap().title = "latest title".into();
                if calls.get() == 6 {
                    let task = state.tasks.create("latest owner").unwrap();
                    let scope = TaskScope {
                        task: task.id,
                        revision: task.revision,
                    };
                    state.tasks.attach(&scope, 8, Origin::Borrowed).unwrap();
                    let tasks = state.tasks.list();
                    state.spaces.update_tasks(&tasks);
                    state.spaces.adopt(8, &scope.task);
                    state.spaces.show(&scope.task);
                }
            });
            matches!(kind, ShellProbe::Back)
        })
        .unwrap();
        assert_eq!(calls.get(), 6);
        assert_eq!(snapshot.active_tab, Some(8));
        let first = snapshot.tabs.iter().find(|tab| tab.id == 7).unwrap();
        assert_eq!(first.title, "latest title");
        assert!(first.can_go_back && !first.can_go_forward && !first.loading);
        let owned = snapshot.tabs.iter().find(|tab| tab.id == 8).unwrap();
        assert_eq!(owned.space, snapshot.active_space);
        assert_eq!(owned.task_status, "latest owner, AI 제어 중");
        assert!(owned.stop_scope.is_some());
        let ordered: Vec<_> = snapshot
            .spaces
            .iter()
            .flat_map(|space| space.tabs.iter().copied())
            .collect();
        assert_eq!(
            snapshot.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            ordered
        );
        clear();
    }

    #[test]
    fn shell_snapshot_stops_native_reads_at_each_invalidated_getter_boundary() {
        for boundary in 1..=3 {
            for change in 0..7 {
                context_menu_fixture(false);
                let calls = Cell::new(0);
                let snapshot = shell_snapshot_with(|_, _| {
                    calls.set(calls.get() + 1);
                    if calls.get() == boundary {
                        if change >= 5 {
                            // The real loading callback uses this state mutation;
                            // loading completion deliberately does not change epoch.
                            observe_loading_state(7, change == 6);
                        } else {
                            STATE.with(|slot| {
                                let mut state = slot.borrow_mut();
                                match change {
                                    0 => {
                                        state.tabs.remove(&7);
                                    }
                                    1 => {
                                        state.tabs.get_mut(&7).unwrap().browser =
                                            native_blank_tab().browser;
                                    }
                                    2 => state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
                                    3 => {
                                        state.tabs.get_mut(&7).unwrap().navigation.document_epoch +=
                                            1
                                    }
                                    4 => state.tabs.get_mut(&7).unwrap().rejected_creation = true,
                                    _ => unreachable!(),
                                }
                            });
                        }
                    }
                    false
                });
                assert!(snapshot.is_none());
                assert_eq!(
                    calls.get(),
                    boundary,
                    "no later getter may run on an invalidated sample"
                );
                if change >= 5 {
                    STATE.with(|slot| {
                        let state = slot.borrow();
                        assert_eq!(state.tabs[&7].navigation.epoch, 4);
                        assert_eq!(state.tabs[&7].navigation.loading_event_revision, 1);
                    });
                }
            }
        }
        clear();
    }

    #[test]
    fn shell_snapshot_rechecks_earlier_tabs_and_rejects_unsampled_new_tabs() {
        for new_tab in [false, true] {
            context_menu_fixture(false);
            let calls = Cell::new(0);
            let snapshot = shell_snapshot_with(|_, _| {
                calls.set(calls.get() + 1);
                if calls.get() == 6 {
                    STATE.with(|slot| {
                        let mut state = slot.borrow_mut();
                        if new_tab {
                            state.tabs.insert(9, native_blank_tab());
                            state.spaces.insert(9, USER_SPACE, false);
                        } else {
                            state.tabs.get_mut(&7).unwrap().navigation.epoch += 1;
                        }
                    });
                }
                false
            });
            assert_eq!(calls.get(), 6);
            assert!(
                snapshot.is_none(),
                "final projection cannot combine generations or omit a new live tab"
            );
        }
        clear();
    }

    #[test]
    fn nested_shell_snapshot_is_bounded_and_read_guard_is_released() {
        context_menu_fixture(false);
        let nested = Cell::new(false);
        let outer = shell_snapshot_with(|_, _| {
            if !nested.replace(true) {
                assert!(
                    shell_snapshot_with(|_, _| panic!("nested acquisition must not call CEF"))
                        .is_none()
                );
                assert!(
                    shell_snapshot_with(|_, _| panic!(
                        "failed nested acquisition must leave the outer guard active"
                    ))
                    .is_none()
                );
            }
            false
        });
        assert!(outer.is_some());
        assert!(shell_snapshot_with(|_, _| false).is_some());
        assert!(
            shell_snapshot_with(|_, _| {
                observe_loading_state(7, true);
                false
            })
            .is_none()
        );
        assert!(
            shell_snapshot_with(|_, _| false).is_some(),
            "invalidated acquisition must release its guard too"
        );
        clear();
    }

    fn navigation_test_guard() -> NavigationControlGuard {
        STATE.with(|slot| {
            let state = slot.borrow();
            let target = NavigationControlTarget {
                tab: 7,
                space: state.spaces.active_space().to_owned(),
                navigation_epoch: state.tabs[&7].navigation.epoch,
                stop: true,
            };
            navigation_control_guard(&state, 7, Some(&target)).unwrap()
        })
    }

    #[test]
    fn finished_or_already_requested_stop_preserves_owner_and_has_no_side_effects() {
        for pending in [false, true] {
            let scope = context_menu_fixture(true).unwrap();
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
                nav.stop_pending = pending;
                nav.stopped = pending;
                nav.provisional = pending;
                nav.intent_pending = pending;
            });
            let result = execute_navigation_control(
                navigation_test_guard(),
                true,
                || true,
                || true,
                |_| (true, pending),
                |_| panic!("no-op stop must not revoke permissions/find/history"),
                |_, _| panic!("no-op stop must never reload or repeat stop"),
            )
            .unwrap();
            assert_eq!(
                result,
                NavigationControlOutcome {
                    navigation_epoch: 4,
                    preserve_address: true
                }
            );
            STATE.with(|slot| {
                let state = slot.borrow();
                assert_eq!(state.tabs[&7].navigation.epoch, 4);
                let task = state.tasks.info(&scope.task).unwrap();
                assert_eq!(
                    (task.status, task.revision),
                    (Status::Active, scope.revision)
                );
            });
        }
        clear();
    }

    #[test]
    fn pending_intent_and_later_resource_load_dispatch_stop_once() {
        for intent_only in [true, false] {
            context_menu_fixture(false);
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
                nav.intent_pending = intent_only;
                nav.stopped = !intent_only;
            });
            let cleanup_count = Cell::new(0);
            let sent = Cell::new(0);
            let result = execute_navigation_control(
                navigation_test_guard(),
                true,
                || true,
                || true,
                |_| (true, !intent_only),
                |_| {
                    cleanup_count.set(cleanup_count.get() + 1);
                    Ok(())
                },
                |_, stop| {
                    assert!(stop);
                    sent.set(sent.get() + 1);
                },
            )
            .unwrap();
            assert_eq!((cleanup_count.get(), sent.get()), (1, 1));
            assert_eq!(result.navigation_epoch, 5);
            assert!(result.preserve_address);
            execute_navigation_control(
                navigation_test_guard(),
                true,
                || true,
                || true,
                |_| (true, true),
                |_| panic!("second stop has no cleanup"),
                |_, _| panic!("second stop has no dispatch"),
            )
            .unwrap();
            STATE.with(|slot| assert_eq!(slot.borrow().tabs[&7].navigation.epoch, 5));
        }
        clear();
    }

    #[test]
    fn native_stop_allows_exact_handoff_but_not_an_extra_document_change() {
        for changed_document in [false, true] {
            let scope = context_menu_fixture(true).unwrap();
            STATE.with(|slot| {
                slot.borrow_mut()
                    .tabs
                    .get_mut(&7)
                    .unwrap()
                    .navigation
                    .intent_pending = true
            });
            let reads = Cell::new(0);
            let sent = Cell::new(0);
            let result = execute_navigation_control(
                navigation_test_guard(),
                true,
                || true,
                || true,
                |_| {
                    reads.set(reads.get() + 1);
                    if changed_document && reads.get() == 2 {
                        STATE.with(|slot| {
                            slot.borrow_mut()
                                .tabs
                                .get_mut(&7)
                                .unwrap()
                                .navigation
                                .document_epoch += 1
                        });
                    }
                    (true, false)
                },
                |_| Ok(()),
                |_, stop| {
                    assert!(stop);
                    sent.set(sent.get() + 1);
                },
            );
            STATE.with(|slot| {
                let state = slot.borrow();
                let task = state.tasks.info(&scope.task).unwrap();
                assert_eq!(
                    (task.status, task.revision),
                    (Status::Paused, scope.revision + 1)
                );
                assert_eq!(
                    state.tabs[&7].navigation.epoch,
                    if changed_document { 5 } else { 6 }
                );
            });
            assert_eq!(sent.get(), usize::from(!changed_document));
            assert_eq!(result.is_ok(), !changed_document);
        }
        clear();
    }

    #[test]
    fn navigation_failure_after_handoff_preserves_only_the_same_page_address() {
        // First readiness check succeeds, then the AI handoff expires refs.
        // A later chrome/IME rejection must preserve E+1 only for that page.
        for stop in [true, false] {
            for change in 0..4 {
                let scope = context_menu_fixture(true).unwrap();
                let readiness = Cell::new(0);
                let error = execute_navigation_control(
                    navigation_test_guard(),
                    stop,
                    || {
                        readiness.set(readiness.get() + 1);
                        if readiness.get() == 1 {
                            return true;
                        }
                        STATE.with(|slot| {
                            let mut state = slot.borrow_mut();
                            match change {
                                1 => state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
                                2 => {
                                    state
                                        .tasks
                                        .resume(&TaskScope {
                                            task: scope.task.clone(),
                                            revision: scope.revision + 1,
                                        })
                                        .unwrap();
                                }
                                3 => {
                                    state.spaces.show(USER_SPACE);
                                }
                                _ => {}
                            }
                        });
                        false
                    },
                    || true,
                    |_| (true, stop),
                    |_| panic!("rejected chrome input must not run cleanup"),
                    |_, _| panic!("rejected chrome input must not dispatch"),
                )
                .unwrap_err();
                assert_eq!(
                    error.preserved_address,
                    (change == 0).then_some(NavigationControlOutcome {
                        navigation_epoch: 5,
                        preserve_address: true,
                    })
                );
            }
        }
        clear();
    }

    #[test]
    fn address_receipt_survives_loading_only_refresh_but_rejects_replaced_context() {
        for change in 0..7 {
            context_menu_fixture(false);
            let target = NavigationControlTarget {
                tab: 7,
                space: USER_SPACE.into(),
                navigation_epoch: 4,
                stop: true,
            };
            let outcome = execute_navigation_control(
                navigation_test_guard(),
                true,
                || true,
                || true,
                |_| (true, true),
                |_| Ok(()),
                |_, _| {},
            )
            .unwrap();
            let receipt =
                capture_navigation_address_receipt(&target, "https://example.test/menu", &outcome)
                    .unwrap();
            assert!(
                capture_navigation_address_receipt(&target, "https://example.test/other", &outcome)
                    .is_none()
            );
            let nonpreserving = NavigationControlOutcome {
                preserve_address: false,
                ..outcome.clone()
            };
            assert!(
                capture_navigation_address_receipt(
                    &target,
                    "https://example.test/menu",
                    &nonpreserving
                )
                .is_none()
            );
            let mut invalid_outcome = outcome.clone();
            invalid_outcome.navigation_epoch += 1;
            assert!(
                capture_navigation_address_receipt(
                    &target,
                    "https://example.test/menu",
                    &invalid_outcome
                )
                .is_none()
            );
            let changed_loading = Cell::new(false);
            assert!(
                shell_snapshot_with(|_, _| {
                    if !changed_loading.replace(true) {
                        if change == 0 {
                            STATE.with(|slot| {
                                let mut state = slot.borrow_mut();
                                state.tabs.remove(&8);
                                state.spaces.remove(8);
                            });
                        } else {
                            observe_loading_state(7, false);
                        }
                    }
                    false
                })
                .is_none()
            );
            assert!(
                navigation_address_receipt_current(&receipt),
                "loading-only or unrelated tab invalidation must not lose the active draft"
            );
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                match change {
                    0 => state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
                    1 => state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
                    2 => state.tabs.get_mut(&7).unwrap().browser = native_blank_tab().browser,
                    3 => {
                        state.spaces.select_tab(8);
                    }
                    4 => {
                        let task = state.tasks.create("new receipt owner").unwrap();
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
                    5 => {
                        state.tabs.get_mut(&7).unwrap().navigation.requested_url =
                            "https://example.test/changed".into()
                    }
                    6 => {
                        let task = state.tasks.create("different visible space").unwrap();
                        let tasks = state.tasks.list();
                        state.spaces.update_tasks(&tasks);
                        state.spaces.show(&task.id);
                    }
                    _ => unreachable!(),
                }
            });
            assert!(!navigation_address_receipt_current(&receipt));
        }
        clear();
    }

    #[test]
    fn accepted_stop_keeps_address_when_only_chrome_eligibility_changes() {
        context_menu_fixture(false);
        let eligible = Cell::new(true);
        let outcome = execute_navigation_control(
            navigation_test_guard(),
            true,
            || true,
            || eligible.get(),
            |_| (true, true),
            |_| Ok(()),
            |_, stop| {
                assert!(stop);
                eligible.set(false);
                STATE.with(|slot| slot.borrow_mut().spaces.set_overview(true));
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            NavigationControlOutcome {
                navigation_epoch: 5,
                preserve_address: true
            }
        );
        clear();
    }

    #[test]
    fn reload_intent_never_stops_new_loading_or_hands_off_a_busy_owner() {
        for busy_kind in 0..4 {
            let scope = context_menu_fixture(true).unwrap();
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
                nav.provisional = busy_kind == 1;
                nav.intent_pending = busy_kind == 2;
                nav.recovering = busy_kind == 3;
            });
            let result = execute_navigation_control(
                navigation_test_guard(),
                false,
                || true,
                || true,
                |_| (true, busy_kind == 0),
                |_| panic!("busy reload must not clean up"),
                |_, _| panic!("busy reload must not dispatch"),
            );
            assert!(result.is_err());
            STATE.with(|slot| {
                let state = slot.borrow();
                assert_eq!(state.tabs[&7].navigation.epoch, 4);
                assert_eq!(
                    state.tasks.info(&scope.task).unwrap().status,
                    Status::Active
                );
            });
        }
        context_menu_fixture(false);
        let sent = Cell::new(0);
        let result = execute_navigation_control(
            navigation_test_guard(),
            false,
            || true,
            || true,
            |_| (true, false),
            |_| Ok(()),
            |_, stop| {
                assert!(!stop);
                sent.set(sent.get() + 1);
            },
        )
        .unwrap();
        assert_eq!(sent.get(), 1);
        assert_eq!(
            result,
            NavigationControlOutcome {
                navigation_epoch: 5,
                preserve_address: false
            }
        );
        STATE.with(|slot| {
            let state = slot.borrow();
            assert!(state.tabs[&7].navigation.intent_pending);
            assert!(state.tabs[&7].navigation.provisional);
        });
        clear();
    }

    #[test]
    fn native_preflight_finishing_load_is_observed_before_handoff() {
        let scope = context_menu_fixture(true).unwrap();
        STATE.with(|slot| {
            slot.borrow_mut()
                .tabs
                .get_mut(&7)
                .unwrap()
                .navigation
                .intent_pending = true
        });
        let sampled = Cell::new(false);
        let live_loading = Cell::new(true);
        let result = execute_navigation_control(
            navigation_test_guard(),
            true,
            || {
                assert!(
                    !sampled.get(),
                    "native UI predicate must precede the loading sample"
                );
                STATE.with(|slot| {
                    slot.borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .finish_loading()
                });
                live_loading.set(false);
                true
            },
            || true,
            |_| {
                sampled.set(true);
                (true, live_loading.get())
            },
            |_| panic!("completed load must not clear native controls"),
            |_, _| panic!("completed load must not dispatch"),
        )
        .unwrap();
        assert_eq!(result.navigation_epoch, 4);
        assert!(result.preserve_address);
        STATE.with(|slot| {
            let state = slot.borrow();
            let task = state.tasks.info(&scope.task).unwrap();
            assert_eq!(
                (task.status, task.revision),
                (Status::Active, scope.revision)
            );
        });
        clear();
    }

    #[test]
    fn navigation_getter_reentry_and_cleanup_reject_changed_targets_before_dispatch() {
        let changes: [fn(&mut State); 6] = [
            |state| state.tabs.get_mut(&7).unwrap().navigation.epoch += 1,
            |state| state.tabs.get_mut(&7).unwrap().navigation.document_epoch += 1,
            |state| {
                state.tabs.get_mut(&7).unwrap().browser = native_blank_tab().browser;
            },
            |state| {
                state.spaces.select_tab(8);
            },
            |state| state.spaces.set_overview(true),
            |state| {
                let task = state.tasks.create("reentrant owner").unwrap();
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
        for after_cleanup in [false, true] {
            for change in changes {
                context_menu_fixture(false);
                let changed = Cell::new(false);
                let result = execute_navigation_control(
                    navigation_test_guard(),
                    true,
                    || true,
                    || true,
                    |_| {
                        if !after_cleanup && !changed.replace(true) {
                            STATE.with(|slot| change(&mut slot.borrow_mut()));
                        }
                        (true, true)
                    },
                    |_| {
                        if after_cleanup {
                            STATE.with(|slot| change(&mut slot.borrow_mut()));
                        }
                        Ok(())
                    },
                    |_, _| panic!("reentrant change must prevent dispatch"),
                );
                assert!(result.is_err());
            }
        }
        clear();
    }

    #[test]
    fn accepted_stop_cannot_rebase_address_after_its_callback_navigates() {
        context_menu_fixture(false);
        let result = execute_navigation_control(
            navigation_test_guard(),
            true,
            || true,
            || true,
            |_| (true, true),
            |_| Ok(()),
            |_, stop| {
                assert!(stop);
                STATE.with(|slot| {
                    let mut state = slot.borrow_mut();
                    let nav = &mut state.tabs.get_mut(&7).unwrap().navigation;
                    assert_eq!(nav.epoch, 5);
                    assert!(nav.stop_pending);
                    nav.commit("https://example.test/reentrant");
                });
            },
        )
        .unwrap();
        assert_eq!(result.navigation_epoch, 5);
        assert!(!result.preserve_address);
        clear();
    }

    #[test]
    fn cli_stop_keeps_idle_dispatch_background_selection_and_checks_cancellation() {
        context_menu_fixture(false);
        STATE.with(|slot| {
            slot.borrow_mut().spaces.select_tab(8);
        });
        let guard = STATE
            .with(|slot| navigation_control_guard(&slot.borrow(), 7, None))
            .unwrap();
        let sent = Cell::new(0);
        let cleaned = Cell::new(0);
        execute_cli_stop(
            guard,
            || true,
            |_| (true, false),
            |_| {
                cleaned.set(cleaned.get() + 1);
                Ok(())
            },
            |_| {
                sent.set(sent.get() + 1);
            },
        )
        .unwrap();
        assert_eq!((cleaned.get(), sent.get()), (1, 1));
        STATE.with(|slot| {
            let state = slot.borrow();
            assert_eq!(state.spaces.active_tab(), Some(8));
            assert_eq!(state.tabs[&7].navigation.epoch, 4);
        });
        let guard = STATE
            .with(|slot| navigation_control_guard(&slot.borrow(), 7, None))
            .unwrap();
        let active = Cell::new(true);
        assert!(
            execute_cli_stop(
                guard,
                || active.get(),
                |_| {
                    active.set(false);
                    (true, true)
                },
                |_| panic!("cancelled request must not clean up"),
                |_| panic!("cancelled request must not dispatch"),
            )
            .is_err()
        );
        clear();
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
    fn wire_response_keeps_capture_until_matching_browser_and_never_reuses_ids() {
        let mut state = State::default();
        let original = native_blank_tab();
        let browser = original.browser.clone();
        let unrelated = native_blank_tab();
        state.tabs.insert(7, original);
        // A timed-out capture has no pending command, but retains its permit.
        state.captures.insert(42, 7);
        assert!(!response_matches(&state, &unrelated.browser, 42, None));
        assert!(!response_matches(&state, &browser, 43, None));
        assert!(response_matches(&state, &browser, 42, None));
        assert!(!response_matches(&state, &browser, 42, Some("child")));
        assert_eq!(state.captures.get(&42), Some(&7));
        state.tabs.insert(7, unrelated);
        assert!(!response_matches(&state, &browser, 42, None));
        state.next_message = i32::MAX - 1;
        assert_eq!(state.next_control_message(), Some(i32::MAX));
        assert_eq!(state.next_control_message(), None);
        assert_eq!(state.next_control_message(), None);
        assert_eq!(state.next_message, i32::MAX);
    }

    #[test]
    fn child_renderer_crash_skips_a_probe_but_fails_an_observed_session() {
        let attached = json!({"sessionId":"child","targetInfo":{"targetId":"t","type":"iframe"}});
        let run = |current: (Option<String>, FramePhase), visited: bool, session: Option<&str>| {
            clear();
            let tab = native_blank_tab();
            let browser = tab.browser.clone();
            let inventory = Shared::new(FrameInventory {
                browser: browser.clone(),
                document_epoch: 0,
                owner: None,
                requested_frame: None,
                page_read: None,
                action: None,
                pointer: None,
                response: RefCell::new(None),
                read: RefCell::new(None),
                current: RefCell::new(current),
                invalidated: Cell::new(false),
                visited: RefCell::new(BTreeSet::from([None])),
                frames: RefCell::new(Vec::new()),
                truncated: Cell::new(false),
            });
            if visited {
                inventory.visited.borrow_mut().insert(Some("child".into()));
            }
            STATE.with(|slot| {
                let mut state = slot.borrow_mut();
                state.tabs.insert(7, tab);
                let control = state.frame_controls.entry(7).or_default();
                control.enabled = true;
                control.active_token = Some(1);
                control.active_inventory = Some(inventory.clone());
                control.sessions.attach(None, &attached);
            });
            frame_event(&browser, session, "Inspector.targetCrashed", b"{}");
            let crashed = STATE.with(|slot| {
                slot.borrow().frame_controls[&7]
                    .sessions
                    .get("child")
                    .map(|entry| entry.crashed)
            });
            (crashed, inventory.invalidated.get())
        };
        // Probe: the crashed child is skipped and the inventory continues.
        assert_eq!(run((Some("child".into()), FramePhase::CrashProbe), false, Some("child")), (Some(true), false));
        // A pending command on, or frames from, that renderer can never finish:
        // the inventory fails and tracking stops, clearing the registry.
        assert_eq!(run((Some("child".into()), FramePhase::Enable), false, Some("child")), (None, true));
        assert_eq!(run((None, FramePhase::Tree), true, Some("child")), (None, true));
        // Unvisited: skipped by the next selection instead of failing now.
        assert_eq!(run((None, FramePhase::Tree), false, Some("child")), (Some(true), false));
        // The root renderer keeps its RENDERER_TERMINATED path.
        assert_eq!(run((None, FramePhase::Tree), true, None), (Some(false), false));
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
                focus_completion: None,
                frame_completion: None,
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
    fn posted_control_message_revalidates_tab_and_ignores_malformed_bytes() {
        clear();
        let tab = native_blank_tab();
        let browser = tab.browser.clone();
        let (reply, receiver) = test_reply(1);
        let pending = Pending {
            frame: None,
            tab: 7,
            epoch: tab.navigation.epoch,
            reply,
            step: Step::Evaluate,
            evaluation: Evaluation::Page {
                mutation: false,
                evidence: None,
            },
        };
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.insert(7, tab);
            state.pending.insert(42, pending.clone());
        });
        let message = br#"{"id":42,"result":{"result":{"value":{"ok":true}}}}"#;
        control_message(&browser, message);
        assert!(receiver.try_recv().unwrap().ok);
        // A task queued before OnBeforeClose runs after the tab is gone.
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.tabs.remove(&7);
            state.pending.insert(42, pending);
        });
        control_message(&browser, message);
        assert!(receiver.try_recv().is_err());
        control_message(&browser, b"{\"id\":42,");
        assert!(receiver.try_recv().is_err());
        assert!(STATE.with(|state| state.borrow().pending.contains_key(&42)));
        clear();
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
                        frame: None,
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
        let snapshot = shell_snapshot().unwrap();
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
                        frame: None,
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

    fn test_fill_evidence() -> ActionEvidence {
        ActionEvidence::for_command(&Command::Fill {
            tab: 7,
            reference: "4/fill-ref".into(),
            text: "private test value".into(),
        })
        .unwrap()
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
                Some(test_fill_evidence()),
                "fill",
                json!({"attempted":true,"trusted":false,"checked":false,"changed":true,"text":"excluded","unknown":{"body":"excluded"}}),
                Some(json!({"attempted":true,"trusted":false})),
            ),
            (
                Some(test_fill_evidence()),
                "fill",
                json!({"attempted":false,"trusted":false}),
                Some(json!({"attempted":false,"trusted":false})),
            ),
            (
                Some(test_fill_evidence()),
                "fill",
                json!({"attempted":"true","trusted":1,"text":"excluded"}),
                None,
            ),
            (
                Some(test_fill_evidence()),
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
                        frame: None,
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
            let pending = STATE.with(|slot| slot.borrow().pending[&42].clone());
            let wrong_fill_action = pending.fill_target().is_some() && action != "fill";
            assert!(bind_fill_target(&browser, &pending).is_none());
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
            if wrong_fill_action {
                assert_eq!(wire["error"]["code"], "INVALID_PAGE_RESULT");
                assert_eq!(wire["error"]["details"], json!({"outcomeUnknown":true}));
            } else {
                assert_eq!(wire["error"]["code"], code);
                assert_eq!(wire["error"].get("details"), expected.as_ref());
            }
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
            (test_fill_evidence(), "fill"),
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
                        frame: None,
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

    fn test_input_preparation() -> (Pending, Browser, mpsc::Receiver<Response>) {
        test_key_preparation("Enter")
    }

    fn test_key_preparation(key: &str) -> (Pending, Browser, mpsc::Receiver<Response>) {
        clear();
        let tab = native_blank_tab();
        let browser = tab.browser.clone();
        let (mut reply, receiver) = test_reply(701);
        let scope = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let task = state.tasks.create("Keyboard test").unwrap();
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
            frame: None,
            tab: 7,
            epoch: 4,
            reply,
            evaluation: Evaluation::Page {
                mutation: true,
                evidence: ActionEvidence::for_command(&Command::Press {
                    tab: 7,
                    reference: "4/document:3:8".into(),
                    key: key.into(),
                }),
            },
            step: Step::Evaluate,
        };
        pending.press_target().unwrap().context.set(Some(17));
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.next_message = 42;
            state.pending.insert(42, pending.clone());
        });
        (pending, browser, receiver)
    }

    fn test_press_batch() -> (Shared<InputBatch>, mpsc::Receiver<Response>) {
        test_key_batch("Enter")
    }

    fn test_key_batch(key: &str) -> (Shared<InputBatch>, mpsc::Receiver<Response>) {
        let (pending, browser, receiver) = test_key_preparation(key);
        let value = test_key_prepared(key);
        let kind = prepared_input_kind(&pending, &value).unwrap();
        (
            reserve_input_batch(42, &pending, &browser, kind).unwrap(),
            receiver,
        )
    }

    fn test_key_prepared(key: &str) -> Json {
        json!({"ok":true,"action":"press","key":key,"prepared":true,"attempted":false,
            "observation":"0123456789abcdef0123456789abcdef"})
    }

    fn finish_test_key_observation(batch: &Shared<InputBatch>, observed: &str) {
        let NativeInput::Key(target) = &batch.kind else {
            panic!("key observation helper requires a key batch");
        };
        let observation = target.observation.borrow().clone().unwrap();
        let params = input_verification_params(batch).unwrap();
        assert_eq!(params["contextId"], 17);
        assert_eq!(
            params["expression"],
            format!(
                "window.__lightBrowserBridge.act({})",
                json!({"action":"press","ref":"document:3:8","key":target.key.canonical(),
                    "observation":observation,"verifyOnly":true})
            )
        );
        let (id, pending) = reserve_input_verification(batch).unwrap().unwrap();
        assert!(matches!(pending.step, Step::PressVerify));
        assert_input_busy(batch.reply.scope.clone());
        let bytes = serde_json::to_vec(&json!({"result":{"value":{
            "ok":true,"action":"press","key":target.key.canonical(),"input":"chromium",
            "attempted":true,"observation":observation,"keydownTarget":observed}}}))
        .unwrap();
        // Exercise the real pending/callback route without a mock CEF host or a
        // production bypass for the later Runtime.evaluate transport.
        devtools_result(&batch.browser, id, true, &bytes);
    }

    #[test]
    fn key_synchronous_acknowledgements_keep_the_lock_until_both_receipts() {
        for (name, code) in [
            ("Enter", 13),
            ("Tab", 9),
            ("Escape", 27),
            ("ArrowLeft", 37),
            ("ArrowUp", 38),
            ("ArrowRight", 39),
            ("ArrowDown", 40),
            ("Home", 36),
            ("End", 35),
            ("PageUp", 33),
            ("PageDown", 34),
            ("Backspace", 8),
            ("Delete", 46),
            ("Space", 32),
            ("Shift+Enter", 13),
            ("Shift+Tab", 9),
            ("Shift+ArrowLeft", 37),
            ("Shift+ArrowUp", 38),
            ("Shift+ArrowRight", 39),
            ("Shift+ArrowDown", 40),
            ("Shift+Home", 36),
            ("Shift+End", 35),
            ("Shift+PageUp", 33),
            ("Shift+PageDown", 34),
        ] {
            let (batch, receiver) = test_key_batch(name);
            let mut sent = Vec::new();
            let verify = enqueue_input_batch(
                &batch,
                || true,
                |part, id| {
                    let params = batch.kind.params(part);
                    let base = name.strip_prefix("Shift+").unwrap_or(name);
                    let text = match base {
                        "Enter" => Some("\r"),
                        "Space" => Some(" "),
                        _ => None,
                    };
                    assert_eq!(
                        params["type"],
                        if part == 1 {
                            "keyUp"
                        } else if text.is_some() {
                            "keyDown"
                        } else {
                            "rawKeyDown"
                        }
                    );
                    assert_eq!(params["key"], if base == "Space" { " " } else { base });
                    assert_eq!(params["code"], base);
                    assert_eq!(params["windowsVirtualKeyCode"], code);
                    assert_eq!(params["nativeVirtualKeyCode"], 0);
                    assert_eq!(
                        params["modifiers"],
                        if name.starts_with("Shift+") { 8 } else { 0 }
                    );
                    assert_eq!(params["autoRepeat"], false);
                    assert_eq!(params["isKeypad"], false);
                    assert!(params.get("commands").is_none());
                    assert_eq!(
                        params.get("text").and_then(Json::as_str),
                        if part == 0 { text } else { None }
                    );
                    assert_eq!(
                        params.get("unmodifiedText").and_then(Json::as_str),
                        if part == 0 { text } else { None }
                    );
                    sent.push(part);
                    devtools_result(&batch.browser, id, true, b"{}");
                    assert!(receiver.try_recv().is_err());
                    STATE.with(|state| assert!(input_registered(&state.borrow(), &batch)));
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
            assert!(verify);
            assert!(receiver.try_recv().is_err());
            assert_input_busy(batch.reply.scope.clone());
            finish_test_key_observation(&batch, "matched");
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            assert_eq!(wire["result"]["page"]["dispatchAcknowledged"], true);
            assert_eq!(wire["result"]["page"]["input"], "chromium");
            assert_eq!(wire["result"]["page"]["key"], name);
            assert_eq!(wire["result"]["page"]["keydownTarget"], "matched");
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
        }
        clear();
    }

    #[test]
    fn enter_async_reversed_results_wait_for_both_and_allow_own_navigation() {
        let (batch, receiver) = test_press_batch();
        enqueue_input_batch(&batch, || true, |_, id| id);
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
        let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
        assert_eq!(wire["result"]["page"]["keydownTarget"], "unconfirmed");
        assert_eq!(wire["result"]["page"]["dispatchAcknowledged"], true);
        assert!(receiver.try_recv().is_err());
        STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        clear();
    }

    #[test]
    fn enter_rejected_receipt_cannot_succeed_after_a_synchronous_ack() {
        for rejected in 0..2 {
            let (batch, receiver) = test_press_batch();
            let mut sent = Vec::new();
            enqueue_input_batch(
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
    fn key_navigation_during_keydown_never_sends_keyup_to_the_new_document() {
        for key in ["Enter", "Tab", "Shift+ArrowLeft", "Backspace"] {
            let (batch, receiver) = test_key_batch(key);
            let mut sent = Vec::new();
            enqueue_input_batch(
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
        }
        clear();
    }

    #[test]
    fn key_rechecks_after_native_getters_and_does_not_revive_canceled_batches() {
        for key in ["Enter", "Tab", "Shift+ArrowLeft", "Backspace"] {
            for cancel in [false, true] {
                let (batch, receiver) = test_key_batch(key);
                let mut sent = 0;
                enqueue_input_batch(
                    &batch,
                    || {
                        if cancel {
                            batch.reply.active.store(false, Ordering::Release);
                            remove_wait(batch.reply.token);
                        } else {
                            STATE.with(|state| {
                                state.borrow_mut().tabs.insert(7, native_blank_tab())
                            });
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
        }
        clear();
    }

    #[test]
    fn key_handoff_detach_and_renderer_exit_terminate_both_parts_once() {
        for key in ["Enter", "Tab", "Shift+ArrowLeft", "Backspace"] {
            for cause in 0..3 {
                let (batch, receiver) = test_key_batch(key);
                let mut sent = Vec::new();
                enqueue_input_batch(
                    &batch,
                    || true,
                    |part, id| {
                        sent.push(part);
                        devtools_result(&batch.browser, id, true, b"{}");
                        match cause {
                            0 => revoke_task_requests(
                                &batch.reply.scope.as_ref().unwrap().task,
                                Some(7),
                            ),
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
        }
        clear();
    }

    #[test]
    fn enter_protocol_error_stops_dispatch_and_malformed_late_results_never_succeed() {
        for immediate in [false, true] {
            let (batch, receiver) = test_press_batch();
            let mut sent = Vec::new();
            enqueue_input_batch(
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
            let (_pending, browser, receiver) = test_input_preparation();
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
    fn press_preparation_cannot_replace_the_requested_key_or_accept_mixed_legacy_success() {
        for (requested, returned, legacy) in [
            ("Enter", "Escape", false),
            ("Shift+Tab", "Tab", false),
            ("Backspace", "Delete", false),
            ("Space", " ", false),
            ("ArrowDown", "ArrowDown", true),
        ] {
            let (pending, browser, receiver) = test_key_preparation(requested);
            assert_eq!(pending.press_target().unwrap().key.canonical(), requested);
            assert!(
                pending
                    .press_target()
                    .unwrap()
                    .observation
                    .borrow()
                    .is_none()
            );
            let mut value = test_key_prepared(requested);
            value["key"] = json!(returned);
            if legacy {
                value["trusted"] = json!(false);
            }
            devtools_result(
                &browser,
                42,
                true,
                &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
            );
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "INVALID_PAGE_RESULT");
            assert_eq!(
                error.details.unwrap(),
                json!({"outcomeUnknown":true,"attempted":false})
            );
            STATE.with(|state| {
                assert_eq!(state.borrow().next_message, 42);
                assert!(state.borrow().pending.is_empty());
            });
        }
        clear();
    }

    #[test]
    fn raw_protocol_unsupported_keys_are_rejected_before_native_or_task_work() {
        for key in [
            "",
            "enter",
            " ",
            "Meta+Tab",
            "Control+Enter",
            "Shift+Delete",
            "Shift+Space",
            "a",
        ] {
            for scoped in [false, true] {
                clear();
                let mut command = Command::Press {
                    tab: 7,
                    reference: "4/doc:2:3".into(),
                    key: key.into(),
                };
                if scoped {
                    command = Command::TaskRun {
                        task: "not-created".into(),
                        revision: 1,
                        command: Box::new(command),
                    };
                }
                let (reply, receiver) = test_reply(1);
                handle(&command, reply);
                assert_eq!(
                    receiver.try_recv().unwrap().error.unwrap().code,
                    "UNSUPPORTED_KEY"
                );
                STATE.with(|state| {
                    let state = state.borrow();
                    assert!(state.pending.is_empty() && state.tabs.is_empty());
                    assert_eq!(state.next_message, 0);
                });
            }
        }
        clear();
    }

    #[test]
    fn read_focused_ref_is_from_the_same_returned_list_and_reusable_with_its_epoch() {
        let long = "r".repeat(256);
        for (focused, returned, usable) in [
            (json!("doc:2:3"), json!("doc:2:3"), true),
            (json!("other:2:3"), json!("doc:2:3"), false),
            (Json::Null, json!("doc:2:3"), false),
            (json!(7), json!("doc:2:3"), false),
            (json!(""), json!(""), false),
            (json!("bad\nref"), json!("bad\nref"), false),
            (json!(long), json!(long), false),
        ] {
            clear();
            let tab = native_blank_tab();
            let browser = tab.browser.clone();
            let epoch = tab.navigation.epoch;
            let (reply, receiver) = test_reply(1);
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.tabs.insert(7, tab);
                state.pending.insert(
                    42,
                    Pending {
                        frame: None,
                        tab: 7,
                        epoch,
                        reply,
                        evaluation: Evaluation::Page {
                            mutation: false,
                            evidence: None,
                        },
                        step: Step::Evaluate,
                    },
                );
            });
            let bytes = serde_json::to_vec(&json!({"result":{"value":{
                "focusedRef":focused,"interactive":[{"ref":returned,"role":"textbox"}]}}}))
            .unwrap();
            devtools_result(&browser, 42, true, &bytes);
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            if usable {
                let reference = wire["result"]["page"]["focusedRef"].as_str().unwrap();
                assert_eq!(
                    reference,
                    wire["result"]["page"]["interactive"][0]["ref"]
                        .as_str()
                        .unwrap()
                );
                assert_eq!(reference, format!("{epoch}/doc:2:3"));
                let (reply, _) = test_reply(2);
                let expression = action_expression(
                    "press",
                    reference,
                    Some(("key", ActionValue::String("Tab"))),
                    epoch,
                    &reply,
                );
                assert_eq!(
                    expression,
                    format!(
                        "window.__lightBrowserBridge.act({})",
                        json!({"action":"press","ref":"doc:2:3","key":"Tab"})
                    )
                );
            } else {
                assert!(wire["result"]["page"]["focusedRef"].is_null());
            }
        }
        clear();
    }

    fn test_fill_preparation(text: &str) -> (Pending, Browser, mpsc::Receiver<Response>) {
        let (mut pending, browser, receiver) = test_input_preparation();
        pending.evaluation = Evaluation::Page {
            mutation: true,
            evidence: ActionEvidence::for_command(&Command::Fill {
                tab: 7,
                reference: "4/document:3:8".into(),
                text: text.into(),
            }),
        };
        pending.fill_target().unwrap().context.set(Some(17));
        assert!(bind_fill_target(&browser, &pending).is_none());
        STATE.with(|state| state.borrow_mut().pending.insert(42, pending.clone()));
        (pending, browser, receiver)
    }

    #[test]
    fn pointer_actions_start_focus_preparation_on_their_first_step() {
        let (mut pending, _browser, _receiver) = test_input_preparation();
        for command in [
            Command::Click { tab: 7, reference: "4/document:3:8".into() },
            Command::Hover { tab: 7, reference: "4/document:3:8".into() },
        ] {
            pending.evaluation = Evaluation::Page {
                mutation: true,
                evidence: ActionEvidence::for_command(&command),
            };
            pending.step = Step::Frame(String::new());
            assert!(focus_preparation(&pending));
            // Later main-document steps run inside the lease already started.
            pending.step = Step::Evaluate;
            assert!(!focus_preparation(&pending));
        }
        clear();
    }

    #[test]
    fn navigation_after_dispatched_root_keys_or_pointer_press_keeps_the_lease_result() {
        let (pending, browser, _receiver) = test_input_preparation();
        let lease = |phase, evidence, after| FocusLease {
            browser: browser.clone(),
            session: None,
            token: pending.reply.token,
            reply: pending.reply.clone(),
            evidence: Some(evidence),
            attempted: false,
            phase,
            enable: FocusMessage::new(1),
            disable: None,
            disable_after_enable: false,
            initial: None,
            after,
            stop_frames: false,
            started: Instant::now(),
        };
        let key = pending.press_target().unwrap().clone();
        let batch = reserve_input_batch(42, &pending, &browser, NativeInput::Key(key.clone())).unwrap();
        let verify = || Some(FocusContinuation::Verify(batch.clone()));
        let respond = |ok: bool| Some(FocusContinuation::Respond {
            reply: pending.reply.clone(),
            response: if ok { Response::success("id", json!({})) } else { Response::error("id", "X", "x") },
        });
        // Keys not yet sent: the document change is not their effect.
        assert!(!root_key_dispatched(&lease(FocusPhase::Active, ActionEvidence::Press(key.clone()), None)));
        key.attempted.set(true);
        assert!(root_key_dispatched(&lease(FocusPhase::Active, ActionEvidence::Press(key.clone()), None)));
        assert!(root_key_dispatched(&lease(FocusPhase::Disabling, ActionEvidence::Press(key.clone()), verify())));
        assert!(!root_key_dispatched(&lease(FocusPhase::Disabling, ActionEvidence::Press(key.clone()), respond(false))));
        let mut child = lease(FocusPhase::Active, ActionEvidence::Press(key.clone()), None);
        child.session = Some("child".into());
        assert!(!root_key_dispatched(&child));

        let click = ActionEvidence::for_command(&Command::Click { tab: 7, reference: "4/document:3:8".into() }).unwrap();
        STATE.with(|state| {
            let state = state.borrow();
            // Only the key batch is pending; no press/release is in flight.
            assert!(!pointer_press_dispatched(&state, &lease(FocusPhase::Active, click.clone(), None)));
            assert!(pointer_press_dispatched(&state, &lease(FocusPhase::Disabling, click.clone(), respond(true))));
            assert!(!pointer_press_dispatched(&state, &lease(FocusPhase::Disabling, click.clone(), respond(false))));
        });
        clear();
    }

    const FILL_OBSERVATION: &str = "0123456789abcdef0123456789abcdef";

    fn fill_prepared(kind: &str) -> Json {
        // Non-contenteditable Chromium fill arms a passive input observation.
        let mut value = json!({"ok":true,"action":"fill","prepared":true,"attempted":false,"input":"chromium","kind":kind});
        if kind != "contenteditable" {
            value["observation"] = json!(FILL_OBSERVATION);
        }
        value
    }

    fn test_fill_batch(kind: &str, text: &str) -> (Shared<InputBatch>, mpsc::Receiver<Response>) {
        let (pending, browser, receiver) = test_fill_preparation(text);
        let mut value = fill_prepared(kind);
        // The original CLI text, never text returned by the page, is submitted.
        value["text"] = json!("page supplied replacement");
        let input = prepared_input_kind(&pending, &value).unwrap();
        (
            reserve_input_batch(42, &pending, &browser, input).unwrap(),
            receiver,
        )
    }

    fn acknowledge_fill(batch: &Shared<InputBatch>) -> bool {
        enqueue_input_batch(
            batch,
            || true,
            |_, id| {
                devtools_result(&batch.browser, id, true, b"{}");
                id
            },
        )
    }

    #[test]
    fn fill_one_engine_request_keeps_lock_until_original_context_verification() {
        for (kind, text) in [
            ("text", ""),
            ("search", "search value"),
            ("tel", "+82 123"),
            ("url", "https://example.test/"),
            ("password", "private fill sentinel"),
            ("textarea", "first\nsecond"),
        ] {
            let (batch, receiver) = test_fill_batch(kind, text);
            assert_eq!(batch.kind.method(), "Input.insertText");
            let mut submissions = Vec::new();
            assert!(enqueue_input_batch(
                &batch,
                || true,
                |part, id| {
                    submissions.push(batch.kind.params(part));
                    devtools_result(&batch.browser, id, true, b"{}");
                    assert_input_busy(batch.reply.scope.clone());
                    assert!(receiver.try_recv().is_err());
                    id
                }
            ));
            assert_eq!(submissions, [json!({"text":text})]);
            assert_input_busy(batch.reply.scope.clone());
            assert!(receiver.try_recv().is_err()); // Input ACK is not fill success.
            let params = input_verification_params(&batch).unwrap();
            assert_eq!(params["contextId"], 17);
            assert_eq!(
                params["expression"],
                format!(
                    "window.__lightBrowserBridge.act({})",
                    json!({"action":"fill","ref":"document:3:8","text":text,"kind":kind,"verifyOnly":true,"observation":FILL_OBSERVATION})
                )
            );
            let (id, pending) = reserve_input_verification(&batch).unwrap().unwrap();
            assert!(matches!(pending.step, Step::FillVerify));
            assert_input_busy(batch.reply.scope.clone());
            STATE.with(|state| assert_eq!(state.borrow().pending.len(), 1));
            devtools_result(&batch.browser, batch.ids[0], true, b"{}");
            assert!(receiver.try_recv().is_err());
            let bytes = serde_json::to_vec(&json!({"result":{"value":{
                "ok":true,"action":"fill","verified":true,"input":"chromium","attempted":true,"inputTarget":"matched",
                "text":"private fill sentinel","value":"private fill sentinel"}}}))
            .unwrap();
            devtools_result(&batch.browser, id, true, &bytes);
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            assert_eq!(wire["result"]["page"]["verified"], true);
            assert_eq!(wire["result"]["page"]["dispatchAcknowledged"], true);
            assert!(wire["result"]["page"].get("trusted").is_none());
            assert!(!wire.to_string().contains("private fill sentinel"));
            devtools_result(&batch.browser, id, true, &bytes);
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| {
                let state = state.borrow();
                assert!(state.pending.is_empty());
                assert_eq!(state.spaces.active_space(), USER_SPACE);
                assert_eq!(state.spaces.active_tab(), Some(8));
            });
        }
        clear();
    }

    #[test]
    fn fill_malformed_preparation_cannot_fall_back_to_legacy_or_submit_input() {
        for (key, replacement) in [
            ("kind", json!("email")),
            ("kind", json!("number")),
            ("kind", Json::Null),
            ("input", json!("dom")),
            ("prepared", json!(false)),
            ("attempted", json!(true)),
            ("trusted", json!(false)),
            ("verified", json!(false)),
            ("action", json!("check")),
        ] {
            let (pending, browser, receiver) = test_fill_preparation("private fill sentinel");
            let mut value = fill_prepared("text");
            value[key] = replacement;
            devtools_result(
                &browser,
                42,
                true,
                &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
            );
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "INVALID_PAGE_RESULT");
            assert_eq!(pending.fill_target().unwrap().kind.get(), None);
            assert!(!pending.fill_target().unwrap().attempted.get());
            STATE.with(|state| {
                assert_eq!(state.borrow().next_message, 42);
                assert!(state.borrow().pending.is_empty());
            });
        }
        let (pending, browser, receiver) = test_fill_preparation("text");
        pending.fill_target().unwrap().context.set(None);
        devtools_result(
            &browser,
            42,
            true,
            &serde_json::to_vec(&json!({"result":{"value":fill_prepared("text")}})).unwrap(),
        );
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "INVALID_PAGE_RESULT"
        );
        clear();
    }

    #[test]
    fn fill_legacy_results_preserve_fields_without_echoing_page_values() {
        for changed in [None, Some(false), Some(true)] {
            let (_, browser, receiver) = test_fill_preparation("private fill sentinel");
            let mut value = json!({"ok":true,"action":"fill","trusted":false,
                "detail":"private fill sentinel","text":"private fill sentinel","value":"private fill sentinel"});
            if let Some(changed) = changed {
                value["changed"] = json!(changed);
            }
            devtools_result(
                &browser,
                42,
                true,
                &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
            );
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            assert_eq!(wire["result"]["page"]["trusted"], false);
            assert_eq!(
                wire["result"]["page"]
                    .get("changed")
                    .and_then(Json::as_bool),
                changed
            );
            assert!(wire["result"]["page"].get("input").is_none());
            assert!(!wire.to_string().contains("private fill sentinel"));
            STATE.with(|state| assert_eq!(state.borrow().next_message, 42));
        }
        let (_, browser, receiver) = test_fill_preparation("private fill sentinel");
        devtools_result(&browser, 42, true, &serde_json::to_vec(&json!({"result":{"value":{
            "ok":false,"action":"fill","error":"FILL_STATE_NOT_REACHED","attempted":true,"trusted":false,
            "detail":"private fill sentinel","actualValue":"private fill sentinel"}}})).unwrap());
        let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
        assert_eq!(wire["error"]["code"], "FILL_STATE_NOT_REACHED");
        assert_eq!(
            wire["error"]["details"],
            json!({"attempted":true,"trusted":false})
        );
        assert!(!wire.to_string().contains("private fill sentinel"));
        clear();
    }

    #[test]
    fn fill_rejected_receipt_or_engine_error_never_advances_to_verification() {
        for failure in 0..3 {
            let (batch, receiver) = test_fill_batch("text", "private fill sentinel");
            let mut sent = 0;
            assert!(!enqueue_input_batch(
                &batch,
                || true,
                |_, id| {
                    sent += 1;
                    match failure {
                        0 => devtools_result(&batch.browser, id, true, b"{}"),
                        1 => devtools_result(
                            &batch.browser,
                            id,
                            false,
                            br#"{"message":"private fill sentinel"}"#,
                        ),
                        _ => devtools_result(&batch.browser, id, true, b"private fill sentinel"),
                    }
                    assert!(receiver.try_recv().is_err());
                    if failure == 0 { 0 } else { id }
                }
            ));
            assert_eq!(sent, 1);
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            assert_eq!(
                wire["error"]["code"],
                ["CONTROL_UNAVAILABLE", "CONTROL_ERROR", "INVALID_RESULT"][failure]
            );
            assert_eq!(wire["error"]["details"]["attempted"], true);
            assert!(!wire.to_string().contains("private fill sentinel"));
            assert!(reserve_input_verification(&batch).unwrap().is_none());
            devtools_result(&batch.browser, batch.ids[0], true, b"{}");
            assert!(receiver.try_recv().is_err());
        }
        clear();
    }

    #[test]
    fn fill_native_getters_recheck_browser_document_owner_and_cancellation_before_send() {
        for cause in 0..4 {
            let (batch, receiver) = test_fill_batch("text", "value");
            let mut sent = 0;
            assert!(!enqueue_input_batch(
                &batch,
                || {
                    match cause {
                        0 => {
                            STATE.with(|state| {
                                state.borrow_mut().tabs.insert(7, native_blank_tab())
                            });
                        }
                        1 => STATE.with(|state| {
                            state
                                .borrow_mut()
                                .tabs
                                .get_mut(&7)
                                .unwrap()
                                .navigation
                                .document_epoch += 1
                        }),
                        2 => {
                            STATE.with(|state| {
                                state
                                    .borrow_mut()
                                    .tasks
                                    .handoff(batch.reply.scope.as_ref().unwrap())
                                    .unwrap()
                            });
                        }
                        _ => {
                            batch.reply.active.store(false, Ordering::Release);
                            remove_wait(batch.reply.token);
                        }
                    }
                    true
                },
                |_, id| {
                    sent += 1;
                    id
                }
            ));
            assert_eq!(sent, 0);
            if cause < 3 {
                let error = receiver.try_recv().unwrap().error.unwrap();
                assert_eq!(error.details.unwrap()["attempted"], false);
            }
            assert!(receiver.try_recv().is_err());
            devtools_result(&batch.browser, batch.ids[0], true, b"{}");
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    #[test]
    fn fill_does_not_verify_or_succeed_after_navigation_or_foreign_browser_ack() {
        for cause in 0..3 {
            let (batch, receiver) = test_fill_batch("text", "value");
            assert!(!enqueue_input_batch(
                &batch,
                || true,
                |_, id| {
                    if cause == 0 {
                        STATE.with(|state| {
                            state
                                .borrow_mut()
                                .tabs
                                .get_mut(&7)
                                .unwrap()
                                .navigation
                                .epoch += 1
                        });
                    } else if cause == 1 {
                        STATE.with(|state| {
                            state
                                .borrow_mut()
                                .tabs
                                .get_mut(&7)
                                .unwrap()
                                .navigation
                                .document_epoch += 1
                        });
                    }
                    if cause == 2 {
                        devtools_result(&native_blank_tab().browser, id, true, b"{}");
                    } else {
                        devtools_result(&batch.browser, id, true, b"{}");
                    }
                    id
                }
            ));
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                if cause == 2 {
                    "TAB_CLOSED"
                } else {
                    "PAGE_CHANGED"
                }
            );
            assert_eq!(error.details.unwrap()["attempted"], true);
            assert!(reserve_input_verification(&batch).unwrap().is_none());
        }
        clear();
    }

    #[test]
    fn fill_postverification_rejects_wrong_shapes_and_filters_sensitive_error_text() {
        for cause in 0..5 {
            let (batch, receiver) = test_fill_batch("password", "private fill sentinel");
            assert!(acknowledge_fill(&batch));
            let (id, _) = reserve_input_verification(&batch).unwrap().unwrap();
            let mut value = json!({"ok":true,"action":"fill","input":"chromium","attempted":true,"verified":true});
            match cause {
                0 => value["trusted"] = json!(false),
                1 => value["verified"] = json!(false),
                2 => value["input"] = json!("dom"),
                _ => {
                    value = json!({"ok":false,"action":"fill","input":"chromium","attempted":true,
                    "error":if cause == 3 {"FILL_STATE_UNCONFIRMED"} else {"FILL_STATE_NOT_REACHED"},
                    "detail":"private fill sentinel","value":"private fill sentinel"});
                }
            }
            let bytes = serde_json::to_vec(&json!({"result":{"value":value}})).unwrap();
            devtools_result(&batch.browser, id, true, &bytes);
            let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
            assert_eq!(
                wire["error"]["code"],
                if cause < 3 {
                    "INVALID_PAGE_RESULT"
                } else if cause == 3 {
                    "FILL_STATE_UNCONFIRMED"
                } else {
                    "FILL_STATE_NOT_REACHED"
                }
            );
            assert_eq!(
                wire["error"]["details"],
                json!({"outcomeUnknown":true,"attempted":true})
            );
            assert!(!wire.to_string().contains("private fill sentinel"));
            devtools_result(&batch.browser, id, true, &bytes);
            assert!(receiver.try_recv().is_err());
        }
        clear();
    }

    #[test]
    fn fill_late_verification_cannot_revive_handoff_or_validate_a_changed_document() {
        for cause in 0..4 {
            let (batch, receiver) = test_fill_batch("text", "value");
            assert!(acknowledge_fill(&batch));
            let (id, _) = reserve_input_verification(&batch).unwrap().unwrap();
            match cause {
                0 => revoke_task_requests(&batch.reply.scope.as_ref().unwrap().task, Some(7)),
                1 => STATE.with(|state| {
                    state
                        .borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .document_epoch += 1
                }),
                2 => {
                    STATE.with(|state| state.borrow_mut().tabs.insert(7, native_blank_tab()));
                }
                _ => {
                    STATE.with(|state| {
                        state
                            .borrow_mut()
                            .tasks
                            .handoff(batch.reply.scope.as_ref().unwrap())
                            .unwrap()
                    });
                }
            }
            let bytes = br#"{"result":{"value":{"ok":true,"action":"fill","input":"chromium","attempted":true,"verified":true}}}"#;
            devtools_result(&batch.browser, id, true, bytes);
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.details.unwrap()["attempted"], true);
            devtools_result(&batch.browser, id, true, bytes);
            assert!(receiver.try_recv().is_err());
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    fn test_click_preparation() -> (Pending, Browser, mpsc::Receiver<Response>) {
        let (mut pending, browser, receiver) = test_input_preparation();
        pending.evaluation = Evaluation::Page {
            mutation: true,
            evidence: ActionEvidence::for_command(&Command::Click {
                tab: 7,
                reference: "4/document:3:8".into(),
            }),
        };
        pending.click_target().unwrap().context.set(Some(17));
        STATE.with(|state| {
            state.borrow_mut().pending.insert(42, pending.clone());
        });
        (pending, browser, receiver)
    }

    fn click_prepared(point: Json) -> Json {
        json!({"ok":true,"action":"click","prepared":true,"attempted":false,"point":point})
    }

    fn test_click_move() -> (Shared<InputBatch>, mpsc::Receiver<Response>) {
        let (pending, browser, receiver) = test_click_preparation();
        let kind =
            prepared_input_kind(&pending, &click_prepared(json!({"x":42.5,"y":90.25}))).unwrap();
        (
            reserve_input_batch(42, &pending, &browser, kind).unwrap(),
            receiver,
        )
    }

    fn assert_input_busy(scope: Option<TaskScope>) {
        let (mut reply, blocked) = test_reply(702);
        reply.scope = scope;
        handle(&Command::Read { tab: 7 }, reply);
        assert_eq!(blocked.try_recv().unwrap().error.unwrap().code, "TAB_BUSY");
    }

    #[test]
    fn click_move_verify_and_button_pair_keep_one_operation_and_the_original_ref() {
        let (batch, receiver) = test_click_move();
        let mut events = Vec::new();
        let verify = enqueue_input_batch(
            &batch,
            || true,
            |part, id| {
                events.push(batch.kind.params(part));
                devtools_result(&batch.browser, id, true, b"{}");
                assert_input_busy(batch.reply.scope.clone());
                assert!(receiver.try_recv().is_err());
                id
            },
        );
        assert!(verify);
        assert_input_busy(batch.reply.scope.clone());
        STATE.with(|state| assert_eq!(state.borrow().pending.len(), 1));
        let params = input_verification_params(&batch).unwrap();
        assert_eq!(params["contextId"], 17);
        assert_eq!(
            params["expression"],
            format!(
                "window.__lightBrowserBridge.act({})",
                json!({"action":"click","ref":"document:3:8","point":{"x":42.5,"y":90.25},"verifyOnly":true})
            )
        );
        let (id, pending) = reserve_input_verification(&batch).unwrap().unwrap();
        assert_input_busy(batch.reply.scope.clone());
        STATE.with(|state| assert_eq!(state.borrow().pending.len(), 1));
        // A duplicate pointer acknowledgement cannot advance the replacement.
        devtools_result(&batch.browser, batch.ids[0], true, b"{}");
        assert!(receiver.try_recv().is_err());
        let kind =
            prepared_input_kind(&pending, &click_prepared(json!({"x":42.5,"y":90.25}))).unwrap();
        let click = reserve_input_batch(id, &pending, &batch.browser, kind).unwrap();
        assert_input_busy(batch.reply.scope.clone());
        assert!(!enqueue_input_batch(
            &click,
            || true,
            |part, id| {
                events.push(click.kind.params(part));
                devtools_result(&click.browser, id, true, b"{}");
                assert_input_busy(click.reply.scope.clone());
                id
            }
        ));
        assert_eq!(
            events,
            [
                json!({"type":"mouseMoved","x":42.5,"y":90.25,"button":"none","buttons":0,"clickCount":0,"modifiers":0,"pointerType":"mouse"}),
                json!({"type":"mousePressed","x":42.5,"y":90.25,"button":"left","buttons":1,"clickCount":1,"modifiers":0,"pointerType":"mouse"}),
                json!({"type":"mouseReleased","x":42.5,"y":90.25,"button":"left","buttons":0,"clickCount":1,"modifiers":0,"pointerType":"mouse"}),
            ]
        );
        let wire = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
        assert_eq!(wire["result"]["page"]["action"], "click");
        assert_eq!(wire["result"]["page"]["dispatchAcknowledged"], true);
        assert!(wire["result"]["page"].get("key").is_none());
        assert!(wire["result"]["page"].get("trusted").is_none());
        STATE.with(|state| {
            let state = state.borrow();
            assert!(state.pending.is_empty());
            assert_eq!(state.spaces.active_space(), USER_SPACE);
            assert_eq!(state.spaces.active_tab(), Some(8));
        });
        assert!(receiver.try_recv().is_err());
        clear();
    }

    #[test]
    fn click_rejected_move_receipt_never_verifies_or_dispatches_buttons() {
        let (batch, receiver) = test_click_move();
        let mut sent = 0;
        assert!(!enqueue_input_batch(
            &batch,
            || true,
            |_, id| {
                sent += 1;
                devtools_result(&batch.browser, id, true, b"{}");
                assert!(receiver.try_recv().is_err());
                0
            }
        ));
        assert_eq!(sent, 1);
        let error = receiver.try_recv().unwrap().error.unwrap();
        assert_eq!(error.code, "CONTROL_UNAVAILABLE");
        assert_eq!(
            error.details.unwrap(),
            json!({"tab":7,"outcomeUnknown":true,"attempted":true})
        );
        assert!(reserve_input_verification(&batch).unwrap().is_none());
        devtools_result(&batch.browser, batch.ids[0], true, b"{}");
        assert!(receiver.try_recv().is_err());
        STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        clear();
    }

    #[test]
    fn click_hover_rejection_or_changed_point_keeps_engine_attempted_evidence() {
        for changed_point in [false, true] {
            let (batch, receiver) = test_click_move();
            assert!(enqueue_input_batch(
                &batch,
                || true,
                |_, id| {
                    devtools_result(&batch.browser, id, true, b"{}");
                    id
                }
            ));
            let (id, _) = reserve_input_verification(&batch).unwrap().unwrap();
            let value = if changed_point {
                click_prepared(json!({"x":43.0,"y":90.25}))
            } else {
                json!({"ok":false,"action":"click","attempted":false,"error":"CLICK_TARGET_CHANGED","detail":"The fixed point no longer reaches the reference."})
            };
            devtools_result(
                &batch.browser,
                id,
                true,
                &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
            );
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(
                error.code,
                if changed_point {
                    "INVALID_PAGE_RESULT"
                } else {
                    "CLICK_TARGET_CHANGED"
                }
            );
            assert_eq!(
                error.details.unwrap(),
                json!({"outcomeUnknown":true,"attempted":true})
            );
            STATE.with(|state| {
                let state = state.borrow();
                assert!(state.pending.is_empty());
                assert_eq!(state.next_message, id); // No mousePressed allocation.
            });
        }
        clear();
    }

    #[test]
    fn click_verification_getter_boundary_rejects_navigation_handoff_and_timeout() {
        for cause in 0..3 {
            let (batch, receiver) = test_click_move();
            assert!(enqueue_input_batch(
                &batch,
                || true,
                |_, id| {
                    devtools_result(&batch.browser, id, true, b"{}");
                    id
                }
            ));
            match cause {
                0 => STATE.with(|state| {
                    state
                        .borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .epoch += 1;
                }),
                1 => revoke_task_requests(&batch.reply.scope.as_ref().unwrap().task, Some(7)),
                _ => {
                    batch.reply.active.store(false, Ordering::Release);
                    remove_wait(batch.reply.token);
                }
            }
            match reserve_input_verification(&batch) {
                Err(failure) => {
                    assert_eq!(failure.code, "PAGE_CHANGED");
                    batch.progress.borrow_mut().failure = Some(failure);
                    assert!(!finish_input_batch(&batch));
                }
                Ok(result) => assert!(result.is_none()),
            }
            if cause < 2 {
                let error = receiver.try_recv().unwrap().error.unwrap();
                assert_eq!(error.details.unwrap()["attempted"], true);
            }
            assert!(receiver.try_recv().is_err());
            devtools_result(&batch.browser, batch.ids[0], true, b"{}");
            STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        }
        clear();
    }

    #[test]
    fn click_malformed_preparation_never_sets_a_point_or_attempts_native_input() {
        for value in [
            json!({"ok":true,"action":"click","trusted":false}),
            click_prepared(json!({"x":-1,"y":5})),
            click_prepared(json!({"x":5,"y":1_000_001})),
            click_prepared(json!({"x":"5","y":5})),
            click_prepared(json!({"x":null,"y":5})),
            click_prepared(json!({"x":5})),
        ] {
            let (pending, browser, receiver) = test_click_preparation();
            let target = pending.click_target().unwrap();
            devtools_result(
                &browser,
                42,
                true,
                &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
            );
            let error = receiver.try_recv().unwrap().error.unwrap();
            assert_eq!(error.code, "INVALID_PAGE_RESULT");
            assert_eq!(error.details.unwrap()["attempted"], false);
            assert_eq!(target.point.get(), None);
            STATE.with(|state| assert_eq!(state.borrow().next_message, 42));
        }
        let (pending, browser, receiver) = test_click_preparation();
        pending.click_target().unwrap().context.set(None);
        let value = click_prepared(json!({"x":42.5,"y":90.25}));
        devtools_result(
            &browser,
            42,
            true,
            &serde_json::to_vec(&json!({"result":{"value":value}})).unwrap(),
        );
        assert_eq!(
            receiver.try_recv().unwrap().error.unwrap().code,
            "INVALID_PAGE_RESULT"
        );
        assert_eq!(pending.click_target().unwrap().point.get(), None);
        clear();
    }

    #[test]
    fn click_button_pair_preserves_move_attempt_when_canceled_before_press() {
        let (batch, receiver) = test_click_move();
        assert!(enqueue_input_batch(
            &batch,
            || true,
            |_, id| {
                devtools_result(&batch.browser, id, true, b"{}");
                id
            }
        ));
        let (id, pending) = reserve_input_verification(&batch).unwrap().unwrap();
        let kind =
            prepared_input_kind(&pending, &click_prepared(json!({"x":42.5,"y":90.25}))).unwrap();
        let click = reserve_input_batch(id, &pending, &batch.browser, kind).unwrap();
        let mut sent = 0;
        assert!(!enqueue_input_batch(
            &click,
            || {
                STATE.with(|state| {
                    state
                        .borrow_mut()
                        .tabs
                        .get_mut(&7)
                        .unwrap()
                        .navigation
                        .epoch += 1
                });
                true
            },
            |_, id| {
                sent += 1;
                id
            }
        ));
        assert_eq!(sent, 0);
        let error = receiver.try_recv().unwrap().error.unwrap();
        assert_eq!(error.code, "PAGE_CHANGED");
        assert_eq!(error.details.unwrap()["attempted"], true);
        STATE.with(|state| assert!(state.borrow().pending.is_empty()));
        clear();
    }

    #[test]
    fn completed_dom_probe_is_released_when_wait_cannot_finish() {
        for expired in [false, true] {
            clear();
            let (reply, receiver) = test_reply(17);
            let active = reply.active.clone();
            let started = Instant::now() - Duration::from_secs(if expired { 2 } else { 0 });
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
                        frame: None,
                        tab: 7,
                        epoch: tab.navigation.epoch,
                        evaluation: Evaluation::ReadyState { token: 17, started },
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
                        started,
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
                        frame: None,
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
                        frame: None,
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
            let started = state.waits[&3].started;
            state.pending.insert(
                3,
                Pending {
                    frame: None,
                    tab: 7,
                    epoch: 1,
                    reply: waiting,
                    evaluation: Evaluation::ReadyState { token: 3, started },
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
                        frame: None,
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
            let started = state.waits[&1].started;
            state.pending.insert(
                1,
                Pending {
                    frame: None,
                    tab: 7,
                    epoch: 1,
                    reply: wait_reply,
                    evaluation: Evaluation::ReadyState { token: 1, started },
                    step: Step::Evaluate,
                },
            );
            state.pending.insert(
                2,
                Pending {
                    frame: None,
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
