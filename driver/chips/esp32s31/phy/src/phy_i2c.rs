//! Non-blocking ESP32-S31 PHY-I2C command encoding and RC-calibration plan.
//!
//! The rev0 ROM PHY-I2C leaves busy-wait on bit 25 of the host command
//! register. This module deliberately does not reproduce those loops. It
//! separates command publication from completion observation so an outer
//! Rust async owner can arrange a wakeup and inspect the register once.
//!
//! Reference: qualified ESP32-S31 rev0 ROM image.
//! The relevant complete ROM bodies are `phy_chip_i2c_readReg_org` at
//! `0x2f82_9ffa`, `phy_chip_i2c_writeReg` at `0x2f82_a30e`, and
//! `phy_get_rc_dout` at `0x2f82_61ac`. ESP32-S31 `libphy.a[phy_i2c.o]`
//! supplies the target-specific host configuration and read-mask callbacks
//! installed around those ROM leaves. Neither oracle is linked into the
//! firmware.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the body is one `ret`.
#[inline]
pub const fn phy_get_i2c_data() {}

/// Complete pinned target hook; the ESP32-S31 archive body is one `ret`.
#[inline]
pub const fn phy_i2c_enter_critical() {}

/// Complete pinned target hook; the ESP32-S31 archive body is one `ret`.
#[inline]
pub const fn phy_i2c_exit_critical() {}

/// Initialize the six-byte master-memory descriptor exactly as the pinned
/// archive leaf.
#[inline]
pub fn phy_i2c_master_mem_cfg(configuration: &mut [u8; 6]) {
    configuration[0] = 0;
    configuration[1] = 0;
    configuration[3] = 1;
    configuration[4] = 0x2c;
    configuration[2] = 1;
    configuration[5] = 1;
}

/// Initialize the command-memory descriptor and its two-word mode value.
#[inline]
pub fn phy_i2c_master_command_mem_cfg(configuration: &mut [u8; 8], mode: &mut u32) {
    configuration[3] = 1;
    configuration[4] = 1;
    configuration[5] = 1;
    configuration[7] = 1;
    configuration[0] = 0;
    configuration[1] = 0;
    configuration[2] = 0;
    configuration[6] = 0x2c;
    *mode = 2;
}

use crate::phy_frequency::{
    PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
    PhyChannelFrequencyInitControl, PhyChannelFrequencyInitFailure, PhyChannelFrequencyInitOutcome,
    PhyChannelFrequencyInitRequest, PhyChannelFrequencyInitTransition,
};
use crate::phy_pbus::{
    PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
    PhyPbusForceTest,
};

const fn saturate_phy_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

#[cfg(test)]
const LEGACY_PHY_PARAMETER_LEN: usize = 0x1fc;
use crate::phy_xtal_duty::{
    XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
    XtalDutyCalibrationParameters, XtalDutyCalibrationTransition,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::SharedPhyAccess;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{analog_i2c, phy_i2c as hal_phy_i2c};

#[cfg(test)]
const PHY_I2C_BUSY: u32 = 1 << 25;
// ESP32-S31 ROM `phy_chip_i2c_readReg` publishes `0x0400_0000` and
// `phy_chip_i2c_writeReg` publishes `0x0500_0000`. These command bits differ
// from the older ESP PHY-I2C layout by one hexadecimal digit.
#[cfg(test)]
const PHY_I2C_READ: u32 = 1 << 26;
#[cfg(test)]
const PHY_I2C_WRITE: u32 = 1 << 24 | 1 << 26;
#[cfg(any(target_arch = "riscv32", test))]
const PHY_I2C_MASTER_COMMAND_COUNT: usize = 45;
const PHY_I2C_SDM_STABLE_VALUE: u8 = 0x5b;
const PHY_I2C_SDM_DEADLINE_CYCLES: u32 = 9_999;

const PHY_I2C_READ_MASKS: [u16; 13] = [
    0x0100, 0x0020, 0x0010, 0x0000, 0x0000, 0x0080, 0x0004, 0x0000, 0x0800, 0x0040, 0x0008, 0x0000,
    0x8000,
];
#[cfg(any(target_arch = "riscv32", test))]
const PHY_I2C_HOST_ONE_BLOCKS: u16 = 0x0647;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cAddress {
    block: u8,
    register: u8,
}

impl PhyI2cAddress {
    pub const fn new(block: u8, register: u8) -> Option<Self> {
        if block >= 0x61 && block <= 0x6d {
            Some(Self { block, register })
        } else {
            None
        }
    }

    /// Constructs an address whose block was recovered from the pinned
    /// ESP32-S31 implementation and is therefore known to be in range.
    pub(crate) const fn new_internal(block: u8, register: u8) -> Self {
        debug_assert!(block >= 0x61 && block <= 0x6d);
        Self { block, register }
    }

    pub const fn block(self) -> u8 {
        self.block
    }

    pub const fn register(self) -> u8 {
        self.register
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn host(self) -> u8 {
        let index = self.block.wrapping_sub(0x61);
        ((PHY_I2C_HOST_ONE_BLOCKS >> index) & 1) as u8
    }

    pub const fn read_mask(self) -> u16 {
        PHY_I2C_READ_MASKS[self.block.wrapping_sub(0x61) as usize]
    }
}

/// Named analog PHY-I2C registers and fields recovered far enough to become
/// candidates for a future SVD register cluster.
///
/// These are not memory-mapped CPU addresses. `block` selects the internal
/// analog PHY-I2C peripheral and `register` selects its byte register. Names
/// describe only behavior proved by the rev0 ROM/blob call graph and HIL;
/// they deliberately avoid unevidenced electrical terminology.
pub mod analog_registers {
    use super::PhyI2cAddress;

    /// One byte field within an internal analog PHY-I2C register.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Field {
        pub address: PhyI2cAddress,
        pub high_bit: u8,
        pub low_bit: u8,
    }

    impl Field {
        const fn new(address: PhyI2cAddress, high_bit: u8, low_bit: u8) -> Self {
            Self {
                address,
                high_bit,
                low_bit,
            }
        }
    }

    /// Block 0x61, register 9.
    ///
    /// `phy_xtal_duty_cal` reads this byte as the cold crystal-duty seed.
    /// The former TX oracle wrote `0x22`, matching the live vendor seed.
    pub const XTAL_DUTY_SEED: PhyI2cAddress = PhyI2cAddress::new_internal(0x61, 0x09);

    /// Block 0x61, register 10.
    ///
    /// The crystal-duty search writes each candidate here and restores the
    /// chosen candidate after power measurement.
    pub const XTAL_DUTY_CANDIDATE: PhyI2cAddress = PhyI2cAddress::new_internal(0x61, 0x0a);

    /// Block 0x62, register 1: low eight bits of the RFPLL capacitor code.
    pub const RFPLL_CAPACITOR_LOW: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 0x01);

    /// Block 0x62, register 2, bit 6: high bit of the nine-bit RFPLL
    /// capacitor code. Other bits in this byte have separate RFPLL roles.
    pub const RFPLL_CAPACITOR_HIGH: Field =
        Field::new(PhyI2cAddress::new_internal(0x62, 0x02), 6, 6);

    /// Block 0x63, register 6, bits 2:0: low SDM frequency-programming bits.
    /// Bits 7:3 are preserved by the ROM writer and participate in the
    /// frequency-command-memory image.
    pub const RFPLL_SDM_LOW: Field = Field::new(PhyI2cAddress::new_internal(0x63, 0x06), 2, 0);

    /// Block 0x6b, register 2: two TX capacitor banks.
    ///
    /// TX-cap calibration searches bits 3:0 and 7:4 independently. Channel
    /// programming later publishes the selected packed byte.
    pub const TX_CAPACITOR_BANKS: PhyI2cAddress = PhyI2cAddress::new_internal(0x6b, 0x02);
    pub const TX_CAPACITOR_LOW: Field = Field::new(TX_CAPACITOR_BANKS, 3, 0);
    pub const TX_CAPACITOR_HIGH: Field = Field::new(TX_CAPACITOR_BANKS, 7, 4);

    /// Block 0x67 registers 0x1c..0x1f are temporarily forced to value two
    /// around archive `phy_bt_tx_pwctrl_init`. Only the first byte and the low
    /// six bits of the third byte are sampled; the same saved values are
    /// restored to both members of each pair after calibration.
    pub const BT_TX_POWER_CONTROL_LOW_0: PhyI2cAddress = PhyI2cAddress::new_internal(0x67, 0x1c);
    pub const BT_TX_POWER_CONTROL_LOW_1: PhyI2cAddress = PhyI2cAddress::new_internal(0x67, 0x1d);
    pub const BT_TX_POWER_CONTROL_HIGH_0: Field =
        Field::new(PhyI2cAddress::new_internal(0x67, 0x1e), 5, 0);
    pub const BT_TX_POWER_CONTROL_HIGH_1: Field =
        Field::new(PhyI2cAddress::new_internal(0x67, 0x1f), 5, 0);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
}

#[cfg(test)]
const fn encode_read(address: PhyI2cAddress) -> u32 {
    PHY_I2C_READ | ((address.register as u32) << 8) | address.block as u32
}

#[cfg(test)]
const fn encode_write(address: PhyI2cAddress, value: u8) -> u32 {
    PHY_I2C_WRITE | ((value as u32) << 16) | ((address.register as u32) << 8) | address.block as u32
}

#[cfg(test)]
const fn command_is_busy(command: u32) -> bool {
    command & PHY_I2C_BUSY != 0
}

#[cfg(test)]
const fn read_result(command: u32) -> u8 {
    (command >> 16) as u8
}

/// Exact non-I/O arithmetic of complete rev0 ROM `phy_encode_i2c_master`.
pub const fn phy_encode_i2c_master(block: u32, register: u32, value: u32) -> u32 {
    block | register.wrapping_shl(8) | value.wrapping_shl(16)
}

/// Safe fixed-size replacement for complete rev0 ROM `phy_byte_to_word`.
pub const fn phy_byte_to_word(bytes: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*bytes)
}

#[cfg(any(target_arch = "riscv32", test))]
const fn encode_master_command(block: u8, register: u8, value: u8) -> u32 {
    phy_encode_i2c_master(block as u32, register as u32, value as u32)
}

// Complete command order recovered from
// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. Values which depend on
// the explicit PHY parameter image are replaced in `master_command`.
#[cfg(any(target_arch = "riscv32", test))]
const PHY_I2C_MASTER_TEMPLATE: [(u8, u8, u8); PHY_I2C_MASTER_COMMAND_COUNT] = [
    (0x67, 0x02, 0x07),
    (0x6b, 0x01, 0x01),
    (0x6b, 0x02, 0x73),
    (0x6b, 0x03, 0xba),
    (0x6b, 0x04, 0x88),
    (0x6b, 0x05, 0x01),
    (0x6b, 0x06, 0x11),
    (0x6b, 0x07, 0xfd),
    (0x6b, 0x08, 0xbb),
    (0x6b, 0x09, 0x02),
    (0x6b, 0x0a, 0x08),
    (0x6b, 0x0b, 0x04),
    (0x6b, 0x0c, 0xa7),
    (0x6b, 0x0d, 0x7a),
    (0x6b, 0x0e, 0xf4),
    (0x6b, 0x0f, 0x81),
    (0x62, 0x00, 0x68),
    (0x62, 0x04, 0xa8),
    (0x62, 0x0b, 0x44),
    (0x62, 0x0d, 0x0a),
    (0x62, 0x0f, 0x00),
    (0x62, 0x15, 0x08),
    (0x66, 0x02, 0x70),
    (0x67, 0x02, 0x27),
    (0x67, 0x04, 0x00),
    (0x67, 0x05, 0x00),
    (0x67, 0x06, 0x00),
    (0x67, 0x07, 0x00),
    (0x67, 0x0c, 0x00),
    (0x67, 0x0d, 0x00),
    (0x67, 0x0e, 0x00),
    (0x67, 0x0f, 0x00),
    (0x67, 0x14, 0x00),
    (0x67, 0x15, 0x00),
    (0x67, 0x16, 0x00),
    (0x67, 0x17, 0x00),
    (0x67, 0x18, 0x00),
    (0x67, 0x19, 0x00),
    (0x67, 0x1c, 0x00),
    (0x67, 0x1d, 0x00),
    (0x67, 0x1e, 0x00),
    (0x67, 0x1f, 0x00),
    (0x63, 0x06, 0x00),
    (0x6a, 0x00, 0xaf),
    (0x6a, 0x01, 0x7f),
];

#[cfg(any(target_arch = "riscv32", test))]
const PHY_I2C_MASTER_DYNAMIC_INDICES: [usize; 19] = [
    20, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
];

#[cfg(test)]
fn master_dynamic_values(parameter: &[u8; LEGACY_PHY_PARAMETER_LEN]) -> [u8; 19] {
    master_dynamic_values_from_snapshot(PhyRfInitParameterSnapshot::new(
        FilterDcapParameters::from_legacy_parameter_image(parameter),
        parameter[0x18e],
    ))
}

#[cfg(any(target_arch = "riscv32", test))]
fn master_dynamic_values_from_snapshot(parameter: PhyRfInitParameterSnapshot) -> [u8; 19] {
    let filter = parameter.filter_dcap();
    let high_filter = saturate_phy_value(filter.parameter_ed as i32 + 6, 0x3c, 2);
    let low_filter = saturate_phy_value(filter.parameter_ed as i32 - 2, 0x3c, 2);
    let auxiliary = filter.parameter_ee.wrapping_add(2);
    [
        parameter.parameter_18e(),
        filter.parameter_e9,
        filter.parameter_e9,
        filter.parameter_ea,
        filter.parameter_ea,
        filter.parameter_e9,
        filter.parameter_e9,
        filter.parameter_ea,
        filter.parameter_ea,
        high_filter,
        high_filter,
        low_filter,
        filter.parameter_ed,
        auxiliary,
        auxiliary,
        filter.parameter_f0,
        filter.parameter_f0,
        filter.parameter_f0 | 0x40,
        filter.parameter_f0,
    ]
}

#[cfg(test)]
fn master_command(index: usize, parameter: &[u8; LEGACY_PHY_PARAMETER_LEN]) -> u32 {
    let (block, register, fixed_value) = PHY_I2C_MASTER_TEMPLATE[index];
    let dynamic_values = master_dynamic_values(parameter);
    let mut cursor = 0;
    let mut value = fixed_value;
    while cursor != PHY_I2C_MASTER_DYNAMIC_INDICES.len() {
        if PHY_I2C_MASTER_DYNAMIC_INDICES[cursor] == index {
            value = dynamic_values[cursor];
            break;
        }
        cursor += 1;
    }
    encode_master_command(block, register, value)
}

#[cfg(test)]
fn master_command_from_snapshot(index: usize, parameter: PhyRfInitParameterSnapshot) -> u32 {
    let (block, register, fixed_value) = PHY_I2C_MASTER_TEMPLATE[index];
    let dynamic_values = master_dynamic_values_from_snapshot(parameter);
    let mut cursor = 0;
    let mut value = fixed_value;
    while cursor != PHY_I2C_MASTER_DYNAMIC_INDICES.len() {
        if PHY_I2C_MASTER_DYNAMIC_INDICES[cursor] == index {
            value = dynamic_values[cursor];
            break;
        }
        cursor += 1;
    }
    encode_master_command(block, register, value)
}

/// Program the complete PHY-I2C command RAM from Rust-owned cold state.
///
/// The shared-PHY capability is borrowed from the active protocol lifecycle,
/// making exclusive ownership explicit for the complete finite 45-store
/// transaction without depending on a Wi-Fi radio owner.
///
/// Basis: complete
/// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`; destinations come from
/// the SVD-generated 45-element command-RAM array.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_master_command_memory(
    registers: &mut impl SharedPhyAccess,
    parameter: PhyRfInitParameterSnapshot,
) {
    let dynamic_values = master_dynamic_values_from_snapshot(parameter);
    let mut index = 0;
    let mut dynamic_cursor = 0;
    while index != PHY_I2C_MASTER_COMMAND_COUNT {
        let (block, register, fixed_value) = PHY_I2C_MASTER_TEMPLATE[index];
        let value = if dynamic_cursor != PHY_I2C_MASTER_DYNAMIC_INDICES.len()
            && PHY_I2C_MASTER_DYNAMIC_INDICES[dynamic_cursor] == index
        {
            let value = dynamic_values[dynamic_cursor];
            dynamic_cursor += 1;
            value
        } else {
            fixed_value
        };
        hal_phy_i2c::write_command_memory(
            registers,
            index,
            encode_master_command(block, register, value),
        )
        .unwrap_or_else(|_| unreachable!("bounded command-memory index"));
        index += 1;
    }
}

