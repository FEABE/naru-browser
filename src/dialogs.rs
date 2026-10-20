//! Bounded model for JavaScript dialogs held for AI-owned tabs.
//!
//! This holds metadata only. The CEF callbacks stay in `dialog_runtime` so this
//! module can be tested without a CEF runtime.

use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const MAX_PER_TAB: usize = 1;
/// Global cap for task-owned entries. User-owned entries (handed off) are bounded
/// by the per-tab cap only, so a user's unanswered dialogs never starve the AI.
pub const MAX_PENDING: usize = 4;
pub const MAX_MESSAGE_CHARS: usize = 2048;
pub const MAX_TEXT_BYTES: usize = 4096;
/// Original prompt default kept for accept-without-TEXT (display copy is shorter, up to 65536 characters).
pub const MAX_DEFAULT_PROMPT_CHARS: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Alert,
    Confirm,
    Prompt,
    BeforeUnload,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::Confirm => "confirm",
            Self::Prompt => "prompt",
            Self::BeforeUnload => "beforeunload",
        }
    }
}

/// Who may answer a queued dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The owning task answers through the CLI.
    Task,
    /// Control was handed to the user; only the native panel may answer.
    User,
}

impl Owner {
    pub fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub token: u64,
    pub owner: Owner,
    pub tab: i32,
    pub kind: Kind,
    pub message: String,
    pub default_prompt: String,
    pub default_prompt_truncated: bool,
    /// Used by accept without TEXT; never shown.
    pub default_prompt_full: String,
    pub origin: String,
}

impl Entry {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "type": self.kind.label(),
            "message": self.message,
            "defaultPrompt": self.default_prompt,
            "origin": self.origin,
            "owner": self.owner.label(),
        });
        if self.default_prompt_truncated {
            value["defaultPromptTruncated"] = json!(true);
        }
        value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TabFull,
    GlobalFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogError {
    pub code: &'static str,
    pub message: &'static str,
}

/// Cut `text` to at most `limit` Unicode scalar values.
pub fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[derive(Default)]
pub struct Model {
    entries: Vec<Entry>,
    next_token: u64,
    overflow_total: u64,
    overflow_by_tab: BTreeMap<i32, u64>,
}

impl Model {
    pub fn insert(
        &mut self,
        tab: i32,
        kind: Kind,
        message: &str,
        default_prompt: &str,
        origin: &str,
    ) -> Result<u64, Refusal> {
        let refusal = if self.entries.iter().filter(|entry| entry.tab == tab).count() >= MAX_PER_TAB
        {
            Some(Refusal::TabFull)
        } else if self
            .entries
            .iter()
            .filter(|entry| entry.owner == Owner::Task)
            .count()
            >= MAX_PENDING
        {
            Some(Refusal::GlobalFull)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            self.overflow_total = self.overflow_total.saturating_add(1);
            let count = self.overflow_by_tab.entry(tab).or_insert(0);
            *count = count.saturating_add(1);
            return Err(refusal);
        }
        self.next_token = self.next_token.saturating_add(1);
        let token = self.next_token;
        self.entries.push(Entry {
            token,
            owner: Owner::Task,
            tab,
            kind,
            message: truncate_chars(message, MAX_MESSAGE_CHARS),
            default_prompt: truncate_chars(default_prompt, MAX_MESSAGE_CHARS),
            default_prompt_truncated: default_prompt.chars().count() > MAX_MESSAGE_CHARS,
            default_prompt_full: truncate_chars(default_prompt, MAX_DEFAULT_PROMPT_CHARS),
            origin: origin.to_owned(),
        });
        Ok(token)
    }

    pub fn get(&self, tab: i32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tab == tab)
    }

    /// Mark the tab's queued dialog as owned by the user. True when one exists.
    pub fn hand_to_user(&mut self, tab: i32) -> bool {
        match self.entries.iter_mut().find(|entry| entry.tab == tab) {
            Some(entry) => {
                entry.owner = Owner::User;
                true
            }
            None => false,
        }
    }

    pub fn take(&mut self, tab: i32) -> Option<Entry> {
        let index = self.entries.iter().position(|entry| entry.tab == tab)?;
        Some(self.entries.remove(index))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn rows(&self) -> Vec<Entry> {
        self.entries.clone()
    }

    pub fn overflow_total(&self) -> u64 {
        self.overflow_total
    }

    pub fn overflow_for(&self, tab: i32) -> u64 {
        self.overflow_by_tab.get(&tab).copied().unwrap_or(0)
    }

    /// Forget a closed tab's overflow counter.
    pub fn forget_tab(&mut self, tab: i32) {
        self.overflow_by_tab.remove(&tab);
    }
}

/// Validate an accept request against the pending dialog and return the
/// prompt text to continue with (`None` for non-prompt dialogs).
pub fn accept_text(entry: &Entry, text: Option<&str>) -> Result<Option<String>, DialogError> {
    match (entry.kind, text) {
        (Kind::Prompt, Some(text)) => Ok(Some(text.to_owned())),
        (Kind::Prompt, None) => Ok(Some(entry.default_prompt_full.clone())),
        (_, Some(_)) => Err(DialogError {
            code: "DIALOG_TEXT_NOT_ALLOWED",
            message: "Only a prompt dialog accepts TEXT.",
        }),
        (_, None) => Ok(None),
    }
}

