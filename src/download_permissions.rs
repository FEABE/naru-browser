//! Bounded metadata for explicit multiple-download permission decisions.

use crate::{protocol::TaskScope, tasks::Status};
use std::collections::BTreeMap;

pub const MAX_PENDING: usize = 4;
pub const MAX_ORIGIN_BYTES: usize = 2048;
const MAX_SOURCE_CHARS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteSetting {
    Allowed,
    Ask,
    Blocked,
    Unknown,
}

/// Derive one exact HTTP(S) origin from a live page URL. Never turn an invalid
/// or opaque URL into an empty scope, which CEF treats as a global setting.
pub(crate) fn site_origin(value: &str) -> Result<String, &'static str> {
    let invalid = "현재 사이트 주소를 확인할 수 없습니다.";
    let normalized = crate::address::normalize_url(value).map_err(|_| invalid)?;
    let (scheme, rest) = normalized.split_once("://").ok_or(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().ok_or(invalid)?;
    if !authority.is_ascii() {
        return Err(invalid);
    }
    let authority = authority.to_ascii_lowercase();
    // CEF supplies canonical URLs, but pending permission origins may include
    // an explicit default port. Match those without merging distinct ports.
    let authority = authority
        .strip_suffix(if scheme == "https" { ":443" } else { ":80" })
        .unwrap_or(&authority);
    let origin = format!("{scheme}://{authority}");
    if origin.len() > MAX_ORIGIN_BYTES {
        return Err(invalid);
    }
    Ok(origin)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub tab: i32,
    pub epoch: u64,
    // A paused task is still an owner, even though its user downloads are
    // unscoped. Resuming or adopting must invalidate the pending approval.
    pub owner: Option<(TaskScope, Status)>,
}

/// Chromium can defer creating a prompt until a hidden tab becomes visible.
/// CanDownload has no prompt/request ID, so keep one immutable provenance per
/// document instead of reassigning an old request to a later controller.
#[derive(Default)]
pub(crate) struct Provenance {
    first_attempt: Option<Target>,
    revoked: bool,
}

impl Provenance {
    pub fn record(&mut self, target: Target) {
        if let Some(first) = &self.first_attempt {
            self.revoked |= first != &target;
        } else {
            self.first_attempt = Some(target);
        }
    }

    pub fn revoke(&mut self) {
        self.revoked |= self.first_attempt.is_some();
    }

