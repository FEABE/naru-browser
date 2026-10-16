//! Bounded download metadata. Native callbacks and file-system operations belong
//! to the runtime; this model never retains a browser or chooses a path itself.

use crate::protocol::TaskScope;
use std::{collections::VecDeque, path::Path};

pub const MAX_ACTIVE: usize = 4;
pub const MAX_RECORDS: usize = 24;
pub const MAX_PATH_BYTES: usize = 8192;
const MAX_NAME_CHARS: usize = 256;
const MAX_SOURCE_CHARS: usize = 80;
const MAX_REASON_CHARS: usize = 160;

/// Identity of the document when a file was accepted by Naru. Metadata only;
/// revealing it neither replays the URL nor takes control of its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTarget {
    pub(crate) tab: i32,
    pub(crate) generation: u64,
    pub(crate) document_epoch: u64,
    pub(crate) owner: Option<(TaskScope, crate::tasks::Status)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Choosing,
    Downloading,
    Canceling,
    Canceled,
    Complete,
    Interrupted,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Canceled | Self::Complete | Self::Interrupted)
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u32,
    pub tab: i32,
    pub scope: Option<TaskScope>,
    pub status: Status,
    pub name: String,
    source: String,
    pub(crate) source_target: Option<SourceTarget>,
    path: Option<String>,
    // Set only by a fresh Complete update matching the user's approved path.
    // Terminal records are immutable, so no second path string is retained.
    file_verified: bool,
    received: i64,
    total: i64,
    percent: Option<i32>,
    reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub path: Option<String>,
    pub status: Status,
    pub received: i64,
    pub total: i64,
    pub percent: i32,
    pub reason: Option<String>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            path: None,
            status: Status::Downloading,
            received: 0,
            total: 0,
            percent: -1,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRow {
    pub id: u32,
    pub name: String,
    pub source: String,
    pub detail: String,
    pub can_choose: bool,
    pub can_cancel: bool,
    pub can_reveal: bool,
    pub can_reveal_file: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub rows: Vec<DownloadRow>,
    pub active_count: usize,
    pub notice: Option<String>,
}

#[derive(Debug, Default)]
pub struct Model {
    entries: VecDeque<Entry>,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_accept(&self) -> bool {
        self.active_count() < MAX_ACTIVE
    }

    pub fn activity(&self) -> (usize, usize) {
        self.entries
            .iter()
            .fold((0, 0), |(active, pending), entry| {
                (
                    active + usize::from(!entry.status.is_terminal()),
                    pending + usize::from(entry.status == Status::Pending),
                )
            })
    }

    /// Only OnBeforeDownload seeds a record. `source` is a human-facing space
    /// name supplied by the runtime, never a download or referring URL.
    pub fn insert(
        &mut self,
        id: u32,
        tab: i32,
        scope: Option<TaskScope>,
        name: &str,
        source: &str,
        source_target: Option<SourceTarget>,
    ) -> Result<(), String> {
        if self.entry(id).is_some() {
            return Err("이미 등록된 다운로드입니다.".into());
        }
        if !self.can_accept() {
            return Err("진행 중인 다운로드는 최대 4개입니다.".into());
        }
        if self.entries.len() >= MAX_RECORDS {
            let index = self
                .entries
                .iter()
                .position(|entry| entry.status.is_terminal())
                .ok_or_else(|| "다운로드 목록에 빈자리가 없습니다.".to_owned())?;
            self.entries.remove(index);
        }
        self.entries.push_back(Entry {
            id,
            tab,
            scope,
            status: Status::Pending,
            name: display_name(name),
            source: bounded_text(source, MAX_SOURCE_CHARS),
            source_target: source_target.filter(|target| target.tab == tab),
            path: None,
            file_verified: false,
            received: 0,
            total: 0,
            percent: None,
            reason: None,
        });
        Ok(())
    }

    pub fn entry(&self, id: u32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn choose(&mut self, id: u32) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        if entry.status != Status::Pending {
            return false;
        }
        entry.status = Status::Choosing;
        true
    }

