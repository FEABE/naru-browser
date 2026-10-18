//! Bounded child DevTools sessions for one already-authorized browser.
//! The caller checks Browser/task ownership before applying protocol events.

use serde::Serialize;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CHILD_SESSIONS: usize = 16;
pub const MAX_SESSION_DEPTH: usize = 8;
pub const MAX_FRAMES: usize = 128;
pub const MAX_FRAME_DEPTH: usize = 32;
pub const MAX_FRAME_URL_BYTES: usize = 4096;
pub const MAX_FRAME_NAME_BYTES: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 256;

/// CDP frame identifiers are opaque strings, not CEF Browser/Frame identifiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameRecord {
    pub frame_id: String,
    pub parent_id: Option<String>,
    pub session_id: Option<String>,
    pub url: String,
    pub name: String,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct FrameTree {
    pub frames: Vec<FrameRecord>,
    pub truncated: bool,
}

/// The caller caps the raw response before parsing JSON. Only bounded metadata
/// is copied here; no source JSON, loader state or execution context is retained.
pub fn normalize_frame_tree(
    result: &Json,
    session: Option<&str>,
) -> Result<FrameTree, &'static str> {
    if session.is_some_and(|session| !valid_identifier(session)) {
        return Err("Invalid frame tree session identifier.");
    }
    let root = result.get("frameTree").ok_or("Missing frame tree.")?;
    let mut tree = FrameTree::default();
    let record = frame_record(root, None, session, &mut tree.truncated)?;
    let parent = record.frame_id.clone();
    tree.frames.push(record);
    let mut visited = 1;
    append_children(root, &parent, session, 1, &mut visited, &mut tree);
    Ok(tree)
}

fn frame_record(
    node: &Json,
    structural_parent: Option<&str>,
    session: Option<&str>,
    truncated: &mut bool,
) -> Result<FrameRecord, &'static str> {
    let frame = node
        .get("frame")
        .filter(|frame| frame.is_object())
        .ok_or("Missing frame metadata.")?;
    let id = frame["id"]
        .as_str()
        .filter(|id| valid_identifier(id))
        .ok_or("Invalid frame identifier.")?;
    let declared_parent = match frame.get("parentId") {
        None => None,
        Some(parent) => Some(
            parent
                .as_str()
                .filter(|id| valid_identifier(id))
                .ok_or("Invalid parent frame identifier.")?,
        ),
    };
    if declared_parent == Some(id)
        || structural_parent == Some(id)
        || structural_parent
            .zip(declared_parent)
            .is_some_and(|(expected, actual)| expected != actual)
    {
        return Err("The frame parent does not match its tree position.");
    }
    Ok(FrameRecord {
        frame_id: id.to_owned(),
        parent_id: declared_parent.or(structural_parent).map(str::to_owned),
        session_id: session.map(str::to_owned),
        url: metadata_text(frame.get("url"), MAX_FRAME_URL_BYTES, true, truncated),
        name: metadata_text(frame.get("name"), MAX_FRAME_NAME_BYTES, false, truncated),
    })
}

fn metadata_text(
    value: Option<&Json>,
    limit: usize,
    required: bool,
    truncated: &mut bool,
) -> String {
    let Some(text) = value.and_then(Json::as_str) else {
        *truncated |= required || value.is_some();
        return String::new();
    };
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    *truncated |= end < text.len();
    text[..end].to_owned()
}

