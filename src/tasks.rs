//! In-memory coordination for tasks sharing one user's browser.
//!
//! This registry does not create or close browser tabs and is not a security
//! boundary between processes running as the same operating-system user.

use crate::protocol::TaskScope;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::Read;

pub const MAX_TASKS: usize = 128;
/// Most recent finished tasks kept for `task info`/`task list`. Older finished
/// tasks are pruned when a newer task finishes; active, paused and finishing
/// tasks are never pruned.
pub const MAX_FINISHED_TASKS: usize = 16;
pub const MAX_TASK_TABS: usize = 4;
const MAX_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskError {
    pub code: &'static str,
    pub message: &'static str,
}

impl TaskError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TaskError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Created,
    Borrowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Paused,
    Finishing,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskTab {
    pub tab: i32,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub revision: u64,
    pub tabs: Vec<TaskTab>,
    pub closing_tabs: Vec<i32>,
    /// Tabs preserved by the latest release or initial finish operation.
    pub released_tabs: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishPlan {
    pub task: TaskView,
    /// Request native close for these tabs; retain ownership until tab_closed.
    pub close: Vec<i32>,
}

struct TaskRecord {
    id: String,
    name: String,
    status: Status,
    revision: u64,
    tabs: BTreeMap<i32, Origin>,
    released_tabs: Vec<i32>,
    /// Order in which the task reached `Finished`; `None` until then.
    finished_seq: Option<u64>,
}

impl TaskRecord {
    fn view(&self) -> TaskView {
        TaskView {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status,
            revision: self.revision,
            tabs: self
                .tabs
                .iter()
                .map(|(&tab, &origin)| TaskTab { tab, origin })
                .collect(),
            closing_tabs: if self.status == Status::Finishing {
                self.tabs.keys().copied().collect()
            } else {
                Vec::new()
            },
            released_tabs: self.released_tabs.clone(),
        }
    }

    fn next_revision(&self) -> Result<u64, TaskError> {
        self.revision.checked_add(1).ok_or(TaskError::new(
            "TASK_REVISION_LIMIT",
            "This task cannot issue another control revision.",
        ))
    }
}

#[derive(Default)]
pub struct Registry {
    // At most 128 records with 4 tabs each bounds ownership lookup; at most
    // MAX_FINISHED_TASKS of them are finished.
    records: Vec<TaskRecord>,
    // Finish order, not creation order, decides which finished record is oldest:
    // a long-running early task that just finished must stay inspectable.
    finish_counter: u64,
}

impl Registry {
    pub fn create(&mut self, name: &str) -> Result<TaskView, TaskError> {
        if name.trim().is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(TaskError::new(
                "INVALID_TASK_NAME",
                "Use a nonblank task name of at most 120 characters without control characters.",
            ));
        }
        let prune = if self.records.len() >= MAX_TASKS {
            Some(self.oldest_finished().ok_or(TaskError::new(
                "TASK_LIMIT",
                "Finish an existing task before creating another task.",
            ))?)
        } else {
            None
        };

        // Generate before eviction so failure leaves the registry unchanged.
        let id = self.new_id()?;
        if let Some(index) = prune {
            self.records.remove(index);
        }
        let task = TaskRecord {
            id,
            name: name.to_owned(),
            status: Status::Active,
            revision: 1,
            tabs: BTreeMap::new(),
            released_tabs: Vec::new(),
            finished_seq: None,
        };
        let view = task.view();
        self.records.push(task);
        Ok(view)
    }

    /// Resume-by-name: return the live task that already uses `name` instead of
    /// creating a duplicate. The boolean is true when an existing task was
    /// returned. A paused task is returned as paused with its current revision;
    /// this never changes ownership or revision, so the agent must use the
    /// explicit `task resume` and cannot silently take control back from the
    /// user. Finishing and finished tasks are never resumed (a new task is made).
    pub fn create_or_resume(&mut self, name: &str) -> Result<(TaskView, bool), TaskError> {
        if let Some(task) = self.records.iter().rev().find(|task| {
            task.name == name && matches!(task.status, Status::Active | Status::Paused)
        }) {
            return Ok((task.view(), true));
        }
        Ok((self.create(name)?, false))
    }

    pub fn list(&self) -> Vec<TaskView> {
        self.records.iter().map(TaskRecord::view).collect()
    }

    pub fn info(&self, id: &str) -> Result<TaskView, TaskError> {
        Ok(self.records[self.index(id)?].view())
    }

    pub fn check(&self, scope: &TaskScope) -> Result<(), TaskError> {
        let task = &self.records[self.checked_index(scope)?];
        match task.status {
            Status::Active => Ok(()),
            Status::Paused => Err(TaskError::new(
                "TASK_PAUSED",
                "Resume this task before issuing browser commands.",
            )),
            Status::Finishing => Err(TaskError::new(
                "TASK_FINISHING",
                "This task is waiting for its created tabs to close.",
            )),
            Status::Finished => Err(TaskError::new("TASK_FINISHED", "This task has finished.")),
        }
    }

    pub fn check_capacity(&self, scope: &TaskScope) -> Result<(), TaskError> {
        self.check(scope)?;
        if self.records[self.index(&scope.task)?].tabs.len() >= MAX_TASK_TABS {
            return Err(TaskError::new(
                "TASK_TAB_LIMIT",
                "Release or close a task tab before adding another tab.",
            ));
        }
        Ok(())
    }

    pub fn owner(&self, tab: i32) -> Option<(String, Origin)> {
        self.records
            .iter()
            .find_map(|task| task.tabs.get(&tab).map(|&origin| (task.id.clone(), origin)))
    }

    pub fn check_tab(&self, scope: Option<&TaskScope>, tab: i32) -> Result<(), TaskError> {
        match scope {
            None if self.owner(tab).is_some() => Err(TaskError::new(
                "TASK_REQUIRED",
                "Use the owning task scope to access this tab.",
            )),
            None => Ok(()),
            Some(scope) => {
                self.check(scope)?;
                if self
                    .owner(tab)
                    .is_some_and(|(owner, _)| owner == scope.task)
                {
                    Ok(())
                } else {
                    Err(TaskError::new(
                        "TASK_TAB_MISMATCH",
                        "This tab does not belong to the supplied task.",
                    ))
                }
            }
        }
    }

    pub fn attach(
        &mut self,
        scope: &TaskScope,
        tab: i32,
        origin: Origin,
    ) -> Result<TaskView, TaskError> {
        self.check_capacity(scope)?;
        if self.owner(tab).is_some() {
            return Err(TaskError::new(
                "TAB_ALREADY_ATTACHED",
                "Release this tab from its current task before attaching it.",
            ));
        }
        let index = self.index(&scope.task)?;
        self.records[index].tabs.insert(tab, origin);
        Ok(self.records[index].view())
    }

    pub fn release(&mut self, scope: &TaskScope, tab: i32) -> Result<TaskView, TaskError> {
        self.check_tab(Some(scope), tab)?;
        let index = self.index(&scope.task)?;
        let task = &mut self.records[index];
        task.tabs.remove(&tab);
        task.released_tabs = vec![tab];
        Ok(task.view())
    }

    pub fn handoff(&mut self, scope: &TaskScope) -> Result<TaskView, TaskError> {
        self.check(scope)?;
        let index = self.index(&scope.task)?;
        let task = &mut self.records[index];
        let revision = task.next_revision()?;
        task.status = Status::Paused;
        task.revision = revision;
        Ok(task.view())
    }

    pub fn resume(&mut self, scope: &TaskScope) -> Result<TaskView, TaskError> {
        let index = self.checked_index(scope)?;
        if self.records[index].status != Status::Paused {
            self.check(scope)?;
            return Err(TaskError::new(
                "TASK_NOT_PAUSED",
                "Only a paused task can be resumed.",
            ));
        }
        let task = &mut self.records[index];
        let revision = task.next_revision()?;
        task.status = Status::Active;
        task.revision = revision;
        Ok(task.view())
    }

    pub fn finish(&mut self, scope: &TaskScope, keep: &[i32]) -> Result<FinishPlan, TaskError> {
        let index = self.checked_index(scope)?;
        let task = &mut self.records[index];
        if matches!(task.status, Status::Finishing | Status::Finished) {
            if !keep.is_empty() {
                return Err(TaskError::new(
                    "INVALID_KEEP",
                    "The finish plan has started; retry with an empty keep list.",
                ));
            }
            let view = task.view();
            return Ok(FinishPlan {
                close: view.closing_tabs.clone(),
                task: view,
            });
        }

        let mut preserved = BTreeSet::new();
        for &tab in keep {
            if !task.tabs.contains_key(&tab) || !preserved.insert(tab) {
                return Err(TaskError::new(
                    "INVALID_KEEP",
                    "Keep must contain unique tab IDs belonging to this task.",
                ));
            }
        }
        let revision = task.next_revision()?;
        let released = task
            .tabs
            .iter()
            .filter_map(|(&tab, &origin)| {
                (origin == Origin::Borrowed || preserved.contains(&tab)).then_some(tab)
            })
            .collect();

        // Native close happens later. Until its callback, created tabs remain
        // owned by this finishing task and cannot be reclaimed by another one.
        task.tabs
            .retain(|tab, origin| *origin == Origin::Created && !preserved.contains(tab));
        task.status = if task.tabs.is_empty() {
            Status::Finished
        } else {
            Status::Finishing
        };
        task.revision = revision;
        task.released_tabs = released;
        let view = task.view();
        if view.status == Status::Finished {
            self.mark_finished(index);
        }
        Ok(FinishPlan {
            close: view.closing_tabs.clone(),
            task: view,
        })
    }

    pub fn tab_closed(&mut self, tab: i32) -> Option<TaskView> {
        let index = self
            .records
            .iter()
            .position(|task| task.tabs.contains_key(&tab))?;
        let task = &mut self.records[index];
        task.tabs.remove(&tab);
        let finished = task.status == Status::Finishing && task.tabs.is_empty();
        if finished {
            task.status = Status::Finished;
        }
        let view = task.view();
        if finished {
            self.mark_finished(index);
        }
        Some(view)
    }

    /// Stamp a task that just reached `Finished` and keep only the
    /// `MAX_FINISHED_TASKS` most recently finished records. The task that just
    /// finished has the newest stamp, so it is never the one pruned.
    fn mark_finished(&mut self, index: usize) {
        self.finish_counter += 1;
        self.records[index].finished_seq = Some(self.finish_counter);
        while self
            .records
            .iter()
            .filter(|task| task.status == Status::Finished)
            .count()
            > MAX_FINISHED_TASKS
        {
            let Some(oldest) = self.oldest_finished() else {
                break;
            };
            self.records.remove(oldest);
        }
    }

    fn oldest_finished(&self) -> Option<usize> {
        self.records
            .iter()
            .enumerate()
            .filter(|(_, task)| task.status == Status::Finished)
            .min_by_key(|(_, task)| task.finished_seq.unwrap_or(0))
            .map(|(index, _)| index)
    }

    fn index(&self, id: &str) -> Result<usize, TaskError> {
        self.records
            .iter()
            .position(|task| task.id == id)
            .ok_or(TaskError::new("TASK_NOT_FOUND", "No task has that ID."))
    }

    fn checked_index(&self, scope: &TaskScope) -> Result<usize, TaskError> {
        let index = self.index(&scope.task)?;
        if self.records[index].revision != scope.revision {
            return Err(TaskError::new(
                "TASK_STALE_REVISION",
                "The task control revision changed; inspect the current task state.",
            ));
        }
        Ok(index)
    }

    fn new_id(&self) -> Result<String, TaskError> {
        let unavailable = TaskError::new(
            "TASK_ID_UNAVAILABLE",
            "Could not obtain a unique task ID from the operating system.",
        );
        let mut random = File::open("/dev/urandom").map_err(|_| unavailable)?;
        for _ in 0..8 {
            let mut bytes = [0u8; 16];
            random.read_exact(&mut bytes).map_err(|_| unavailable)?;
            let mut id = String::with_capacity(37);
            id.push_str("task-");
            for byte in bytes {
                use fmt::Write;
                write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
            }
            if self.records.iter().all(|task| task.id != id) {
                return Ok(id);
            }
        }
        Err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(task: &TaskView) -> TaskScope {
        TaskScope {
            task: task.id.clone(),
            revision: task.revision,
        }
    }

    #[test]
    fn names_use_unicode_character_limits_and_ids_use_random_hex() {
        let mut registry = Registry::default();
        for name in ["", "   ", "task\nname", "task\0name", &"가".repeat(121)] {
            assert_eq!(registry.create(name).unwrap_err().code, "INVALID_TASK_NAME");
        }
        let name = "가".repeat(120);
        let first = registry.create(&name).unwrap();
        let second = registry.create("second").unwrap();
        assert_eq!(first.name, name);
        assert_eq!(first.id.len(), 37);
        assert!(first.id.starts_with("task-"));
        assert!(first.id[5..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first.id, second.id);
        assert_eq!(first.status, Status::Active);
        assert_eq!(first.revision, 1);
        assert_eq!(registry.list(), vec![first, second]);
    }

    #[test]
    fn create_or_resume_returns_live_task_with_same_name() {
        let mut registry = Registry::default();
        let (first, resumed) = registry.create_or_resume("research").unwrap();
        assert!(!resumed);
        let (again, resumed) = registry.create_or_resume("research").unwrap();
        assert!(resumed);
        assert_eq!(again, first);
        assert_eq!(registry.list().len(), 1);
        let (other, resumed) = registry.create_or_resume("other").unwrap();
        assert!(!resumed);
        assert_ne!(other.id, first.id);
        assert_eq!(
            registry.create_or_resume("  ").unwrap_err().code,
            "INVALID_TASK_NAME"
        );
    }

    #[test]
    fn create_or_resume_never_steals_control_from_a_paused_task() {
        let mut registry = Registry::default();
        let (first, _) = registry.create_or_resume("handoff").unwrap();
        let paused = registry.handoff(&scope(&first)).unwrap();
        let (again, resumed) = registry.create_or_resume("handoff").unwrap();
        assert!(resumed);
        assert_eq!(again, paused);
        assert_eq!(again.status, Status::Paused);
        assert_eq!(again.revision, paused.revision);
        assert_eq!(
            registry.check(&scope(&again)).unwrap_err().code,
            "TASK_PAUSED"
        );
    }

    #[test]
    fn create_or_resume_skips_finishing_and_finished_tasks() {
        let mut registry = Registry::default();
        let (done, _) = registry.create_or_resume("job").unwrap();
        registry.finish(&scope(&done), &[]).unwrap();
        let (fresh, resumed) = registry.create_or_resume("job").unwrap();
        assert!(!resumed);
        assert_ne!(fresh.id, done.id);
        registry.attach(&scope(&fresh), 5, Origin::Created).unwrap();
        let finishing = registry.finish(&scope(&fresh), &[]).unwrap().task;
        assert_eq!(finishing.status, Status::Finishing);
        let (third, resumed) = registry.create_or_resume("job").unwrap();
        assert!(!resumed);
        assert_ne!(third.id, finishing.id);
        let (again, resumed) = registry.create_or_resume("job").unwrap();
        assert!(resumed);
        assert_eq!(again.id, third.id);
    }

    #[test]
    fn scopes_cannot_access_foreign_or_unowned_tabs() {
        let mut registry = Registry::default();
        let first = scope(&registry.create("first").unwrap());
        let second = scope(&registry.create("second").unwrap());
        registry.attach(&first, 10, Origin::Created).unwrap();
        registry.attach(&second, 20, Origin::Borrowed).unwrap();
        assert!(registry.check_tab(Some(&first), 10).is_ok());
        assert_eq!(
            registry.check_tab(None, 10).unwrap_err().code,
            "TASK_REQUIRED"
        );
        assert_eq!(
            registry.check_tab(Some(&second), 10).unwrap_err().code,
            "TASK_TAB_MISMATCH"
        );
        assert_eq!(
            registry.check_tab(Some(&first), 30).unwrap_err().code,
            "TASK_TAB_MISMATCH"
        );
        assert!(registry.check_tab(None, 30).is_ok());
        assert_eq!(
            registry
                .attach(&second, 10, Origin::Borrowed)
                .unwrap_err()
                .code,
            "TAB_ALREADY_ATTACHED"
        );
        assert_eq!(registry.owner(10), Some((first.task, Origin::Created)));
    }

    #[test]
    fn handoff_and_resume_invalidate_every_older_revision() {
        let mut registry = Registry::default();
        let initial = scope(&registry.create("control").unwrap());
        registry.attach(&initial, 7, Origin::Borrowed).unwrap();
        let paused = registry.handoff(&initial).unwrap();
        assert_eq!(paused.status, Status::Paused);
        assert_eq!(paused.revision, initial.revision + 1);
        assert_eq!(
            registry.check(&initial).unwrap_err().code,
            "TASK_STALE_REVISION"
        );
        let paused_scope = scope(&paused);
        assert_eq!(
            registry.check(&paused_scope).unwrap_err().code,
            "TASK_PAUSED"
        );
        assert_eq!(
            registry.release(&paused_scope, 7).unwrap_err().code,
            "TASK_PAUSED"
        );
        let resumed = registry.resume(&paused_scope).unwrap();
        assert_eq!(resumed.status, Status::Active);
        assert_eq!(resumed.revision, paused.revision + 1);
        assert_eq!(
            registry.check(&paused_scope).unwrap_err().code,
            "TASK_STALE_REVISION"
        );
        assert!(registry.check_tab(Some(&scope(&resumed)), 7).is_ok());
        assert_eq!(
            registry.resume(&scope(&resumed)).unwrap_err().code,
            "TASK_NOT_PAUSED"
        );
    }

    #[test]
    fn invalid_keep_does_not_partially_finish_or_release_any_tabs() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("atomic finish").unwrap());
        registry.attach(&task_scope, 1, Origin::Created).unwrap();
        registry.attach(&task_scope, 2, Origin::Borrowed).unwrap();
        let before = registry.info(&task_scope.task).unwrap();
        for keep in [&[1, 99][..], &[1, 1][..]] {
            assert_eq!(
                registry.finish(&task_scope, keep).unwrap_err().code,
                "INVALID_KEEP"
            );
            assert_eq!(registry.info(&task_scope.task).unwrap(), before);
            assert_eq!(
                registry.owner(2),
                Some((task_scope.task.clone(), Origin::Borrowed))
            );
        }
    }

    #[test]
    fn finish_preserves_borrowed_and_kept_tabs_until_created_tabs_close() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("finish").unwrap());
        for (tab, origin) in [
            (1, Origin::Created),
            (2, Origin::Created),
            (3, Origin::Borrowed),
            (4, Origin::Borrowed),
        ] {
            registry.attach(&task_scope, tab, origin).unwrap();
        }
        let plan = registry.finish(&task_scope, &[2, 3]).unwrap();
        assert_eq!(plan.task.status, Status::Finishing);
        assert_eq!(plan.close, vec![1]);
        assert_eq!(plan.task.closing_tabs, vec![1]);
        assert_eq!(plan.task.released_tabs, vec![2, 3, 4]);
        assert_eq!(
            registry.owner(1),
            Some((task_scope.task.clone(), Origin::Created))
        );
        for tab in [2, 3, 4] {
            assert!(registry.owner(tab).is_none());
            assert!(registry.check_tab(None, tab).is_ok());
        }
        assert_eq!(
            registry.check_tab(None, 1).unwrap_err().code,
            "TASK_REQUIRED"
        );
        assert_eq!(
            registry.check(&scope(&plan.task)).unwrap_err().code,
            "TASK_FINISHING"
        );
        let completed = registry.tab_closed(1).unwrap();
        assert_eq!(completed.status, Status::Finished);
        assert_eq!(completed.revision, plan.task.revision);
        assert!(completed.tabs.is_empty() && completed.closing_tabs.is_empty());
        assert_eq!(completed.released_tabs, vec![2, 3, 4]);
    }

    #[test]
    fn finishing_retries_only_pending_closes_and_does_not_accept_new_keep() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("retry finish").unwrap());
        registry.attach(&task_scope, 1, Origin::Created).unwrap();
        registry.attach(&task_scope, 2, Origin::Created).unwrap();
        let plan = registry.finish(&task_scope, &[]).unwrap();
        let finishing = scope(&plan.task);
        assert_eq!(
            registry.finish(&task_scope, &[]).unwrap_err().code,
            "TASK_STALE_REVISION"
        );
        assert_eq!(
            registry.finish(&finishing, &[1]).unwrap_err().code,
            "INVALID_KEEP"
        );
        assert_eq!(registry.finish(&finishing, &[]).unwrap(), plan);
        registry.tab_closed(1).unwrap();
        assert_eq!(registry.finish(&finishing, &[]).unwrap().close, vec![2]);
        registry.tab_closed(2).unwrap();
        let completed = registry.finish(&finishing, &[]).unwrap();
        assert_eq!(completed.task.status, Status::Finished);
        assert!(completed.close.is_empty());
        assert_eq!(
            registry.finish(&finishing, &[2]).unwrap_err().code,
            "INVALID_KEEP"
        );
        assert_eq!(
            registry.check(&finishing).unwrap_err().code,
            "TASK_FINISHED"
        );
    }

    #[test]
    fn paused_and_empty_tasks_can_finish_without_closing_borrowed_tabs() {
        let mut registry = Registry::default();
        let initial = scope(&registry.create("paused finish").unwrap());
        registry.attach(&initial, 1, Origin::Borrowed).unwrap();
        let paused = registry.handoff(&initial).unwrap();
        let plan = registry.finish(&scope(&paused), &[]).unwrap();
        assert_eq!(plan.task.status, Status::Finished);
        assert!(plan.close.is_empty());
        assert_eq!(plan.task.released_tabs, vec![1]);
        assert!(registry.owner(1).is_none());
        let empty = scope(&registry.create("empty finish").unwrap());
        assert_eq!(
            registry.finish(&empty, &[]).unwrap().task.status,
            Status::Finished
        );
    }

    #[test]
    fn release_and_manual_close_do_not_finish_active_empty_tasks() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("reusable task").unwrap());
        registry.attach(&task_scope, 1, Origin::Created).unwrap();
        let released = registry.release(&task_scope, 1).unwrap();
        assert_eq!(released.status, Status::Active);
        assert_eq!(released.released_tabs, vec![1]);
        assert!(released.tabs.is_empty());
        assert!(registry.owner(1).is_none());
        registry.attach(&task_scope, 2, Origin::Borrowed).unwrap();
        let closed = registry.tab_closed(2).unwrap();
        assert_eq!(closed.status, Status::Active);
        assert!(closed.tabs.is_empty());
        assert!(registry.tab_closed(2).is_none());
        registry.attach(&task_scope, 3, Origin::Created).unwrap();
        assert_eq!(
            registry.release(&task_scope, 3).unwrap().released_tabs,
            vec![3]
        );
        assert_eq!(
            registry.release(&task_scope, 3).unwrap_err().code,
            "TASK_TAB_MISMATCH"
        );
    }

    #[test]
    fn tab_capacity_is_recovered_by_release_and_manual_close() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("capacity").unwrap());
        for tab in 1..=MAX_TASK_TABS as i32 {
            registry.attach(&task_scope, tab, Origin::Created).unwrap();
        }
        assert_eq!(
            registry.check_capacity(&task_scope).unwrap_err().code,
            "TASK_TAB_LIMIT"
        );
        assert_eq!(
            registry
                .attach(&task_scope, 33, Origin::Created)
                .unwrap_err()
                .code,
            "TASK_TAB_LIMIT"
        );
        registry.release(&task_scope, 1).unwrap();
        registry.attach(&task_scope, 33, Origin::Created).unwrap();
        registry.tab_closed(2).unwrap();
        assert!(registry.check_capacity(&task_scope).is_ok());
    }

    #[test]
    fn registry_prunes_only_the_oldest_finished_record_when_full() {
        let mut registry = Registry::default();
        let tasks = (0..MAX_TASKS)
            .map(|index| registry.create(&format!("task {index}")).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(registry.create("overflow").unwrap_err().code, "TASK_LIMIT");
        assert_eq!(registry.list(), tasks);
        registry.finish(&scope(&tasks[0]), &[]).unwrap();
        registry.finish(&scope(&tasks[2]), &[]).unwrap();
        registry.create("replacement").unwrap();
        assert_eq!(registry.list().len(), MAX_TASKS);
        assert_eq!(
            registry.info(&tasks[0].id).unwrap_err().code,
            "TASK_NOT_FOUND"
        );
        assert_eq!(registry.info(&tasks[1].id).unwrap().status, Status::Active);
        assert_eq!(
            registry.info(&tasks[2].id).unwrap().status,
            Status::Finished
        );
    }

    #[test]
    fn finished_tasks_are_pruned_by_finish_order_beyond_the_retention_limit() {
        let mut registry = Registry::default();
        let tasks = (0..MAX_FINISHED_TASKS + 4)
            .map(|index| registry.create(&format!("task {index}")).unwrap())
            .collect::<Vec<_>>();
        // Finish in reverse creation order: the first-created task finishes last
        // and must stay inspectable even though it is the oldest by creation.
        for task in tasks.iter().rev().take(MAX_FINISHED_TASKS) {
            let plan = registry.finish(&scope(task), &[]).unwrap();
            assert_eq!(plan.task.status, Status::Finished);
            assert_eq!(registry.info(&task.id).unwrap().status, Status::Finished);
        }
        assert_eq!(registry.list().len(), tasks.len());
        let just_finished = &tasks[0];
        registry.finish(&scope(just_finished), &[]).unwrap();
        assert_eq!(
            registry.info(&just_finished.id).unwrap().status,
            Status::Finished,
            "a just-finished task is retained for task info"
        );
        // The earliest finisher (last created) was pruned; the rest remain.
        let oldest_finisher = tasks.last().unwrap();
        assert_eq!(
            registry.info(&oldest_finisher.id).unwrap_err().code,
            "TASK_NOT_FOUND"
        );
        let finished = registry
            .list()
            .into_iter()
            .filter(|task| task.status == Status::Finished)
            .count();
        assert_eq!(finished, MAX_FINISHED_TASKS);
        // The three never-finished tasks that were not finished are untouched.
        for task in &tasks[1..4] {
            assert_eq!(registry.info(&task.id).unwrap().status, Status::Active);
        }
    }

    #[test]
    fn pruning_never_removes_active_paused_or_finishing_tasks() {
        let mut registry = Registry::default();
        let active = registry.create("active").unwrap();
        let paused = registry.create("paused").unwrap();
        registry.handoff(&scope(&paused)).unwrap();
        let paused = registry.info(&paused.id).unwrap();
        let finishing = registry.create("finishing").unwrap();
        registry
            .attach(&scope(&finishing), 99, Origin::Created)
            .unwrap();
        let finishing_view = registry.finish(&scope(&finishing), &[]).unwrap().task;
        assert_eq!(finishing_view.status, Status::Finishing);
        for index in 0..MAX_FINISHED_TASKS * 2 {
            let task = registry.create(&format!("done {index}")).unwrap();
            registry.finish(&scope(&task), &[]).unwrap();
        }
        assert_eq!(registry.info(&active.id).unwrap().status, Status::Active);
        assert_eq!(registry.info(&paused.id).unwrap().status, Status::Paused);
        assert_eq!(
            registry.info(&finishing_view.id).unwrap().status,
            Status::Finishing
        );
        assert_eq!(registry.list().len(), 3 + MAX_FINISHED_TASKS);
        // A finishing task counts as finished only after its tab closes, and
        // then it is the newest finisher, so it survives the next prune.
        let closed = registry.tab_closed(99).unwrap();
        assert_eq!(closed.status, Status::Finished);
        // The 17th finished record (this one) displaces the oldest finisher.
        assert_eq!(registry.list().len(), 2 + MAX_FINISHED_TASKS);
        assert_eq!(
            registry.info(&finishing_view.id).unwrap().status,
            Status::Finished
        );
    }

    #[test]
    fn retention_does_not_change_resume_by_name_or_finished_checks() {
        let mut registry = Registry::default();
        let (live, _) = registry.create_or_resume("keeper").unwrap();
        let mut last = None;
        for index in 0..MAX_FINISHED_TASKS + 5 {
            let task = registry.create(&format!("done {index}")).unwrap();
            registry.finish(&scope(&task), &[]).unwrap();
            last = Some(task);
        }
        let (again, resumed) = registry.create_or_resume("keeper").unwrap();
        assert!(resumed);
        assert_eq!(again, live);
        // A finished (retained) task still reports TASK_FINISHED, and its name
        // is never resumed.
        let last = last.unwrap();
        let finished = registry.info(&last.id).unwrap();
        assert_eq!(finished.status, Status::Finished);
        assert_eq!(
            registry.check(&scope(&finished)).unwrap_err().code,
            "TASK_FINISHED"
        );
        let (fresh, resumed) = registry.create_or_resume(&last.name).unwrap();
        assert!(!resumed);
        assert_ne!(fresh.id, last.id);
    }

    #[test]
    fn task_views_serialize_using_the_public_contract() {
        let mut registry = Registry::default();
        let task_scope = scope(&registry.create("serialization").unwrap());
        registry.attach(&task_scope, 1, Origin::Borrowed).unwrap();
        registry.attach(&task_scope, 2, Origin::Created).unwrap();
        let plan = registry.finish(&task_scope, &[]).unwrap();
        let value = serde_json::to_value(plan.task).unwrap();
        assert_eq!(value["status"], "finishing");
        assert_eq!(value["tabs"][0]["origin"], "created");
        assert_eq!(value["closingTabs"], serde_json::json!([2]));
        assert_eq!(value["releasedTabs"], serde_json::json!([1]));
        assert!(value.get("closing_tabs").is_none());
    }
}
