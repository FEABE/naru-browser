//! Small, process-local URL history. No Browser, Frame or document is retained.

use std::collections::VecDeque;

pub const MAX_CLOSED_PAGES: usize = 10;
pub const MAX_CLOSED_URL_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub token: u64,
    pub url: String,
}

#[derive(Default)]
pub struct ClosedPages {
    entries: VecDeque<Entry>,
    sequence: u64,
    pending: Option<u64>,
}

impl ClosedPages {
    pub fn record(&mut self, url: &str) -> bool {
        if url.len() > MAX_CLOSED_URL_BYTES {
            return false;
        }
        let Ok(url) = crate::address::normalize_url(url) else {
            return false;
        };
        let Some(token) = self.sequence.checked_add(1) else {
            return false;
        };
        self.sequence = token;
        if self.entries.len() == MAX_CLOSED_PAGES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { token, url });
        true
    }

    pub fn can_reopen(&self) -> bool {
        self.pending.is_none() && !self.entries.is_empty()
    }

    // Keep the original until creation succeeds. CEF creation may reenter and
    // close another tab; completion must remove its exact token, not the tail.
    // At most ten stored URLs plus this one in-flight clone are owned.
    pub fn begin_reopen(&mut self) -> Option<Entry> {
        if self.pending.is_some() {
            return None;
        }
        let entry = self.entries.back()?.clone();
        self.pending = Some(entry.token);
        Some(entry)
    }

    pub fn finish_reopen(&mut self, token: u64, success: bool) {
        if self.pending != Some(token) {
            return;
        }
        self.pending = None;
        if success {
            self.entries.retain(|entry| entry.token != token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_with_a_fixed_ten_url_bound() {
        let mut pages = ClosedPages::default();
        for index in 0..12 {
            assert!(pages.record(&format!("https://example.test/{index}")));
        }
        assert_eq!(pages.entries.len(), 10);
        for index in (2..12).rev() {
            let entry = pages.begin_reopen().unwrap();
            assert_eq!(entry.url, format!("https://example.test/{index}"));
            pages.finish_reopen(entry.token, true);
        }
        assert!(!pages.can_reopen());
    }

    #[test]
    fn rejects_unsupported_credentials_and_oversize_without_evicting() {
        let mut pages = ClosedPages::default();
        assert!(pages.record("https://example.test/kept"));
        for invalid in [
            "about:blank",
            "file:///tmp/a",
            "javascript:alert(1)",
            "data:text/plain,a",
            "https://user:password@example.test/",
            "not a URL",
        ] {
            assert!(!pages.record(invalid));
        }
        assert!(!pages.record(&format!("https://example.test/{}", "a".repeat(8192))));
        assert!(!pages.record(&format!("https://example.test/{}", "가".repeat(1100))));
        assert_eq!(pages.entries.len(), 1);
        assert_eq!(
            pages.begin_reopen().unwrap().url,
            "https://example.test/kept"
        );
    }

    #[test]
    fn failed_creation_retains_entry_and_blocks_duplicate_reentry() {
        let mut pages = ClosedPages::default();
        pages.record("https://example.test/a");
        let entry = pages.begin_reopen().unwrap();
        assert!(!pages.can_reopen());
        assert!(pages.begin_reopen().is_none());
        pages.finish_reopen(entry.token + 1, true);
        assert!(!pages.can_reopen());
        pages.finish_reopen(entry.token, false);
        assert_eq!(pages.begin_reopen(), Some(entry));
    }

    #[test]
    fn completion_only_consumes_its_entry_despite_reentrant_closes() {
        let mut pages = ClosedPages::default();
        pages.record("https://example.test/a");
        let first = pages.begin_reopen().unwrap();
        pages.record("https://example.test/b");
        pages.finish_reopen(first.token, true);
        let second = pages.begin_reopen().unwrap();
        assert_eq!(second.url, "https://example.test/b");
        pages.finish_reopen(first.token, true);
        assert!(!pages.can_reopen());
        pages.finish_reopen(second.token, true);
        assert!(pages.entries.is_empty());
    }

    #[test]
    fn eviction_during_reopen_and_token_exhaustion_keep_bounds() {
        let mut pages = ClosedPages::default();
        pages.record("https://example.test/old");
        let first = pages.begin_reopen().unwrap();
        for index in 0..10 {
            pages.record(&format!("https://example.test/{index}"));
        }
        pages.finish_reopen(first.token, true);
        assert_eq!(pages.entries.len(), 10);
        pages.sequence = u64::MAX;
        assert!(!pages.record("https://example.test/overflow"));
        assert_eq!(pages.entries.len(), 10);
        assert_eq!(pages.entries.back().unwrap().url, "https://example.test/9");
    }
}
