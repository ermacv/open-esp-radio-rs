//! Owned ESP32-S31 PHY frequency and channel register leaves.
//!
//! Addresses, masks, values, and access order come from complete rev0 ROM
//! bodies and the pinned `libphy.a`. Internal electrical meanings are not
//! public, so operation-derived PAC fields retain `UNKNOWN` where appropriate.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{
    power::{phy_frequency_channel_oracle, phy_i2c_command_ram},
    Field32, Register32,
};

#[cfg(any(test, target_arch = "riscv32"))]
const fn field_value(field: Field32, value: u32) -> u32 {
    match field.checked_value(value) {
        Some(value) => value,
        None => panic!("value does not fit recovered register field"),
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);

    fn modify(&mut self, register: Register32, clear_mask: u32, set_bits: u32) {
        let previous = self.read(register);
        self.write(register, (previous & !clear_mask) | (set_bits & clear_mask));
    }
}

#[cfg(target_arch = "riscv32")]
impl RegisterIo for RadioRegisters {
    fn read(&mut self, register: Register32) -> u32 {
        self.read32(register)
    }

    fn write(&mut self, register: Register32, value: u32) {
        self.write32(register, value);
    }
}

/// Clear the two low Wi-Fi control bits at the start of PHY ownership.
///
/// Pinned `libphy.a[phy_init.o]::register_chipv7_phy`, size `0x1e6`, performs
/// this fresh-read update before `phy_force_txrx_off`. Bit 1 is independently
/// identified by complete ROM `phy_wifi_enable_set`; bit 0 remains unknown.
#[cfg(target_arch = "riscv32")]
pub fn prepare_wifi_control(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
) {
    crate::wifi_bb::prepare_cold_start(platform);
    registers.set_wifi_baseband_enabled_image(false);
}

/// Select the two-bit baseband mode used by the Rust cold-init transition.
///
/// Complete pinned `libphy.a[phy_init.o]::phy_bb_init`, size `0x16a`, writes
/// mode two before calibration and mode zero after its initial channel change.
#[cfg(target_arch = "riscv32")]
pub fn set_baseband_mode(registers: &mut RadioRegisters, mode: u8) {
    set_baseband_mode_with(registers, mode);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_baseband_mode_with(io: &mut impl RegisterIo, mode: u8) {
    let field = phy_frequency_channel_oracle::frequency_parameter_1_status::BASEBAND_MODE_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS,
        field.mask(),
        field_value(field, u32::from(mode)),
    );
}

/// Pulse the complete rev0 ROM `phy_freq_module_resetn` reset/release bit.
///
/// The body at `0x2f82_4abe`, size `0x1c`, clears then sets bit 18 with a
/// fresh read before each store. Frequency-memory mode reuses the same bit as
/// address bit ten; this method makes no claim that the roles are independent.
#[cfg(target_arch = "riscv32")]
pub fn reset_module(registers: &mut RadioRegisters) {
    reset_module_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn reset_module_with(io: &mut impl RegisterIo) {
    let field = phy_frequency_channel_oracle::frequency_control::
        MEMORY_ADDRESS_HIGH_OR_MODULE_RESET_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        field.mask(),
        0,
    );
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        field.mask(),
        field.mask(),
    );
}

/// Select whether hardware owns frequency updates.
///
/// Complete rev0 ROM `phy_dis_hw_set_freq` at `0x2f82_4fb2`, size `0x14`,
/// sets bit 31 before its two-microsecond delay. Complete
/// `phy_en_hw_set_freq` at `0x2f82_4f9e`, size `0x14`, clears the bit. Delay
/// ownership remains in the caller's async state machine.
#[cfg(target_arch = "riscv32")]
pub fn set_hardware_control(registers: &mut RadioRegisters, enabled: bool) {
    set_hardware_control_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_hardware_control_with(io: &mut impl RegisterIo, enabled: bool) {
    let disable = phy_frequency_channel_oracle::frequency_control::HARDWARE_FREQUENCY_DISABLE;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        disable.mask(),
        if enabled { 0 } else { disable.mask() },
    );
}

/// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
///
/// The body at `0x2f82_4c46`, size `0x60`, performs three fresh-read control
/// updates followed by full writes of `0x19800249` and `0x25824e58`.
/// `parameter_override` makes the ROM's hidden `phy_param[0x193]` branch
/// explicit and selects the recovered `(0, 2)` mode encoding.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(registers: &mut RadioRegisters, parameter_override: bool) {
    initialize_registers_with(registers, parameter_override);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_registers_with(io: &mut impl RegisterIo, parameter_override: bool) {
    let switch = phy_frequency_channel_oracle::frequency_control::CHANNEL_SWITCH_PULSE;
    let hardware = phy_frequency_channel_oracle::frequency_control::HARDWARE_FREQUENCY_DISABLE;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        switch.mask() | hardware.mask(),
        0,
    );

    let enabled = phy_frequency_channel_oracle::frequency_control::MODULE_ENABLE_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        enabled.mask(),
        enabled.mask(),
    );

    let mode = phy_frequency_channel_oracle::frequency_control::REGISTER_MODE_UNKNOWN;
    let mode_value = if parameter_override { 0x20 } else { 0x42 };
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        mode.mask(),
        field_value(mode, mode_value),
    );
    io.write(
        phy_frequency_channel_oracle::FREQUENCY_PARAMETER_0_OPAQUE,
        0x1980_0249,
    );
    io.write(
        phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS,
        0x2582_4e58,
    );
}

/// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
///
/// The body at `0x2f82_4bb6`, size `0x3e`, replaces an eleven-bit address,
/// publishes one 24-bit data/eight-bit mode word, then sets and clears the
/// bit-20 write pulse with fresh reads.
#[cfg(target_arch = "riscv32")]
pub fn write_memory(registers: &mut RadioRegisters, address: u16, value: u32, mode: u8) {
    write_memory_with(registers, address, value, mode);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn write_memory_with(io: &mut impl RegisterIo, address: u16, value: u32, mode: u8) {
    let low = phy_frequency_channel_oracle::frequency_control::MEMORY_ADDRESS_LOW_UNKNOWN;
    let high = phy_frequency_channel_oracle::frequency_control::
        MEMORY_ADDRESS_HIGH_OR_MODULE_RESET_UNKNOWN;
    let address = u32::from(address);
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        low.mask() | high.mask(),
        field_value(low, address & 0x3ff) | field_value(high, (address >> 10) & 1),
    );

    let data = phy_frequency_channel_oracle::frequency_memory_data::DATA;
    let mode_field = phy_frequency_channel_oracle::frequency_memory_data::MODE;
    io.write(
        phy_frequency_channel_oracle::FREQUENCY_MEMORY_DATA,
        field_value(data, value) | field_value(mode_field, u32::from(mode)),
    );

    let pulse = phy_frequency_channel_oracle::frequency_control::MEMORY_WRITE_PULSE;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        pulse.mask(),
        0,
    );
}