fn append_children(
    node: &Json,
    parent: &str,
    session: Option<&str>,
    depth: usize,
    visited: &mut usize,
    tree: &mut FrameTree,
) {
    let children = match node.get("childFrames") {
        None => return,
        Some(Json::Array(children)) => children,
        Some(_) => {
            tree.truncated = true;
            return;
        }
    };
    if children.is_empty() {
        return;
    }
    if depth >= MAX_FRAME_DEPTH {
        tree.truncated = true;
        return;
    }
    for child in children {
        // Count malformed entries too, so discarded metadata cannot cause an
        // unbounded scan. Root is depth 1 and consumes the first visit.
        if *visited >= MAX_FRAMES {
            tree.truncated = true;
            break;
        }
        *visited += 1;
        match frame_record(child, Some(parent), session, &mut tree.truncated) {
            Ok(record) => {
                let parent = record.frame_id.clone();
                tree.frames.push(record);
                append_children(child, &parent, session, depth + 1, visited, tree);
            }
            Err(_) => tree.truncated = true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: String,
    /// None means the implicit root session, which is not stored or counted.
    pub parent: Option<String>,
    pub target: String,
    pub depth: usize,
    pub ready: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachOutcome {
    Added {
        id: String,
    },
    Duplicate,
    Reject {
        /// Only a validated, unregistered id is safe for caller-side detach.
        /// A conflicting id must never detach the existing registered session.
        session: Option<String>,
        reason: &'static str,
    },
}

#[derive(Debug, Default)]
pub struct Registry {
    sessions: BTreeMap<String, Session>,
    truncated: bool,
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedContext {
    pub id: i32,
    pub unique_id: String,
}

/// One requested frame/world, within an already-validated DevTools session.
/// The caller starts observation before Runtime.enable and confirms the id from
/// Page.createIsolatedWorld before using the system-unique evaluation target.
#[derive(Debug)]
pub struct ContextTracker {
    frame_id: String,
    world_name: String,
    observed: Option<TrackedContext>,
    confirmed: bool,
    invalidated: bool,
}

impl ContextTracker {
    pub fn new(frame_id: &str, world_name: &str) -> Option<Self> {
        if !valid_identifier(frame_id) || !valid_identifier(world_name) {
            return None;
        }
        Some(Self {
            frame_id: frame_id.to_owned(),
            world_name: world_name.to_owned(),
            observed: None,
            confirmed: false,
            invalidated: false,
        })
    }

    /// Browser/session ownership must be checked before forwarding an event.
    pub fn observe(&mut self, method: &str, params: &Json) {
        if self.invalidated {
            return;
        }
        match method {
            "Runtime.executionContextCreated" => {
                let context = &params["context"];
                if context["name"].as_str() != Some(self.world_name.as_str())
                    || context["auxData"]["frameId"].as_str() != Some(self.frame_id.as_str())
                {
                    return;
                }
                if context["auxData"]["type"].as_str() != Some("isolated")
                    || context["auxData"]["isDefault"].as_bool() != Some(false)
                {
                    self.invalidate();
                    return;
                }
                let id = context["id"]
                    .as_i64()
                    .and_then(|id| i32::try_from(id).ok())
                    .filter(|id| *id > 0);
                let unique_id = context["uniqueId"]
                    .as_str()
                    .filter(|id| valid_identifier(id));
                let (Some(id), Some(unique_id)) = (id, unique_id) else {
                    self.invalidate();
                    return;
                };
                match &self.observed {
                    Some(previous) if previous.id == id && previous.unique_id == unique_id => {}
                    Some(_) => self.invalidate(),
                    None => {
                        self.observed = Some(TrackedContext {
                            id,
                            unique_id: unique_id.to_owned(),
                        });
                    }
                }
            }
            "Runtime.executionContextDestroyed" => {
                if self.observed.as_ref().is_some_and(|context| {
                    params["executionContextId"].as_i64() == Some(i64::from(context.id))
                        || params["executionContextUniqueId"].as_str()
                            == Some(context.unique_id.as_str())
                }) {
                    self.invalidate();
                }
            }
            "Runtime.executionContextsCleared" => self.invalidate(),
            "Page.frameNavigated" => {
                if params["frame"]["id"].as_str() == Some(self.frame_id.as_str()) {
                    self.invalidate();
                }
            }
            "Page.frameDetached" => {
                if params["frameId"].as_str() == Some(self.frame_id.as_str()) {
                    self.invalidate();
                }
            }
            _ => {}
        }
    }

    pub fn confirm(&mut self, create_response_id: i32) -> Option<&TrackedContext> {
        if self.invalidated
            || create_response_id <= 0
            || !self
                .observed
                .as_ref()
                .is_some_and(|context| context.id == create_response_id)
        {
            self.invalidate();
            return None;
        }
        self.confirmed = true;
        self.context()
    }

    pub fn context(&self) -> Option<&TrackedContext> {
        if self.confirmed && !self.invalidated {
            self.observed.as_ref()
        } else {
            None
        }
    }

    pub fn invalidated(&self) -> bool {
        self.invalidated
    }

    pub fn frame_id(&self) -> &str {
        &self.frame_id
    }

    pub fn world_name(&self) -> &str {
        &self.world_name
    }

    fn invalidate(&mut self) {
        self.observed = None;
        self.confirmed = false;
        self.invalidated = true;
    }
}

impl Registry {
    pub fn attach(&mut self, parent: Option<&str>, params: &Json) -> AttachOutcome {
        let Some(id) = params["sessionId"]
            .as_str()
            .filter(|id| valid_identifier(id))
        else {
            return self.reject(None, "Invalid child session identifier.", true);
        };
        let rejected_id = (!self.sessions.contains_key(id)).then_some(id);
        match params["targetInfo"]["type"].as_str() {
            Some("iframe") => {}
            Some(_) => {
                // Workers and other target types are outside the frame inventory.
                return self.reject(rejected_id, "Only iframe targets are supported.", false);
            }
            None => return self.reject(rejected_id, "Missing target type.", true),
        }
        let Some(target) = params["targetInfo"]["targetId"]
            .as_str()
            .filter(|id| valid_identifier(id))
        else {
            return self.reject(rejected_id, "Invalid frame target identifier.", true);
        };
        let depth = match parent {
            None => 1,
            Some(parent) => {
                let Some(parent) = self
                    .sessions
                    .get(parent)
                    .filter(|_| valid_identifier(parent))
                else {
                    return self.reject(rejected_id, "The parent session is unknown.", true);
                };
                parent.depth + 1
            }
        };
        if let Some(existing) = self.sessions.get(id) {
            return if existing.parent.as_deref() == parent && existing.target == target {
                AttachOutcome::Duplicate
            } else {
                self.reject(
                    None,
                    "The session identifier already belongs to another attachment.",
                    true,
                )
            };
        }
        if self
            .sessions
            .values()
            .any(|session| session.target == target)
        {
            return self.reject(
                Some(id),
                "The frame target already has a registered session.",
                true,
            );
        }
        if depth > MAX_SESSION_DEPTH {
            return self.reject(Some(id), "The child session depth limit was reached.", true);
        }
        if self.sessions.len() >= MAX_CHILD_SESSIONS {
            return self.reject(Some(id), "The child session count limit was reached.", true);
        }
        self.sessions.insert(
            id.to_owned(),
            Session {
                id: id.to_owned(),
                parent: parent.map(str::to_owned),
                target: target.to_owned(),
                depth,
                ready: false,
            },
        );
        AttachOutcome::Added { id: id.to_owned() }
    }

    fn reject(
        &mut self,
        session: Option<&str>,
        reason: &'static str,
        incomplete: bool,
    ) -> AttachOutcome {
        self.truncated |= incomplete;
        AttachOutcome::Reject {
            session: session.map(str::to_owned),
            reason,
        }
    }

    /// A detach event applies only to the session owned by its envelope's parent.
    pub fn detach(&mut self, parent: Option<&str>, session: &str) -> Vec<String> {
        if self
            .sessions
            .get(session)
            .is_some_and(|entry| entry.parent.as_deref() == parent)
        {
            self.remove_subtree(session)
        } else {
            Vec::new()
        }
    }

    /// Administrative removal after a known session's setup/control failure.
    /// Returns that session and all descendants, for pending-request cleanup.
    pub fn remove(&mut self, session: &str) -> Vec<String> {
        let removed = self.remove_subtree(session);
        self.truncated |= !removed.is_empty();
        removed
    }

    fn remove_subtree(&mut self, session: &str) -> Vec<String> {
        if !self.sessions.contains_key(session) {
            return Vec::new();
        }
        let mut removed = BTreeSet::from([session.to_owned()]);
        // At most 16 entries: the bounded scan avoids a separate child index.
        loop {
            let before = removed.len();
            for entry in self.sessions.values() {
                if entry
                    .parent
                    .as_ref()
                    .is_some_and(|parent| removed.contains(parent))
                {
                    removed.insert(entry.id.clone());
                }
            }
            if removed.len() == before {
                break;
            }
        }
        for id in &removed {
            self.sessions.remove(id);
        }
        removed.into_iter().collect()
    }

    pub fn mark_ready(&mut self, session: &str) -> bool {
        let Some(session) = self.sessions.get_mut(session) else {
            return false;
        };
        session.ready = true;
        true
    }

    pub fn contains(&self, session: &str) -> bool {
        self.sessions.contains_key(session)
    }

    pub fn get(&self, session: &str) -> Option<&Session> {
        self.sessions.get(session)
    }

    pub fn ids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Once an iframe was missed, detach alone cannot prove a complete inventory.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Starts a new inventory. The caller also invalidates outstanding requests.
    pub fn clear(&mut self) {
        self.sessions.clear();
        self.truncated = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attached(session: &str, target: &str) -> Json {
        json!({"sessionId":session,"targetInfo":{"targetId":target,"type":"iframe"}})
    }

    fn context_created(id: i32, unique_id: &str, frame: &str, world: &str) -> Json {
        json!({"context":{"id":id,"uniqueId":unique_id,"name":world,"auxData":{"frameId":frame,"type":"isolated","isDefault":false}}})
    }

    #[test]
    fn context_requires_exact_frame_world_and_create_response_before_exposing_unique_id() {
        let mut tracker = ContextTracker::new("frame", "LightBrowserFrameControl").unwrap();
        for event in [
            context_created(1, "other-world", "frame", ""),
            context_created(2, "other-frame", "other", "LightBrowserFrameControl"),
            json!({"context":{"id":3,"uniqueId":"missing-frame","name":"LightBrowserFrameControl"}}),
        ] {
            tracker.observe("Runtime.executionContextCreated", &event);
        }
        assert!(tracker.context().is_none());
        assert!(!tracker.invalidated());
        let event = context_created(7, "unique-seven", "frame", "LightBrowserFrameControl");
        tracker.observe("Runtime.executionContextCreated", &event);
        assert!(tracker.context().is_none());
        assert_eq!(tracker.confirm(7).unwrap().unique_id, "unique-seven");
        tracker.observe("Runtime.executionContextCreated", &event);
        tracker.observe(
            "Runtime.executionContextDestroyed",
            &json!({"executionContextId":8,"executionContextUniqueId":"other"}),
        );
        tracker.observe("Page.frameNavigated", &json!({"frame":{"id":"other"}}));
        tracker.observe("Page.frameDetached", &json!({"frameId":"other"}));
        assert_eq!(tracker.context().unwrap().id, 7);
        assert_eq!(tracker.frame_id(), "frame");
        assert_eq!(tracker.world_name(), "LightBrowserFrameControl");
    }

    #[test]
    fn context_missing_mismatched_or_ambiguous_identity_cannot_recover() {
        let event = context_created(7, "unique-seven", "frame", "world");
        let mut missing = ContextTracker::new("frame", "world").unwrap();
        assert!(missing.confirm(7).is_none());
        missing.observe("Runtime.executionContextCreated", &event);
        assert!(missing.invalidated());
        assert!(missing.confirm(7).is_none());
        for response_id in [0, -1, 8] {
            let mut tracker = ContextTracker::new("frame", "world").unwrap();
            tracker.observe("Runtime.executionContextCreated", &event);
            assert!(tracker.confirm(response_id).is_none());
            assert!(tracker.invalidated());
        }
        for replacement in [
            context_created(8, "unique-seven", "frame", "world"),
            context_created(7, "replacement", "frame", "world"),
            context_created(0, "invalid-id", "frame", "world"),
            context_created(7, "", "frame", "world"),
            context_created(7, "bad\n", "frame", "world"),
            context_created(7, "한글", "frame", "world"),
            context_created(7, &"x".repeat(257), "frame", "world"),
            json!({"context":{"id":2147483648_i64,"uniqueId":"too-large","name":"world","auxData":{"frameId":"frame","type":"isolated","isDefault":false}}}),
        ] {
            let mut tracker = ContextTracker::new("frame", "world").unwrap();
            tracker.observe("Runtime.executionContextCreated", &event);
            assert!(tracker.confirm(7).is_some());
            tracker.observe("Runtime.executionContextCreated", &replacement);
            tracker.observe("Runtime.executionContextCreated", &event);
            assert!(tracker.invalidated());
            assert!(tracker.context().is_none());
        }
        for metadata in [
            json!({"frameId":"frame","type":"default","isDefault":false}),
            json!({"frameId":"frame","type":"isolated","isDefault":true}),
            json!({"frameId":"frame","type":"isolated"}),
            json!({"frameId":"frame","isDefault":false}),
        ] {
            let mut tracker = ContextTracker::new("frame", "world").unwrap();
            let mut malformed = event.clone();
            malformed["context"]["auxData"] = metadata;
            tracker.observe("Runtime.executionContextCreated", &malformed);
            tracker.observe("Runtime.executionContextCreated", &event);
            assert!(tracker.invalidated());
            assert!(tracker.confirm(7).is_none());
        }
        for invalid in [
            "".to_owned(),
            "bad\0".into(),
            "한글".into(),
            "x".repeat(257),
        ] {
            assert!(ContextTracker::new(&invalid, "world").is_none());
            assert!(ContextTracker::new("frame", &invalid).is_none());
        }
    }

    #[test]
    fn context_destroy_clear_or_matching_frame_change_permanently_invalidates() {
        let event = context_created(7, "unique-seven", "frame", "world");
        for (method, params) in [
            (
                "Runtime.executionContextDestroyed",
                json!({"executionContextId":7}),
            ),
            (
                "Runtime.executionContextDestroyed",
                json!({"executionContextUniqueId":"unique-seven"}),
            ),
            ("Runtime.executionContextsCleared", json!({})),
            ("Page.frameNavigated", json!({"frame":{"id":"frame"}})),
            ("Page.frameDetached", json!({"frameId":"frame"})),
        ] {
            for confirmed in [false, true] {
                let mut tracker = ContextTracker::new("frame", "world").unwrap();
                tracker.observe("Runtime.executionContextCreated", &event);
                if confirmed {
                    assert!(tracker.confirm(7).is_some());
                }
                tracker.observe(method, &params);
                tracker.observe("Runtime.executionContextCreated", &event);
                assert!(tracker.invalidated());
                assert!(tracker.context().is_none());
                assert!(tracker.confirm(7).is_none());
            }
        }
        let mut cleared_before_creation = ContextTracker::new("frame", "world").unwrap();
        cleared_before_creation.observe("Runtime.executionContextsCleared", &json!({}));
        cleared_before_creation.observe("Runtime.executionContextCreated", &event);
        assert!(cleared_before_creation.confirm(7).is_none());
    }

    #[test]
    fn duplicate_preserves_identity_and_ready_but_conflicts_cannot_replace_or_detach_it() {
        let mut registry = Registry::default();
        assert_eq!(
            registry.attach(None, &attached("a", "frame-a")),
            AttachOutcome::Added { id: "a".into() }
        );
        assert!(!registry.get("a").unwrap().ready);
        assert!(registry.mark_ready("a"));
        assert_eq!(
            registry.attach(None, &attached("a", "frame-a")),
            AttachOutcome::Duplicate
        );
        assert!(registry.get("a").unwrap().ready);
        assert!(matches!(
            registry.attach(None, &attached("a", "replacement")),
            AttachOutcome::Reject { session: None, .. }
        ));
        assert_eq!(registry.get("a").unwrap().target, "frame-a");
        assert!(
            matches!(registry.attach(Some("a"), &attached("alias", "frame-a")), AttachOutcome::Reject { session: Some(session), .. } if session == "alias")
        );
        assert!(matches!(
            registry.attach(Some("a"), &attached("a", "frame-a")),
            AttachOutcome::Reject { session: None, .. }
        ));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn unknown_parent_unsupported_type_and_invalid_identifiers_never_enter_inventory() {
        let mut registry = Registry::default();
        let worker =
            json!({"sessionId":"worker","targetInfo":{"targetId":"worker-target","type":"worker"}});
        assert!(
            matches!(registry.attach(None, &worker), AttachOutcome::Reject { session: Some(session), .. } if session == "worker")
        );
        assert!(!registry.truncated());
        assert!(
            matches!(registry.attach(Some("missing"), &attached("a", "frame-a")), AttachOutcome::Reject { session: Some(session), .. } if session == "a")
        );
        for invalid in [
            String::new(),
            "한글".into(),
            "a\n".into(),
            "a\0".into(),
            "x".repeat(257),
        ] {
            assert!(matches!(
                registry.attach(None, &attached(&invalid, "valid")),
                AttachOutcome::Reject { session: None, .. }
            ));
            assert!(matches!(
                registry.attach(None, &attached("valid", &invalid)),
                AttachOutcome::Reject {
                    session: Some(_),
                    ..
                }
            ));
        }
        assert!(registry.is_empty());
        assert!(registry.truncated());
    }

    #[test]
    fn count_and_depth_include_unready_sessions_and_truncation_survives_removal() {
        let mut registry = Registry::default();
        for depth in 1..=MAX_SESSION_DEPTH {
            let parent = (depth > 1).then(|| format!("s{}", depth - 1));
            let id = format!("s{depth}");
            assert!(matches!(
                registry.attach(parent.as_deref(), &attached(&id, &format!("f{depth}"))),
                AttachOutcome::Added { .. }
            ));
            assert_eq!(registry.get(&id).unwrap().depth, depth);
        }
        assert!(matches!(
            registry.attach(Some("s8"), &attached("too-deep", "deep-frame")),
            AttachOutcome::Reject { .. }
        ));
        assert!(registry.truncated());
        assert_eq!(registry.remove("s1").len(), MAX_SESSION_DEPTH);
        assert!(registry.truncated());
        registry.clear();
        assert!(!registry.truncated());
        for number in 0..MAX_CHILD_SESSIONS {
            assert!(matches!(
                registry.attach(
                    None,
                    &attached(&format!("s{number}"), &format!("f{number}"))
                ),
                AttachOutcome::Added { .. }
            ));
        }
        assert!(matches!(
            registry.attach(None, &attached("overflow", "overflow-frame")),
            AttachOutcome::Reject { .. }
        ));
        assert_eq!(registry.len(), MAX_CHILD_SESSIONS);
        assert!(registry.truncated());
    }

    #[test]
    fn detach_requires_exact_parent_and_removes_only_its_subtree() {
        let mut registry = Registry::default();
        registry.attach(None, &attached("a", "fa"));
        registry.attach(Some("a"), &attached("b", "fb"));
        registry.attach(Some("b"), &attached("c", "fc"));
        registry.attach(None, &attached("other", "fo"));
        assert!(registry.detach(None, "b").is_empty());
        assert!(registry.detach(Some("other"), "b").is_empty());
        assert_eq!(registry.ids(), ["a", "b", "c", "other"]);
        assert_eq!(registry.detach(Some("a"), "b"), ["b", "c"]);
        assert!(!registry.mark_ready("b"));
        assert!(registry.contains("a"));
        assert!(registry.contains("other"));
        assert_eq!(registry.detach(None, "a"), ["a"]);
        assert!(!registry.truncated());
        assert_eq!(registry.remove("other"), ["other"]);
        assert!(registry.truncated());
        registry.clear();
        assert!(registry.is_empty());
        assert!(!registry.truncated());
    }

    #[test]
    fn frame_tree_keeps_cdp_identity_and_structural_parent_without_merging() {
        let result = json!({"frameTree": {
            "frame":{"id":"root","parentId":"outer","url":"https://frame.test/"},
            "childFrames":[
                {"frame":{"id":"child","parentId":"root","name":"first","url":"about:blank"}},
                {"frame":{"id":"child","url":"about:blank"}}
            ]
        }});
        let tree = normalize_frame_tree(&result, Some("child-session")).unwrap();
        assert!(!tree.truncated);
        assert_eq!(tree.frames.len(), 3);
        assert_eq!(tree.frames[0].parent_id.as_deref(), Some("outer"));
        assert_eq!(tree.frames[1].parent_id.as_deref(), Some("root"));
        assert_eq!(tree.frames[2].parent_id.as_deref(), Some("root"));
        assert_eq!(tree.frames[2].frame_id, "child");
        assert_eq!(tree.frames[2].session_id.as_deref(), Some("child-session"));
        assert!(normalize_frame_tree(&result, Some("bad\n")).is_err());
        assert!(
            normalize_frame_tree(
                &json!({"frameTree":{"frame":{"id":"한글","url":"about:blank"}}}),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn frame_tree_clamps_utf8_and_omits_invalid_child_branches() {
        let result = json!({"frameTree": {
            "frame":{"id":"root","url":"한".repeat(2000),"name":"é".repeat(200)},
            "childFrames":[
                {"frame":{"id":"bad","parentId":"other","url":"about:blank"},"childFrames":[{"frame":{"id":"hidden","url":"about:blank"}}]},
                {"frame":{"id":"bad-id\0","url":"about:blank"}},
                {"frame":{"id":"good","url":"about:blank"}}
            ]
        }});
        let tree = normalize_frame_tree(&result, None).unwrap();
        assert!(tree.truncated);
        assert_eq!(tree.frames.len(), 2);
        assert_eq!(tree.frames[0].url.len(), 4095);
        assert_eq!(tree.frames[0].name.len(), MAX_FRAME_NAME_BYTES);
        assert_eq!(tree.frames[1].frame_id, "good");
    }

    #[test]
    fn frame_tree_count_and_depth_budgets_are_separate_from_session_limits() {
        let children = (0..MAX_FRAMES + 1)
            .map(|index| json!({"frame":{"id":format!("f{index}"),"url":"about:blank"}}))
            .collect::<Vec<_>>();
        let wide =
            json!({"frameTree":{"frame":{"id":"root","url":"about:blank"},"childFrames":children}});
        let tree = normalize_frame_tree(&wide, None).unwrap();
        assert_eq!(tree.frames.len(), MAX_FRAMES);
        assert!(tree.truncated);
        let mut deep = json!({"frame":{"id":"leaf","url":"about:blank"}});
        for index in 0..MAX_FRAME_DEPTH {
            deep = json!({"frame":{"id":format!("f{index}"),"url":"about:blank"},"childFrames":[deep]});
        }
        let tree = normalize_frame_tree(&json!({"frameTree":deep}), None).unwrap();
        assert_eq!(tree.frames.len(), MAX_FRAME_DEPTH);
        assert!(tree.truncated);
    }
}
