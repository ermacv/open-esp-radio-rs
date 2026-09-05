//! Final runtime split and interrupt activation, retaining owners on failure.

use bt_hci::controller::ExternalController;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio_bluetooth_hci::LeControllerHciEndpoints;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerHciBound, BluetoothControllerModemTimerTask,
    BluetoothControllerPublishedRuntimeEndpoints, BluetoothControllerPublishedRuntimeSplit,
    BluetoothControllerPublishedRuntimeSplitFailure,
};
use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothDtmAbsoluteRecheck;

use super::{
    Esp32s31BluetoothHardwareRunner, Esp32s31BluetoothRunners, Esp32s31BluetoothSystem,
    PublishedStorage, RuntimeWakers,
};
use crate::{Esp32s31BluetoothInterruptBindError, bind_production_bluetooth_interrupt_runtime};

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
    owner: &'static mut BluetoothControllerHciBound<
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
