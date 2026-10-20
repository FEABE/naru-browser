//! Bounded, in-memory metadata for file selections requested by active AI tasks.

use crate::{downloads::SourceTarget, protocol::TaskScope};
use std::{collections::BTreeMap, path::Path};

pub const MAX_PENDING: usize = 4;
pub const MAX_FILES: usize = 64;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_FILTERS: usize = 32;
pub const MAX_FILTER_BYTES: usize = 256;
pub const MAX_EXTENSIONS: usize = 128;
const MAX_EXTENSION_BYTES: usize = 64;
const MAX_SOURCE_CHARS: usize = 80;
const MAX_HINT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Open,
    Multiple,
    Folder,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "파일 1개",
            Self::Multiple => "파일 여러 개",
            Self::Folder => "폴더",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Multiple => "multiple",
            Self::Folder => "folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub token: u64,
    pub tab: i32,
    pub source: String,
    pub mode: Mode,
    pub choosing: bool,
    pub accept_hint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Filters {
    pub extensions: Vec<String>,
    pub hint: String,
}

impl Filters {
    pub fn new(filters: &[String], expanded: &[String]) -> Result<Self, &'static str> {
        if filters.len() > MAX_FILTERS
            || expanded.len() > MAX_FILTERS
            || filters
                .iter()
                .chain(expanded)
                .any(|value| value.len() > MAX_FILTER_BYTES)
        {
            return Err("파일 형식 요청이 너무 길어 선택 창을 열 수 없습니다.");
        }
        let mut extensions = Vec::new();
        let mut unknown = false;
        for (index, filter) in filters.iter().enumerate() {
            if let Some(extension) = filter.strip_prefix('.') {
                if let Some(extension) = normalized_extension(extension) {
                    add_extension(&mut extensions, extension)?;
                } else {
                    unknown = true;
                }
            } else {
                let expansion = expanded.get(index).map(String::as_str).unwrap_or("");
                let mut found = false;
                for extension in expansion.split(';').filter(|part| !part.trim().is_empty()) {
                    if let Some(extension) =
                        normalized_extension(extension.trim().trim_start_matches('.'))
                    {
                        add_extension(&mut extensions, extension)?;
                        found = true;
                    } else {
                        unknown = true;
                    }
                }
                unknown |= !found;
            }
        }
        // Filters are alternatives. Narrowing to just the recognized subset
        // would hide files allowed by an unexpanded MIME type.
        if unknown {
            extensions.clear();
        }
        let mut hint = String::new();
        for filter in filters {
            let value = filter
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>();
            if value.is_empty() {
                continue;
            }
            if !hint.is_empty() {
                push_bounded(&mut hint, " · ", MAX_HINT_BYTES);
            }
            push_bounded(&mut hint, &value, MAX_HINT_BYTES);
            if hint.len() >= MAX_HINT_BYTES {
                break;
            }
        }
        if hint.is_empty() {
            hint = "모든 파일".into();
        }
        Ok(Self { extensions, hint })
    }
}

fn normalized_extension(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_EXTENSION_BYTES
        || !value.as_bytes().first()?.is_ascii_alphanumeric()
        || value.split('.').any(str::is_empty)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn add_extension(extensions: &mut Vec<String>, value: String) -> Result<(), &'static str> {
    if !extensions.contains(&value) {
        if extensions.len() >= MAX_EXTENSIONS {
            return Err("요청한 파일 확장자가 너무 많아 선택 창을 열 수 없습니다.");
        }
        extensions.push(value);
    }
    Ok(())
}

