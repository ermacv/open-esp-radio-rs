//! Generated-PAC ownership for MAC receive-interface addresses.

#![forbid(unsafe_code)]

use super::{MacInterface, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
    ///
    /// The complete leaf performs three ordered hardware operations. In
    /// particular, the enable edge is a fresh-read RMW and must not be folded
    /// into the preceding full-word high-address store.
    pub fn program_receive_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        let interface = interface.bits() as usize;
        let addresses = &self.peripherals.wifi_mac.wifi_mac_interface_address;
        super::svd::zero_based_field_write::mac_interface_address_low(
            addresses,
            interface,
            u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
        );
        super::svd::zero_based_field_write::mac_interface_address_high(
            addresses,
            interface,
            u16::from_le_bytes([address[4], address[5]]),
        );
        addresses
            .address_high(interface)
            .modify(|_, w| w.rx_policy_enable().set_bit());
    }

    /// Publish the STA and AP interface addresses used by the open cold path.
    pub fn program_sta_ap_receive_addresses(
        &mut self,
        station_address: [u8; 6],
        access_point_address: [u8; 6],
    ) {
        self.program_receive_interface_address(MacInterface::Station, station_address);
        self.program_receive_interface_address(MacInterface::AccessPoint, access_point_address);
    }
}
