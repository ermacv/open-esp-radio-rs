//! The HCI Controller of the composition: in-process transport, Controller
//! core and service loop over the radio runtime.
//!
//! The transport and the core live for the whole boot. Each Controller epoch
//! serves one Host epoch: [`BluetoothHciService::retire`] closes the old
//! Host end once both directions are drained, and
//! [`BluetoothHciService::restart`] rebuilds the core and opens a new Host end
//! on the same storage. Handles of a retired Host epoch stay closed.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_bluetooth_controller::{LeController, LeControllerConfig, LeVersionInformation};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeRandomSource, LeRandomUnavailable,
};
use oer_bluetooth_hci_transport::{
    HciRestartError, HciRetired, HciRetirementError, InProcessHciControllerTransport,
    InProcessHciHostTransport, LeControllerHciEndpoints, LeControllerHciResources,
};
use oer_bluetooth_runtime::{ServeExit, serve};
use oer_esp32s31_bluetooth_memory::BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY;
use oer_esp32s31_bluetooth_runtime::BluetoothRuntimeError;
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
use oer_time_embassy::EmbassyClock;
use static_cell::StaticCell;

use crate::system::BluetoothSystemRuntime;

/// Host-to-Controller packet slots; they also bound the Host's ACL credits.
pub const HOST_TO_CONTROLLER: usize = 4;
/// Controller-to-Host packet slots.
pub const CONTROLLER_TO_HOST: usize = 8;
/// Largest HCI packet body: a 251-octet ACL payload and its header.
pub const PACKET: usize = 258;
/// Controller-to-Host packets the core queues.
pub const OUTPUT: usize = 12;
/// LE ACL data length reported to the Host.
const ACL_DATA_LENGTH: u16 = 251;

/// Host end of the Controller's HCI transport.
pub type BluetoothHostTransport = InProcessHciHostTransport<
    'static,
    CriticalSectionRawMutex,
    HOST_TO_CONTROLLER,
    CONTROLLER_TO_HOST,
    PACKET,
>;

type Resources = LeControllerHciResources<
    CriticalSectionRawMutex,
    HOST_TO_CONTROLLER,
    CONTROLLER_TO_HOST,
    PACKET,
>;
type ControllerTransport = InProcessHciControllerTransport<
    'static,
    CriticalSectionRawMutex,
    HOST_TO_CONTROLLER,
    CONTROLLER_TO_HOST,
    PACKET,
>;

static RESOURCES: StaticCell<Resources> = StaticCell::new();
static CORE: StaticCell<LeController<'static, OUTPUT>> = StaticCell::new();

/// The SoC entropy service as the Controller's random source.
pub struct BluetoothEntropy<'d> {
    entropy: Entropy<'d>,
}

impl<'d> BluetoothEntropy<'d> {
    /// Bind an entropy service the caller keeps for the Controller's life.
    pub const fn new(entropy: Entropy<'d>) -> Self {
        Self { entropy }
    }
}

impl LeRandomSource for BluetoothEntropy<'_> {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        self.entropy.random_bytes().map_err(|_| LeRandomUnavailable)
    }
}

/// The HCI Controller: the Host's transport end and the service to run.
pub struct BluetoothHci {
    /// Hand this to the Host stack.
    pub host: BluetoothHostTransport,
    /// Spawn [`BluetoothHciService::run`] on one task.
    pub service: BluetoothHciService,
}

/// Serves the Host over the radio runtime.
pub struct BluetoothHciService {
    transport: ControllerTransport,
    core: &'static mut LeController<'static, OUTPUT>,
    runtime: &'static BluetoothSystemRuntime,
    config: LeControllerConfig,
    random: &'static (dyn LeRandomSource + 'static),
    retired: Option<HciRetired<'static>>,
}

/// Why the Host end could not be restarted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHciRestartError {
    /// No retired Host epoch awaits a restart.
    NotRetired,
    /// The transport refused the restart.
    Transport(HciRestartError),
}

impl BluetoothHciService {
    /// Serve until the transport closes or the radio fails.
    ///
    /// Cancelling it leaves the core mid-request; only [`Self::restart`]
    /// makes the core serve again, after the Host epoch was retired.
    pub async fn run(&mut self) -> ServeExit<BluetoothRuntimeError> {
        serve(&self.transport, self.core, self.runtime, &EmbassyClock).await
    }

    /// Wait until the Host and the service drained both directions of the
    /// current Host epoch, or report that the transport closed.
    pub async fn wait_retirement_ready(&self) -> Result<(), HciRetirementError> {
        self.transport.wait_retirement_ready().await
    }

    /// Close the current Host epoch once both directions are empty. Every
    /// handle of that epoch then reports the transport closed.
    ///
    /// # Errors
    ///
    /// A direction still holds a packet, or the transport closed terminally.
    pub fn retire(&mut self) -> Result<(), HciRetirementError> {
        let retired = self.transport.try_retire()?;
        self.retired = Some(retired);
        Ok(())
    }

    /// Open a new Host epoch on the same storage and give it a fresh
    /// Controller core.
    ///
    /// # Errors
    ///
    /// No Host epoch was retired, or the transport refused the restart; the
    /// retired epoch stays pending.
    pub fn restart(&mut self) -> Result<BluetoothHostTransport, BluetoothHciRestartError> {
        let retired = self
            .retired
            .take()
            .ok_or(BluetoothHciRestartError::NotRetired)?;
        match self.transport.restart(retired) {
            Ok(host) => {
                reset_core(self.core, self.config, self.random);
                Ok(host)
            }
            Err((error, retired)) => {
                self.retired = Some(retired);
                Err(BluetoothHciRestartError::Transport(error))
            }
        }
    }
}

// Rebuild the core in its static place; keep the large value out of the
// caller's frame.
#[inline(never)]
#[allow(
    large_assignments,
    reason = "the Controller core is rebuilt once per Host epoch in its static place; the linked image's stack gate and runtime stack painting check its stack use"
)]
fn reset_core(
    core: &mut LeController<'static, OUTPUT>,
    config: LeControllerConfig,
    random: &'static (dyn LeRandomSource + 'static),
) {
    *core = LeController::new(config, Some(random));
}

/// Create the HCI Controller of `runtime` once per boot. `public_address` is the
/// device's public address, `version` its Link Layer identity and `random`
/// the entropy for LE Rand and encryption.
///
/// # Panics
///
/// When called a second time: the transport and core are static.
#[inline(never)]
#[allow(
    large_assignments,
    reason = "the Controller core moves once into its static cell; the linked image's stack gate and runtime stack painting check its stack use"
)]
// CAPABILITY: bluetooth-trouble-host-integration, trouble-host-integration
pub fn start_bluetooth_hci(
    runtime: &'static BluetoothSystemRuntime,
    public_address: BluetoothPublicDeviceAddress,
    version: Option<LeVersionInformation>,
    random: &'static (dyn LeRandomSource + 'static),
) -> BluetoothHci {
    let bootstrap =
        LeControllerBootstrapConfig::new(public_address, ACL_DATA_LENGTH, HOST_TO_CONTROLLER as u8)
            .expect("the HCI profile is valid")
            .with_filter_accept_list_size(BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u8);
    let resources = RESOURCES.init(Resources::new(bootstrap).expect("the packet profile fits"));
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let config = LeControllerConfig { bootstrap, version };
    let core = CORE.init(LeController::new(config, Some(random)));
    BluetoothHci {
        host,
        service: BluetoothHciService {
            transport: controller,
            core,
            runtime,
            config,
            random,
            retired: None,
        },
    }
}
