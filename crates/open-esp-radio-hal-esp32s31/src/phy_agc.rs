//! Owned ESP32-S31 PHY AGC register leaves.
//!
//! Register addresses, masks, values, and access order come from complete
//! rev0 ROM and pinned `libphy.a` bodies. Internal electrical meanings are
//! not public, so the PAC deliberately retains `UNKNOWN` names instead of
//! borrowing names from a neighboring chip.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{
    power::{modem_syscon, phy_agc_oracle},
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

/// Apply complete rev0 ROM `phy_bb_agc_reg_update`.
///
/// The body at `0x2f82_860e`, size `0xa6`, has no call, loop, wait, callback,
/// or software-global access. This operation preserves all fifteen writes
/// and fresh-read updates in instruction order. OPAQUE full-word values are
/// intentionally kept as exact ROM constants.
#[cfg(target_arch = "riscv32")]
pub fn update_baseband_registers(registers: &mut RadioRegisters) {
    update_baseband_registers_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn update_baseband_registers_with(io: &mut impl RegisterIo) {
    io.write(phy_agc_oracle::AGC_UPDATE_8070_OPAQUE, 0x0000_08c7);
    io.write(phy_agc_oracle::AGC_UPDATE_78A4_OPAQUE, 0x0001_721f);

    let clear = phy_agc_oracle::rx_11b_mode_control::BB_AGC_UPDATE_CLEAR_UNKNOWN;
    io.modify(phy_agc_oracle::RX_11B_MODE_CONTROL, clear.mask(), 0);

    io.write(phy_agc_oracle::AGC_UPDATE_8010_OPAQUE, 0x0008_52a1);
    io.write(phy_agc_oracle::AGC_UPDATE_8018_OPAQUE, 0x0060_0030);
    io.write(phy_agc_oracle::AGC_UPDATE_801C_OPAQUE, 0x0100_00a0);
    io.write(phy_agc_oracle::AGC_UPDATE_8020_OPAQUE, 0x0000_0180);
    io.write(phy_agc_oracle::AGC_UPDATE_8028_OPAQUE, 0xc040_3020);
    io.write(phy_agc_oracle::AGC_UPDATE_802C_OPAQUE, 0x0100_0080);

    let set = phy_agc_oracle::agc_update_8078_control::BB_AGC_UPDATE_SET_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_UPDATE_8078_CONTROL,
        set.mask(),
        set.mask(),
    );

    io.write(phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0xfe3f_e1fe);
    io.write(phy_agc_oracle::AGC_UPDATE_7048_OPAQUE, 0xff7d_a4f3);
    io.write(phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0x06ac_c7c8);
    io.write(phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0xb220_8553);

    let enable = modem_syscon::wifi_bb_cfg::BB_AGC_UPDATE_ENABLE_UNKNOWN;
    io.modify(modem_syscon::WIFI_BB_CFG, enable.mask(), enable.mask());
}

