//! Ownership boundary for MAC interface-address publication.

use open_esp_radio_esp32s31_pac::{ColdRadioRegisters, MacInterface};

/// Finite hardware capability needed to publish the two cold-path addresses.
pub trait MacInterfaceAddressHardware {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]);
}

impl MacInterfaceAddressHardware for ColdRadioRegisters {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.program_receive_interface_address(interface, address);
    }
}

/// Publish the STA and AP receive addresses required by the cold MAC path.
pub(crate) fn program_cold_receive_addresses<H: MacInterfaceAddressHardware>(
    hardware: &mut H,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) {
    hardware.program_interface_address(MacInterface::Station, station_address);
    hardware.program_interface_address(MacInterface::AccessPoint, access_point_address);
}
