//! Presentation spaces share the existing profile, but keep tab selection separate.
//! Task authorization remains in `tasks::Registry`.

use crate::tasks::{Origin, Status, TaskView};
use serde::Serialize;
use std::collections::BTreeMap;

pub const USER_SPACE: &str = "user";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub revision: u64,
    pub tabs: Vec<i32>,
    pub selected_tab: Option<i32>,
    pub active: bool,
}

struct Space {
    name: String,
    status: String,
    revision: u64,
    selected: Option<i32>,
    tabs: Vec<i32>,
}

pub struct Spaces {
    records: BTreeMap<String, Space>,
    tabs: BTreeMap<i32, String>,
    // An explicitly adopted tab returns to its original presentation space.
    borrowed_from: BTreeMap<i32, String>,
    active: String,
    overview: bool,
}

impl Default for Spaces {
    fn default() -> Self {
        let mut records = BTreeMap::new();
        records.insert(
            USER_SPACE.into(),
            Space {
                name: "내 공간".into(),
                status: "user".into(),
                revision: 0,
                selected: None,
                tabs: Vec::new(),
            },
        );
        Self {
            records,
            tabs: BTreeMap::new(),
            borrowed_from: BTreeMap::new(),
            active: USER_SPACE.into(),
            overview: false,
        }
    }
}

impl Spaces {
    pub fn active_space(&self) -> &str {
        &self.active
    }
    pub fn active_tab(&self) -> Option<i32> {
        self.records
            .get(&self.active)
            .and_then(|space| space.selected)
    }
    pub fn overview(&self) -> bool {
        self.overview
    }
    pub fn set_overview(&mut self, overview: bool) {
        self.overview = overview;
    }
    pub fn contains(&self, id: &str) -> bool {
        self.records.contains_key(id)
    }
    pub fn tab_space(&self, tab: i32) -> Option<&str> {
        self.tabs.get(&tab).map(String::as_str)
    }
    pub fn ordered_tabs(&self, space: &str) -> &[i32] {
        self.records
            .get(space)
            .map_or(&[], |record| record.tabs.as_slice())
    }

    /// Move the currently selected tab one place in its visible space. The
    /// expected neighbor and direction reject a menu target that became stale.
    pub fn reorder_adjacent(
        &mut self,
        tab: i32,
        expected_space: &str,
        neighbor: i32,
        forward: bool,
    ) -> bool {
        if self.overview
            || self.active != expected_space
            || self.tab_space(tab) != Some(expected_space)
            || self.tab_space(neighbor) != Some(expected_space)
        {
            return false;
        }
        let Some(space) = self.records.get_mut(expected_space) else {
            return false;
        };
        if space.selected != Some(tab) {
            return false;
        }
        let Some(index) = space.tabs.iter().position(|&candidate| candidate == tab) else {
            return false;
        };
        let adjacent = if forward {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        };
        let Some(adjacent) = adjacent.filter(|&index| space.tabs.get(index) == Some(&neighbor))
        else {
            return false;
        };
        space.tabs.swap(index, adjacent);
        true
    }

    /// Place a tab before another tab, or at the end, without selecting it.
    /// A drag uses its original visible order so background changes cancel it.
    pub fn reorder_before(
        &mut self,
        tab: i32,
        expected_space: &str,
        before: Option<i32>,
        expected_order: &[i32],
    ) -> Option<bool> {
        if self.overview
            || self.active != expected_space
            || self.tab_space(tab) != Some(expected_space)
            || before == Some(tab)
            || before.is_some_and(|id| self.tab_space(id) != Some(expected_space))
        {
            return None;
        }
        let space = self.records.get_mut(expected_space)?;
        if space.tabs != expected_order {
            return None;
        }
        let from = space.tabs.iter().position(|&id| id == tab)?;
        let boundary = match before {
            Some(id) => space.tabs.iter().position(|&candidate| candidate == id)?,
            None => space.tabs.len(),
        };
        let to = if from < boundary {
            boundary - 1
        } else {
            boundary
        };
        if from == to {
            return Some(false);
        }
        if from < to {
            space.tabs[from..=to].rotate_left(1);
        } else {
            space.tabs[to..=from].rotate_right(1);
        }
        Some(true)
    }

