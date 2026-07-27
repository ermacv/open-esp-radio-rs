//! Owned ESP32-S31 PHY/baseband configuration leaves.
//!
//! The operations in this module deliberately preserve every fresh-read
//! update from the complete rev0 ROM and pinned `libphy.a` bodies. Field names
//! retain `UNKNOWN` where the instruction stream proves a mask and value but
//! does not establish an electrical meaning.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{
    power::{phy_baseband_config_oracle as bb, phy_pbus as pbus},
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

    fn replace(&mut self, register: Register32, field: Field32, value: u32) {
        self.modify(register, field.mask(), field_value(field, value));
    }

    fn set(&mut self, register: Register32, field: Field32) {
        self.modify(register, field.mask(), field.mask());
    }

    fn clear(&mut self, register: Register32, field: Field32) {
        self.modify(register, field.mask(), 0);
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

/// Enable the two IQ-correction modes selected by PHY register initialization.
///
/// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c`, size `0x24`,
/// sets the PAC RX- and TX-IQ correction-mode fields with two independent
/// fresh-read updates.
#[cfg(target_arch = "riscv32")]
pub fn enable_iq_correction(registers: &mut RadioRegisters) {
    enable_iq_correction_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn enable_iq_correction_with(io: &mut impl RegisterIo) {
    let rx_low = bb::iq_correction_control::RX_IQ_CORRECTION_MODE_LOW;
    let rx_high = bb::iq_correction_control::RX_IQ_CORRECTION_MODE_HIGH;
    io.modify(
        bb::IQ_CORRECTION_CONTROL,
        rx_low.mask() | rx_high.mask(),
        rx_low.mask() | rx_high.mask(),
    );

    let tx_low = bb::iq_correction_aux::TX_IQ_CORRECTION_MODE_LOW;
    let tx_high = bb::iq_correction_aux::TX_IQ_CORRECTION_MODE_HIGH;
    io.modify(
        bb::IQ_CORRECTION_AUX,
        tx_low.mask() | tx_high.mask(),
        tx_low.mask() | tx_high.mask(),
    );
}

/// Preserve the two fresh status publications at RXIQ root entry.
///
/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rxiq_cal_init`, size
/// `0x198`, sets the shared status/clock word's bit 14 and bit 15 through two
/// independent reads. Their electrical status meaning remains unknown.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_status(registers: &mut RadioRegisters) {
    configure_rxiq_root_status_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rxiq_root_status_with(io: &mut impl RegisterIo) {
    io.set(
        pbus::STATUS_CLOCK_FORCE,
        pbus::status_clock_force::RX_CLOCK_LOW_OR_RXIQ_STATUS_FIRST_UNKNOWN,
    );
    io.set(
        pbus::STATUS_CLOCK_FORCE,
        pbus::status_clock_force::RX_CLOCK_HIGH_OR_RXIQ_STATUS_SECOND_UNKNOWN,
    );
}

/// Apply one complete RXIQ root correction-mode prefix or suffix.
///
/// The pinned parent performs four separate fresh-read writes in each branch.
/// The prefix sets each low mode bit before clearing each high mode bit. The
/// suffix sets both high bits, clears the RX low bit, then clears the shared
/// root-status bit. Keeping these as distinct edges preserves the observable
/// intermediate hardware states that the former combined raw transform lost.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_correction(registers: &mut RadioRegisters, begin: bool) {
    configure_rxiq_root_correction_with(registers, begin);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_rxiq_root_correction_with(io: &mut impl RegisterIo, begin: bool) {
    use bb::{iq_correction_aux as aux, iq_correction_control as control};

    if begin {
        io.set(
            bb::IQ_CORRECTION_CONTROL,
            control::RX_IQ_CORRECTION_MODE_LOW,
        );
        io.set(bb::IQ_CORRECTION_AUX, aux::TX_IQ_CORRECTION_MODE_LOW);
        io.clear(
            bb::IQ_CORRECTION_CONTROL,
            control::RX_IQ_CORRECTION_MODE_HIGH,
        );
        io.clear(bb::IQ_CORRECTION_AUX, aux::TX_IQ_CORRECTION_MODE_HIGH);
    } else {
        io.set(
            bb::IQ_CORRECTION_CONTROL,
            control::RX_IQ_CORRECTION_MODE_HIGH,
        );
        io.set(bb::IQ_CORRECTION_AUX, aux::TX_IQ_CORRECTION_MODE_HIGH);
        io.clear(
            bb::IQ_CORRECTION_CONTROL,
            control::RX_IQ_CORRECTION_MODE_LOW,
        );
        io.clear(
            pbus::STATUS_CLOCK_FORCE,
            pbus::status_clock_force::RX_CLOCK_HIGH_OR_RXIQ_STATUS_SECOND_UNKNOWN,
        );
    }
}

/// Configure the baseband TX-power tracking register leaf.
///
/// Complete pinned `libphy.a[phy_reg.o]::phy_bb_txpwr_track`, size `0xf4`,
/// performs fourteen ordered fresh-read updates through the four PAC
/// `TX_POWER_TRACK_CONTROL` identities. Unknown value fields are reproduced
/// exactly.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_power_tracking(registers: &mut RadioRegisters, enabled: bool) {
    configure_tx_power_tracking_with(registers, enabled);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_tx_power_tracking_with(io: &mut impl RegisterIo, enabled: bool) {
    use bb::tx_power_track_control_0 as c0;
    use bb::tx_power_track_control_1 as c1;
    use bb::tx_power_track_control_2 as c2;
    use bb::tx_power_track_control_3 as c3;

    io.replace(
        bb::TX_POWER_TRACK_CONTROL_0,
        c0::TRACK_ENABLE,
        u32::from(enabled),
    );
    io.clear(bb::TX_POWER_TRACK_CONTROL_0, c0::INIT_CLEAR_UNKNOWN);
    io.set(bb::TX_POWER_TRACK_CONTROL_0, c0::INIT_SET_UNKNOWN);
    // The complete body clears these adjacent bits through separate reads.
    io.modify(bb::TX_POWER_TRACK_CONTROL_1, 1, 0);
    io.modify(bb::TX_POWER_TRACK_CONTROL_1, 2, 0);
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_3,
        c3::TRACK_VALUE_1_UNKNOWN,
        0x79,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_3,
        c3::TRACK_VALUE_0_UNKNOWN,
        0x83,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_2,
        c2::TRACK_VALUE_3_UNKNOWN,
        0x8d,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_2,
        c2::TRACK_VALUE_2_UNKNOWN,
        0x96,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_2,
        c2::TRACK_VALUE_1_UNKNOWN,
        0xa0,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_2,
        c2::TRACK_VALUE_0_UNKNOWN,
        0xb1,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_1,
        c1::TRACK_VALUE_2_UNKNOWN,
        0xbe,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_1,
        c1::TRACK_VALUE_1_UNKNOWN,
        0xd2,
    );
    io.replace(
        bb::TX_POWER_TRACK_CONTROL_1,
        c1::TRACK_VALUE_0_UNKNOWN,
        0xe6,
    );
}

/// Configure the PHY I2C TX-rate fields and TX-gain compensation bytes.
///
/// Complete rev0 ROM `phy_i2c_txrate_init` at `0x2f82_86d0`, size `0x38`,
/// replaces two PAC TX-rate fields, then dispatches through `g_phyFuns+0x30`.
/// The pinned target is complete
/// `libphy.a[phy_reg.o]::phy_txgain_comp_pacfg_new(1)`, size `0x54`, which
/// performs the four ordered PAC TX-gain compensation byte updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_tx_rate(registers: &mut RadioRegisters) {
    configure_i2c_tx_rate_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_i2c_tx_rate_with(io: &mut impl RegisterIo) {
    use bb::i2c_tx_rate_control as rate;
    use bb::tx_gain_compensation as gain;

    io.replace(bb::I2C_TX_RATE_CONTROL, rate::TX_RATE_HIGH_UNKNOWN, 0x55);
    io.replace(bb::I2C_TX_RATE_CONTROL, rate::TX_RATE_LOW_UNKNOWN, 2);
    io.replace(
        bb::TX_GAIN_COMPENSATION,
        gain::COMPENSATION_BYTE_0_UNKNOWN,
        0,
    );
    io.replace(
        bb::TX_GAIN_COMPENSATION,
        gain::COMPENSATION_BYTE_1_UNKNOWN,
        0xfa,
    );
    io.replace(
        bb::TX_GAIN_COMPENSATION,
        gain::COMPENSATION_BYTE_2_UNKNOWN,
        0xff,
    );
    io.replace(
        bb::TX_GAIN_COMPENSATION,
        gain::COMPENSATION_BYTE_3_UNKNOWN,
        0,
    );
}

/// Configure the baseband watchdog register leaf.
///
/// Complete rev0 ROM `phy_bb_wdg_cfg` at `0x2f82_7860`, size `0x2c`,
/// publishes low value `0x00aa` and the evidenced control bit through
/// `BASEBAND_WATCHDOG_CONTROL`, then sets the PAC watchdog-enable bit.
#[cfg(target_arch = "riscv32")]
pub fn configure_watchdog(registers: &mut RadioRegisters) {
    configure_watchdog_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_watchdog_with(io: &mut impl RegisterIo) {
    use bb::baseband_watchdog_control as control;

    let mask = control::WATCHDOG_CONFIG_UNKNOWN.mask() | control::WATCHDOG_CONTROL_UNKNOWN.mask();
    let value = field_value(control::WATCHDOG_CONFIG_UNKNOWN, 0x00aa)
        | control::WATCHDOG_CONTROL_UNKNOWN.mask();
    io.modify(bb::BASEBAND_WATCHDOG_CONTROL, mask, value);
    io.set(
        bb::BASEBAND_WATCHDOG_ENABLE,
        bb::baseband_watchdog_enable::WATCHDOG_ENABLE,
    );
}

/// Enable the recovered automatic noise-floor controls.
///
/// Complete rev0 ROM `phy_noise_floor_auto_set` at `0x2f82_7d3c`, size
/// `0x36`, performs four ordered fresh-read sets through
/// `NOISE_FLOOR_CONTROL` and the two PAC noise-floor enable identities.
#[cfg(target_arch = "riscv32")]
pub fn configure_noise_floor_auto(registers: &mut RadioRegisters) {
    configure_noise_floor_auto_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_noise_floor_auto_with(io: &mut impl RegisterIo) {
    io.set(
        bb::NOISE_FLOOR_CONTROL,
        bb::noise_floor_control::AUTO_CONTROL_LOW_UNKNOWN,
    );
    io.set(
        bb::NOISE_FLOOR_CONTROL,
        bb::noise_floor_control::AUTO_CONTROL_HIGH_UNKNOWN,
    );
    io.set(
        bb::NOISE_FLOOR_ENABLE_0,
        bb::noise_floor_enable_0::AUTO_ENABLE_UNKNOWN,
    );
    io.set(
        bb::NOISE_FLOOR_ENABLE_1,
        bb::noise_floor_enable_1::AUTO_ENABLE_UNKNOWN,
    );
}

/// Apply the complete baseband register initialization leaf.
///
/// Complete rev0 ROM `phy_bb_reg_init` at `0x2f82_79c6`, size `0x140`,
/// supplies all local writes. Calls to already owned
/// `phy_freq_nrx_init_baseband` and `phy_btbb_wifi_bb_cfg2` leaves remain at
/// their original positions in the sequence.
#[cfg(target_arch = "riscv32")]
pub fn initialize_baseband(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
) {
    use crate::phy_frequency;

    initialize_baseband_prefix_with(registers);
    phy_frequency::initialize_nrx_baseband(registers);
    initialize_baseband_middle_with(registers);
    phy_frequency::set_baseband_init_control(platform);
    initialize_baseband_tail_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_baseband_prefix_with(io: &mut impl RegisterIo) {
    io.set(bb::BASEBAND_INIT_7400, bb::baseband_init_7400::INIT_UNKNOWN);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_baseband_middle_with(io: &mut impl RegisterIo) {
    io.replace(
        bb::BASEBAND_INIT_7808,
        bb::baseband_init_7808::INIT_VALUE_UNKNOWN,
        0x60,
    );
    io.replace(
        bb::BASEBAND_INIT_78DC,
        bb::baseband_init_78dc::INIT_VALUE_UNKNOWN,
        2,
    );
    io.clear(
        bb::BASEBAND_INIT_78E4,
        bb::baseband_init_78e4::INIT_CLEAR_UNKNOWN,
    );
    io.clear(
        bb::BASEBAND_TX_PA_TIMING,
        bb::baseband_tx_pa_timing::BASEBAND_INIT_CLEAR_UNKNOWN,
    );
    io.clear(
        bb::BASEBAND_INIT_790C,
        bb::baseband_init_790c::INIT_CLEAR_UNKNOWN,
    );
    io.set(
        bb::BASEBAND_INIT_7CA8,
        bb::baseband_init_7ca8::INIT_ENABLE_UNKNOWN,
    );
    io.clear(
        bb::BASEBAND_INIT_7980,
        bb::baseband_init_7980::INIT_CLEAR_UNKNOWN,
    );
    // Complete ROM uses two fresh reads for the adjacent mode bits.
    let mode = bb::baseband_init_7890::INIT_MODE_UNKNOWN;
    io.modify(bb::BASEBAND_INIT_7890, field_value(mode, 2), 0);
    let low_mode_bit = field_value(mode, 1);
    io.modify(bb::BASEBAND_INIT_7890, low_mode_bit, low_mode_bit);
    io.clear(
        bb::BASEBAND_INIT_7A28,
        bb::baseband_init_7a28::INIT_CLEAR_UNKNOWN,
    );
    let nibbles = bb::baseband_init_7cd0::INIT_LOW_UNKNOWN.mask()
        | bb::baseband_init_7cd0::INIT_HIGH_UNKNOWN.mask();
    io.modify(bb::BASEBAND_INIT_7CD0, nibbles, nibbles);
    io.set(
        bb::BASEBAND_TX_PA_CONTROL,
        bb::baseband_tx_pa_control::BASEBAND_INIT_ENABLE_UNKNOWN,
    );
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_baseband_tail_with(io: &mut impl RegisterIo) {
    // The complete body clears bits 7:6 and bit 8 through separate reads.
    let clear = bb::baseband_init_743c::INIT_CLEAR_UNKNOWN;
    io.modify(bb::BASEBAND_INIT_743C, field_value(clear, 3), 0);
    io.modify(bb::BASEBAND_INIT_743C, field_value(clear, 4), 0);
    io.set(
        bb::BASEBAND_INIT_7428,
        bb::baseband_init_7428::INIT_ENABLE_UNKNOWN,
    );
    io.replace(
        bb::BASEBAND_INIT_7428,
        bb::baseband_init_7428::INIT_VALUE_UNKNOWN,
        0x15,
    );
    let low = bb::baseband_init_7cd0::INIT_LOW_UNKNOWN;
    let high = bb::baseband_init_7cd0::INIT_HIGH_UNKNOWN;
    let set_bits = field_value(low, 0x0b) | field_value(high, 0x0f);
    io.modify(bb::BASEBAND_INIT_7CD0, set_bits, set_bits);
}

/// Apply the complete PA-on register configuration leaf.
///
/// Complete rev0 ROM `phy_tx_paon_set` at `0x2f82_764c`, size `0x78`,
/// performs six ordered updates through the PAC baseband TX/PA control,
/// timing, PA table, and two shared front-end control identities.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_pa_on(registers: &mut RadioRegisters) {
    configure_tx_pa_on_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_tx_pa_on_with(io: &mut impl RegisterIo) {
    io.replace(
        bb::BASEBAND_TX_PA_CONTROL,
        bb::baseband_tx_pa_control::PA_ON_FIELD_UNKNOWN,
        0x14,
    );
    io.replace(
        bb::TX_PA_CONTROL_0,
        bb::tx_pa_control_0::PA_ON_HIGH_UNKNOWN,
        0x78,
    );
    io.write(bb::TX_PA_TABLE_OPAQUE, 0x0661_a45f);
    io.replace(
        bb::BASEBAND_TX_PA_TIMING,
        bb::baseband_tx_pa_timing::PA_ON_TIMING_UNKNOWN,
        0x1e,
    );
    // ROM `lui a4, 0xa0e0` materializes 0x0a0e_0000.
    io.replace(
        bb::TX_PA_CONTROL_1,
        bb::tx_pa_control_1::PA_ON_HIGH_UNKNOWN,
        0x0a0e,
    );
    io.replace(
        bb::TX_PA_CONTROL_1,
        bb::tx_pa_control_1::PA_ON_BYTE_1_UNKNOWN,
        0xc8,
    );
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        writes: Vec<(Register32, u32)>,
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
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
    fn rxiq_root_preserves_all_ten_fresh_blob_edges() {
        let mut io = FakeRegisters::default();

        configure_rxiq_root_status_with(&mut io);
        configure_rxiq_root_correction_with(&mut io, true);
        configure_rxiq_root_correction_with(&mut io, false);

        assert_eq!(
            io.writes,
            [
                (pbus::STATUS_CLOCK_FORCE, 0x0000_4000),
                (pbus::STATUS_CLOCK_FORCE, 0x0000_c000),
                (bb::IQ_CORRECTION_CONTROL, 0x2000_0000),
                (bb::IQ_CORRECTION_AUX, 0x0000_2000),
                (bb::IQ_CORRECTION_CONTROL, 0x2000_0000),
                (bb::IQ_CORRECTION_AUX, 0x0000_2000),
                (bb::IQ_CORRECTION_CONTROL, 0x6000_0000),
                (bb::IQ_CORRECTION_AUX, 0x0000_6000),
                (bb::IQ_CORRECTION_CONTROL, 0x4000_0000),
                (pbus::STATUS_CLOCK_FORCE, 0x0000_4000),
            ]
        );
    }

    #[test]
    fn tx_power_tracking_preserves_all_fourteen_fresh_reads() {
        let mut io = FakeRegisters::default();
        configure_tx_power_tracking_with(&mut io, true);

        assert_eq!(io.writes.len(), 14);
        assert_eq!(
            io.writes.last(),
            Some(&(bb::TX_POWER_TRACK_CONTROL_1, 0x5f69_7300))
        );
        assert_eq!(
            io.values.iter().find_map(|(register, value)| {
                (*register == bb::TX_POWER_TRACK_CONTROL_2).then_some(*value)
            }),
            Some(0x8d96_a0b1)
        );
    }

    #[test]
    fn i2c_rate_keeps_dispatch_target_write_order() {
        let mut io = FakeRegisters::default();
        configure_i2c_tx_rate_with(&mut io);

        assert_eq!(io.writes.len(), 6);
        assert_eq!(io.writes[0], (bb::I2C_TX_RATE_CONTROL, 0x0154_0000));
        assert_eq!(io.writes[1], (bb::I2C_TX_RATE_CONTROL, 0x0154_0002));
        assert_eq!(io.writes[5], (bb::TX_GAIN_COMPENSATION, 0x00ff_fa00));
    }

    #[test]
    fn baseband_local_sequence_has_eighteen_fresh_updates() {
        let mut io = FakeRegisters::default();
        initialize_baseband_prefix_with(&mut io);
        initialize_baseband_middle_with(&mut io);
        initialize_baseband_tail_with(&mut io);

        assert_eq!(io.writes.len(), 18);
        assert_eq!(io.writes[0], (bb::BASEBAND_INIT_7400, 0x0000_6000));
        assert_eq!(
            io.writes.last(),
            Some(&(bb::BASEBAND_INIT_7CD0, 0x000f_000f))
        );
    }

    #[test]
    fn watchdog_noise_and_pa_leaves_match_oracle_images() {
        let mut io = FakeRegisters::default();
        enable_iq_correction_with(&mut io);
        configure_watchdog_with(&mut io);
        configure_noise_floor_auto_with(&mut io);
        configure_tx_pa_on_with(&mut io);

        assert_eq!(io.writes[2], (bb::BASEBAND_WATCHDOG_CONTROL, 0x4000_00aa));
        assert_eq!(
            io.values.iter().find_map(|(register, value)| {
                (*register == bb::TX_PA_CONTROL_1).then_some(*value)
            }),
            Some(0x0a0e_c800)
        );
    }
}
