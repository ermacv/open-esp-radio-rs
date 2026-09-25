//! Exclusive handoff at the unreserved successor, preserving the entire session.

use super::super::LegacyConnectablePeripheralFirstRunningEvidence as Evidence;
use super::super::maintenance::{
    PeripheralMaintenanceBlocked as Blocked, PeripheralMaintenanceBudget as Budget, Window,
};
use super::super::progress::PeripheralConnectionProgressDeadline as Progress;
use super::*;
use crate::controller as ctrl;
type Task<'a, S, const N: usize> = ctrl::ControllerPublishedTaskService<'a, S, N>;
type Candidate = crate::le::peripheral::PeripheralConnectionRecurringEventCandidate;

struct Suspended<'a> {
    state: PeripheralConnectionState<()>,
    order: Order<'a, ()>,
}
impl<'a> Suspended<'a> {
    fn resume<S: SchedulerRunInterruptStorage, const N: usize>(
        self,
        radio: radio::Radio<'a, S, N>,
    ) -> PeripheralConnectionActiveSession<'a, S, N> {
        self.state.map_radio(|()| radio).into_session(self.order)
    }
    fn deadlines(&self) -> super::super::deadlines::Deadlines {
        super::super::deadlines::Deadlines {
            supervision: self.state.supervision,
            termination: self.state.termination,
            procedure: self.state.procedure,
        }
    }
}

/// Fresh-time acquisition retains the exact candidate, HCI ordering and LL state.
/// Recheck to completion; dropping this owner requires external reset.
#[must_use = "drive the same acquisition or retain its sealed failure"]
pub struct PeripheralPhyMaintenancePending<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    pending: ctrl::ControllerSchedulerCurrentPending<'a, S, N>,
    candidate: Candidate,
    evidence: Evidence,
    session: Suspended<'a>,
    budget: Budget,
    allow_skip: bool,
    acquisition_started: u64,
    progress: Progress,
}

/// Protocol-admitted window with a CPU-owned graph. Physical admission must
/// still check the exact role allocations, scheduler, IRQs and PHY owner.
/// This owner exposes no runnable task or mutable connection half.
#[must_use = "cancel before PHY access or perform the consuming physical transaction"]
pub struct PeripheralPhyMaintenanceReady<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    pub(crate) task: Task<'a, S, N>,
    pub(crate) candidate: Candidate,
    evidence: Evidence,
    session: Suspended<'a>,
    pub(crate) window: Window,
}

