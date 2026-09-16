//! Quiescent shared-PHY tracking with the original HCI and powered runtime.

use super::*;
use crate::ble_phy::{BlePhyRetainedOwners, BlePhyRetiredMemory};
use crate::resources::platform_retirement::ControllerRuntimePlatform;
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerOutputReleaseError, InterruptOutputAfterRoutesOwner,
};
use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, RegisteredBluetoothPhyTrackEvaluationFailure,
    TargetBluetoothPhyParamTrackingFailure, TargetPhyParamTrackingError,
    state::client::{PhyPllTrackClock, PhyTrackTimeError},
    tracking::parameters::PhyParamTrackingOutcome,
};

/// Exact failed maintenance edge; no failed edge returns a runnable task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerPhyMaintenanceError<E> {
    EpochMismatch,
    Task(ControllerTaskRetirementError),
    Hardware(BluetoothControllerOutputReleaseError),
    Clock(PhyTrackTimeError),
    Tracking(TargetPhyParamTrackingError),
    Storage(E),
}

#[allow(
    clippy::large_enum_variant,
    reason = "failed PHY owners remain allocation-free"
)]
enum PhyStage {
    InSlot,
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
> {
    error: ControllerPhyMaintenanceError<S::RestartError>,
    _task: ControllerIdleCommandTask<'a, S, SC>,
    _timer: ControllerModemTimerRetired<'a, S, MT>,
    _interrupt: InterruptOutputAfterRoutesOwner,
    _phy: PhyStage,
}

impl<S: InterruptOwnerRestartStorage, const SC: usize, const MT: usize>
    ControllerPhyMaintenanceFailure<'_, S, SC, MT>
{
    pub const fn error(&self) -> &ControllerPhyMaintenanceError<S::RestartError> {
        &self.error
    }
}

/// Same HCI authority, timer epoch and task borrows after a maintenance window.
/// `None` means the real deadline was not due; `Some` reports the target result,
/// including whether the selected PHY policy inhibited calibration.
#[must_use = "restore routing before driving the returned task and timer"]
pub struct ControllerPhyMaintained<'a, S, const SC: usize, const MT: usize> {
    pub task: ControllerIdleCommandTask<'a, S, SC>,
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
    ///
    /// # Cancellation
    /// Once polled, drive to a terminal result. Cancellation or failure does not
    /// manufacture a runnable task or a cold owner; it requires external reset.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit affine capabilities for one maintenance join"
    )]
    pub async fn maintain_phy<
        P,
        D,
        O,
        M: RawMutex,
        const MT: usize,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        mut self,
        timer: ControllerModemTimerRetired<'a, S, MT>,
        interrupt: InterruptOutputAfterRoutesOwner,
        platform: &mut ControllerRuntimePlatform<'a, P>,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        clock: &mut impl PhyPllTrackClock,
        observer: O,
    ) -> Result<
        ControllerPhyMaintained<'a, S, SC, MT>,
        ControllerPhyMaintenanceFailure<'a, S, SC, MT>,
    >
    where
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let admission = self
            .task
            .runtime
            .runtime
            .retirement_ready()
            .map_err(ControllerTaskRetirementError::Runtime)
            .and_then(|()| {
                self.task
                    .runtime
                    .task
                    .controller_time_retirement_ready()
                    .map_err(ControllerTaskRetirementError::ControllerTime)
            })
            .and_then(|()| {
                self.task
                    .roles
                    .retirement_ready()
                    .map_err(ControllerTaskRetirementError::Role)
            });
        let error = if !self.accepts_hci_endpoint(controller)
            || !timer.matches_storage(self.task.storage)
        {
            Some(ControllerPhyMaintenanceError::EpochMismatch)
        } else if let Err(error) = admission {
            Some(ControllerPhyMaintenanceError::Task(error))
        } else {
            interrupt
                .validate_idle_controller(self.task.runtime.task.maintenance_registers())
                .err()
                .map(ControllerPhyMaintenanceError::Hardware)
        };
        if let Some(error) = error {
            return Err(ControllerPhyMaintenanceFailure {
                error,
                _task: self,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::InSlot,
            });
        }
        let Some(platform) = platform.platform_mut_for_epoch(controller.epoch_identity()) else {
            return Err(ControllerPhyMaintenanceFailure {
                error: ControllerPhyMaintenanceError::EpochMismatch,
                _task: self,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::InSlot,
            });
        };
        let (phy, ()) = self
            .task
            .ble_phy_owners
            .try_retire(|| Ok::<_, core::convert::Infallible>(()))
            .unwrap();
        let (phy, memory) = phy.into_shutdown_parts();
        let evaluation = match phy.evaluate_due_tracking(clock) {
            Ok(evaluation) => evaluation,
            Err(failure) => {
                return Err(ControllerPhyMaintenanceFailure {
                    error: ControllerPhyMaintenanceError::Clock(failure.error()),
                    _task: self,
                    _timer: timer,
                    _interrupt: interrupt,
                    _phy: PhyStage::Evaluation {
                        _memory: memory,
                        _failure: failure,
                    },
                });
            }
        };
        let (phy, outcome) = match evaluation.into_owner() {
            Ok(phy) => (phy, None),
            Err(pending) => {
                let result = {
                    let mut registers = self.task.runtime.task.shared_phy_hal();
                    let mut tracking = core::pin::pin!(
                        oer_esp32s31_phy::run_target_bluetooth_phy_param_tracking::<P, D, O>(
                            platform,
                            &mut registers,
                            pending.begin_tracking(),
                            observer
                        )
                    );
                    core::future::poll_fn(|cx| poll_tracking(tracking.as_mut(), cx)).await
                };
                match result {
                    Ok(success) => {
                        let (phy, outcome) = success.into_parts();
                        (phy, Some(outcome))
                    }
                    Err(failure) => {
                        return Err(ControllerPhyMaintenanceFailure {
                            error: ControllerPhyMaintenanceError::Tracking(failure.error()),
                            _task: self,
                            _timer: timer,
                            _interrupt: interrupt,
                            _phy: PhyStage::Tracking {
                                _memory: memory,
                                _failure: failure,
                            },
                        });
                    }
                }
            }
        };
        finish(self, timer, interrupt, memory.with_client(phy), outcome)
    }
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
    const SC: usize,
    const MT: usize,
>(
    mut task: ControllerIdleCommandTask<'a, S, SC>,
    timer: ControllerModemTimerRetired<'a, S, MT>,
    interrupt: InterruptOutputAfterRoutesOwner,
    phy: BlePhyRetainedOwners,
    outcome: Option<PhyParamTrackingOutcome>,
) -> Result<ControllerPhyMaintained<'a, S, SC, MT>, ControllerPhyMaintenanceFailure<'a, S, SC, MT>>
{
    task.task
        .ble_phy_owners
        .restore(phy)
        .unwrap_or_else(|_| panic!("original PHY slot is empty"));
    let interrupt = match interrupt
        .try_reactivate_idle_controller_output(task.task.runtime.task.maintenance_registers())
    {
        Ok(interrupt) => interrupt,
        Err((error, interrupt)) => {
            return Err(ControllerPhyMaintenanceFailure {
                error: ControllerPhyMaintenanceError::Hardware(error),
                _task: task,
                _timer: timer,
                _interrupt: interrupt,
                _phy: PhyStage::InSlot,
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
            _phy: PhyStage::InSlot,
        });
    }
    Ok(ControllerPhyMaintained {
        task,
        timer: ControllerModemTimerTask::new(storage, timer_runtime),
        outcome,
    })
}