    /// Accept the raw path returned by the native save dialog. Do not truncate,
    /// normalize or perform I/O: the runtime owns the actual save operation.
    pub fn start(&mut self, id: u32, path: &str) -> Result<(), String> {
        validate_path(path)?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.status == Status::Choosing)
            .ok_or_else(|| "저장 위치를 선택 중인 다운로드가 아닙니다.".to_owned())?;
        entry.path = Some(path.to_owned());
        entry.name = display_name(path);
        entry.status = Status::Downloading;
        Ok(())
    }

    /// Ignore unknown and terminal records. Progress must not dismiss a pending
    /// native chooser or reverse an unconfirmed cancellation request.
    pub fn update(&mut self, id: u32, progress: Progress) -> bool {
        self.update_with_approval(id, progress, None)
    }

    pub(crate) fn update_with_approval(
        &mut self,
        id: u32,
        progress: Progress,
        approved_path: Option<&str>,
    ) -> bool {
        if matches!(
            progress.status,
            Status::Pending | Status::Choosing | Status::Canceling
        ) {
            return false;
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.status.is_terminal())
        else {
            return false;
        };
        let path = progress.path.filter(|path| validate_path(path).is_ok());
        entry.file_verified = matches!(entry.status, Status::Downloading | Status::Canceling)
            && progress.status == Status::Complete
            && path
                .as_deref()
                .is_some_and(|path| Some(path) == approved_path);
        if let Some(path) = path {
            entry.name = display_name(&path);
            entry.path = Some(path);
        }
        entry.received = progress.received.max(0);
        entry.total = progress.total.max(0);
        entry.percent = (0..=100)
            .contains(&progress.percent)
            .then_some(progress.percent);
        entry.reason = progress
            .reason
            .map(|reason| bounded_text(&reason, MAX_REASON_CHARS))
            .filter(|reason| !reason.is_empty());
        if progress.status.is_terminal() || entry.status == Status::Downloading {
            entry.status = progress.status;
        }
        true
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.status.is_terminal())
        else {
            return false;
        };
        entry.request_cancel()
    }

    pub fn cancel_tab(&mut self, tab: i32) -> Vec<u32> {
        self.cancel_matching(|entry| entry.tab == tab)
    }

    /// Match every revision of this task, like revoke_task_requests. Unscoped
    /// user downloads and other tasks remain untouched.
    pub fn cancel_task(&mut self, task: &str, only_tab: Option<i32>) -> Vec<u32> {
        self.cancel_matching(|entry| {
            entry.scope.as_ref().is_some_and(|scope| scope.task == task)
                && only_tab.is_none_or(|tab| entry.tab == tab)
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            rows: self
                .entries
                .iter()
                .rev()
                .map(|entry| DownloadRow {
                    id: entry.id,
                    name: entry.name.clone(),
                    source: entry.source.clone(),
                    detail: entry.detail(),
                    can_choose: entry.status == Status::Pending,
                    can_cancel: !entry.status.is_terminal() && entry.status != Status::Canceling,
                    can_reveal: entry.source_target.is_some(),
                    can_reveal_file: entry.completed_file_path().is_some(),
                })
                .collect(),
            active_count: self.active_count(),
            notice: None,
        }
    }

    pub fn clear_finished(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| !entry.status.is_terminal());
        before - self.entries.len()
    }

    fn active_count(&self) -> usize {
        self.activity().0
    }

    fn cancel_matching(&mut self, matches: impl Fn(&Entry) -> bool) -> Vec<u32> {
        let mut canceled = Vec::new();
        for entry in &mut self.entries {
            if matches(entry) && entry.request_cancel() {
                canceled.push(entry.id);
            }
        }
        canceled
    }
}

impl Entry {
    pub(crate) fn completed_file_path(&self) -> Option<&str> {
        (self.status == Status::Complete && self.file_verified)
            .then_some(self.path.as_deref())
            .flatten()
    }

    fn request_cancel(&mut self) -> bool {
        self.status = match self.status {
            Status::Pending | Status::Choosing => Status::Canceled,
            Status::Downloading => Status::Canceling,
            Status::Canceling | Status::Canceled | Status::Complete | Status::Interrupted => {
                return false;
            }
        };
        true
    }