/// Admission keeps all authority in exactly one successor or sealed failure.
#[must_use = "retain the complete maintenance or runnable session owner"]
pub enum PeripheralPhyMaintenanceStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Waiting(PeripheralPhyMaintenancePending<'a, S, N>),
    Deferred {
        reason: Blocked,
        session: PeripheralConnectionActiveSession<'a, S, N>,
    },
    Ready(PeripheralPhyMaintenanceReady<'a, S, N>),
    Failed(PeripheralPhyMaintenanceFailure<'a, S, N>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralPhyMaintenanceError {
    RestorationExpired,
    EpochUnavailable,
    TimeBegin(ctrl::ControllerSchedulerCurrentBeginError),
    Time(ctrl::ControllerSchedulerCurrentError),
    AcquisitionExpired,
    Candidate(crate::le::peripheral::PeripheralConnectionRecurringCandidateError),
    TimingUnavailable,
}

/// No failure returns a runnable connection or releases its graph allocation.
#[must_use = "retain failed hardware and HCI ownership until reset"]
pub struct PeripheralPhyMaintenanceFailure<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    error: PeripheralPhyMaintenanceError,
    _owner: FailedOwner<'a, S, N>,
    _session: Suspended<'a>,
}
#[allow(
    dead_code,
    reason = "sealed affine failure owners are intentionally retained"
)]
enum FailedOwner<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Task(Task<'a, S, N>, Candidate, Evidence),
    Pending(
        ctrl::ControllerSchedulerCurrentPending<'a, S, N>,
        Candidate,
        Evidence,
    ),
    Completed(
        Task<'a, S, N>,
        ctrl::PeripheralConnectionRecurringRetry,
        Evidence,
    ),
}
impl<S: SchedulerRunInterruptStorage, const N: usize> PeripheralPhyMaintenanceFailure<'_, S, N> {
    pub const fn error(&self) -> PeripheralPhyMaintenanceError {
        self.error
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionActiveSession<'a, S, N>
{
    /// Request one maintenance window at an unreserved successor. Always tries
    /// the natural gap first. `allow_skip` must come from the coordinator's
    /// force-eligible deferral policy; it never overrides LL or time admission.
    /// Observes the actual retained PHY's due schedule without acknowledging it;
    /// not-due work never acquires a window. This path cannot preempt DTM.
    #[inline(never)]
    pub fn begin_phy_maintenance(
        self,
        budget: Budget,
        allow_skip: bool,
    ) -> PeripheralPhyMaintenanceStep<'a, S, N> {
        let reason = match self.order.owner() {
            radio::Radio::Candidate {
                task,
                candidate,
                restoration,
                ..
            } => {
                if restoration.is_some() {
                    Some(Blocked::RestorationPending)
                } else if candidate.delta().skipped() != 0 {
                    Some(Blocked::RecoveryInProgress)
                } else {
                    match task.bluetooth_tracking_schedule_at(S::monotonic_micros()) {
                        Ok(oer_esp32s31_phy::tracking::schedule::Schedule::Due(_)) => None,
                        Ok(_) => Some(Blocked::NotDue),
                        Err(_) => Some(Blocked::Clock),
                    }
                }
            }
            _ => Some(Blocked::RadioPhase),
        };
        if let Some(reason) = reason {
            return PeripheralPhyMaintenanceStep::Deferred {
                reason,
                session: self,
            };
        }
        let Self {
            order,
            control,
            encryption,
            supervision,
            termination,
            procedure,
            progress_deadline,
            host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        let (radio, order) = order.into_parts();
        let radio::Radio::Candidate {
            task,
            candidate,
            evidence,
            ..
        } = radio
        else {
            unreachable!()
        };
        let session = Suspended {
            order,
            state: PeripheralConnectionState {
                radio: (),
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            },
        };
        let started = S::monotonic_micros();
        let retained = match task.retain_scheduler_epoch() {
            Ok(retained) => retained,
            Err(unavailable) => {
                return PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
                    error: PeripheralPhyMaintenanceError::EpochUnavailable,
                    _owner: FailedOwner::Task(unavailable.into_task_service(), candidate, evidence),
                    _session: session,
                });
            }
        };
        match retained.begin_fresh_scheduler_current() {
            Ok(pending) => PeripheralPhyMaintenanceStep::Waiting(PeripheralPhyMaintenancePending {
                pending,
                candidate,
                evidence,
                session,
                budget,
                allow_skip,
                acquisition_started: started,
                progress: Progress::for_operation(started),
            }),
            Err(failure) => {
                let (retained, error) = failure.into_parts();
                PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
                    error: PeripheralPhyMaintenanceError::TimeBegin(error),
                    _owner: FailedOwner::Task(retained.into_task_service(), candidate, evidence),
                    _session: session,
                })
            }
        }
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralPhyMaintenancePending<'a, S, N>
{
    /// One finite readiness check; the acquisition budget is never renewed.
    #[inline(never)]
    pub fn recheck(self) -> PeripheralPhyMaintenanceStep<'a, S, N> {
        let Self {
            pending,
            candidate,
            evidence,
            session,
            budget,
            allow_skip,
            acquisition_started,
            progress,
        } = self;
        if progress.expired(S::monotonic_micros()) {
            return PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
                error: PeripheralPhyMaintenanceError::AcquisitionExpired,
                _owner: FailedOwner::Pending(pending, candidate, evidence),
                _session: session,
            });
        }
        let mut now = match pending.recheck() {
            Ok(ctrl::ControllerSchedulerCurrentStep::Waiting(pending)) => {
                return PeripheralPhyMaintenanceStep::Waiting(Self {
                    pending,
                    candidate,
                    evidence,
                    session,
                    budget,
                    allow_skip,
                    acquisition_started,
                    progress,
                });
            }
            Ok(ctrl::ControllerSchedulerCurrentStep::Ready(now)) => now,
            Err(failure) => {
                let (retained, error) = failure.into_parts();
                return PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
                    error: PeripheralPhyMaintenanceError::Time(error),
                    _owner: FailedOwner::Task(retained.into_task_service(), candidate, evidence),
                    _session: session,
                });
            }
        };
        if progress.expired(S::monotonic_micros()) {
            return PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
                error: PeripheralPhyMaintenanceError::AcquisitionExpired,
                _owner: FailedOwner::Task(
                    now.into_retained_epoch().into_task_service(),
                    candidate,
                    evidence,
                ),
                _session: session,
            });
        }
        // Freeze the transaction origin before previewing successive anchors.
        // Planning consumes this budget; a later preview never renews it.
        let planning_started = S::monotonic_micros();
        let check = |now: &ctrl::ControllerSchedulerNowReady<'a, S, N>, candidate: &Candidate| {
            now.peripheral_maintenance_window(
                candidate,
                budget,
                acquisition_started,
                planning_started,
                session.deadlines(),
            )
        };
        match check(&now, &candidate) {
            Ok(window) => {
                return PeripheralPhyMaintenanceStep::Ready(PeripheralPhyMaintenanceReady {
                    task: now.into_retained_epoch().into_task_service(),
                    candidate,
                    evidence,
                    session,
                    window,
                });
            }
            Err(Blocked::WindowTooShort) if allow_skip => {}
            Err(reason) => {
                return deferred(
                    now.into_retained_epoch().into_task_service(),
                    candidate,
                    evidence,
                    session,
                    reason,
                );
            }
        }
        let mut candidate = candidate;
        loop {
            let skipped = candidate.delta().skipped() + 1;
            let task = now.maintenance_task_mut();
            let (completed, _) = task
                .cancel_peripheral_connection_recurring_candidate(candidate)
                .into_parts();
            // The finite counter domain and the original execution budget both
            // bound planning. No RF access or skip credit has been committed.
            let elapsed = S::monotonic_micros().checked_sub(planning_started);
            let Some(delta) =
                oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::from_skipped(
                    skipped,
                )
                .filter(|delta| delta.get() <= i16::MAX as u16)
                .filter(|_| {
                    elapsed
                        .is_some_and(|elapsed| elapsed < u64::from(budget.execution_micros().get()))
                })
            else {
                return restore_contiguous(
                    now.into_retained_epoch().into_task_service(),
                    completed,
                    evidence,
                    session,
                    Blocked::WindowTooShort,
                );
            };
            candidate = match task
                .prepare_peripheral_connection_maintenance_candidate(completed, delta)
            {
                ctrl::PeripheralConnectionRecurringCandidateStep::Prepared(candidate) => candidate,
                ctrl::PeripheralConnectionRecurringCandidateStep::Rejected {
                    error:
                        crate::le::peripheral::PeripheralConnectionRecurringCandidateError::Maintenance(
                            reason,
                        ),
                    retry,
                } => {
                    let (completed, _) = retry.into_parts();
                    return restore_contiguous(
                        now.into_retained_epoch().into_task_service(),
                        completed,
                        evidence,
                        session,
                        Blocked::LinkLayer(reason),
                    );
                }
                failure => {
                    return failed_candidate(
                        now.into_retained_epoch().into_task_service(),
                        failure,
                        evidence,
                        session,
                    );
                }
            };
            match check(&now, &candidate) {
                Ok(window) => {
                    return PeripheralPhyMaintenanceStep::Ready(PeripheralPhyMaintenanceReady {
                        task: now.into_retained_epoch().into_task_service(),
                        candidate,
                        evidence,
                        session,
                        window,
                    });
                }
                Err(Blocked::WindowTooShort) => continue,
                Err(reason) => {
                    let mut task = now.into_retained_epoch().into_task_service();
                    let (completed, _) = task
                        .cancel_peripheral_connection_recurring_candidate(candidate)
                        .into_parts();
                    return restore_contiguous(task, completed, evidence, session, reason);
                }
            }
        }
    }
}
fn deferred<'a, S: SchedulerRunInterruptStorage, const N: usize>(
    task: Task<'a, S, N>,
    candidate: Candidate,
    evidence: Evidence,
    session: Suspended<'a>,
    reason: Blocked,
) -> PeripheralPhyMaintenanceStep<'a, S, N> {
    PeripheralPhyMaintenanceStep::Deferred {
        reason,
        session: session.resume(radio::Radio::Candidate {
            task,
            candidate,
            evidence,
            restoration: None,
        }),
    }
}
fn restore_contiguous<'a, S: SchedulerRunInterruptStorage, const N: usize>(
    mut task: Task<'a, S, N>,
    completed: crate::le::peripheral::PeripheralConnectionSchedulerCompleted,
    evidence: Evidence,
    session: Suspended<'a>,
    reason: Blocked,
) -> PeripheralPhyMaintenanceStep<'a, S, N> {
    let result = task.prepare_peripheral_connection_recurring_candidate(
        completed,
        oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::new(1).unwrap(),
    );
    match result {
        ctrl::PeripheralConnectionRecurringCandidateStep::Prepared(candidate) => {
            deferred(task, candidate, evidence, session, reason)
        }
        failure => failed_candidate(task, failure, evidence, session),
    }
}
fn failed_candidate<'a, S: SchedulerRunInterruptStorage, const N: usize>(
    task: Task<'a, S, N>,
    failure: ctrl::PeripheralConnectionRecurringCandidateStep,
    evidence: Evidence,
    session: Suspended<'a>,
) -> PeripheralPhyMaintenanceStep<'a, S, N> {
    let (error, retry) = match failure {
        ctrl::PeripheralConnectionRecurringCandidateStep::SchedulerEpochUnavailable(retry) => {
            (PeripheralPhyMaintenanceError::EpochUnavailable, retry)
        }
        ctrl::PeripheralConnectionRecurringCandidateStep::TimingPolicyUnavailable(retry) => {
            (PeripheralPhyMaintenanceError::TimingUnavailable, retry)
        }
        ctrl::PeripheralConnectionRecurringCandidateStep::Rejected { error, retry } => {
            (PeripheralPhyMaintenanceError::Candidate(error), retry)
        }
        ctrl::PeripheralConnectionRecurringCandidateStep::Prepared(_) => unreachable!(),
    };
    PeripheralPhyMaintenanceStep::Failed(PeripheralPhyMaintenanceFailure {
        error,
        _owner: FailedOwner::Completed(task, retry, evidence),
        _session: session,
    })
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize> PeripheralPhyMaintenanceReady<'a, S, N> {
    pub const fn execution_deadline(
        &self,
    ) -> oer_esp32s31_phy::tracking::deadline::TrackingDeadline {
        self.window.execution
    }
    pub const fn restoration_deadline(
        &self,
    ) -> oer_esp32s31_phy::tracking::deadline::TrackingDeadline {
        self.window.restoration
    }
    /// Cancel before hardware access. No skip credit is spent by a preview.
    pub fn cancel(self) -> PeripheralPhyMaintenanceStep<'a, S, N> {
        let Self {
            mut task,
            candidate,
            evidence,
            session,
            ..
        } = self;
        let (completed, _) = task
            .cancel_peripheral_connection_recurring_candidate(candidate)
            .into_parts();
        restore_contiguous(task, completed, evidence, session, Blocked::Cancelled)
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    crate::controller::boot::maintenance::MaintenanceAuthority<'a, S, N>
    for PeripheralPhyMaintenanceReady<'a, S, N>
{
    fn task_mut(&mut self) -> &mut Task<'a, S, N> {
        &mut self.task
    }
    fn roles_ready(&self) -> Result<(), ctrl::ControllerRoleRetirementError> {
        self.task
            .peripheral_maintenance_roles_ready(&self.candidate)
    }
    fn accepts<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        &self,
        controller: &LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
    ) -> bool {
        self.session.order.accepts_endpoint(controller)
    }
}