/// Select the exact AGC state used by the open channel transition.
///
/// `enabled=false` reproduces complete rev0 ROM `phy_disable_agc` at
/// `0x2f82_7460`, size `0x10`. `enabled=true` reproduces complete
/// `phy_enable_agc` at `0x2f82_7470`, size `0x28`: clear the disable bit, then
/// set and clear the enable pulse with a fresh read before each write.
#[cfg(target_arch = "riscv32")]
pub fn set_enabled(registers: &mut RadioRegisters, enabled: bool) {
    set_enabled_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_enabled_with(io: &mut impl RegisterIo, enabled: bool) {
    let disable = phy_agc_oracle::agc_antenna_control::AGC_DISABLE_UNKNOWN;
    if !enabled {
        io.modify(
            phy_agc_oracle::AGC_ANTENNA_CONTROL,
            disable.mask(),
            disable.mask(),
        );
        return;
    }

    io.modify(phy_agc_oracle::AGC_ANTENNA_CONTROL, disable.mask(), 0);
    let pulse = phy_agc_oracle::agc_shared_control::PULSE_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
    io.modify(phy_agc_oracle::AGC_SHARED_CONTROL, pulse.mask(), 0);
}

/// Apply complete rev0 ROM `phy_agc_reg_init`.
///
/// The body at `0x2f82_78d8`, size `0xd8`, performs ten independently read
/// register updates with no call, loop, wait, callback, or software-global
/// access. Both inputs are the caller-owned parameter bytes at offsets
/// `0x121` and `0x120`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(registers: &mut RadioRegisters, parameter_121: u8, parameter_120: u8) {
    initialize_registers_with(registers, parameter_121, parameter_120);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_registers_with(io: &mut impl RegisterIo, parameter_121: u8, parameter_120: u8) {
    let limit = phy_agc_oracle::rx_gain_limit_control::RX_GAIN_LIMIT_UNKNOWN;
    let gain_minus_one = u32::from(parameter_121.wrapping_sub(1) & 0x7f);
    io.modify(
        phy_agc_oracle::RX_GAIN_LIMIT_CONTROL,
        limit.mask(),
        field_value(limit, gain_minus_one),
    );

    let low_limit = phy_agc_oracle::agc_gain_limit_low::PARAMETER_MINUS_ONE_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_GAIN_LIMIT_LOW,
        low_limit.mask(),
        field_value(low_limit, gain_minus_one),
    );

    let gain_index = phy_agc_oracle::agc_shared_control::RX_GAIN_INDEX_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        gain_index.mask(),
        field_value(gain_index, u32::from(parameter_121 & 0x7f)),
    );

    let saturation_low = phy_agc_oracle::agc_saturation_control::LOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SATURATION_CONTROL,
        saturation_low.mask(),
        field_value(saturation_low, 0x0bb8),
    );

    let parameter = phy_agc_oracle::agc_parameter_control::PARAMETER_121_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_PARAMETER_CONTROL,
        parameter.mask(),
        field_value(parameter, u32::from(parameter_121)),
    );
    let parameter_offset = phy_agc_oracle::agc_parameter_control::PARAMETER_120_OFFSET_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_PARAMETER_CONTROL,
        parameter_offset.mask(),
        field_value(
            parameter_offset,
            u32::from(parameter_120).wrapping_add(0x50) & 0xff,
        ),
    );

    let high = phy_agc_oracle::agc_shared_control::CONTROL_HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        high.mask(),
        field_value(high, 0x32),
    );
    let pulse = phy_agc_oracle::agc_shared_control::PULSE_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
    io.modify(phy_agc_oracle::AGC_SHARED_CONTROL, pulse.mask(), 0);

    let final_high = phy_agc_oracle::agc_init_high_control::INIT_HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_INIT_HIGH_CONTROL,
        final_high.mask(),
        field_value(final_high, 0xd2),
    );
}

/// Apply complete pinned `phy_set_rx_comp_new`.
///
/// The 0x28-byte `libphy.a[phy_reg.o]` body performs exactly two fresh-read
/// field replacements, in low-then-high order, with no call, loop, wait,
/// callback, or software-global access.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_compensation(registers: &mut RadioRegisters) {
    configure_rx_compensation_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rx_compensation_with(io: &mut impl RegisterIo) {
    let low = phy_agc_oracle::agc_shared_control::RX_COMPENSATION_LOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        low.mask(),
        field_value(low, 0xed),
    );

    let high = phy_agc_oracle::rx_compensation_high_control::RX_COMPENSATION_HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::RX_COMPENSATION_HIGH_CONTROL,
        high.mask(),
        field_value(high, 0xed),
    );
}

/// Apply the two MMIO updates after the 1 us edge in `phy_pbus_force_mode`.
///
/// Complete rev0 ROM first replaces the shared high byte with `0x32`, then
/// sets the shared pulse bit using another fresh read. Delay ownership stays
/// in the caller's async state machine.
#[cfg(target_arch = "riscv32")]
pub fn configure_pbus_work_mode_pulse(registers: &mut RadioRegisters) {
    configure_pbus_work_mode_pulse_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_pbus_work_mode_pulse_with(io: &mut impl RegisterIo) {
    let high = phy_agc_oracle::agc_shared_control::CONTROL_HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        high.mask(),
        field_value(high, 0x32),
    );

    let pulse = phy_agc_oracle::agc_shared_control::PULSE_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
}