/// Publish the complete rev0 ROM `phy_freq_i2c_num_addr` register image.
///
/// The body at `0x2f82_4cca`, size `0x6a`, performs one fresh-read ten-bit
/// control replacement followed by three full packed-word stores. The Rust
/// transition prepares all eleven five-bit number addresses without retaining
/// the ROM's input pointer.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_number_addresses(
    registers: &mut RadioRegisters,
    control_field: u32,
    words: [u32; 3],
) {
    configure_i2c_number_addresses_with(registers, control_field, words);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_i2c_number_addresses_with(
    io: &mut impl RegisterIo,
    control_field: u32,
    words: [u32; 3],
) {
    let first = phy_frequency_channel_oracle::i2c_number_control::NUMBER_ADDRESS_0_UNKNOWN;
    let second = phy_frequency_channel_oracle::i2c_number_control::NUMBER_ADDRESS_1_UNKNOWN;
    let mask = first.mask() | second.mask();
    io.modify(
        phy_frequency_channel_oracle::I2C_NUMBER_CONTROL,
        mask,
        control_field,
    );
    for (register, value) in phy_frequency_channel_oracle::I2C_NUMBER_WORD
        .into_iter()
        .zip(words)
    {
        io.write(register, value);
    }
}

/// Publish the first half of complete rev0 ROM `phy_freq_chan_en_sw`.
///
/// The body at `0x2f82_4ada`, size `0x38`, first replaces the low channel
/// index and then sets bit 19 using a second fresh read. Its one-microsecond
/// delay and final clear are explicit separate async transition steps.
#[cfg(target_arch = "riscv32")]
pub fn start_channel_switch(registers: &mut RadioRegisters, frequency_index: u8) {
    start_channel_switch_with(registers, frequency_index);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn start_channel_switch_with(io: &mut impl RegisterIo, frequency_index: u8) {
    let index = phy_frequency_channel_oracle::frequency_control::CHANNEL_INDEX;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        index.mask(),
        field_value(index, u32::from(frequency_index)),
    );
    let pulse = phy_frequency_channel_oracle::frequency_control::CHANNEL_SWITCH_PULSE;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
}

/// Complete the delayed clear from rev0 ROM `phy_freq_chan_en_sw`.
#[cfg(target_arch = "riscv32")]
pub fn clear_channel_switch(registers: &mut RadioRegisters) {
    clear_channel_switch_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn clear_channel_switch_with(io: &mut impl RegisterIo) {
    let pulse = phy_frequency_channel_oracle::frequency_control::CHANNEL_SWITCH_PULSE;
    io.modify(
        phy_frequency_channel_oracle::FREQUENCY_CONTROL,
        pulse.mask(),
        0,
    );
}

/// Sample the channel parent's frequency-ready word exactly once.
///
/// Complete pinned `libphy.a[phy_rfpll.o]::phy_chip_set_chan`, size `0x10e`,
/// samples `FREQUENCY_PARAMETER_1_STATUS[8]` after the external settle delay.
/// This leaf deliberately returns the full word expected by the owned state
/// machine and never waits or resamples.
#[cfg(target_arch = "riscv32")]
pub fn sample_frequency_ready(registers: &mut RadioRegisters) -> u32 {
    sample_frequency_ready_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn sample_frequency_ready_with(io: &mut impl RegisterIo) -> u32 {
    io.read(phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS)
}

/// Apply complete rev0 ROM `phy_nrx_freq_set`.
///
/// The body at `0x2f82_80ec`, size `0x32`, performs two independent reads,
/// computes `(80 << high_byte) / frequency`, preserves the second read's high
/// byte, clears bits 23:20, and publishes the low twenty-bit quotient.
#[cfg(target_arch = "riscv32")]
pub fn configure_nrx_frequency(registers: &mut RadioRegisters, frequency: u16) {
    configure_nrx_frequency_with(registers, frequency);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_nrx_frequency_with(io: &mut impl RegisterIo, frequency: u16) {
    assert!(frequency != 0, "NRX frequency must be nonzero");
    let register = phy_frequency_channel_oracle::NRX_FREQUENCY_CONTROL;
    let shift_source = io.read(register);
    let previous = io.read(register);
    let shift = shift_source >> 24;
    let quotient = 0x50_u32.wrapping_shl(shift) / u32::from(frequency);

    let quotient_field =
        phy_frequency_channel_oracle::nrx_frequency_control::FREQUENCY_QUOTIENT_OR_INIT_LOW_UNKNOWN;
    let shift_low =
        phy_frequency_channel_oracle::nrx_frequency_control::SHIFT_LOW_OR_INIT_HIGH_UNKNOWN;
    let shift_high = phy_frequency_channel_oracle::nrx_frequency_control::SHIFT_HIGH_UNKNOWN;
    io.write(
        register,
        (previous & (shift_low.mask() | shift_high.mask()))
            | field_value(quotient_field, quotient & quotient_field.max_value()),
    );
}

/// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
///
/// The body at `0x2f82_79c6`, size `0x140`, first replaces bits 23:0 with
/// `0x0433af`, then replaces bits 28:24 with `0x17`, using a fresh read for
/// each update. The rest of that ROM body remains sequenced by its caller.
#[cfg(target_arch = "riscv32")]
pub fn initialize_nrx_baseband(registers: &mut RadioRegisters) {
    initialize_nrx_baseband_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_nrx_baseband_with(io: &mut impl RegisterIo) {
    let low =
        phy_frequency_channel_oracle::nrx_frequency_control::FREQUENCY_QUOTIENT_OR_INIT_LOW_UNKNOWN;
    let middle = phy_frequency_channel_oracle::nrx_frequency_control::INIT_MIDDLE_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::NRX_FREQUENCY_CONTROL,
        low.mask() | middle.mask(),
        0x0004_33af,
    );
    let high = phy_frequency_channel_oracle::nrx_frequency_control::SHIFT_LOW_OR_INIT_HIGH_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::NRX_FREQUENCY_CONTROL,
        high.mask(),
        field_value(high, 0x17),
    );
}

/// Set the single shared Wi-Fi baseband bit used by `phy_bb_reg_init`.
///
/// Complete rev0 ROM `phy_bb_reg_init` sets bit 11. The independently
/// recovered three-bit AGC-update field occupies bits 13:11, so this method
/// sets only its low encoding bit and preserves the other two.
#[cfg(target_arch = "riscv32")]
pub fn set_baseband_init_control(platform: &mut impl crate::wifi_bb::PhyWifiBbControl) {
    crate::wifi_bb::set_baseband_init_control(platform);
}

#[cfg(any(test, target_arch = "riscv32"))]
const fn bss_tx_offset(cbw: u8) -> u32 {
    if cbw == 2 {
        2
    } else if cbw == 3 {
        1
    } else {
        0
    }
}

/// Apply complete rev0 ROM `phy_bb_bss_cbw40` and all three finite children.
///
/// The parent at `0x2f82_6052`, size `0x2e`, calls complete
/// `phy_mac_tx_chan_offset`, `phy_bb_bss_cbw40_dig`, and
/// `phy_wifi_fbw_sel`. This method retains their five fresh-read writes and
/// exact zero/nonzero CBW branches.
#[cfg(target_arch = "riscv32")]
pub fn configure_bss_cbw(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
    cbw: u8,
) {
    configure_bss_cbw_prefix_with(registers, cbw);
    crate::wifi_bb::set_bss_cbw_40_digital(platform, cbw != 0);
    configure_bss_cbw_suffix_with(registers, cbw);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_bss_cbw_prefix_with(io: &mut impl RegisterIo, cbw: u8) {
    let offset = phy_frequency_channel_oracle::channel_tx_offset_control::CHANNEL_OFFSET_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::CHANNEL_TX_OFFSET_CONTROL,
        offset.mask(),
        field_value(offset, bss_tx_offset(cbw)),
    );
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_bss_cbw_suffix_with(io: &mut impl RegisterIo, cbw: u8) {
    let clear_low = phy_frequency_channel_oracle::fbw_bt_filter_control::FBW_CLEAR_LOW_UNKNOWN;
    let clear_high = phy_frequency_channel_oracle::fbw_bt_filter_control::FBW_CLEAR_HIGH_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        clear_low.mask() | clear_high.mask(),
        0,
    );

    let middle = phy_frequency_channel_oracle::fbw_bt_filter_control::FBW_SELECT_MID_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        middle.mask(),
        field_value(middle, u32::from(cbw != 0)),
    );

    let high = phy_frequency_channel_oracle::fbw_bt_filter_control::FBW_SELECT_HIGH_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        high.mask(),
        field_value(high, u32::from(cbw != 0)),
    );
}

/// Publish the TX-cap readback exactly as ROM `phy_i2c_master_mem_txcap`.
///
/// The complete body at `0x2f82_a832`, size `0x24`, reads PHY-I2C block
/// `0x6b`, host one, register two, then writes
/// `value << 16 | 0x026b` to command-memory entry one. The Rust channel
/// transition owns the preceding I2C read and passes only its byte here.
#[cfg(target_arch = "riscv32")]
pub fn publish_tx_cap(registers: &mut RadioRegisters, value: u8) {
    publish_tx_cap_with(registers, value);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn publish_tx_cap_with(io: &mut impl RegisterIo, value: u8) {
    let block = phy_i2c_command_ram::command_memory::BLOCK;
    let register = phy_i2c_command_ram::command_memory::REGISTER;
    let data = phy_i2c_command_ram::command_memory::DATA;
    io.write(
        phy_i2c_command_ram::COMMAND_MEMORY[1],
        field_value(block, 0x6b) | field_value(register, 2) | field_value(data, u32::from(value)),
    );
}

#[cfg(any(test, target_arch = "riscv32"))]
#[derive(Clone, Copy)]
struct ChannelCbwFields {
    tx_offset: u32,
    control_0: u32,
    control_1_high: u32,
    control_1_low: u32,
}

#[cfg(any(test, target_arch = "riscv32"))]
const fn channel_cbw_fields(cbw: u8) -> ChannelCbwFields {
    let high = cbw >> 4;
    if high != 0 {
        let normalized = high.wrapping_sub(1) as u32;
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized & 3,
            control_1_high: (normalized >> 2) & 7,
            control_1_low: normalized & 3,
        }
    } else {
        let low = cbw & 0x0f;
        let normalized = if low < 2 { 0 } else { low - 2 } as u32;
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized & 3,
            control_1_high: if low != 0 { 1 } else { 0 },
            control_1_low: if cbw & 0x0e != 0 { 1 } else { 0 },
        }
    }
}

