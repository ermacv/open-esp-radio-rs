//! Plaintext application and its read-only observation surface.
use super::console;
use core::cell::Cell;
use gatt_application::gatt::{self, Observation};
use oer_hil_protocol::{BluetoothGattEvidence, Command, Event, FeatureCapabilities, RejectReason};
use trouble_host::{Controller, Stack, prelude::DefaultPacketPool};

struct Profile(Cell<BluetoothGattEvidence>);
impl console::Profile for Profile {
    fn features(&self) -> FeatureCapabilities {
        FeatureCapabilities {
            bluetooth_gatt: true,
            ..FeatureCapabilities::default()
        }
    }
    fn command(&self, command: Command) -> Event {
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

pub(super) async fn run<C: Controller>(
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
    stack: &Stack<'_, C, DefaultPacketPool>,
) -> core::convert::Infallible {
    let profile = Profile(Cell::new(BluetoothGattEvidence::default()));
    match embassy_futures::select::select(
        gatt::run(stack, |event| observe(&profile.0, event)),
        console::run(usb, boot, &profile),
    )
    .await
    {
        embassy_futures::select::Either::First(_) => panic!("GATT application stopped"),
        embassy_futures::select::Either::Second(never) => never,
    }
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