/// Clear the shared pulse bit after the 2 us `phy_pbus_force_mode` edge.
#[cfg(target_arch = "riscv32")]
pub fn clear_pbus_work_mode_pulse(registers: &mut RadioRegisters) {
    clear_pbus_work_mode_pulse_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn clear_pbus_work_mode_pulse_with(io: &mut impl RegisterIo) {
    let pulse = phy_agc_oracle::agc_shared_control::PULSE_UNKNOWN;
    io.modify(phy_agc_oracle::AGC_SHARED_CONTROL, pulse.mask(), 0);
}

/// Apply complete rev0 ROM `phy_ant_init`.
///
/// The body at `0x2f82_7df4`, size `0x44`, performs three fresh-read updates.
/// The shared middle register retains one PAC identity with the independent
/// AGC-disable field; unrelated bits are preserved.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna(registers: &mut RadioRegisters) {
    configure_antenna_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_antenna_with(io: &mut impl RegisterIo) {
    let low = phy_agc_oracle::antenna_control_0::LOW_CLEAR_UNKNOWN;
    let bit_12 = phy_agc_oracle::antenna_control_0::BIT_12_CLEAR_UNKNOWN;
    io.modify(
        phy_agc_oracle::ANTENNA_CONTROL_0,
        low.mask() | bit_12.mask(),
        0,
    );

    let middle = phy_agc_oracle::agc_antenna_control::ANTENNA_INIT_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_ANTENNA_CONTROL,
        middle.mask(),
        field_value(middle, 0x34),
    );

    let low = phy_agc_oracle::antenna_control_2::LOW_UNKNOWN;
    let high = phy_agc_oracle::antenna_control_2::HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::ANTENNA_CONTROL_2,
        low.mask() | high.mask(),
        field_value(low, 0x1e) | field_value(high, 0x1e),
    );
}

/// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
///
/// The body at `0x2f82_8944`, size `0x42`, first writes the common full-word
/// configuration, then performs two fresh-read updates of the shared
/// saturation control. `enabled=false` is the pre-check phase and
/// `enabled=true` is the post-gain-table phase.
#[cfg(target_arch = "riscv32")]
pub fn configure_rf_rx_saturation(registers: &mut RadioRegisters, enabled: bool) {
    configure_rf_rx_saturation_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rf_rx_saturation_with(io: &mut impl RegisterIo, enabled: bool) {
    io.write(phy_agc_oracle::RF_RX_SATURATION_CONFIG, 0x0000_0404);

    let bit_19 = phy_agc_oracle::agc_saturation_control::RF_RX_SATURATION_BIT_19_UNKNOWN;
    let bit_24 = phy_agc_oracle::agc_saturation_control::RF_RX_SATURATION_BIT_24_UNKNOWN;
    let bit_28 = phy_agc_oracle::agc_saturation_control::RF_RX_SATURATION_BIT_28_UNKNOWN;
    let high = phy_agc_oracle::agc_saturation_control::RF_RX_SATURATION_HIGH_UNKNOWN;
    let phase_mask = bit_19.mask() | bit_24.mask() | bit_28.mask() | high.mask();
    io.modify(
        phy_agc_oracle::AGC_SATURATION_CONTROL,
        phase_mask,
        if enabled { phase_mask } else { 0 },
    );

    let low = phy_agc_oracle::agc_saturation_control::LOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SATURATION_CONTROL,
        low.mask(),
        field_value(low, if enabled { 0x0800 } else { 0x0400 }),
    );
}