/// Shared by the protocol validator and tests.
pub fn text_is_valid(text: &str) -> bool {
    text.len() <= MAX_TEXT_BYTES && !text.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(model: &mut Model, tab: i32) -> Result<u64, Refusal> {
        model.insert(tab, Kind::Alert, "hello", "", "https://example.test")
    }

    #[test]
    fn one_pending_dialog_per_tab() {
        let mut model = Model::default();
        assert!(insert(&mut model, 1).is_ok());
        assert_eq!(insert(&mut model, 1), Err(Refusal::TabFull));
        assert_eq!(model.len(), 1);
        assert_eq!(model.overflow_total(), 1);
        assert_eq!(model.overflow_for(1), 1);
        assert_eq!(model.overflow_for(2), 0);
    }

    #[test]
    fn four_pending_dialogs_globally() {
        let mut model = Model::default();
        for tab in 1..=4 {
            assert!(insert(&mut model, tab).is_ok());
        }
        assert_eq!(insert(&mut model, 5), Err(Refusal::GlobalFull));
        assert_eq!(model.len(), 4);
        assert_eq!(model.overflow_for(5), 1);
        // Taking one frees a global slot.
        assert!(model.take(2).is_some());
        assert!(insert(&mut model, 5).is_ok());
    }

    #[test]
    fn take_is_per_tab_and_idempotent() {
        let mut model = Model::default();
        insert(&mut model, 1).unwrap();
        insert(&mut model, 2).unwrap();
        assert_eq!(model.take(1).map(|entry| entry.tab), Some(1));
        assert!(model.take(1).is_none());
        assert!(model.get(2).is_some());
        assert!(!model.is_empty());
    }

    #[test]
    fn message_is_truncated_by_characters() {
        let mut model = Model::default();
        let long = "가".repeat(MAX_MESSAGE_CHARS + 50);
        model
            .insert(1, Kind::Prompt, &long, &long, "https://example.test")
            .unwrap();
        let entry = model.get(1).unwrap();
        assert_eq!(entry.message.chars().count(), MAX_MESSAGE_CHARS);
        assert_eq!(entry.default_prompt.chars().count(), MAX_MESSAGE_CHARS);
        assert!(entry.default_prompt_truncated);
        assert_eq!(accept_text(entry, None), Ok(Some(long.clone())));
        let json = entry.to_json();
        assert_eq!(json["defaultPromptTruncated"], true);
        assert_eq!(json["type"], "prompt");
        assert_eq!(json["origin"], "https://example.test");
    }

    #[test]
    fn accept_text_rules() {
        let mut model = Model::default();
        model
            .insert(1, Kind::Prompt, "name?", "anon", "https://a.test")
            .unwrap();
        model
            .insert(2, Kind::Confirm, "sure?", "", "https://a.test")
            .unwrap();
        let prompt = model.get(1).unwrap();
        assert_eq!(accept_text(prompt, Some("bob")), Ok(Some("bob".into())));
        assert_eq!(accept_text(prompt, None), Ok(Some("anon".into())));
        let confirm = model.get(2).unwrap();
        assert_eq!(accept_text(confirm, None), Ok(None));
        assert_eq!(
            accept_text(confirm, Some("x")).unwrap_err().code,
            "DIALOG_TEXT_NOT_ALLOWED"
        );
    }

    #[test]
    fn user_owned_entries_keep_the_tab_cap_but_free_a_global_slot() {
        let mut model = Model::default();
        insert(&mut model, 1).unwrap();
        assert_eq!(model.get(1).unwrap().owner, Owner::Task);
        assert!(model.hand_to_user(1));
        assert!(!model.hand_to_user(2));
        assert_eq!(model.get(1).unwrap().owner, Owner::User);
        assert_eq!(model.get(1).unwrap().to_json()["owner"], "user");
        // Still one per tab, but no longer counted against the global cap.
        assert_eq!(insert(&mut model, 1), Err(Refusal::TabFull));
        for tab in 2..=5 {
            insert(&mut model, tab).unwrap();
        }
        assert_eq!(model.len(), 5);
        assert_eq!(insert(&mut model, 6), Err(Refusal::GlobalFull));
        assert_eq!(model.take(1).map(|entry| entry.owner), Some(Owner::User));
    }

    #[test]
    fn text_validation_limits() {
        assert!(text_is_valid(""));
        assert!(text_is_valid(&"a".repeat(MAX_TEXT_BYTES)));
        assert!(!text_is_valid(&"a".repeat(MAX_TEXT_BYTES + 1)));
        assert!(!text_is_valid("a\0b"));
    }

    #[test]
    fn forgetting_a_tab_clears_its_overflow_counter() {
        let mut model = Model::default();
        insert(&mut model, 1).unwrap();
        let _ = insert(&mut model, 1);
        model.forget_tab(1);
        assert_eq!(model.overflow_for(1), 0);
        assert_eq!(model.overflow_total(), 1);
    }
}