/// PHY has returned, but the exact successor still must reach RUN before the
/// original restoration deadline. This state has no preview-cancellation edge.
#[must_use = "restore CPU IRQ routing and drive the original successor promptly"]
pub struct PeripheralPhyMaintenanceRestoring<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    ready: PeripheralPhyMaintenanceReady<'a, S, N>,
}
impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralPhyMaintenanceRestoring<'a, S, N>
{
    /// Observe the actual post-maintenance schedule without consuming restoration.
    pub fn phy_tracking_schedule_at(
        &self,
        now_micros: u64,
    ) -> Result<
        oer_esp32s31_phy::tracking::schedule::Schedule,
        oer_esp32s31_phy::state::client::PhyTrackTimeError,
    > {
        self.ready.task.bluetooth_tracking_schedule_at(now_micros)
    }

    /// Re-enter ordinary HCI servicing with a guarded successor. All subsequent
    /// acquisition, admission and publication steps retain this same deadline;
    /// a late successor cannot use ordinary multi-event recovery.
    #[allow(
        clippy::result_large_err,
        reason = "late restoration seals the complete session"
    )]
    pub fn into_session(
        self,
    ) -> Result<
        PeripheralConnectionActiveSession<'a, S, N>,
        PeripheralPhyMaintenanceFailure<'a, S, N>,
    > {
        let PeripheralPhyMaintenanceReady {
            task,
            candidate,
            evidence,
            session,
            window,
        } = self.ready;
        if window
            .restoration
            .check(Some(S::monotonic_micros()))
            .is_err()
        {
            return Err(PeripheralPhyMaintenanceFailure {
                error: PeripheralPhyMaintenanceError::RestorationExpired,
                _owner: FailedOwner::Task(task, candidate, evidence),
                _session: session,
            });
        }
        Ok(session.resume(radio::Radio::Candidate {
            task,
            candidate,
            evidence,
            restoration: Some(window.restoration),
        }))
    }
}