/// Publish the two final limits from complete pinned `phy_set_rx_gain_table`.
///
/// The `libphy.a[phy_rx_gain.o]` body writes the caller-owned final Wi-Fi
/// index into bits 14:8 of the shared control and the same index capped at
/// `0x4c` into bits 24:18 of the limit control. Each update uses a fresh read.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_gain_limits(registers: &mut RadioRegisters, wifi_last_index: u8) {
    configure_rx_gain_limits_with(registers, wifi_last_index);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rx_gain_limits_with(io: &mut impl RegisterIo, wifi_last_index: u8) {
    let index = phy_agc_oracle::agc_shared_control::RX_GAIN_INDEX_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SHARED_CONTROL,
        index.mask(),
        field_value(index, u32::from(wifi_last_index & 0x7f)),
    );

    let limit = phy_agc_oracle::rx_gain_limit_control::RX_GAIN_LIMIT_UNKNOWN;
    io.modify(
        phy_agc_oracle::RX_GAIN_LIMIT_CONTROL,
        limit.mask(),
        field_value(limit, u32::from(wifi_last_index.min(0x4c))),
    );
}

/// Publish one Wi-Fi AGC saturation-gain word to both recovered destinations.
///
/// Basis: complete rev0 ROM `phy_wifi_agc_sat_gain` at `0x2f82_7db0`, size
/// `0x0c`. It performs exactly two stores, with no read, call, branch, wait,
/// or hidden state.
#[cfg(target_arch = "riscv32")]
pub fn set_saturation_gain(registers: &mut RadioRegisters, value: u32) {
    set_saturation_gain_with(registers, value);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn set_saturation_gain_with(io: &mut impl RegisterIo, value: u32) {
    io.write(phy_agc_oracle::SATURATION_GAIN_LOW, value);
    io.write(phy_agc_oracle::SATURATION_GAIN_HIGH, value);
}

/// Apply complete pinned `phy_reg_update_new` and both of its finite leaves.
///
/// The `libphy.a[phy_init.o]` parent sets one AGC bit, calls the complete ROM
/// saturation-gain leaf, replaces the nine-bit window, performs two
/// independently read updates on one RX word, then tail-calls complete
/// `libphy.a[phy_reg.o]::phy_set_ftm_en(1)`. This method retains all seven
/// writes in that exact order.
#[cfg(target_arch = "riscv32")]
pub fn update_post_initialization(registers: &mut RadioRegisters) {
    update_post_initialization_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn update_post_initialization_with(io: &mut impl RegisterIo) {
    let agc = phy_agc_oracle::agc_saturation_control::POST_INIT_SET_UNKNOWN;
    io.modify(
        phy_agc_oracle::AGC_SATURATION_CONTROL,
        agc.mask(),
        agc.mask(),
    );

    set_saturation_gain_with(io, 0x0818_212d);

    let window = phy_agc_oracle::rx_11b_window_control::WINDOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::RX_11B_WINDOW_CONTROL,
        window.mask(),
        field_value(window, 0x1c0),
    );

    let low = phy_agc_oracle::post_init_rx_control::LOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::POST_INIT_RX_CONTROL,
        low.mask(),
        field_value(low, 0x17),
    );
    let high = phy_agc_oracle::post_init_rx_control::HIGH_UNKNOWN;
    io.modify(
        phy_agc_oracle::POST_INIT_RX_CONTROL,
        high.mask(),
        field_value(high, 0x17),
    );

    let ftm = phy_agc_oracle::ftm_control::ENABLE;
    io.modify(phy_agc_oracle::FTM_CONTROL, ftm.mask(), ftm.mask());
}

/// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
///
/// The body at `0x2f82_7588`, size `0xc4`, performs five fresh-read field
/// updates. Values are represented through instruction-evidenced PAC fields;
/// no neighboring-chip field name is assumed.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_11b_optimization(registers: &mut RadioRegisters, enabled: bool) {
    configure_rx_11b_optimization_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rx_11b_optimization_with(io: &mut impl RegisterIo, enabled: bool) {
    let path0_high = phy_agc_oracle::rx_11b_path_control_0::RX_11B_HIGH_UNKNOWN;
    let path0_low = phy_agc_oracle::rx_11b_path_control_0::RX_11B_LOW_UNKNOWN;
    let path1_high = phy_agc_oracle::rx_11b_path_control_1::RX_11B_HIGH_UNKNOWN;
    let path1_low = phy_agc_oracle::rx_11b_path_control_1::RX_11B_LOW_UNKNOWN;
    let mode = phy_agc_oracle::rx_11b_mode_control::RX_11B_MODE_UNKNOWN;

    let (path0_high_value, path0_low_value, path1_high_value, path1_low_value, mode_value) =
        if enabled {
            (0x3f, 0x21, 0x21, 0x03, 0x09)
        } else {
            (0x3e, 0x18, 0x18, 0x04, 0x06)
        };

    io.modify(
        phy_agc_oracle::RX_11B_PATH_CONTROL_0,
        path0_high.mask(),
        field_value(path0_high, path0_high_value),
    );
    io.modify(
        phy_agc_oracle::RX_11B_PATH_CONTROL_0,
        path0_low.mask(),
        field_value(path0_low, path0_low_value),
    );
    io.modify(
        phy_agc_oracle::RX_11B_PATH_CONTROL_1,
        path1_high.mask(),
        field_value(path1_high, path1_high_value),
    );
    io.modify(
        phy_agc_oracle::RX_11B_PATH_CONTROL_1,
        path1_low.mask(),
        field_value(path1_low, path1_low_value),
    );
    io.modify(
        phy_agc_oracle::RX_11B_MODE_CONTROL,
        mode.mask(),
        field_value(mode, mode_value),
    );

    let window = phy_agc_oracle::rx_11b_window_control::WINDOW_UNKNOWN;
    io.modify(
        phy_agc_oracle::RX_11B_WINDOW_CONTROL,
        window.mask(),
        field_value(window, 0x1c8),
    );
}

#[cfg(test)]
mod tests {
    use std::{vec, vec::Vec};

