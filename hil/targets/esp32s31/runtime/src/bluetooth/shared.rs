//! The Bluetooth LE client of the Wi-Fi image's shared radio.
//!
//! The Wi-Fi image owns the radio system, its PHY tracking and coexistence
//! schedule tasks and the HIL console. This client parks the Controller on the
//! radio's Bluetooth partition, starts it on the shared radio and runs the
//! radio runner, the HCI service and the Trouble GATT application on their own
//! tasks. The application's observations are published for the Wi-Fi
//! console's `QueryBluetoothGatt` answer.

use core::cell::Cell;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
use oer_esp32s31_bluetooth_system::{
    BluetoothEntropy, BluetoothHostTransport, BluetoothParked, BluetoothSystem, start_bluetooth_hci,
};
use oer_esp32s31_hal::root::BluetoothPartition;
use oer_hil_protocol::BluetoothGattEvidence;
use static_cell::StaticCell;

use super::{Radio, VERSION, gatt};

static SYSTEM: StaticCell<BluetoothSystem> = StaticCell::new();
/// The last application observation; `None` until the Host reports one.
static EVIDENCE: Mutex<CriticalSectionRawMutex, Cell<Option<BluetoothGattEvidence>>> =
    Mutex::new(Cell::new(None));

/// Start the client on the shared radio after the radio system exists.
///
/// The caller keeps running the radio's PHY tracking and coexistence
/// schedule; this client adds no radio-system task.
pub(crate) fn start(
    spawner: Spawner,
    radio: &'static Radio,
    partition: BluetoothPartition,
    public_address: BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
) {
    spawner.spawn(
        client(spawner, radio, partition, public_address, entropy)
            .expect("Bluetooth client task must allocate once"),
    );
}

/// The application's observations with the current CPU0 stack measurement.
pub(crate) fn evidence() -> BluetoothGattEvidence {
    let mut value = EVIDENCE.lock(Cell::get).unwrap_or_default();
    value.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
    value
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the start result crosses one poll boundary; the linked-image stack-frame audit bounds this task"
)]
async fn client(
    spawner: Spawner,
    radio: &'static Radio,
    partition: BluetoothPartition,
    public_address: BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
) {
    let Ok(parked) = BluetoothParked::new(partition) else {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-memory\r\n");
    };
    let system = match oer_esp32s31_bluetooth_system::start(radio, parked, public_address).await {
        Ok(system) => system,
        Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-start\r\n"),
    };
    let hci = start_bluetooth_hci(system.runtime(), public_address, Some(VERSION), entropy);
    spawner.spawn(super::runner(radio, SYSTEM.init(system)).expect("Bluetooth runner task"));
    spawner.spawn(super::service(hci.service).expect("Bluetooth HCI task"));
    spawner.spawn(application(hci.host).expect("Bluetooth GATT task"));
}

#[embassy_executor::task]
async fn application(host: BluetoothHostTransport) {
    let stack = gatt::build(host);
    let mut runner = stack.runner();
    let state = Cell::new(BluetoothGattEvidence::default());
    let host = core::pin::pin!(runner.run());
    let application = core::pin::pin!(gatt_application::gatt::run(&stack, |event| {
        gatt::observe(&state, event);
        EVIDENCE.lock(|evidence| evidence.set(Some(state.get())));
    }));
    match select(host, application).await {
        Either::First(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-host-stopped\r\n")
        }
        Either::Second(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-gatt-stopped\r\n")
        }
    }
}
