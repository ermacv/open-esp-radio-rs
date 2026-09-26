use oer_esp32s31_hal::ieee802154::mac::Ieee802154InterruptOwner;

use crate::{InterruptPort, InterruptSnapshot};

#[test]
fn hal_interrupt_owner_satisfies_the_production_port_contract() {
    fn require_port<Port: InterruptPort>()
    where
        Port::Snapshot: InterruptSnapshot,
    {
    }

    require_port::<Ieee802154InterruptOwner>();
}