    fn detail(&self) -> String {
        let mut detail = match self.status {
            Status::Pending => "저장 위치를 선택해 주세요".into(),
            Status::Choosing => "저장 위치 선택 중".into(),
            Status::Downloading => {
                let mut detail = if self.total > 0 {
                    format!("다운로드 중, {} / {} 바이트", self.received, self.total)
                } else {
                    format!("다운로드 중, {} 바이트", self.received)
                };
                if let Some(percent) = self.percent {
                    detail.push_str(&format!(", {percent}%"));
                }
                detail
            }
            Status::Canceled => "취소됨".into(),
            Status::Canceling => "취소 요청 중".into(),
            Status::Complete => format!("완료, {} 바이트", self.received),
            Status::Interrupted => match &self.reason {
                Some(reason) => format!("중단됨, {reason}"),
                None => "중단됨".into(),
            },
        };
        if matches!(
            self.status,
            Status::Downloading | Status::Complete | Status::Interrupted
        ) {
            if let Some(path) = &self.path {
                detail.push_str(", 위치: ");
                detail.push_str(path);
            }
        }
        detail
    }
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('\0')
        || !Path::new(path).is_absolute()
    {
        return Err("저장 경로는 NUL 없이 8192바이트 이내의 절대 경로여야 합니다.".into());
    }
    Ok(())
}

fn display_name(value: &str) -> String {
    let basename = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let name = bounded_text(basename, MAX_NAME_CHARS);
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
}

