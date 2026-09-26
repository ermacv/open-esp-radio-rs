//! Quiescent shared-PHY tracking with the original HCI and powered runtime.

use super::*;
use oer_esp32s31_bluetooth::ble_phy::{BlePhyRetainedOwners, BlePhyRetiredMemory};
use oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform;
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerOutputReleaseError, InterruptOutputAfterRoutesOwner,
};
use oer_esp32s31_phy::{
    BluetoothPhyMaintenanceFailure, PhyAsyncDelay, PhyTargetObserver,
    RegisteredBluetoothPhyTrackEvaluationFailure, TargetBluetoothPhyParamTrackingFailure,
    TargetPhyParamTrackingError,
    state::client::{PhyPllTrackClock, PhyTrackTimeError},
    tracking::PhyParamTrackingOutcome,
    tracking::deadline::{TrackingDeadline, TrackingDeadlineError},
};

/// Exact failed maintenance edge; no failed edge returns a runnable task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerPhyMaintenanceError<E> {
    EpochMismatch,
    Task(ControllerTaskRetirementError),
    Hardware(BluetoothControllerOutputReleaseError),
    Clock(PhyTrackTimeError),
    Tracking(TargetPhyParamTrackingError),
    Deadline(TrackingDeadlineError),
    Storage(E),
}

#[allow(
    clippy::large_enum_variant,
    reason = "failed PHY owners remain allocation-free"
)]
enum PhyStage {
    InSlot,
    /// PHY is settled, but reactivation/publication did not complete.
    Restoration,
    Settled {
        _owner: BlePhyRetainedOwners,
    },
    Evaluation {
        _memory: BlePhyRetiredMemory,
        _failure: RegisteredBluetoothPhyTrackEvaluationFailure,
    },
    Tracking {
        _memory: BlePhyRetiredMemory,
        _failure: TargetBluetoothPhyParamTrackingFailure,
    },
}

/// Failure retains the original command authority, allocations and unrouted IRQs.
/// The platform remains in its original exclusive lease. Recovery requires an
/// explicit complete owner transition or an out-of-band reset; dropping this
/// value never grants permission to restart or release the powered platform.
#[must_use = "retain the failed powered maintenance frontier"]
pub struct ControllerPhyMaintenanceFailure<
    'a,
    S: InterruptOwnerRestartStorage,
    const SC: usize,
    const MT: usize,
    A = ControllerIdleCommandTask<'a, S, SC>,
> {
    error: ControllerPhyMaintenanceError<S::RestartError>,
    _task: A,
    _timer: ControllerModemTimerRetired<'a, S, MT>,
    _interrupt: InterruptOutputAfterRoutesOwner,
    _phy: PhyStage,
}

impl<S: InterruptOwnerRestartStorage, const SC: usize, const MT: usize, A>
    ControllerPhyMaintenanceFailure<'_, S, SC, MT, A>
{
    pub const fn error(&self) -> &ControllerPhyMaintenanceError<S::RestartError> {
        &self.error
    }

    /// Distinguish preflight rejection from failed execution/restoration using
    /// the retained owner, not the potentially identical public error code.
    pub const fn failure_stage(
        &self,
    ) -> oer_esp32s31_phy::tracking::fail_stop::MaintenanceFailureStage {
        use oer_esp32s31_phy::tracking::fail_stop::MaintenanceFailureStage;
        match &self._phy {
            PhyStage::InSlot | PhyStage::Evaluation { .. } => MaintenanceFailureStage::Admission,
            PhyStage::Tracking { .. } => MaintenanceFailureStage::Execution,
            PhyStage::Settled { .. } | PhyStage::Restoration => {
                MaintenanceFailureStage::Restoration
            }
        }
    }
}

/// Same HCI authority, timer epoch and task borrows after a maintenance window.
/// `None` means the real deadline was not due; `Some` reports the target result,
/// including whether the selected PHY policy inhibited calibration.
#[must_use = "restore routing before driving the returned task and timer"]
pub struct ControllerPhyMaintained<
    'a,
    S,
    const SC: usize,
    const MT: usize,
    A = ControllerIdleCommandTask<'a, S, SC>,
