//! Ownership boundary for MAC interface-address publication.

use open_esp_radio_pac_esp32s31::ColdRadioRegisters;

/// Finite hardware capability needed to publish the two cold-path addresses.
pub trait MacInterfaceAddressHardware {
    fn program_sta_ap_addresses(&mut self, station_address: [u8; 6], access_point_address: [u8; 6]);
}

impl MacInterfaceAddressHardware for ColdRadioRegisters {
    fn program_sta_ap_addresses(
        &mut self,
        station_address: [u8; 6],
        access_point_address: [u8; 6],
    ) {
        self.program_sta_ap_receive_addresses(station_address, access_point_address);
    }
}

/// Publish the STA and AP receive addresses required by the cold MAC path.
pub(crate) fn program_cold_receive_addresses<H: MacInterfaceAddressHardware>(
    hardware: &mut H,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) {
    hardware.program_sta_ap_addresses(station_address, access_point_address);
}
