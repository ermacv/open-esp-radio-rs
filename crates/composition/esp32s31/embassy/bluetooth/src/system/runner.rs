//! One hardware owner and its fair command/timer loop under IRQ fault priority.
//!
//! Boundary classification translates concrete runtime outcomes into the shared
//! runner policy. The loop retains all borrowed owners across the same awaits.

mod fail_stop;
mod maintenance;
mod retirement;
pub(crate) use fail_stop::fail_stop_shared_phy;
pub use retirement::{
    BluetoothHardwareColdReleased, BluetoothHardwareInterruptsRetired,
    BluetoothHardwareMaintenanceError, BluetoothHardwareMaintenanceFailure,
    BluetoothHardwareOutputReleased, BluetoothHardwareRestartError,
    BluetoothHardwareRestartFailure, BluetoothHardwareRetired,
    BluetoothHardwareRetiredWithPlatform, BluetoothHardwareShutdownFailure,
    BluetoothHardwareTimerError, BluetoothHardwareTimerFailure, BluetoothHardwareTimerRetired,
    BluetoothPlatformJoin,
};

use crate::{
    BluetoothInterruptFault, BluetoothInterruptRuntime,
    runner_policy::{
        CommandBoundaryAction, CommandBoundaryClass, HardwareRunnerSchedule,
        ModemTimerTransitionClass, classify_peripheral_fault, modem_timer_requires_quarantine,
        reduce_command_boundary, terminal_maintenance_reason,
    },
};

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use embassy_futures::{
    select::{Either, select},
    yield_now,
};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use esp_hal::rng::Rng;

use oer_bluetooth_hci::LeControllerCommandEndpoint;

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_bluetooth::controller::{
    ControllerModemTimerBegin, ControllerModemTimerRearm, ControllerModemTimerStep,
    ControllerModemTimerTask,
};

use oer_esp32s31_bluetooth_runtime::{
    controller::{
        ControllerCommandBoundary, ControllerCommandTask, DtmAbsoluteRecheck, ModemTimerDriveStep,
        ModemTimerDriver,
    },
    session::{
        advertising::LegacyAdvertisingDelaySource,
        dtm::{DtmControllerTimeRecheck, DtmControllerTimeRecheckStatus},
    },
};

use super::{
    CommandBoundary, ModemDriveStep, PublishedStorage, RuntimeWakers,
    quarantine::{BluetoothHardwareQuarantine, quarantine_routes, retain_quarantine_forever},
};

struct BluetoothAdvertisingDelaySource;

impl LegacyAdvertisingDelaySource for BluetoothAdvertisingDelaySource {
    fn next_advertising_delay(&mut self) -> AdvertisingDelay {
        let micros = (Rng::new().random() % (u32::from(AdvertisingDelay::MAX_MICROS) + 1)) as u16;
        AdvertisingDelay::from_micros(micros)
            .expect("the hardware entropy projection is inside the Link Layer domain")
    }
}

impl oer_esp32s31_bluetooth::le::peripheral::PeripheralEncryptionRandomSource
    for BluetoothAdvertisingDelaySource
{
    fn next_encryption_random(
        &mut self,
    ) -> oer_bluetooth_ll::security::LePeripheralEncryptionRandom {
        let first = Rng::new().random().to_le_bytes();
        let second = Rng::new().random().to_le_bytes();
        let initialization_vector = Rng::new().random().to_le_bytes();
        let mut diversifier = [0; 8];
        diversifier[..4].copy_from_slice(&first);
        diversifier[4..].copy_from_slice(&second);
        oer_bluetooth_ll::security::LePeripheralEncryptionRandom::new(
            diversifier,
            initialization_vector,
        )
    }
}

