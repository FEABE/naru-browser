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
        self.tabs.insert(tab, space.into());
        let entry = self.records.get_mut(space).expect("space exists");
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
        let replacement = self
            .tabs
            .iter()
            .find_map(|(&tab, id)| (id == space).then_some(tab));
        if let Some(record) = self.records.get_mut(space) {
            if record.selected == Some(removed) {
                record.selected = replacement;
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
                tabs: self
                    .tabs
                    .iter()
                    .filter_map(|(&tab, space)| (space == id).then_some(tab))
                    .collect(),
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
}