/// Publish one complete-register PHY-I2C read without waiting for completion.
///
/// Unlike the ROM read leaf, this function also rejects an already-busy host
/// before publishing the command. This is a deliberate fail-fast ownership
/// check, not a claim that the ROM performed the same pre-command check.
///
/// The caller must keep borrowing the same platform I2C owner until
/// [`try_finish_read`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_read(
    platform: &mut impl hal_phy_i2c::PhyI2cMasterControl,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    let host = configure_and_select_phy_i2c_host(platform, address);
    if platform.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    platform.publish_phy_i2c_read_mask(address.read_mask());
    platform.publish_phy_i2c_command(host, address.block(), address.register(), 0, false);
    Ok(())
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is then an incomplete/timeout result; it
/// must not be converted into a self-waking retry loop. This function never
/// loops, delays, or schedules itself.
///
/// `address` must name the in-flight command started by [`try_start_read`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_finish_read(
    platform: &impl hal_phy_i2c::PhyI2cMasterControl,
    address: PhyI2cAddress,
) -> Result<u8, PhyI2cError> {
    let host = hal_host(address.host());
    if platform.phy_i2c_master_is_busy(host) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(platform.sample_phy_i2c_result(host))
    }
}

/// Publish one complete-register PHY-I2C write after observing the
/// pre-command busy state once. It never waits or loops on that state and
/// leaves post-command completion to [`try_finish_write`].
///
/// The caller must keep borrowing the same platform I2C owner until
/// [`try_finish_write`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_write(
    platform: &mut impl hal_phy_i2c::PhyI2cMasterControl,
    address: PhyI2cAddress,
    value: u8,
) -> Result<(), PhyI2cError> {
    let host = configure_and_select_phy_i2c_host(platform, address);
    if platform.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    platform.publish_phy_i2c_command(host, address.block(), address.register(), value, true);
    Ok(())
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is an incomplete/timeout result and must
/// not be converted into a self-waking retry loop.
///
/// `address` must name the in-flight command started by [`try_start_write`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_finish_write(
    platform: &impl hal_phy_i2c::PhyI2cMasterControl,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    if platform.phy_i2c_master_is_busy(hal_host(address.host())) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(())
    }
}

/// Execute the complete accredited-domain behavior of archive
/// `phy_get_i2c_hostid_new`.
///
/// The address constructor limits callers to blocks `0x61..=0x6d`. This edge
/// selects the typed command host from the reviewed `0x0647` bitmap and then
/// performs exactly one platform-owned `ANA_CONF2` read-modify-write. Both
/// read and write command paths use this same function; it is not a
/// verification-only projection.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_and_select_phy_i2c_host(
    platform: &mut impl hal_phy_i2c::PhyI2cMasterControl,
    address: PhyI2cAddress,
) -> hal_phy_i2c::PhyI2cHost {
    let host = hal_host(address.host());
    platform.configure_phy_i2c_host_map();
    host
}

