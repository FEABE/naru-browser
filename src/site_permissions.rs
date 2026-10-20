//! Bounded metadata for user decisions; callbacks remain in the UI runtime.

use crate::downloads::SourceTarget;
use std::collections::BTreeMap;

pub const MAX_PENDING: usize = 4;
pub const MAX_ORIGIN_BYTES: usize = 2048;
pub const MAX_FRAME_BYTES: usize = 256;
// Pinned CEF 156 permission masks. Kept here so this model needs no CEF runtime.
pub const GEOLOCATION: u32 = 256;
pub const NOTIFICATIONS: u32 = 32768;
pub const DEVICE_AUDIO: u32 = 1;
pub const DEVICE_VIDEO: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteSetting {
    Allowed,
    Blocked,
    Ask,
    Unknown,
}

impl SiteSetting {
    pub fn label(self) -> &'static str {
        match self {
            Self::Allowed => "허용",
            Self::Blocked => "차단",
            Self::Ask => "요청 시 확인",
            Self::Unknown => "확인할 수 없음",
        }
    }

    pub fn can_reset(self) -> bool {
        matches!(self, Self::Allowed | Self::Blocked)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub source: SourceTarget,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Generic { prompt: u64, mask: u32 },
    Media { mask: u32, frame: String },
}

impl Kind {
    pub fn generic(prompt: u64, mask: u32) -> Result<Self, &'static str> {
        if mask == 0 || mask & !(GEOLOCATION | NOTIFICATIONS) != 0 {
            return Err("이 사이트 권한은 아직 지원하지 않아 요청을 취소했습니다.");
        }
        Ok(Self::Generic { prompt, mask })
    }

    pub fn media(mask: u32, frame: String) -> Result<Self, &'static str> {
        if mask == 0 || mask & !(DEVICE_AUDIO | DEVICE_VIDEO) != 0 {
            return Err("화면 공유 등 이 미디어 권한은 아직 지원하지 않아 요청을 취소했습니다.");
        }
        if frame.is_empty() || frame.len() > MAX_FRAME_BYTES || frame.chars().any(char::is_control)
        {
            return Err("미디어를 요청한 문서를 확인할 수 없습니다.");
        }
        Ok(Self::Media { mask, frame })
    }

    pub fn is_generic(&self) -> bool {
        matches!(self, Self::Generic { .. })
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Generic {
                mask: GEOLOCATION, ..
            } => "위치",
            Self::Generic {
                mask: NOTIFICATIONS,
                ..
            } => "알림",
            Self::Generic { .. } => "위치, 알림",
            Self::Media {
                mask: DEVICE_AUDIO, ..
            } => "마이크",
            Self::Media {
                mask: DEVICE_VIDEO, ..
            } => "카메라",
            Self::Media { .. } => "마이크, 카메라",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub token: u64,
    pub tab: i32,
    pub origin: String,
    pub source: String,
    pub permissions: String,
    pub scope_hint: String,
    pub allow_label: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub target: Target,
    pub kind: Kind,
    pub row: Row,
}

/// An origin supplied by CEF must be an exact HTTP(S) authority, not a URL
/// whose path or opaque origin could be silently broadened into another scope.
pub(crate) fn exact_origin(value: &str) -> Result<String, &'static str> {
    let invalid = "권한을 요청한 사이트 주소를 확인할 수 없습니다.";
    if value.len() > MAX_ORIGIN_BYTES || !value.is_ascii() || value.chars().any(char::is_whitespace)
    {
        return Err(invalid);
    }
    let normalized = crate::address::normalize_url(value).map_err(|_| invalid)?;
    let (_, authority) = normalized.split_once("://").ok_or(invalid)?;
    let suffix = &authority[authority.find(['/', '?', '#']).unwrap_or(authority.len())..];
    if !suffix.is_empty() && suffix != "/" {
        return Err(invalid);
    }
    crate::download_permissions::site_origin(&normalized).map_err(|_| invalid)
}

#[derive(Default)]
pub(crate) struct Model {
    requests: BTreeMap<u64, Request>,
    next_token: u64,
}

