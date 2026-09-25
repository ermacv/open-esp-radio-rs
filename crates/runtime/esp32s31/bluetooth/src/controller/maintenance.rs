//! Automatic maintenance timing, independent from LL admission and RF ownership.
//!
//! A copied PHY due time only arms wakes. The physical owner re-observes demand
//! before execution. Deferral does not acknowledge tracking. Active DTM has no
//! handoff edge: a hard deadline is a terminal observation, even while waiting
//! for a Host command. All limits are explicit caller configuration.

use oer_esp32s31_bluetooth_controller::le::peripheral::maintenance::PeripheralMaintenanceBudget;

/// Caller-selected bounds; these values need measured hardware qualification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyMaintenancePolicy {
    budget: PeripheralMaintenanceBudget,
    force_after_micros: u64,
    hard_after_micros: u64,
}
impl PhyMaintenancePolicy {
    /// Delays are measured from the PHY's original due time, never from a retry.
    /// There must be room for a complete transaction before the hard deadline.
    pub const fn new(
        budget: PeripheralMaintenanceBudget,
        force_after_micros: u64,
        hard_after_micros: u64,
    ) -> Option<Self> {
        let duration =
            budget.execution_micros().get() as u64 + budget.restoration_micros().get() as u64;
        if force_after_micros >= hard_after_micros
            || duration >= hard_after_micros - force_after_micros
        {
            return None;
        }
        Some(Self {
            budget,
            force_after_micros,
            hard_after_micros,
        })
    }
    pub const fn budget(self) -> PeripheralMaintenanceBudget {
        self.budget
    }
}

/// Terminal maintenance timing or ownership rejection, without a runnable fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMaintenanceError {
    NotIdle,
    PolicyAlreadyConfigured,
    OwnerUnavailable,
    Clock,
    TimelineOverflow,
    HardDeadline,
    LowerAdmission,
    Restoration,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug)]
pub(super) struct Schedule {
    policy: PhyMaintenancePolicy,
    due: Option<u64>,
    previous: u64,
}
#[cfg(any(target_arch = "riscv32", test))]
impl Schedule {
    fn new(
        policy: PhyMaintenancePolicy,
        due: Option<u64>,
        now: u64,
    ) -> Result<Self, PhyMaintenanceError> {
        if let Some(due) = due {
            due.checked_add(policy.hard_after_micros)
                .ok_or(PhyMaintenanceError::TimelineOverflow)?;
        }
        let state = Self {
            policy,
            due,
            previous: now,
        };
        state.check(now)?;
        Ok(state)
    }
    fn check(self, now: u64) -> Result<(), PhyMaintenanceError> {
        if now < self.previous {
            return Err(PhyMaintenanceError::Clock);
        }
        if self.hard().is_some_and(|hard| now >= hard) {
            return Err(PhyMaintenanceError::HardDeadline);
        }
        Ok(())
    }
    fn reuse_policy(
        self,
        policy: PhyMaintenancePolicy,
        now: u64,
    ) -> Result<(), PhyMaintenanceError> {
        self.check(now)?;
        if self.policy != policy {
            return Err(PhyMaintenanceError::PolicyAlreadyConfigured);
        }
        Ok(())
    }
    fn sample(&mut self, now: u64) -> Result<(), PhyMaintenanceError> {
        self.check(now)?;
        self.previous = now;
        Ok(())
    }
    fn hard(self) -> Option<u64> {
        self.due.map(|due| due + self.policy.hard_after_micros)
    }
    fn due(self, now: u64) -> bool {
        self.due.is_some_and(|due| now >= due)
    }
    fn allow_skip(self, now: u64) -> bool {
        self.due
            .is_some_and(|due| now >= due + self.policy.force_after_micros)
    }
    fn wake(self, now: u64) -> Option<u64> {
        self.due
            .map(|due| if now < due { due } else { self.hard().unwrap() })
    }
    fn fits(self, now: u64) -> bool {
        now.checked_add(
            u64::from(self.policy.budget.execution_micros().get())
                + u64::from(self.policy.budget.restoration_micros().get()),
        )
        .zip(self.hard())
        .is_some_and(|(end, hard)| end < hard)
    }
}

