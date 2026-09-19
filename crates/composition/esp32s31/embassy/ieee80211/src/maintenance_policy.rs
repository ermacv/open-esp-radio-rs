//! Maintenance failure disposition, independent of the system stop mechanism.

use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

/// Boundary that rejected or terminated a connected maintenance request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseError {
    Unavailable,
    Busy,
    Interrupted,
    MacStop,
    RxBusy,
    RxPause,
    IrqPause,
    RxResume,
    IrqResume,
    RegisterReclaim,
    PhyAdmission,
    PhyRelease,
    RegisterRepublish,
    PhyTracking,
    MacRestoration,
    ReceivePolicyChanged,
    PeerNotification,
    InvalidDuration,
}

impl PauseError {
    /// Apply only to a retained terminal checkpoint. A request rejection does
    /// not invalidate the PHY. Reclaim/IRQ-pause/republish faults retain their
    /// already-stopped MAC and paused RX frontier without restarting it.
    pub(crate) const fn shared_phy_failure(self) -> Option<SharedPhyFailStop> {
        match self {
            Self::MacStop
            | Self::RxPause
            | Self::RxResume
            | Self::IrqResume
            | Self::PhyRelease
            | Self::PhyTracking
            | Self::MacRestoration
            | Self::ReceivePolicyChanged => Some(SharedPhyFailStop::MaintenanceFailed),
            Self::Unavailable
            | Self::Busy
            | Self::Interrupted
            | Self::RxBusy
            | Self::IrqPause
            | Self::RegisterReclaim
            | Self::PhyAdmission
            | Self::RegisterRepublish
            | Self::PeerNotification
            | Self::InvalidDuration => None,
        }
    }
}

/// Semantic state of the stopped-role maintenance failure, after the concrete
/// owner has been retained. No MMIO values or runnable capabilities live here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoppedFailure {
    BeforePhy,
    QuiescedOwnership,
    StopUnconfirmed,
    PhyInvalid,
    RestorationUnconfirmed,
}

impl StoppedFailure {
    pub(crate) const fn shared_phy_failure(self) -> Option<SharedPhyFailStop> {
        match self {
            Self::BeforePhy | Self::QuiescedOwnership => None,
            Self::StopUnconfirmed | Self::PhyInvalid | Self::RestorationUnconfirmed => {
                Some(SharedPhyFailStop::MaintenanceFailed)
            }
        }
    }
}

/// Borrow the retained frontier during escalation. This neither moves a large
/// owner union nor drops it before the diverging platform operation.
#[cfg(any(test, target_arch = "riscv32"))]
fn enforce_with<E: ?Sized>(
    failure: &E,
    reason: Option<SharedPhyFailStop>,
    stop: impl FnOnce(SharedPhyFailStop, &E) -> core::convert::Infallible,
) {
    if let Some(reason) = reason {
        stop(reason, failure);
    }
}

/// Only the composition selects system reset. Radio drivers and the shared PHY
/// contract have no dependency on the SoC reset or watchdog mechanism.
#[cfg(target_arch = "riscv32")]
pub(crate) fn enforce<E: ?Sized>(failure: &E, reason: Option<SharedPhyFailStop>) {
    enforce_with(failure, reason, |reason, _retained| {
        let _reason = core::hint::black_box(reason);
        oer_esp32s31_soc::reset_system()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn rejected_requests_and_quiesced_ownership_faults_do_not_reset() {
        for error in [
            PauseError::Unavailable,
            PauseError::Busy,
            PauseError::Interrupted,
            PauseError::RxBusy,
            PauseError::IrqPause,
            PauseError::RegisterReclaim,
            PauseError::PhyAdmission,
            PauseError::RegisterRepublish,
            PauseError::PeerNotification,
            PauseError::InvalidDuration,
        ] {
            assert_eq!(error.shared_phy_failure(), None);
            enforce_with(&error, error.shared_phy_failure(), |_, _| {
                panic!("unexpected reset")
            });
        }
        for state in [StoppedFailure::BeforePhy, StoppedFailure::QuiescedOwnership] {
            assert_eq!(state.shared_phy_failure(), None);
        }
    }

    #[test]
    fn uncertain_stop_phy_and_restoration_share_the_terminal_reason() {
        for error in [
            PauseError::MacStop,
            PauseError::RxPause,
            PauseError::RxResume,
            PauseError::IrqResume,
            PauseError::PhyRelease,
            PauseError::PhyTracking,
            PauseError::MacRestoration,
            PauseError::ReceivePolicyChanged,
        ] {
            assert_eq!(
                error.shared_phy_failure(),
                Some(SharedPhyFailStop::MaintenanceFailed)
            );
        }
        for state in [
            StoppedFailure::StopUnconfirmed,
            StoppedFailure::PhyInvalid,
            StoppedFailure::RestorationUnconfirmed,
        ] {
            assert_eq!(
                state.shared_phy_failure(),
                Some(SharedPhyFailStop::MaintenanceFailed)
            );
        }
    }

    #[test]
    fn escalation_observes_the_original_owner_before_any_drop() {
        use core::cell::Cell;
        let dropped = Cell::new(false);
        struct Owner<'a>(&'a Cell<bool>);
        impl Drop for Owner<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let owner = Owner(&dropped);
        let reached = Cell::new(false);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enforce_with(
                &owner,
                PauseError::PhyTracking.shared_phy_failure(),
                |reason, retained| {
                    assert_eq!(reason, SharedPhyFailStop::MaintenanceFailed);
                    assert!(core::ptr::eq(retained, &owner));
                    assert!(!dropped.get());
                    reached.set(true);
                    panic!("stand-in for the diverging platform reset");
                },
            );
        }));
        assert!(outcome.is_err());
        assert!(reached.get());
        assert!(!dropped.get());
        // Host-only cleanup after catching the stand-in; hardware reset never returns.
        drop(owner);
        assert!(dropped.get());
    }
}
