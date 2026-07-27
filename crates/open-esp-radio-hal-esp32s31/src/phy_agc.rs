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
    let disable = phy_agc_oracle::disable_control::DISABLE_UNKNOWN;
    if !enabled {
        io.modify(
            phy_agc_oracle::DISABLE_CONTROL,
            disable.mask(),
            disable.mask(),
        );
        return;
    }

    io.modify(phy_agc_oracle::DISABLE_CONTROL, disable.mask(), 0);
    let pulse = phy_agc_oracle::enable_pulse_control::ENABLE_PULSE_UNKNOWN;
    io.modify(
        phy_agc_oracle::ENABLE_PULSE_CONTROL,
        pulse.mask(),
        pulse.mask(),
    );
    io.modify(phy_agc_oracle::ENABLE_PULSE_CONTROL, pulse.mask(), 0);
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
    let agc = phy_agc_oracle::post_init_agc_control::POST_INIT_SET_UNKNOWN;
    io.modify(
        phy_agc_oracle::POST_INIT_AGC_CONTROL,
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
        configure_rx_11b_optimization_with, set_enabled_with, set_saturation_gain_with,
        update_baseband_registers_with, update_post_initialization_with, RegisterIo,
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
            .with(phy_agc_oracle::DISABLE_CONTROL, 0x2123_4567)
            .with(phy_agc_oracle::ENABLE_PULSE_CONTROL, 0x1000_0042);

        set_enabled_with(&mut io, true);
        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::DISABLE_CONTROL, 0x0123_4567),
                (phy_agc_oracle::ENABLE_PULSE_CONTROL, 0x1080_0042),
                (phy_agc_oracle::ENABLE_PULSE_CONTROL, 0x1000_0042),
            ]
        );

        io.writes.clear();
        set_enabled_with(&mut io, false);
        assert_eq!(
            io.writes,
            vec![(phy_agc_oracle::DISABLE_CONTROL, 0x2123_4567)]
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
            .with(phy_agc_oracle::POST_INIT_AGC_CONTROL, 0x1000_0001)
            .with(phy_agc_oracle::RX_11B_WINDOW_CONTROL, u32::MAX)
            .with(phy_agc_oracle::POST_INIT_RX_CONTROL, u32::MAX)
            .with(phy_agc_oracle::FTM_CONTROL, 0xffff_fffe);

        update_post_initialization_with(&mut io);

        assert_eq!(
            io.writes,
            vec![
                (phy_agc_oracle::POST_INIT_AGC_CONTROL, 0x1400_0001),
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
