//! The plaintext Trouble GATT application over the HCI Controller.
//!
//! The Trouble Host owns HCI and ATT over the Host end of the transport; the
//! HIL console only reads the value-only observations the application
//! reports.

use core::cell::Cell;

use gatt_application::gatt::{self, Observation};
use oer_bluetooth_hci::bt_hci::controller::ExternalController;
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
use oer_hil_protocol::{BluetoothGattEvidence, Command, Event, FeatureCapabilities, RejectReason};
use static_cell::StaticCell;
use trouble_host::prelude::*;

use super::console;

/// Command slots the Host keeps in flight.
const COMMAND_SLOTS: usize = 4;

type Controller = ExternalController<BluetoothHostTransport, COMMAND_SLOTS>;

static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();

struct Profile(Cell<BluetoothGattEvidence>);

impl console::Profile for Profile {
    fn features(&self) -> FeatureCapabilities {
        FeatureCapabilities {
            bluetooth_gatt: true,
            ..FeatureCapabilities::default()
        }
    }

    fn maximum_payload_bytes(&self) -> u16 {
        0
    }

    async fn command(&self, command: Command) -> Event {
        match command {
            Command::QueryBluetoothGatt => {
                let mut value = self.0.get();
                value.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
                Event::BluetoothGatt(value)
            }
            _ => Event::Rejected(RejectReason::InvalidState),
        }
    }
}

#[embassy_executor::task]
pub(super) async fn task(
    host: BluetoothHostTransport,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) {
    let resources = RESOURCES.init_with(HostResources::new);
    let stack = build(host, resources);
    let mut runner = stack.runner();
    let profile = Profile(Cell::new(BluetoothGattEvidence::default()));
    let host = core::pin::pin!(runner.run());
    let application = core::pin::pin!(gatt::run(&stack, |event| observe(&profile.0, event)));
    let console = core::pin::pin!(console::run(usb, boot, &profile));
    match embassy_futures::select::select3(host, application, console).await {
        embassy_futures::select::Either3::First(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-host-stopped\r\n")
        }
        embassy_futures::select::Either3::Second(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-gatt-stopped\r\n")
        }
        embassy_futures::select::Either3::Third(never) => match never {},
    }
}

// Host construction materializes security-manager and connection state
// beyond the caller-owned resource arrays; keep it out of the task frame.
#[inline(never)]
fn build(
    host: BluetoothHostTransport,
    resources: &'static mut HostResources<DefaultPacketPool, 1, 3>,
) -> Stack<'static, Controller, DefaultPacketPool> {
    trouble_host::new(Controller::new(host), resources).build()
}

fn observe(state: &Cell<BluetoothGattEvidence>, event: Observation) {
    let mut value = state.get();
    let increment =
        |counter: &mut u32| *counter = counter.checked_add(1).expect("GATT evidence exhausted");
    match event {
        Observation::Ready(address) => value.address = Some(address),
        Observation::Advertising => {
            value.advertising = true;
            increment(&mut value.advertising_starts);
        }
        Observation::Connected => {
            value.advertising = false;
            value.connected = true;
            increment(&mut value.connections);
        }
        Observation::Read(current) => {
            value.value = current;
            increment(&mut value.reads);
        }
        Observation::Written(current) => {
            value.value = current;
            increment(&mut value.writes);
        }
        Observation::Disconnected(reason) => {
            value.connected = false;
            value.last_disconnect_reason = Some(reason);
            increment(&mut value.disconnections);
        }
    }
    state.set(value);
}
