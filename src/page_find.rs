//! One native find session, with no browser or AppKit dependencies.
//!
//! CEF 8db2f367 / Chromium 156.0.8078.17 filter aborted and older query
//! sessions before OnFindResult. IDs are allocated inside WebContents, and a
//! delayed short-query find can reuse its ID for Enter: never predict IDs.
//! https://chromium.googlesource.com/chromium/src/+/refs/tags/156.0.8078.17/components/find_in_page/find_tab_helper.cc
//! https://chromium.googlesource.com/chromium/src/+/refs/tags/156.0.8078.17/content/browser/find_request_manager.cc

pub const MAX_QUERY_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub tab: i32,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub target: Target,
    pub token: u64,
    pub query: String,
    pub forward: bool,
    pub find_next: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Plan {
    pub stop: Option<i32>,
    pub issue: Option<Request>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultUpdate {
    pub target: Target,
    pub token: u64,
    pub count: i32,
    pub ordinal: i32,
    pub final_update: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ReplyPlan {
    pub update: Option<ResultUpdate>,
    pub issue: Option<Request>,
}

struct Session {
    request: Request,
    // At most one next/previous intent survives while CEF is finding.
    queued: Option<Request>,
    identifier: Option<i32>,
    after_identifier: Option<i32>,
    issued: bool,
    pending: bool,
    count: i32,
    ordinal: i32,
}

impl Session {
    fn latest_token(&self) -> u64 {
        self.queued.as_ref().unwrap_or(&self.request).token
    }
}

#[derive(Default)]
pub struct State {
    session: Option<Session>,
    last_token: u64,
}

impl State {
    pub fn target(&self) -> Option<Target> {
        self.session.as_ref().map(|session| session.request.target)
    }

    pub fn is_current(&self, request: &Request) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.request == *request)
    }

    /// Arm only immediately before Find, after StopFinding and final checks.
    /// Results cannot bind to a newly prepared request during native reentry.
    pub fn mark_issued(&mut self, request: &Request) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if session.request != *request || session.issued {
            return false;
        }
        session.issued = true;
        true
    }

    pub fn submit(&mut self, mut request: Request) -> Result<Plan, &'static str> {
        if request.token == 0 || request.token <= self.last_token {
            return Err("이미 지난 찾기 요청입니다.");
        }
        if request.query.chars().count() > MAX_QUERY_CHARS
            || request.query.chars().any(char::is_control)
        {
            return Err("찾을 내용은 제어 문자를 제외한 200자 이내로 입력해 주세요.");
        }
        self.last_token = request.token;
        if request.query.is_empty() {
            return Ok(Plan {
                stop: self
                    .session
                    .take()
                    .map(|session| session.request.target.tab),
                issue: None,
            });
        }
        if let Some(session) = self.session.as_mut() {
            if request.find_next
                && session.request.target == request.target
                && session.request.query == request.query
            {
                if session.pending {
                    session.queued = Some(request);
                    return Ok(Plan::default());
                }
                session.after_identifier = session.identifier;
                session.identifier = None;
                session.pending = true;
                session.issued = false;
                session.request = request.clone();
                return Ok(Plan {
                    stop: None,
                    issue: Some(request),
                });
            }
        }
        // Stop + restart is also required for the same text after closing the
        // bar. Otherwise CEF may skip an identical find_next=false request.
        let previous = self.session.take();
        let after_identifier = previous.as_ref().and_then(|session| {
            (session.request.target.tab == request.target.tab)
                .then_some(session.identifier.or(session.after_identifier))
                .flatten()
        });
        request.find_next = false;
        // The backend always resets the new target immediately before Find.
        // Only a different former tab needs a separate stop operation here.
        let stop = previous.and_then(|session| {
            (session.request.target.tab != request.target.tab).then_some(session.request.target.tab)
        });
        self.session = Some(Session {
            request: request.clone(),
            queued: None,
            identifier: None,
            after_identifier,
            pending: true,
            issued: false,
            count: -1,
            ordinal: -1,
        });
        Ok(Plan {
            stop,
            issue: Some(request),
        })
    }

    pub fn result(
        &mut self,
        target: Target,
        identifier: i32,
        count: i32,
        ordinal: i32,
        final_update: bool,
    ) -> ReplyPlan {
        let Some(session) = self.session.as_mut() else {
            return ReplyPlan::default();
        };
        if !session.issued
            || session.request.target != target
            || identifier < 0
            || session
                .after_identifier
                .is_some_and(|old| identifier <= old)
            || session
                .identifier
                .is_some_and(|current| identifier != current)
        {
            return ReplyPlan::default();
        }
        session.identifier = Some(identifier);
        // -1 means unchanged/unknown, not zero matches. CEF normally merges
        // these values too, but its first partial update can still be unknown.
        if count >= 0 {
            session.count = count;
        }
        if ordinal >= 0 {
            session.ordinal = ordinal;
        }
        // Dynamic document updates may follow the final reply for this request.
        // They update the count, but do not create a new pending user request.
        if final_update {
            session.pending = false;
        }
        if final_update {
            if let Some(next) = session.queued.take() {
                session.after_identifier = Some(identifier);
                session.identifier = None;
                session.pending = true;
                session.issued = false;
                session.request = next.clone();
                return ReplyPlan {
                    update: None,
                    issue: Some(next),
                };
            }
        }
        ReplyPlan {
            update: session.queued.is_none().then_some(ResultUpdate {
                target,
                token: session.request.token,
                count: session.count,
                ordinal: session.ordinal,
                final_update,
            }),
            issue: None,
        }
    }

    pub fn stop(&mut self, tab: i32, token: u64) -> bool {
        if self.session.as_ref().is_some_and(|session| {
            session.request.target.tab == tab && session.latest_token() == token
        }) {
            self.session = None;
            true
        } else {
            false
        }
    }

    pub fn invalidate(&mut self, tab: i32) -> bool {
        if self.target().is_some_and(|target| target.tab == tab) {
            self.session = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(token: u64, query: &str, next: bool) -> Request {
        Request {
            target: Target { tab: 7, epoch: 3 },
            token,
            query: query.into(),
            forward: true,
            find_next: next,
        }
    }
    fn submit(state: &mut State, request: Request) -> Result<Plan, &'static str> {
        let plan = state.submit(request)?;
        if let Some(request) = plan.issue.as_ref() {
            assert!(state.mark_issued(request));
        }
        Ok(plan)
    }
    fn reply(
        state: &mut State,
        id: i32,
        count: i32,
        ordinal: i32,
        final_update: bool,
    ) -> ReplyPlan {
        let plan = state.result(
            Target { tab: 7, epoch: 3 },
            id,
            count,
            ordinal,
            final_update,
        );
        if let Some(request) = plan.issue.as_ref() {
            assert!(state.mark_issued(request));
        }
        plan
    }

    #[test]
    fn query_replacement_stops_old_session_and_keeps_unknown_partial_values() {
        let mut state = State::default();
        submit(&mut state, request(1, "old", false)).unwrap();
        assert_eq!(
            reply(&mut state, 18, -1, -1, false).update.unwrap().count,
            -1
        );
        let plan = submit(&mut state, request(2, "new", false)).unwrap();
        assert_eq!(plan.stop, None);
        assert!(!plan.issue.as_ref().unwrap().find_next);
        assert!(reply(&mut state, 18, 99, 99, true).update.is_none());
        let current = reply(&mut state, 21, 2, 0, true).update.unwrap();
        assert_eq!((current.token, current.count), (2, 2));
        assert_eq!(reply(&mut state, 21, -1, 1, true).update.unwrap().count, 2);
        assert_eq!(
            reply(&mut state, 21, 3, -1, true).update.unwrap().ordinal,
            1
        );
        // A late dynamic partial changes counts without making a completed
        // user request busy again. The next user intent can start immediately.
        reply(&mut state, 21, 4, -1, false);
        assert!(
            submit(&mut state, request(3, "new", true))
                .unwrap()
                .issue
                .is_some()
        );
    }

    #[test]
    fn rapid_next_coalesces_to_one_intent_and_rejects_previous_request_replies() {
        let mut state = State::default();
        submit(&mut state, request(1, "a", false)).unwrap();
        assert!(
            submit(&mut state, request(2, "a", true))
                .unwrap()
                .issue
                .is_none()
        );
        let mut last = request(3, "a", true);
        last.forward = false;
        assert!(submit(&mut state, last.clone()).unwrap().issue.is_none());
        assert!(reply(&mut state, 0, 4, 0, false).update.is_none());
        assert_eq!(reply(&mut state, 0, 4, 0, true).issue, Some(last));
        assert!(reply(&mut state, 0, 3, 0, true).update.is_none());
        assert_eq!(reply(&mut state, 5, 4, 4, true).update.unwrap().token, 3);
    }

    #[test]
    fn close_only_accepts_latest_token_and_invalidates_late_dynamic_results() {
        let mut state = State::default();
        submit(&mut state, request(1, "a", false)).unwrap();
        submit(&mut state, request(2, "a", true)).unwrap();
        assert!(!state.stop(7, 1));
        assert!(!state.stop(8, 2));
        assert!(state.stop(7, 2));
        assert!(reply(&mut state, 4, 9, 1, true).update.is_none());
        submit(&mut state, request(3, "a", false)).unwrap();
        assert!(!state.invalidate(8));
        assert!(state.invalidate(7));
        assert!(reply(&mut state, 6, 9, 1, true).update.is_none());
    }

    #[test]
    fn document_and_tab_identity_are_required_and_new_query_drops_queued_next() {
        let mut state = State::default();
        submit(&mut state, request(1, "a", false)).unwrap();
        submit(&mut state, request(2, "a", true)).unwrap();
        submit(&mut state, request(3, "b", false)).unwrap();
        assert!(
            state
                .result(Target { tab: 7, epoch: 4 }, 8, 1, 1, true)
                .update
                .is_none()
        );
        assert!(
            state
                .result(Target { tab: 8, epoch: 3 }, 8, 1, 1, true)
                .update
                .is_none()
        );
        let plan = reply(&mut state, 8, 1, 1, true);
        assert!(plan.issue.is_none());
        assert_eq!(plan.update.unwrap().token, 3);
    }

    #[test]
    fn input_is_bounded_and_invalid_requests_preserve_current_session() {
        let mut state = State::default();
        assert!(submit(&mut state, request(0, "a", false)).is_err());
        submit(&mut state, request(1, &"가".repeat(200), false)).unwrap();
        assert!(submit(&mut state, request(2, &"가".repeat(201), false)).is_err());
        assert!(submit(&mut state, request(2, "\r", false)).is_err());
        assert!(submit(&mut state, request(1, "old token", false)).is_err());
        let plan = submit(&mut state, request(2, "", false)).unwrap();
        assert_eq!(plan.stop, Some(7));
        assert!(plan.issue.is_none());
        assert!(state.target().is_none());
    }

    #[test]
    fn prepared_requests_ignore_results_until_reset_and_final_native_checks_finish() {
        let mut state = State::default();
        let first = request(1, "a", false);
        state.submit(first.clone()).unwrap();
        assert!(reply(&mut state, 4, 99, 1, true).update.is_none());
        let current = request(2, "b", false);
        state.submit(current.clone()).unwrap();
        assert!(!state.mark_issued(&first));
        assert!(state.mark_issued(&current));
        assert!(!state.mark_issued(&current));
        assert_eq!(reply(&mut state, 5, 2, 1, true).update.unwrap().token, 2);
    }
}