#[cfg(target_arch = "riscv32")]
mod actor;
#[cfg(target_arch = "riscv32")]
pub use actor::{ControllerMaintenanceContinuation, ControllerMaintenanceResumeFailure};
#[cfg(target_arch = "riscv32")]
pub(super) use oer_esp32s31_bluetooth_controller::le::peripheral::{
    PeripheralPhyMaintenanceFailure as Failure, PeripheralPhyMaintenancePending as Pending,
    PeripheralPhyMaintenanceReady as Ready,
};

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU32;
    fn policy() -> PhyMaintenancePolicy {
        let n = |v| NonZeroU32::new(v).unwrap();
        PhyMaintenancePolicy::new(
            PeripheralMaintenanceBudget::new(n(10), n(5), n(2)).unwrap(),
            20,
            100,
        )
        .unwrap()
    }
    #[test]
    fn deferral_uses_original_due_time_even_after_many_retries() {
        let mut schedule = Schedule::new(policy(), Some(100), 0).unwrap();
        assert_eq!(schedule.wake(99), Some(100));
        assert!(!schedule.due(99));
        schedule.sample(100).unwrap();
        assert!(schedule.due(100));
        assert!(!schedule.allow_skip(119));
        assert!(schedule.allow_skip(120));
        for now in 120..200 {
            schedule.sample(now).unwrap();
        }
        assert_eq!(schedule.wake(199), Some(200));
        assert_eq!(schedule.sample(200), Err(PhyMaintenanceError::HardDeadline));
    }
    #[test]
    fn dtm_deferral_expires_without_an_hci_command_or_acknowledging_demand() {
        let mut schedule = Schedule::new(policy(), Some(100), 100).unwrap();
        // A non-preemptible role does nothing with due work. Its wake is still
        // the independent hard deadline, not an unbounded wait for Test End.
        for now in [110, 150, 199] {
            schedule.sample(now).unwrap();
            assert!(schedule.due(now));
            assert_eq!(schedule.wake(now), Some(200));
        }
        assert_eq!(schedule.check(200), Err(PhyMaintenanceError::HardDeadline));
    }
    #[test]
    fn execution_and_restoration_must_both_fit_before_hard_expiry() {
        let schedule = Schedule::new(policy(), Some(100), 0).unwrap();
        assert!(schedule.fits(184));
        assert!(!schedule.fits(185));
        assert!(!schedule.fits(u64::MAX));
    }
    #[test]
    fn clocks_and_unrepresentable_deadlines_fail_closed() {
        assert!(matches!(
            Schedule::new(policy(), Some(u64::MAX - 50), 0),
            Err(PhyMaintenanceError::TimelineOverflow)
        ));
        let mut schedule = Schedule::new(policy(), Some(100), 10).unwrap();
        schedule.sample(120).unwrap();
        assert_eq!(schedule.sample(119), Err(PhyMaintenanceError::Clock));
    }
    #[test]
    fn reentry_cannot_extend_the_same_epochs_hard_deadline() {
        let policy = policy();
        let schedule = Schedule::new(policy, Some(100), 100).unwrap();
        schedule.reuse_policy(policy, 199).unwrap();
        let extended = PhyMaintenancePolicy::new(policy.budget, 20, 200).unwrap();
        assert_eq!(
            schedule.reuse_policy(extended, 199),
            Err(PhyMaintenanceError::PolicyAlreadyConfigured)
        );
        assert_eq!(
            schedule.reuse_policy(policy, 200),
            Err(PhyMaintenanceError::HardDeadline)
        );
    }

    #[test]
    fn successful_owner_observation_reanchors_only_to_the_new_phy_due_time() {
        let old = Schedule::new(policy(), Some(100), 100).unwrap();
        let next = Schedule::new(old.policy, Some(1000), 150).unwrap();
        assert!(!next.due(151));
        assert_eq!(next.wake(151), Some(1000));
        let inactive = Schedule::new(old.policy, None, 150).unwrap();
        assert_eq!(inactive.wake(2000), None);
        assert!(!inactive.due(2000));
    }
}