/// Sole hardware-side owner after the final Controller split.
///
/// `run` services the command actor and source-127 task fairly under strict IRQ
/// fault priority. Terminal command owners, unsupported expirations and exact
/// faults are retained forever only after the complete route set is disabled
/// or its exact disable rejection is retained for fail-stop quarantine.
#[must_use = "the hardware runner owns the live Controller epoch"]
pub struct BluetoothHardwareRunner<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    command: Option<ControllerCommandTask<'static, PublishedStorage, SCHEDULER_CAPACITY>>,
    watchdog: &'static crate::WatchdogConfig,
    restoration_protection: Option<oer_esp32s31_soc_esp_hal::watchdog::DeadlineLease<'static>>,
    controller: LeControllerCommandEndpoint<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    modem_timer: Option<ControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>>,
    modem_driver: ModemTimerDriver<'static, CriticalSectionRawMutex>,
    interrupt: Option<BluetoothInterruptRuntime>,
    packet: [u8; PACKET_CAPACITY],
    recheck: DtmAbsoluteRecheck,
    advertising_delay: BluetoothAdvertisingDelaySource,
    wakers: &'static RuntimeWakers,
    schedule: HardwareRunnerSchedule,
}

/// Complete returned owner after an interrupt-route cycle was rejected.
/// No task/HCI/timer or live/inactive IRQ authority is discarded.
#[must_use = "retain the complete failed runner and interrupt transition"]
pub struct BluetoothHardwareRouteCycleFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    _runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    _interrupt: RouteCycleFailure,
}

enum RouteCycleFailure {
    Disable {
        _failure: crate::BluetoothInterruptDisableFailure,
    },
    Bind {
        _failure: crate::BluetoothInterruptBindFailure,
    },
}

