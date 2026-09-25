//! Actor handoff leaves no protocol owner in an await-local temporary.

use super::*;
use crate::controller::{
    ControllerCommandBoundary as Boundary, ControllerCommandState as State,
    ControllerCommandTask as Actor, SchedulerRunInterruptStorage,
};
use oer_esp32s31_bluetooth::{
    controller::ControllerIdleCommandTask as Idle,
    le::peripheral::{
        PeripheralPhyMaintenanceRestoring as Restoring, PeripheralPhyMaintenanceStep as Step,
    },
};
use oer_esp32s31_phy::tracking::schedule::Schedule as PhySchedule;

fn due(schedule: PhySchedule) -> Option<u64> {
    match schedule {
        PhySchedule::Inactive => None,
        PhySchedule::At(at) => Some(at),
        PhySchedule::Due(demand) => Some(demand.due_since_micros()),
    }
}

/// Suspended command actor, including diagnostics and its original deferral policy.
/// Only a restored lower owner can make it runnable again.
#[must_use = "return the same maintained authority to its suspended actor"]
pub struct ControllerMaintenanceContinuation<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    maintenance: Option<Schedule>,
    advertising_rejected_packets: Option<u32>,
    advertising_last_receive_rejection: Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )>,
    advertising_completion: Option<
        oer_esp32s31_bluetooth::memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    >,
    epoch: core::marker::PhantomData<fn() -> Actor<'a, S, N>>,
}

