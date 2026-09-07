//! Final runtime split and interrupt activation, retaining owners on failure.

use bt_hci::controller::ExternalController;

use crate::{BluetoothInterruptBindError, bind_production_bluetooth_interrupt_runtime};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_bluetooth_hci::LeControllerHciEndpoints;

use oer_esp32s31_bluetooth::controller::{
    ControllerModemTimerTask, ControllerPublishedRuntimeEndpoints, ControllerPublishedRuntimeSplit,
    ControllerPublishedRuntimeSplitFailure, hci::ControllerHciBound,
};

use oer_esp32s31_bluetooth_embassy::controller::DtmAbsoluteRecheck;

use super::{
    BluetoothHardwareRunner, BluetoothRunners, BluetoothSystem, PublishedStorage, RuntimeWakers,
};

type PublishedSplitFailure<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = ControllerPublishedRuntimeSplitFailure<
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
> = ControllerPublishedRuntimeEndpoints<
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
pub struct BluetoothInterruptCompositionFailure<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    error: BluetoothInterruptBindError,
    _task: oer_esp32s31_bluetooth::controller::ControllerIdleCommandTask<
        'static,
        PublishedStorage,
        SCHEDULER_CAPACITY,
    >,
    _modem_timer: ControllerModemTimerTask<'static, PublishedStorage, MODEM_TIMER_CAPACITY>,
    _hci: LeControllerHciEndpoints<
        'static,
        CriticalSectionRawMutex,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _recheck: DtmAbsoluteRecheck,
}

impl<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothInterruptCompositionFailure<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Exact final route/dispatcher activation error.
    pub const fn error(&self) -> BluetoothInterruptBindError {
        self.error
    }
}

/// Why a published final Controller could not become a product-level system.
#[must_use = "a failed final split retains an opaque powered Controller owner"]
pub enum BluetoothSystemBuildError<
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
        BluetoothInterruptCompositionFailure<
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
    owner: &'static mut ControllerHciBound<
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
    recheck: DtmAbsoluteRecheck,
) -> Result<
    BluetoothSystem<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    BluetoothSystemBuildError<
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
        ControllerPublishedRuntimeSplit::Ready(endpoints) => endpoints,
        ControllerPublishedRuntimeSplit::CommandReadyUnavailable(failure) => {
            return Err(BluetoothSystemBuildError::RuntimeSplitUnavailable(failure));
        }
    };
    let ControllerPublishedRuntimeEndpoints {
        interrupt,
        task,
        modem_timer,
        hci,
    } = endpoints;
    let interrupt = match bind_production_bluetooth_interrupt_runtime(interrupt, wakers) {
        Ok(interrupt) => interrupt,
        Err(error) => {
            return Err(BluetoothSystemBuildError::InterruptComposition(
                BluetoothInterruptCompositionFailure {
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

    Ok(BluetoothSystem {
        hci: ExternalController::new(host),
        runners: BluetoothRunners {
            hardware: BluetoothHardwareRunner::new(
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