#[cfg(target_arch = "riscv32")]
const fn hal_host(host: u8) -> hal_phy_i2c::PhyI2cHost {
    if host == 0 {
        hal_phy_i2c::PhyI2cHost::Host0
    } else {
        hal_phy_i2c::PhyI2cHost::Host1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiasRegAction {
    Write { address: PhyI2cAddress, value: u8 },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiasRegCompletion {
    WriteCompleted { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiasRegTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Event-driven replacement plan for
/// `libphy.a[phy_i2c.o]::phy_bias_reg_set`.
///
/// The complete 48-byte vendor body ignores its argument and performs two
/// synchronous `phy_i2c_writeReg` calls. This transition retains the exact
/// `(block, register, value)` order but requires a separate completion edge
/// for each write. It owns no timer, waker, allocation, or hidden state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BiasRegTransition {
    step: u8,
}

impl BiasRegTransition {
    pub const fn new(_requested_state: bool) -> Self {
        Self { step: 0 }
    }

    pub const fn action(self) -> BiasRegAction {
        match self.step {
            0 => BiasRegAction::Write {
                address: PhyI2cAddress {
                    block: 0x6a,
                    register: 0,
                },
                value: 0xaf,
            },
            1 => BiasRegAction::Write {
                address: PhyI2cAddress {
                    block: 0x6a,
                    register: 1,
                },
                value: 0x7f,
            },
            _ => BiasRegAction::Complete,
        }
    }

    pub fn advance(&mut self, completion: BiasRegCompletion) -> Result<(), BiasRegTransitionError> {
        let BiasRegCompletion::WriteCompleted { address } = completion;
        match self.action() {
            BiasRegAction::Write {
                address: expected, ..
            } if address == expected => {
                self.step += 1;
                Ok(())
            }
            BiasRegAction::Write { .. } => Err(BiasRegTransitionError::WrongCompletion),
            BiasRegAction::Complete => Err(BiasRegTransitionError::AlreadyComplete),
        }
    }
}

/// Execute the finite register prefix which precedes the vendor
/// `ets_delay_us(100)` call in `phy_open_i2c_xpd_new(true)`.
///
/// This leaf deliberately stops before the delay and delegates to the
/// PMU-named HAL. Basis: complete pinned `libphy.a[phy_reg.o]` sequence at
/// offsets `0x2e..0x4e`; field identities come from the official S31 PMU
/// description.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_pre_delay(platform: &mut impl analog_i2c::PhyPmuControl) {
    analog_i2c::prepare_open_i2c_pre_delay(platform);
}

/// Execute the finite common register suffix of `phy_open_i2c_xpd_new`.
///
/// The conditional PMU reset edge is preserved by the owned HAL. Basis:
/// complete pinned `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`; PMU field
/// identities come from the official S31 PMU description.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_power_and_pulse(platform: &mut impl analog_i2c::PhyPmuControl) {
    analog_i2c::complete_open_i2c_power_and_reset(platform);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdOutcome {
    Stable,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdAction {
    ConfigurePreDelay,
    DelayMicros(u32),
    ConfigurePowerAndPulse,
    CheckSdmDeadline {
        started_at_cycle: u32,
        maximum_cycles: u32,
    },
    ReadSdmSample {
        address: PhyI2cAddress,
    },
    Complete(OpenI2cXpdOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdCompletion {
    PreDelayConfigured,
    DelayElapsed,
    PowerAndPulseConfigured { started_at_cycle: u32 },
    DeadlineObserved { expired: bool },
    SdmSample(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpenI2cXpdStep {
    PreDelayConfiguration,
    Delay,
    PowerAndPulseConfiguration,
    DeadlineCheck { started_at_cycle: u32 },
    SdmSample { started_at_cycle: u32 },
    Complete(OpenI2cXpdOutcome),
}

/// Event-driven replacement plan for `phy_open_i2c_xpd_new` and ROM
/// `phy_wait_i2c_sdm_stable`.
///
/// The vendor path contains one synchronous 100-microsecond delay and then a
/// cycle-counter/I2C polling loop. Here the delay, deadline observation and
/// every I2C sample are explicit completions delivered by the outer async
/// radio owner. A mismatching SDM value returns to `CheckSdmDeadline`; it does
/// not self-wake or read again from `poll`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenI2cXpdTransition {
    step: OpenI2cXpdStep,
    samples: u16,
}

impl OpenI2cXpdTransition {
    pub const fn new(with_pre_delay: bool) -> Self {
        Self {
            step: if with_pre_delay {
                OpenI2cXpdStep::PreDelayConfiguration
            } else {
                OpenI2cXpdStep::PowerAndPulseConfiguration
            },
            samples: 0,
        }
    }

    pub const fn action(self) -> OpenI2cXpdAction {
        const SDM_SAMPLE: PhyI2cAddress = PhyI2cAddress {
            block: 0x63,
            register: 0,
        };

        match self.step {
            OpenI2cXpdStep::PreDelayConfiguration => OpenI2cXpdAction::ConfigurePreDelay,
            OpenI2cXpdStep::Delay => OpenI2cXpdAction::DelayMicros(100),
            OpenI2cXpdStep::PowerAndPulseConfiguration => OpenI2cXpdAction::ConfigurePowerAndPulse,
            OpenI2cXpdStep::DeadlineCheck { started_at_cycle } => {
                OpenI2cXpdAction::CheckSdmDeadline {
                    started_at_cycle,
                    maximum_cycles: PHY_I2C_SDM_DEADLINE_CYCLES,
                }
            }
            OpenI2cXpdStep::SdmSample { .. } => OpenI2cXpdAction::ReadSdmSample {
                address: SDM_SAMPLE,
            },
            OpenI2cXpdStep::Complete(outcome) => OpenI2cXpdAction::Complete(outcome),
        }
    }

    pub const fn samples(self) -> u16 {
        self.samples
    }

    pub fn advance(
        &mut self,
        completion: OpenI2cXpdCompletion,
    ) -> Result<(), OpenI2cXpdTransitionError> {
        self.step = match (self.step, completion) {
            (OpenI2cXpdStep::PreDelayConfiguration, OpenI2cXpdCompletion::PreDelayConfigured) => {
                OpenI2cXpdStep::Delay
            }
            (OpenI2cXpdStep::Delay, OpenI2cXpdCompletion::DelayElapsed) => {
                OpenI2cXpdStep::PowerAndPulseConfiguration
            }
            (
                OpenI2cXpdStep::PowerAndPulseConfiguration,
                OpenI2cXpdCompletion::PowerAndPulseConfigured { started_at_cycle },
            ) => OpenI2cXpdStep::DeadlineCheck { started_at_cycle },
            (
                OpenI2cXpdStep::DeadlineCheck { .. },
                OpenI2cXpdCompletion::DeadlineObserved { expired: true },
            ) => OpenI2cXpdStep::Complete(OpenI2cXpdOutcome::TimedOut),
            (
                OpenI2cXpdStep::DeadlineCheck { started_at_cycle },
                OpenI2cXpdCompletion::DeadlineObserved { expired: false },
            ) => OpenI2cXpdStep::SdmSample { started_at_cycle },
            (
                OpenI2cXpdStep::SdmSample { started_at_cycle },
                OpenI2cXpdCompletion::SdmSample(value),
            ) => {
                self.samples = self.samples.saturating_add(1);
                if value == PHY_I2C_SDM_STABLE_VALUE {
                    OpenI2cXpdStep::Complete(OpenI2cXpdOutcome::Stable)
                } else {
                    OpenI2cXpdStep::DeadlineCheck { started_at_cycle }
                }
            }
            (OpenI2cXpdStep::Complete(_), _) => {
                return Err(OpenI2cXpdTransitionError::AlreadyComplete);
            }
            _ => return Err(OpenI2cXpdTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cBbpllOutcome {
    Enabled { register_snapshot: u8 },
    Restored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cBbpllAction {
    ReadMaskedByte { address: PhyI2cAddress },
    WriteByte { address: PhyI2cAddress, value: u8 },
    ReadSnapshot { address: PhyI2cAddress },
    Complete(I2cBbpllOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cBbpllCompletion {
    I2cReadCompleted { address: PhyI2cAddress, value: u8 },
    I2cWriteCompleted { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cBbpllTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum I2cBbpllStep {
    ReadMaskedByte,
    WriteEnabledByte(u8),
    ReadSnapshot,
    WriteRestoredByte(u8),
    Complete(I2cBbpllOutcome),
}

/// Owned replacement for complete rev0 ROM `phy_i2c_bbpll_set`.
///
/// Enabling performs a masked read/modify/write of bits 3:2 in PHY-I2C
/// register `(0x66, 4)`, reads the resulting byte again, and returns that byte
/// as explicit Rust-owned state. ROM stored it through the mutable
/// `phy_param` indirection at offset `0x4a`. Restoring accepts that byte as an
/// input instead of reading global C state. Every I2C edge is an external
/// completion; the transition never polls or retries itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cBbpllTransition {
    step: I2cBbpllStep,
}

impl I2cBbpllTransition {
    const ADDRESS: PhyI2cAddress = PhyI2cAddress {
        block: 0x66,
        register: 4,
    };

    pub const fn enable() -> Self {
        Self {
            step: I2cBbpllStep::ReadMaskedByte,
        }
    }

    pub const fn restore(register_snapshot: u8) -> Self {
        Self {
            step: I2cBbpllStep::WriteRestoredByte(register_snapshot),
        }
    }

    pub const fn action(self) -> I2cBbpllAction {
        match self.step {
            I2cBbpllStep::ReadMaskedByte => I2cBbpllAction::ReadMaskedByte {
                address: Self::ADDRESS,
            },
            I2cBbpllStep::WriteEnabledByte(value) | I2cBbpllStep::WriteRestoredByte(value) => {
                I2cBbpllAction::WriteByte {
                    address: Self::ADDRESS,
                    value,
                }
            }
            I2cBbpllStep::ReadSnapshot => I2cBbpllAction::ReadSnapshot {
                address: Self::ADDRESS,
            },
            I2cBbpllStep::Complete(outcome) => I2cBbpllAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: I2cBbpllCompletion,
    ) -> Result<(), I2cBbpllTransitionError> {
        self.step = match (self.step, completion) {
            (
                I2cBbpllStep::ReadMaskedByte,
                I2cBbpllCompletion::I2cReadCompleted { address, value },
            ) if address == Self::ADDRESS => I2cBbpllStep::WriteEnabledByte(value & !0x0c),
            (
                I2cBbpllStep::WriteEnabledByte(_),
                I2cBbpllCompletion::I2cWriteCompleted { address },
            ) if address == Self::ADDRESS => I2cBbpllStep::ReadSnapshot,
            (
                I2cBbpllStep::ReadSnapshot,
                I2cBbpllCompletion::I2cReadCompleted { address, value },
            ) if address == Self::ADDRESS => I2cBbpllStep::Complete(I2cBbpllOutcome::Enabled {
                register_snapshot: value,
            }),
            (
                I2cBbpllStep::WriteRestoredByte(_),
                I2cBbpllCompletion::I2cWriteCompleted { address },
            ) if address == Self::ADDRESS => I2cBbpllStep::Complete(I2cBbpllOutcome::Restored),
            (I2cBbpllStep::Complete(_), _) => {
                return Err(I2cBbpllTransitionError::AlreadyComplete);
            }
            _ => return Err(I2cBbpllTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateAction {
    ReadI2c { address: PhyI2cAddress },
    WriteI2c { address: PhyI2cAddress, value: u8 },
    ConfigureMmio { rate: u32 },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateCompletion {
    I2cReadCompleted { address: PhyI2cAddress, value: u8 },
    I2cWriteCompleted { address: PhyI2cAddress },
    MmioConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdcRateStep {
    ReadI2c,
    WriteI2c(u8),
    ConfigureMmio,
    Complete,
}

/// Event-driven replacement for complete rev0 ROM `phy_adc_rate_set`.
///
/// ROM uses `phy_i2c_writeReg_Mask(0x66, 0, 4, 3, 2, !rate * 2)`,
/// whose nested read and write both busy-wait. Rust owns those as two
/// separately completed PHY-I2C transactions, then emits the finite two-write
/// MMIO suffix. No action polls or repeats itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdcRateTransition {
    step: AdcRateStep,
    rate: bool,
}

impl AdcRateTransition {
    const ADDRESS: PhyI2cAddress = PhyI2cAddress {
        block: 0x66,
        register: 4,
    };

    pub const fn new(rate: bool) -> Self {
        Self {
            step: AdcRateStep::ReadI2c,
            rate,
        }
    }

    pub const fn action(self) -> AdcRateAction {
        match self.step {
            AdcRateStep::ReadI2c => AdcRateAction::ReadI2c {
                address: Self::ADDRESS,
            },
            AdcRateStep::WriteI2c(value) => AdcRateAction::WriteI2c {
                address: Self::ADDRESS,
                value,
            },
            AdcRateStep::ConfigureMmio => AdcRateAction::ConfigureMmio {
                rate: self.rate as u32,
            },
            AdcRateStep::Complete => AdcRateAction::Complete,
        }
    }

    pub fn advance(&mut self, completion: AdcRateCompletion) -> Result<(), AdcRateTransitionError> {
        self.step = match (self.step, completion) {
            (AdcRateStep::ReadI2c, AdcRateCompletion::I2cReadCompleted { address, value })
                if address == Self::ADDRESS =>
            {
                let field = if self.rate { 0 } else { 0x08 };
                AdcRateStep::WriteI2c((value & !0x0c) | field)
            }
            (AdcRateStep::WriteI2c(_), AdcRateCompletion::I2cWriteCompleted { address })
                if address == Self::ADDRESS =>
            {
                AdcRateStep::ConfigureMmio
            }
            (AdcRateStep::ConfigureMmio, AdcRateCompletion::MmioConfigured) => {
                AdcRateStep::Complete
            }
            (AdcRateStep::Complete, _) => return Err(AdcRateTransitionError::AlreadyComplete),
            _ => return Err(AdcRateTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskedI2cWriteAction {
    ReadByte { address: PhyI2cAddress },
    WriteByte { address: PhyI2cAddress, value: u8 },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskedI2cWriteCompletion {
    I2cReadCompleted { address: PhyI2cAddress, value: u8 },
    I2cWriteCompleted { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskedI2cWriteTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MaskedI2cWriteStep {
    ReadByte,
    WriteByte(u8),
    Complete,
}

/// One owned replacement for ROM `phy_i2c_writeReg_Mask`.
///
/// Construction validates the bit range. The current byte crosses the async
/// read edge as a completion value, is transformed in Rust, and is then owned
/// until the separately completed write. No hidden I2C read, write, or wait
/// remains inside a nominally synchronous action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaskedI2cWriteTransition {
    address: PhyI2cAddress,
    high_bit: u8,
    low_bit: u8,
    field_value: u8,
    step: MaskedI2cWriteStep,
}

impl MaskedI2cWriteTransition {
    pub const fn new(
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        field_value: u8,
    ) -> Option<Self> {
        if high_bit < 8 && low_bit <= high_bit {
            Some(Self {
                address,
                high_bit,
                low_bit,
                field_value,
                step: MaskedI2cWriteStep::ReadByte,
            })
        } else {
            None
        }
    }

    pub const fn action(self) -> MaskedI2cWriteAction {
        match self.step {
            MaskedI2cWriteStep::ReadByte => MaskedI2cWriteAction::ReadByte {
                address: self.address,
            },
            MaskedI2cWriteStep::WriteByte(value) => MaskedI2cWriteAction::WriteByte {
                address: self.address,
                value,
            },
            MaskedI2cWriteStep::Complete => MaskedI2cWriteAction::Complete,
        }
    }

    const fn mask(self) -> u8 {
        let width = self.high_bit - self.low_bit + 1;
        ((((1u16 << width) - 1) << self.low_bit) & 0xff) as u8
    }

    pub fn advance(
        &mut self,
        completion: MaskedI2cWriteCompletion,
    ) -> Result<(), MaskedI2cWriteTransitionError> {
        self.step = match (self.step, completion) {
            (
                MaskedI2cWriteStep::ReadByte,
                MaskedI2cWriteCompletion::I2cReadCompleted { address, value },
            ) if address == self.address => {
                let mask = self.mask();
                MaskedI2cWriteStep::WriteByte(
                    (value & !mask) | ((self.field_value << self.low_bit) & mask),
                )
            }
            (
                MaskedI2cWriteStep::WriteByte(_),
                MaskedI2cWriteCompletion::I2cWriteCompleted { address },
            ) if address == self.address => MaskedI2cWriteStep::Complete,
            (MaskedI2cWriteStep::Complete, _) => {
                return Err(MaskedI2cWriteTransitionError::AlreadyComplete);
            }
            _ => return Err(MaskedI2cWriteTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskedI2cWriteBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable lowering for one explicit read or write emitted by
/// [`MaskedI2cWriteTransition`].
#[derive(Debug, Eq, PartialEq)]
pub struct MaskedI2cWriteBinding {
    outer_action: MaskedI2cWriteAction,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl MaskedI2cWriteBinding {
    pub fn new(action: MaskedI2cWriteAction) -> Result<Self, MaskedI2cWriteBindingError> {
        let request = match action {
            MaskedI2cWriteAction::ReadByte { address } => {
                crate::phy_cold::PhyColdI2cRequest::read_byte(address)
            }
            MaskedI2cWriteAction::WriteByte { address, value } => {
                crate::phy_cold::PhyColdI2cRequest::write_byte(address, value)
            }
            MaskedI2cWriteAction::Complete => {
                return Err(MaskedI2cWriteBindingError::UnsupportedAction);
            }
        };
        Ok(Self {
            outer_action: action,
            transaction: crate::phy_cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::phy_cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: hal_phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: hal_phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &P,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<MaskedI2cWriteCompletion, MaskedI2cWriteBindingError> {
        match (self.outer_action, self.transaction.action()) {
            (
                MaskedI2cWriteAction::ReadByte { address },
                crate::phy_cold::PhyColdI2cAction::Complete(
                    crate::phy_cold::PhyColdI2cOutcome::Read {
                        address: completed_address,
                        value,
                    },
                ),
            ) if completed_address == address => {
                Ok(MaskedI2cWriteCompletion::I2cReadCompleted { address, value })
            }
            (
                MaskedI2cWriteAction::WriteByte { address, .. },
                crate::phy_cold::PhyColdI2cAction::Complete(
                    crate::phy_cold::PhyColdI2cOutcome::Written {
                        address: completed_address,
                    },
                ),
            ) if completed_address == address => {
                Ok(MaskedI2cWriteCompletion::I2cWriteCompleted { address })
            }
            (_, crate::phy_cold::PhyColdI2cAction::Complete(_)) => {
                Err(MaskedI2cWriteBindingError::UnexpectedOutcome)
            }
            _ => Err(MaskedI2cWriteBindingError::IncompleteTransaction),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationSetAction {
    MaskedWrite(MaskedI2cWriteAction),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationSetCompletion {
    MaskedWrite(MaskedI2cWriteCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationSetTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RcCalibrationSetStep {
    MaskedWrite {
        index: u8,
        transition: MaskedI2cWriteTransition,
    },
    Complete,
}

/// Exact three-field async plan for ROM `phy_i2c_rc_cal_set(3, 1, 9)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RcCalibrationSetTransition {
    step: RcCalibrationSetStep,
}

impl RcCalibrationSetTransition {
    const fn write(index: u8) -> MaskedI2cWriteTransition {
        let (register, high_bit, low_bit, value) = match index {
            0 => (0x11, 5, 4, 3),
            1 => (0x0f, 7, 3, 1),
            _ => (0x13, 5, 2, 9),
        };
        MaskedI2cWriteTransition {
            address: PhyI2cAddress {
                block: 0x6b,
                register,
            },
            high_bit,
            low_bit,
            field_value: value,
            step: MaskedI2cWriteStep::ReadByte,
        }
    }

    pub const fn new() -> Self {
        Self {
            step: RcCalibrationSetStep::MaskedWrite {
                index: 0,
                transition: Self::write(0),
            },
        }
    }

    pub const fn action(self) -> RcCalibrationSetAction {
        match self.step {
            RcCalibrationSetStep::MaskedWrite { transition, .. } => {
                RcCalibrationSetAction::MaskedWrite(transition.action())
            }
            RcCalibrationSetStep::Complete => RcCalibrationSetAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: RcCalibrationSetCompletion,
    ) -> Result<(), RcCalibrationSetTransitionError> {
        self.step = match (self.step, completion) {
            (
                RcCalibrationSetStep::MaskedWrite {
                    index,
                    mut transition,
                },
                RcCalibrationSetCompletion::MaskedWrite(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| RcCalibrationSetTransitionError::WrongCompletion)?;
                if transition.action() == MaskedI2cWriteAction::Complete {
                    if index == 2 {
                        RcCalibrationSetStep::Complete
                    } else {
                        RcCalibrationSetStep::MaskedWrite {
                            index: index + 1,
                            transition: Self::write(index + 1),
                        }
                    }
                } else {
                    RcCalibrationSetStep::MaskedWrite { index, transition }
                }
            }
            (RcCalibrationSetStep::Complete, _) => {
                return Err(RcCalibrationSetTransitionError::AlreadyComplete);
            }
        };
        Ok(())
    }
}

impl Default for RcCalibrationSetTransition {
    fn default() -> Self {
        Self::new()
    }
}

/// Five explicit parameter bytes consumed by ROM `phy_filter_dcap_set`.
///
/// Offset-based names are deliberate: the electrical meaning of these
/// vendor parameter fields has not yet been established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterDcapParameters {
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl FilterDcapParameters {
    pub const fn new(
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    #[cfg(test)]
    pub fn from_legacy_parameter_image(parameter: &[u8; LEGACY_PHY_PARAMETER_LEN]) -> Self {
        Self::new(
            parameter[0xe9],
            parameter[0xea],
            parameter[0xed],
            parameter[0xee],
            parameter[0xf0],
        )
    }

    pub const fn parameter_ee(self) -> u8 {
        self.parameter_ee
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapAction {
    Write { address: PhyI2cAddress, value: u8 },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapCompletion {
    WriteCompleted { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact finite 18-write plan recovered from ROM `phy_filter_dcap_set`.
///
/// Every write is completed by the outer non-blocking PHY-I2C owner. The
/// transition stores only a five-byte snapshot and never reads `phy_param`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterDcapTransition {
    parameter: FilterDcapParameters,
    index: u8,
}

impl FilterDcapTransition {
    pub const fn new(parameter: FilterDcapParameters) -> Self {
        Self {
            parameter,
            index: 0,
        }
    }

    pub const fn parameters(self) -> FilterDcapParameters {
        self.parameter
    }

    const fn write(register: u8, value: u8) -> FilterDcapAction {
        FilterDcapAction::Write {
            address: PhyI2cAddress {
                block: 0x67,
                register,
            },
            value,
        }
    }

    pub const fn action(self) -> FilterDcapAction {
        let high_filter = saturate_phy_value(self.parameter.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_value(self.parameter.parameter_ed as i32 - 2, 0x3c, 2);
        match self.index {
            0 => Self::write(0x14, high_filter),
            1 => Self::write(0x15, high_filter),
            2 => Self::write(0x16, low_filter),
            3 => Self::write(0x17, self.parameter.parameter_ed),
            4 => Self::write(0x18, self.parameter.parameter_ee),
            5 => Self::write(0x19, self.parameter.parameter_ee),
            6 => Self::write(0x1c, self.parameter.parameter_f0),
            7 => Self::write(0x1d, self.parameter.parameter_f0),
            8 => Self::write(0x1e, self.parameter.parameter_f0 | 0x40),
            9 => Self::write(0x1f, self.parameter.parameter_f0),
            10 => Self::write(0x04, self.parameter.parameter_e9),
            11 => Self::write(0x05, self.parameter.parameter_e9),
            12 => Self::write(0x06, self.parameter.parameter_ea),
            13 => Self::write(0x07, self.parameter.parameter_ea),
            14 => Self::write(0x0c, self.parameter.parameter_e9),
            15 => Self::write(0x0d, self.parameter.parameter_e9),
            16 => Self::write(0x0e, self.parameter.parameter_ea),
            17 => Self::write(0x0f, self.parameter.parameter_ea),
            _ => FilterDcapAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: FilterDcapCompletion,
    ) -> Result<(), FilterDcapTransitionError> {
        match (self.action(), completion) {
            (
                FilterDcapAction::Write { address, .. },
                FilterDcapCompletion::WriteCompleted { address: completed },
            ) if address == completed => {
                self.index += 1;
                Ok(())
            }
            (FilterDcapAction::Complete, _) => Err(FilterDcapTransitionError::AlreadyComplete),
            _ => Err(FilterDcapTransitionError::WrongCompletion),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfInitParameterSnapshot {
    filter_dcap: FilterDcapParameters,
    parameter_18e: u8,
}

impl PhyRfInitParameterSnapshot {
    pub const fn new(filter_dcap: FilterDcapParameters, parameter_18e: u8) -> Self {
        Self {
            filter_dcap,
            parameter_18e,
        }
    }

    pub const fn filter_dcap(self) -> FilterDcapParameters {
        self.filter_dcap
    }

    pub const fn parameter_18e(self) -> u8 {
        self.parameter_18e
    }

    pub const fn with_parameter_18e(self, parameter_18e: u8) -> Self {
        Self {
            filter_dcap: self.filter_dcap,
            parameter_18e,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Action {
    Write { address: PhyI2cAddress, value: u8 },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Completion {
    WriteCompleted { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1TransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact finite 26-write plan recovered from
/// `libphy.a[phy_i2c.o]::phy_i2c_init1`.
///
/// The vendor body reads `phy_param[0x18e]` and `phy_param[0xee]`. This
/// transition receives both through an owned snapshot and binds every write
/// completion to the address that was published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cInit1Transition {
    parameter: PhyRfInitParameterSnapshot,
    index: u8,
}

impl I2cInit1Transition {
    pub const fn new(parameter: PhyRfInitParameterSnapshot) -> Self {
        Self {
            parameter,
            index: 0,
        }
    }

    const fn write(block: u8, register: u8, value: u8) -> I2cInit1Action {
        I2cInit1Action::Write {
            address: PhyI2cAddress { block, register },
            value,
        }
    }

    pub const fn action(self) -> I2cInit1Action {
        let parameter_ee_plus_two = self.parameter.filter_dcap().parameter_ee().wrapping_add(2);
        match self.index {
            0 => Self::write(0x6b, 0x01, 0x01),
            1 => Self::write(0x6b, 0x02, 0x73),
            2 => Self::write(0x6b, 0x03, 0xba),
            3 => Self::write(0x6b, 0x04, 0x88),
            4 => Self::write(0x6b, 0x0e, 0xf4),
            5 => Self::write(0x6b, 0x09, 0x02),
            6 => Self::write(0x6b, 0x07, 0xfd),
            7 => Self::write(0x6b, 0x08, 0xbb),
            8 => Self::write(0x6b, 0x05, 0x01),
            9 => Self::write(0x6b, 0x06, 0x11),
            10 => Self::write(0x6b, 0x0c, 0xa7),
            11 => Self::write(0x6b, 0x0d, 0x7a),
            12 => Self::write(0x6b, 0x0a, 0x08),
            13 => Self::write(0x6b, 0x0b, 0x04),
            14 => Self::write(0x6b, 0x0f, 0x81),
            15 => Self::write(0x62, 0x00, 0x68),
            16 => Self::write(0x62, 0x04, 0xa8),
            17 => Self::write(0x62, 0x0f, self.parameter.parameter_18e()),
            18 => Self::write(0x62, 0x0b, 0x44),
            19 => Self::write(0x62, 0x15, 0x08),
            20 => Self::write(0x63, 0x06, 0x00),
            21 => Self::write(0x62, 0x0d, 0x0a),
            22 => Self::write(0x67, 0x02, 0x27),
            23 => Self::write(0x66, 0x02, 0x70),
            24 => Self::write(0x67, 0x18, parameter_ee_plus_two),
            25 => Self::write(0x67, 0x19, parameter_ee_plus_two),
            _ => I2cInit1Action::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: I2cInit1Completion,
    ) -> Result<(), I2cInit1TransitionError> {
        match (self.action(), completion) {
            (
                I2cInit1Action::Write { address, .. },
                I2cInit1Completion::WriteCompleted { address: completed },
            ) if address == completed => {
                self.index += 1;
                Ok(())
            }
            (I2cInit1Action::Complete, _) => Err(I2cInit1TransitionError::AlreadyComplete),
            _ => Err(I2cInit1TransitionError::WrongCompletion),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllChargePumpOutcome {
    pub parameter_18e: u8,
    pub lock_observed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpAction {
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    DelayMicros(u32),
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    Complete(RfpllChargePumpOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpCompletion {
    Write,
    Delay,
    ReadMasked(u8),
    ReadByte { address: PhyI2cAddress, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfpllChargePumpStep {
    InitialWrite(u8),
    Delay { attempt: u8 },
    LockRead { attempt: u8 },
    CapRead { lock_observed: bool },
    EnableAdjustedValue { value: u8, lock_observed: bool },
    WriteAdjustedValue { value: u8, lock_observed: bool },
    FinalRead { lock_observed: bool },
    Complete(RfpllChargePumpOutcome),
}

/// Event-driven replacement for complete ROM `phy_rfpll_chgp_cal`.
///
/// The ROM body performs as many as 100 synchronous 20-microsecond
/// delay/read iterations and prints on the final miss. Rust exposes every
/// delay and I2C observation as an external completion. The non-blocking
/// result retains `lock_observed` instead of invoking `ets_printf`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllChargePumpTransition {
    step: RfpllChargePumpStep,
}

impl RfpllChargePumpTransition {
    const REGISTER_F: PhyI2cAddress = PhyI2cAddress {
        block: 0x62,
        register: 0x0f,
    };
    const REGISTER_E: PhyI2cAddress = PhyI2cAddress {
        block: 0x62,
        register: 0x0e,
    };

    pub const fn new() -> Self {
        Self {
            step: RfpllChargePumpStep::InitialWrite(0),
        }
    }

    const fn initial_write(index: u8) -> RfpllChargePumpAction {
        let (high_bit, value) = match index {
            0 => (6, 0),
            1 => (5, 0),
            _ => (5, 1),
        };
        RfpllChargePumpAction::WriteMasked {
            address: Self::REGISTER_F,
            high_bit,
            low_bit: high_bit,
            value,
        }
    }

    pub const fn action(self) -> RfpllChargePumpAction {
        match self.step {
            RfpllChargePumpStep::InitialWrite(index) => Self::initial_write(index),
            RfpllChargePumpStep::Delay { .. } => RfpllChargePumpAction::DelayMicros(20),
            RfpllChargePumpStep::LockRead { .. } => RfpllChargePumpAction::ReadMasked {
                address: Self::REGISTER_E,
                high_bit: 7,
                low_bit: 7,
            },
            RfpllChargePumpStep::CapRead { .. } => RfpllChargePumpAction::ReadMasked {
                address: Self::REGISTER_E,
                high_bit: 4,
                low_bit: 0,
            },
            RfpllChargePumpStep::EnableAdjustedValue { .. } => RfpllChargePumpAction::WriteMasked {
                address: Self::REGISTER_F,
                high_bit: 6,
                low_bit: 6,
                value: 1,
            },
            RfpllChargePumpStep::WriteAdjustedValue { value, .. } => {
                RfpllChargePumpAction::WriteMasked {
                    address: Self::REGISTER_F,
                    high_bit: 4,
                    low_bit: 0,
                    value,
                }
            }
            RfpllChargePumpStep::FinalRead { .. } => RfpllChargePumpAction::ReadByte {
                address: Self::REGISTER_F,
            },
            RfpllChargePumpStep::Complete(outcome) => RfpllChargePumpAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: RfpllChargePumpCompletion,
    ) -> Result<(), RfpllChargePumpTransitionError> {
        self.step = match (self.step, completion) {
            (RfpllChargePumpStep::InitialWrite(index), RfpllChargePumpCompletion::Write) => {
                if index == 2 {
                    RfpllChargePumpStep::Delay { attempt: 0 }
                } else {
                    RfpllChargePumpStep::InitialWrite(index + 1)
                }
            }
            (RfpllChargePumpStep::Delay { attempt }, RfpllChargePumpCompletion::Delay) => {
                RfpllChargePumpStep::LockRead { attempt }
            }
            (
                RfpllChargePumpStep::LockRead { attempt },
                RfpllChargePumpCompletion::ReadMasked(value),
            ) => {
                if value != 0 {
                    RfpllChargePumpStep::CapRead {
                        lock_observed: true,
                    }
                } else if attempt == 99 {
                    RfpllChargePumpStep::CapRead {
                        lock_observed: false,
                    }
                } else {
                    RfpllChargePumpStep::Delay {
                        attempt: attempt + 1,
                    }
                }
            }
            (
                RfpllChargePumpStep::CapRead { lock_observed },
                RfpllChargePumpCompletion::ReadMasked(value),
            ) => {
                let adjusted = ((u16::from(value) * 7) / 6 + 9).min(0x1f) as u8;
                RfpllChargePumpStep::EnableAdjustedValue {
                    value: adjusted,
                    lock_observed,
                }
            }
            (
                RfpllChargePumpStep::EnableAdjustedValue {
                    value,
                    lock_observed,
                },
                RfpllChargePumpCompletion::Write,
            ) => RfpllChargePumpStep::WriteAdjustedValue {
                value,
                lock_observed,
            },
            (
                RfpllChargePumpStep::WriteAdjustedValue { lock_observed, .. },
                RfpllChargePumpCompletion::Write,
            ) => RfpllChargePumpStep::FinalRead { lock_observed },
            (
                RfpllChargePumpStep::FinalRead { lock_observed },
                RfpllChargePumpCompletion::ReadByte { address, value },
            ) if address == Self::REGISTER_F => {
                RfpllChargePumpStep::Complete(RfpllChargePumpOutcome {
                    parameter_18e: value,
                    lock_observed,
                })
            }
            (RfpllChargePumpStep::Complete(_), _) => {
                return Err(RfpllChargePumpTransitionError::AlreadyComplete);
            }
            _ => return Err(RfpllChargePumpTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for RfpllChargePumpTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sar2InitAction {
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sar2InitCompletion {
    MaskedWrite,
    ByteWrite { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sar2InitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact two-write expansion of ROM `phy_i2c_sar2_init_code(0x578)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sar2InitTransition {
    step: u8,
}

impl Sar2InitTransition {
    const CONTROL_ADDRESS: PhyI2cAddress = PhyI2cAddress {
        block: 0x69,
        register: 4,
    };
    const VALUE_ADDRESS: PhyI2cAddress = PhyI2cAddress {
        block: 0x69,
        register: 3,
    };

    pub const fn new() -> Self {
        Self { step: 0 }
    }

    pub const fn action(self) -> Sar2InitAction {
        match self.step {
            0 => Sar2InitAction::WriteMasked {
                address: Self::CONTROL_ADDRESS,
                high_bit: 3,
                low_bit: 0,
                value: 5,
            },
            1 => Sar2InitAction::WriteByte {
                address: Self::VALUE_ADDRESS,
                value: 0x78,
            },
            _ => Sar2InitAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: Sar2InitCompletion,
    ) -> Result<(), Sar2InitTransitionError> {
        match (self.action(), completion) {
            (Sar2InitAction::WriteMasked { .. }, Sar2InitCompletion::MaskedWrite) => {
                self.step = 1;
                Ok(())
            }
            (
                Sar2InitAction::WriteByte { address, .. },
                Sar2InitCompletion::ByteWrite { address: completed },
            ) if address == completed => {
                self.step = 2;
                Ok(())
            }
            (Sar2InitAction::Complete, _) => Err(Sar2InitTransitionError::AlreadyComplete),
            _ => Err(Sar2InitTransitionError::WrongCompletion),
        }
    }
}

impl Default for Sar2InitTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixOutcome {
    ChannelFrequencyInitialized {
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
        channel_frequency: PhyChannelFrequencyInitOutcome,
    },
    ChannelFrequencyInitializationFailed(PhyChannelFrequencyInitFailure),
    SdmTimedOut,
    PbusForceTestTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixAction {
    ConfigureFeBbClock,
    ConfigureBbpllCalibration {
        enabled: bool,
    },
    Bias(BiasRegAction),
    OpenI2cXpd(OpenI2cXpdAction),
    PbusClear(PhyPbusClearAction),
    ConfigureI2cClockSelection {
        selection: u32,
    },
    I2cBbpll(I2cBbpllAction),
    AdcRate(AdcRateAction),
    ConfigureI2cMasterRegisters,
    ConfigurePowerDetectorRegisters,
    ConfigureFrontEndRegisters,
    ConfigureTemperatureSensorRead,
    ConfigureTxPowerControlBackground,
    RcCalibrationSet(RcCalibrationSetAction),
    InspectRcCalibrationState,
    RcCalibration(RcCalibrationAction),
    CaptureFilterDcapParameters,
    FilterDcap(FilterDcapAction),
    ReadParameter18e {
        address: PhyI2cAddress,
    },
    I2cInit1(I2cInit1Action),
    RfpllChargePump(RfpllChargePumpAction),
    ConfigureI2cMasterCommandMemory {
        parameter: PhyRfInitParameterSnapshot,
    },
    ReadMasked69 {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    Sar2Init(Sar2InitAction),
    CaptureXtalDutyParameters,
    XtalDuty(XtalDutyCalibrationAction),
    ConfigureFrontEndRegisterUpdate,
    CaptureChannelFrequencyControl,
    ChannelFrequency(PhyChannelFrequencyInitAction),
    DelayMicros(u32),
    Complete(PhyRfInitPrefixOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixCompletion {
    FeBbClockConfigured,
    BbpllCalibrationConfigured,
    Bias(BiasRegCompletion),
    OpenI2cXpd(OpenI2cXpdCompletion),
    PbusClear(PhyPbusClearCompletion),
    I2cClockSelectionConfigured,
    I2cBbpll(I2cBbpllCompletion),
    AdcRate(AdcRateCompletion),
    I2cMasterRegistersConfigured,
    PowerDetectorRegistersConfigured,
    FrontEndRegistersConfigured,
    TemperatureSensorReadConfigured,
    TxPowerControlBackgroundConfigured,
    RcCalibrationSet(RcCalibrationSetCompletion),
    RcCalibrationStateInspected { already_complete: bool },
    RcCalibration(RcCalibrationCompletion),
    FilterDcapParametersCaptured(FilterDcapParameters),
    FilterDcap(FilterDcapCompletion),
    Parameter18eRead { address: PhyI2cAddress, value: u8 },
    I2cInit1(I2cInit1Completion),
    RfpllChargePump(RfpllChargePumpCompletion),
    I2cMasterCommandMemoryConfigured,
    Masked69Read(u8),
    Sar2Init(Sar2InitCompletion),
    XtalDutyParametersCaptured(XtalDutyCalibrationParameters),
    XtalDuty(XtalDutyCalibrationCompletion),
    FrontEndRegisterUpdateConfigured,
    ChannelFrequencyControlCaptured(PhyChannelFrequencyInitControl),
    ChannelFrequency(PhyChannelFrequencyInitCompletion),
    DelayElapsed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRfInitPrefixStep {
    FeBbClock,
    BbpllCalibration,
    Bias(BiasRegTransition),
    OpenI2cXpd(OpenI2cXpdTransition),
    PostI2cDelay,
    PbusClear(PhyPbusClearTransition),
    I2cClockSelection,
    I2cBbpll(I2cBbpllTransition),
    AdcRate {
        transition: AdcRateTransition,
        bbpll_register_snapshot: u8,
    },
    I2cMasterRegisters {
        bbpll_register_snapshot: u8,
    },
    PowerDetectorRegisters {
        bbpll_register_snapshot: u8,
    },
    FrontEndRegisters {
        bbpll_register_snapshot: u8,
    },
    TemperatureSensorRead {
        bbpll_register_snapshot: u8,
    },
    TxPowerControlBackground {
        bbpll_register_snapshot: u8,
    },
    RcCalibrationSet {
        transition: RcCalibrationSetTransition,
        bbpll_register_snapshot: u8,
    },
    RcCalibrationState {
        bbpll_register_snapshot: u8,
    },
    RcCalibration {
        transition: RcCalibrationTransition,
        bbpll_register_snapshot: u8,
    },
    FilterDcapParameters {
        bbpll_register_snapshot: u8,
    },
    FilterDcap {
        transition: FilterDcapTransition,
        bbpll_register_snapshot: u8,
    },
    Parameter18eRead {
        bbpll_register_snapshot: u8,
        filter_dcap: FilterDcapParameters,
    },
    I2cInit1 {
        transition: I2cInit1Transition,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
    },
    RfpllChargePump {
        transition: RfpllChargePumpTransition,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
    },
    I2cMasterCommandMemory {
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    Masked69Read {
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    Sar2Init {
        transition: Sar2InitTransition,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    XtalDutyParameters {
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
    },
    XtalDuty {
        transition: XtalDutyCalibrationTransition,
        xtal_parameters: XtalDutyCalibrationParameters,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
    },
    FrontEndRegisterUpdate {
        xtal_parameters: XtalDutyCalibrationParameters,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
    },
    ChannelFrequencyControl {
        xtal_parameters: XtalDutyCalibrationParameters,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
    },
    ChannelFrequency {
        transition: PhyChannelFrequencyInitTransition,
        bbpll_register_snapshot: u8,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
    },
    Complete(PhyRfInitPrefixOutcome),
}

/// Event-driven composition of operations one through twenty-five in the complete
/// pinned `libphy.a[phy_init.o]::phy_rf_init` body.
///
/// The two MMIO leaves are finite actions. Both bias writes and every SDM
/// sample require an external PHY-I2C completion. The 100- and 10-microsecond
/// intervals are separate executor timer edges. No transition is caused by
/// polling this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfInitPrefixTransition {
    step: PhyRfInitPrefixStep,
}

impl PhyRfInitPrefixTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyRfInitPrefixStep::FeBbClock,
        }
    }

    pub const fn action(self) -> PhyRfInitPrefixAction {
        match self.step {
            PhyRfInitPrefixStep::FeBbClock => PhyRfInitPrefixAction::ConfigureFeBbClock,
            PhyRfInitPrefixStep::BbpllCalibration => {
                PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled: true }
            }
            PhyRfInitPrefixStep::Bias(transition) => match transition.action() {
                BiasRegAction::Complete => {
                    PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay)
                }
                action => PhyRfInitPrefixAction::Bias(action),
            },
            PhyRfInitPrefixStep::OpenI2cXpd(transition) => match transition.action() {
                OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable) => {
                    PhyRfInitPrefixAction::DelayMicros(10)
                }
                OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut) => {
                    PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
                }
                action => PhyRfInitPrefixAction::OpenI2cXpd(action),
            },
            PhyRfInitPrefixStep::PostI2cDelay => PhyRfInitPrefixAction::DelayMicros(10),
            PhyRfInitPrefixStep::PbusClear(transition) => match transition.action() {
                PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared) => {
                    PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection: 8 }
                }
                PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(
                    transaction,
                )) => PhyRfInitPrefixAction::Complete(
                    PhyRfInitPrefixOutcome::PbusForceTestTimedOut(transaction),
                ),
                action => PhyRfInitPrefixAction::PbusClear(action),
            },
            PhyRfInitPrefixStep::I2cClockSelection => {
                PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection: 8 }
            }
            PhyRfInitPrefixStep::I2cBbpll(transition) => {
                PhyRfInitPrefixAction::I2cBbpll(transition.action())
            }
            PhyRfInitPrefixStep::AdcRate { transition, .. } => match transition.action() {
                AdcRateAction::Complete => PhyRfInitPrefixAction::ConfigureI2cMasterRegisters,
                action => PhyRfInitPrefixAction::AdcRate(action),
            },
            PhyRfInitPrefixStep::I2cMasterRegisters { .. } => {
                PhyRfInitPrefixAction::ConfigureI2cMasterRegisters
            }
            PhyRfInitPrefixStep::PowerDetectorRegisters { .. } => {
                PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters
            }
            PhyRfInitPrefixStep::FrontEndRegisters { .. } => {
                PhyRfInitPrefixAction::ConfigureFrontEndRegisters
            }
            PhyRfInitPrefixStep::TemperatureSensorRead { .. } => {
                PhyRfInitPrefixAction::ConfigureTemperatureSensorRead
            }
            PhyRfInitPrefixStep::TxPowerControlBackground { .. } => {
                PhyRfInitPrefixAction::ConfigureTxPowerControlBackground
            }
            PhyRfInitPrefixStep::RcCalibrationSet {
                transition,
                bbpll_register_snapshot: _,
            } => match transition.action() {
                RcCalibrationSetAction::Complete => {
                    PhyRfInitPrefixAction::InspectRcCalibrationState
                }
                action => PhyRfInitPrefixAction::RcCalibrationSet(action),
            },
            PhyRfInitPrefixStep::RcCalibrationState { .. } => {
                PhyRfInitPrefixAction::InspectRcCalibrationState
            }
            PhyRfInitPrefixStep::RcCalibration {
                transition,
                bbpll_register_snapshot: _,
            } => match transition.action() {
                RcCalibrationAction::Complete => PhyRfInitPrefixAction::CaptureFilterDcapParameters,
                action => PhyRfInitPrefixAction::RcCalibration(action),
            },
            PhyRfInitPrefixStep::FilterDcapParameters { .. } => {
                PhyRfInitPrefixAction::CaptureFilterDcapParameters
            }
            PhyRfInitPrefixStep::FilterDcap {
                transition,
                bbpll_register_snapshot: _,
            } => match transition.action() {
                FilterDcapAction::Complete => PhyRfInitPrefixAction::ReadParameter18e {
                    address: PhyI2cAddress {
                        block: 0x62,
                        register: 0x0f,
                    },
                },
                action => PhyRfInitPrefixAction::FilterDcap(action),
            },
            PhyRfInitPrefixStep::Parameter18eRead { .. } => {
                PhyRfInitPrefixAction::ReadParameter18e {
                    address: PhyI2cAddress {
                        block: 0x62,
                        register: 0x0f,
                    },
                }
            }
            PhyRfInitPrefixStep::I2cInit1 {
                transition,
                bbpll_register_snapshot: _,
                parameter: _,
            } => match transition.action() {
                I2cInit1Action::Complete => PhyRfInitPrefixAction::RfpllChargePump(
                    RfpllChargePumpTransition::new().action(),
                ),
                action => PhyRfInitPrefixAction::I2cInit1(action),
            },
            PhyRfInitPrefixStep::RfpllChargePump {
                transition,
                bbpll_register_snapshot: _,
                parameter,
            } => match transition.action() {
                RfpllChargePumpAction::Complete(outcome) => {
                    PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory {
                        parameter: parameter.with_parameter_18e(outcome.parameter_18e),
                    }
                }
                action => PhyRfInitPrefixAction::RfpllChargePump(action),
            },
            PhyRfInitPrefixStep::I2cMasterCommandMemory { parameter, .. } => {
                PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { parameter }
            }
            PhyRfInitPrefixStep::Masked69Read { .. } => PhyRfInitPrefixAction::ReadMasked69 {
                address: PhyI2cAddress {
                    block: 0x69,
                    register: 4,
                },
                high_bit: 3,
                low_bit: 0,
            },
            PhyRfInitPrefixStep::Sar2Init { transition, .. } => match transition.action() {
                Sar2InitAction::Complete => PhyRfInitPrefixAction::CaptureXtalDutyParameters,
                action => PhyRfInitPrefixAction::Sar2Init(action),
            },
            PhyRfInitPrefixStep::XtalDutyParameters { .. } => {
                PhyRfInitPrefixAction::CaptureXtalDutyParameters
            }
            PhyRfInitPrefixStep::XtalDuty { transition, .. } => match transition.action() {
                XtalDutyCalibrationAction::Complete(_) => {
                    PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
                }
                action => PhyRfInitPrefixAction::XtalDuty(action),
            },
            PhyRfInitPrefixStep::FrontEndRegisterUpdate { .. } => {
                PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
            }
            PhyRfInitPrefixStep::ChannelFrequencyControl { .. } => {
                PhyRfInitPrefixAction::CaptureChannelFrequencyControl
            }
            PhyRfInitPrefixStep::ChannelFrequency { transition, .. } => {
                PhyRfInitPrefixAction::ChannelFrequency(transition.action())
            }
            PhyRfInitPrefixStep::Complete(outcome) => PhyRfInitPrefixAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRfInitPrefixCompletion,
    ) -> Result<(), PhyRfInitPrefixTransitionError> {
        self.step = match (self.step, completion) {
            (PhyRfInitPrefixStep::FeBbClock, PhyRfInitPrefixCompletion::FeBbClockConfigured) => {
                PhyRfInitPrefixStep::BbpllCalibration
            }
            (
                PhyRfInitPrefixStep::BbpllCalibration,
                PhyRfInitPrefixCompletion::BbpllCalibrationConfigured,
            ) => PhyRfInitPrefixStep::Bias(BiasRegTransition::new(true)),
            (
                PhyRfInitPrefixStep::Bias(mut transition),
                PhyRfInitPrefixCompletion::Bias(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == BiasRegAction::Complete {
                    PhyRfInitPrefixStep::OpenI2cXpd(OpenI2cXpdTransition::new(true))
                } else {
                    PhyRfInitPrefixStep::Bias(transition)
                }
            }
            (
                PhyRfInitPrefixStep::OpenI2cXpd(mut transition),
                PhyRfInitPrefixCompletion::OpenI2cXpd(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable) => {
                        PhyRfInitPrefixStep::PostI2cDelay
                    }
                    OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut) => {
                        PhyRfInitPrefixStep::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
                    }
                    _ => PhyRfInitPrefixStep::OpenI2cXpd(transition),
                }
            }
            (PhyRfInitPrefixStep::PostI2cDelay, PhyRfInitPrefixCompletion::DelayElapsed) => {
                PhyRfInitPrefixStep::PbusClear(PhyPbusClearTransition::new())
            }
            (
                PhyRfInitPrefixStep::PbusClear(mut transition),
                PhyRfInitPrefixCompletion::PbusClear(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared) => {
                        PhyRfInitPrefixStep::I2cClockSelection
                    }
                    PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(
                        transaction,
                    )) => PhyRfInitPrefixStep::Complete(
                        PhyRfInitPrefixOutcome::PbusForceTestTimedOut(transaction),
                    ),
                    _ => PhyRfInitPrefixStep::PbusClear(transition),
                }
            }
            (
                PhyRfInitPrefixStep::I2cClockSelection,
                PhyRfInitPrefixCompletion::I2cClockSelectionConfigured,
            ) => PhyRfInitPrefixStep::I2cBbpll(I2cBbpllTransition::enable()),
            (
                PhyRfInitPrefixStep::I2cBbpll(mut transition),
                PhyRfInitPrefixCompletion::I2cBbpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    I2cBbpllAction::Complete(I2cBbpllOutcome::Enabled { register_snapshot }) => {
                        PhyRfInitPrefixStep::AdcRate {
                            transition: AdcRateTransition::new(true),
                            bbpll_register_snapshot: register_snapshot,
                        }
                    }
                    I2cBbpllAction::Complete(I2cBbpllOutcome::Restored) => {
                        return Err(PhyRfInitPrefixTransitionError::WrongCompletion);
                    }
                    _ => PhyRfInitPrefixStep::I2cBbpll(transition),
                }
            }
            (
                PhyRfInitPrefixStep::AdcRate {
                    mut transition,
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::AdcRate(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == AdcRateAction::Complete {
                    PhyRfInitPrefixStep::I2cMasterRegisters {
                        bbpll_register_snapshot,
                    }
                } else {
                    PhyRfInitPrefixStep::AdcRate {
                        transition,
                        bbpll_register_snapshot,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::I2cMasterRegisters {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured,
            ) => PhyRfInitPrefixStep::PowerDetectorRegisters {
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::PowerDetectorRegisters {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured,
            ) => PhyRfInitPrefixStep::FrontEndRegisters {
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::FrontEndRegisters {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::FrontEndRegistersConfigured,
            ) => PhyRfInitPrefixStep::TemperatureSensorRead {
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::TemperatureSensorRead {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured,
            ) => PhyRfInitPrefixStep::TxPowerControlBackground {
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::TxPowerControlBackground {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured,
            ) => PhyRfInitPrefixStep::RcCalibrationSet {
                transition: RcCalibrationSetTransition::new(),
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::RcCalibrationSet {
                    mut transition,
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::RcCalibrationSet(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == RcCalibrationSetAction::Complete {
                    PhyRfInitPrefixStep::RcCalibrationState {
                        bbpll_register_snapshot,
                    }
                } else {
                    PhyRfInitPrefixStep::RcCalibrationSet {
                        transition,
                        bbpll_register_snapshot,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::RcCalibrationState {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::RcCalibrationStateInspected { already_complete },
            ) => {
                if already_complete {
                    PhyRfInitPrefixStep::FilterDcapParameters {
                        bbpll_register_snapshot,
                    }
                } else {
                    PhyRfInitPrefixStep::RcCalibration {
                        transition: RcCalibrationTransition::new(),
                        bbpll_register_snapshot,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::RcCalibration {
                    mut transition,
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::RcCalibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == RcCalibrationAction::Complete {
                    PhyRfInitPrefixStep::FilterDcapParameters {
                        bbpll_register_snapshot,
                    }
                } else {
                    PhyRfInitPrefixStep::RcCalibration {
                        transition,
                        bbpll_register_snapshot,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::FilterDcapParameters {
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(parameter),
            ) => PhyRfInitPrefixStep::FilterDcap {
                transition: FilterDcapTransition::new(parameter),
                bbpll_register_snapshot,
            },
            (
                PhyRfInitPrefixStep::FilterDcap {
                    mut transition,
                    bbpll_register_snapshot,
                },
                PhyRfInitPrefixCompletion::FilterDcap(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == FilterDcapAction::Complete {
                    PhyRfInitPrefixStep::Parameter18eRead {
                        bbpll_register_snapshot,
                        filter_dcap: transition.parameters(),
                    }
                } else {
                    PhyRfInitPrefixStep::FilterDcap {
                        transition,
                        bbpll_register_snapshot,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::Parameter18eRead {
                    bbpll_register_snapshot,
                    filter_dcap,
                },
                PhyRfInitPrefixCompletion::Parameter18eRead {
                    address:
                        PhyI2cAddress {
                            block: 0x62,
                            register: 0x0f,
                        },
                    value,
                },
            ) => {
                let parameter = PhyRfInitParameterSnapshot::new(filter_dcap, value);
                PhyRfInitPrefixStep::I2cInit1 {
                    transition: I2cInit1Transition::new(parameter),
                    bbpll_register_snapshot,
                    parameter,
                }
            }
            (
                PhyRfInitPrefixStep::I2cInit1 {
                    mut transition,
                    bbpll_register_snapshot,
                    parameter,
                },
                PhyRfInitPrefixCompletion::I2cInit1(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == I2cInit1Action::Complete {
                    PhyRfInitPrefixStep::RfpllChargePump {
                        transition: RfpllChargePumpTransition::new(),
                        bbpll_register_snapshot,
                        parameter,
                    }
                } else {
                    PhyRfInitPrefixStep::I2cInit1 {
                        transition,
                        bbpll_register_snapshot,
                        parameter,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::RfpllChargePump {
                    mut transition,
                    bbpll_register_snapshot,
                    parameter,
                },
                PhyRfInitPrefixCompletion::RfpllChargePump(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllChargePumpAction::Complete(outcome) => {
                        PhyRfInitPrefixStep::I2cMasterCommandMemory {
                            bbpll_register_snapshot,
                            parameter: parameter.with_parameter_18e(outcome.parameter_18e),
                            rfpll_lock_observed: outcome.lock_observed,
                        }
                    }
                    _ => PhyRfInitPrefixStep::RfpllChargePump {
                        transition,
                        bbpll_register_snapshot,
                        parameter,
                    },
                }
            }
            (
                PhyRfInitPrefixStep::I2cMasterCommandMemory {
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured,
            ) => PhyRfInitPrefixStep::Masked69Read {
                bbpll_register_snapshot,
                parameter,
                rfpll_lock_observed,
            },
            (
                PhyRfInitPrefixStep::Masked69Read {
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::Masked69Read(value),
            ) => {
                if value == 0 {
                    PhyRfInitPrefixStep::Sar2Init {
                        transition: Sar2InitTransition::new(),
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                    }
                } else {
                    PhyRfInitPrefixStep::XtalDutyParameters {
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized: false,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::Sar2Init {
                    mut transition,
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::Sar2Init(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == Sar2InitAction::Complete {
                    PhyRfInitPrefixStep::XtalDutyParameters {
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized: true,
                    }
                } else {
                    PhyRfInitPrefixStep::Sar2Init {
                        transition,
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::XtalDutyParameters {
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                },
                PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(xtal_parameters),
            ) => PhyRfInitPrefixStep::XtalDuty {
                transition: XtalDutyCalibrationTransition::new(xtal_parameters),
                xtal_parameters,
                bbpll_register_snapshot,
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
            },
            (
                PhyRfInitPrefixStep::XtalDuty {
                    mut transition,
                    xtal_parameters,
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                },
                PhyRfInitPrefixCompletion::XtalDuty(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyCalibrationAction::Complete(xtal_duty) => {
                        PhyRfInitPrefixStep::FrontEndRegisterUpdate {
                            xtal_parameters,
                            bbpll_register_snapshot,
                            parameter,
                            rfpll_lock_observed,
                            sar2_reinitialized,
                            xtal_duty,
                        }
                    }
                    _ => PhyRfInitPrefixStep::XtalDuty {
                        transition,
                        xtal_parameters,
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized,
                    },
                }
            }
            (
                PhyRfInitPrefixStep::FrontEndRegisterUpdate {
                    xtal_parameters,
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                    xtal_duty,
                },
                PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured,
            ) => PhyRfInitPrefixStep::ChannelFrequencyControl {
                xtal_parameters,
                bbpll_register_snapshot,
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
                xtal_duty,
            },
            (
                PhyRfInitPrefixStep::ChannelFrequencyControl {
                    xtal_parameters,
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                    xtal_duty,
                },
                PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(control),
            ) => PhyRfInitPrefixStep::ChannelFrequency {
                transition: PhyChannelFrequencyInitTransition::new(
                    PhyChannelFrequencyInitRequest {
                        frequency_register_parameter_override: control
                            .frequency_register_parameter_override,
                        frequency_table_initialized: control.frequency_table_initialized,
                        crystal_selector: xtal_parameters.rf_frequency_offset_base,
                        middle_xtal_duty: xtal_duty.low_frequency.best_candidate,
                        outer_xtal_duty: xtal_duty.high_frequency.best_candidate,
                        front_end_parameter_bit: control.front_end_parameter_bit,
                    },
                ),
                bbpll_register_snapshot,
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
                xtal_duty,
            },
            (
                PhyRfInitPrefixStep::ChannelFrequency {
                    mut transition,
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                    xtal_duty,
                },
                PhyRfInitPrefixCompletion::ChannelFrequency(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyChannelFrequencyInitAction::Complete(channel_frequency) => {
                        PhyRfInitPrefixStep::Complete(
                            PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
                                bbpll_register_snapshot,
                                parameter,
                                rfpll_lock_observed,
                                sar2_reinitialized,
                                xtal_duty,
                                channel_frequency,
                            },
                        )
                    }
                    PhyChannelFrequencyInitAction::Failed(failure) => {
                        PhyRfInitPrefixStep::Complete(
                            PhyRfInitPrefixOutcome::ChannelFrequencyInitializationFailed(failure),
                        )
                    }
                    _ => PhyRfInitPrefixStep::ChannelFrequency {
                        transition,
                        bbpll_register_snapshot,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized,
                        xtal_duty,
                    },
                }
            }
            (PhyRfInitPrefixStep::Complete(_), _) => {
                return Err(PhyRfInitPrefixTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRfInitPrefixTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyRfInitPrefixTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationAction {
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    DelayMicros(u32),
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ApplyResult(u8),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationCompletion {
    Write,
    Delay,
    Read(u8),
    Applied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact finite action plan recovered from ROM `phy_get_rc_dout`.
///
/// The owner executes each I2C action through a non-blocking transaction and
/// implements `DelayMicros(100)` with its Rust async timer. No action advances
/// merely because the future was polled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RcCalibrationTransition {
    step: u8,
    result: u8,
}

impl RcCalibrationTransition {
    pub const fn new() -> Self {
        Self { step: 0, result: 0 }
    }

    pub const fn action(self) -> RcCalibrationAction {
        const BLOCK_61_REG_8: PhyI2cAddress = PhyI2cAddress {
            block: 0x61,
            register: 8,
        };
        const BLOCK_6B_REG_13: PhyI2cAddress = PhyI2cAddress {
            block: 0x6b,
            register: 0x13,
        };
        const BLOCK_6B_REG_14: PhyI2cAddress = PhyI2cAddress {
            block: 0x6b,
            register: 0x14,
        };

        match self.step {
            0 => RcCalibrationAction::WriteMasked {
                address: BLOCK_61_REG_8,
                high_bit: 2,
                low_bit: 2,
                value: 1,
            },
            1 => RcCalibrationAction::WriteMasked {
                address: BLOCK_6B_REG_13,
                high_bit: 0,
                low_bit: 0,
                // ROM `phy_get_rc_dout` asserts the RC-calibration enable
                // before pulsing bit 1. Leaving this clear makes the result
                // register stay at zero and poisons every derived RX filter
                // code in PHY-I2C block 0x67.
                value: 1,
            },
            2 => RcCalibrationAction::WriteMasked {
                address: BLOCK_6B_REG_13,
                high_bit: 1,
                low_bit: 1,
                value: 0,
            },
            3 => RcCalibrationAction::WriteMasked {
                address: BLOCK_6B_REG_13,
                high_bit: 1,
                low_bit: 1,
                value: 1,
            },
            4 => RcCalibrationAction::DelayMicros(100),
            5 => RcCalibrationAction::ReadMasked {
                address: BLOCK_6B_REG_14,
                high_bit: 5,
                low_bit: 0,
            },
            6 => RcCalibrationAction::WriteMasked {
                address: BLOCK_61_REG_8,
                high_bit: 2,
                low_bit: 2,
                value: 0,
            },
            7 => RcCalibrationAction::WriteMasked {
                address: BLOCK_6B_REG_13,
                high_bit: 0,
                low_bit: 0,
                value: 0,
            },
            8 => RcCalibrationAction::ApplyResult(self.result),
            _ => RcCalibrationAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: RcCalibrationCompletion,
    ) -> Result<(), RcCalibrationTransitionError> {
        let matches = matches!(
            (self.action(), completion),
            (
                RcCalibrationAction::WriteMasked { .. },
                RcCalibrationCompletion::Write
            ) | (
                RcCalibrationAction::DelayMicros(_),
                RcCalibrationCompletion::Delay
            ) | (
                RcCalibrationAction::ReadMasked { .. },
                RcCalibrationCompletion::Read(_)
            ) | (
                RcCalibrationAction::ApplyResult(_),
                RcCalibrationCompletion::Applied
            )
        );
        if !matches {
            return if self.action() == RcCalibrationAction::Complete {
                Err(RcCalibrationTransitionError::AlreadyComplete)
            } else {
                Err(RcCalibrationTransitionError::WrongCompletion)
            };
        }
        if let RcCalibrationCompletion::Read(value) = completion {
            self.result = value;
        }
        self.step += 1;
        Ok(())
    }
}

impl Default for RcCalibrationTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdcRateAction, AdcRateCompletion, AdcRateTransition, AdcRateTransitionError, BiasRegAction,
        BiasRegCompletion, BiasRegTransition, BiasRegTransitionError, FilterDcapAction,
        FilterDcapCompletion, FilterDcapParameters, FilterDcapTransition,
        FilterDcapTransitionError, I2cBbpllAction, I2cBbpllCompletion, I2cBbpllOutcome,
        I2cBbpllTransition, I2cBbpllTransitionError, I2cInit1Action, I2cInit1Completion,
        I2cInit1Transition, I2cInit1TransitionError, MaskedI2cWriteAction,
        MaskedI2cWriteCompletion, MaskedI2cWriteTransition, MaskedI2cWriteTransitionError,
        OpenI2cXpdAction, OpenI2cXpdCompletion, OpenI2cXpdOutcome, OpenI2cXpdTransition,
        OpenI2cXpdTransitionError, PHY_I2C_MASTER_COMMAND_COUNT, PhyI2cAddress,
        PhyRfInitParameterSnapshot, PhyRfInitPrefixAction, PhyRfInitPrefixCompletion,
        PhyRfInitPrefixOutcome, PhyRfInitPrefixStep, PhyRfInitPrefixTransition,
        PhyRfInitPrefixTransitionError, RcCalibrationAction, RcCalibrationCompletion,
        RcCalibrationSetAction, RcCalibrationSetCompletion, RcCalibrationSetTransition,
        RcCalibrationTransition, RcCalibrationTransitionError, RfpllChargePumpAction,
        RfpllChargePumpCompletion, RfpllChargePumpOutcome, RfpllChargePumpTransition,
        Sar2InitAction, Sar2InitCompletion, Sar2InitTransition, command_is_busy, encode_read,
        encode_write, master_command, master_command_from_snapshot, read_result,
    };
    use crate::phy_cold::PhyColdExternalBinding;
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
    };
    use crate::phy_frequency::{
        PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
        PhyChannelFrequencyInitControl, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
    };

    #[test]
    fn pure_rom_word_encoders_preserve_full_input_arithmetic() {
        assert_eq!(super::phy_encode_i2c_master(0x67, 3, 0xab), 0x00ab_0367);
        assert_eq!(
            super::phy_encode_i2c_master(0xffff_ffff, 0xffff_ffff, 0xffff_ffff),
            0xffff_ffff
        );
        assert_eq!(
            super::phy_byte_to_word(&[0x67, 0x03, 0xab, 0x5a]),
            0x5aab_0367
        );
    }
    use super::LEGACY_PHY_PARAMETER_LEN;
    use crate::phy_pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest};
    use crate::phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
    use crate::phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion};
    use crate::phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
    };
    use crate::phy_xtal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
        XtalDutyCalibrationParameters, XtalDutyPassAction, XtalDutyPassCompletion,
        XtalDutyPassOutcome, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
        XtalDutyRestoreAction, XtalDutyRestoreCompletion, XtalDutySearchAction,
        XtalDutySearchCompletion,
    };

    fn complete_dc_iq(action: PhyDcIqAction) -> PhyDcIqCompletion {
        match action {
            PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => PhyDcIqCompletion::EnableSet {
                request,
                phase,
                enabled,
            },
            PhyDcIqAction::DelayMicros {
                request,
                phase,
                micros,
            } => PhyDcIqCompletion::DelayElapsed {
                request,
                phase,
                micros,
            },
            PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                }
            }
            PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: PhyDcIqAccumulatorSnapshot {
                    i: 0,
                    q: 0,
                    power: 0,
                },
            },
            action => panic!("unexpected terminal DC/IQ action: {action:?}"),
        }
    }

    fn complete_signal_power(
        action: PhySignalPowerAction,
        component: i32,
    ) -> PhySignalPowerCompletion {
        match action {
            PhySignalPowerAction::ConfigureClock {
                request,
                clock,
                enabled,
            } => PhySignalPowerCompletion::ClockConfigured {
                request,
                clock,
                enabled,
            },
            PhySignalPowerAction::SetEstimatorEnable {
                request,
                phase,
                enabled,
            } => PhySignalPowerCompletion::EstimatorEnableSet {
                request,
                phase,
                enabled,
            },
            PhySignalPowerAction::DelayMicros {
                request,
                phase,
                micros,
            } => PhySignalPowerCompletion::DelayElapsed {
                request,
                phase,
                micros,
            },
            PhySignalPowerAction::ConfigureEstimator { request, control } => {
                PhySignalPowerCompletion::EstimatorConfigured { request, control }
            }
            PhySignalPowerAction::AwaitReadinessEdge { request, .. } => {
                PhySignalPowerCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                }
            }
            PhySignalPowerAction::ReadAccumulators(request) => {
                let shift = u32::from(request.shift.wrapping_sub(2)) & 0x1f;
                PhySignalPowerCompletion::AccumulatorsRead {
                    request,
                    snapshot: PhySignalPowerAccumulatorSnapshot {
                        sum_i: component.wrapping_shl(shift),
                        difference_i: 0,
                        difference_q: 0,
                        sum_q: 0,
                    },
                }
            }
            action => panic!("unexpected terminal signal-power action: {action:?}"),
        }
    }

    fn complete_rx_dco(action: PhyRxDcoAction) -> PhyRxDcoCompletion {
        match action {
            PhyRxDcoAction::MaskRxDcoControl => {
                PhyRxDcoCompletion::RxDcoControlMasked { saved_field: 0 }
            }
            PhyRxDcoAction::ReadPbus { selector, path } => PhyRxDcoCompletion::PbusRead {
                selector,
                path,
                value: 0,
            },
            PhyRxDcoAction::ForcePbus(transaction) => {
                PhyRxDcoCompletion::PbusForceCompleted(transaction)
            }
            PhyRxDcoAction::DelayMicros { iteration, micros } => {
                PhyRxDcoCompletion::DelayElapsed { iteration, micros }
            }
            PhyRxDcoAction::DcIq(action) => PhyRxDcoCompletion::DcIq(complete_dc_iq(action)),
            PhyRxDcoAction::RestoreRxDcoControl { saved_field } => {
                PhyRxDcoCompletion::RxDcoControlRestored { saved_field }
            }
            action => panic!("unexpected terminal RX-DCO action: {action:?}"),
        }
    }

    fn complete_rfpll(
        action: RfpllFrequencyAction,
        cap_status_reads: &mut u8,
    ) -> RfpllFrequencyCompletion {
        match action {
            RfpllFrequencyAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            } => RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
            RfpllFrequencyAction::WriteByte { address, .. } => {
                RfpllFrequencyCompletion::ByteWrite { address }
            }
            RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: if high_bit == 1 { 1 } else { 0 },
            },
            RfpllFrequencyAction::ReadByte { address } => {
                let value = if address.register() == 5 {
                    100
                } else {
                    let value = if (*cap_status_reads).is_multiple_of(3) {
                        0
                    } else {
                        1 << 2
                    };
                    *cap_status_reads = (*cap_status_reads).wrapping_add(1);
                    value
                };
                RfpllFrequencyCompletion::ByteRead { address, value }
            }
            RfpllFrequencyAction::DelayMicros(micros) => {
                RfpllFrequencyCompletion::DelayElapsed(micros)
            }
            action => panic!("unexpected terminal RFPLL action: {action:?}"),
        }
    }

    fn complete_xtal_prepare(
        action: XtalDutyPrepareAction,
        rfpll_cap_status_reads: &mut u8,
    ) -> XtalDutyPrepareCompletion {
        match action {
            XtalDutyPrepareAction::Rfpll(action) => {
                XtalDutyPrepareCompletion::Rfpll(complete_rfpll(action, rfpll_cap_status_reads))
            }
            XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            } => XtalDutyPrepareCompletion::CalibrationToneConfigured {
                enabled,
                selector,
                step,
            },
            XtalDutyPrepareAction::ConfigureRxClock { enabled } => {
                XtalDutyPrepareCompletion::RxClockConfigured { enabled }
            }
            XtalDutyPrepareAction::ConfigureTxClock { enabled } => {
                XtalDutyPrepareCompletion::TxClockConfigured { enabled }
            }
            XtalDutyPrepareAction::ConfigurePbusDebugMode => {
                XtalDutyPrepareCompletion::PbusDebugModeConfigured
            }
            XtalDutyPrepareAction::ForcePbus(transaction) => {
                XtalDutyPrepareCompletion::PbusForceCompleted(transaction)
            }
            XtalDutyPrepareAction::MaskRxDcoControl => {
                XtalDutyPrepareCompletion::RxDcoControlMasked { saved_field: 1 }
            }
            XtalDutyPrepareAction::RxDco(action) => {
                XtalDutyPrepareCompletion::RxDco(complete_rx_dco(action))
            }
            XtalDutyPrepareAction::RestoreRxDcoControl { saved_field } => {
                XtalDutyPrepareCompletion::RxDcoControlRestored { saved_field }
            }
            action => panic!("unexpected terminal preparation action: {action:?}"),
        }
    }

    fn complete_xtal_restore(action: XtalDutyRestoreAction) -> XtalDutyRestoreCompletion {
        match action {
            XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            } => XtalDutyRestoreCompletion::CalibrationToneConfigured {
                enabled,
                selector,
                step,
            },
            XtalDutyRestoreAction::ConfigureRxClock { enabled } => {
                XtalDutyRestoreCompletion::RxClockConfigured { enabled }
            }
            XtalDutyRestoreAction::ConfigureTxClock { enabled } => {
                XtalDutyRestoreCompletion::TxClockConfigured { enabled }
            }
            XtalDutyRestoreAction::ForcePbus(transaction) => {
                XtalDutyRestoreCompletion::PbusForceCompleted(transaction)
            }
            XtalDutyRestoreAction::ConfigurePbusWorkMode => {
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            action => panic!("unexpected restoration action: {action:?}"),
        }
    }

    fn drive_rf_init_xtal_duty(
        transition: &mut PhyRfInitPrefixTransition,
        initial_duty: u8,
    ) -> XtalDutyCalibrationOutcome {
        let mut current_candidate = None;
        let mut rfpll_cap_status_reads = 0;
        loop {
            let outer_action = transition.action();
            if !matches!(outer_action, PhyRfInitPrefixAction::Complete(_)) {
                assert!(
                    PhyColdExternalBinding::lower(outer_action).is_ok(),
                    "reachable crystal-duty action has no external lowering: {outer_action:?}"
                );
            }
            match outer_action {
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
                    address,
                    ..
                }) => {
                    assert_eq!((address.block(), address.register()), (0x61, 9));
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::InitialDutyRead {
                                address,
                                value: initial_duty,
                            },
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(
                    XtalDutyCalibrationAction::DisableCalibrationPath { address, .. },
                ) => {
                    assert_eq!((address.block(), address.register()), (0x61, 7));
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::CalibrationPathDisabled { address },
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::WriteMasked { address, .. },
                )) => {
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(
                                XtalDutyPassCompletion::MaskedWrite { address },
                            ),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::WriteByte { address, value },
                )) => {
                    assert_eq!((address.block(), address.register()), (0x61, 0x0a));
                    assert_eq!(value, initial_duty);
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(
                                XtalDutyPassCompletion::ByteWrite { address },
                            ),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(action),
                )) => {
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                                complete_xtal_prepare(action, &mut rfpll_cap_status_reads),
                            )),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                        address,
                        candidate,
                    }),
                )) => {
                    current_candidate = Some(candidate);
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                                XtalDutySearchCompletion::CandidateWritten { address, candidate },
                            )),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros {
                        candidate,
                        micros: 20,
                    }),
                )) => {
                    assert_eq!(current_candidate, Some(candidate));
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                                XtalDutySearchCompletion::DelayElapsed { candidate },
                            )),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(action)),
                )) => {
                    let candidate = current_candidate.unwrap();
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                                XtalDutySearchCompletion::SignalPower(complete_signal_power(
                                    action,
                                    i32::from(0x80 - candidate),
                                )),
                            )),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Restore(action),
                )) => {
                    transition
                        .advance(PhyRfInitPrefixCompletion::XtalDuty(
                            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                                complete_xtal_restore(action),
                            )),
                        ))
                        .unwrap();
                }
                PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
                    if let PhyRfInitPrefixStep::FrontEndRegisterUpdate { xtal_duty, .. } =
                        transition.step
                    {
                        return xtal_duty;
                    }
                    panic!("front-end update action without its owned step");
                }
                PhyRfInitPrefixAction::Complete(
                    PhyRfInitPrefixOutcome::ChannelFrequencyInitialized { xtal_duty, .. },
                ) => return xtal_duty,
                action => panic!("unexpected RF-init crystal-duty action: {action:?}"),
            }
        }
    }

    fn drive_warm_channel_frequency(transition: &mut PhyRfInitPrefixTransition) {
        loop {
            let completion = match transition.action() {
                PhyRfInitPrefixAction::ChannelFrequency(
                    PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                        parameter_override,
                    },
                ) => PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override,
                },
                PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                    action,
                )) => {
                    let completion = match action {
                        PhyFrequencyI2cAction::WriteMasked {
                            address,
                            high_bit,
                            low_bit,
                            ..
                        } => PhyFrequencyI2cCompletion::MaskedWrite {
                            address,
                            high_bit,
                            low_bit,
                        },
                        PhyFrequencyI2cAction::ReadByte { address } => {
                            let value = if address == PhyI2cAddress::new(0x62, 0x0b).unwrap() {
                                0x5a
                            } else if address == PhyI2cAddress::new(0x63, 0).unwrap() {
                                0x8f
                            } else {
                                0x10
                            };
                            PhyFrequencyI2cCompletion::ByteRead { address, value }
                        }
                        PhyFrequencyI2cAction::WriteMemory {
                            descriptor_index,
                            copy_index,
                            address,
                            ..
                        } => PhyFrequencyI2cCompletion::MemoryWrite {
                            descriptor_index,
                            copy_index,
                            address,
                        },
                        PhyFrequencyI2cAction::ConfigureNumberAddresses(image) => {
                            PhyFrequencyI2cCompletion::NumberAddressesConfigured(image)
                        }
                        action => panic!("unexpected terminal frequency-I2C action: {action:?}"),
                    };
                    PhyChannelFrequencyInitCompletion::I2c(completion)
                }
                PhyRfInitPrefixAction::Complete(_) => return,
                action => panic!("unexpected warm channel-frequency action: {action:?}"),
            };
            transition
                .advance(PhyRfInitPrefixCompletion::ChannelFrequency(completion))
                .unwrap();
        }
    }

    #[test]
    fn recovered_block_table_selects_exact_hosts_and_read_masks() {
        let expected = [
            (0x61, 1, 0x100),
            (0x62, 1, 0x020),
            (0x63, 1, 0x010),
            (0x64, 0, 0x000),
            (0x65, 0, 0x000),
            (0x66, 0, 0x080),
            (0x67, 1, 0x004),
            (0x68, 0, 0x000),
            (0x69, 0, 0x800),
            (0x6a, 1, 0x040),
            (0x6b, 1, 0x008),
            (0x6c, 0, 0x000),
            (0x6d, 0, 0x8000),
        ];
        for (block, host, mask) in expected {
            let address = PhyI2cAddress::new(block, 0x12).unwrap();
            assert_eq!(address.host(), host);
            assert_eq!(address.read_mask(), mask);
        }
        assert!(PhyI2cAddress::new(0x60, 0).is_none());
        assert!(PhyI2cAddress::new(0x6e, 0).is_none());
    }

    #[test]
    fn command_words_match_complete_rom_leaf_encoding() {
        let address = PhyI2cAddress::new(0x6b, 0x14).unwrap();
        assert_eq!(encode_read(address), 0x0400_146b);
        assert_eq!(encode_write(address, 0xa5), 0x05a5_146b);
        assert!(!command_is_busy(0x05a5_146b));
        assert!(command_is_busy(0x07a5_146b));
        assert_eq!(read_result(0x043c_146b), 0x3c);
    }

    #[test]
    fn master_command_table_matches_complete_vendor_body() {
        let mut parameter = [0_u8; LEGACY_PHY_PARAMETER_LEN];
        parameter[0x18e] = 0x55;
        parameter[0xe9] = 0x12;
        parameter[0xea] = 0x34;
        parameter[0xed] = 0x20;
        parameter[0xee] = 0xfe;
        parameter[0xf0] = 0x9a;

        let expected = [
            (0x67, 0x02, 0x07),
            (0x6b, 0x01, 0x01),
            (0x6b, 0x02, 0x73),
            (0x6b, 0x03, 0xba),
            (0x6b, 0x04, 0x88),
            (0x6b, 0x05, 0x01),
            (0x6b, 0x06, 0x11),
            (0x6b, 0x07, 0xfd),
            (0x6b, 0x08, 0xbb),
            (0x6b, 0x09, 0x02),
            (0x6b, 0x0a, 0x08),
            (0x6b, 0x0b, 0x04),
            (0x6b, 0x0c, 0xa7),
            (0x6b, 0x0d, 0x7a),
            (0x6b, 0x0e, 0xf4),
            (0x6b, 0x0f, 0x81),
            (0x62, 0x00, 0x68),
            (0x62, 0x04, 0xa8),
            (0x62, 0x0b, 0x44),
            (0x62, 0x0d, 0x0a),
            (0x62, 0x0f, 0x55),
            (0x62, 0x15, 0x08),
            (0x66, 0x02, 0x70),
            (0x67, 0x02, 0x27),
            (0x67, 0x04, 0x12),
            (0x67, 0x05, 0x12),
            (0x67, 0x06, 0x34),
            (0x67, 0x07, 0x34),
            (0x67, 0x0c, 0x12),
            (0x67, 0x0d, 0x12),
            (0x67, 0x0e, 0x34),
            (0x67, 0x0f, 0x34),
            (0x67, 0x14, 0x26),
            (0x67, 0x15, 0x26),
            (0x67, 0x16, 0x1e),
            (0x67, 0x17, 0x20),
            (0x67, 0x18, 0x00),
            (0x67, 0x19, 0x00),
            (0x67, 0x1c, 0x9a),
            (0x67, 0x1d, 0x9a),
            (0x67, 0x1e, 0xda),
            (0x67, 0x1f, 0x9a),
            (0x63, 0x06, 0x00),
            (0x6a, 0x00, 0xaf),
            (0x6a, 0x01, 0x7f),
        ];
        assert_eq!(expected.len(), PHY_I2C_MASTER_COMMAND_COUNT);
        let snapshot = PhyRfInitParameterSnapshot::new(
            FilterDcapParameters::from_legacy_parameter_image(&parameter),
            parameter[0x18e],
        );
        for (index, (block, register, value)) in expected.into_iter().enumerate() {
            let expected_word = (block as u32) | ((register as u32) << 8) | ((value as u32) << 16);
            assert_eq!(
                master_command(index, &parameter),
                expected_word,
                "master command {index}"
            );
            assert_eq!(
                master_command_from_snapshot(index, snapshot),
                expected_word,
                "owned master command {index}"
            );
        }
    }

    #[test]
    fn rc_calibration_plan_has_only_explicit_async_edges() {
        let mut transition = RcCalibrationTransition::new();
        for _ in 0..4 {
            assert!(matches!(
                transition.action(),
                RcCalibrationAction::WriteMasked { .. }
            ));
            transition.advance(RcCalibrationCompletion::Write).unwrap();
        }
        assert_eq!(transition.action(), RcCalibrationAction::DelayMicros(100));
        assert_eq!(
            transition.advance(RcCalibrationCompletion::Write),
            Err(RcCalibrationTransitionError::WrongCompletion)
        );
        transition.advance(RcCalibrationCompletion::Delay).unwrap();
        assert!(matches!(
            transition.action(),
            RcCalibrationAction::ReadMasked {
                high_bit: 5,
                low_bit: 0,
                ..
            }
        ));
        transition
            .advance(RcCalibrationCompletion::Read(0x2d))
            .unwrap();
        transition.advance(RcCalibrationCompletion::Write).unwrap();
        transition.advance(RcCalibrationCompletion::Write).unwrap();
        assert_eq!(transition.action(), RcCalibrationAction::ApplyResult(0x2d));
        transition
            .advance(RcCalibrationCompletion::Applied)
            .unwrap();
        assert_eq!(transition.action(), RcCalibrationAction::Complete);
        assert_eq!(
            transition.advance(RcCalibrationCompletion::Applied),
            Err(RcCalibrationTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn bias_register_plan_requires_two_ordered_i2c_completions() {
        let first = PhyI2cAddress::new(0x6a, 0).unwrap();
        let second = PhyI2cAddress::new(0x6a, 1).unwrap();
        let mut transition = BiasRegTransition::new(true);

        assert_eq!(
            transition.action(),
            BiasRegAction::Write {
                address: first,
                value: 0xaf
            }
        );
        assert_eq!(
            transition.advance(BiasRegCompletion::WriteCompleted { address: second }),
            Err(BiasRegTransitionError::WrongCompletion)
        );
        transition
            .advance(BiasRegCompletion::WriteCompleted { address: first })
            .unwrap();
        assert_eq!(
            transition.action(),
            BiasRegAction::Write {
                address: second,
                value: 0x7f
            }
        );
        transition
            .advance(BiasRegCompletion::WriteCompleted { address: second })
            .unwrap();
        assert_eq!(transition.action(), BiasRegAction::Complete);
        assert_eq!(
            transition.advance(BiasRegCompletion::WriteCompleted { address: second }),
            Err(BiasRegTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn bias_register_argument_is_instruction_proven_unused() {
        assert_eq!(BiasRegTransition::new(false), BiasRegTransition::new(true));
    }

    #[test]
    fn adc_rate_owns_masked_i2c_read_write_and_mmio_edges() {
        let address = PhyI2cAddress::new(0x66, 4).unwrap();
        let mut high_rate = AdcRateTransition::new(true);
        assert_eq!(high_rate.action(), AdcRateAction::ReadI2c { address });
        assert_eq!(
            high_rate.advance(AdcRateCompletion::I2cWriteCompleted { address }),
            Err(AdcRateTransitionError::WrongCompletion)
        );
        high_rate
            .advance(AdcRateCompletion::I2cReadCompleted {
                address,
                value: 0xaf,
            })
            .unwrap();
        assert_eq!(
            high_rate.action(),
            AdcRateAction::WriteI2c {
                address,
                value: 0xa3,
            }
        );
        high_rate
            .advance(AdcRateCompletion::I2cWriteCompleted { address })
            .unwrap();
        assert_eq!(high_rate.action(), AdcRateAction::ConfigureMmio { rate: 1 });
        high_rate
            .advance(AdcRateCompletion::MmioConfigured)
            .unwrap();
        assert_eq!(high_rate.action(), AdcRateAction::Complete);

        let mut low_rate = AdcRateTransition::new(false);
        low_rate
            .advance(AdcRateCompletion::I2cReadCompleted {
                address,
                value: 0xa3,
            })
            .unwrap();
        assert_eq!(
            low_rate.action(),
            AdcRateAction::WriteI2c {
                address,
                value: 0xab,
            }
        );
    }

    #[test]
    fn masked_i2c_write_owns_read_transform_and_write_edges() {
        let address = PhyI2cAddress::new(0x6b, 0x11).unwrap();
        assert!(MaskedI2cWriteTransition::new(address, 3, 4, 0).is_none());
        assert!(MaskedI2cWriteTransition::new(address, 8, 0, 0).is_none());

        let mut transition = MaskedI2cWriteTransition::new(address, 5, 4, 3).unwrap();
        assert_eq!(
            transition.action(),
            MaskedI2cWriteAction::ReadByte { address }
        );
        assert_eq!(
            transition.advance(MaskedI2cWriteCompletion::I2cWriteCompleted { address }),
            Err(MaskedI2cWriteTransitionError::WrongCompletion)
        );
        transition
            .advance(MaskedI2cWriteCompletion::I2cReadCompleted {
                address,
                value: 0x0f,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            MaskedI2cWriteAction::WriteByte {
                address,
                value: 0x3f,
            }
        );
        transition
            .advance(MaskedI2cWriteCompletion::I2cWriteCompleted { address })
            .unwrap();
        assert_eq!(transition.action(), MaskedI2cWriteAction::Complete);
    }

    #[test]
    fn rc_calibration_set_preserves_all_three_rom_masked_writes() {
        let requests = [
            (PhyI2cAddress::new(0x6b, 0x11).unwrap(), 0x30),
            (PhyI2cAddress::new(0x6b, 0x0f).unwrap(), 0x08),
            (PhyI2cAddress::new(0x6b, 0x13).unwrap(), 0x24),
        ];
        let mut transition = RcCalibrationSetTransition::new();
        for (address, expected_value) in requests {
            assert_eq!(
                transition.action(),
                RcCalibrationSetAction::MaskedWrite(MaskedI2cWriteAction::ReadByte { address })
            );
            transition
                .advance(RcCalibrationSetCompletion::MaskedWrite(
                    MaskedI2cWriteCompletion::I2cReadCompleted { address, value: 0 },
                ))
                .unwrap();
            assert_eq!(
                transition.action(),
                RcCalibrationSetAction::MaskedWrite(MaskedI2cWriteAction::WriteByte {
                    address,
                    value: expected_value,
                })
            );
            transition
                .advance(RcCalibrationSetCompletion::MaskedWrite(
                    MaskedI2cWriteCompletion::I2cWriteCompleted { address },
                ))
                .unwrap();
        }
        assert_eq!(transition.action(), RcCalibrationSetAction::Complete);
    }

    #[test]
    fn filter_dcap_owns_exact_rom_parameter_snapshot_and_write_order() {
        let parameter = FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87);
        let mut transition = FilterDcapTransition::new(parameter);
        let expected = [
            (0x14, 0x3c),
            (0x15, 0x3c),
            (0x16, 0x38),
            (0x17, 0x3a),
            (0x18, 0x56),
            (0x19, 0x56),
            (0x1c, 0x87),
            (0x1d, 0x87),
            (0x1e, 0xc7),
            (0x1f, 0x87),
            (0x04, 0x12),
            (0x05, 0x12),
            (0x06, 0x34),
            (0x07, 0x34),
            (0x0c, 0x12),
            (0x0d, 0x12),
            (0x0e, 0x34),
            (0x0f, 0x34),
        ];

        for (register, value) in expected {
            let address = PhyI2cAddress::new(0x67, register).unwrap();
            assert_eq!(
                transition.action(),
                FilterDcapAction::Write { address, value }
            );
            let wrong_address = PhyI2cAddress::new(0x67, register.wrapping_add(1)).unwrap();
            assert_eq!(
                transition.advance(FilterDcapCompletion::WriteCompleted {
                    address: wrong_address,
                }),
                Err(FilterDcapTransitionError::WrongCompletion)
            );
            transition
                .advance(FilterDcapCompletion::WriteCompleted { address })
                .unwrap();
        }

        assert_eq!(transition.action(), FilterDcapAction::Complete);
        assert_eq!(
            transition.advance(FilterDcapCompletion::WriteCompleted {
                address: PhyI2cAddress::new(0x67, 0x0f).unwrap(),
            }),
            Err(FilterDcapTransitionError::AlreadyComplete)
        );

        let mut image = [0_u8; LEGACY_PHY_PARAMETER_LEN];
        image[0xe9] = 1;
        image[0xea] = 2;
        image[0xed] = 3;
        image[0xee] = 4;
        image[0xf0] = 5;
        assert_eq!(
            FilterDcapParameters::from_legacy_parameter_image(&image),
            FilterDcapParameters::new(1, 2, 3, 4, 5)
        );
    }

    #[test]
    fn i2c_init1_owns_both_dynamic_parameters_and_all_26_writes() {
        let filter = FilterDcapParameters::new(1, 2, 3, 0xfe, 5);
        let parameter = PhyRfInitParameterSnapshot::new(filter, 0x55);
        let mut transition = I2cInit1Transition::new(parameter);
        let expected = [
            (0x6b, 0x01, 0x01),
            (0x6b, 0x02, 0x73),
            (0x6b, 0x03, 0xba),
            (0x6b, 0x04, 0x88),
            (0x6b, 0x0e, 0xf4),
            (0x6b, 0x09, 0x02),
            (0x6b, 0x07, 0xfd),
            (0x6b, 0x08, 0xbb),
            (0x6b, 0x05, 0x01),
            (0x6b, 0x06, 0x11),
            (0x6b, 0x0c, 0xa7),
            (0x6b, 0x0d, 0x7a),
            (0x6b, 0x0a, 0x08),
            (0x6b, 0x0b, 0x04),
            (0x6b, 0x0f, 0x81),
            (0x62, 0x00, 0x68),
            (0x62, 0x04, 0xa8),
            (0x62, 0x0f, 0x55),
            (0x62, 0x0b, 0x44),
            (0x62, 0x15, 0x08),
            (0x63, 0x06, 0x00),
            (0x62, 0x0d, 0x0a),
            (0x67, 0x02, 0x27),
            (0x66, 0x02, 0x70),
            (0x67, 0x18, 0x00),
            (0x67, 0x19, 0x00),
        ];

        for (block, register, value) in expected {
            let address = PhyI2cAddress::new(block, register).unwrap();
            assert_eq!(
                transition.action(),
                I2cInit1Action::Write { address, value }
            );
            transition
                .advance(I2cInit1Completion::WriteCompleted { address })
                .unwrap();
        }
        assert_eq!(transition.action(), I2cInit1Action::Complete);
        assert_eq!(
            transition.advance(I2cInit1Completion::WriteCompleted {
                address: PhyI2cAddress::new(0x67, 0x19).unwrap(),
            }),
            Err(I2cInit1TransitionError::AlreadyComplete)
        );
        assert_eq!(parameter.parameter_18e(), 0x55);
        assert_eq!(parameter.filter_dcap(), filter);
    }

    fn complete_rfpll_initial_writes(transition: &mut RfpllChargePumpTransition) {
        for (high_bit, value) in [(6, 0), (5, 0), (5, 1)] {
            assert_eq!(
                transition.action(),
                RfpllChargePumpAction::WriteMasked {
                    address: PhyI2cAddress::new(0x62, 0x0f).unwrap(),
                    high_bit,
                    low_bit: high_bit,
                    value,
                }
            );
            transition
                .advance(RfpllChargePumpCompletion::Write)
                .unwrap();
        }
    }

    #[test]
    fn rfpll_charge_pump_lock_path_uses_async_delay_and_owned_result() {
        let mut transition = RfpllChargePumpTransition::new();
        complete_rfpll_initial_writes(&mut transition);
        assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
        transition
            .advance(RfpllChargePumpCompletion::Delay)
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::ReadMasked {
                address: PhyI2cAddress::new(0x62, 0x0e).unwrap(),
                high_bit: 7,
                low_bit: 7,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::ReadMasked(1))
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::ReadMasked {
                address: PhyI2cAddress::new(0x62, 0x0e).unwrap(),
                high_bit: 4,
                low_bit: 0,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::ReadMasked(12))
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::WriteMasked {
                address: PhyI2cAddress::new(0x62, 0x0f).unwrap(),
                high_bit: 6,
                low_bit: 6,
                value: 1,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::Write)
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::WriteMasked {
                address: PhyI2cAddress::new(0x62, 0x0f).unwrap(),
                high_bit: 4,
                low_bit: 0,
                value: 23,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::Write)
            .unwrap();
        let final_address = PhyI2cAddress::new(0x62, 0x0f).unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::ReadByte {
                address: final_address,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::ReadByte {
                address: final_address,
                value: 0xaa,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::Complete(RfpllChargePumpOutcome {
                parameter_18e: 0xaa,
                lock_observed: true,
            })
        );
    }

    #[test]
    fn rfpll_charge_pump_final_miss_is_data_not_a_blocking_print() {
        let mut transition = RfpllChargePumpTransition::new();
        complete_rfpll_initial_writes(&mut transition);
        for attempt in 0..100 {
            assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
            transition
                .advance(RfpllChargePumpCompletion::Delay)
                .unwrap();
            transition
                .advance(RfpllChargePumpCompletion::ReadMasked(0))
                .unwrap();
            if attempt != 99 {
                assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
            }
        }
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::ReadMasked {
                address: PhyI2cAddress::new(0x62, 0x0e).unwrap(),
                high_bit: 4,
                low_bit: 0,
            }
        );
        transition
            .advance(RfpllChargePumpCompletion::ReadMasked(31))
            .unwrap();
        transition
            .advance(RfpllChargePumpCompletion::Write)
            .unwrap();
        transition
            .advance(RfpllChargePumpCompletion::Write)
            .unwrap();
        let final_address = PhyI2cAddress::new(0x62, 0x0f).unwrap();
        transition
            .advance(RfpllChargePumpCompletion::ReadByte {
                address: final_address,
                value: 0xbb,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::Complete(RfpllChargePumpOutcome {
                parameter_18e: 0xbb,
                lock_observed: false,
            })
        );
    }

    #[test]
    fn sar2_zero_branch_expands_0x578_into_two_owned_writes() {
        let mut transition = Sar2InitTransition::new();
        assert_eq!(
            transition.action(),
            Sar2InitAction::WriteMasked {
                address: PhyI2cAddress::new(0x69, 4).unwrap(),
                high_bit: 3,
                low_bit: 0,
                value: 5,
            }
        );
        transition.advance(Sar2InitCompletion::MaskedWrite).unwrap();
        let value_address = PhyI2cAddress::new(0x69, 3).unwrap();
        assert_eq!(
            transition.action(),
            Sar2InitAction::WriteByte {
                address: value_address,
                value: 0x78,
            }
        );
        transition
            .advance(Sar2InitCompletion::ByteWrite {
                address: value_address,
            })
            .unwrap();
        assert_eq!(transition.action(), Sar2InitAction::Complete);
    }

    #[test]
    fn i2c_bbpll_moves_rom_phy_param_snapshot_into_owned_state() {
        let address = PhyI2cAddress::new(0x66, 4).unwrap();
        let mut enable = I2cBbpllTransition::enable();
        assert_eq!(enable.action(), I2cBbpllAction::ReadMaskedByte { address });
        assert_eq!(
            enable.advance(I2cBbpllCompletion::I2cWriteCompleted { address }),
            Err(I2cBbpllTransitionError::WrongCompletion)
        );
        enable
            .advance(I2cBbpllCompletion::I2cReadCompleted {
                address,
                value: 0xaf,
            })
            .unwrap();
        assert_eq!(
            enable.action(),
            I2cBbpllAction::WriteByte {
                address,
                value: 0xa3,
            }
        );
        enable
            .advance(I2cBbpllCompletion::I2cWriteCompleted { address })
            .unwrap();
        assert_eq!(enable.action(), I2cBbpllAction::ReadSnapshot { address });
        enable
            .advance(I2cBbpllCompletion::I2cReadCompleted {
                address,
                value: 0xa3,
            })
            .unwrap();
        assert_eq!(
            enable.action(),
            I2cBbpllAction::Complete(I2cBbpllOutcome::Enabled {
                register_snapshot: 0xa3,
            })
        );

        let mut restore = I2cBbpllTransition::restore(0xa3);
        assert_eq!(
            restore.action(),
            I2cBbpllAction::WriteByte {
                address,
                value: 0xa3,
            }
        );
        restore
            .advance(I2cBbpllCompletion::I2cWriteCompleted { address })
            .unwrap();
        assert_eq!(
            restore.action(),
            I2cBbpllAction::Complete(I2cBbpllOutcome::Restored)
        );
    }

    #[test]
    fn rf_init_prefix_composes_mmio_i2c_and_timer_edges_in_vendor_order() {
        let bias_zero = PhyI2cAddress::new(0x6a, 0).unwrap();
        let bias_one = PhyI2cAddress::new(0x6a, 1).unwrap();
        let mut transition = PhyRfInitPrefixTransition::new();

        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureFeBbClock
        );
        assert_eq!(
            transition.advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured),
            Err(PhyRfInitPrefixTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyRfInitPrefixCompletion::FeBbClockConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled: true }
        );
        transition
            .advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
            .unwrap();

        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::Bias(BiasRegAction::Write {
                address: bias_zero,
                value: 0xaf
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::Bias(
                BiasRegCompletion::WriteCompleted { address: bias_zero },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::Bias(
                BiasRegCompletion::WriteCompleted { address: bias_one },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay)
        );

        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PreDelayConfigured,
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(100))
        );
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DelayElapsed,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured {
                    started_at_cycle: 0x1234_5678,
                },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired: false },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::SdmSample(0x5b),
            ))
            .unwrap();

        assert_eq!(transition.action(), PhyRfInitPrefixAction::DelayMicros(10));
        transition
            .advance(PhyRfInitPrefixCompletion::DelayElapsed)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode)
        );
        transition
            .advance(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::DebugModeConfigured,
            ))
            .unwrap();
        for transaction in [
            PhyPbusForceTest::new(4, 1, 0),
            PhyPbusForceTest::new(4, 2, 0),
            PhyPbusForceTest::new(5, 1, 0),
            PhyPbusForceTest::new(5, 2, 0),
            PhyPbusForceTest::new(0, 1, 0),
            PhyPbusForceTest::new(0, 2, 0),
            PhyPbusForceTest::new(1, 1, 0),
            PhyPbusForceTest::new(1, 2, 0),
            PhyPbusForceTest::new(2, 1, 0x100),
            PhyPbusForceTest::new(3, 1, 0x100),
            PhyPbusForceTest::new(2, 2, 0x100),
            PhyPbusForceTest::new(3, 2, 0x100),
        ] {
            transition
                .advance(PhyRfInitPrefixCompletion::PbusClear(
                    PhyPbusClearCompletion::ForceTestCompleted(transaction),
                ))
                .unwrap();
        }
        transition
            .advance(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: false,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection: 8 }
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cClockSelectionConfigured)
            .unwrap();
        let bbpll_address = PhyI2cAddress::new(0x66, 4).unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::ReadMaskedByte {
                address: bbpll_address
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cBbpll(
                I2cBbpllCompletion::I2cReadCompleted {
                    address: bbpll_address,
                    value: 0xaf,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::WriteByte {
                address: bbpll_address,
                value: 0xa3,
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cBbpll(
                I2cBbpllCompletion::I2cWriteCompleted {
                    address: bbpll_address,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::ReadSnapshot {
                address: bbpll_address
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cBbpll(
                I2cBbpllCompletion::I2cReadCompleted {
                    address: bbpll_address,
                    value: 0xa3,
                },
            ))
            .unwrap();
        let adc_address = PhyI2cAddress::new(0x66, 4).unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ReadI2c {
                address: adc_address
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::AdcRate(
                AdcRateCompletion::I2cReadCompleted {
                    address: adc_address,
                    value: 0xff,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::WriteI2c {
                address: adc_address,
                value: 0xf3,
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::AdcRate(
                AdcRateCompletion::I2cWriteCompleted {
                    address: adc_address,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio { rate: 1 })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::AdcRate(
                AdcRateCompletion::MmioConfigured,
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureI2cMasterRegisters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureFrontEndRegisters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::FrontEndRegistersConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureTemperatureSensorRead
        );
        transition
            .advance(PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureTxPowerControlBackground
        );
        transition
            .advance(PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured)
            .unwrap();
        for (address, expected_value) in [
            (PhyI2cAddress::new(0x6b, 0x11).unwrap(), 0x30),
            (PhyI2cAddress::new(0x6b, 0x0f).unwrap(), 0x08),
            (PhyI2cAddress::new(0x6b, 0x13).unwrap(), 0x24),
        ] {
            assert_eq!(
                transition.action(),
                PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
                    MaskedI2cWriteAction::ReadByte { address }
                ))
            );
            transition
                .advance(PhyRfInitPrefixCompletion::RcCalibrationSet(
                    RcCalibrationSetCompletion::MaskedWrite(
                        MaskedI2cWriteCompletion::I2cReadCompleted { address, value: 0 },
                    ),
                ))
                .unwrap();
            assert_eq!(
                transition.action(),
                PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
                    MaskedI2cWriteAction::WriteByte {
                        address,
                        value: expected_value,
                    }
                ))
            );
            transition
                .advance(PhyRfInitPrefixCompletion::RcCalibrationSet(
                    RcCalibrationSetCompletion::MaskedWrite(
                        MaskedI2cWriteCompletion::I2cWriteCompleted { address },
                    ),
                ))
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::InspectRcCalibrationState
        );

        let mut already_calibrated = transition;
        already_calibrated
            .advance(PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
                already_complete: true,
            })
            .unwrap();
        assert_eq!(
            already_calibrated.action(),
            PhyRfInitPrefixAction::CaptureFilterDcapParameters
        );

        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
                already_complete: false,
            })
            .unwrap();
        for expected in [
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x61, 8).unwrap(),
                high_bit: 2,
                low_bit: 2,
                value: 1,
            },
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x6b, 0x13).unwrap(),
                high_bit: 0,
                low_bit: 0,
                value: 1,
            },
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x6b, 0x13).unwrap(),
                high_bit: 1,
                low_bit: 1,
                value: 0,
            },
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x6b, 0x13).unwrap(),
                high_bit: 1,
                low_bit: 1,
                value: 1,
            },
        ] {
            assert_eq!(
                transition.action(),
                PhyRfInitPrefixAction::RcCalibration(expected)
            );
            transition
                .advance(PhyRfInitPrefixCompletion::RcCalibration(
                    RcCalibrationCompletion::Write,
                ))
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(100))
        );
        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Delay,
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked {
                address: PhyI2cAddress::new(0x6b, 0x14).unwrap(),
                high_bit: 5,
                low_bit: 0,
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Read(0x2d),
            ))
            .unwrap();
        for expected in [
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x61, 8).unwrap(),
                high_bit: 2,
                low_bit: 2,
                value: 0,
            },
            RcCalibrationAction::WriteMasked {
                address: PhyI2cAddress::new(0x6b, 0x13).unwrap(),
                high_bit: 0,
                low_bit: 0,
                value: 0,
            },
        ] {
            assert_eq!(
                transition.action(),
                PhyRfInitPrefixAction::RcCalibration(expected)
            );
            transition
                .advance(PhyRfInitPrefixCompletion::RcCalibration(
                    RcCalibrationCompletion::Write,
                ))
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ApplyResult(0x2d))
        );
        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Applied,
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::CaptureFilterDcapParameters
        );

        transition
            .advance(PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(
                FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87),
            ))
            .unwrap();
        while let PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Write { address, .. }) =
            transition.action()
        {
            transition
                .advance(PhyRfInitPrefixCompletion::FilterDcap(
                    FilterDcapCompletion::WriteCompleted { address },
                ))
                .unwrap();
        }
        let parameter_18e_address = PhyI2cAddress::new(0x62, 0x0f).unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ReadParameter18e {
                address: parameter_18e_address,
            }
        );
        assert_eq!(
            transition.advance(PhyRfInitPrefixCompletion::Parameter18eRead {
                address: PhyI2cAddress::new(0x62, 0x0e).unwrap(),
                value: 0x55,
            }),
            Err(PhyRfInitPrefixTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyRfInitPrefixCompletion::Parameter18eRead {
                address: parameter_18e_address,
                value: 0x55,
            })
            .unwrap();
        while let PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Write { address, .. }) =
            transition.action()
        {
            transition
                .advance(PhyRfInitPrefixCompletion::I2cInit1(
                    I2cInit1Completion::WriteCompleted { address },
                ))
                .unwrap();
        }
        for _ in 0..3 {
            transition
                .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                    RfpllChargePumpCompletion::Write,
                ))
                .unwrap();
        }
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::Delay,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::ReadMasked(1),
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::ReadMasked(12),
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::Write,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::Write,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::ReadByte {
                    address: parameter_18e_address,
                    value: 0xaa,
                },
            ))
            .unwrap();
        let final_parameter = PhyRfInitParameterSnapshot::new(
            FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87),
            0xaa,
        );
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory {
                parameter: final_parameter,
            }
        );
        transition
            .advance(PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ReadMasked69 {
                address: PhyI2cAddress::new(0x69, 4).unwrap(),
                high_bit: 3,
                low_bit: 0,
            }
        );
        let mut already_initialized = transition;
        already_initialized
            .advance(PhyRfInitPrefixCompletion::Masked69Read(1))
            .unwrap();
        assert_eq!(
            already_initialized.action(),
            PhyRfInitPrefixAction::CaptureXtalDutyParameters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::Masked69Read(0))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::Sar2Init(Sar2InitAction::WriteMasked {
                address: PhyI2cAddress::new(0x69, 4).unwrap(),
                high_bit: 3,
                low_bit: 0,
                value: 5,
            })
        );
        transition
            .advance(PhyRfInitPrefixCompletion::Sar2Init(
                Sar2InitCompletion::MaskedWrite,
            ))
            .unwrap();
        let sar2_value_address = PhyI2cAddress::new(0x69, 3).unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::Sar2Init(
                Sar2InitCompletion::ByteWrite {
                    address: sar2_value_address,
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::CaptureXtalDutyParameters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(
                XtalDutyCalibrationParameters {
                    rf_frequency_offset_base: 0x31,
                    pbus_rx_path_value: 0x42,
                },
            ))
            .unwrap();
        let xtal_duty = drive_rf_init_xtal_duty(&mut transition, 0x2a);
        assert_eq!(
            xtal_duty,
            XtalDutyCalibrationOutcome {
                initial_duty: 0x2a,
                low_frequency: XtalDutyPassOutcome {
                    frequency_code: 0x988,
                    best_candidate: 0x3e,
                    best_filtered_power: 0x42 * 0x42,
                },
                high_frequency: XtalDutyPassOutcome {
                    frequency_code: 0x9b0,
                    best_candidate: 0x3e,
                    best_filtered_power: 0x42 * 0x42,
                },
            }
        );
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
        );
        transition
            .advance(PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::CaptureChannelFrequencyControl
        );
        transition
            .advance(PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(
                PhyChannelFrequencyInitControl {
                    frequency_register_parameter_override: false,
                    frequency_table_initialized: true,
                    front_end_parameter_bit: false,
                },
            ))
            .unwrap();
        drive_warm_channel_frequency(&mut transition);
        let PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
            bbpll_register_snapshot,
            parameter,
            rfpll_lock_observed,
            sar2_reinitialized,
            xtal_duty: completed_xtal_duty,
            channel_frequency,
        }) = transition.action()
        else {
            panic!("RF init did not complete channel-frequency initialization");
        };
        assert_eq!(bbpll_register_snapshot, 0xa3);
        assert_eq!(parameter, final_parameter);
        assert!(rfpll_lock_observed);
        assert!(sar2_reinitialized);
        assert_eq!(completed_xtal_duty, xtal_duty);
        assert!(channel_frequency.table_was_initialized);
        assert!(channel_frequency.table_is_initialized);
        assert_eq!(channel_frequency.calibration, None);
    }

    #[test]
    fn rf_init_prefix_propagates_sdm_timeout_without_running_post_delay() {
        let bias_zero = PhyI2cAddress::new(0x6a, 0).unwrap();
        let bias_one = PhyI2cAddress::new(0x6a, 1).unwrap();
        let mut transition = PhyRfInitPrefixTransition::new();
        transition
            .advance(PhyRfInitPrefixCompletion::FeBbClockConfigured)
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::Bias(
                BiasRegCompletion::WriteCompleted { address: bias_zero },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::Bias(
                BiasRegCompletion::WriteCompleted { address: bias_one },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PreDelayConfigured,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DelayElapsed,
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured {
                    started_at_cycle: 0x1234_5678,
                },
            ))
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired: true },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
        );
        assert_eq!(
            transition.advance(PhyRfInitPrefixCompletion::DelayElapsed),
            Err(PhyRfInitPrefixTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn open_i2c_xpd_delayed_path_requires_explicit_async_completions() {
        let mut transition = OpenI2cXpdTransition::new(true);
        assert_eq!(transition.action(), OpenI2cXpdAction::ConfigurePreDelay);
        assert_eq!(
            transition.advance(OpenI2cXpdCompletion::DelayElapsed),
            Err(OpenI2cXpdTransitionError::WrongCompletion)
        );
        transition
            .advance(OpenI2cXpdCompletion::PreDelayConfigured)
            .unwrap();
        assert_eq!(transition.action(), OpenI2cXpdAction::DelayMicros(100));
        transition
            .advance(OpenI2cXpdCompletion::DelayElapsed)
            .unwrap();
        assert_eq!(
            transition.action(),
            OpenI2cXpdAction::ConfigurePowerAndPulse
        );
        transition
            .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 0x1234_5678,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle: 0x1234_5678,
                maximum_cycles: 9_999
            }
        );
    }

    #[test]
    fn open_i2c_xpd_samples_only_after_deadline_and_i2c_edges() {
        let mut transition = OpenI2cXpdTransition::new(false);
        transition
            .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 0xffff_ff00,
            })
            .unwrap();
        transition
            .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: false })
            .unwrap();
        assert_eq!(
            transition.action(),
            OpenI2cXpdAction::ReadSdmSample {
                address: PhyI2cAddress::new(0x63, 0).unwrap()
            }
        );

        transition
            .advance(OpenI2cXpdCompletion::SdmSample(0x42))
            .unwrap();
        assert_eq!(transition.samples(), 1);
        assert!(matches!(
            transition.action(),
            OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle: 0xffff_ff00,
                ..
            }
        ));

        transition
            .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: false })
            .unwrap();
        transition
            .advance(OpenI2cXpdCompletion::SdmSample(0x5b))
            .unwrap();
        assert_eq!(
            transition.action(),
            OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable)
        );
        assert_eq!(transition.samples(), 2);
        assert_eq!(
            transition.advance(OpenI2cXpdCompletion::SdmSample(0x5b)),
            Err(OpenI2cXpdTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn open_i2c_xpd_deadline_is_a_terminal_outcome() {
        let mut transition = OpenI2cXpdTransition::new(false);
        transition
            .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 7,
            })
            .unwrap();
        transition
            .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: true })
            .unwrap();
        assert_eq!(
            transition.action(),
            OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut)
        );
        assert_eq!(transition.samples(), 0);
    }
}