/// Apply complete rev0 ROM `phy_bb_cbw_chan_cfg`.
///
/// The body at `0x2f82_8238`, size `0x74`, derives four fields from the CBW
/// byte and performs four fresh-read replacements in TX-offset, control-zero,
/// control-one-high, control-one-low order.
#[cfg(target_arch = "riscv32")]
pub fn configure_channel_cbw(registers: &mut RadioRegisters, cbw: u8) {
    configure_channel_cbw_with(registers, cbw);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_channel_cbw_with(io: &mut impl RegisterIo, cbw: u8) {
    let fields = channel_cbw_fields(cbw);
    let offset = phy_frequency_channel_oracle::channel_tx_offset_control::CHANNEL_OFFSET_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::CHANNEL_TX_OFFSET_CONTROL,
        offset.mask(),
        field_value(offset, fields.tx_offset),
    );

    let control_0 = phy_frequency_channel_oracle::channel_cbw_control_0::CBW_LOW_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::CHANNEL_CBW_CONTROL_0,
        control_0.mask(),
        field_value(control_0, fields.control_0),
    );

    let high = phy_frequency_channel_oracle::channel_cbw_control_1::CBW_HIGH_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::CHANNEL_CBW_CONTROL_1,
        high.mask(),
        field_value(high, fields.control_1_high),
    );
    let low = phy_frequency_channel_oracle::channel_cbw_control_1::CBW_LOW_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::CHANNEL_CBW_CONTROL_1,
        low.mask(),
        field_value(low, fields.control_1_low),
    );
}