    pub fn update_tasks(&mut self, tasks: &[TaskView]) {
        for task in tasks {
            let status = match task.status {
                Status::Active => "active",
                Status::Paused => "paused",
                Status::Finishing => "finishing",
                Status::Finished => "finished",
            };
            // Finished tasks with no retained page do not accumulate UI cards.
            if task.status == Status::Finished
                && !self.tabs.values().any(|id| id == &task.id)
                && !self.borrowed_from.values().any(|id| id == &task.id)
            {
                self.records.remove(&task.id);
                continue;
            }
            let space = self
                .records
                .entry(task.id.clone())
                .or_insert_with(|| Space {
                    name: task.name.clone(),
                    status: status.into(),
                    revision: task.revision,
                    selected: None,
                    tabs: Vec::new(),
                });
            space.name.clone_from(&task.name);
            space.status = status.into();
            space.revision = task.revision;
        }
        // Task history may evict a finished record. Retained pages still need a
        // place to live, so keep its small presentation record until they close.
        self.records.retain(|id, _| {
            id == USER_SPACE
                || self.tabs.values().any(|space| space == id)
                || self.borrowed_from.values().any(|space| space == id)
                || tasks
                    .iter()
                    .any(|task| &task.id == id && task.status != Status::Finished)
        });
        if !self.records.contains_key(&self.active) {
            self.active = USER_SPACE.into();
        }
    }

    /// `foreground` affects only the currently viewed space, never another one.
    pub fn insert(&mut self, tab: i32, space: &str, foreground: bool) {
        let space = if self.contains(space) {
            space
        } else {
            USER_SPACE
        };
        if let Some(previous) = self.tabs.insert(tab, space.into()) {
            if previous != space {
                self.repair_selection(&previous, tab);
            }
        }
        let entry = self.records.get_mut(space).expect("space exists");
        if !entry.tabs.contains(&tab) {
            entry.tabs.push(tab);
        }
        if entry.selected.is_none() || (foreground && self.active == space && !self.overview) {
            entry.selected = Some(tab);
        }
    }