fn classify_command<const SCHEDULER_CAPACITY: usize>(
    boundary: &CommandBoundary<'_, SCHEDULER_CAPACITY>,
    advertising_completion: Option<
        oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    >,
    advertising_rejected_packets: Option<u32>,
    advertising_last_receive_rejection: Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )>,
) -> CommandBoundaryAction {
    use crate::diagnostics::{BluetoothExecutionEvent as Observed, record};
    match boundary {
        ControllerCommandBoundary::IdleRestored(
            oer_esp32s31_bluetooth_runtime::controller::ControllerIdleCompletion::LegacyConnectableAdvertisingStartRejected { cause },
        ) => {
            use oer_esp32s31_bluetooth::le::advertising::LegacyConnectableAdvertisingFirstRunnerRecoveredError as E;
            use crate::diagnostics::BluetoothAdvertisingStartRejection as R;
            let reason = match cause {
                E::Configuration(_) => R::Configuration,
                E::GenerationExhausted => R::GenerationExhausted,
                E::PduFit(_) => R::PduFit,
                E::AdvertisingEventActive => R::AdvertisingEventActive,
                E::PeripheralEventActive(_) => R::PeripheralEventActive,
                E::MemoryPreparation(_) => R::MemoryPreparation,
                E::TimingWindow => R::TimingWindow,
                E::Timeline(_) => R::Timeline,
                E::Sequence(_) => R::Sequence,
                E::EventFields(_) => R::EventFields,
                E::EmptyList(_) => R::EmptyList,
            };
            record(Observed::AdvertisingStartRejected(reason), format_args!("advertising rejected: {cause:?}"));
        }
        ControllerCommandBoundary::IdleRestored(
            oer_esp32s31_bluetooth_runtime::controller::ControllerIdleCompletion::PeripheralDisconnected { reason },
        ) => record(
            Observed::PeripheralDisconnected { reason: *reason },
            format_args!("peripheral disconnected: {reason}"),
        ),
        ControllerCommandBoundary::LegacyConnectableAdvertisingActive => record(
            Observed::ConnectableAdvertisingRun,
            format_args!(
                "advertising RUN; prev={:?}; rejected={:?}; last={:?}",
                advertising_completion,
                advertising_rejected_packets,
                advertising_last_receive_rejection
            ),
        ),
        ControllerCommandBoundary::PhyMaintenanceRestored(observation) => {
            crate::maintenance_observation::restored(*observation);
            record(Observed::PeripheralRun, format_args!("peripheral maintenance RUN"));
        }
        ControllerCommandBoundary::PeripheralConnectionActive => {
            record(Observed::PeripheralRun, format_args!("peripheral RUN"))
        }
        ControllerCommandBoundary::LegacyConnectableAdvertisingFailStop(fault) => record(
            Observed::Terminal,
            format_args!("advertising first: {:?}", fault.cause()),
        ),
        ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(fault) => record(
            Observed::Terminal,
            format_args!("advertising recurring: {:?}", fault.cause()),
        ),
        ControllerCommandBoundary::LegacyConnectableAdvertisingActiveFailStop(fault) => record(
            Observed::Terminal,
            format_args!(
                "adv {:?}; rx={:?}; nodes={:?}",
                fault.cause(),
                fault.receive_error(),
                fault.receive_observations().map(|nodes| nodes.map(|n| (
                    u8::from(n.completed),
                    u8::from(n.packet_retained),
                    u8::from(n.producer_updated),
                    u8::from(n.epoch_updated),
                    n.header
                )))
            ),
        ),
        ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault) => record(
            Observed::Terminal,
            format_args!("advertising pending: {:?}", fault.cause()),
        ),
        ControllerCommandBoundary::UnownedFinishedList(list) => record(
            Observed::Terminal,
            format_args!("unowned finished list: {:?}", list),
        ),
        ControllerCommandBoundary::PeripheralConnectionFirstFailStop(fault) => record(
            Observed::Terminal,
            format_args!("peripheral first: {:?}", fault.cause()),
        ),
        ControllerCommandBoundary::PeripheralConnectionActiveFailStop(fault) => record(
            Observed::Terminal,
            format_args!("peripheral active: {:?}", fault.cause()),
        ),
        ControllerCommandBoundary::Retryable(retry) => {
            record(Observed::Retry, format_args!("retry: {:?}", retry))
        }
        _ => {}
    }
    let class = match boundary {
        ControllerCommandBoundary::IdleRestored(_) => {
            CommandBoundaryClass::IdleRestored
        }
        ControllerCommandBoundary::PhyMaintenanceRestored(_) => CommandBoundaryClass::PhyRestored,
        ControllerCommandBoundary::Retryable(_) => CommandBoundaryClass::Retryable,
        ControllerCommandBoundary::PhyMaintenanceFailed(error) => {
            CommandBoundaryClass::SharedPhyFailed(terminal_maintenance_reason(*error))
        }
        ControllerCommandBoundary::PeripheralConnectionActiveFailStop(fault) => {
            classify_peripheral_fault(fault.cause())
        }
        ControllerCommandBoundary::PeripheralConnectionActiveResetFailStop(fault) => {
            classify_peripheral_fault(fault.cause())
        }
        ControllerCommandBoundary::NonCommand(_)
        | ControllerCommandBoundary::EndpointMismatch
        | ControllerCommandBoundary::HciFault(_)
        | ControllerCommandBoundary::PhyMaintenanceIdle
        | ControllerCommandBoundary::PhyMaintenancePeripheral
        | ControllerCommandBoundary::ControllerTimeExhausted
        | ControllerCommandBoundary::FirstEventFailed(_)
        | ControllerCommandBoundary::FirstPreparationCleanupFault { .. }
        | ControllerCommandBoundary::FirstPreparationRestoreRejected(_)
        | ControllerCommandBoundary::FirstPreparationFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringSequencePendingCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringGraphPreparedCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringCandidateCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringPreparedCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringMergedCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingActiveFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingStoppingFailStop(_)
        | ControllerCommandBoundary::PeripheralConnectionCommandEndpointMismatch(_)
        | ControllerCommandBoundary::PeripheralConnectionFirstFailStop(_)
        | ControllerCommandBoundary::PeripheralConnectionResetFailStop(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingCommandEndpointMismatch(_)
        | ControllerCommandBoundary::IdleCommandEndpointMismatch(_)
        | ControllerCommandBoundary::ActiveCommandEndpointMismatch(_)
        | ControllerCommandBoundary::PendingRadioFault(_)
        | ControllerCommandBoundary::CommandReadyRadioFault(_)
        | ControllerCommandBoundary::TestEndStoppingFault(_)
        | ControllerCommandBoundary::ResetStoppingFault(_) => {
            CommandBoundaryClass::Terminal
        }
        ControllerCommandBoundary::LegacyAdvertisingActive(_)
        | ControllerCommandBoundary::LegacyConnectableAdvertisingActive
        | ControllerCommandBoundary::PeripheralConnectionActive
        | ControllerCommandBoundary::PassiveScanningActive
        | ControllerCommandBoundary::PassiveScanMalformedPdu(_)
        | ControllerCommandBoundary::PassiveScanReportEncodingFault(_) => {
            CommandBoundaryClass::Progress
        }
        ControllerCommandBoundary::LegacyAdvertisingCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyAdvertisingActiveCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyAdvertisingRecurringCommandEndpointMismatch(_)
        | ControllerCommandBoundary::PassiveScanCommandEndpointMismatch(_)
        | ControllerCommandBoundary::PassiveScanActiveCommandEndpointMismatch(_)
        | ControllerCommandBoundary::LegacyAdvertisingFault(_)
        | ControllerCommandBoundary::LegacyAdvertisingPendingFault(_)
        | ControllerCommandBoundary::LegacyAdvertisingStoppingFault(_)
        | ControllerCommandBoundary::LegacyAdvertisingRecurringStopFault(_)
        | ControllerCommandBoundary::LegacyAdvertisingRecurringFault(_)
        | ControllerCommandBoundary::LegacyAdvertisingSequenceExhausted(_)
        | ControllerCommandBoundary::PassiveScanFault(_)
        | ControllerCommandBoundary::PassiveScanPendingFault(_)
        | ControllerCommandBoundary::PassiveScanStoppingFault(_)
        | ControllerCommandBoundary::PassiveScanRecurringFault(_) => {
            CommandBoundaryClass::Terminal
        }
        ControllerCommandBoundary::UnownedFinishedList(_) => {
            CommandBoundaryClass::UnownedFinishedList
        }
    };
    let action = reduce_command_boundary(class);
    if matches!(action, CommandBoundaryAction::Quarantine) {
        record(Observed::Terminal, format_args!("command quarantine"));
    }
    action
}

