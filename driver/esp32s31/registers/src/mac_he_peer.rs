//! Generated-PAC ownership for the finite HE20 peer/association leaves.

use super::RadioRegisters;

/// Hardware-visible subset of one parsed HE20 peer.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_he.o]`
/// `ieee80211_parse_hecap`/`ieee80211_parse_heopr` callers and complete
/// `libpp.a[hal_mac_ctl.o]` leaves. The protocol crate owns parsing;
/// this type keeps the chip-specific register transform inside the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHe20PeerConfig {
    pub packet_padding_eight_us: u8,
    pub operation_parameters: u32,
    pub bss_color_information: u8,
    pub extended_range_single_user_disabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHe20PeerError {
    InvalidAssociationId,
    UnsupportedRtsThreshold,
}

#[cfg(test)]
const fn repeated_packet_padding(packet_padding_eight_us: u8) -> u32 {
    let padding_us = ((packet_padding_eight_us as u32) << 3) & 0x1f;
    padding_us | (padding_us << 5) | (padding_us << 10) | (padding_us << 15) | (padding_us << 20)
}

impl RadioRegisters {
    /// Enable interface-zero HE BSSID matching before installing peer state.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_ctl.o]::
    /// hal_he_bssid_init`, size `0x4c`, tail-calling the complete
    /// `hal_he_set_power_save`, size `0x36`. The associated-STA path in
    /// `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`, size `0x2b6`,
    /// calls this exact interface-zero leaf before clearing the color bitmap
    /// and programming the parsed HE Operation BSS color.
    fn initialize_interface_zero_he_bssid(&mut self) {
        let control = self
            .peripherals
            .wifi_mac_he_init_suffix
            .multi_bssid_control();
        // Keep the four fresh-read RMW edges distinct and in blob order.
        control.modify(|_, w| w.he_bssid_enable().set_bit());
        control.modify(|_, w| unsafe { w.bssid_select().bits(0) });

        let power_save = self.peripherals.wifi_mac_rx_power_save.control();
        power_save.modify(|_, w| w.intra_ppdu_ps_enable().set_bit());
        power_save.modify(|_, w| w.intra_ps_check_bss_color_enable().set_bit());
    }

    /// Program the finite HE20 receive-side state reached by the pinned peer
    /// capability and operation parsers.
    ///
    /// A finite RTS threshold fails closed: its vendor table builder contains
    /// a separate floating-point loop that has not been promoted yet.
    pub fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        if rts_threshold.is_some() {
            return Err(MacHe20PeerError::UnsupportedRtsThreshold);
        }

        self.initialize_interface_zero_he_bssid();
        let init = &self.peripherals.wifi_mac_he_init_suffix;
        let color_information = config.bss_color_information;
        if color_information & 0xbf != 0 {
            let color = u32::from(color_information & 0x3f);
            let partial = color_information & 0x40 != 0;
            let disabled = color_information & 0x80 != 0;
            let register = init.multi_bssid_control();
            let mut image = register.read().bits();
            image &= !(0x0800_0000 | 0x07e0_0000 | 0x1000_0000);
            if !disabled {
                image |= 0x0800_0000 | (color << 21);
            }
            if partial {
                image |= 0x1000_0000;
            }
            // SAFETY: this is the complete instruction-exact RMW image for
            // the recovered register; all unrelated bits came from the read.
            // SAFETY: the SVD marks the register writable, and `image`
            // preserves every field outside the recovered transform.
            unsafe { register.write_with_zero(|w| w.bits(image)) };
        }

        self.peripherals
            .wifi_mac_he_init_prefix
            .rx_field_control()
            .modify(|r, w| unsafe { w.bitmap_control().bits(r.bitmap_control().bits() | 1) });
        init.he_default_control().modify(|_, w| unsafe {
            w.default_pe_duration()
                .bits((config.operation_parameters & 0x07) as u8)
        });

        let duration = (config.packet_padding_eight_us << 3) & 0x1f;
        init.he_packet_padding().modify(|_, w| unsafe {
            w.bpsk_duration()
                .bits(duration)
                .qpsk_duration()
                .bits(duration)
                .qam16_duration()
                .bits(duration)
                .qam64_duration()
                .bits(duration)
                .qam256_duration()
                .bits(duration)
        });

        let queues = &self.peripherals.wifi_mac_tx_queue_vector;
        for physical in 0..4 {
            queues
                .he_rts_control(physical)
                .modify(|_, w| w.he_rts_disabled().set_bit());
        }

        init.ersu_and_vht_control().modify(|_, w| {
            w.auto_ack_allow_ersu()
                .bit(!config.extended_range_single_user_disabled)
        });
        if !config.extended_range_single_user_disabled {
            // The complete ER-SU-permitted leaf writes all four baseline
            // ACK-rate bytes to 0x80.
            // SAFETY: the complete recovered leaf publishes this whole word.
            unsafe {
                init.ersu_ack_rate()
                    .write_with_zero(|w| w.bits(0x8080_8080));
            }
        }
        Ok(())
    }

    /// Install interface-zero HE association state after a successful
    /// Association Response.
    pub fn program_he20_association(
        &mut self,
        association_id: u16,
        minimum_mpdu_start_spacing: u8,
        bssid_index: u8,
    ) -> Result<(), MacHe20PeerError> {
        if association_id == 0 || association_id > 0x07ff {
            return Err(MacHe20PeerError::InvalidAssociationId);
        }

        self.peripherals
            .wifi_mac_bssid_policy
            .bssid_high(0)
            .modify(|_, w| unsafe {
                w.minimum_mpdu_start_spacing()
                    .bits(minimum_mpdu_start_spacing & 0x07)
                    .association_id()
                    .bits(association_id)
            });

        let init = &self.peripherals.wifi_mac_he_init_suffix;
        let broadcast_low = init.broadcast_ru_low();
        let mut low = broadcast_low.read().bits();
        low = (low & !0x0000_07ff) | u32::from(association_id);
        low |= 0x0040_0000;
        low = (low & !0x003f_f800) | (u32::from(bssid_index) << 11);
        // SAFETY: the SVD marks the register writable, and `low` is an RMW
        // image preserving all fields outside the complete recovered leaf.
        unsafe { broadcast_low.write_with_zero(|w| w.bits(low)) };

        let broadcast_high = init.broadcast_ru_high();
        let mut high = broadcast_high.read().bits();
        high |= 0x0000_0800;
        high &= !0x0000_07ff;
        high |= 0x0080_0000;
        high &= !0x007f_f000;
        // SAFETY: the SVD marks the register writable, and `high` is an RMW
        // image preserving all fields outside the complete recovered leaf.
        unsafe { broadcast_high.write_with_zero(|w| w.bits(high)) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::repeated_packet_padding;

    #[test]
    fn packet_padding_matches_five_recovered_fields() {
        assert_eq!(repeated_packet_padding(0), 0);
        assert_eq!(repeated_packet_padding(2), 0x0108_4210);
        assert_eq!(repeated_packet_padding(3), 0x018c_6318);
    }
}