    pub fn show(&mut self, id: &str) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = id.into();
        self.overview = false;
        true
    }

    pub fn select_tab(&mut self, tab: i32) -> bool {
        let Some(space) = self.tabs.get(&tab) else {
            return false;
        };
        // A stale hidden-space tab button cannot move the user's current space.
        if space != &self.active {
            return false;
        }
        self.records
            .get_mut(space)
            .expect("tab space exists")
            .selected = Some(tab);
        self.overview = false;
        true
    }

    fn move_tab(&mut self, tab: i32, destination: &str) {
        if self.tab_space(tab) == Some(destination) {
            return;
        }
        let Some(previous) = self.tabs.remove(&tab) else {
            return;
        };
        self.repair_selection(&previous, tab);
        self.insert(tab, destination, false);
    }

    pub fn adopt(&mut self, tab: i32, task: &str) {
        if let Some(previous) = self.tabs.get(&tab) {
            self.borrowed_from.insert(tab, previous.clone());
        }
        self.move_tab(tab, task);
    }

    pub fn release(&mut self, tab: i32) {
        let destination = self
            .borrowed_from
            .remove(&tab)
            .unwrap_or_else(|| USER_SPACE.into());
        self.move_tab(tab, &destination);
    }

    pub fn finish(&mut self, tabs: &[(i32, Origin)], kept: &[i32]) {
        for &(tab, origin) in tabs {
            if origin == Origin::Borrowed {
                self.release(tab);
            } else if kept.contains(&tab) {
                // Ownership ends in Registry; a retained result stays in its
                // completed space until the user closes or explicitly adopts it.
                self.borrowed_from.remove(&tab);
            }
        }
    }

    pub fn remove(&mut self, tab: i32) {
        self.borrowed_from.remove(&tab);
        if let Some(space) = self.tabs.remove(&tab) {
            self.repair_selection(&space, tab);
            if space != USER_SPACE
                && self
                    .records
                    .get(&space)
                    .is_some_and(|record| record.status == "finished")
                && !self.tabs.values().any(|id| id == &space)
                && !self.borrowed_from.values().any(|id| id == &space)
            {
                self.records.remove(&space);
                if self.active == space {
                    self.active = USER_SPACE.into();
                }
            }
        }
    }

    fn repair_selection(&mut self, space: &str, removed: i32) {
        if let Some(record) = self.records.get_mut(space) {
            record.tabs.retain(|&tab| tab != removed);
            if record.tabs.is_empty() {
                record.tabs = Vec::new();
            } else if record.tabs.capacity() > 32 && record.tabs.len() <= record.tabs.capacity() / 4
            {
                record.tabs.shrink_to(record.tabs.len() * 2);
            }
            if record.selected == Some(removed) {
                record.selected = record.tabs.first().copied();
            }
        }
    }

    pub fn views(&self) -> Vec<SpaceView> {
        let mut views: Vec<_> = self
            .records
            .iter()
            .map(|(id, space)| SpaceView {
                id: id.clone(),
                name: space.name.clone(),
                kind: if id == USER_SPACE { "user" } else { "task" }.into(),
                status: space.status.clone(),
                revision: space.revision,
                selected_tab: space.selected,
                active: self.active == *id,
                tabs: space.tabs.clone(),
            })
            .collect();
        views.sort_by_key(|view| (view.id != USER_SPACE, view.id.clone()));
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Registry;

    #[test]
    fn background_ai_creation_preserves_user_and_per_space_selection() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let task = tasks.create("Research").unwrap();
        spaces.insert(1, USER_SPACE, true);
        spaces.update_tasks(&tasks.list());
        spaces.insert(2, &task.id, false);
        spaces.insert(3, &task.id, false);
        assert_eq!(spaces.active_tab(), Some(1));
        assert_eq!(spaces.active_space(), USER_SPACE);
        assert!(spaces.show(&task.id));
        assert_eq!(spaces.active_tab(), Some(2));
        assert!(spaces.select_tab(3));
        assert!(spaces.show(USER_SPACE));
        assert!(!spaces.select_tab(2));
        assert!(spaces.show(&task.id));
        assert_eq!(spaces.active_tab(), Some(3));
        assert!(!spaces.show("missing"));
        assert_eq!(spaces.active_space(), task.id);
    }

    #[test]
    fn close_adopt_release_do_not_select_a_foreign_tab() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let task = tasks.create("Research").unwrap();
        spaces.update_tasks(&tasks.list());
        spaces.insert(1, USER_SPACE, true);
        spaces.insert(2, USER_SPACE, true);
        spaces.adopt(2, &task.id);
        assert_eq!(spaces.active_tab(), Some(1));
        spaces.release(2);
        assert_eq!(spaces.active_tab(), Some(1));
        assert_eq!(spaces.tab_space(2), Some(USER_SPACE));
        spaces.remove(1);
        assert_eq!(spaces.active_tab(), Some(2));
    }

    #[test]
    fn kept_results_remain_in_done_space_borrowed_tabs_return() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let mut task = tasks.create("Research").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.insert(1, USER_SPACE, true);
        spaces.adopt(1, &task.id);
        spaces.insert(2, &task.id, false);
        spaces.finish(&[(1, Origin::Borrowed), (2, Origin::Created)], &[2]);
        task.status = Status::Finished;
        spaces.update_tasks(&[task.clone()]);
        assert_eq!(spaces.tab_space(1), Some(USER_SPACE));
        assert_eq!(spaces.tab_space(2), Some(task.id.as_str()));
        assert!(spaces.show(&task.id));
        spaces.remove(2);
        assert_eq!(spaces.active_space(), USER_SPACE);
        assert!(!spaces.contains(&task.id));
    }

    #[test]
    fn overview_and_empty_task_do_not_create_or_select_pages() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let mut task = tasks.create("Empty").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.set_overview(true);
        spaces.insert(1, USER_SPACE, true);
        assert!(spaces.overview());
        spaces.show(&task.id);
        assert_eq!(spaces.active_tab(), None);
        task.status = Status::Finished;
        spaces.update_tasks(&[task]);
        assert_eq!(spaces.active_space(), USER_SPACE);
        assert_eq!(spaces.views().len(), 1);
    }

    #[test]
    fn adopting_a_manual_tab_in_the_same_space_preserves_its_selection() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let task = tasks.create("Research").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.show(&task.id);
        spaces.insert(1, &task.id, true);
        spaces.insert(2, &task.id, true);
        spaces.adopt(2, &task.id);
        assert_eq!(spaces.active_tab(), Some(2));
        spaces.release(2);
        assert_eq!(spaces.active_tab(), Some(2));
    }

    #[test]
    fn borrowed_result_returns_to_finished_space_after_history_eviction() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let mut first = tasks.create("Finished result").unwrap();
        let second = tasks.create("Borrower").unwrap();
        spaces.update_tasks(&[first.clone(), second.clone()]);
        spaces.insert(1, &first.id, false);
        first.status = Status::Finished;
        spaces.update_tasks(&[first.clone(), second.clone()]);
        spaces.adopt(1, &second.id);
        spaces.update_tasks(&[second]);
        assert!(spaces.contains(&first.id));
        spaces.release(1);
        assert_eq!(spaces.tab_space(1), Some(first.id.as_str()));
        spaces.show(&first.id);
        assert_eq!(spaces.active_tab(), Some(1));
    }

    #[test]
    fn insertion_order_is_shared_by_views_and_duplicate_inserts_keep_position() {
        let mut spaces = Spaces::default();
        for tab in [30, 4, 20] {
            spaces.insert(tab, USER_SPACE, false);
        }
        spaces.insert(4, USER_SPACE, false);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[30, 4, 20]);
        assert_eq!(spaces.views()[0].tabs, vec![30, 4, 20]);
        assert_eq!(spaces.active_tab(), Some(30));
        assert!(spaces.ordered_tabs("missing").is_empty());

        let task = Registry::default().create("Destination").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.insert(8, &task.id, false);
        spaces.insert(30, &task.id, false);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[4, 20]);
        assert_eq!(spaces.ordered_tabs(&task.id), &[8, 30]);
        assert_eq!(spaces.active_tab(), Some(4));
        assert_eq!(spaces.tab_space(30), Some(task.id.as_str()));
    }

    #[test]
    fn reordering_changes_only_order_and_rejects_the_stale_direction() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let task = tasks.create("Ordered task").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.insert(70, USER_SPACE, true);
        for tab in [9, 2, 7] {
            spaces.insert(tab, &task.id, false);
        }
        spaces.show(&task.id);
        spaces.select_tab(2);
        assert!(spaces.reorder_adjacent(2, &task.id, 9, false));
        assert_eq!(spaces.ordered_tabs(&task.id), &[2, 9, 7]);
        // The same pair is still adjacent, but the old upward action is stale.
        assert!(!spaces.reorder_adjacent(2, &task.id, 9, false));
        assert!(!spaces.reorder_adjacent(2, &task.id, 7, false));
        assert!(spaces.reorder_adjacent(2, &task.id, 9, true));
        assert!(spaces.reorder_adjacent(2, &task.id, 7, true));
        assert_eq!(spaces.ordered_tabs(&task.id), &[9, 7, 2]);
        assert!(!spaces.reorder_adjacent(2, &task.id, 9, true));
        assert_eq!(spaces.active_space(), task.id);
        assert_eq!(spaces.active_tab(), Some(2));
        assert!(!spaces.overview());
        spaces.update_tasks(&tasks.list());
        let view = spaces
            .views()
            .into_iter()
            .find(|view| view.id == task.id)
            .unwrap();
        assert_eq!(view.revision, task.revision);
        assert_eq!(view.status, "active");
        assert_eq!(view.tabs, vec![9, 7, 2]);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[70]);
    }

    #[test]
    fn reorder_rejects_hidden_overview_unselected_removed_and_nonadjacent_targets() {
        let mut spaces = Spaces::default();
        let task = Registry::default().create("Background").unwrap();
        spaces.update_tasks(&[task.clone()]);
        for tab in [10, 20, 30] {
            spaces.insert(tab, USER_SPACE, false);
        }
        spaces.insert(40, &task.id, false);
        spaces.insert(50, &task.id, false);
        assert!(!spaces.reorder_adjacent(40, &task.id, 50, true));
        assert!(!spaces.reorder_adjacent(20, USER_SPACE, 30, true));
        assert!(!spaces.reorder_adjacent(10, &task.id, 20, true));
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 40, true));
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 30, true));
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 10, true));
        spaces.set_overview(true);
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 20, true));
        assert!(spaces.overview());
        spaces.set_overview(false);
        spaces.remove(20);
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 20, true));
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[10, 30]);
        assert_eq!(spaces.ordered_tabs(&task.id), &[40, 50]);
        assert_eq!(spaces.active_tab(), Some(10));
        spaces.remove(30);
        assert!(!spaces.reorder_adjacent(10, USER_SPACE, 10, false));
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[10]);
    }

    #[test]
    fn adoption_release_and_finished_cleanup_preserve_each_spaces_order() {
        let mut spaces = Spaces::default();
        let mut tasks = Registry::default();
        let mut task = tasks.create("Borrower").unwrap();
        spaces.update_tasks(&[task.clone()]);
        for tab in [30, 10, 20] {
            spaces.insert(tab, USER_SPACE, false);
        }
        spaces.insert(8, &task.id, false);
        spaces.insert(9, &task.id, false);
        spaces.select_tab(20);
        assert!(spaces.reorder_adjacent(20, USER_SPACE, 10, false));
        spaces.adopt(20, &task.id);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[30, 10]);
        assert_eq!(spaces.ordered_tabs(&task.id), &[8, 9, 20]);
        assert_eq!(spaces.active_tab(), Some(30));
        spaces.finish(
            &[
                (20, Origin::Borrowed),
                (8, Origin::Created),
                (9, Origin::Created),
            ],
            &[8, 9],
        );
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[30, 10, 20]);
        assert_eq!(spaces.ordered_tabs(&task.id), &[8, 9]);
        assert_eq!(spaces.active_tab(), Some(30));
        task.status = Status::Finished;
        spaces.update_tasks(&[task.clone()]);
        spaces.show(&task.id);
        spaces.remove(8);
        assert_eq!(spaces.ordered_tabs(&task.id), &[9]);
        assert_eq!(spaces.active_tab(), Some(9));
        spaces.remove(9);
        assert!(!spaces.contains(&task.id));
        assert!(spaces.ordered_tabs(&task.id).is_empty());
        assert_eq!(spaces.active_space(), USER_SPACE);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[30, 10, 20]);
    }

    #[test]
    fn drag_order_moves_background_tabs_to_any_boundary_without_selecting() {
        let mut spaces = Spaces::default();
        for id in [10, 20, 30, 40] {
            spaces.insert(id, USER_SPACE, false);
        }
        let capacity = spaces.records[USER_SPACE].tabs.capacity();
        assert_eq!(
            spaces.reorder_before(40, USER_SPACE, Some(20), &[10, 20, 30, 40]),
            Some(true)
        );
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[10, 40, 20, 30]);
        assert_eq!(spaces.active_tab(), Some(10));
        assert_eq!(
            spaces.reorder_before(40, USER_SPACE, None, &[10, 40, 20, 30]),
            Some(true)
        );
        assert_eq!(
            spaces.reorder_before(40, USER_SPACE, None, &[10, 20, 30, 40]),
            Some(false)
        );
        assert_eq!(
            spaces.reorder_before(20, USER_SPACE, Some(30), &[10, 20, 30, 40]),
            Some(false)
        );
        assert_eq!(
            spaces.reorder_before(30, USER_SPACE, Some(10), &[10, 20, 30, 40]),
            Some(true)
        );
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[30, 10, 20, 40]);
        assert_eq!(spaces.active_tab(), Some(10));
        assert_eq!(spaces.records[USER_SPACE].tabs.capacity(), capacity);
    }

    #[test]
    fn drag_order_rejects_stale_or_foreign_targets_without_partial_reordering() {
        let mut spaces = Spaces::default();
        let task = Registry::default().create("Background work").unwrap();
        spaces.update_tasks(&[task.clone()]);
        for id in [1, 2, 3] {
            spaces.insert(id, USER_SPACE, false);
        }
        spaces.insert(4, &task.id, false);
        for (tab, before, order) in [
            (2, Some(4), vec![1, 2, 3]),
            (4, Some(1), vec![1, 2, 3]),
            (2, Some(2), vec![1, 2, 3]),
            (2, Some(1), vec![3, 2, 1]),
            (2, Some(1), vec![1, 2]),
        ] {
            assert_eq!(spaces.reorder_before(tab, USER_SPACE, before, &order), None);
            assert_eq!(spaces.ordered_tabs(USER_SPACE), &[1, 2, 3]);
        }
        spaces.set_overview(true);
        assert_eq!(
            spaces.reorder_before(3, USER_SPACE, Some(1), &[1, 2, 3]),
            None
        );
        spaces.show(&task.id);
        assert_eq!(
            spaces.reorder_before(3, USER_SPACE, Some(1), &[1, 2, 3]),
            None
        );
        spaces.show(USER_SPACE);
        spaces.remove(2);
        assert_eq!(
            spaces.reorder_before(3, USER_SPACE, Some(1), &[1, 2, 3]),
            None
        );
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[1, 3]);
        assert_eq!(spaces.active_tab(), Some(1));
        assert_eq!(spaces.ordered_tabs(&task.id), &[4]);
    }

    #[test]
    fn closing_many_tabs_reclaims_order_capacity_without_changing_the_remaining_order() {
        let mut spaces = Spaces::default();
        let task = Registry::default().create("Preserved space").unwrap();
        spaces.update_tasks(&[task.clone()]);
        spaces.insert(1000, &task.id, false);
        for tab in 1..=128 {
            spaces.insert(tab, USER_SPACE, false);
        }
        assert!(spaces.records[USER_SPACE].tabs.capacity() >= 128);
        for tab in 1..=112 {
            spaces.remove(tab);
        }
        assert_eq!(
            spaces.ordered_tabs(USER_SPACE),
            &(113..=128).collect::<Vec<_>>()
        );
        assert!(spaces.records[USER_SPACE].tabs.capacity() <= 32);
        assert_eq!(spaces.active_tab(), Some(113));
        assert_eq!(spaces.active_space(), USER_SPACE);
        assert_eq!(spaces.ordered_tabs(&task.id), &[1000]);
        for tab in 113..=128 {
            spaces.remove(tab);
        }
        assert_eq!(spaces.records[USER_SPACE].tabs.capacity(), 0);
        assert_eq!(spaces.active_tab(), None);
        assert_eq!(spaces.ordered_tabs(&task.id), &[1000]);
        spaces.insert(200, USER_SPACE, false);
        assert_eq!(spaces.ordered_tabs(USER_SPACE), &[200]);
        assert_eq!(spaces.active_tab(), Some(200));
    }
}