    pub fn permits(&self, target: &Target) -> bool {
        !self.revoked && self.first_attempt.as_ref() == Some(target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRow {
    pub token: u64,
    pub origin: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub target: Target,
    pub prompt_id: u64,
    pub row: PermissionRow,
}

#[derive(Default)]
pub(crate) struct Model {
    requests: BTreeMap<u64, Request>,
    next_token: u64,
}

impl Model {
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn rows(&self) -> Vec<PermissionRow> {
        self.requests
            .values()
            .map(|request| request.row.clone())
            .collect()
    }

    pub fn insert(
        &mut self,
        target: Target,
        prompt_id: u64,
        origin: &str,
        source: &str,
    ) -> Result<u64, &'static str> {
        if self.requests.len() >= MAX_PENDING {
            return Err("다운로드 권한 요청은 최대 4개까지 기다릴 수 있습니다.");
        }
        if self
            .requests
            .values()
            .any(|request| request.target.tab == target.tab && request.prompt_id == prompt_id)
        {
            return Err("이미 대기 중인 다운로드 권한 요청입니다.");
        }
        let origin = display_origin(origin)?;
        let token = self
            .next_token
            .checked_add(1)
            .ok_or("다운로드 권한 요청을 더 만들 수 없습니다.")?;
        self.next_token = token;
        self.requests.insert(
            token,
            Request {
                target,
                prompt_id,
                row: PermissionRow {
                    token,
                    origin,
                    source: source
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(MAX_SOURCE_CHARS)
                        .collect(),
                },
            },
        );
        Ok(token)
    }

    pub fn take(&mut self, token: u64) -> Option<Request> {
        self.requests.remove(&token)
    }

    pub fn get(&self, token: u64) -> Option<&Request> {
        self.requests.get(&token)
    }

    pub fn take_matching(&mut self, matches: impl Fn(&Request) -> bool) -> Vec<Request> {
        let tokens = self
            .requests
            .iter()
            .filter_map(|(token, request)| matches(request).then_some(*token))
            .collect::<Vec<_>>();
        tokens
            .into_iter()
            .filter_map(|token| self.take(token))
            .collect()
    }
}

/// CEF supplies an origin, never a download URL. Reject malformed values rather
/// than truncating an authority into a misleading permission label.
fn display_origin(value: &str) -> Result<String, &'static str> {
    let invalid = "다운로드를 요청한 사이트 주소를 확인할 수 없습니다.";
    if value.len() > MAX_ORIGIN_BYTES || !value.is_ascii() || value.chars().any(char::is_whitespace)
    {
        return Err(invalid);
    }
    let normalized = crate::address::normalize_url(value).map_err(|_| invalid)?;
    let (_, rest) = normalized.split_once("://").ok_or(invalid)?;
    let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let suffix = &rest[authority_len..];
    if !suffix.is_empty() && suffix != "/" {
        return Err(invalid);
    }
    Ok(normalized.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tab: i32) -> Target {
        Target {
            tab,
            epoch: 7,
            owner: None,
        }
    }

    #[test]
    fn site_origin_preserves_scheme_and_nondefault_port_without_url_details() {
        assert_eq!(
            site_origin("HTTPS://EXAMPLE.test:443/a?q=secret#part"),
            Ok("https://example.test".into())
        );
        assert_eq!(
            site_origin("http://example.test:80/a"),
            Ok("http://example.test".into())
        );
        assert_eq!(
            site_origin("https://example.test:8443/a"),
            Ok("https://example.test:8443".into())
        );
        assert_eq!(
            site_origin("https://[::1]:443/a"),
            Ok("https://[::1]".into())
        );
        assert_eq!(site_origin("http://[::80]/a"), Ok("http://[::80]".into()));
        for value in [
            "",
            "about:blank",
            "data:text/plain,a",
            "file:///tmp/a",
            "https://user:password@example.test",
            "https://example.test:bad",
            "https://예시.test/a",
        ] {
            assert!(site_origin(value).is_err(), "{value}");
        }
        assert!(site_origin(&format!("https://{}.test", "a".repeat(MAX_ORIGIN_BYTES))).is_err());
    }

    #[test]
    fn hidden_request_provenance_cannot_be_reassigned_after_handoff_or_round_trip() {
        let original = target(7);
        let mut provenance = Provenance::default();
        assert!(!provenance.permits(&original));
        provenance.record(original.clone());
        assert!(provenance.permits(&original));
        provenance.revoke();
        provenance.record(original.clone());
        assert!(!provenance.permits(&original));
        let reassigned = Target {
            owner: Some((
                TaskScope {
                    task: "new-owner".into(),
                    revision: 1,
                },
                Status::Active,
            )),
            ..original
        };
        provenance.record(reassigned.clone());
        assert!(!provenance.permits(&reassigned));
    }

    #[test]
    fn unobserved_documents_can_start_after_handoff_but_new_attempt_cannot_replace_old_owner() {
        let mut provenance = Provenance::default();
        provenance.revoke();
        let original = target(7);
        provenance.record(original.clone());
        assert!(provenance.permits(&original));
        let other = Target {
            epoch: 8,
            ..original.clone()
        };
        provenance.record(other.clone());
        assert!(!provenance.permits(&other));
        assert!(!provenance.permits(&original));
    }

    #[test]
    fn capacity_and_duplicates_never_evict_an_existing_decision() {
        let mut model = Model::default();
        let first = model
            .insert(target(1), 9, "https://example.test", "사용자")
            .unwrap();
        assert!(
            model
                .insert(target(1), 9, "https://other.test", "사용자")
                .is_err()
        );
        for tab in 2..=4 {
            model
                .insert(target(tab), 9, "https://example.test", "사용자")
                .unwrap();
        }
        assert!(
            model
                .insert(target(5), 9, "https://example.test", "사용자")
                .is_err()
        );
        assert_eq!(model.len(), MAX_PENDING);
        assert_eq!(
            model.take(first).unwrap().row.origin,
            "https://example.test"
        );
        let next = model
            .insert(target(5), 9, "https://example.test", "사용자")
            .unwrap();
        assert!(next > first);
        assert!(model.take(first).is_none());
    }

    #[test]
    fn removing_all_requests_does_not_recycle_ui_tokens() {
        let mut model = Model::default();
        let first = model
            .insert(target(1), 1, "http://localhost:8123", "사용자")
            .unwrap();
        model.take_matching(|_| true);
        let second = model
            .insert(target(1), 1, "http://localhost:8123", "사용자")
            .unwrap();
        assert!(second > first);
        model.next_token = u64::MAX;
        assert!(
            model
                .insert(target(2), 1, "https://example.test", "사용자")
                .is_err()
        );
        assert_eq!(model.len(), 1);
    }

    #[test]
    fn origin_is_complete_and_never_contains_paths_credentials_or_controls() {
        for (value, expected) in [
            ("HTTPS://example.test:8443/", "https://example.test:8443"),
            ("http://[::1]:8123", "http://[::1]:8123"),
            ("https://xn--3e0b707e.test", "https://xn--3e0b707e.test"),
        ] {
            assert_eq!(display_origin(value).unwrap(), expected);
        }
        for value in [
            "",
            "null",
            "file:///tmp/a",
            "blob:https://example.test/id",
            "https://user:secret@example.test",
            "https://example.test/private",
            "https://example.test/?secret=value",
            "https://example.test#fragment",
            "https://example.test\\evil",
            "https://example.test\n",
            " https://example.test",
            "https://한글.test",
            "https://example.test:65536",
            "https://[invalid]",
        ] {
            assert!(display_origin(value).is_err(), "{value}");
        }
        assert!(display_origin(&format!("https://{}", "a".repeat(MAX_ORIGIN_BYTES))).is_err());
    }

    #[test]
    fn removal_matches_scope_even_for_paused_user_requests_and_keeps_other_tabs() {
        let mut model = Model::default();
        let scope = TaskScope {
            task: "task-1".into(),
            revision: 2,
        };
        let mut owned = target(1);
        owned.owner = Some((scope.clone(), Status::Paused));
        let first = model
            .insert(owned.clone(), 1, "https://example.test", &"한".repeat(200))
            .unwrap();
        model
            .insert(target(2), 1, "https://example.test", "다른 탭")
            .unwrap();
        assert_eq!(model.rows()[0].source.chars().count(), MAX_SOURCE_CHARS);
        assert_ne!(owned, target(1));
        assert_ne!(
            owned,
            Target {
                owner: Some((scope, Status::Active)),
                ..owned.clone()
            }
        );
        assert_ne!(
            owned,
            Target {
                epoch: 8,
                ..owned.clone()
            }
        );
        let removed = model.take_matching(|request| {
            request
                .target
                .owner
                .as_ref()
                .is_some_and(|(owner, _)| owner.task == "task-1")
        });
        assert_eq!(removed[0].row.token, first);
        assert_eq!(model.len(), 1);
        assert_eq!(model.rows()[0].source, "다른 탭");
    }
}
