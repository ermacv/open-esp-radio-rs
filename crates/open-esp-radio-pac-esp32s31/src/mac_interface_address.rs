//! Generated-PAC ownership for MAC receive-interface addresses.

use super::RadioRegisters;

impl RadioRegisters {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
    ///
    /// The complete leaf performs three ordered hardware operations. In
    /// particular, the enable edge is a fresh-read RMW and must not be folded
    /// into the preceding full-word high-address store.
    fn program_receive_interface_address(&mut self, interface: usize, address: [u8; 6]) {
        let addresses = &self.peripherals.wifi_mac_interface_address;
        // SAFETY: both values exactly fill their generated fields, and the
        // complete recovered leaf publishes them as full-word stores.
        unsafe {
            addresses.address_low(interface).write_with_zero(|w| {
                w.bytes_0_3().bits(u32::from_le_bytes([
                    address[0], address[1], address[2], address[3],
                ]))
            });
            addresses.address_high(interface).write_with_zero(|w| {
                w.bytes_4_5()
                    .bits(u16::from_le_bytes([address[4], address[5]]))
            });
        }
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
        self.program_receive_interface_address(0, station_address);
        self.program_receive_interface_address(1, access_point_address);
    }
}