fn modem_step_requires_quarantine(step: &ModemDriveStep) -> bool {
    let class = match step {
        ModemTimerDriveStep::Begin(begin) => match begin {
            ControllerModemTimerBegin::NotReady => ModemTimerTransitionClass::BeginNotReady,
            ControllerModemTimerBegin::Started => ModemTimerTransitionClass::BeginStarted,
            ControllerModemTimerBegin::StorageRejected(_)
            | ControllerModemTimerBegin::AlreadyActive => ModemTimerTransitionClass::BeginRejected,
        },
        ModemTimerDriveStep::Step(step) => match step {
            ControllerModemTimerStep::Recheck => ModemTimerTransitionClass::StepRecheck,
            ControllerModemTimerStep::RearmPending => ModemTimerTransitionClass::StepRearmPending,
            ControllerModemTimerStep::Idle
            | ControllerModemTimerStep::ExpirationPending(_)
            | ControllerModemTimerStep::Published(_)
            | ControllerModemTimerStep::Backpressured(_) => {
                ModemTimerTransitionClass::StepUnsupported
            }
        },
        ModemTimerDriveStep::Rearm(rearm) => match rearm {
            ControllerModemTimerRearm::Rearmed => ModemTimerTransitionClass::Rearmed,
            ControllerModemTimerRearm::StorageRejected(_) | ControllerModemTimerRearm::NotReady => {
                ModemTimerTransitionClass::RearmRejected
            }
        },
    };
    let quarantine = modem_timer_requires_quarantine(class);
    if quarantine {
        crate::diagnostics::record(
            crate::diagnostics::BluetoothExecutionEvent::Terminal,
            format_args!("modem timer quarantine: {:?}", class),
        );
    }
    quarantine
}

#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc command winner retains its exact affine lower owner"
)]
enum HardwareSelection<'packet, const SCHEDULER_CAPACITY: usize> {
    IdleRequested,
    MaintenanceWake,
    Command(CommandBoundary<'packet, SCHEDULER_CAPACITY>),
    ModemTimer(ModemDriveStep),
    InterruptFault(BluetoothInterruptFault),
}

enum RetryGateSelection {
    MaintenanceWake,
    RecheckCompleted,
    ModemTimer(ModemDriveStep),
    InterruptFault(BluetoothInterruptFault),
}