fn bounded_text(value: &str, limit: usize) -> String {
    value
        .split_whitespace()
        .flat_map(|part| std::iter::once(' ').chain(part.chars()))
        .skip(1)
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_identity_is_bounded_to_its_record_and_never_controls_file_lifetime() {
        let mut model = Model::new();
        let source = SourceTarget {
            tab: 7,
            generation: 3,
            document_epoch: 11,
            owner: Some((
                TaskScope {
                    task: "paused-task".into(),
                    revision: 5,
                },
                crate::tasks::Status::Paused,
            )),
        };
        model
            .insert(1, 7, None, "a.txt", "작업 A, 탭 7", Some(source.clone()))
            .unwrap();
        assert!(model.snapshot().rows[0].can_reveal);
        assert_eq!(model.entry(1).unwrap().scope, None);
        // A paused user's file remains a user file when that task is canceled.
        assert!(model.cancel_task("paused-task", None).is_empty());
        assert!(model.choose(1));
        model.start(1, "/tmp/source-test.txt").unwrap();
        model.update(
            1,
            Progress {
                status: Status::Complete,
                ..Default::default()
            },
        );
        assert_eq!(model.entry(1).unwrap().source_target, Some(source.clone()));
        assert!(model.snapshot().rows[0].can_reveal);
        // Never point a different file tab at the captured source.
        model
            .insert(2, 8, None, "b.txt", "작업 B", Some(source))
            .unwrap();
        assert!(!model.snapshot().rows[0].can_reveal);
        assert_eq!(model.clear_finished(), 1);
        assert!(model.entry(1).is_none());
    }

    fn insert(model: &mut Model, id: u32) {
        model
            .insert(id, id as i32, None, "file.txt", "사용자 공간", None)
            .unwrap();
    }

    #[test]
    fn file_reveal_requires_a_fresh_complete_path_matching_user_approval() {
        let approved = "/tmp/한글 file %.txt";
        for (fresh, approval, status, expected) in [
            (Some(approved), Some(approved), Status::Complete, true),
            (None, Some(approved), Status::Complete, false),
            (
                Some("/tmp/other.txt"),
                Some(approved),
                Status::Complete,
                false,
            ),
            (Some(approved), None, Status::Complete, false),
            (Some(approved), Some(approved), Status::Downloading, false),
            (Some(approved), Some(approved), Status::Canceled, false),
            (Some(approved), Some(approved), Status::Interrupted, false),
            (Some("relative"), Some("relative"), Status::Complete, false),
        ] {
            let mut model = Model::new();
            insert(&mut model, 1);
            model.choose(1);
            model.start(1, approved).unwrap();
            // Earlier matching progress is insufficient if the completion omits it.
            model.update_with_approval(
                1,
                Progress {
                    path: Some(approved.into()),
                    ..Default::default()
                },
                Some(approved),
            );
            assert!(model.update_with_approval(
                1,
                Progress {
                    path: fresh.map(str::to_owned),
                    status,
                    ..Default::default()
                },
                approval
            ));
            assert_eq!(
                model.snapshot().rows[0].can_reveal_file,
                expected,
                "{fresh:?}, {approval:?}, {status:?}"
            );
            assert_eq!(
                model.entry(1).unwrap().completed_file_path(),
                expected.then_some(approved)
            );
        }
        let oversized = format!("/{}", "x".repeat(MAX_PATH_BYTES));
        let mut model = Model::new();
        insert(&mut model, 1);
        model.choose(1);
        model.start(1, approved).unwrap();
        model.update_with_approval(
            1,
            Progress {
                path: Some(oversized.clone()),
                status: Status::Complete,
                ..Default::default()
            },
            Some(&oversized),
        );
        assert!(!model.snapshot().rows[0].can_reveal_file);
    }

    #[test]
    fn verified_completion_survives_cancel_race_but_not_record_removal_or_reuse() {
        let path = "/tmp/completed.txt";
        let mut model = Model::new();
        insert(&mut model, 1);
        model.choose(1);
        model.start(1, path).unwrap();
        model.cancel(1);
        assert!(!model.snapshot().rows[0].can_reveal_file);
        assert!(model.update_with_approval(
            1,
            Progress {
                path: Some(path.into()),
                status: Status::Complete,
                ..Default::default()
            },
            Some(path)
        ));
        assert_eq!(model.entry(1).unwrap().completed_file_path(), Some(path));
        // A late update cannot change an already verified terminal destination.
        assert!(!model.update_with_approval(
            1,
            Progress {
                path: Some("/tmp/late.txt".into()),
                status: Status::Complete,
                ..Default::default()
            },
            Some("/tmp/late.txt")
        ));
        assert_eq!(model.entry(1).unwrap().completed_file_path(), Some(path));
        assert_eq!(model.clear_finished(), 1);
        assert!(!model.update_with_approval(
            1,
            Progress {
                path: Some(path.into()),
                status: Status::Complete,
                ..Default::default()
            },
            Some(path)
        ));
        insert(&mut model, 1);
        // Even a matching path cannot approve a fresh pending record.
        model.update_with_approval(
            1,
            Progress {
                path: Some(path.into()),
                status: Status::Complete,
                ..Default::default()
            },
            Some(path),
        );
        assert!(!model.snapshot().rows[0].can_reveal_file);
    }

    #[test]
    fn file_reveal_metadata_is_evicted_with_its_bounded_download_record() {
        let mut model = Model::new();
        for id in 1..=(MAX_RECORDS + 1) as u32 {
            insert(&mut model, id);
            model.choose(id);
            model.start(id, "/tmp/completed.txt").unwrap();
            model.update_with_approval(
                id,
                Progress {
                    path: Some("/tmp/completed.txt".into()),
                    status: Status::Complete,
                    ..Default::default()
                },
                Some("/tmp/completed.txt"),
            );
        }
        assert_eq!(model.snapshot().rows.len(), MAX_RECORDS);
        assert!(model.entry(1).is_none());
        assert!(model.snapshot().rows.iter().all(|row| row.can_reveal_file));
        assert_eq!(model.clear_finished(), MAX_RECORDS);
        assert!(model.snapshot().rows.is_empty());
    }

    #[test]
    fn pending_choosing_and_downloading_share_the_four_item_limit() {
        let mut model = Model::new();
        for id in 1..=4 {
            insert(&mut model, id);
        }
        assert!(model.choose(2));
        assert!(model.choose(3));
        model.start(3, "/tmp/file.txt").unwrap();
        assert!(!model.can_accept());
        assert!(model.insert(5, 5, None, "f", "사용자", None).is_err());
        assert_eq!(model.snapshot().active_count, 4);
        assert_eq!(model.activity(), (4, 2));
        assert!(model.cancel(2));
        assert!(model.can_accept());
        insert(&mut model, 5);
        assert_eq!(model.snapshot().active_count, 4);
        assert_eq!(model.snapshot().rows.len(), 5);
        assert_eq!(model.activity(), (4, 3));
    }

    #[test]
    fn metadata_evicts_the_oldest_terminal_entry_and_preserves_active_entries() {
        let mut model = Model::new();
        insert(&mut model, 1);
        for id in 2..=24 {
            insert(&mut model, id);
            assert!(model.cancel(id));
        }
        insert(&mut model, 25);
        assert_eq!(model.snapshot().rows.len(), MAX_RECORDS);
        assert!(model.entry(1).is_some());
        assert!(model.entry(2).is_none());
        assert!(model.entry(3).is_some());
        assert_eq!(model.snapshot().rows[0].id, 25);
        assert_eq!(model.snapshot().active_count, 2);
        assert_eq!(model.clear_finished(), 22);
        assert_eq!(model.snapshot().rows.len(), 2);
    }

    #[test]
    fn unknown_terminal_and_cleared_downloads_cannot_be_revived_by_updates() {
        let mut model = Model::new();
        assert!(!model.update(99, Progress::default()));
        insert(&mut model, 1);
        assert!(model.cancel(1));
        assert!(!model.cancel(1));
        assert!(!model.choose(1));
        assert!(!model.update(
            1,
            Progress {
                status: Status::Complete,
                ..Progress::default()
            }
        ));
        assert_eq!(model.entry(1).unwrap().status, Status::Canceled);
        assert!(model.insert(1, 1, None, "new.txt", "사용자", None).is_err());
        assert_eq!(model.clear_finished(), 1);
        assert!(!model.update(1, Progress::default()));
        assert!(model.snapshot().rows.is_empty());
    }

    #[test]
    fn active_cancel_keeps_its_slot_until_cef_reports_a_terminal_status() {
        let mut model = Model::new();
        for id in 1..=4 {
            insert(&mut model, id);
        }
        for id in 1..=2 {
            assert!(model.choose(id));
            model.start(id, "/tmp/file.txt").unwrap();
            assert!(model.cancel(id));
            assert!(!model.cancel(id));
            assert_eq!(model.entry(id).unwrap().status, Status::Canceling);
        }
        assert!(!model.can_accept());
        assert_eq!(model.snapshot().active_count, 4);
        let row = model
            .snapshot()
            .rows
            .into_iter()
            .find(|row| row.id == 1)
            .unwrap();
        assert_eq!(row.detail, "취소 요청 중");
        assert!(!row.can_choose);
        assert!(!row.can_cancel);
        assert!(model.cancel_tab(1).is_empty());
        assert_eq!(model.clear_finished(), 0);
        assert!(model.update(1, Progress::default()));
        assert_eq!(model.entry(1).unwrap().status, Status::Canceling);
        assert!(model.insert(5, 5, None, "f", "공간", None).is_err());
        assert!(model.update(
            1,
            Progress {
                status: Status::Complete,
                ..Progress::default()
            }
        ));
        assert_eq!(model.entry(1).unwrap().status, Status::Complete);
        insert(&mut model, 5);
        assert_eq!(model.snapshot().active_count, 4);
        assert!(model.update(
            2,
            Progress {
                status: Status::Canceled,
                ..Progress::default()
            }
        ));
        assert_eq!(model.entry(2).unwrap().status, Status::Canceled);
        assert_eq!(model.snapshot().active_count, 3);
        assert!(!model.update(2, Progress::default()));
    }

    #[test]
    fn progress_does_not_finish_path_selection_and_terminal_progress_is_final() {
        let mut model = Model::new();
        insert(&mut model, 1);
        assert!(model.update(
            1,
            Progress {
                received: 25,
                total: 100,
                percent: 25,
                ..Progress::default()
            }
        ));
        assert_eq!(model.entry(1).unwrap().status, Status::Pending);
        assert!(model.snapshot().rows[0].can_choose);
        assert!(model.choose(1));
        assert!(!model.choose(1));
        assert!(model.update(1, Progress::default()));
        assert_eq!(model.entry(1).unwrap().status, Status::Choosing);
        model.start(1, "/tmp/선택한 이름.txt").unwrap();
        assert_eq!(model.entry(1).unwrap().status, Status::Downloading);
        assert_eq!(model.entry(1).unwrap().name, "선택한 이름.txt");
        assert!(
            model.snapshot().rows[0]
                .detail
                .ends_with(", 위치: /tmp/선택한 이름.txt")
        );
        assert!(model.update(
            1,
            Progress {
                status: Status::Complete,
                received: 100,
                total: 100,
                percent: 100,
                ..Progress::default()
            }
        ));
        assert_eq!(model.snapshot().active_count, 0);
        assert!(!model.snapshot().rows[0].can_cancel);
        assert!(!model.update(1, Progress::default()));
        assert_eq!(model.entry(1).unwrap().status, Status::Complete);
        assert_eq!(
            model.snapshot().rows[0].detail,
            "완료, 100 바이트, 위치: /tmp/선택한 이름.txt"
        );
    }

    #[test]
    fn task_cancel_matches_all_revisions_but_preserves_unscoped_and_other_tasks() {
        let mut model = Model::new();
        for (id, tab, task, revision) in [
            (1, 7, Some("a"), 1),
            (2, 8, Some("a"), 3),
            (3, 9, Some("b"), 1),
            (4, 7, None, 0),
        ] {
            model
                .insert(
                    id,
                    tab,
                    task.map(|task| TaskScope {
                        task: task.into(),
                        revision,
                    }),
                    "f",
                    "공간",
                    None,
                )
                .unwrap();
        }
        assert!(model.choose(2));
        model.start(2, "/tmp/task-file.txt").unwrap();
        assert_eq!(model.cancel_task("a", Some(7)), vec![1]);
        assert_eq!(model.cancel_task("a", None), vec![2]);
        assert!(model.cancel_task("a", None).is_empty());
        assert_eq!(model.entry(2).unwrap().status, Status::Canceling);
        assert_eq!(model.entry(3).unwrap().status, Status::Pending);
        assert_eq!(model.entry(4).unwrap().status, Status::Pending);
        assert_eq!(model.cancel_tab(7), vec![4]);
        assert_eq!(model.snapshot().active_count, 2);
    }

    #[test]
    fn paths_are_validated_by_bytes_and_preserved_without_display_normalization() {
        for path in [
            "".to_owned(),
            "relative/file.txt".into(),
            "/tmp/a\0b".into(),
            format!("/{}", "a".repeat(MAX_PATH_BYTES)),
        ] {
            let mut model = Model::new();
            insert(&mut model, 1);
            model.choose(1);
            assert!(model.start(1, &path).is_err());
            assert_eq!(model.entry(1).unwrap().status, Status::Choosing);
            assert!(model.entry(1).unwrap().path.is_none());
        }
        let mut model = Model::new();
        insert(&mut model, 1);
        model.choose(1);
        let path = format!("/{}", "a".repeat(MAX_PATH_BYTES - 1));
        model.start(1, &path).unwrap();
        assert_eq!(model.entry(1).unwrap().path.as_deref(), Some(path.as_str()));
        assert!(model.update(
            1,
            Progress {
                path: Some(format!("{path}a")),
                status: Status::Interrupted,
                ..Progress::default()
            }
        ));
        assert_eq!(model.entry(1).unwrap().path.as_deref(), Some(path.as_str()));

        let mut model = Model::new();
        insert(&mut model, 2);
        model.choose(2);
        let raw = "/tmp/  공백\n이름  .txt";
        model.start(2, raw).unwrap();
        assert_eq!(model.entry(2).unwrap().path.as_deref(), Some(raw));
    }

    #[test]
    fn unicode_display_metadata_and_progress_are_bounded_independently() {
        let mut model = Model::new();
        model
            .insert(
                1,
                1,
                None,
                &format!("/tmp/{}", "가".repeat(300)),
                &"나".repeat(200),
                None,
            )
            .unwrap();
        assert_eq!(model.entry(1).unwrap().name.chars().count(), MAX_NAME_CHARS);
        assert!(model.update(
            1,
            Progress {
                received: -1,
                total: -1,
                percent: -1,
                status: Status::Interrupted,
                reason: Some("다".repeat(500)),
                ..Progress::default()
            }
        ));
        let row = model.snapshot().rows.remove(0);
        assert_eq!(row.name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(row.source.chars().count(), MAX_SOURCE_CHARS);
        assert_eq!(
            model
                .entry(1)
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            MAX_REASON_CHARS
        );
        assert_eq!(model.entry(1).unwrap().received, 0);
        assert_eq!(model.entry(1).unwrap().total, 0);
        assert_eq!(model.entry(1).unwrap().percent, None);
        assert!(row.detail.chars().count() <= MAX_REASON_CHARS + 5);
        assert!(model.snapshot().notice.is_none());
    }
}