fn push_bounded(output: &mut String, value: &str, max_bytes: usize) {
    for character in value.chars() {
        if output.len() + character.len_utf8() > max_bytes {
            break;
        }
        output.push(character);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub target: SourceTarget,
    pub scope: TaskScope,
    pub generation: u64,
    pub row: Row,
    pub extensions: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Model {
    requests: BTreeMap<u64, Request>,
    next_token: u64,
    // Canceling metadata must not free the native slot before its dismissal.
    choosing: Option<u64>,
}

impl Model {
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn rows(&self) -> Vec<Row> {
        self.requests
            .values()
            .map(|request| request.row.clone())
            .collect()
    }

    pub fn choosing_token(&self) -> Option<u64> {
        self.choosing
    }

    pub fn insert(
        &mut self,
        target: SourceTarget,
        scope: TaskScope,
        generation: u64,
        source: &str,
        mode: Mode,
        filters: Filters,
    ) -> Result<u64, &'static str> {
        if self.requests.len() >= MAX_PENDING {
            return Err("파일 선택 요청은 최대 4개까지 기다릴 수 있습니다.");
        }
        if self
            .requests
            .values()
            .any(|request| request.target.tab == target.tab)
        {
            return Err("이 탭에서 이미 파일 선택을 기다리고 있습니다.");
        }
        let token = self
            .next_token
            .checked_add(1)
            .ok_or("파일 선택 요청을 더 만들 수 없습니다.")?;
        self.next_token = token;
        self.requests.insert(
            token,
            Request {
                row: Row {
                    token,
                    tab: target.tab,
                    source: source
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(MAX_SOURCE_CHARS)
                        .collect(),
                    mode,
                    choosing: false,
                    accept_hint: filters.hint,
                },
                target,
                scope,
                generation,
                extensions: filters.extensions,
            },
        );
        Ok(token)
    }

    pub fn get(&self, token: u64) -> Option<&Request> {
        self.requests.get(&token)
    }

    pub fn begin(&mut self, token: u64) -> Result<Request, &'static str> {
        if self.choosing.is_some() {
            return Err("열려 있는 파일 선택 창을 먼저 닫아 주세요.");
        }
        let request = self
            .requests
            .get_mut(&token)
            .ok_or("파일 선택 요청이 만료되었습니다.")?;
        request.row.choosing = true;
        self.choosing = Some(token);
        Ok(request.clone())
    }

    pub fn remove(&mut self, token: u64) -> Option<Request> {
        self.requests.remove(&token)
    }

    pub fn remove_matching(&mut self, matches: impl Fn(&Request) -> bool) -> Vec<Request> {
        let tokens = self
            .requests
            .iter()
            .filter_map(|(token, request)| matches(request).then_some(*token))
            .collect::<Vec<_>>();
        tokens
            .into_iter()
            .filter_map(|token| self.remove(token))
            .collect()
    }

    pub fn dismiss(&mut self, token: u64) -> Option<Request> {
        if self.choosing != Some(token) {
            return None;
        }
        self.choosing = None;
        // Keep the row until runtime finishes preparing the CEF path list.
        // Reentrant navigation/task revocation can still remove it and veto
        // delivery even after the native picker has already disappeared.
        self.get(token).cloned()
    }
}