impl<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothHardwareRunner<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Install the independent SoC entropy service before polling the Host.
    ///
    /// This enables standard HCI LE Rand for this epoch. The caller-owned
    /// service outlives the runner, including Reset, radio maintenance and
    /// checked physical cold release/restart. Install once before the first
    /// Host, not again for each reconstructed Host or Controller generation.
    /// Rebinding or installation after bootstrap starts is rejected unchanged.
    pub fn install_entropy(
        &mut self,
        entropy: &'static crate::entropy::BluetoothEntropy<'static>,
    ) -> Result<(), oer_bluetooth_hci::LeRandomSourceAlreadyConfigured> {
        self.controller.install_random_source(entropy)
    }

    /// Set diagnostic thermal thresholds at the idle command boundary, preserving
    /// maintenance deadlines. Restore the returned setting after the experiment.
    pub fn set_idle_phy_tracking_debug(
        &mut self,
        debug: oer_esp32s31_phy::state::PhyTemperatureTrackingDebug,
    ) -> Result<
        oer_esp32s31_phy::state::PhyTemperatureTrackingDebug,
        oer_esp32s31_bluetooth_runtime::controller::ControllerCommandRetirementError,
    > {
        self.command.as_mut().ok_or(
            oer_esp32s31_bluetooth_runtime::controller::ControllerCommandRetirementError::OwnerUnavailable
        )?.set_idle_phy_tracking_debug(debug)
    }

    pub(super) fn new(
        task: oer_esp32s31_bluetooth::controller::ControllerIdleCommandTask<
            'static,
            PublishedStorage,
            SCHEDULER_CAPACITY,
        >,
        controller: LeControllerCommandEndpoint<
            'static,
            CriticalSectionRawMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        modem_timer: ControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>,
        interrupt: BluetoothInterruptRuntime,
        recheck: DtmAbsoluteRecheck,
        wakers: &'static RuntimeWakers,
        watchdog: &'static crate::WatchdogConfig,
    ) -> Self {
        let modem_driver = wakers.modem_timer().driver();
        Self {
            watchdog,
            restoration_protection: None,
            command: Some(ControllerCommandTask::new(task)),
            controller,
            modem_timer: Some(modem_timer),
            modem_driver,
            interrupt: Some(interrupt),
            packet: [0; PACKET_CAPACITY],
            recheck,
            advertising_delay: BluetoothAdvertisingDelaySource,
            wakers,
            schedule: HardwareRunnerSchedule::new(),
        }
    }

    /// Disable and rebind all IRQ routes while the runner is outside execution.
    ///
    /// The runner is available before execution or after `run_until_idle`. No
    /// command actor or modem-timer future is in flight at this boundary. Hardware and
    /// pending notifications stay with the same epoch; this neither stops DMA
    /// nor releases PHY ownership. Any sticky ISR fault survives the cycle.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "the failed no-alloc transition retains every affine runner owner"
    )]
    pub fn cycle_interrupt_routes(
        mut self,
    ) -> Result<
        Self,
        BluetoothHardwareRouteCycleFailure<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    > {
        let interrupt = self
            .interrupt
            .take()
            .expect("a returned runner retains its route owner");
        let disabled = match interrupt.disable() {
            Ok(disabled) => disabled,
            Err(failure) => {
                return Err(BluetoothHardwareRouteCycleFailure {
                    _runner: self,
                    _interrupt: RouteCycleFailure::Disable { _failure: failure },
                });
            }
        };
        match disabled.bind() {
            Ok(interrupt) => {
                self.interrupt = Some(interrupt);
                Ok(self)
            }
            Err(failure) => Err(BluetoothHardwareRouteCycleFailure {
                _runner: self,
                _interrupt: RouteCycleFailure::Bind { _failure: failure },
            }),
        }
    }

    /// Run the complete hardware side forever or retain a terminal quarantine.
    ///
    /// IRQ faults have strict polling priority. Command and source-127 work
    /// rotate their inner polling order, and every finite nonterminal result
    /// yields once before the next iteration. A retryable actor boundary cannot
    /// re-enter the actor until one absolute recheck has completed; timer work
    /// and IRQ faults remain serviceable while that gate is armed.
    ///
    /// Idle-restored command completion and the finite timer path through
    /// `Started`, `Recheck`, `RearmPending` and `Rearmed` continue internally.
    /// Every other command boundary, unsupported timer expiration/invariant,
    /// timeline exhaustion or ISR fault closes HCI, disables all three routes and retains
    /// the exact cause forever inside this future.
    pub async fn run(mut self) -> ! {
        self.drive_until_idle(core::future::pending(), false).await;
        unreachable!("the permanent runner has no stop request")
    }

    /// Service this epoch until a request resolves and all software work is idle.
    ///
    /// The request latches once. Active roles continue until the Host completes
    /// their normal Disconnect/Reset/disable path; the request issues no command.
    /// Accepted commands, outgoing packets and timer work continue to drain.
    /// The returned runner retains live IRQ routes and an open HCI channel;
    /// observations are not a shutdown barrier. Use `retire_modem_timer` and
    /// `try_retire_hci` to establish those barriers, handling a racing producer.
    ///
    /// As with `run`, keep this consuming future pinned until completion. Dropping
    /// it loses the affine runner; cancelling an individual readiness wait inside
    /// it does not. Terminal faults retain their existing fail-stop quarantine.
    pub async fn run_until_idle(mut self, request: impl core::future::Future<Output = ()>) -> Self {
        {
            let mut running = pin!(self.drive_until_idle(request, false));
            poll_fn(|cx| poll_idle_handoff(running.as_mut(), cx)).await;
        }
        self
    }

    fn software_idle(&self) -> bool {
        !self.schedule.retry_gate()
            && self.command.as_ref().expect("live actor").phase()
                == oer_esp32s31_bluetooth_runtime::controller::ControllerCommandPhase::Idle
            && self
                .modem_timer
                .as_ref()
                .expect("live timer")
                .retirement_ready()
    }

    async fn drive_until_idle(
        &mut self,
        request: impl core::future::Future<Output = ()>,
        maintenance_handoff: bool,
    ) -> bool {
        let mut request = core::pin::pin!(request);
        let mut requested = false;
        loop {
            if let Some(error) = self
                .command
                .as_ref()
                .expect("live actor")
                .phy_maintenance_error()
            {
                crate::diagnostics::record(
                    crate::diagnostics::BluetoothExecutionEvent::Terminal,
                    format_args!("PHY maintenance: {:?}", error),
                );
                self.controller.close_transport();
                fail_stop_shared_phy(terminal_maintenance_reason(error), self);
            }
            let hard_deadline = self
                .command
                .as_ref()
                .expect("live actor")
                .phy_maintenance_hard_deadline();
            let maintenance_wake = self
                .command
                .as_ref()
                .expect("live actor")
                .phy_maintenance_wake_at();
            if self.recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
                self.controller.close_transport();
                let routes = quarantine_routes(&mut self.interrupt);
                retain_until_rf_deadline(
                    hard_deadline,
                    BluetoothHardwareQuarantine::<SCHEDULER_CAPACITY>::ControllerTimeExhausted {
                        _routes: routes,
                    },
                )
                .await;
            }

            let primary_first = self.schedule.begin_iteration();

            if self.schedule.retry_gate() {
                let selection = {
                    let interrupt = self
                        .interrupt
                        .as_ref()
                        .expect("a retry gate retains one live interrupt epoch");
                    let interrupt_fault = interrupt.wait_fault();
                    let modem_driver = &self.modem_driver;
                    let modem_timer = self.modem_timer.as_mut().expect("live timer owner");
                    let modem = async {
                        let _ = modem_driver.wait_ready(&*modem_timer).await;
                        modem_driver.drive_once(modem_timer)
                    };
                    let recheck = self.recheck.wait_until_absolute_recheck();

                    if primary_first {
                        match select(
                            interrupt_fault,
                            select(
                                wait_maintenance_wake(maintenance_wake),
                                select(recheck, modem),
                            ),
                        )
                        .await
                        {
                            Either::First(fault) => RetryGateSelection::InterruptFault(fault),
                            Either::Second(Either::First(())) => {
                                RetryGateSelection::MaintenanceWake
                            }
                            Either::Second(Either::Second(Either::First(()))) => {
                                RetryGateSelection::RecheckCompleted
                            }
                            Either::Second(Either::Second(Either::Second(step))) => {
                                RetryGateSelection::ModemTimer(step)
                            }
                        }
                    } else {
                        match select(
                            interrupt_fault,
                            select(
                                wait_maintenance_wake(maintenance_wake),
                                select(modem, recheck),
                            ),
                        )
                        .await
                        {
                            Either::First(fault) => RetryGateSelection::InterruptFault(fault),
                            Either::Second(Either::First(())) => {
                                RetryGateSelection::MaintenanceWake
                            }
                            Either::Second(Either::Second(Either::First(step))) => {
                                RetryGateSelection::ModemTimer(step)
                            }
                            Either::Second(Either::Second(Either::Second(()))) => {
                                RetryGateSelection::RecheckCompleted
                            }
                        }
                    }
                };

                match selection {
                    RetryGateSelection::MaintenanceWake => {}
                    RetryGateSelection::RecheckCompleted => {
                        self.schedule.complete_recheck();
                        yield_now().await;
                    }
                    RetryGateSelection::ModemTimer(step) => {
                        if modem_step_requires_quarantine(&step) {
                            self.controller.close_transport();
                            let routes = quarantine_routes(&mut self.interrupt);
                            retain_until_rf_deadline(
                                hard_deadline,
                                BluetoothHardwareQuarantine::<SCHEDULER_CAPACITY>::ModemTimer {
                                    _step: step,
                                    _routes: routes,
                                },
                            )
                            .await;
                        }
                        yield_now().await;
                    }
                    RetryGateSelection::InterruptFault(fault) => {
                        crate::diagnostics::record(
                            crate::diagnostics::BluetoothExecutionEvent::Terminal,
                            format_args!("interrupt: {:?}", fault),
                        );
                        self.controller.close_transport();
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_until_rf_deadline(
                            hard_deadline,
                            BluetoothHardwareQuarantine::<SCHEDULER_CAPACITY>::InterruptFault {
                                _fault: fault,
                                _routes: routes,
                            },
                        )
                        .await;
                    }
                }
                continue;
            }

            let can_stop = self.software_idle();
            let selection = {
                let drained = self.controller.wait_retirement_ready();
                let mut stop = pin!(async {
                    if !requested {
                        request.as_mut().await;
                        requested = true;
                    }
                    if can_stop && drained.await.is_ok() {
                        return;
                    }
                    // Active work keeps running. On closed transport the command
                    // actor owns error classification and terminal quarantine.
                    core::future::pending::<()>().await;
                });
                let interrupt = self
                    .interrupt
                    .as_ref()
                    .expect("the live Controller loop retains its interrupt epoch");
                let mut interrupt_fault = pin!(interrupt.wait_fault());
                let mut maintenance_timer = pin!(wait_maintenance_wake(maintenance_wake));
                let modem_driver = &self.modem_driver;
                let modem_timer = self.modem_timer.as_mut().expect("live timer owner");
                let mut modem = pin!(async {
                    let _ = modem_driver.wait_ready(&*modem_timer).await;
                    modem_driver.drive_once(modem_timer)
                });
                let mut command = pin!(
                    self.command
                        .as_mut()
                        .expect("the live Controller loop retains its command actor")
                        .run(
                            self.wakers,
                            &mut self.controller,
                            &mut self.packet,
                            &mut self.recheck,
                            &mut self.advertising_delay,
                        )
                );

                // Poll directly into one owner-bearing result. Nested Select
                // enums multiply the large command boundary's stack temporaries.
                poll_fn(|cx| {
                    if let Poll::Ready(fault) = interrupt_fault.as_mut().poll(cx) {
                        return Poll::Ready(HardwareSelection::InterruptFault(fault));
                    }
                    if maintenance_timer.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(HardwareSelection::MaintenanceWake);
                    }
                    if stop.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(HardwareSelection::IdleRequested);
                    }
                    if primary_first {
                        if let Poll::Ready(boundary) = command.as_mut().poll(cx) {
                            return Poll::Ready(HardwareSelection::Command(boundary));
                        }
                        if let Poll::Ready(step) = modem.as_mut().poll(cx) {
                            return Poll::Ready(HardwareSelection::ModemTimer(step));
                        }
                    } else {
                        if let Poll::Ready(step) = modem.as_mut().poll(cx) {
                            return Poll::Ready(HardwareSelection::ModemTimer(step));
                        }
                        if let Poll::Ready(boundary) = command.as_mut().poll(cx) {
                            return Poll::Ready(HardwareSelection::Command(boundary));
                        }
                    }
                    Poll::Pending
                })
                .await
            };

            match selection {
                HardwareSelection::MaintenanceWake => {}
                HardwareSelection::Command(
                    ControllerCommandBoundary::PhyMaintenanceIdle
                    | ControllerCommandBoundary::PhyMaintenancePeripheral,
                ) if maintenance_handoff => {
                    if self
                        .modem_timer
                        .as_ref()
                        .expect("live timer")
                        .retirement_ready()
                    {
                        return true;
                    }
                    // Keep timer service live until its actual owner is drained.
                    yield_now().await;
                }
                HardwareSelection::IdleRequested => {
                    // A command future can advance an owner before returning
                    // Pending. Recheck after dropping every losing future.
                    if self.software_idle() {
                        return false;
                    }
                }
                HardwareSelection::Command(boundary) => {
                    match classify_command(
                        &boundary,
                        self.command
                            .as_ref()
                            .expect("live actor")
                            .advertising_completion(),
                        self.command
                            .as_ref()
                            .expect("live actor")
                            .advertising_rejected_packets(),
                        self.command
                            .as_ref()
                            .expect("live actor")
                            .advertising_last_receive_rejection(),
                    ) {
                        CommandBoundaryAction::Continue => yield_now().await,
                        CommandBoundaryAction::CompleteRestoration => {
                            // Actor resume alone is not proof: wait for the
                            // guarded RUN boundary or proven idle restoration.
                            if let Some(protection) = self.restoration_protection.take() {
                                crate::WatchdogConfig::complete(protection);
                            }
                            yield_now().await;
                        }
                        CommandBoundaryAction::GateRetry => {
                            self.schedule.arm_retry();
                            yield_now().await;
                        }
                        CommandBoundaryAction::FailStopSharedPhy(reason) => {
                            self.controller.close_transport();
                            // The boundary may borrow packet storage from `self`.
                            // The borrowed runner remains on this diverging frame;
                            // transfer only the boundary, never reconstruct owners.
                            fail_stop_shared_phy(reason, boundary);
                        }
                        CommandBoundaryAction::Quarantine => {
                            let actor = self
                                .command
                                .take()
                                .expect("terminal quarantine retains the exact command actor");
                            self.controller.close_transport();
                            let routes = quarantine_routes(&mut self.interrupt);
                            retain_until_rf_deadline(
                                hard_deadline,
                                BluetoothHardwareQuarantine::Command {
                                    _boundary: boundary,
                                    _actor: actor,
                                    _routes: routes,
                                },
                            )
                            .await;
                        }
                    }
                }
                HardwareSelection::ModemTimer(step) => {
                    if modem_step_requires_quarantine(&step) {
                        self.controller.close_transport();
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_until_rf_deadline(
                            hard_deadline,
                            BluetoothHardwareQuarantine::<SCHEDULER_CAPACITY>::ModemTimer {
                                _step: step,
                                _routes: routes,
                            },
                        )
                        .await;
                    }
                    yield_now().await;
                }
                HardwareSelection::InterruptFault(fault) => {
                    crate::diagnostics::record(
                        crate::diagnostics::BluetoothExecutionEvent::Terminal,
                        format_args!("interrupt fault: {:?}", fault),
                    );
                    self.controller.close_transport();
                    let routes = quarantine_routes(&mut self.interrupt);
                    retain_until_rf_deadline(
                        hard_deadline,
                        BluetoothHardwareQuarantine::<SCHEDULER_CAPACITY>::InterruptFault {
                            _fault: fault,
                            _routes: routes,
                        },
                    )
                    .await;
                }
            }
        }
    }
}

// Keep polling the borrowed actor separate from returning the complete runner.
// Otherwise fat LTO combines mutually exclusive command and handoff temporaries.
#[inline(never)]
fn poll_idle_handoff<F: Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> Poll<F::Output> {
    future.poll(cx)
}

async fn wait_maintenance_wake(at: Option<u64>) {
    match at {
        Some(at) => embassy_time::Timer::at(embassy_time::Instant::from_micros(at)).await,
        None => core::future::pending().await,
    }
}

// A terminal actor can still retain autonomous RF hardware. Its quarantine
// cannot discard the maintenance hard wake and leave RF running indefinitely.
async fn retain_until_rf_deadline<T>(hard_deadline: Option<u64>, owners: T) -> ! {
    if let Some(at) = hard_deadline {
        wait_maintenance_wake(Some(at)).await;
        fail_stop_shared_phy(
            oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop::MaintenanceHardDeadlineExceeded,
            owners,
        );
    }
    retain_quarantine_forever(owners).await
}
