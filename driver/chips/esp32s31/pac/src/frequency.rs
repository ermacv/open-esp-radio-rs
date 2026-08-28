//! Ownership-bound frequency-memory and channel-switch leaves.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

/// Semantic PHY-I2C number-address sequence consumed by
/// `phy_freq_i2c_num_addr`.
///
/// The register split and field placement remain private to the PAC. Callers
/// provide only the eleven addresses recovered from the vendor descriptor
/// graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cNumberAddresses {
    values: [u8; 11],
}

impl PhyFrequencyI2cNumberAddresses {
    /// Bind the complete recovered address sequence to its five-bit hardware
    /// domain.
    pub const fn new(values: [u8; 11]) -> Option<Self> {
        let mut index = 0;
        while index != values.len() {
            if values[index] > 0x1f {
                return None;
            }
            index += 1;
        }
        Some(Self { values })
    }
}

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

const fn channel_cbw_fields(cbw: u32) -> ChannelCbwFields {
    let high = cbw >> 4;
    if high != 0 {
        let normalized = high.wrapping_sub(1) as u8;
        let low = cbw as u8 & 0x0f;
        ChannelCbwFields {
            tx_offset: low,
            control_0: low & 3,
            control_1_high: normalized & 7,
            control_1_low: normalized & 3,
        }
    } else {
        let low = cbw as u8 & 0x0f;
        let normalized = low.saturating_sub(2);
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized & 3,
            control_1_high: if low != 0 { 1 } else { 0 },
            control_1_low: if cbw & 0x0e != 0 { 1 } else { 0 },
        }
    }
}

const fn rv32_signed_div(dividend: i32, divisor: i32) -> i32 {
    if divisor == 0 {
        -1
    } else if dividend == i32::MIN && divisor == -1 {
        i32::MIN
    } else {
        dividend / divisor
    }
}

fn nrx_frequency_quotient(shift: u32, frequency: u32) -> u32 {
    let numerator = 0x50_u32.wrapping_shl(shift) as i32;
    let quotient = rv32_signed_div(numerator, frequency as i32) as u32;
    quotient % 0x0100_0000
}

impl RadioPhyRegisters {
    /// Select the recovered two-bit baseband mode.
    pub fn set_frequency_baseband_mode(&mut self, mode: u8) {
        assert!(mode <= 3, "baseband mode must fit two bits");
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_parameter_1_status()
            .modify(|_, w| w.baseband_mode_unknown().set(mode));
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
        frequency.frequency_control().modify(|_, w| {
            w.register_mode_unknown()
                .set(if parameter_override { 0x20 } else { 0x42 })
        });
        super::generated::frequency_parameter_0(
            frequency,
            super::generated::FrequencyParameter0Image::new(0x1980_0249),
        );
        super::svd::zero_based_field_write::frequency_parameter_1_initialization(
            frequency, 0, 0x16, 0x12c127,
        );
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
        frequency.frequency_control().modify(|_, w| {
            w.memory_address_low_unknown()
                .set(address & 0x03ff)
                .memory_address_high_or_module_reset_unknown()
                .bit(address & 0x0400 != 0)
        });
        super::svd::zero_based_field_write::frequency_memory_data(frequency, value, mode);
        frequency
            .frequency_control()
            .modify(|_, w| w.memory_write_pulse().set_bit());
        frequency
            .frequency_control()
            .modify(|_, w| w.memory_write_pulse().clear_bit());
    }

    /// Apply complete rev0 ROM `phy_read_rf_freq_mem`.
    pub fn read_frequency_memory(&mut self, address: u16, mode: u8) -> u32 {
        assert!(
            address <= 0x07ff,
            "frequency-memory address must fit eleven bits"
        );
        assert!(mode <= 3, "frequency-memory read mode must fit two bits");
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        frequency.frequency_control().modify(|_, w| {
            w.memory_address_low_unknown()
                .set(address & 0x03ff)
                .memory_address_high_or_module_reset_unknown()
                .bit(address & 0x0400 != 0)
        });
        frequency
            .i2c_number_control()
            .modify(|_, w| w.memory_read_mode().set(mode));
        frequency
            .frequency_memory_read_control()
            .modify(|_, w| w.read_pulse().set_bit());
        frequency
            .frequency_memory_read_control()
            .modify(|_, w| w.read_pulse().clear_bit());
        frequency
            .frequency_memory_read_result()
            .read()
            .value()
            .bits()
    }

    /// Restore the low frequency-control channel index without pulsing a switch.
    pub fn set_frequency_channel_index(&mut self, frequency_index: u8) {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_control()
            .modify(|_, w| w.channel_index().set(frequency_index));
    }