pub(crate) fn validate_paths(mode: Mode, paths: &[String]) -> Result<(), &'static str> {
    if paths.is_empty() || paths.len() > MAX_FILES || (mode != Mode::Multiple && paths.len() != 1) {
        return Err("선택한 파일 개수가 요청과 맞지 않습니다.");
    }
    if paths.iter().any(|path| {
        path.len() > MAX_PATH_BYTES || path.contains('\0') || !Path::new(path).is_absolute()
    }) {
        return Err("선택한 파일 경로를 확인할 수 없습니다.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(model: &mut Model, tab: i32) -> u64 {
        model
            .insert(
                SourceTarget {
                    tab,
                    generation: 1,
                    document_epoch: 1,
                    owner: None,
                },
                TaskScope {
                    task: "task-1".into(),
                    revision: 1,
                },
                1,
                "AI 작업",
                Mode::Open,
                Filters::new(&[], &[]).unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn file_selection_queue_bounds_tab_duplicates_and_monotonic_tokens() {
        let mut model = Model::default();
        let first = insert(&mut model, 1);
        assert!(
            model
                .insert(
                    SourceTarget {
                        tab: 1,
                        generation: 2,
                        document_epoch: 2,
                        owner: None
                    },
                    TaskScope {
                        task: "task-2".into(),
                        revision: 1
                    },
                    1,
                    "다른 작업",
                    Mode::Open,
                    Filters::new(&[], &[]).unwrap(),
                )
                .is_err()
        );
        for tab in 2..=4 {
            insert(&mut model, tab);
        }
        assert_eq!(model.len(), MAX_PENDING);
        assert!(
            model
                .insert(
                    SourceTarget {
                        tab: 5,
                        generation: 1,
                        document_epoch: 1,
                        owner: None
                    },
                    TaskScope {
                        task: "task-1".into(),
                        revision: 1
                    },
                    1,
                    "AI 작업",
                    Mode::Open,
                    Filters::new(&[], &[]).unwrap(),
                )
                .is_err()
        );
        model.remove_matching(|_| true);
        assert!(insert(&mut model, 1) > first);
        model.remove_matching(|_| true);
        model.next_token = u64::MAX;
        assert!(
            model
                .insert(
                    SourceTarget {
                        tab: 1,
                        generation: 1,
                        document_epoch: 1,
                        owner: None
                    },
                    TaskScope {
                        task: "task-1".into(),
                        revision: 1
                    },
                    1,
                    "AI 작업",
                    Mode::Open,
                    Filters::new(&[], &[]).unwrap(),
                )
                .is_err()
        );
        assert_eq!(model.len(), 0);
    }

    #[test]
    fn file_selection_canceled_sheet_holds_slot_until_exact_completion() {
        let mut model = Model::default();
        let old = insert(&mut model, 1);
        let next = insert(&mut model, 2);
        assert!(model.begin(old).unwrap().row.choosing);
        model.remove(old);
        assert_eq!(model.choosing_token(), Some(old));
        assert!(model.begin(next).is_err());
        assert!(model.dismiss(next).is_none());
        assert_eq!(model.choosing_token(), Some(old));
        assert!(model.dismiss(old).is_none());
        assert_eq!(model.choosing_token(), None);
        model.begin(next).unwrap();
        assert!(model.dismiss(old).is_none());
        assert_eq!(model.choosing_token(), Some(next));
        assert_eq!(model.dismiss(next).unwrap().row.token, next);
        assert!(model.get(next).is_some());
        assert!(model.dismiss(next).is_none());
        model.remove_matching(|request| request.target.tab == 2);
        assert!(model.remove(next).is_none());
    }

    #[test]
    fn file_selection_filters_bound_expansion_and_do_not_narrow_unknown_mime() {
        let known = Filters::new(
            &["image/png".into(), ".TXT".into(), ".tar.gz".into()],
            &[".png;.PNG".into(), "".into(), "".into()],
        )
        .unwrap();
        assert_eq!(known.extensions, ["png", "txt", "tar.gz"]);
        let unknown = Filters::new(
            &["image/png".into(), "application/x-unknown".into()],
            &[".png".into(), "".into()],
        )
        .unwrap();
        assert!(unknown.extensions.is_empty());
        for malformed in [".*", "../secret", ".a/b", ".a..b", ".\0"] {
            assert!(
                Filters::new(&[malformed.into()], &[])
                    .unwrap()
                    .extensions
                    .is_empty()
            );
        }
        assert!(Filters::new(&vec![".txt".into(); MAX_FILTERS + 1], &[]).is_err());
        assert!(Filters::new(&["x".repeat(MAX_FILTER_BYTES + 1)], &[]).is_err());
        let filters = vec!["a/b".into(); MAX_FILTERS];
        let expanded = (0..MAX_FILTERS)
            .map(|i| {
                (0..5)
                    .map(|n| format!(".e{i}_{n}"))
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .collect::<Vec<_>>();
        assert!(Filters::new(&filters, &expanded).is_err());
        let hints = Filters::new(&vec!["한".repeat(80); MAX_FILTERS], &[]).unwrap();
        assert!(hints.hint.len() <= MAX_HINT_BYTES);
    }

    #[test]
    fn file_selection_paths_require_mode_count_absolute_names_and_byte_bounds() {
        assert!(validate_paths(Mode::Open, &["/tmp/한글.txt".into()]).is_ok());
        assert!(validate_paths(Mode::Folder, &["/tmp/folder".into()]).is_ok());
        assert!(validate_paths(Mode::Multiple, &vec!["/tmp/a".into(); MAX_FILES]).is_ok());
        assert!(validate_paths(Mode::Multiple, &vec!["/tmp/a".into(); MAX_FILES + 1]).is_err());
        assert!(validate_paths(Mode::Open, &["/tmp/a".into(), "/tmp/b".into()]).is_err());
        for mode in [Mode::Open, Mode::Multiple, Mode::Folder] {
            assert!(validate_paths(mode, &[]).is_err());
            assert!(validate_paths(mode, &["relative.txt".into()]).is_err());
            assert!(validate_paths(mode, &["/tmp/a\0b".into()]).is_err());
            assert!(validate_paths(mode, &[format!("/{}", "한".repeat(MAX_PATH_BYTES))]).is_err());
        }
    }
}
