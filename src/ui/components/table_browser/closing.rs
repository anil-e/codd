use std::collections::BTreeMap;

use crate::models::table_browser::TableCell;

pub(super) struct DiscardedEdits {
    pub pending_edits: BTreeMap<(usize, usize), TableCell>,
    pub error: Option<String>,
    pub commit_unknown: bool,
}

enum ClosePhase {
    AwaitingResponse,
    Saving,
    Prepared,
    DiscardPrepared,
    Saved,
    Committed,
    DiscardCommitted(DiscardedEdits),
    SavedCommitted,
}

pub(super) struct CloseSession {
    pub id: u64,
    phase: ClosePhase,
    interrupted_read: bool,
}

pub(super) struct CancelledClose {
    pub reload: bool,
    backup: Option<DiscardedEdits>,
}

impl CancelledClose {
    pub fn restore(
        self,
        pending: &mut BTreeMap<(usize, usize), TableCell>,
        error: &mut Option<String>,
        commit_unknown: &mut bool,
    ) {
        if let Some(backup) = self.backup {
            *pending = backup.pending_edits;
            *error = backup.error;
            *commit_unknown = backup.commit_unknown;
        }
    }
}

impl CloseSession {
    pub fn new(id: u64, interrupted_read: bool, pending: bool) -> Self {
        Self {
            id,
            phase: if pending {
                ClosePhase::AwaitingResponse
            } else {
                ClosePhase::Prepared
            },
            interrupted_read,
        }
    }

    pub fn accepts_response(&self, id: u64) -> bool {
        self.id == id && matches!(self.phase, ClosePhase::AwaitingResponse)
    }

    pub fn prepare_discard(&mut self, id: u64) -> bool {
        if !self.accepts_response(id) {
            return false;
        }

        self.phase = ClosePhase::DiscardPrepared;

        true
    }

    pub fn saved(&mut self) {
        self.phase = ClosePhase::Saved;
    }

    pub fn saving(&mut self) {
        self.phase = ClosePhase::Saving;
    }

    pub fn prepared(&self, id: u64) -> bool {
        self.id == id
            && !matches!(
                self.phase,
                ClosePhase::AwaitingResponse | ClosePhase::Saving
            )
    }

    pub fn needs_discard(&self) -> bool {
        matches!(self.phase, ClosePhase::DiscardPrepared)
    }

    pub fn commit(&mut self, id: u64, backup: Option<DiscardedEdits>) -> bool {
        if !self.prepared(id) {
            return false;
        }

        self.phase = match self.phase {
            ClosePhase::DiscardPrepared => {
                let Some(backup) = backup else {
                    return false;
                };

                ClosePhase::DiscardCommitted(backup)
            }
            ClosePhase::Prepared => ClosePhase::Committed,
            ClosePhase::Saved => ClosePhase::SavedCommitted,
            _ => return true,
        };

        true
    }

    fn reload_on_cancel(&self) -> bool {
        self.interrupted_read
            || matches!(self.phase, ClosePhase::Saved | ClosePhase::SavedCommitted)
    }

    pub fn cancel(session: &mut Option<Self>, id: u64) -> Option<CancelledClose> {
        let close = session.take_if(|close| close.id == id)?;
        let reload = close.reload_on_cancel();
        let backup = match close.phase {
            ClosePhase::DiscardCommitted(backup) => Some(backup),
            _ => None,
        };

        Some(CancelledClose { reload, backup })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_responses_commits_and_cancellations_leave_session_untouched() {
        let mut session = Some(CloseSession::new(12, false, true));
        let close = session.as_mut().unwrap();
        assert!(!close.prepare_discard(11));
        assert!(!close.commit(11, None));
        assert!(!close.commit(12, None));
        assert!(CloseSession::cancel(&mut session, 11).is_none());
        assert!(session.as_ref().unwrap().accepts_response(12));
        assert!(session.as_mut().unwrap().prepare_discard(12));
        assert!(!session.as_mut().unwrap().prepare_discard(12));
        assert!(!session.as_mut().unwrap().commit(12, None));
        assert!(CloseSession::cancel(&mut session, 12).is_some());
        assert!(CloseSession::cancel(&mut session, 12).is_none());
    }

    #[test]
    fn discard_commit_cancel_restores_exact_backup_even_after_repeated_commit() {
        let pending_edits = BTreeMap::from([((3, 2), TableCell::new("changed".into()))]);
        let mut session = Some(CloseSession::new(4, false, true));
        let close = session.as_mut().unwrap();
        assert!(close.prepare_discard(4));
        assert!(close.needs_discard());
        assert!(close.commit(
            4,
            Some(DiscardedEdits {
                pending_edits: pending_edits.clone(),
                error: Some("uncertain commit".into()),
                commit_unknown: true,
            })
        ));
        assert!(!close.needs_discard());
        assert!(close.commit(4, None));
        assert!(!close.commit(3, None));

        let cancelled = CloseSession::cancel(&mut session, 4).unwrap();
        assert!(!cancelled.reload);
        let mut restored = BTreeMap::new();
        let mut error = None;
        let mut commit_unknown = false;
        cancelled.restore(&mut restored, &mut error, &mut commit_unknown);
        assert_eq!(restored, pending_edits);
        assert_eq!(error.as_deref(), Some("uncertain commit"));
        assert!(commit_unknown);
    }

    #[test]
    fn successful_save_cancellation_reloads_before_and_after_commit() {
        for commit in [false, true] {
            let mut session = Some(CloseSession::new(7, false, true));
            let close = session.as_mut().unwrap();
            close.saving();
            assert!(!close.accepts_response(7));
            assert!(!close.prepared(7));
            close.saved();
            assert!(close.prepared(7));
            if commit {
                assert!(close.commit(7, None));
            }
            let cancelled = CloseSession::cancel(&mut session, 7).unwrap();
            assert!(cancelled.reload);
            assert!(cancelled.backup.is_none());
        }
    }

    #[test]
    fn failed_or_in_flight_save_cancellation_has_no_discard_or_reload() {
        let mut session = Some(CloseSession::new(9, false, true));
        session.as_mut().unwrap().saving();
        let cancelled = CloseSession::cancel(&mut session, 9).unwrap();
        assert!(cancelled.backup.is_none());
        assert!(!cancelled.reload);
        let original = BTreeMap::from([((1, 0), TableCell::new("pending".into()))]);
        let mut pending = original.clone();
        let mut error = Some("save failed".into());
        let mut commit_unknown = true;
        cancelled.restore(&mut pending, &mut error, &mut commit_unknown);
        assert_eq!(pending, original);
        assert_eq!(error.as_deref(), Some("save failed"));
        assert!(commit_unknown);
        assert!(session.is_none());
    }

    #[test]
    fn interrupted_read_reloads_on_cancel_with_or_without_preparation() {
        for pending in [false, true] {
            let mut session = Some(CloseSession::new(2, true, pending));
            assert!(CloseSession::cancel(&mut session, 2).unwrap().reload);
        }
    }
}
