//! Reinitialize actual cold hardware and refill the original exclusive leases.

use super::*;
use crate::{
    controller::ControllerInterruptOwnersReady,
    interrupt::InterruptOwnerRestartStorage,
    modem_timer::ControllerModemTimerTask,
    phy::{
        ControllerPhyClientAcquireFailure, ControllerPhyInitializationFailure,
        ControllerPhyTrackingFailure, PhyInitializationConfig,
    },
    resources::{
        TeardownPendingPlatform,
        platform_retirement::{ControllerPlatformLease, ControllerRuntimePlatform},
        runtime_owner::RuntimeOwnerLease,
    },
    runtime_resources::ControllerRuntimeResources,
};
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    InProcessHciHostTransport, LeControllerCommandEndpoint, LeControllerCommandReadyClaim,
};
use oer_esp32s31_phy::{NoopPhyTargetObserver, PhyAsyncDelay, state::client::PhyPllTrackClock};

/// Failure boundary of a powered restart. Every branch retains the actual owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRestartError<E> {
    Hci(oer_bluetooth_hci::LeControllerHciRestartError),
    Clock(crate::clock::ClockError),
    LowPower(oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerOwnerError),
    Phy(crate::phy::PhyInitializationError),
    Acquire(oer_esp32s31_phy::state::client::PhyClientAcquireError),
    Tracking(oer_esp32s31_phy::TargetPhyParamTrackingError),
    Storage(E),
}

struct Retained<'a, P, S, const SC: usize, const MT: usize> {
    software: ControllerPublishedTaskService<'a, S, SC>,
    roles: super::super::super::role_retirement::ControllerRoleResources,
    memory: Option<crate::ble_phy::BlePhyRetiredMemory>,
    hci: oer_bluetooth_hci::LeControllerHciRetired<'a, ()>,
    timer_runtime: crate::runtime_resources::ControllerModemTimerRuntime<'a, MT>,
    timer_storage: &'a S,
    platform_slot: RuntimeOwnerLease<'a, TeardownPendingPlatform<P>>,
}

#[allow(
    clippy::large_enum_variant,
    reason = "allocation-free quarantine retains each actual failed stage"
)]
enum Stage<'a, P, S, const SC: usize, const MT: usize> {
    Before(ControllerColdReleased<'a, P, S, SC, MT>),
    Clock(crate::clock::ClockEnableFailure<P>),
    LowPower(crate::low_power::ControllerLowPowerHardwareInitializationFailure<P, MT, SC>),
    Phy(ControllerPhyInitializationFailure<P, MT, SC>),
    Acquire(ControllerPhyClientAcquireFailure<P, MT, SC>),
    Tracking(ControllerPhyTrackingFailure<P, MT, SC>),
    Storage(ControllerInterruptOwnersReady<P, MT, SC>),
}

/// Failed restart retains all original SRAM borrows and the failed hardware stage.
/// It exposes no route activation, owner replacement or implicit powered cleanup.
#[must_use = "retain the complete failed restart until explicit recovery or board reset"]
pub struct ControllerRestartFailure<
    'a,
    P,
    S: InterruptOwnerRestartStorage,
    const SC: usize,
    const MT: usize,
> {
    error: ControllerRestartError<S::RestartError>,
    _retained: Option<Retained<'a, P, S, SC, MT>>,
    _stage: Stage<'a, P, S, SC, MT>,
}
impl<
    P,
    S: InterruptOwnerRestartStorage + crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    const SC: usize,
    const MT: usize,
> ControllerRestartFailure<'_, P, S, SC, MT>
{
    pub const fn error(&self) -> &ControllerRestartError<S::RestartError> {
        &self.error
    }

    /// Whether the retained restart failed inside PHY execution without safe
    /// cleanup. Preflight, client policy and publication errors are not PHY
    /// execution failures and retain their existing non-runnable owners.
    pub fn phy_hardware_ambiguous(&self) -> bool {
        match &self._stage {
            Stage::Phy(failure) => failure.phy_hardware_ambiguous(),
            Stage::Tracking(_) => true,
            Stage::Before(_)
            | Stage::Clock(_)
            | Stage::LowPower(_)
            | Stage::Acquire(_)
            | Stage::Storage(_) => false,
        }
    }
}

