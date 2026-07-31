//! Ownership-bound frequency-memory and channel-switch leaves.

use super::RadioRegisters;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChannelCbwFields {
    tx_offset: u8,
    control_0: u8,
    control_1_high: u8,
    control_1_low: u8,
}

const fn bss_tx_offset(cbw: u8) -> u8 {
    if cbw == 2 {
        2
    } else if cbw == 3 {
        1
    } else {
        0
    }
}

const fn channel_cbw_fields(cbw: u8) -> ChannelCbwFields {
    let high = cbw >> 4;
    if high != 0 {
        let normalized = high.wrapping_sub(1);
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized & 3,
            control_1_high: (normalized >> 2) & 7,
            control_1_low: normalized & 3,
        }
    } else {
        let low = cbw & 0x0f;
        let normalized = low.saturating_sub(2);
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized & 3,
            control_1_high: if low != 0 { 1 } else { 0 },
            control_1_low: if cbw & 0x0e != 0 { 1 } else { 0 },
        }
    }
}

fn nrx_frequency_image(shift_source: u32, previous: u32, frequency: u16) -> u32 {
    assert!(frequency != 0, "NRX frequency must be nonzero");
    let shift = shift_source >> 24;
    let quotient = 0x50_u32.wrapping_shl(shift) / u32::from(frequency);
    (previous & 0xff00_0000) | (quotient & 0x000f_ffff)
}

impl RadioRegisters {
    /// Select the recovered two-bit baseband mode.
    pub fn set_frequency_baseband_mode(&mut self, mode: u8) {
        assert!(mode <= 3, "baseband mode must fit two bits");
        // SAFETY: the assertion proves mode fits the generated field.
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_parameter_1_status()
            .modify(|_, w| unsafe { w.baseband_mode_unknown().bits(mode) });
    }

    /// Pulse the frequency-module reset/release bit through two fresh RMWs.
    pub fn reset_frequency_module(&mut self) {
        let control = self
            .peripherals
            .phy_frequency_channel_oracle
            .frequency_control();
        control.modify(|_, w| w.memory_address_high_or_module_reset_unknown().clear_bit());
        control.modify(|_, w| w.memory_address_high_or_module_reset_unknown().set_bit());
    }

    /// Select whether hardware owns frequency updates.
    pub fn set_hardware_frequency_control(&mut self, enabled: bool) {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_control()
            .modify(|_, w| w.hardware_frequency_disable().bit(!enabled));
    }

