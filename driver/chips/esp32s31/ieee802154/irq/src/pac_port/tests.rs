use open_esp_radio_esp32s31_pac::Ieee802154InterruptRegisters;

use crate::{InterruptPort, InterruptSnapshot};

#[test]
fn restricted_pac_owner_satisfies_the_production_port_contract() {
    fn require_port<Port: InterruptPort>()
    where
        Port::Snapshot: InterruptSnapshot,
    {
    }

    require_port::<Ieee802154InterruptRegisters>();
}
