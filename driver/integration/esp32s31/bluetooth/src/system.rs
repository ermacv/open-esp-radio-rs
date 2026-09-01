//! Final affine split from a published Controller into Host and hardware sides.

use bt_hci::controller::ExternalController;
use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::rng::Rng;
use open_esp_radio_bluetooth_hci::{
    InProcessHciHostTransport, LeControllerCommandEndpoint, LeControllerHciEndpoints,
};
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerInterruptOwnersPublished, BluetoothControllerModemTimerBegin,
    BluetoothControllerModemTimerRearm, BluetoothControllerModemTimerStep,
    BluetoothControllerModemTimerTask, BluetoothControllerPublishedRuntimeEndpoints,
    BluetoothControllerPublishedRuntimeSplit, BluetoothControllerPublishedRuntimeSplitFailure,
};
use open_esp_radio_esp32s31_bluetooth_embassy::{
    EmbassyBluetoothControllerCommandBoundary, EmbassyBluetoothControllerCommandTask,
    EmbassyBluetoothDtmAbsoluteRecheck, EmbassyBluetoothDtmControllerTimeRecheck,
    EmbassyBluetoothDtmControllerTimeRecheckStatus, EmbassyBluetoothLegacyAdvertisingDelaySource,
    EmbassyBluetoothModemTimerDriveStep, EmbassyBluetoothModemTimerDriver,
    EmbassyBluetoothRuntimeWakers,
};
use open_esp_radio_esp32s31_radio_platform_esp_hal::{
    EspHalBluetoothModemLpTimerStorageError, PublishedEspHalBluetoothInterruptOwners,
};

use crate::{
    Esp32s31BluetoothInterruptBindError, Esp32s31BluetoothInterruptDisableFailure,
    Esp32s31BluetoothInterruptFault, Esp32s31BluetoothInterruptRuntime,
    bind_production_bluetooth_interrupt_runtime,
    runner_policy::{
        CommandBoundaryAction, CommandBoundaryClass, HardwareRunnerSchedule,
        ModemTimerTransitionClass, modem_timer_requires_quarantine, reduce_command_boundary,
    },
};

type PublishedStorage = PublishedEspHalBluetoothInterruptOwners;
type RuntimeWakers = EmbassyBluetoothRuntimeWakers<CriticalSectionRawMutex>;

type CommandBoundary<'packet, const SCHEDULER_CAPACITY: usize> =
    EmbassyBluetoothControllerCommandBoundary<
        'static,
        'static,
        'packet,
        PublishedStorage,
        SCHEDULER_CAPACITY,
    >;

type ModemDriveStep = EmbassyBluetoothModemTimerDriveStep<
    EspHalBluetoothModemLpTimerStorageError,
    EspHalBluetoothModemLpTimerStorageError,
>;

struct Esp32s31BluetoothAdvertisingDelaySource;

impl EmbassyBluetoothLegacyAdvertisingDelaySource for Esp32s31BluetoothAdvertisingDelaySource {
    fn next_advertising_delay(&mut self) -> AdvertisingDelay {
        let micros = (Rng::new().random() % (u32::from(AdvertisingDelay::MAX_MICROS) + 1)) as u16;
        AdvertisingDelay::from_micros(micros)
            .expect("the hardware entropy projection is inside the Link Layer domain")
    }
}

/// Standard `bt-hci` Host facade backed by the source-owned in-process transport.
pub type Esp32s31BluetoothHostController<
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = ExternalController<
    InProcessHciHostTransport<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    1,
>;

type PublishedSplitFailure<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = BluetoothControllerPublishedRuntimeSplitFailure<
    'static,
    CriticalSectionRawMutex,
    PublishedStorage,
    MODEM_TIMER_CAPACITY,
    SCHEDULER_CAPACITY,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

type PublishedEndpoints<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = BluetoothControllerPublishedRuntimeEndpoints<
    'static,
    CriticalSectionRawMutex,
    PublishedStorage,
    MODEM_TIMER_CAPACITY,
    SCHEDULER_CAPACITY,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

/// Product-level Bluetooth composition with a standard Host facade and one
/// affine hardware runner.
#[must_use = "the Host facade and hardware runner belong to one Controller epoch"]
pub struct Esp32s31BluetoothSystem<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    /// Standard `bt-hci` Controller consumed by a Host stack such as `bt-host`.
    pub hci: Esp32s31BluetoothHostController<
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    /// All executor-side owners for this exact Controller epoch.
    pub runners: Esp32s31BluetoothRunners<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Named runner aggregate matching the product-level Wi-Fi composition shape.
