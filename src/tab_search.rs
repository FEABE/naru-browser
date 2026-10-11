//! Bounded open-tab matching and a shared tab/bookmark result selection model.
//! Bookmark matches come from the existing runtime; this module stores nothing.

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
}

/// Keep the current tab order first, then bookmark order, within one UI budget.
pub fn combined_results(
    tabs: impl IntoIterator<Item = i32>,
    bookmarks: impl IntoIterator<Item = u64>,
) -> Vec<SearchResultId> {
    tabs.into_iter()
        .map(SearchResultId::Tab)
        .chain(bookmarks.into_iter().map(SearchResultId::Bookmark))
        .take(MAX_RESULTS)
        .collect()
}

pub fn bounded_query(value: &str) -> String {
    value.chars().take(MAX_QUERY_CHARS).collect()
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
            words
                .iter()
                .all(|word| fields.iter().any(|field| field.contains(word)))
        })
        .map(|tab| tab.id)
        .take(MAX_RESULTS)
        .collect()
}

#[derive(Default)]
pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub selected: Option<SearchResultId>,
}

impl SearchState {
    pub fn set_query(&mut self, value: &str) {
        let query = bounded_query(value);
        if self.query != query {
            self.query = query;
            self.selected = None;
        }
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
        use SearchResultId::{Bookmark, Tab};
        assert_eq!(
            combined_results([7], [7, u64::MAX]),
            [Tab(7), Bookmark(7), Bookmark(u64::MAX)]
        );
        let results = combined_results(1..=25, 1..=30);
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[24], Tab(25));
        assert_eq!(results[25], Bookmark(1));
        assert_eq!(results.last(), Some(&Bookmark(15)));
        assert_eq!(combined_results(1..=60, [7]).len(), MAX_RESULTS);
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