impl Model {
    pub fn rows(&self) -> Vec<Row> {
        self.requests
            .values()
            .map(|request| request.row.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn get(&self, token: u64) -> Option<&Request> {
        self.requests.get(&token)
    }

    pub fn insert(
        &mut self,
        target: Target,
        kind: Kind,
        origin: &str,
        source: &str,
    ) -> Result<u64, &'static str> {
        if self.requests.len() >= MAX_PENDING {
            return Err("사이트 권한 요청은 최대 4개까지 기다릴 수 있습니다.");
        }
        if self
            .requests
            .values()
            .any(|request| request.target.source.tab == target.source.tab)
        {
            return Err("이 탭에서 이미 사이트 권한 결정을 기다리고 있습니다.");
        }
        let origin = exact_origin(origin)?;
        let token = self
            .next_token
            .checked_add(1)
            .ok_or("사이트 권한 요청을 더 만들 수 없습니다.")?;
        let (allow_label, scope_hint) = if kind.is_generic() {
            (
                "이 사이트 허용",
                "허용은 브라우저 프로필에 저장되며 내 공간과 AI 공간에서 함께 적용됩니다.",
            )
        } else {
            (
                "이번 요청 허용",
                "이번 미디어 요청만 허용합니다. macOS 권한은 별도이며, 작업을 중지해도 이미 시작된 스트림은 자동으로 종료되지 않습니다.",
            )
        };
        self.next_token = token;
        self.requests.insert(
            token,
            Request {
                row: Row {
                    token,
                    tab: target.source.tab,
                    origin,
                    source: source
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(80)
                        .collect(),
                    permissions: kind.label().into(),
                    scope_hint: scope_hint.into(),
                    allow_label: allow_label.into(),
                },
                target,
                kind,
            },
        );
        Ok(token)
    }

    pub fn take(&mut self, token: u64) -> Option<Request> {
        self.requests.remove(&token)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tab: i32) -> Target {
        Target {
            source: SourceTarget {
                tab,
                generation: 1,
                document_epoch: 1,
                owner: None,
            },
            generation: 1,
        }
    }

    #[test]
    fn site_permission_model_bounds_origin_masks_and_monotonic_tokens() {
        for origin in [
            "",
            "file:///tmp/a",
            "data:text/plain,x",
            "https://a.test/path",
            "https://a.test?q=x",
            "https://u:p@a.test",
        ] {
            assert!(exact_origin(origin).is_err());
        }
        assert_eq!(
            exact_origin("HTTPS://A.test:443/"),
            Ok("https://a.test".into())
        );
        assert_eq!(
            exact_origin("https://a.test:8443"),
            Ok("https://a.test:8443".into())
        );
        assert!(Kind::generic(1, GEOLOCATION | NOTIFICATIONS).is_ok());
        assert!(Kind::generic(1, GEOLOCATION | 16384).is_err());
        assert!(Kind::generic(1, 0).is_err());
        assert!(Kind::media(DEVICE_AUDIO | DEVICE_VIDEO, "frame".into()).is_ok());
        assert!(Kind::media(DEVICE_VIDEO | 8, "frame".into()).is_err());
        let mut model = Model::default();
        let kind = Kind::generic(1, GEOLOCATION).unwrap();
        let first = model
            .insert(target(1), kind.clone(), "https://a.test", "user")
            .unwrap();
        assert!(
            model
                .insert(target(1), kind.clone(), "https://b.test", "user")
                .is_err()
        );
        for tab in 2..=4 {
            model
                .insert(target(tab), kind.clone(), "https://a.test", "user")
                .unwrap();
        }
        assert!(
            model
                .insert(target(5), kind.clone(), "https://a.test", "user")
                .is_err()
        );
        assert_eq!(model.take_matching(|_| true).len(), 4);
        let next = model
            .insert(target(1), kind, "https://a.test", "user")
            .unwrap();
        assert!(next > first);
        assert!(model.get(first).is_none());
        assert_eq!(model.rows()[0].allow_label, "이 사이트 허용");
    }
}
