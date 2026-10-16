//! Bounded tab, bookmark and history matching and native result selection.
//! Persisted matches come from their runtimes; this module stores nothing.

use std::collections::HashSet;

pub const MAX_QUERY_CHARS: usize = 200;
pub const MAX_RESULTS: usize = 40;
const MAX_FIELD_CHARS: usize = 4096;

pub struct SearchTab<'a> {
    pub id: i32,
    pub title: &'a str,
    pub url: &'a str,
    pub space_name: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchResultId {
    Tab(i32),
    Bookmark(u64),
    History(u64),
    Query(u64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    All,
    History,
}

/// Keep source order within one budget. History omits URLs already shown by a
/// tab or bookmark, while those two explicit result kinds remain independent.
pub fn combined_results<'a>(
    tabs: impl IntoIterator<Item = (i32, &'a str)>,
    bookmarks: impl IntoIterator<Item = (u64, &'a str)>,
    history: impl IntoIterator<Item = (u64, &'a str)>,
) -> Vec<SearchResultId> {
    let mut results = Vec::new();
    let mut urls = HashSet::new();
    for (id, url) in tabs
        .into_iter()
        .map(|(id, url)| (SearchResultId::Tab(id), url))
        .chain(
            bookmarks
                .into_iter()
                .map(|(id, url)| (SearchResultId::Bookmark(id), url)),
        )
        .take(MAX_RESULTS)
    {
        results.push(id);
        if let Ok(url) = crate::address::normalize_url(url) {
            urls.insert(url);
        }
    }
    if results.len() == MAX_RESULTS {
        return results;
    }
    for (id, url) in history {
        let Ok(url) = crate::address::normalize_url(url) else {
            continue;
        };
        if urls.insert(url) {
            results.push(SearchResultId::History(id));
            if results.len() == MAX_RESULTS {
                break;
            }
        }
    }
    results
}

/// Keep internal matches first, reserving one of the existing result slots for
/// an explicit navigation action. This does not issue a network request.
pub fn append_query_result(results: &mut Vec<SearchResultId>, revision: u64) {
    results.truncate(MAX_RESULTS - 1);
    results.push(SearchResultId::Query(revision));
}

pub fn bounded_query(value: &str) -> String {
    value.chars().take(MAX_QUERY_CHARS).collect()
}

pub fn display_title<'a>(title: &'a str, url: &str) -> &'a str {
    if title.is_empty() || url == "about:blank" {
        "새 탭"
    } else {
        title
    }
}

pub fn search<'a>(tabs: impl IntoIterator<Item = SearchTab<'a>>, query: &str) -> Vec<i32> {
    let query = bounded_query(query).to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    tabs.into_iter()
        .filter(|tab| {
            if words.is_empty() {
                return true;
            }
            let fields = [tab.title, tab.url, tab.space_name].map(|field| {
                field
                    .chars()
                    .take(MAX_FIELD_CHARS)
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            });
            // Only the fixed, short Korean fallback can differ from the raw
            // title. Match it by reference without copying normal titles or
            // bypassing the raw fields' character limit.
            let displayed = display_title(tab.title, tab.url);
            let alias = (displayed != tab.title).then_some(displayed);
            words.iter().all(|word| {
                fields.iter().any(|field| field.contains(word))
                    || alias.is_some_and(|alias| alias.contains(word))
            })
        })
        .map(|tab| tab.id)
        .take(MAX_RESULTS)
        .collect()
}

#[derive(Default)]
pub struct SearchState {
    pub open: bool,
    pub mode: SearchMode,
    pub query: String,
    pub query_overflow: bool,
    pub revision: u64,
    pub selected: Option<SearchResultId>,
    pub confirm_clear: bool,
    pub notice: Option<String>,
}

impl SearchState {
    pub fn open(&mut self, mode: SearchMode) {
        if !self.open {
            self.close();
            self.open = true;
        }
        self.set_mode(mode);
    }

    pub fn close(&mut self) {
        let revision = self.revision.saturating_add(1);
        *self = Self::default();
        self.revision = revision;
    }

    pub fn set_mode(&mut self, mode: SearchMode) {
        if self.mode != mode {
            self.revision = self.revision.saturating_add(1);
            self.mode = mode;
            self.selected = None;
            self.confirm_clear = false;
            self.notice = None;
        }
    }

    pub fn set_query(&mut self, value: &str) {
        let query = bounded_query(value);
        // A programmatic write of the bounded text must not turn a truncated
        // address into an executable URL. Require a genuinely different edit.
        let overflow = value.chars().nth(MAX_QUERY_CHARS).is_some()
            || (self.query_overflow && self.query == query);
        if self.query != query || self.query_overflow != overflow {
            self.query = query;
            self.query_overflow = overflow;
            self.revision = self.revision.saturating_add(1);
            self.selected = None;
            self.notice = None;
        }
    }

