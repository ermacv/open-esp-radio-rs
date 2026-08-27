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

use crate::phy_xtal_duty::{
    XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
    XtalDutyCalibrationParameters, XtalDutyCalibrationTransition,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::SharedPhyAccess;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{analog_i2c, phy_i2c as hal_phy_i2c};

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

    /// Block 0x62, register 5: low eight bits of the live calibrated RFPLL
    /// capacitor code sampled by `phy_read_pll_cap`.
    pub const RFPLL_CALIBRATED_CAPACITOR_LOW: PhyI2cAddress =
        PhyI2cAddress::new_internal(0x62, 0x05);

    /// Block 0x62, register 7, bit 2: high bit of the live calibrated RFPLL
    /// capacitor code sampled by `phy_read_pll_cap`.
    pub const RFPLL_CALIBRATED_CAPACITOR_HIGH: Field =
        Field::new(PhyI2cAddress::new_internal(0x62, 0x07), 2, 2);

    /// Block 0x62, register 12, bits 3:2: correction direction consumed by
    /// complete rev0 ROM `phy_rfpll_cap_correct`.
    pub const RFPLL_CAPACITOR_CORRECTION_DIRECTION: Field =
        Field::new(PhyI2cAddress::new_internal(0x62, 0x0c), 3, 2);

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

    /// Block 0x6b, register 3, bits 3:0: first Wi-Fi TX-temperature tracking
    /// field. Periodic tracking selects one of the reviewed values 10, 8 or 6.
    pub const WIFI_TX_TEMPERATURE_TRACKING_0: Field =
        Field::new(PhyI2cAddress::new_internal(0x6b, 0x03), 3, 0);

    /// Block 0x6b, register 7, bits 3:0: second Wi-Fi TX-temperature tracking
    /// field. Periodic tracking selects the reviewed value 15 or 13.
    pub const WIFI_TX_TEMPERATURE_TRACKING_1: Field =
        Field::new(PhyI2cAddress::new_internal(0x6b, 0x07), 3, 0);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
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
    let filter = parameter.filter_dcap();
    hal_phy_i2c::configure_command_memory(
        registers,
        hal_phy_i2c::PhyI2cCommandMemoryInputs::new(
            parameter.parameter_18e(),
            filter.parameter_e9,
            filter.parameter_ea,
            filter.parameter_ed,
            filter.parameter_ee,
            filter.parameter_f0,
        ),
    );
}