impl<
    'a,
    S: SchedulerRunInterruptStorage
        + crate::interrupt::InterruptOwnerRestartStorage
        + crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    const N: usize,
> PeripheralPhyMaintenanceReady<'a, S, N>
{
    /// Consume this protocol window and the actual unrouted IRQ/timer owners.
    /// The shared physical executor rechecks all hardware and role identities,
    /// lends the original platform/PHY, and atomically restores both ISR slots.
    /// `D` and `S` must use the same monotonic microsecond domain. Execution is
    /// bounded by the admitted deadline; restoration retains its separate bound.
    /// Active DTM cannot create this owner and rejects the other-role check.
    /// Cancellation after polling begins, including a failed hardware edge,
    /// never returns runnable authority; drive to completion or reset externally.
    #[allow(clippy::too_many_arguments, reason = "explicit affine physical join")]
    pub async fn maintain_phy<
        P,
        D: oer_esp32s31_phy::PhyAsyncDelay,
        O: oer_esp32s31_phy::PhyTargetObserver,
        M: RawMutex,
        const MT: usize,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        self,
        timer: crate::modem_timer::ControllerModemTimerRetired<'a, S, MT>,
        interrupt: oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
        platform: &mut crate::resources::platform_retirement::ControllerRuntimePlatform<'a, P>,
        controller: &mut LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        clock: &mut impl oer_esp32s31_phy::state::client::PhyPllTrackClock,
        observer: O,
    ) -> Result<
        ctrl::ControllerPhyMaintained<'a, S, N, MT, PeripheralPhyMaintenanceRestoring<'a, S, N>>,
        ctrl::ControllerPhyMaintenanceFailure<'a, S, N, MT, Self>,
    > {
        let deadline = self.window.execution;
        let maintained = crate::controller::boot::maintenance::maintain::<
            P,
            D,
            O,
            M,
            S,
            Self,
            N,
            MT,
            H2C,
            C2H,
            PC,
        >(
            self,
            timer,
            interrupt,
            platform,
            controller,
            clock,
            observer,
            Some(deadline),
        )
        .await?;
        Ok(ctrl::ControllerPhyMaintained {
            task: PeripheralPhyMaintenanceRestoring {
                ready: maintained.task,
            },
            timer: maintained.timer,
            outcome: maintained.outcome,
        })
    }
}