/// Fresh powered Controller endpoints using the original statically borrowed slots.
/// All CPU routes remain disabled. The old ISR service and notifications must
/// join these endpoints before activation; old Host handles remain closed.
#[must_use = "join the restarted endpoints before enabling CPU routes"]
pub struct ControllerRestarted<
    'a,
    P,
    S: crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    M: RawMutex,
    const SC: usize,
    const MT: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    pub task: ControllerIdleCommandTask<'a, S, SC>,
    pub timer: ControllerModemTimerTask<'a, S, MT>,
    pub platform: ControllerRuntimePlatform<'a, P>,
    pub host: InProcessHciHostTransport<'a, M, H2C, C2H, PC>,
}

impl<
    'a,
    P,
    S: InterruptOwnerRestartStorage + crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    const SC: usize,
    const MT: usize,
> ControllerColdReleased<'a, P, S, SC, MT>
{
    /// Run the existing cold initialization chain on the returned hardware and
    /// original BLE/DF memory, then refill the same exclusive runtime leases.
    /// Role allocations and their generations remain intact. The selected
    /// calibration profile is explicit, as for initial cold start.
    ///
    /// # Cancellation
    /// Once polled, drive to a terminal result. Cancellation never releases the
    /// platform reservation or grants reuse of partially initialized hardware.
    #[allow(
        clippy::result_large_err,
        reason = "all failed stages retain actual allocation-free ownership"
    )]
    pub async fn restart<
        D: PhyAsyncDelay,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        config: PhyInitializationConfig,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<
        ControllerRestarted<'a, P, S, M, SC, MT, H2C, C2H, PC>,
        ControllerRestartFailure<'a, P, S, SC, MT>,
    > {
        if let Err(error) = controller.check_restart_transport(&self.storage._hci) {
            return Err(ControllerRestartFailure {
                error: ControllerRestartError::Hci(error),
                _retained: None,
                _stage: Stage::Before(self),
            });
        }
        self.storage
            ._software
            .runtime
            .runtime
            .clear_notifications_after_cold_release();
        let public_address = controller.bootstrap_config().public_address();
        let Self {
            radio,
            platform_slot,
            storage,
            ..
        } = self;
        let ControllerRetiredStorage {
            _software,
            _roles,
            _memory,
            _hci,
            _timer_runtime,
            _timer_storage,
        } = storage;
        let retained = Retained {
            software: _software,
            roles: _roles,
            memory: Some(_memory),
            hci: _hci,
            timer_runtime: _timer_runtime,
            timer_storage: _timer_storage,
            platform_slot,
        };
        macro_rules! step {
            ($operation:expr, $variant:ident) => {
                match $operation {
                    Ok(owner) => owner,
                    Err(failure) => {
                        return Err(ControllerRestartFailure {
                            error: ControllerRestartError::$variant(failure.error()),
                            _retained: Some(retained),
                            _stage: Stage::$variant(failure),
                        })
                    }
                }
            };
        }
        let clocked = step!(radio.enable_clocks(), Clock);
        let scheduler = clocked
            .initialize_controller_hal()
            .initialize_scheduler(ControllerRuntimeResources::<MT, SC>::new());
        let low_power = step!(scheduler.initialize_modem_lp_timer_hardware(), LowPower);
        let registered = step!(
            low_power
                .initialize_common_phy::<D, NoopPhyTargetObserver>(config, NoopPhyTargetObserver)
                .await,
            Phy
        );
        let acquired = step!(registered.acquire_phy_client(clock), Acquire);
        let initialized = match acquired.into_owner() {
            Ok(owner) => owner,
            Err(pending) => step!(
                pending
                    .begin_tracking()
                    .complete_tracking::<D, NoopPhyTargetObserver>(NoopPhyTargetObserver)
                    .await,
                Tracking
            ),
        };
        complete_initialization(initialized, retained, public_address, controller)
    }
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "publication failure retains the complete initialized graph"
)]
fn complete_initialization<
    'a,
    P,
    S: InterruptOwnerRestartStorage + crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    M: RawMutex,
    const SC: usize,
    const MT: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    initialized: crate::common_phy_state::ControllerPhyInitialized<P, MT, SC>,
    mut retained: Retained<'a, P, S, SC, MT>,
    public_address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    controller: &mut LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
) -> Result<
    ControllerRestarted<'a, P, S, M, SC, MT, H2C, C2H, PC>,
    ControllerRestartFailure<'a, P, S, SC, MT>,