    /// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
    pub fn initialize_frequency_registers(&mut self, parameter_override: bool) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        frequency.frequency_control().modify(|_, w| {
            w.channel_switch_pulse()
                .clear_bit()
                .hardware_frequency_disable()
                .clear_bit()
        });
        frequency
            .frequency_control()
            .modify(|_, w| w.module_enable_unknown().set_bit());
        // SAFETY: both complete branch images fit the generated eight-bit
        // field.
        frequency.frequency_control().modify(|_, w| unsafe {
            w.register_mode_unknown()
                .bits(if parameter_override { 0x20 } else { 0x42 })
        });
        // SAFETY: both values are complete full-word stores from the ROM.
        unsafe {
            frequency
                .frequency_parameter_0_opaque()
                .write_with_zero(|w| w.opaque_value().bits(0x1980_0249));
            frequency
                .frequency_parameter_1_status()
                .write_with_zero(|w| w.bits(0x2582_4e58));
        }
    }

    /// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
    pub fn write_frequency_memory(&mut self, address: u16, value: u32, mode: u8) {
        assert!(
            address <= 0x07ff,
            "frequency-memory address must fit eleven bits"
        );
        assert!(
            value <= 0x00ff_ffff,
            "frequency-memory data must fit 24 bits"
        );
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        // SAFETY: both address components are masked or shifted into their
        // generated ten- and one-bit fields.
        frequency.frequency_control().modify(|_, w| unsafe {
            w.memory_address_low_unknown()
                .bits(address & 0x03ff)
                .memory_address_high_or_module_reset_unknown()
                .bit(address & 0x0400 != 0)
        });
        // SAFETY: value is assertion-bounded to 24 bits and mode is exactly
        // the generated eight-bit argument type.
        unsafe {
            frequency
                .frequency_memory_data()
                .write_with_zero(|w| w.data().bits(value).mode().bits(mode));
        }
        frequency
            .frequency_control()
            .modify(|_, w| w.memory_write_pulse().set_bit());
        frequency
            .frequency_control()
            .modify(|_, w| w.memory_write_pulse().clear_bit());
    }

    /// Publish the complete `phy_freq_i2c_num_addr` register image.
    pub fn configure_frequency_i2c_number_addresses(
        &mut self,
        control_field: u32,
        words: [u32; 3],
    ) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        let address_0 = ((control_field >> 8) & 0x1f) as u8;
        let address_1 = ((control_field >> 13) & 0x1f) as u8;
        // SAFETY: both decoded values are masked to five bits.
        frequency.i2c_number_control().modify(|_, w| unsafe {
            w.number_address_0_unknown()
                .bits(address_0)
                .number_address_1_unknown()
                .bits(address_1)
        });
        // SAFETY: the complete ROM body publishes the three caller-prepared
        // packed words in full and in this exact order.
        unsafe {
            frequency
                .i2c_number_word(0)
                .write_with_zero(|w| w.bits(words[0]));
            frequency
                .i2c_number_word(1)
                .write_with_zero(|w| w.bits(words[1]));
            frequency
                .i2c_number_word(2)
                .write_with_zero(|w| w.bits(words[2]));
        }
    }

    /// Publish the two pre-delay edges of `phy_freq_chan_en_sw`.
    pub fn start_frequency_channel_switch(&mut self, frequency_index: u8) {
        let control = self
            .peripherals
            .phy_frequency_channel_oracle
            .frequency_control();
        // SAFETY: frequency_index is exactly the generated eight-bit field
        // argument.
        control.modify(|_, w| unsafe { w.channel_index().bits(frequency_index) });
        control.modify(|_, w| w.channel_switch_pulse().set_bit());
    }

    /// Complete the caller-delayed clear of `phy_freq_chan_en_sw`.
    pub fn clear_frequency_channel_switch(&mut self) {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_control()
            .modify(|_, w| w.channel_switch_pulse().clear_bit());
    }

    /// Sample the complete frequency-ready word exactly once.
    pub fn sample_frequency_ready_image(&mut self) -> u32 {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_parameter_1_status()
            .read()
            .bits()
    }

    /// Apply complete rev0 ROM `phy_nrx_freq_set`.
    pub fn configure_nrx_frequency(&mut self, frequency: u16) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .nrx_frequency_control();
        // The ROM intentionally samples this word twice. The first image is
        // the shift source; the second supplies the high byte preserved by
        // the final full-word store.
        let shift_source = register.read().bits();
        let previous = register.read().bits();
        let image = nrx_frequency_image(shift_source, previous, frequency);
        // SAFETY: `image` is the complete instruction-derived register image:
        // the second sample's high byte plus the bounded twenty-bit quotient.
        unsafe {
            register.write_with_zero(|w| w.bits(image));
        }
    }

    /// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_nrx_baseband(&mut self) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .nrx_frequency_control();
        // SAFETY: the complete ROM body replaces the generated 20- and
        // four-bit fields with these bounded constants in one fresh RMW.
        register.modify(|_, w| unsafe {
            w.frequency_quotient_or_init_low_unknown()
                .bits(0x0004_33af)
                .init_middle_unknown()
                .bits(0)
        });
        // SAFETY: 0x17 fits the generated five-bit field.
        register.modify(|_, w| unsafe { w.shift_low_or_init_high_unknown().bits(0x17) });
    }

    /// Publish the channel-offset prefix of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_prefix(&mut self, cbw: u8) {
        let value = bss_tx_offset(cbw);
        // SAFETY: the branch-derived value is in 0..=2 and therefore fits
        // the generated four-bit field.
        self.peripherals
            .phy_frequency_channel_oracle
            .channel_tx_offset_control()
            .modify(|_, w| unsafe { w.channel_offset_unknown().bits(value) });
    }

    /// Publish the three FBW suffix edges of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_suffix(&mut self, cbw: u8) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .fbw_bt_filter_control();
        register.modify(|_, w| {
            w.fbw_clear_low_unknown()
                .clear_bit()
                .fbw_clear_high_unknown()
                .clear_bit()
        });
        // SAFETY: a Boolean encoding fits both generated two-bit fields.
        register.modify(|_, w| unsafe { w.fbw_select_mid_unknown().bits(u8::from(cbw != 0)) });
        // SAFETY: a Boolean encoding fits both generated two-bit fields.
        register.modify(|_, w| unsafe { w.fbw_select_high_unknown().bits(u8::from(cbw != 0)) });
    }

    /// Apply the four fresh-read replacements of complete `phy_bb_cbw_chan_cfg`.
    pub fn configure_channel_cbw(&mut self, cbw: u8) {
        let fields = channel_cbw_fields(cbw);
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        // SAFETY: all values are masked or branch-bounded to their generated
        // four-, two-, three-, and two-bit field widths.
        frequency
            .channel_tx_offset_control()
            .modify(|_, w| unsafe { w.channel_offset_unknown().bits(fields.tx_offset) });
        frequency
            .channel_cbw_control_0()
            .modify(|_, w| unsafe { w.cbw_low_unknown().bits(fields.control_0) });
        frequency
            .channel_cbw_control_1()
            .modify(|_, w| unsafe { w.cbw_high_unknown().bits(fields.control_1_high) });
        frequency
            .channel_cbw_control_1()
            .modify(|_, w| unsafe { w.cbw_low_unknown().bits(fields.control_1_low) });
    }

    /// Apply the three fresh-read updates of complete `phy_bt_filter_reg`.
    pub fn configure_bt_filter(&mut self) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .fbw_bt_filter_control();
        register.modify(|_, w| w.bt_filter_enable_unknown().set_bit());
        register.modify(|_, w| w.bt_filter_low_unknown().clear_bit());
        // SAFETY: zero fits the generated two-bit field.
        register.modify(|_, w| unsafe { w.bt_filter_mode_unknown().bits(0) });
    }

    /// Publish the TX-cap readback into command-memory entry one.
    pub fn publish_frequency_tx_cap(&mut self, value: u8) {
        // SAFETY: the complete ROM leaf publishes the three generated
        // eight-bit fields as one write-only command word.
        unsafe {
            self.peripherals
                .phy_i2c_command_ram
                .command_memory(1)
                .write_with_zero(|w| w.block().bits(0x6b).register().bits(2).data().bits(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChannelCbwFields, bss_tx_offset, channel_cbw_fields, nrx_frequency_image};

    #[test]
    fn bss_offsets_retain_complete_rom_branches() {
        assert_eq!(bss_tx_offset(0), 0);
        assert_eq!(bss_tx_offset(1), 0);
        assert_eq!(bss_tx_offset(2), 2);
        assert_eq!(bss_tx_offset(3), 1);
        assert_eq!(bss_tx_offset(4), 0);
    }

    #[test]
    fn channel_cbw_derivation_retains_both_complete_rom_paths() {
        assert_eq!(
            channel_cbw_fields(0),
            ChannelCbwFields {
                tx_offset: 0,
                control_0: 0,
                control_1_high: 0,
                control_1_low: 0,
            }
        );
        assert_eq!(
            channel_cbw_fields(2),
            ChannelCbwFields {
                tx_offset: 0,
                control_0: 0,
                control_1_high: 1,
                control_1_low: 1,
            }
        );
        assert_eq!(
            channel_cbw_fields(0x50),
            ChannelCbwFields {
                tx_offset: 4,
                control_0: 0,
                control_1_high: 1,
                control_1_low: 0,
            }
        );
    }

    #[test]
    fn nrx_image_preserves_second_high_byte_and_clears_middle_nibble() {
        assert_eq!(
            nrx_frequency_image(0x0200_0000, 0xa5f0_1234, 2_462),
            0xa500_0000
        );
    }
}