    /// Publish the complete semantic `phy_freq_i2c_num_addr` sequence.
    pub fn configure_frequency_i2c_number_addresses(
        &mut self,
        addresses: PhyFrequencyI2cNumberAddresses,
    ) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        frequency.i2c_number_control().modify(|_, w| {
            w.number_address_0_unknown()
                .set(addresses.values[0])
                .number_address_1_unknown()
                .set(addresses.values[1])
        });
        super::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency,
            0,
            addresses.values[2],
            addresses.values[3],
            addresses.values[4],
            addresses.values[5],
            addresses.values[6],
            addresses.values[7],
            0,
        );
        super::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency,
            1,
            addresses.values[8],
            addresses.values[9],
            addresses.values[10],
            0,
            0,
            0,
            0,
        );
        super::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency, 2, 0, 0, 0, 0, 0, 0, 0,
        );
    }

    /// Publish the two pre-delay edges of `phy_freq_chan_en_sw`.
    pub fn start_frequency_channel_switch(&mut self, frequency_index: u8) {
        let control = self
            .peripherals
            .phy_frequency_channel_oracle
            .frequency_control();
        control.modify(|_, w| w.channel_index().set(frequency_index));
        control.modify(|_, w| w.channel_switch_pulse().set_bit());
    }

    /// Complete the caller-delayed clear of `phy_freq_chan_en_sw`.
    pub fn clear_frequency_channel_switch(&mut self) {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_control()
            .modify(|_, w| w.channel_switch_pulse().clear_bit());
    }

    /// Sample the generated frequency-ready field exactly once.
    pub fn frequency_ready(&mut self) -> bool {
        self.peripherals
            .phy_frequency_channel_oracle
            .frequency_parameter_1_status()
            .read()
            .frequency_ready()
            .bit_is_set()
    }

    /// Apply complete rev0 ROM `phy_nrx_freq_set`.
    pub fn configure_nrx_frequency(&mut self, frequency: u32) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .nrx_frequency_control();
        // The ROM intentionally samples this word twice. The first sample
        // supplies the complete eight-bit shift selector; the second supplies
        // both high fields preserved by the final publication.
        let shift_source = register.read();
        let shift = u32::from(shift_source.shift_low_or_init_high_unknown().bits())
            + u32::from(shift_source.shift_high_unknown().bits()) * 0x20;
        let previous = register.read();
        super::svd::zero_based_field_write::publish_nrx_frequency_fields(
            &self.peripherals.phy_frequency_channel_oracle,
            nrx_frequency_quotient(shift, frequency),
            previous.shift_low_or_init_high_unknown().bits(),
            previous.shift_high_unknown().bits(),
        );
    }

    /// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_nrx_baseband(&mut self) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .nrx_frequency_control();
        // The complete ROM body replaces the generated twenty-four-bit
        // multifunction field with this bounded initialization image.
        register.modify(|_, w| w.frequency_quotient_or_init_low_unknown().set(0x0004_33af));
        register.modify(|_, w| w.shift_low_or_init_high_unknown().set(0x17));
    }

    /// Publish the channel-offset prefix of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_prefix(&mut self, cbw: u8) {
        let value = bss_tx_offset(cbw);
        self.peripherals
            .phy_frequency_channel_oracle
            .channel_tx_offset_control()
            .modify(|_, w| w.channel_offset_low_unknown().set(value));
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
        register.modify(|_, w| w.fbw_select_mid_unknown().set(u8::from(cbw != 0)));
        register.modify(|_, w| w.fbw_select_high_unknown().set(u8::from(cbw != 0)));
    }

    /// Apply the four fresh-read replacements of complete `phy_bb_cbw_chan_cfg`.
    pub fn configure_channel_cbw(&mut self, cbw: u32) {
        let fields = channel_cbw_fields(cbw);
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        // The ROM clears the shared bits 7:4 before publishing the bounded
        // low channel-offset nibble. The independently named upper two
        // minimum-power bits remain untouched by these field accessors.
        frequency.channel_tx_offset_control().modify(|_, w| {
            w.channel_offset_low_unknown()
                .set(fields.tx_offset)
                .channel_offset_high_or_minimum_power_low_unknown()
                .set(0)
        });
        frequency
            .channel_cbw_control_0()
            .modify(|_, w| w.cbw_low_unknown().set(fields.control_0));
        frequency
            .channel_cbw_control_1()
            .modify(|_, w| w.cbw_high_unknown().set(fields.control_1_high));
        frequency
            .channel_cbw_control_1()
            .modify(|_, w| w.cbw_low_unknown().set(fields.control_1_low));
    }

    /// Apply the three fresh-read updates of complete `phy_bt_filter_reg`.
    pub fn configure_bt_filter(&mut self) {
        let register = self
            .peripherals
            .phy_frequency_channel_oracle
            .fbw_bt_filter_control();
        register.modify(|_, w| w.bt_filter_enable_unknown().set_bit());
        register.modify(|_, w| w.bt_filter_low_unknown().clear_bit());
        register.modify(|_, w| w.bt_filter_mode_unknown().set(0));
    }

    /// Publish the TX-cap readback into command-memory entry one.
    pub fn publish_frequency_tx_cap(&mut self, value: u8) {
        super::svd::zero_based_field_write::phy_i2c_command_memory(
            &self.peripherals.phy_i2c_command_ram,
            1,
            0x6b,
            2,
            value,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChannelCbwFields, bss_tx_offset, channel_cbw_fields, nrx_frequency_quotient,
        rv32_signed_div,
    };

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
                tx_offset: 0,
                control_0: 0,
                control_1_high: 4,
                control_1_low: 0,
            }
        );
        assert_eq!(
            channel_cbw_fields(0x53),
            ChannelCbwFields {
                tx_offset: 3,
                control_0: 3,
                control_1_high: 4,
                control_1_low: 0,
            }
        );
        assert_eq!(channel_cbw_fields(0x1_010), channel_cbw_fields(0x10));
        assert_eq!(channel_cbw_fields(0x1_00f).tx_offset, 0x0f);
    }

    #[test]
    fn nrx_division_matches_the_complete_rv32_input_domain() {
        assert_eq!(rv32_signed_div(80, 0), -1);
        assert_eq!(rv32_signed_div(i32::MIN, -1), i32::MIN);
        assert_eq!(rv32_signed_div(-1_610_612_736, 7), -230_087_533);

        assert_eq!(nrx_frequency_quotient(0, 0), 0x00ff_ffff);
        assert_eq!(nrx_frequency_quotient(0x19, 7), 0x0049_2493);
    }
}