    use super::{
        clear_pbus_work_mode_pulse_with, configure_antenna_with,
        configure_pbus_work_mode_pulse_with, configure_rf_rx_saturation_with,
        configure_rx_11b_optimization_with, configure_rx_compensation_with,
        configure_rx_gain_limits_with, initialize_registers_with, set_enabled_with,
        set_saturation_gain_with, update_baseband_registers_with, update_post_initialization_with,
        RegisterIo,
    };
    use open_esp_radio_pac_esp32s31::{
        power::{modem_syscon, phy_agc_oracle},
        Register32,
    };

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        writes: Vec<(Register32, u32)>,
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
            self.writes.push((register, value));
        }
    }

    #[test]
    fn baseband_update_preserves_the_complete_rom_order() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::RX_11B_MODE_CONTROL, u32::MAX)
            .with(phy_agc_oracle::AGC_UPDATE_8078_CONTROL, 0x8000_0042)
            .with(modem_syscon::WIFI_BB_CFG, 0x0000_0002);

        update_baseband_registers_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_UPDATE_8070_OPAQUE, 0x0000_08c7),
                (phy_agc_oracle::AGC_UPDATE_78A4_OPAQUE, 0x0001_721f),
                (phy_agc_oracle::RX_11B_MODE_CONTROL, 0xfbff_ffff),
                (phy_agc_oracle::AGC_UPDATE_8010_OPAQUE, 0x0008_52a1),
                (phy_agc_oracle::AGC_UPDATE_8018_OPAQUE, 0x0060_0030),
                (phy_agc_oracle::AGC_UPDATE_801C_OPAQUE, 0x0100_00a0),
                (phy_agc_oracle::AGC_UPDATE_8020_OPAQUE, 0x0000_0180),
                (phy_agc_oracle::AGC_UPDATE_8028_OPAQUE, 0xc040_3020),
                (phy_agc_oracle::AGC_UPDATE_802C_OPAQUE, 0x0100_0080),
                (phy_agc_oracle::AGC_UPDATE_8078_CONTROL, 0x8070_0042),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0xfe3f_e1fe),
                (phy_agc_oracle::AGC_UPDATE_7048_OPAQUE, 0xff7d_a4f3),
                (phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0x06ac_c7c8),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0xb220_8553),
                (modem_syscon::WIFI_BB_CFG, 0x0000_3802),
            ]
        );
    }

    #[test]
    fn enable_and_disable_keep_the_three_fresh_read_edges() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::AGC_ANTENNA_CONTROL, 0x2123_4567)
            .with(phy_agc_oracle::AGC_SHARED_CONTROL, 0x1000_0042);

        set_enabled_with(&mut io, true);
        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_ANTENNA_CONTROL, 0x0123_4567),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x1080_0042),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x1000_0042),
            ]
        );

        io.writes.clear();
        set_enabled_with(&mut io, false);
        assert_eq!(
            io.writes,
            vec![(phy_agc_oracle::AGC_ANTENNA_CONTROL, 0x2123_4567)]
        );
    }

    #[test]
    fn register_initialization_preserves_all_ten_rom_updates() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::RX_GAIN_LIMIT_CONTROL, u32::MAX)
            .with(phy_agc_oracle::AGC_GAIN_LIMIT_LOW, u32::MAX)
            .with(phy_agc_oracle::AGC_SHARED_CONTROL, 0x0012_55aa)
            .with(phy_agc_oracle::AGC_SATURATION_CONTROL, 0xa5a5_5a5a)
            .with(phy_agc_oracle::AGC_PARAMETER_CONTROL, 0xabc0_000f)
            .with(phy_agc_oracle::AGC_INIT_HIGH_CONTROL, 0x1234_5678);

        initialize_registers_with(&mut io, 0x45, 0x20);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::RX_GAIN_LIMIT_CONTROL, 0xff13_ffff),
                (phy_agc_oracle::AGC_GAIN_LIMIT_LOW, 0xffff_ff13),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x0012_45aa),
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0xa5a0_0bb8),
                (phy_agc_oracle::AGC_PARAMETER_CONTROL, 0xabc0_045f),
                (phy_agc_oracle::AGC_PARAMETER_CONTROL, 0xabc7_045f),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3212_45aa),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3292_45aa),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3212_45aa),
                (phy_agc_oracle::AGC_INIT_HIGH_CONTROL, 0xd234_5678),
            ]
        );
    }

    #[test]
    fn rx_compensation_retains_both_fresh_blob_updates() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::AGC_SHARED_CONTROL, 0x1234_5678)
            .with(phy_agc_oracle::RX_COMPENSATION_HIGH_CONTROL, 0x1234_5678);

        configure_rx_compensation_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x1234_56ed),
                (phy_agc_oracle::RX_COMPENSATION_HIGH_CONTROL, 0xed34_5678),
            ]
        );
    }

    #[test]
    fn pbus_delayed_tail_retains_setup_set_and_clear_reads() {
        let mut io = FakeRegisters::default().with(phy_agc_oracle::AGC_SHARED_CONTROL, 0x0012_55aa);

        configure_pbus_work_mode_pulse_with(&mut io);
        clear_pbus_work_mode_pulse_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3212_55aa),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3292_55aa),
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x3212_55aa),
            ]
        );
    }

    #[test]
    fn antenna_initialization_preserves_the_three_rom_updates() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::ANTENNA_CONTROL_0, 0xa5a5_5a5a)
            .with(phy_agc_oracle::AGC_ANTENNA_CONTROL, 0xa5a5_5a5a)
            .with(phy_agc_oracle::ANTENNA_CONTROL_2, 0xa5a5_5a5a);

        configure_antenna_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::ANTENNA_CONTROL_0, 0xa5a5_4800),
                (phy_agc_oracle::AGC_ANTENNA_CONTROL, 0xa5a5_a25a),
                (phy_agc_oracle::ANTENNA_CONTROL_2, 0x1ea5_1e5a),
            ]
        );
    }

    #[test]
    fn rf_rx_saturation_retains_both_complete_rom_branches() {
        let mut io =
            FakeRegisters::default().with(phy_agc_oracle::AGC_SATURATION_CONTROL, 0xa5a5_5a5a);

        configure_rf_rx_saturation_with(&mut io, true);
        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::RF_RX_SATURATION_CONFIG, 0x0000_0404),
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0xf5ad_5a5a),
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0xf5a8_0800),
            ]
        );

        io.writes.clear();
        io.values.clear();
        io = io.with(phy_agc_oracle::AGC_SATURATION_CONTROL, 0xa5a5_5a5a);
        configure_rf_rx_saturation_with(&mut io, false);
        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::RF_RX_SATURATION_CONFIG, 0x0000_0404),
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0x24a5_5a5a),
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0x24a0_0400),
            ]
        );
    }

    #[test]
    fn final_rx_gain_limits_use_the_owned_index_and_vendor_cap() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::AGC_SHARED_CONTROL, 0x1234_5678)
            .with(phy_agc_oracle::RX_GAIN_LIMIT_CONTROL, u32::MAX);

        configure_rx_gain_limits_with(&mut io, 0x4e);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_SHARED_CONTROL, 0x1234_4e78),
                (phy_agc_oracle::RX_GAIN_LIMIT_CONTROL, 0xff33_ffff),
            ]
        );
    }

    #[test]
    fn saturation_gain_is_two_ordered_full_word_writes() {
        let mut io = FakeRegisters::default();

        set_saturation_gain_with(&mut io, 0x0008_1825);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::SATURATION_GAIN_LOW, 0x0008_1825),
                (phy_agc_oracle::SATURATION_GAIN_HIGH, 0x0008_1825),
            ]
        );
    }

    #[test]
    fn post_initialization_retains_both_fresh_rx_reads() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::AGC_SATURATION_CONTROL, 0x1000_0001)
            .with(phy_agc_oracle::RX_11B_WINDOW_CONTROL, u32::MAX)
            .with(phy_agc_oracle::POST_INIT_RX_CONTROL, u32::MAX)
            .with(phy_agc_oracle::FTM_CONTROL, 0xffff_fffe);

        update_post_initialization_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::AGC_SATURATION_CONTROL, 0x1400_0001),
                (phy_agc_oracle::SATURATION_GAIN_LOW, 0x0818_212d),
                (phy_agc_oracle::SATURATION_GAIN_HIGH, 0x0818_212d),
                (phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0xffff_ffc0),
                (phy_agc_oracle::POST_INIT_RX_CONTROL, 0xffff_ff97),
                (phy_agc_oracle::POST_INIT_RX_CONTROL, 0xffff_cb97),
                (phy_agc_oracle::FTM_CONTROL, u32::MAX),
            ]
        );
    }

    #[test]
    fn rx_11b_enabled_branch_uses_only_the_five_recovered_fields() {
        let mut io = FakeRegisters::default()
            .with(phy_agc_oracle::RX_11B_MODE_CONTROL, 0x0400_0000)
            .with(phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0x8000_0000);

        configure_rx_11b_optimization_with(&mut io, true);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0x003f_0000),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0x003f_2100),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0x0000_8400),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0x0000_8403),
                (phy_agc_oracle::RX_11B_MODE_CONTROL, 0x0400_9000),
                (phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0x8000_01c8),
            ]
        );
    }

    #[test]
    fn rx_11b_disabled_branch_matches_the_complete_alternate_path() {
        let mut io = FakeRegisters::default();

        configure_rx_11b_optimization_with(&mut io, false);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0x003e_0000),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_0, 0x003e_1800),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0x0000_6000),
                (phy_agc_oracle::RX_11B_PATH_CONTROL_1, 0x0000_6004),
                (phy_agc_oracle::RX_11B_MODE_CONTROL, 0x0000_6000),
                (phy_agc_oracle::RX_11B_WINDOW_CONTROL, 0x0000_01c8),
            ]
        );
    }
}
