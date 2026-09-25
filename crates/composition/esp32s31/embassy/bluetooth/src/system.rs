//! Final affine split from a published Controller into Host and hardware sides.
//!
//! Construction splits the published owners and activates the interrupt routes.
//! The runner retains one hardware epoch; quarantine keeps terminal owners alive.
//! Polling and retry decisions remain in the host-testable `runner_policy` module.

mod construction;
mod quarantine;
mod runner;
pub(crate) use runner::fail_stop_shared_phy;

pub use construction::{
    BluetoothInterruptCompositionFailure, BluetoothSystemBuildError,
    compose_esp32s31_bluetooth_system,
};

pub use runner::{
    BluetoothHardwareColdReleased, BluetoothHardwareInterruptsRetired,
    BluetoothHardwareMaintenanceError, BluetoothHardwareMaintenanceFailure,
    BluetoothHardwareOutputReleased, BluetoothHardwareRestartError,
    BluetoothHardwareRestartFailure, BluetoothHardwareRetired,
    BluetoothHardwareRetiredWithPlatform, BluetoothHardwareRouteCycleFailure,
    BluetoothHardwareRunner, BluetoothHardwareShutdownFailure, BluetoothHardwareTimerError,
    BluetoothHardwareTimerFailure, BluetoothHardwareTimerRetired, BluetoothPlatformJoin,
};

use bt_hci::controller::ExternalController;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_bluetooth_hci::InProcessHciHostTransport;
use oer_bluetooth_hci::LeHostAclCreditSender;

use oer_esp32s31_bluetooth_runtime::{
    controller::{ControllerCommandBoundary, ModemTimerDriveStep},
    notification::RuntimeNotifications,
};

use oer_esp32s31_radio_platform_esp_hal::{
    EspHalBluetoothModemLpTimerStorageError, PublishedEspHalBluetoothInterruptOwners,
};

use trouble_host::{HostResources, PacketPool};

use crate::{BluetoothTroubleSystem, trouble::compose_trouble_bluetooth_system};

type PublishedStorage = PublishedEspHalBluetoothInterruptOwners;
type RuntimeWakers = RuntimeNotifications<CriticalSectionRawMutex>;

type CommandBoundary<'packet, const SCHEDULER_CAPACITY: usize> =
    ControllerCommandBoundary<'static, 'static, 'packet, PublishedStorage, SCHEDULER_CAPACITY>;

type ModemDriveStep = ModemTimerDriveStep<
    EspHalBluetoothModemLpTimerStorageError,
    EspHalBluetoothModemLpTimerStorageError,
>;

/// Standard `bt-hci` Host facade backed by the source-owned in-process transport.
pub type BluetoothHostController<
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

/// Restricted Host authority for the response-less ACL credit command.
pub type BluetoothHostAclCredits<
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = LeHostAclCreditSender<
    'static,
    CriticalSectionRawMutex,
    HOST_TO_CONTROLLER_DEPTH,
    PACKET_CAPACITY,
>;

/// Product-level Bluetooth composition with a standard Host facade and one
/// affine hardware runner.
#[must_use = "the Host facade and hardware runner belong to one Controller epoch"]
pub struct BluetoothSystem<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    /// Standard `bt-hci` Controller consumed by a Host stack such as `bt-host`.
    pub hci: BluetoothHostController<
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    /// Host authority for returning Controller-to-Host ACL packet credits.
    pub host_acl_credits: BluetoothHostAclCredits<HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    /// All executor-side owners for this exact Controller epoch.
    pub runners: BluetoothRunners<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Named runner aggregate matching the product-level Wi-Fi composition shape.
#[must_use = "spawn or retain every hardware runner"]
pub struct BluetoothRunners<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    /// Sole owner of command, timer, IRQ and Controller transport work.
    pub hardware: BluetoothHardwareRunner<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

impl<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothSystem<
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Consume this exact Controller epoch into the pinned Trouble Host stack.
    ///
    /// The returned `stack.runner()` and `hardware.run()` futures must be
    /// polled concurrently. Peripheral and GATT application handles are
    /// obtained directly from the returned Trouble stack.
    pub fn into_trouble<
        'resources,
        P: PacketPool,
        const CONNECTIONS: usize,
        const L2CAP_CHANNELS: usize,
        const ADVERTISING_SETS: usize,
        const BONDS: usize,
    >(
        self,
        resources: &'resources mut HostResources<
            P,
            CONNECTIONS,
            L2CAP_CHANNELS,
            ADVERTISING_SETS,
            BONDS,
        >,
    ) -> BluetoothTroubleSystem<
        'resources,
        BluetoothHostController<
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        P,
        BluetoothHardwareRunner<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    > {
        let BluetoothSystem {
            hci,
            host_acl_credits: _,
            runners: BluetoothRunners { hardware },
        } = self;
        compose_trouble_bluetooth_system(hci, hardware, resources)
    }
}