    pub fn query_destination(
        &self,
    ) -> Result<Option<(String, crate::address::AddressKind)>, crate::address::AddressError> {
        if !self.open
            || self.mode != SearchMode::All
            || self.query_overflow
            || self.confirm_clear
            || self.revision == u64::MAX
        {
            return Ok(None);
        }
        crate::address::resolve_with_kind(&self.query)
    }

    pub fn reconcile(&mut self, results: &[SearchResultId]) {
        if !self
            .selected
            .is_some_and(|selected| results.contains(&selected))
        {
            self.selected = results.first().copied();
        }
    }

    pub fn move_selection(&mut self, results: &[SearchResultId], forward: bool) {
        self.reconcile(results);
        let Some(index) = results.iter().position(|id| Some(*id) == self.selected) else {
            return;
        };
        let next = if forward {
            (index + 1).min(results.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        self.selected = Some(results[next]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_action_is_last_within_the_existing_budget_and_internal_matches_stay_first() {
        use SearchResultId::{Query, Tab};
        let mut results: Vec<_> = (1..=MAX_RESULTS as i32).map(Tab).collect();
        append_query_result(&mut results, 9);
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(
            results[..MAX_RESULTS - 1],
            (1..MAX_RESULTS as i32).map(Tab).collect::<Vec<_>>()
        );
        assert_eq!(results.last(), Some(&Query(9)));

        let mut state = SearchState::default();
        state.reconcile(&results);
        assert_eq!(state.selected, Some(Tab(1)));
        state.selected = Some(Query(9));
        let mut refreshed = vec![Tab(12)];
        append_query_result(&mut refreshed, 9);
        state.reconcile(&refreshed);
        assert_eq!(state.selected, Some(Query(9)));
        let mut empty = Vec::new();
        append_query_result(&mut empty, 10);
        state.reconcile(&empty);
        assert_eq!(state.selected, Some(Query(10)));
    }

    #[test]
    fn only_explicit_all_mode_queries_offer_resolved_addresses_or_searches() {
        use crate::address::{AddressError, AddressKind};
        let mut state = SearchState::default();
        state.open(SearchMode::All);
        assert_eq!(state.query_destination(), Ok(None));
        for (query, kind) in [
            ("example.test/path", AddressKind::Url),
            ("https://www.google.com/search?q=direct", AddressKind::Url),
            ("한글 검색", AddressKind::Search),
            ("site:example.test", AddressKind::Search),
        ] {
            state.set_query(query);
            let (url, actual_kind) = state.query_destination().unwrap().unwrap();
            assert_eq!(actual_kind, kind);
            assert_eq!(Some(url), crate::address::resolve(query).unwrap());
        }
        for (query, error) in [
            ("file:///tmp/file", AddressError::UnsupportedScheme),
            (
                "https://name:secret@example.test/",
                AddressError::Credentials,
            ),
            ("https://", AddressError::InvalidAddress),
        ] {
            state.set_query(query);
            assert_eq!(state.query_destination(), Err(error));
        }
        state.set_query("unmatched search");
        state.set_mode(SearchMode::History);
        assert_eq!(state.query_destination(), Ok(None));
        state.set_mode(SearchMode::All);
        state.confirm_clear = true;
        assert_eq!(state.query_destination(), Ok(None));
        state.close();
        assert_eq!(state.query_destination(), Ok(None));
    }

    #[test]
    fn truncated_queries_require_a_different_edit_before_navigation_is_enabled() {
        for raw in [
            format!("https://example.test/{}", "a".repeat(MAX_QUERY_CHARS)),
            "나".repeat(MAX_QUERY_CHARS + 1),
        ] {
            let mut state = SearchState::default();
            state.open(SearchMode::All);
            state.set_query(&raw);
            assert!(state.query_overflow);
            assert_eq!(state.query.chars().count(), MAX_QUERY_CHARS);
            // The shortened text parses successfully; opening it would change
            // the user's input rather than report the truncation.
            assert!(crate::address::resolve(&state.query).unwrap().is_some());
            assert_eq!(state.query_destination(), Ok(None));
            let bounded = state.query.clone();
            let revision = state.revision;
            state.set_query(&bounded); // native/programmatic field writeback
            assert!(state.query_overflow);
            assert_eq!(state.revision, revision);
            state.set_mode(SearchMode::History);
            state.set_mode(SearchMode::All);
            assert_eq!(state.query_destination(), Ok(None));
            state.set_query("example.test/edited");
            assert!(!state.query_overflow);
            assert!(state.query_destination().unwrap().is_some());
        }
    }

    #[test]
    fn query_revision_survives_reopen_and_never_reuses_a_saturated_target() {
        let mut state = SearchState::default();
        state.open(SearchMode::All);
        state.set_query("same input");
        let first = state.revision;
        state.set_query("same input");
        assert_eq!(state.revision, first);
        state.set_query("other input");
        state.set_query("same input");
        assert!(state.revision > first);
        let before_mode = state.revision;
        state.set_mode(SearchMode::History);
        state.set_mode(SearchMode::All);
        assert!(state.revision > before_mode);
        let before_close = state.revision;
        state.close();
        state.open(SearchMode::All);
        state.set_query("same input");
        assert!(state.revision > before_close);
        state.revision = u64::MAX - 1;
        state.set_query("last edit");
        assert_eq!(state.revision, u64::MAX);
        assert_eq!(state.query_destination(), Ok(None));
        state.close();
        state.open(SearchMode::All);
        state.set_query("same input");
        assert_eq!(state.revision, u64::MAX);
        assert_eq!(state.query_destination(), Ok(None));
        state.reconcile(&[SearchResultId::Tab(7)]);
        assert_eq!(state.selected, Some(SearchResultId::Tab(7)));
    }

    #[test]
    fn visible_new_tab_alias_preserves_raw_matches_without_matching_ordinary_tabs() {
        let tabs = || {
            [
                SearchTab {
                    id: 1,
                    title: "",
                    url: "about:blank",
                    space_name: "내 공간",
                },
                SearchTab {
                    id: 2,
                    title: "Hidden Original",
                    url: "about:blank",
                    space_name: "AI 작업",
                },
                SearchTab {
                    id: 3,
                    title: "",
                    url: "https://example.test/untitled",
                    space_name: "내 공간",
                },
                SearchTab {
                    id: 4,
                    title: "상품 관리",
                    url: "https://example.test/products",
                    space_name: "내 공간",
                },
            ]
        };
        assert_eq!(display_title("Hidden Original", "about:blank"), "새 탭");
        assert_eq!(display_title("", "https://example.test/untitled"), "새 탭");
        assert_eq!(
            display_title("상품 관리", "https://example.test/products"),
            "상품 관리"
        );
        assert_eq!(search(tabs(), "새"), [1, 2, 3]);
        assert_eq!(search(tabs(), "새 탭"), [1, 2, 3]);
        assert_eq!(search(tabs(), "about:blank"), [1, 2]);
        assert_eq!(search(tabs(), "ORIGINAL 새 작업"), [2]);
        assert_eq!(search(tabs(), "새 untitled"), [3]);
        assert_eq!(search(tabs(), "상품"), [4]);
        assert!(search(tabs(), "새 products").is_empty());
        let long_title = format!("{}needle", "a".repeat(MAX_FIELD_CHARS));
        assert!(
            search(
                [SearchTab {
                    id: 5,
                    title: &long_title,
                    url: "https://example.test/",
                    space_name: "내 공간"
                }],
                "needle"
            )
            .is_empty()
        );
    }

    #[test]
    fn searches_all_words_across_title_url_and_space_without_reordering() {
        let tabs = || {
            [
                SearchTab {
                    id: 3,
                    title: "한글 상품",
                    url: "https://SHOP.test/",
                    space_name: "AI 작업",
                },
                SearchTab {
                    id: 7,
                    title: "Shop",
                    url: "https://other.test/",
                    space_name: "내 공간",
                },
            ]
        };
        assert_eq!(search(tabs(), "상품 shop 작업"), [3]);
        assert_eq!(search(tabs(), "SHOP"), [3, 7]);
        assert_eq!(search(tabs(), "내 공간"), [7]);
        assert!(search(tabs(), "없는 내용").is_empty());
    }

    #[test]
    fn caps_query_and_results_without_splitting_korean() {
        let query = "한".repeat(MAX_QUERY_CHARS + 5);
        assert_eq!(bounded_query(&query), "한".repeat(MAX_QUERY_CHARS));
        let results = search(
            (1..=100).map(|id| SearchTab {
                id,
                title: "",
                url: "",
                space_name: "",
            }),
            " ",
        );
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results.last(), Some(&(MAX_RESULTS as i32)));
    }

    #[test]
    fn background_refresh_retains_selection_but_closed_tabs_are_removed() {
        use SearchResultId::Tab;
        let mut state = SearchState::default();
        state.reconcile(&[Tab(2), Tab(8), Tab(11)]);
        state.move_selection(&[Tab(2), Tab(8), Tab(11)], true);
        assert_eq!(state.selected, Some(Tab(8)));
        state.reconcile(&[Tab(1), Tab(2), Tab(8), Tab(11)]);
        assert_eq!(state.selected, Some(Tab(8)));
        state.reconcile(&[Tab(2), Tab(11)]);
        assert_eq!(state.selected, Some(Tab(2)));
        state.move_selection(&[Tab(2), Tab(11)], false);
        assert_eq!(state.selected, Some(Tab(2)));
        state.set_query("다른 검색");
        assert_eq!(state.selected, None);
        state.reconcile(&[]);
        state.move_selection(&[], true);
        assert_eq!(state.selected, None);
    }

    #[test]
    fn mixed_results_share_one_limit_and_keep_their_kind_when_ids_overlap() {
        use SearchResultId::{Bookmark, History, Tab};
        assert_eq!(
            combined_results(
                [(7, "https://example.test/tab")],
                [
                    (7, "https://example.test/bookmark"),
                    (u64::MAX, "https://example.test/last")
                ],
                [(7, "https://example.test/history")]
            ),
            [Tab(7), Bookmark(7), Bookmark(u64::MAX), History(7)]
        );
        let results = combined_results(
            (1..=25).map(|id| (id, "about:blank")),
            (1..=30).map(|id| (id, "https://example.test/")),
            [(7, "https://example.test/history")],
        );
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[24], Tab(25));
        assert_eq!(results[25], Bookmark(1));
        assert_eq!(results.last(), Some(&Bookmark(15)));
        assert_eq!(
            combined_results(
                (1..=60).map(|id| (id, "about:blank")),
                [(7, "https://example.test/")],
                []
            )
            .len(),
            MAX_RESULTS
        );
    }

    #[test]
    fn history_omits_normalized_duplicates_but_keeps_runtime_recent_order() {
        use SearchResultId::{Bookmark, History, Tab};
        let history = [
            (9, "HTTPS://example.test/a b"),
            (8, "https://example.test/bookmark"),
            (7, "https://example.test/recent"),
            (6, "https://example.test/older"),
            (5, "https://example.test/recent"),
            (4, "file:///tmp/not-history"),
        ];
        assert_eq!(
            combined_results(
                [(1, "https://example.test/a%20b")],
                [(1, "https://example.test/bookmark")],
                history
            ),
            [Tab(1), Bookmark(1), History(7), History(6)]
        );
        // History-only mode supplies no tab/bookmark results, so it exposes
        // records even when the same URLs are open or bookmarked elsewhere.
        assert_eq!(
            combined_results([], [], history),
            [History(9), History(8), History(7), History(6)]
        );
        let urls: Vec<_> = (0..60)
            .map(|id| format!("https://example.test/{id}"))
            .collect();
        let result = combined_results(
            [],
            [],
            urls.iter()
                .enumerate()
                .map(|(id, url)| (id as u64, url.as_str())),
        );
        assert_eq!(result.len(), MAX_RESULTS);
        assert_eq!(result.last(), Some(&History(39)));
    }

    #[test]
    fn search_modes_and_history_clear_preserve_query_without_reusing_deleted_selection() {
        use SearchResultId::{History, Tab};
        let mut state = SearchState {
            open: true,
            ..SearchState::default()
        };
        state.set_query("한글 검색");
        state.reconcile(&[Tab(7), History(7)]);
        state.set_mode(SearchMode::History);
        assert_eq!(state.query, "한글 검색");
        assert_eq!(state.selected, None);
        state.reconcile(&[History(7), History(9)]);
        state.selected = Some(History(9));
        state.reconcile(&[History(10), History(7), History(9)]);
        assert_eq!(state.selected, Some(History(9)));
        state.reconcile(&[]); // Clear immediately invalidates the old history IDs.
        assert_eq!(state.selected, None);
        assert_eq!(state.query, "한글 검색");
        assert_eq!(state.mode, SearchMode::History);
        state.confirm_clear = true;
        state.notice = Some("retry".into());
        state.set_mode(SearchMode::All);
        assert!(!state.confirm_clear);
        assert!(state.notice.is_none());
        assert_eq!(state.query, "한글 검색");
        state.reconcile(&[Tab(7)]);
        assert_eq!(state.selected, Some(Tab(7)));
    }

    #[test]
    fn bookmark_selection_survives_refresh_but_not_deletion_or_a_new_query() {
        use SearchResultId::{Bookmark, Tab};
        let mut state = SearchState::default();
        state.reconcile(&[Tab(7), Bookmark(7)]);
        state.move_selection(&[Tab(7), Bookmark(7)], true);
        assert_eq!(state.selected, Some(Bookmark(7)));
        state.reconcile(&[Tab(2), Tab(7), Bookmark(7)]);
        assert_eq!(state.selected, Some(Bookmark(7)));
        state.reconcile(&[Tab(7)]);
        assert_eq!(state.selected, Some(Tab(7)));
        state.set_query("상품");
        assert_eq!(state.selected, None);
        state.reconcile(&[Bookmark(9)]);
        assert_eq!(state.selected, Some(Bookmark(9)));
        state.set_query("상품");
        assert_eq!(state.selected, Some(Bookmark(9)));
        state.reconcile(&[]);
        assert_eq!(state.selected, None);
    }
}
