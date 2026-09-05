//! Final affine split from a published Controller into Host and hardware sides.
//!
//! Construction splits the published owners and activates the interrupt routes.
//! The runner retains one hardware epoch; quarantine keeps terminal owners alive.
//! Polling and retry decisions remain in the host-testable `runner_policy` module.

mod construction;
mod quarantine;
mod runner;

pub use construction::{
    Esp32s31BluetoothInterruptCompositionFailure, Esp32s31BluetoothSystemBuildError,
    compose_esp32s31_bluetooth_system,
};
pub use runner::Esp32s31BluetoothHardwareRunner;

use bt_hci::controller::ExternalController;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio_bluetooth_hci::InProcessHciHostTransport;
use open_esp_radio_esp32s31_bluetooth_embassy::{
    EmbassyBluetoothControllerCommandBoundary, EmbassyBluetoothModemTimerDriveStep,
    EmbassyBluetoothRuntimeWakers,
};
use open_esp_radio_esp32s31_radio_platform_esp_hal::{
    EspHalBluetoothModemLpTimerStorageError, PublishedEspHalBluetoothInterruptOwners,
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