/// Apply complete rev0 ROM `phy_wifi_enable_set`.
///
/// The body at `0x2f82_8220`, size `0x18`, performs one fresh-read set or
/// clear of the instruction- and symbol-identified Wi-Fi enable bit.
#[cfg(target_arch = "riscv32")]
pub fn set_wifi_enabled(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
    enabled: bool,
) {
    crate::wifi_bb::set_wifi_enabled(platform, enabled);
    registers.set_wifi_baseband_enabled_image(enabled);
}

/// Apply complete rev0 ROM `phy_mac_enable_bb`.
///
/// The body at `0x2f82_7836`, size `0x2a`, sets bit 28, clears Wi-Fi enable,
/// then sets Wi-Fi enable, with a fresh read before every store.
#[cfg(target_arch = "riscv32")]
pub fn enable_mac_baseband(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
) {
    crate::wifi_bb::enable_mac_baseband(platform);
    registers.set_wifi_baseband_enabled_image(true);
}

/// Apply complete rev0 ROM `phy_bt_filter_reg`.
///
/// The body at `0x2f82_7e90`, size `0x34`, sets bit 25, clears bit 22, then
/// clears bits 24:23, using a fresh read for each independent update.
#[cfg(target_arch = "riscv32")]
pub fn configure_bt_filter(registers: &mut RadioRegisters) {
    configure_bt_filter_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_bt_filter_with(io: &mut impl RegisterIo) {
    let enable = phy_frequency_channel_oracle::fbw_bt_filter_control::BT_FILTER_ENABLE_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        enable.mask(),
        enable.mask(),
    );
    let low = phy_frequency_channel_oracle::fbw_bt_filter_control::BT_FILTER_LOW_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        low.mask(),
        0,
    );
    let mode = phy_frequency_channel_oracle::fbw_bt_filter_control::BT_FILTER_MODE_UNKNOWN;
    io.modify(
        phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL,
        mode.mask(),
        0,
    );
}

