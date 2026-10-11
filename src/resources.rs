//! Admission limits for automation-created tabs, including pending creations.
//! The caller supplies current tab counts and validates task ownership separately.

use crate::protocol::TaskScope;
use crate::tasks::{MAX_TASK_TABS, TaskError};
use std::collections::BTreeMap;

pub const MAX_AUTOMATION_TABS: usize = 8;

#[derive(Debug)]
struct Reservation {
    scope: Option<TaskScope>,
    opener: Option<(i32, i32)>,
}

#[derive(Debug, Default)]
pub struct ResourceBudget {
    reservations: BTreeMap<u64, Reservation>,
    last_ticket: u64,
}

impl ResourceBudget {
    pub fn reserve(
        &mut self,
        existing_tabs: usize,
        task_tabs: usize,
        scope: Option<TaskScope>,
        opener: Option<(i32, i32)>,
    ) -> Result<u64, TaskError> {
        if existing_tabs.saturating_add(self.len()) >= MAX_AUTOMATION_TABS {
            return Err(TaskError {
                code: "TAB_LIMIT",
                message: "The browser tab limit includes tabs still being created.",
            });
        }
        if scope.as_ref().is_some_and(|scope| {
            task_tabs.saturating_add(self.task_reserved(&scope.task)) >= MAX_TASK_TABS
        }) {
            return Err(TaskError {
                code: "TASK_TAB_LIMIT",
                message: "The task tab limit includes tabs still being created.",
            });
        }
        let ticket = self.last_ticket.checked_add(1).ok_or(TaskError {
            code: "RESERVATION_LIMIT",
            message: "No more unique tab reservation tickets are available.",
        })?;
        self.reservations
            .insert(ticket, Reservation { scope, opener });
        self.last_ticket = ticket;
        Ok(ticket)
    }

    pub fn task_reserved(&self, task: &str) -> usize {
        self.reservations
            .values()
            .filter(|reservation| {
                reservation
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.task == task)
            })
            .count()
    }

    pub fn release(&mut self, ticket: u64) -> bool {
        self.reservations.remove(&ticket).is_some()
    }

    pub fn contains(&self, ticket: u64) -> bool {
        self.reservations.contains_key(&ticket)
    }

    pub fn abort_popup(&mut self, opener: i32, popup: i32) -> usize {
        let before = self.len();
        self.reservations
            .retain(|_, reservation| reservation.opener != Some((opener, popup)));
        before - self.len()
    }

    pub fn close_opener(&mut self, tab: i32) -> usize {
        let before = self.len();
        self.reservations
            .retain(|_, reservation| !reservation.opener.is_some_and(|(opener, _)| opener == tab));
        before - self.len()
    }

    pub fn len(&self) -> usize {
        self.reservations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reservations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(task: &str, revision: u64) -> TaskScope {
        TaskScope {
            task: task.into(),
            revision,
        }
    }

    #[test]
    fn global_limit_counts_existing_tabs_and_pending_creations() {
        let mut budget = ResourceBudget::default();
        let ticket = budget
            .reserve(MAX_AUTOMATION_TABS - 1, 0, None, None)
            .unwrap();
        assert_eq!(budget.len(), 1);
        assert_eq!(
            budget
                .reserve(MAX_AUTOMATION_TABS - 1, 0, None, None)
                .unwrap_err()
                .code,
            "TAB_LIMIT"
        );
        assert!(budget.contains(ticket));
        assert!(budget.release(ticket));
        assert!(
            budget
                .reserve(MAX_AUTOMATION_TABS - 1, 0, None, None)
                .is_ok()
        );
        assert_eq!(
            budget.reserve(usize::MAX, 0, None, None).unwrap_err().code,
            "TAB_LIMIT"
        );
    }

    #[test]
    fn global_limit_also_applies_when_every_tab_is_still_pending() {
        let mut budget = ResourceBudget::default();
        for _ in 0..MAX_AUTOMATION_TABS {
            budget.reserve(0, 0, None, None).unwrap();
        }
        assert_eq!(
            budget.reserve(0, 0, None, None).unwrap_err().code,
            "TAB_LIMIT"
        );
        assert_eq!(budget.len(), MAX_AUTOMATION_TABS);
    }

    #[test]
    fn task_limit_counts_reservations_across_revisions_but_not_other_tasks() {
        let mut budget = ResourceBudget::default();
        let first = budget.reserve(0, 0, Some(scope("a", 1)), None).unwrap();
        budget.reserve(0, 0, Some(scope("a", 2)), None).unwrap();
        budget.reserve(0, 0, Some(scope("b", 1)), None).unwrap();
        budget.reserve(0, usize::MAX, None, None).unwrap();
        assert_eq!(budget.task_reserved("a"), 2);
        assert_eq!(budget.task_reserved("b"), 1);
        assert_eq!(budget.task_reserved("missing"), 0);
        assert_eq!(
            budget
                .reserve(0, MAX_TASK_TABS - 2, Some(scope("a", 3)), None)
                .unwrap_err()
                .code,
            "TASK_TAB_LIMIT"
        );
        assert_eq!(budget.len(), 4);
        assert!(budget.release(first));
        assert!(
            budget
                .reserve(0, MAX_TASK_TABS - 2, Some(scope("a", 3)), None)
                .is_ok()
        );
        assert_eq!(
            budget
                .reserve(0, usize::MAX, Some(scope("b", 1)), None)
                .unwrap_err()
                .code,
            "TASK_TAB_LIMIT"
        );
    }

    #[test]
    fn popup_abort_and_opener_close_release_only_matching_reservations() {
        let mut budget = ResourceBudget::default();
        let popup = budget
            .reserve(0, 0, Some(scope("a", 1)), Some((10, 1)))
            .unwrap();
        let sibling = budget
            .reserve(0, 0, Some(scope("a", 1)), Some((10, 2)))
            .unwrap();
        let other = budget
            .reserve(0, 0, Some(scope("b", 1)), Some((20, 1)))
            .unwrap();
        let direct = budget.reserve(0, 0, Some(scope("a", 1)), None).unwrap();
        assert_eq!(budget.abort_popup(10, 1), 1);
        assert!(!budget.contains(popup));
        assert_eq!(budget.abort_popup(10, 1), 0);
        assert!(budget.contains(sibling));
        assert_eq!(budget.close_opener(10), 1);
        assert!(!budget.contains(sibling));
        assert_eq!(budget.close_opener(10), 0);
        assert!(budget.contains(other));
        assert!(budget.contains(direct));
        assert_eq!(budget.task_reserved("a"), 1);
        assert_eq!(budget.task_reserved("b"), 1);
    }

    #[test]
    fn canceled_ticket_never_aliases_a_later_creation() {
        let mut budget = ResourceBudget::default();
        let stale = budget.reserve(0, 0, None, Some((10, 1))).unwrap();
        budget.abort_popup(10, 1);
        assert!(budget.is_empty());
        let current = budget.reserve(0, 0, None, Some((10, 1))).unwrap();
        assert!(current > stale);
        assert!(!budget.contains(stale));
        assert!(!budget.release(stale));
        assert!(budget.contains(current));
        assert_eq!(budget.len(), 1);
    }

    #[test]
    fn exhausted_ticket_counter_does_not_wrap_or_change_reservations() {
        let mut budget = ResourceBudget {
            last_ticket: u64::MAX,
            ..Default::default()
        };
        assert_eq!(
            budget.reserve(0, 0, None, None).unwrap_err().code,
            "RESERVATION_LIMIT"
        );
        assert!(budget.is_empty());
    }
}
