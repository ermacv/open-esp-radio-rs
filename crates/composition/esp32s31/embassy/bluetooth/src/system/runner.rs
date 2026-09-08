//! One hardware owner and its fair command/timer loop under IRQ fault priority.
//!
//! Boundary classification translates concrete runtime outcomes into the shared
//! runner policy. The loop retains all borrowed owners across the same awaits.

use crate::{
    BluetoothInterruptFault, BluetoothInterruptRuntime,
    runner_policy::{
        CommandBoundaryAction, CommandBoundaryClass, HardwareRunnerSchedule,
        ModemTimerTransitionClass, modem_timer_requires_quarantine, reduce_command_boundary,
    },
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

use oer_esp32s31_bluetooth_embassy::{
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
    controller: LeControllerCommandEndpoint<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    modem_timer: ControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>,
    modem_driver: ModemTimerDriver<'static, CriticalSectionRawMutex>,
    interrupt: Option<BluetoothInterruptRuntime>,
    packet: [u8; PACKET_CAPACITY],
    recheck: DtmAbsoluteRecheck,
    advertising_delay: BluetoothAdvertisingDelaySource,
    wakers: &'static RuntimeWakers,
    schedule: HardwareRunnerSchedule,
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
        ControllerCommandBoundary::LegacyConnectableAdvertisingActive => record(
            Observed::ConnectableAdvertisingRun,
            format_args!(
                "advertising RUN; prev={:?}; rejected={:?}; last={:?}",
                advertising_completion,
                advertising_rejected_packets,
                advertising_last_receive_rejection
            ),
        ),
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
        ControllerCommandBoundary::Retryable(_) => CommandBoundaryClass::Retryable,
        ControllerCommandBoundary::NonCommand(_)
        | ControllerCommandBoundary::EndpointMismatch
        | ControllerCommandBoundary::HciFault(_)
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
        | ControllerCommandBoundary::PeripheralConnectionActiveFailStop(_)
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
    Command(CommandBoundary<'packet, SCHEDULER_CAPACITY>),
    ModemTimer(ModemDriveStep),
    InterruptFault(BluetoothInterruptFault),
}

enum RetryGateSelection {
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
    ) -> Self {
        let modem_driver = wakers.modem_timer().driver();
        Self {
            command: Some(ControllerCommandTask::new(task)),
            controller,
            modem_timer,
            modem_driver,
            interrupt: Some(interrupt),
            packet: [0; PACKET_CAPACITY],
            recheck,
            advertising_delay: BluetoothAdvertisingDelaySource,
            wakers,
            schedule: HardwareRunnerSchedule::new(),
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
    /// timeline exhaustion or ISR fault disables all three routes and retains
    /// the exact cause forever inside this future.
    pub async fn run(mut self) -> ! {
        loop {
            if self.recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
                let routes = quarantine_routes(&mut self.interrupt);
                retain_quarantine_forever(
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
                    let modem_timer = &mut self.modem_timer;
                    let modem = async {
                        let _ = modem_driver.wait_ready(&*modem_timer).await;
                        modem_driver.drive_once(modem_timer)
                    };
                    let recheck = self.recheck.wait_until_absolute_recheck();

                    if primary_first {
                        match select(interrupt_fault, select(recheck, modem)).await {
                            Either::First(fault) => RetryGateSelection::InterruptFault(fault),
                            Either::Second(Either::First(())) => {
                                RetryGateSelection::RecheckCompleted
                            }
                            Either::Second(Either::Second(step)) => {
                                RetryGateSelection::ModemTimer(step)
                            }
                        }
                    } else {
                        match select(interrupt_fault, select(modem, recheck)).await {
                            Either::First(fault) => RetryGateSelection::InterruptFault(fault),
                            Either::Second(Either::First(step)) => {
                                RetryGateSelection::ModemTimer(step)
                            }
                            Either::Second(Either::Second(())) => {
                                RetryGateSelection::RecheckCompleted
                            }
                        }
                    }
                };

                match selection {
                    RetryGateSelection::RecheckCompleted => {
                        self.schedule.complete_recheck();
                        yield_now().await;
                    }
                    RetryGateSelection::ModemTimer(step) => {
                        if modem_step_requires_quarantine(&step) {
                            let routes = quarantine_routes(&mut self.interrupt);
                            retain_quarantine_forever(BluetoothHardwareQuarantine::<
                                SCHEDULER_CAPACITY,
                            >::ModemTimer {
                                _step: step,
                                _routes: routes,
                            })
                            .await;
                        }
                        yield_now().await;
                    }
                    RetryGateSelection::InterruptFault(fault) => {
                        crate::diagnostics::record(
                            crate::diagnostics::BluetoothExecutionEvent::Terminal,
                            format_args!("interrupt: {:?}", fault),
                        );
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_quarantine_forever(BluetoothHardwareQuarantine::<
                            SCHEDULER_CAPACITY,
                        >::InterruptFault {
                            _fault: fault,
                            _routes: routes,
                        })
                        .await;
                    }
                }
                continue;
            }

            let selection = {
                let interrupt = self
                    .interrupt
                    .as_ref()
                    .expect("the live Controller loop retains its interrupt epoch");
                let interrupt_fault = interrupt.wait_fault();
                let modem_driver = &self.modem_driver;
                let modem_timer = &mut self.modem_timer;
                let modem = async {
                    let _ = modem_driver.wait_ready(&*modem_timer).await;
                    modem_driver.drive_once(modem_timer)
                };
                let command = self
                    .command
                    .as_mut()
                    .expect("the live Controller loop retains its command actor")
                    .run(
                        self.wakers,
                        &mut self.controller,
                        &mut self.packet,
                        &mut self.recheck,
                        &mut self.advertising_delay,
                    );

                if primary_first {
                    match select(interrupt_fault, select(command, modem)).await {
                        Either::First(fault) => HardwareSelection::InterruptFault(fault),
                        Either::Second(Either::First(boundary)) => {
                            HardwareSelection::Command(boundary)
                        }
                        Either::Second(Either::Second(step)) => HardwareSelection::ModemTimer(step),
                    }
                } else {
                    match select(interrupt_fault, select(modem, command)).await {
                        Either::First(fault) => HardwareSelection::InterruptFault(fault),
                        Either::Second(Either::First(step)) => HardwareSelection::ModemTimer(step),
                        Either::Second(Either::Second(boundary)) => {
                            HardwareSelection::Command(boundary)
                        }
                    }
                }
            };

            match selection {
                HardwareSelection::Command(boundary) => match classify_command(
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
                    CommandBoundaryAction::GateRetry => {
                        self.schedule.arm_retry();
                        yield_now().await;
                    }
                    CommandBoundaryAction::Quarantine => {
                        let actor = self
                            .command
                            .take()
                            .expect("terminal quarantine retains the exact command actor");
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_quarantine_forever(BluetoothHardwareQuarantine::Command {
                            _boundary: boundary,
                            _actor: actor,
                            _routes: routes,
                        })
                        .await;
                    }
                },
                HardwareSelection::ModemTimer(step) => {
                    if modem_step_requires_quarantine(&step) {
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_quarantine_forever(BluetoothHardwareQuarantine::<
                            SCHEDULER_CAPACITY,
                        >::ModemTimer {
                            _step: step,
                            _routes: routes,
                        })
                        .await;
                    }
                    yield_now().await;
                }
                HardwareSelection::InterruptFault(fault) => {
                    crate::diagnostics::record(
                        crate::diagnostics::BluetoothExecutionEvent::Terminal,
                        format_args!("interrupt fault: {:?}", fault),
                    );
                    let routes = quarantine_routes(&mut self.interrupt);
                    retain_quarantine_forever(
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