/// Failed restoration seals both the command actor and the exact lower owner.
#[must_use = "retain until external reset"]
pub struct ControllerMaintenanceResumeFailure<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    error: PhyMaintenanceError,
    _continuation: ControllerMaintenanceContinuation<'a, S, N>,
    _owner: ResumeOwner<'a, S, N>,
}
#[allow(dead_code, reason = "failed affine owners are intentionally retained")]
enum ResumeOwner<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Idle(Idle<'a, S, N>),
    Restoring(Restoring<'a, S, N>),
    Failed(Failure<'a, S, N>),
}
impl<S: SchedulerRunInterruptStorage, const N: usize> ControllerMaintenanceResumeFailure<'_, S, N> {
    pub const fn error(&self) -> PhyMaintenanceError {
        self.error
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize> Actor<'a, S, N> {
    /// Configure automatic demand handling before starting the idle actor.
    /// Failure leaves the exact actor unchanged; no default limits are assumed.
    /// Re-entry retains the original policy and due time; changing limits or
    /// reviving an expired epoch requires a fresh physical epoch.
    pub fn enable_phy_maintenance(
        &mut self,
        policy: PhyMaintenancePolicy,
    ) -> Result<(), PhyMaintenanceError> {
        if self.owner.is_empty() {
            return Err(PhyMaintenanceError::OwnerUnavailable);
        }
        let State::Idle(idle) = self.owner.current() else {
            return Err(PhyMaintenanceError::NotIdle);
        };
        let now = S::monotonic_micros();
        if let Some(schedule) = self.maintenance {
            return schedule.reuse_policy(policy, now);
        }
        let observation = idle
            .phy_tracking_schedule_at(now)
            .map_err(|_| PhyMaintenanceError::Clock)?;
        self.maintenance = Some(Schedule::new(policy, due(observation), now)?);
        Ok(())
    }

    /// Absolute due/hard wake in the same monotonic microseconds as the driver.
    /// Must be polled alongside command and timer service, including in DTM.
    pub fn phy_maintenance_wake_at(&self) -> Option<u64> {
        self.maintenance
            .and_then(|schedule| schedule.wake(S::monotonic_micros()))
    }

    /// The original hard deadline also bounds terminal quarantine with RF owners.
    pub fn phy_maintenance_hard_deadline(&self) -> Option<u64> {
        self.maintenance.and_then(Schedule::hard)
    }

    /// Check even while command processing is awaiting a Host or a retry gate.
    pub fn phy_maintenance_error(&self) -> Option<PhyMaintenanceError> {
        self.maintenance
            .and_then(|schedule| schedule.check(S::monotonic_micros()).err())
    }

    pub(crate) fn step_phy_maintenance<'epoch, 'packet>(
        &mut self,
    ) -> Option<Boundary<'a, 'epoch, 'packet, S, N>> {
        let schedule = self.maintenance.as_mut()?;
        let now = S::monotonic_micros();
        if let Err(error) = schedule.sample(now) {
            return Some(Boundary::PhyMaintenanceFailed(error));
        }
        let schedule = *schedule;
        match self.owner.current() {
            State::PeripheralMaintenanceReady(_) => {
                return Some(Boundary::PhyMaintenancePeripheral);
            }
            State::PeripheralMaintenanceFailed(_) => {
                return Some(Boundary::PhyMaintenanceFailed(
                    PhyMaintenanceError::LowerAdmission,
                ));
            }
            State::Idle(_) if schedule.due(now) && schedule.fits(now) => {
                return Some(Boundary::PhyMaintenanceIdle);
            }
            State::PeripheralMaintenancePending(_) => {}
            State::PeripheralConnectionActive(_) if schedule.due(now) && schedule.fits(now) => {}
            _ => return None,
        }
        let step = match self.owner.take() {
            State::PeripheralMaintenancePending(pending) => pending.recheck(),
            State::PeripheralConnectionActive(session) => {
                session.begin_phy_maintenance(schedule.policy.budget, schedule.allow_skip(now))
            }
            _ => unreachable!("maintenance only consumes the selected peripheral owner"),
        };
        match step {
            Step::Waiting(pending) => {
                self.owner
                    .store(State::PeripheralMaintenancePending(pending));
            }
            Step::Deferred { session, .. } => {
                self.owner.store(State::PeripheralConnectionActive(session));
            }
            Step::Ready(ready) => {
                if schedule
                    .hard()
                    .is_some_and(|hard| ready.restoration_deadline().expires_at_micros() >= hard)
                {
                    // Acquisition consumed the remaining deferral margin. Before
                    // hardware work cancellation is lossless; continue until the
                    // original hard wake instead of renewing either deadline.
                    match ready.cancel() {
                        Step::Deferred { session, .. } => {
                            self.owner.store(State::PeripheralConnectionActive(session))
                        }
                        Step::Failed(failure) => self
                            .owner
                            .store(State::PeripheralMaintenanceFailed(failure)),
                        _ => unreachable!("cancellation restores or seals the original successor"),
                    }
                } else {
                    self.owner.store(State::PeripheralMaintenanceReady(ready));
                    return Some(Boundary::PhyMaintenancePeripheral);
                }
            }
            Step::Failed(failure) => {
                self.owner
                    .store(State::PeripheralMaintenanceFailed(failure));
                return Some(Boundary::PhyMaintenanceFailed(
                    PhyMaintenanceError::LowerAdmission,
                ));
            }
        }
        None
    }

    /// Exact lower admission failure retained in the actor, if any.
    pub fn phy_maintenance_admission_error(
        &self,
    ) -> Option<oer_esp32s31_bluetooth::le::peripheral::PeripheralPhyMaintenanceError> {
        match self.owner.current() {
            State::PeripheralMaintenanceFailed(failure) => Some(failure.error()),
            _ => None,
        }
    }

    /// Whether the active authority is ready for the physical ownership join.
    pub fn peripheral_phy_maintenance_ready(&self) -> bool {
        matches!(self.owner.current(), State::PeripheralMaintenanceReady(_))
    }

    /// Admit idle executor time without renewing the independent hard deadline.
    pub fn phy_maintenance_idle_deadline(
        &self,
    ) -> Option<oer_esp32s31_phy::tracking::deadline::TrackingDeadline> {
        let schedule = self.maintenance?;
        let now = S::monotonic_micros();
        if !matches!(self.owner.current(), State::Idle(_))
            || !schedule.due(now)
            || !schedule.fits(now)
            || schedule.check(now).is_err()
        {
            return None;
        }
        oer_esp32s31_phy::tracking::deadline::TrackingDeadline::new(
            now,
            core::num::NonZeroU64::new(u64::from(schedule.policy.budget.execution_micros().get()))
                .unwrap(),
        )
    }

    /// Consume the actor only at the admitted active maintenance boundary.
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains the unchanged actor"
    )]
    #[inline(never)]
    pub fn take_peripheral_phy_maintenance(
        mut self,
    ) -> Result<
        (ControllerMaintenanceContinuation<'a, S, N>, Ready<'a, S, N>),
        (PhyMaintenanceError, Self),
    > {
        if let Some(error) = self.phy_maintenance_error() {
            return Err((error, self));
        }
        match self.owner.try_transfer(|state| match state {
            State::PeripheralMaintenanceReady(ready) => Ok(ready),
            state => Err((PhyMaintenanceError::OwnerUnavailable, state)),
        }) {
            Some(Ok(ready)) => Ok((ControllerMaintenanceContinuation::retain(self), ready)),
            _ => Err((PhyMaintenanceError::OwnerUnavailable, self)),
        }
    }

    /// Preserve the same actor when idle physical maintenance consumes its task.
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains the unchanged actor"
    )]
    #[inline(never)]
    pub fn take_idle_phy_maintenance(
        mut self,
    ) -> Result<
        (ControllerMaintenanceContinuation<'a, S, N>, Idle<'a, S, N>),
        (PhyMaintenanceError, Self),
    > {
        if let Some(error) = self.phy_maintenance_error() {
            return Err((error, self));
        }
        match self.owner.try_transfer(|state| match state {
            State::Idle(idle) => Ok(idle),
            state => Err((PhyMaintenanceError::NotIdle, state)),
        }) {
            Some(Ok(idle)) => Ok((ControllerMaintenanceContinuation::retain(self), idle)),
            _ => Err((PhyMaintenanceError::NotIdle, self)),
        }
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    ControllerMaintenanceContinuation<'a, S, N>
{
    fn retain(actor: Actor<'a, S, N>) -> Self {
        let Actor {
            owner,
            maintenance,
            advertising_rejected_packets,
            advertising_last_receive_rejection,
            advertising_completion,
        } = actor;
        assert!(
            owner.is_empty(),
            "the lower authority has transferred to maintenance"
        );
        Self {
            maintenance,
            advertising_rejected_packets,
            advertising_last_receive_rejection,
            advertising_completion,
            epoch: core::marker::PhantomData,
        }
    }
    fn restore(self, state: State<'a, S, N>) -> Actor<'a, S, N> {
        Actor {
            owner: crate::controller::owner::ControllerOwnerSlot::new(state),
            maintenance: self.maintenance,
            advertising_rejected_packets: self.advertising_rejected_packets,
            advertising_last_receive_rejection: self.advertising_last_receive_rejection,
            advertising_completion: self.advertising_completion,
        }
    }
    fn refresh(&mut self, observation: PhySchedule, now: u64) -> Result<(), PhyMaintenanceError> {
        if let Some(old) = self.maintenance {
            old.check(now)?;
            self.maintenance = Some(Schedule::new(old.policy, due(observation), now)?);
        }
        Ok(())
    }
    /// Restore the original actor after idle maintenance; diagnostics are preserved.
    #[allow(
        clippy::result_large_err,
        reason = "failed restoration retains all owners"
    )]
    pub fn resume_idle(
        mut self,
        idle: Idle<'a, S, N>,
    ) -> Result<Actor<'a, S, N>, ControllerMaintenanceResumeFailure<'a, S, N>> {
        let now = S::monotonic_micros();
        let result = idle
            .phy_tracking_schedule_at(now)
            .map_err(|_| PhyMaintenanceError::Clock)
            .and_then(|observation| self.refresh(observation, now));
        if let Err(error) = result {
            return Err(ControllerMaintenanceResumeFailure {
                error,
                _continuation: self,
                _owner: ResumeOwner::Idle(idle),
            });
        }
        Ok(self.restore(State::Idle(idle)))
    }
    /// Restore the same ACL actor, preserving the driver's absolute RUN deadline.
    #[allow(
        clippy::result_large_err,
        reason = "failed restoration retains all owners"
    )]
    pub fn resume_peripheral(
        mut self,
        restoring: Restoring<'a, S, N>,
    ) -> Result<Actor<'a, S, N>, ControllerMaintenanceResumeFailure<'a, S, N>> {
        let now = S::monotonic_micros();
        let result = restoring
            .phy_tracking_schedule_at(now)
            .map_err(|_| PhyMaintenanceError::Clock)
            .and_then(|observation| self.refresh(observation, now));
        if let Err(error) = result {
            return Err(ControllerMaintenanceResumeFailure {
                error,
                _continuation: self,
                _owner: ResumeOwner::Restoring(restoring),
            });
        }
        match restoring.into_session() {
            Ok(session) => Ok(self.restore(State::PeripheralConnectionActive(session))),
            Err(failure) => Err(ControllerMaintenanceResumeFailure {
                error: PhyMaintenanceError::Restoration,
                _continuation: self,
                _owner: ResumeOwner::Failed(failure),
            }),
        }
    }
}
