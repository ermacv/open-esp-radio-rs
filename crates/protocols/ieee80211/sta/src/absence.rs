//! AP notification for a bounded, externally owned station absence.
//!
//! This exchange does not grant doze or physical radio access. The caller
//! closes data admission, drives each Null through terminal TX, and separately
//! owns local MAC/DMA quiescence. Even failed PM=1 needs PM=0 recovery: a lost
//! ACK does not prove that the AP did not accept the advertisement.
use oer_ieee80211::station_power_save::StaPowerManagement;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Transmit(StaPowerManagement),
    Absent,
    Restored { admitted: bool },
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Enter,
    EnterPending,
    Absent,
    Leave,
    LeavePending,
    Restored,
    Failed,
}

/// One affine PM exchange. Completion is accepted only for its pending frame.
pub struct Exchange {
    phase: Phase,
    admitted: bool,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}
impl Exchange {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Enter,
            admitted: false,
        }
    }
    pub const fn awaiting_completion(&self) -> bool {
        matches!(self.phase, Phase::EnterPending | Phase::LeavePending)
    }
    pub const fn absent(&self) -> bool {
        matches!(self.phase, Phase::Absent)
    }
    pub fn restore(&mut self) -> bool {
        if !self.absent() {
            return false;
        }
        self.phase = Phase::Leave;
        true
    }
    /// `Some` is the terminal outcome of the sole pending Null transaction.
    pub fn advance(&mut self, acknowledged: Option<bool>) -> Action {
        if self.awaiting_completion() != acknowledged.is_some() {
            self.phase = Phase::Failed;
        } else if let Some(acknowledged) = acknowledged {
            self.phase = match self.phase {
                Phase::EnterPending if acknowledged => {
                    self.admitted = true;
                    Phase::Absent
                }
                Phase::EnterPending => Phase::Leave,
                Phase::LeavePending if acknowledged => Phase::Restored,
                _ => Phase::Failed,
            };
        }
        match self.phase {
            Phase::Enter => {
                self.phase = Phase::EnterPending;
                Action::Transmit(StaPowerManagement::PowerSave)
            }
            Phase::Leave => {
                self.phase = Phase::LeavePending;
                Action::Transmit(StaPowerManagement::Active)
            }
            Phase::Absent => Action::Absent,
            Phase::Restored => Action::Restored {
                admitted: self.admitted,
            },
            _ => Action::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_acknowledged_entry_admits_absence_and_return_needs_its_own_ack() {
        let mut exchange = Exchange::new();
        assert!(!exchange.absent());
        assert_eq!(
            exchange.advance(None),
            Action::Transmit(StaPowerManagement::PowerSave)
        );
        assert!(!exchange.restore());
        assert_eq!(exchange.advance(Some(true)), Action::Absent);
        assert!(exchange.restore());
        assert_eq!(
            exchange.advance(None),
            Action::Transmit(StaPowerManagement::Active)
        );
        assert_eq!(
            exchange.advance(Some(true)),
            Action::Restored { admitted: true }
        );
    }
    #[test]
    fn lost_entry_ack_still_requires_confirmed_active_recovery() {
        let mut exchange = Exchange::new();
        exchange.advance(None);
        assert_eq!(
            exchange.advance(Some(false)),
            Action::Transmit(StaPowerManagement::Active)
        );
        assert!(!exchange.absent());
        assert_eq!(
            exchange.advance(Some(true)),
            Action::Restored { admitted: false }
        );
    }
    #[test]
    fn failed_active_restore_never_releases_the_exchange() {
        for entry_ack in [false, true] {
            let mut exchange = Exchange::new();
            exchange.advance(None);
            exchange.advance(Some(entry_ack));
            if entry_ack {
                assert!(exchange.restore());
                exchange.advance(None);
            }
            assert_eq!(exchange.advance(Some(false)), Action::Failed);
            assert_eq!(exchange.advance(None), Action::Failed);
        }
    }
    #[test]
    fn unmatched_or_missing_completion_fails_closed() {
        let mut exchange = Exchange::new();
        assert_eq!(exchange.advance(Some(true)), Action::Failed);
        let mut exchange = Exchange::new();
        exchange.advance(None);
        assert_eq!(exchange.advance(None), Action::Failed);
    }
}