> {
    pub task: A,
    pub timer: ControllerModemTimerTask<'a, S, MT>,
    pub outcome: Option<PhyParamTrackingOutcome>,
}

impl<'a, S, const SC: usize> ControllerIdleCommandTask<'a, S, SC>
where
    S: InterruptOwnerRestartStorage + ModemLpTimerSoftwareOwnerStorage,
{
    /// Service a due shared-PHY request with all command roles idle, both ISR
    /// owners extracted and CPU routes absent. Admission observes actual time
    /// work, role allocations, scheduler heads, BUSY and primary faults before
    /// any PHY access. The original HCI stays open; queued Host traffic waits.
    ///
    /// The counter, scheduler timeline identity, role generations and DF/BLE
    /// publications remain in the same powered epoch. Both actual ISR owners
    /// are restored atomically before the unchanged task can return.
    /// `tracking_deadline` optionally guards PHY execution in `D`'s monotonic
    /// clock domain. Reserve routing/restoration time outside this deadline.
    /// `None` is an explicitly unbounded idle diagnostic operation; it cannot
    /// establish an active-connection pause budget.
    ///
    /// # Cancellation
    /// Once polled, drive to a terminal result. Cancellation or failure does not
    /// manufacture a runnable task or a cold owner; it requires external reset.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit affine capabilities for one maintenance join"
    )]
    pub fn maintain_phy<
        P,
        D,
        O,
        M: RawMutex,
        const MT: usize,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        self,
        timer: ControllerModemTimerRetired<'a, S, MT>,
        interrupt: InterruptOutputAfterRoutesOwner,
        platform: &mut ControllerRuntimePlatform<'a, P>,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        clock: &mut impl PhyPllTrackClock,
        observer: O,
        tracking_deadline: Option<TrackingDeadline>,
    ) -> impl core::future::Future<
        Output = Result<
            ControllerPhyMaintained<'a, S, SC, MT>,
            ControllerPhyMaintenanceFailure<'a, S, SC, MT>,
        >,
    >
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        maintain::<P, D, O, M, S, Self, SC, MT, H2C, C2H, PC>(
            self,
            timer,
            interrupt,
            platform,
            controller,
            clock,
            observer,
            tracking_deadline,
        )
    }
}

pub(crate) trait MaintenanceAuthority<'a, S, const SC: usize> {
    fn task_mut(&mut self) -> &mut ControllerPublishedTaskService<'a, S, SC>;
    fn roles_ready(&self) -> Result<(), ControllerRoleRetirementError>;
    fn accepts<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
    ) -> bool;
}
impl<'a, S, const SC: usize> MaintenanceAuthority<'a, S, SC>
    for ControllerIdleCommandTask<'a, S, SC>
{
    fn task_mut(&mut self) -> &mut ControllerPublishedTaskService<'a, S, SC> {
        &mut self.task
    }
    fn roles_ready(&self) -> Result<(), ControllerRoleRetirementError> {
        self.task.roles.retirement_ready()
    }
    fn accepts<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
    ) -> bool {
        self.accepts_hci_endpoint(controller)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one physical join consumes explicit affine capabilities"
)]
pub(crate) async fn maintain<
    'a,
    P,
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
    M: RawMutex,
    S: InterruptOwnerRestartStorage + ModemLpTimerSoftwareOwnerStorage,
    A: MaintenanceAuthority<'a, S, SC>,
    const SC: usize,
    const MT: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    mut authority: A,
    timer: ControllerModemTimerRetired<'a, S, MT>,
    interrupt: InterruptOutputAfterRoutesOwner,
    platform: &mut ControllerRuntimePlatform<'a, P>,
    controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
    clock: &mut impl PhyPllTrackClock,
    observer: O,
    tracking_deadline: Option<TrackingDeadline>,
) -> Result<
    ControllerPhyMaintained<'a, S, SC, MT, A>,
    ControllerPhyMaintenanceFailure<'a, S, SC, MT, A>,