> {
    let (ble, df) = retained
        .memory
        .take()
        .expect("original cold allocations")
        .into_restart_parts();
    let ready = initialized
        .initialize_baseband()
        .initialize_ble_phy_engine(ble, df, public_address)
        .prepare_controller_output_and_start_runtime_timer()
        .stage_interrupt_owners();
    let parts = match ready.restore_interrupt_owners(retained.software.storage) {
        Ok(parts) => parts,
        Err((error, ready)) => {
            return Err(ControllerRestartFailure {
                error: ControllerRestartError::Storage(error),
                _retained: Some(retained),
                _stage: Stage::Storage(ready),
            });
        }
    };
    Ok(finish_restart(retained, parts, controller))
}

#[inline(never)]
fn finish_restart<
    'a,
    P,
    S: crate::modem_timer::ModemLpTimerSoftwareOwnerStorage,
    M: RawMutex,
    const SC: usize,
    const MT: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    retained: Retained<'a, P, S, SC, MT>,
    parts: crate::ble_phy::BlePhyRestartParts<P, MT, SC>,
    controller: &mut LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
) -> ControllerRestarted<'a, P, S, M, SC, MT, H2C, C2H, PC> {
    let Retained {
        mut software,
        roles,
        hci,
        timer_runtime,
        timer_storage,
        platform_slot,
        ..
    } = retained;
    let crate::ble_phy::BlePhyRestartParts {
        scheduler,
        physical,
        timing,
        direction_finding,
    } = parts;
    let crate::scheduler::core::SchedulerRestartParts {
        task,
        platform,
        time_scale,
        config,
        scheduler_list,
        runtime,
    } = scheduler;
    software
        .runtime
        .task
        .restore(task)
        .unwrap_or_else(|_| panic!("retired task slot"));
    software
        .roles
        .restore(roles)
        .unwrap_or_else(|_| panic!("retired role slot"));
    software
        .ble_phy_owners
        .restore(physical)
        .unwrap_or_else(|_| panic!("retired PHY slot"));
    software.runtime.time_scale = time_scale;
    software.runtime.config = config;
    *software.runtime._scheduler_list = scheduler_list;
    *software.scheduler_epoch = None;
    software.ble_phy_timing = timing;
    software.direction_finding_workspace = direction_finding;
    *timer_runtime.epoch = runtime.into_started_modem_epoch();
    // Both closed queues and the sole endpoint were retained throughout boot.
    let host = controller
        .restart_transport(hci)
        .unwrap_or_else(|_| panic!("validated exclusive HCI restart"));
    let platform =
        ControllerPlatformLease::restore(platform_slot, platform).bind(controller.epoch_identity());
    let LeControllerCommandReadyClaim::Ready(ready) = controller.claim_initial_command_ready(())
    else {
        unreachable!("fresh HCI epoch")
    };
    ControllerRestarted {
        task: ControllerIdleCommandTask::from_parts(software, ready),
        timer: ControllerModemTimerTask::new(timer_storage, timer_runtime),
        platform,
        host,
    }
}