/// Publish one complete-register PHY-I2C read without waiting for completion.
///
/// Unlike the ROM read leaf, this function also rejects an already-busy host
/// before publishing the command. This is a deliberate fail-fast ownership
/// check, not a claim that the ROM performed the same pre-command check.
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_read`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_read(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    let host = configure_and_select_phy_i2c_host(registers, address);
    if hal_phy_i2c::sample_master_reset_busy(registers, host) {
        return Err(PhyI2cError::Busy);
    }
    hal_phy_i2c::publish_read_mask(registers, address.read_mask());
    hal_phy_i2c::publish_command(
        registers,
        host,
        address.block(),
        address.register(),
        0,
        false,
    );
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
    registers: &impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<u8, PhyI2cError> {
    let host = hal_host(address.host());
    if hal_phy_i2c::sample_master_reset_busy(registers, host) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(hal_phy_i2c::sample_result(registers, host))
    }
}

/// Publish one complete-register PHY-I2C write after observing the
/// pre-command busy state once. It never waits or loops on that state and
/// leaves post-command completion to [`try_finish_write`].
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_write`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_write(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    address: PhyI2cAddress,
    value: u8,
) -> Result<(), PhyI2cError> {
    let host = configure_and_select_phy_i2c_host(registers, address);
    if hal_phy_i2c::sample_master_reset_busy(registers, host) {
        return Err(PhyI2cError::Busy);
    }
    hal_phy_i2c::publish_command(
        registers,
        host,
        address.block(),
        address.register(),
        value,
        true,
    );
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
    registers: &impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    if hal_phy_i2c::sample_master_reset_busy(registers, hal_host(address.host())) {
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
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    address: PhyI2cAddress,
) -> hal_phy_i2c::PhyI2cHost {
    let host = hal_host(address.host());
    hal_phy_i2c::configure_host_map(registers);
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

/// Execute the finite register prefix which precedes the vendor
/// `ets_delay_us(100)` call in `phy_open_i2c_xpd_new(true)`.
///
/// This leaf deliberately stops before the delay and delegates to the
/// shared-PHY HAL. Basis: complete pinned `libphy.a[phy_reg.o]` sequence at
/// offsets `0x2e..0x4e`; field identities remain private to the custom PAC.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_pre_delay(registers: &mut impl SharedPhyAccess) {
    analog_i2c::prepare_open_i2c_pre_delay(registers);
}

/// Execute the finite common register suffix of `phy_open_i2c_xpd_new`.
///
/// The conditional PMU reset edge is preserved by the owned HAL. Basis:
/// complete pinned `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`; PMU field
/// identities remain private to the custom PAC.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_power_and_pulse(registers: &mut impl SharedPhyAccess) {
    analog_i2c::complete_open_i2c_power_and_reset(registers);
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
    pub fn start_target<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
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

    pub(crate) const fn pac_inputs(
        self,
    ) -> open_esp_radio_esp32s31_hal::phy_i2c::PhyFilterDcapInputs {
        open_esp_radio_esp32s31_hal::phy_i2c::PhyFilterDcapInputs::new(
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ed,
            self.parameter_ee,
            self.parameter_f0,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapAction {
    Configure(FilterDcapParameters),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapCompletion {
    Configured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapTransitionError {
    AlreadyComplete,
}

/// Semantic owner of the complete ROM `phy_filter_dcap_set` operation.
///
/// The finite analog-register transaction is owned by the PAC. This parent
/// stores only a five-byte parameter snapshot and never exposes register
/// geometry or reads a shared parameter image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterDcapTransition {
    parameter: FilterDcapParameters,
    complete: bool,
}

impl FilterDcapTransition {
    pub const fn new(parameter: FilterDcapParameters) -> Self {
        Self {
            parameter,
            complete: false,
        }
    }

    pub const fn parameters(self) -> FilterDcapParameters {
        self.parameter
    }

    pub const fn action(self) -> FilterDcapAction {
        if self.complete {
            FilterDcapAction::Complete
        } else {
            FilterDcapAction::Configure(self.parameter)
        }
    }

    pub fn advance(
        &mut self,
        _completion: FilterDcapCompletion,
    ) -> Result<(), FilterDcapTransitionError> {
        if self.complete {
            Err(FilterDcapTransitionError::AlreadyComplete)
        } else {
            self.complete = true;
            Ok(())
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

    pub(crate) const fn pac_initialization_stage_one_inputs(
        self,
    ) -> open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cInitializationStageOneInputs {
        open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cInitializationStageOneInputs::new(
            self.parameter_18e,
            self.filter_dcap.parameter_ee,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Action {
    Configure(PhyRfInitParameterSnapshot),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Completion {
    Configured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1TransitionError {
    AlreadyComplete,
}

/// Semantic owner of complete `libphy.a[phy_i2c.o]::phy_i2c_init1`.
///
/// The PAC owns the finite write plan. This transition receives the two
/// dynamic vendor facts through an owned snapshot without exposing any analog
/// register geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cInit1Transition {
    parameter: PhyRfInitParameterSnapshot,
    complete: bool,
}

impl I2cInit1Transition {
    pub const fn new(parameter: PhyRfInitParameterSnapshot) -> Self {
        Self {
            parameter,
            complete: false,
        }
    }

    pub const fn action(self) -> I2cInit1Action {
        if self.complete {
            I2cInit1Action::Complete
        } else {
            I2cInit1Action::Configure(self.parameter)
        }
    }

    pub fn advance(
        &mut self,
        _completion: I2cInit1Completion,
    ) -> Result<(), I2cInit1TransitionError> {
        if self.complete {
            Err(I2cInit1TransitionError::AlreadyComplete)
        } else {
            self.complete = true;
            Ok(())
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
    ConfigureBiasRegisters,
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
    ConfigureSar2,
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
    BiasRegistersConfigured,
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
    Sar2Configured,
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
    BiasRegisters,
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
    Sar2Configuration {
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
/// The finite MMIO and PAC-owned PHY-I2C plans are semantic actions. Every SDM
/// sample requires an external PHY-I2C completion. The 100- and 10-microsecond
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
            PhyRfInitPrefixStep::BiasRegisters => PhyRfInitPrefixAction::ConfigureBiasRegisters,
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
            PhyRfInitPrefixStep::Sar2Configuration { .. } => PhyRfInitPrefixAction::ConfigureSar2,
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
            ) => PhyRfInitPrefixStep::BiasRegisters,
            (
                PhyRfInitPrefixStep::BiasRegisters,
                PhyRfInitPrefixCompletion::BiasRegistersConfigured,
            ) => PhyRfInitPrefixStep::OpenI2cXpd(OpenI2cXpdTransition::new(true)),
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
                    PhyRfInitPrefixStep::Sar2Configuration {
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
                PhyRfInitPrefixStep::Sar2Configuration {
                    bbpll_register_snapshot,
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::Sar2Configured,
            ) => PhyRfInitPrefixStep::XtalDutyParameters {
                bbpll_register_snapshot,
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized: true,
            },
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
        AdcRateAction, AdcRateCompletion, AdcRateTransition, AdcRateTransitionError,
        FilterDcapAction, FilterDcapCompletion, FilterDcapParameters, FilterDcapTransition,
        FilterDcapTransitionError, I2cBbpllAction, I2cBbpllCompletion, I2cBbpllOutcome,
        I2cBbpllTransition, I2cBbpllTransitionError, I2cInit1Action, I2cInit1Completion,
        I2cInit1Transition, I2cInit1TransitionError, MaskedI2cWriteAction,
        MaskedI2cWriteCompletion, MaskedI2cWriteTransition, MaskedI2cWriteTransitionError,
        OpenI2cXpdAction, OpenI2cXpdCompletion, OpenI2cXpdOutcome, OpenI2cXpdTransition,
        OpenI2cXpdTransitionError, PhyI2cAddress, PhyRfInitParameterSnapshot,
        PhyRfInitPrefixAction, PhyRfInitPrefixCompletion, PhyRfInitPrefixOutcome,
        PhyRfInitPrefixStep, PhyRfInitPrefixTransition, PhyRfInitPrefixTransitionError,
        RcCalibrationAction, RcCalibrationCompletion, RcCalibrationSetAction,
        RcCalibrationSetCompletion, RcCalibrationSetTransition, RcCalibrationTransition,
        RcCalibrationTransitionError, RfpllChargePumpAction, RfpllChargePumpCompletion,
        RfpllChargePumpOutcome, RfpllChargePumpTransition,
    };
    use crate::phy_cold::PhyColdExternalBinding;
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
    };
    use crate::phy_frequency::{
        PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
        PhyChannelFrequencyInitControl, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
    };

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
            PhyRxDcoAction::PrepareRxDcoControlRestore => {
                PhyRxDcoCompletion::RxDcoControlRestorePrepared
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
            PhyRxDcoAction::RestoreRxDcoControl => PhyRxDcoCompletion::RxDcoControlRestored,
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
            XtalDutyPrepareAction::PrepareRxDcoControlRestore => {
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared
            }
            XtalDutyPrepareAction::RxDco(action) => {
                XtalDutyPrepareCompletion::RxDco(complete_rx_dco(action))
            }
            XtalDutyPrepareAction::RestoreRxDcoControl => {
                XtalDutyPrepareCompletion::RxDcoControlRestored
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
    fn filter_dcap_exposes_one_semantic_operation() {
        let parameter = FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87);
        let mut transition = FilterDcapTransition::new(parameter);
        assert_eq!(transition.action(), FilterDcapAction::Configure(parameter));
        transition
            .advance(FilterDcapCompletion::Configured)
            .unwrap();
        assert_eq!(transition.action(), FilterDcapAction::Complete);
        assert_eq!(
            transition.advance(FilterDcapCompletion::Configured),
            Err(FilterDcapTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn i2c_init1_exposes_one_semantic_operation() {
        let filter = FilterDcapParameters::new(1, 2, 3, 0xfe, 5);
        let parameter = PhyRfInitParameterSnapshot::new(filter, 0x55);
        let mut transition = I2cInit1Transition::new(parameter);
        assert_eq!(transition.action(), I2cInit1Action::Configure(parameter));
        transition.advance(I2cInit1Completion::Configured).unwrap();
        assert_eq!(transition.action(), I2cInit1Action::Complete);
        assert_eq!(
            transition.advance(I2cInit1Completion::Configured),
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
            PhyRfInitPrefixAction::ConfigureBiasRegisters
        );
        transition
            .advance(PhyRfInitPrefixCompletion::BiasRegistersConfigured)
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
        assert!(matches!(
            transition.action(),
            PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Configure(_))
        ));
        transition
            .advance(PhyRfInitPrefixCompletion::FilterDcap(
                FilterDcapCompletion::Configured,
            ))
            .unwrap();
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
        assert!(matches!(
            transition.action(),
            PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Configure(_))
        ));
        transition
            .advance(PhyRfInitPrefixCompletion::I2cInit1(
                I2cInit1Completion::Configured,
            ))
            .unwrap();
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
        assert_eq!(transition.action(), PhyRfInitPrefixAction::ConfigureSar2);
        transition
            .advance(PhyRfInitPrefixCompletion::Sar2Configured)
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
        let mut transition = PhyRfInitPrefixTransition::new();
        transition
            .advance(PhyRfInitPrefixCompletion::FeBbClockConfigured)
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
            .unwrap();
        transition
            .advance(PhyRfInitPrefixCompletion::BiasRegistersConfigured)
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