#[must_use = "spawn or retain every hardware runner"]
pub struct Esp32s31BluetoothRunners<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    /// Sole owner of command, timer, IRQ and Controller transport work.
    pub hardware: Esp32s31BluetoothHardwareRunner<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Result of disabling all three routes for terminal quarantine.
enum Esp32s31BluetoothRouteQuarantine {
    /// Terminal quarantine disabled source 124, 127 and 133 together.
    Disabled,
    /// Full-route disable was rejected; quarantine owns the unchanged live epoch.
    DisableRejected {
        _failure: Esp32s31BluetoothInterruptDisableFailure,
    },
}

/// Terminal owner retained forever after complete-route quarantine.
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc quarantine retains exact affine lower owners"
)]
enum Esp32s31BluetoothHardwareQuarantine<'packet, const SCHEDULER_CAPACITY: usize> {
    Command {
        _boundary: CommandBoundary<'packet, SCHEDULER_CAPACITY>,
        _actor:
            EmbassyBluetoothControllerCommandTask<'static, PublishedStorage, SCHEDULER_CAPACITY>,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    ModemTimer {
        _step: ModemDriveStep,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    InterruptFault {
        _fault: Esp32s31BluetoothInterruptFault,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    ControllerTimeExhausted {
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
}

async fn retain_quarantine_forever<T>(_quarantine: T) -> ! {
    core::future::pending().await
}

/// Sole hardware-side owner after the final Controller split.
///
/// `run` services the command actor and source-127 task fairly under strict IRQ
/// fault priority. Terminal command owners, unsupported expirations and exact
/// faults are retained forever only after the complete route set is disabled
/// or its exact disable rejection is retained for fail-stop quarantine.
#[must_use = "the hardware runner owns the live Controller epoch"]
pub struct Esp32s31BluetoothHardwareRunner<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    command: Option<
        EmbassyBluetoothControllerCommandTask<'static, PublishedStorage, SCHEDULER_CAPACITY>,
    >,
    controller: LeControllerCommandEndpoint<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    modem_timer: BluetoothControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>,
    modem_driver: EmbassyBluetoothModemTimerDriver<'static, CriticalSectionRawMutex>,
    interrupt: Option<Esp32s31BluetoothInterruptRuntime>,
    packet: [u8; PACKET_CAPACITY],
    recheck: EmbassyBluetoothDtmAbsoluteRecheck,
    advertising_delay: Esp32s31BluetoothAdvertisingDelaySource,
    wakers: &'static RuntimeWakers,
    schedule: HardwareRunnerSchedule,
}

fn classify_command<const SCHEDULER_CAPACITY: usize>(
    boundary: &CommandBoundary<'_, SCHEDULER_CAPACITY>,
) -> CommandBoundaryAction {
    let class = match boundary {
        EmbassyBluetoothControllerCommandBoundary::IdleRestored(_) => {
            CommandBoundaryClass::IdleRestored
        }
        EmbassyBluetoothControllerCommandBoundary::Retryable(_) => CommandBoundaryClass::Retryable,
        EmbassyBluetoothControllerCommandBoundary::NonCommand(_)
        | EmbassyBluetoothControllerCommandBoundary::EndpointMismatch
        | EmbassyBluetoothControllerCommandBoundary::HciFault(_)
        | EmbassyBluetoothControllerCommandBoundary::ControllerTimeExhausted
        | EmbassyBluetoothControllerCommandBoundary::FirstEventFailed(_)
        | EmbassyBluetoothControllerCommandBoundary::FirstPreparationCleanupFault { .. }
        | EmbassyBluetoothControllerCommandBoundary::FirstPreparationRestoreRejected(_)
        | EmbassyBluetoothControllerCommandBoundary::FirstPreparationFailStop(_)
        | EmbassyBluetoothControllerCommandBoundary::IdleCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::ActiveCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::PendingRadioFault(_)
        | EmbassyBluetoothControllerCommandBoundary::CommandReadyRadioFault(_)
        | EmbassyBluetoothControllerCommandBoundary::TestEndStoppingFault(_)
        | EmbassyBluetoothControllerCommandBoundary::ResetStoppingFault(_) => {
            CommandBoundaryClass::Terminal
        }
        EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingActive(_)
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanningActive
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanMalformedPdu(_)
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanReportEncodingFault(_) => {
            CommandBoundaryClass::Progress
        }
        EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingActiveCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanCommandEndpointMismatch(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingFault(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingPendingFault(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingStoppingFault(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringStopFault(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringFault(_)
        | EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingSequenceExhausted(_)
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanFault(_)
        | EmbassyBluetoothControllerCommandBoundary::PassiveScanRecurringFault(_) => {
            CommandBoundaryClass::Terminal
        }
        EmbassyBluetoothControllerCommandBoundary::UnownedFinishedList(_) => {
            CommandBoundaryClass::UnownedFinishedList
        }
    };
    reduce_command_boundary(class)
}

fn modem_step_requires_quarantine(step: &ModemDriveStep) -> bool {
    let class = match step {
        EmbassyBluetoothModemTimerDriveStep::Begin(begin) => match begin {
            BluetoothControllerModemTimerBegin::NotReady => {
                ModemTimerTransitionClass::BeginNotReady
            }
            BluetoothControllerModemTimerBegin::Started => ModemTimerTransitionClass::BeginStarted,
            BluetoothControllerModemTimerBegin::StorageRejected(_)
            | BluetoothControllerModemTimerBegin::AlreadyActive => {
                ModemTimerTransitionClass::BeginRejected
            }
        },
        EmbassyBluetoothModemTimerDriveStep::Step(step) => match step {
            BluetoothControllerModemTimerStep::Recheck => ModemTimerTransitionClass::StepRecheck,
            BluetoothControllerModemTimerStep::RearmPending => {
                ModemTimerTransitionClass::StepRearmPending
            }
            BluetoothControllerModemTimerStep::Idle
            | BluetoothControllerModemTimerStep::ExpirationPending(_)
            | BluetoothControllerModemTimerStep::Published(_)
            | BluetoothControllerModemTimerStep::Backpressured(_) => {
                ModemTimerTransitionClass::StepUnsupported
            }
        },
        EmbassyBluetoothModemTimerDriveStep::Rearm(rearm) => match rearm {
            BluetoothControllerModemTimerRearm::Rearmed => ModemTimerTransitionClass::Rearmed,
            BluetoothControllerModemTimerRearm::StorageRejected(_)
            | BluetoothControllerModemTimerRearm::NotReady => {
                ModemTimerTransitionClass::RearmRejected
            }
        },
    };
    modem_timer_requires_quarantine(class)
}

fn quarantine_routes(
    interrupt: &mut Option<Esp32s31BluetoothInterruptRuntime>,
) -> Esp32s31BluetoothRouteQuarantine {
    let runtime = interrupt
        .take()
        .expect("terminal quarantine starts from one live route epoch");
    match runtime.disable() {
        Ok(()) => Esp32s31BluetoothRouteQuarantine::Disabled,
        Err(failure) => Esp32s31BluetoothRouteQuarantine::DisableRejected { _failure: failure },
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc command winner retains its exact affine lower owner"
)]
enum HardwareSelection<'packet, const SCHEDULER_CAPACITY: usize> {
    Command(CommandBoundary<'packet, SCHEDULER_CAPACITY>),
    ModemTimer(ModemDriveStep),
    InterruptFault(Esp32s31BluetoothInterruptFault),
}

enum RetryGateSelection {
    RecheckCompleted,
    ModemTimer(ModemDriveStep),
    InterruptFault(Esp32s31BluetoothInterruptFault),
}

impl<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    Esp32s31BluetoothHardwareRunner<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    fn new(
        task: open_esp_radio_esp32s31_bluetooth::BluetoothControllerIdleCommandTask<
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
        modem_timer: BluetoothControllerModemTimerTask<
            'static,
            PublishedStorage,
            MODEM_TIMER_CAPACITY,
        >,
        interrupt: Esp32s31BluetoothInterruptRuntime,
        recheck: EmbassyBluetoothDtmAbsoluteRecheck,
        wakers: &'static RuntimeWakers,
    ) -> Self {
        let modem_driver = wakers.modem_timer().driver();
        Self {
            command: Some(EmbassyBluetoothControllerCommandTask::new(task)),
            controller,
            modem_timer,
            modem_driver,
            interrupt: Some(interrupt),
            packet: [0; PACKET_CAPACITY],
            recheck,
            advertising_delay: Esp32s31BluetoothAdvertisingDelaySource,
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
            if self.recheck.status()
                == EmbassyBluetoothDtmControllerTimeRecheckStatus::TimelineExhausted
            {
                let routes = quarantine_routes(&mut self.interrupt);
                retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::<
                    SCHEDULER_CAPACITY,
                >::ControllerTimeExhausted {
                    _routes: routes,
                })
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
                            retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::<
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
                        let routes = quarantine_routes(&mut self.interrupt);
                        retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::<
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
                HardwareSelection::Command(boundary) => match classify_command(&boundary) {
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
                        retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::Command {
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
                        retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::<
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
                    let routes = quarantine_routes(&mut self.interrupt);
                    retain_quarantine_forever(Esp32s31BluetoothHardwareQuarantine::<
                        SCHEDULER_CAPACITY,
                    >::InterruptFault {
                        _fault: fault,
                        _routes: routes,
                    })
                    .await;
                }
            }
        }
    }
}

/// Opaque fail-stop result after the final split succeeded but IRQ activation
/// failed. Every still-returnable task/HCI owner and the recheck schedule stay
/// retained here; the interrupt service itself remains in its one-shot stable
/// integration storage.
#[must_use = "a failed final composition retains the remaining Controller owners"]
pub struct Esp32s31BluetoothInterruptCompositionFailure<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    error: Esp32s31BluetoothInterruptBindError,
    _task: open_esp_radio_esp32s31_bluetooth::BluetoothControllerIdleCommandTask<
        'static,
        PublishedStorage,
        SCHEDULER_CAPACITY,
    >,
    _modem_timer:
        BluetoothControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>,
    _hci: LeControllerHciEndpoints<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _recheck: EmbassyBluetoothDtmAbsoluteRecheck,
}

impl<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    Esp32s31BluetoothInterruptCompositionFailure<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Exact final route/dispatcher activation error.
    pub const fn error(&self) -> Esp32s31BluetoothInterruptBindError {
        self.error
    }
}

/// Why a published final Controller could not become a product-level system.
#[must_use = "a failed final split retains an opaque powered Controller owner"]
pub enum Esp32s31BluetoothSystemBuildError<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    /// Initial command-ready authority was already unavailable.
    RuntimeSplitUnavailable(
        PublishedSplitFailure<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
    /// Stable full-service dispatch was placed, but route binding failed.
    InterruptComposition(
        Esp32s31BluetoothInterruptCompositionFailure<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
}

/// Split one statically retained final Controller exactly once and expose the
/// standard Host facade plus its sole hardware runner.
#[expect(
    clippy::result_large_err,
    reason = "no-alloc construction failures must retain exact affine Controller owners"
)]
pub fn compose_esp32s31_bluetooth_system<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>(
    owner: &'static mut BluetoothControllerInterruptOwnersPublished<
        P,
        CriticalSectionRawMutex,
        PublishedStorage,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    wakers: &'static RuntimeWakers,
    recheck: EmbassyBluetoothDtmAbsoluteRecheck,
) -> Result<
    Esp32s31BluetoothSystem<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    Esp32s31BluetoothSystemBuildError<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
> {
    let endpoints: PublishedEndpoints<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > = match owner.split_runtime() {
        BluetoothControllerPublishedRuntimeSplit::Ready(endpoints) => endpoints,
        BluetoothControllerPublishedRuntimeSplit::CommandReadyUnavailable(failure) => {
            return Err(Esp32s31BluetoothSystemBuildError::RuntimeSplitUnavailable(
                failure,
            ));
        }
    };
    let BluetoothControllerPublishedRuntimeEndpoints {
        interrupt,
        task,
        modem_timer,
        hci,
    } = endpoints;
    let interrupt = match bind_production_bluetooth_interrupt_runtime(interrupt, wakers) {
        Ok(interrupt) => interrupt,
        Err(error) => {
            return Err(Esp32s31BluetoothSystemBuildError::InterruptComposition(
                Esp32s31BluetoothInterruptCompositionFailure {
                    error,
                    _task: task,
                    _modem_timer: modem_timer,
                    _hci: hci,
                    _recheck: recheck,
                },
            ));
        }
    };
    let LeControllerHciEndpoints { host, controller } = hci;

    Ok(Esp32s31BluetoothSystem {
        hci: ExternalController::new(host),
        runners: Esp32s31BluetoothRunners {
            hardware: Esp32s31BluetoothHardwareRunner::new(
                task,
                controller,
                modem_timer,
                interrupt,
                recheck,
                wakers,
            ),
        },
    })
}