#[cfg(test)]
mod tests {
    use std::{vec, vec::Vec};

    use super::{
        clear_channel_switch_with, configure_bss_cbw_prefix_with, configure_bss_cbw_suffix_with,
        configure_bt_filter_with, configure_channel_cbw_with, configure_i2c_number_addresses_with,
        configure_nrx_frequency_with, initialize_nrx_baseband_with, initialize_registers_with,
        publish_tx_cap_with, reset_module_with, sample_frequency_ready_with,
        set_baseband_mode_with, set_hardware_control_with, start_channel_switch_with,
        write_memory_with, RegisterIo,
    };
    use open_esp_radio_pac_esp32s31::{
        power::{phy_frequency_channel_oracle, phy_i2c_command_ram},
        Register32,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Read(Register32),
        Write(Register32, u32),
    }

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        operations: Vec<Operation>,
    }

    impl FakeRegisters {
        fn with(mut self, register: Register32, value: u32) -> Self {
            self.values.push((register, value));
            self
        }

        fn value(&self, register: Register32) -> u32 {
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
        }
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            self.operations.push(Operation::Read(register));
            self.value(register)
        }

        fn write(&mut self, register: Register32, value: u32) {
            if let Some(entry) = self
                .values
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                entry.1 = value;
            } else {
                self.values.push((register, value));
            }
            self.operations.push(Operation::Write(register, value));
        }
    }

    #[test]
    fn frequency_control_initialization_retains_all_fresh_rom_edges() {
        let control = phy_frequency_channel_oracle::FREQUENCY_CONTROL;
        let mut io = FakeRegisters::default().with(control, 0xffff_ffff);

        reset_module_with(&mut io);
        set_hardware_control_with(&mut io, false);
        initialize_registers_with(&mut io, false);

        assert_eq!(
            io.operations,
            vec![
                Operation::Read(control),
                Operation::Write(control, 0xfffb_ffff),
                Operation::Read(control),
                Operation::Write(control, 0xffff_ffff),
                Operation::Read(control),
                Operation::Write(control, 0xffff_ffff),
                Operation::Read(control),
                Operation::Write(control, 0x7ff7_ffff),
                Operation::Read(control),
                Operation::Write(control, 0x7ff7_ffff),
                Operation::Read(control),
                Operation::Write(control, 0x50b7_ffff),
                Operation::Write(
                    phy_frequency_channel_oracle::FREQUENCY_PARAMETER_0_OPAQUE,
                    0x1980_0249,
                ),
                Operation::Write(
                    phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS,
                    0x2582_4e58,
                ),
            ]
        );
    }

    #[test]
    fn frequency_memory_keeps_address_data_set_clear_order() {
        let control = phy_frequency_channel_oracle::FREQUENCY_CONTROL;
        let mut io = FakeRegisters::default().with(control, 0xa5f8_00a5);

        write_memory_with(&mut io, 0x712, 0x00ab_cdef, 7);

        assert_eq!(
            io.operations,
            vec![
                Operation::Read(control),
                Operation::Write(control, 0xa5ff_12a5),
                Operation::Write(
                    phy_frequency_channel_oracle::FREQUENCY_MEMORY_DATA,
                    0x07ab_cdef,
                ),
                Operation::Read(control),
                Operation::Write(control, 0xa5ff_12a5),
                Operation::Read(control),
                Operation::Write(control, 0xa5ef_12a5),
            ]
        );
    }

    #[test]
    fn packed_i2c_number_addresses_use_one_rmw_and_three_stores() {
        let control = phy_frequency_channel_oracle::I2C_NUMBER_CONTROL;
        let mut io = FakeRegisters::default().with(control, 0xa5fc_00a5);
        let words = [0x0123_4567, 0x089a_bcde, 0];

        configure_i2c_number_addresses_with(&mut io, 0x0000_a400, words);

        assert_eq!(
            io.operations,
            vec![
                Operation::Read(control),
                Operation::Write(control, 0xa5fc_a4a5),
                Operation::Write(phy_frequency_channel_oracle::I2C_NUMBER_WORD[0], words[0]),
                Operation::Write(phy_frequency_channel_oracle::I2C_NUMBER_WORD[1], words[1]),
                Operation::Write(phy_frequency_channel_oracle::I2C_NUMBER_WORD[2], words[2]),
            ]
        );
    }

    #[test]
    fn channel_switch_and_ready_sample_are_independent_edges() {
        let control = phy_frequency_channel_oracle::FREQUENCY_CONTROL;
        let status = phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS;
        let mut io = FakeRegisters::default()
            .with(control, 0x1234_56aa)
            .with(status, 0x2582_4f58);

        start_channel_switch_with(&mut io, 62);
        clear_channel_switch_with(&mut io);
        assert_eq!(sample_frequency_ready_with(&mut io), 0x2582_4f58);

        assert_eq!(
            io.operations,
            vec![
                Operation::Read(control),
                Operation::Write(control, 0x1234_563e),
                Operation::Read(control),
                Operation::Write(control, 0x123c_563e),
                Operation::Read(control),
                Operation::Write(control, 0x1234_563e),
                Operation::Read(status),
            ]
        );
    }

    #[test]
    fn nrx_uses_two_reads_and_baseband_init_uses_two_more() {
        let register = phy_frequency_channel_oracle::NRX_FREQUENCY_CONTROL;
        let mut io = FakeRegisters::default().with(register, 0x0200_1234);

        configure_nrx_frequency_with(&mut io, 2_462);
        initialize_nrx_baseband_with(&mut io);

        assert_eq!(
            io.operations,
            vec![
                Operation::Read(register),
                Operation::Read(register),
                Operation::Write(register, 0x0200_0000),
                Operation::Read(register),
                Operation::Write(register, 0x0204_33af),
                Operation::Read(register),
                Operation::Write(register, 0x1704_33af),
            ]
        );
    }

    #[test]
    fn bss_and_channel_cbw_retain_all_complete_rom_branches() {
        let tx = phy_frequency_channel_oracle::CHANNEL_TX_OFFSET_CONTROL;
        let fbw = phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL;
        let control_0 = phy_frequency_channel_oracle::CHANNEL_CBW_CONTROL_0;
        let control_1 = phy_frequency_channel_oracle::CHANNEL_CBW_CONTROL_1;
        let mut io = FakeRegisters::default()
            .with(tx, 0x1234_567f)
            .with(fbw, 0x03ff_ffff)
            .with(control_0, 0xa5a5_5a5a)
            .with(control_1, 0xa5a5_5a5a);

        configure_bss_cbw_prefix_with(&mut io, 2);
        configure_bss_cbw_suffix_with(&mut io, 2);
        configure_channel_cbw_with(&mut io, 0x50);

        assert_eq!(io.value(tx), 0x1234_5674);
        assert_eq!(io.value(fbw), 0x03d2_ffff);
        assert_eq!(io.value(control_0), 0xa5a5_5a58);
        assert_eq!(io.value(control_1), 0xa5a5_5a44);
    }

    #[test]
    fn shared_frequency_and_filter_methods_preserve_fresh_read_order() {
        let filter = phy_frequency_channel_oracle::FBW_BT_FILTER_CONTROL;
        let status = phy_frequency_channel_oracle::FREQUENCY_PARAMETER_1_STATUS;
        let mut io = FakeRegisters::default()
            .with(filter, u32::MAX)
            .with(status, u32::MAX);

        set_baseband_mode_with(&mut io, 2);
        configure_bt_filter_with(&mut io);

        assert_eq!(io.value(status), 0xffff_fffe);
        assert_eq!(io.value(filter), 0xfe3f_ffff);
    }

    #[test]
    fn tx_cap_uses_the_existing_command_ram_layout() {
        let mut io = FakeRegisters::default();
        publish_tx_cap_with(&mut io, 0xa5);
        assert_eq!(
            io.operations,
            vec![Operation::Write(
                phy_i2c_command_ram::COMMAND_MEMORY[1],
                0x00a5_026b,
            )]
        );
    }
}