> {
    let admission = authority
        .task_mut()
        .runtime
        .runtime_retirement_ready()
        .map_err(ControllerTaskRetirementError::Runtime)
        .and_then(|()| {
            authority
                .task_mut()
                .runtime
                .task_owner()
                .controller_time_retirement_ready()
                .map_err(ControllerTaskRetirementError::ControllerTime)
        })
        .and_then(|()| {
            authority
                .roles_ready()
                .map_err(ControllerTaskRetirementError::Role)
        });
    let error =
        if !authority.accepts(controller) || !timer.matches_storage(authority.task_mut().storage) {
            Some(ControllerPhyMaintenanceError::EpochMismatch)
        } else if let Err(error) = admission {
            Some(ControllerPhyMaintenanceError::Task(error))
        } else {
            None
        };
    if let Some(error) = error {
        return Err(ControllerPhyMaintenanceFailure {
            error,
            _task: authority,
            _timer: timer,
            _interrupt: interrupt,
            _phy: PhyStage::InSlot,
        });
    }
    // The access keeps the task owner and the unrouted output bank borrowed
    // until the PHY operation ends, so no route can be reactivated meanwhile.
    let task = authority.task_mut();
    let mut access =
        match interrupt.try_phy_maintenance(task.runtime.task_owner().maintenance_registers()) {
            Ok(access) => access,
            Err(error) => {
                return Err(ControllerPhyMaintenanceFailure {
                    error: ControllerPhyMaintenanceError::Hardware(error),
                    _task: authority,
                    _timer: timer,
                    _interrupt: interrupt,
                    _phy: PhyStage::InSlot,
                });
            }
        };
    let Some(platform) = platform.platform_mut_for_epoch(controller.epoch_identity()) else {
        return Err(ControllerPhyMaintenanceFailure {
            error: ControllerPhyMaintenanceError::EpochMismatch,
            _task: authority,
            _timer: timer,
            _interrupt: interrupt,
            _phy: PhyStage::InSlot,
        });
    };
    if let Some(deadline) = tracking_deadline
        && let Err(error) = deadline.check(D::now_micros())
    {
        return Err(ControllerPhyMaintenanceFailure {
            error: ControllerPhyMaintenanceError::Deadline(error),
            _task: authority,
            _timer: timer,
            _interrupt: interrupt,
            _phy: PhyStage::InSlot,
        });
    }
    let (phy, ()) = task
        .ble_phy_owners
        .try_retire(|| Ok::<_, core::convert::Infallible>(()))
        .unwrap();
    let (phy, memory) = phy.into_shutdown_parts();
    let result = {
        let maintenance =
            phy.maintain::<P, D, O>(platform, &mut access, clock, observer, tracking_deadline);
        #[cfg(feature = "lifecycle-fault-injection")]
        let maintenance = oer_esp32s31_phy::fault_injection::drive(maintenance);
        let mut maintenance = core::pin::pin!(maintenance);
        core::future::poll_fn(|cx| poll_tracking(maintenance.as_mut(), cx)).await
    };
    let (phy, outcome) = match result {
        Ok(maintained) => maintained,
        Err(BluetoothPhyMaintenanceFailure::Evaluation(failure)) => {
            return Err(ControllerPhyMaintenanceFailure {
                error: ControllerPhyMaintenanceError::Clock(failure.error()),
                _task: authority,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::Evaluation {
                    _memory: memory,
                    _failure: failure,
                },
            });
        }
        Err(BluetoothPhyMaintenanceFailure::Tracking(failure)) => {
            return Err(ControllerPhyMaintenanceFailure {
                error: ControllerPhyMaintenanceError::Tracking(failure.error()),
                _task: authority,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::Tracking {
                    _memory: memory,
                    _failure: failure,
                },
            });
        }
    };
    let phy = memory.with_client(phy);
    #[cfg(feature = "lifecycle-fault-injection")]
    oer_esp32s31_phy::fault_injection::checkpoint(
        oer_esp32s31_phy::fault_injection::Boundary::Restoration,
    )
    .await;
    if let Some(deadline) = tracking_deadline
        && let Err(error) = deadline.check(D::now_micros())
    {
        return Err(ControllerPhyMaintenanceFailure {
            error: ControllerPhyMaintenanceError::Deadline(error),
            _task: authority,
            _timer: timer,
            _interrupt: interrupt,
            _phy: PhyStage::Settled { _owner: phy },
        });
    }
    finish(authority, timer, interrupt, phy, outcome)
}

