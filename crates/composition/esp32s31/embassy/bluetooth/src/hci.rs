//! The HCI Controller of the composition: in-process transport, Controller
//! core and service loop over the radio runtime.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_bluetooth_controller::{LeController, LeControllerConfig, LeVersionInformation};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, InProcessHciControllerTransport, InProcessHciHostTransport,
    LeControllerBootstrapConfig, LeControllerHciEndpoints, LeControllerHciResources,
    LeRandomSource, LeRandomUnavailable,
};
use oer_bluetooth_runtime::{ServeExit, serve};
use oer_esp32s31_bluetooth_runtime::BluetoothRuntimeError;
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
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
}

impl BluetoothHciService {
    /// Serve until the transport closes or the radio fails.
    pub async fn run(self) -> ServeExit<BluetoothRuntimeError> {
        serve(&self.transport, self.core, self.runtime).await
    }
}

/// Create the HCI Controller of `runtime` once. `public_address` is the
/// device's public address, `version` its Link Layer identity and `random`
/// the entropy for LE Rand and encryption.
///
/// # Panics
///
/// When called a second time: the transport and core are static.
#[inline(never)]
#[allow(
    large_assignments,
    reason = "the Controller core moves once into its static cell; the linked-image stack-frame audit bounds this frame"
)]
pub fn start_bluetooth_hci(
    runtime: &'static BluetoothSystemRuntime,
    public_address: BluetoothPublicDeviceAddress,
    version: Option<LeVersionInformation>,
    random: &'static (dyn LeRandomSource + 'static),
) -> BluetoothHci {
    let bootstrap =
        LeControllerBootstrapConfig::new(public_address, ACL_DATA_LENGTH, HOST_TO_CONTROLLER as u8)
            .expect("the HCI profile is valid");
    let resources = RESOURCES.init(Resources::new(bootstrap).expect("the packet profile fits"));
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let core = CORE.init(LeController::new(
        LeControllerConfig { bootstrap, version },
        Some(random),
    ));
    BluetoothHci {
        host,
        service: BluetoothHciService {
            transport: controller,
            core,
            runtime,
        },
    }
}