#[inline(never)]
fn poll_tracking<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    let poll: fn(
        core::pin::Pin<&mut F>,
        &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> = F::poll;
    core::hint::black_box(poll)(future, cx)
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "rejection retains actual powered owners without allocation"
)]
fn finish<
    'a,
    S: InterruptOwnerRestartStorage + ModemLpTimerSoftwareOwnerStorage,
    A: MaintenanceAuthority<'a, S, SC>,
    const SC: usize,
    const MT: usize,
>(
    mut task: A,
    timer: ControllerModemTimerRetired<'a, S, MT>,
    interrupt: InterruptOutputAfterRoutesOwner,
    phy: BlePhyRetainedOwners,
    outcome: Option<PhyParamTrackingOutcome>,
) -> Result<
    ControllerPhyMaintained<'a, S, SC, MT, A>,
    ControllerPhyMaintenanceFailure<'a, S, SC, MT, A>,
> {
    task.task_mut()
        .ble_phy_owners
        .restore(phy)
        .unwrap_or_else(|_| panic!("original PHY slot is empty"));
    let interrupt = match interrupt.try_reactivate_idle_controller_output(
        task.task_mut().runtime.task_owner().maintenance_registers(),
    ) {
        Ok(interrupt) => interrupt,
        Err((error, interrupt)) => {
            return Err(ControllerPhyMaintenanceFailure {
                error: ControllerPhyMaintenanceError::Hardware(error),
                _task: task,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::Restoration,
            });
        }
    };
    let (timer_owner, timer_runtime, storage) = timer.into_shutdown_parts();
    if let Err((error, interrupt, timer_owner)) =
        storage.restore_initialized_interrupt_owners(interrupt, timer_owner)
    {
        return Err(ControllerPhyMaintenanceFailure {
            error: ControllerPhyMaintenanceError::Storage(error),
            _task: task,
            _timer: ControllerModemTimerRetired::from_maintenance_parts(
                timer_owner,
                timer_runtime,
                storage,
            ),
            _interrupt: interrupt.deactivate(),
            _phy: PhyStage::Restoration,
        });
    }
    Ok(ControllerPhyMaintained {
        task,
        timer: ControllerModemTimerTask::new(storage, timer_runtime),
        outcome,
    })
}

impl<'a, S, const SC: usize> ControllerPublishedTaskService<'a, S, SC> {
    pub(crate) fn bluetooth_tracking_schedule_at(
        &self,
        now_micros: u64,
    ) -> Result<oer_esp32s31_phy::tracking::schedule::Schedule, PhyTrackTimeError> {
        self.ble_phy_owners.tracking_schedule_at(now_micros)
    }

    pub(crate) fn peripheral_maintenance_roles_ready(
        &self,
        candidate: &crate::le::peripheral::PeripheralConnectionRecurringEventCandidate,
    ) -> Result<(), ControllerRoleRetirementError> {
        self.roles.other_roles_ready()?;
        if !candidate.belongs_to(&self.roles.peripheral_connection_resources) {
            return Err(ControllerRoleRetirementError::PeripheralConnection);
        }
        Ok(())
    }
}

impl<S, const SC: usize> ControllerIdleCommandTask<'_, S, SC> {
    /// Set diagnostic thermal thresholds only on an idle Controller owner.
    /// Return the old policy for restoration after the measured transaction.
    pub fn set_phy_tracking_debug(
        &mut self,
        debug: oer_esp32s31_phy::state::PhyTemperatureTrackingDebug,
    ) -> oer_esp32s31_phy::state::PhyTemperatureTrackingDebug {
        self.task.ble_phy_owners.set_tracking_debug(debug)
    }
    /// Observe the retained PHY scheduler without acknowledging or granting work.
    pub fn phy_tracking_schedule_at(
        &self,
        now_micros: u64,
    ) -> Result<oer_esp32s31_phy::tracking::schedule::Schedule, PhyTrackTimeError> {
        self.task.bluetooth_tracking_schedule_at(now_micros)
    }
}
