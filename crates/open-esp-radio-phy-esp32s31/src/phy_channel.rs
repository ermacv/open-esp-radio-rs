//! Rust-owned ESP32-S31 channel-programming transition.
//!
//! The pinned root is `libphy.a[phy_rfpll.o]::phy_chip_set_chan`, size
//! `0x10e`.  Its qualified Wi-Fi AP/STA path reaches only channels 1 through
//! 13.  The two vendor PHY-I2C critical-section callbacks are single `ret`
//! instructions in the final image and are intentionally absent: the unique
//! Rust radio owner serializes the complete transition.
//!
//! The rev0 ROM fast-frequency child used for 2.4-GHz channels contains a
//! one-microsecond delay, a ten-microsecond delay and an unbounded poll of
//! `0x2010_0028[8]`.  Rust represents both delays and every readiness sample
//! as caller-completed actions.  Because no completion interrupt for that bit
//! is proved by the available PAC/SVD or ROM symbols, the outer async owner
//! may deliver further samples; it must deliver `FrequencyReadyTimedOut` at
//! its finite deadline.  No transition method spins, sleeps or self-wakes.

use crate::{
    phy_i2c::PhyI2cAddress,
    phy_temperature::{
        PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureFailure,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
};

pub const PHY_FREQUENCY_READY_ADDRESS: usize = 0x2010_0028;
pub const PHY_FREQUENCY_READY_MASK: u32 = 1 << 8;

const TX_CAP_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x6b, 2);

// Pinned `phy_tx_gain.o` `.rodata` slices at offsets 0x6c, 0x90 and 0xb4.
// Their semantic units are not public. The recovered Rust translation below
// uses the same 18 aligned little-endian halfwords.
const WIFI_TX_GAIN_TABLE_LOW: [u16; 18] = [
    0x003f, 0x0037, 0x002f, 0x0027, 0x0027, 0x001f, 0x0017, 0x000f, 0x000f, 0x000d, 0x000c, 0x0007,
    0x0006, 0x0005, 0x0004, 0x0003, 0x0002, 0x0001,
];
const WIFI_TX_GAIN_TABLE_MID: [u16; 18] = [
    0x0100, 0x0100, 0x0100, 0x0100, 0x8000, 0x8000, 0x8000, 0x8000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const WIFI_TX_GAIN_TABLE_HIGH: [u16; 18] = [
    0x001b, 0x0018, 0x0014, 0x000e, 0x0006, 0x0000, 0xfff6, 0xffe9, 0xffe1, 0xffd7, 0xffd0, 0xffc9,
    0xffc4, 0xffbe, 0xffb8, 0xffb0, 0xffa5, 0xff97,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelParameters {
    pub frequency_offset: i16,
    pub crystal_selector: u8,
    /// The qualified basic AP/STA profile requires this vendor option to be
    /// disabled. Channel 14 is rejected independently.
    pub channel_14_mic_enabled: bool,
    /// Preserved as owned state. The complete `phy_11p_set` body only writes
    /// these two bytes back to the former `phy_param` image.
    pub dot11p_enabled: bool,
    pub dot11p_config: u8,
    pub tx_gain_skip_publication: bool,
    pub tx_gain_seed: [u32; 6],
    pub tx_gain_config: u16,
    pub tx_gain_curve: [u8; 6],
    pub tx_gain_correction: i8,
    pub tx_gain_base: u8,
    pub tx_gain_delta: u8,
    /// Former `phy_param[0xdc..=0xe1]`. Only bytes 0, 2 and 4 are selected
    /// by complete ROM `phy_set_txcap_reg`.
    pub tx_capacitance: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelRequest {
    /// A channel number or frequency in MHz, exactly as accepted by the
    /// pinned root.
    pub channel_or_frequency: u16,
    pub cbw: u8,
    pub parameters: PhyChipChannelParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainRequest {
    pub channel: u16,
    pub calibration_curve: [u8; 6],
    pub correction: i8,
    pub base_and_delta: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainImage {
    pub seed: [u32; 6],
    pub output_32: [u32; 8],
    pub output_64: [u32; 16],
    pub output_72: [u32; 18],
    pub config: u16,
}

const fn interpolate_tx_gain_curve(channel: u16, curve: [u8; 6]) -> i8 {
    let channel_index = channel.wrapping_sub(1);
    let value = if channel_index <= 5 {
        let low = curve[0] as i8 as i32;
        let high = curve[1] as i8 as i32;
        (high - low) * channel_index as i32 / 5 + low
    } else if channel_index <= 10 {
        let low = curve[1] as i8 as i32;
        let high = curve[2] as i8 as i32;
        (high - low) * (channel_index - 5) as i32 / 5 + low
    } else {
        curve[2] as i8 as i32 + 2
    };
    value as i8
}

const fn select_tx_gain_index(mut index: u8, target: i16) -> u8 {
    let mut iteration = 0;
    while iteration != WIFI_TX_GAIN_TABLE_HIGH.len() {
        let current = WIFI_TX_GAIN_TABLE_HIGH[index as usize] as i16;
        if target < current {
            if index as usize == WIFI_TX_GAIN_TABLE_HIGH.len() - 1 {
                break;
            }
            index += 1;
            let next = WIFI_TX_GAIN_TABLE_HIGH[index as usize] as i16;
            if target >= next {
                break;
            }
        } else {
            if index == 0 {
                break;
            }
            let previous = WIFI_TX_GAIN_TABLE_HIGH[index as usize - 1] as i16;
            if target < previous {
                break;
            }
            index -= 1;
        }
        iteration += 1;
    }
    index
}

const fn write_packed_byte(words: &mut [u32; 8], index: usize, value: u8) {
    let word = index >> 2;
    let shift = (index & 3) << 3;
    words[word] = (words[word] & !(0xff_u32 << shift)) | ((value as u32) << shift);
}

const fn write_packed_halfword<const N: usize>(words: &mut [u32; N], index: usize, value: u16) {
    let word = index >> 1;
    let shift = (index & 1) << 4;
    words[word] = (words[word] & !(0xffff_u32 << shift)) | ((value as u32) << shift);
}

/// Complete pure translation of rev0 ROM `phy_wifi_get_tx_gain` (`0x2f826ff8`,
/// 258 bytes), including its `phy_set_chan_cal_interp` and
/// `phy_get_tx_gain_value` children.
///
/// The debug-only `ets_printf` branch is omitted. The reference caller always
/// supplies mode zero, and formatting is outside the radio algorithm. All
/// inputs and outputs are ordinary values; there is no MMIO, ROM ABI, global
/// state, allocation, callback, or wait.
pub const fn calculate_wifi_tx_gain(request: PhyWifiTxGainRequest) -> PhyWifiTxGainImage {
    let interpolation = interpolate_tx_gain_curve(request.channel, request.calibration_curve);
    let mut target = request.base_and_delta as i16 + 0x54 - request.correction as i16;
    let mut table_index = 0_u8;
    let mut image = PhyWifiTxGainImage {
        seed: [0; 6],
        output_32: [0; 8],
        output_64: [0; 16],
        output_72: [0; 18],
        config: 0,
    };
    let mut output_index = 0;
    while output_index != 32 {
        table_index = select_tx_gain_index(table_index, target);
        let table_index_usize = table_index as usize;
        let residual = target.wrapping_sub(WIFI_TX_GAIN_TABLE_HIGH[table_index_usize] as i16);
        write_packed_byte(
            &mut image.output_32,
            output_index,
            residual.wrapping_sub(interpolation as i16) as u8,
        );
        write_packed_halfword(
            &mut image.output_64,
            output_index,
            WIFI_TX_GAIN_TABLE_MID[table_index_usize],
        );
        write_packed_halfword(
            &mut image.output_72,
            output_index,
            WIFI_TX_GAIN_TABLE_LOW[table_index_usize],
        );
        target = target.wrapping_sub(4);
        output_index += 1;
    }
    image
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelOutcome {
    pub channel: u16,
    pub frequency_mhz: u16,
    pub cbw: u8,
    pub init_complete: bool,
    pub temperature: PhyTemperatureOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelFailure {
    UnsupportedChannel(u16),
    UnsupportedFrequency(u16),
    Channel14MicEnabled,
    Temperature(PhyTemperatureFailure),
    FrequencyReadyTimedOut { samples: u32 },
    I2cTimedOut { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelDelay {
    FrequencyStartPulse,
    FrequencySettle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelI2cPhase {
    ProgramTxCap,
    CaptureTxCap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelAction {
    SetAgc {
        enabled: bool,
    },
    SetBbpllCalibration {
        enabled: bool,
    },
    Temperature(PhyTemperatureAction),
    StartFrequencySwitch {
        frequency_index: u8,
        crystal_selector: u8,
    },
    DelayMicros {
        phase: PhyChipChannelDelay,
        micros: u32,
    },
    ClearFrequencySwitch,
    AwaitFrequencyReadyEdge {
        address: usize,
        mask: u32,
        samples: u32,
    },
    ConfigureNrx {
        frequency_mhz: u16,
    },
    ConfigureBssCbw {
        cbw: u8,
    },
    ConfigureRxCompensation {
        pass: u8,
    },
    WriteI2c {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    CalculateTxGain(PhyWifiTxGainRequest),
    PublishTxGain(PhyWifiTxGainImage),
    ReadI2c {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
    },
    PublishTxCapCommandMemory {
        value: u8,
    },
    ConfigureChannelCbw {
        cbw: u8,
    },
    ClearDcMemory,
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelCompletion {
    AgcSet {
        enabled: bool,
    },
    BbpllCalibrationSet {
        enabled: bool,
    },
    Temperature(PhyTemperatureCompletion),
    FrequencySwitchStarted {
        frequency_index: u8,
        crystal_selector: u8,
    },
    DelayElapsed {
        phase: PhyChipChannelDelay,
        micros: u32,
    },
    FrequencySwitchCleared,
    FrequencyReadyObserved {
        address: usize,
        value: u32,
    },
    FrequencyReadyTimedOut,
    NrxConfigured {
        frequency_mhz: u16,
    },
    BssCbwConfigured {
        cbw: u8,
    },
    RxCompensationConfigured {
        pass: u8,
    },
    I2cWriteCompleted {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    I2cReadCompleted {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    I2cTimedOut {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
    },
    I2cDeadlineExceeded {
        address: PhyI2cAddress,
    },
    TxGainCalculated {
        request: PhyWifiTxGainRequest,
        image: PhyWifiTxGainImage,
    },
    TxGainPublished,
    TxCapCommandMemoryPublished {
        value: u8,
    },
    ChannelCbwConfigured {
        cbw: u8,
    },
    DcMemoryCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupContinuation {
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    DisableAgc,
    EnableBbpll,
    Temperature(PhyTemperatureTransition),
    StartFrequencySwitch,
    FrequencyStartDelay,
    ClearFrequencySwitch,
    FrequencySettleDelay,
    AwaitFrequencyReady { samples: u32 },
    ConfigureNrxFirst,
    ConfigureBssCbw,
    ConfigureRxCompensationFirst,
    ProgramTxCap,
    ConfigureNrxSecond,
    CalculateTxGain,
    PublishTxGain(PhyWifiTxGainImage),
    CaptureTxCap,
    PublishTxCapCommandMemory { value: u8 },
    ConfigureChannelCbw,
    ConfigureRxCompensationSecond,
    DisableBbpll(CleanupContinuation),
    ClearDcMemory(CleanupContinuation),
    EnableAgc(CleanupContinuation),
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

/// Exact pure translation of rev0 ROM `phy_chan_to_freq`.
pub const fn channel_to_frequency(channel: u16) -> u16 {
    if channel > 14 {
        channel
    } else if channel == 14 {
        2_484
    } else {
        2_407_u16.wrapping_add(channel.wrapping_mul(5))
    }
}

/// Exact pure translation of rev0 ROM `phy_mhz2ieee`.
pub const fn frequency_to_channel(frequency_mhz: u16) -> u16 {
    if frequency_mhz == 2_484 {
        14
    } else if frequency_mhz <= 2_483 {
        ((frequency_mhz.wrapping_sub(2_407) / 5) as u8 as i8) as u16
    } else {
        (((frequency_mhz.wrapping_sub(5_008) / 20).wrapping_add(15)) as u8 as i8) as u16
    }
}

const fn normalized_channel(channel_or_frequency: u16) -> u16 {
    if channel_or_frequency > 2_411 {
        frequency_to_channel(channel_or_frequency)
    } else {
        channel_or_frequency
    }
}

const fn tx_cap_value(channel: u16, capacitance: [u8; 6]) -> u8 {
    let selected = if channel <= 3 {
        capacitance[0]
    } else if channel <= 8 {
        capacitance[2]
    } else {
        capacitance[4]
    };
    selected | 0xc0
}

impl PhyChipChannelRequest {
    pub const fn validate(self) -> Result<(), PhyChipChannelFailure> {
        let channel = normalized_channel(self.channel_or_frequency);
        if self.parameters.channel_14_mic_enabled {
            return Err(PhyChipChannelFailure::Channel14MicEnabled);
        }
        if channel == 0 || channel > 13 {
            return Err(PhyChipChannelFailure::UnsupportedChannel(channel));
        }
        let frequency = channel_to_frequency(self.channel_or_frequency);
        let index = frequency.wrapping_sub(2_400);
        if index > 84 {
            return Err(PhyChipChannelFailure::UnsupportedFrequency(frequency));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelTransition {
    request: PhyChipChannelRequest,
    channel: u16,
    frequency_mhz: u16,
    temperature: Option<PhyTemperatureOutcome>,
    step: Step,
}

impl PhyChipChannelTransition {
    pub const fn new(request: PhyChipChannelRequest) -> Self {
        let channel = normalized_channel(request.channel_or_frequency);
        let frequency_mhz = channel_to_frequency(request.channel_or_frequency);
        let step = match request.validate() {
            Ok(()) => Step::DisableAgc,
            Err(failure) => Step::Failed(failure),
        };
        Self {
            request,
            channel,
            frequency_mhz,
            temperature: None,
            step,
        }
    }

    const fn frequency_index(self) -> u8 {
        self.frequency_mhz.wrapping_sub(2_400) as u8
    }

    const fn outcome(self) -> PhyChipChannelOutcome {
        let Some(temperature) = self.temperature else {
            panic!("channel transition completed without a temperature outcome");
        };
        PhyChipChannelOutcome {
            channel: self.channel,
            frequency_mhz: self.frequency_mhz,
            cbw: self.request.cbw,
            init_complete: self.request.cbw != 0,
            temperature,
        }
    }

    const fn tx_gain_request(self) -> PhyWifiTxGainRequest {
        PhyWifiTxGainRequest {
            channel: self.channel,
            calibration_curve: self.request.parameters.tx_gain_curve,
            correction: self.request.parameters.tx_gain_correction,
            base_and_delta: self
                .request
                .parameters
                .tx_gain_base
                .wrapping_add(self.request.parameters.tx_gain_delta)
                as i8,
        }
    }

    pub const fn action(self) -> PhyChipChannelAction {
        match self.step {
            Step::DisableAgc => PhyChipChannelAction::SetAgc { enabled: false },
            Step::EnableBbpll => PhyChipChannelAction::SetBbpllCalibration { enabled: true },
            Step::Temperature(transition) => PhyChipChannelAction::Temperature(transition.action()),
            Step::StartFrequencySwitch => PhyChipChannelAction::StartFrequencySwitch {
                frequency_index: self.frequency_index(),
                crystal_selector: self.request.parameters.crystal_selector,
            },
            Step::FrequencyStartDelay => PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencyStartPulse,
                micros: 1,
            },
            Step::ClearFrequencySwitch => PhyChipChannelAction::ClearFrequencySwitch,
            Step::FrequencySettleDelay => PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencySettle,
                micros: 10,
            },
            Step::AwaitFrequencyReady { samples } => {
                PhyChipChannelAction::AwaitFrequencyReadyEdge {
                    address: PHY_FREQUENCY_READY_ADDRESS,
                    mask: PHY_FREQUENCY_READY_MASK,
                    samples,
                }
            }
            Step::ConfigureNrxFirst | Step::ConfigureNrxSecond => {
                PhyChipChannelAction::ConfigureNrx {
                    frequency_mhz: self.frequency_mhz,
                }
            }
            Step::ConfigureBssCbw => PhyChipChannelAction::ConfigureBssCbw {
                cbw: self.request.cbw,
            },
            Step::ConfigureRxCompensationFirst => {
                PhyChipChannelAction::ConfigureRxCompensation { pass: 0 }
            }
            Step::ProgramTxCap => PhyChipChannelAction::WriteI2c {
                phase: PhyChipChannelI2cPhase::ProgramTxCap,
                address: TX_CAP_ADDRESS,
                value: tx_cap_value(self.channel, self.request.parameters.tx_capacitance),
            },
            Step::CalculateTxGain => PhyChipChannelAction::CalculateTxGain(self.tx_gain_request()),
            Step::PublishTxGain(image) => PhyChipChannelAction::PublishTxGain(image),
            Step::CaptureTxCap => PhyChipChannelAction::ReadI2c {
                phase: PhyChipChannelI2cPhase::CaptureTxCap,
                address: TX_CAP_ADDRESS,
            },
            Step::PublishTxCapCommandMemory { value } => {
                PhyChipChannelAction::PublishTxCapCommandMemory { value }
            }
            Step::ConfigureChannelCbw => PhyChipChannelAction::ConfigureChannelCbw {
                cbw: self.request.cbw,
            },
            Step::ConfigureRxCompensationSecond => {
                PhyChipChannelAction::ConfigureRxCompensation { pass: 1 }
            }
            Step::DisableBbpll(_) => PhyChipChannelAction::SetBbpllCalibration { enabled: false },
            Step::ClearDcMemory(_) => PhyChipChannelAction::ClearDcMemory,
            Step::EnableAgc(_) => PhyChipChannelAction::SetAgc { enabled: true },
            Step::Complete(outcome) => PhyChipChannelAction::Complete(outcome),
            Step::Failed(failure) => PhyChipChannelAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyChipChannelCompletion,
    ) -> Result<(), PhyChipChannelTransitionError> {
        self.step = match (self.step, completion) {
            (Step::DisableAgc, PhyChipChannelCompletion::AgcSet { enabled: false }) => {
                Step::EnableBbpll
            }
            (
                Step::EnableBbpll,
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled: true },
            ) => Step::Temperature(PhyTemperatureTransition::new()),
            (
                Step::Temperature(mut transition),
                PhyChipChannelCompletion::Temperature(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChipChannelTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTemperatureAction::Complete(outcome) => {
                        self.temperature = Some(outcome);
                        Step::StartFrequencySwitch
                    }
                    PhyTemperatureAction::Failed(failure) => {
                        let continuation = CleanupContinuation::Failed(
                            PhyChipChannelFailure::Temperature(failure),
                        );
                        Step::DisableBbpll(continuation)
                    }
                    _ => Step::Temperature(transition),
                }
            }
            (Step::Temperature(_), PhyChipChannelCompletion::I2cDeadlineExceeded { address }) => {
                Step::DisableBbpll(CleanupContinuation::Failed(
                    PhyChipChannelFailure::I2cTimedOut { address },
                ))
            }
            (
                Step::StartFrequencySwitch,
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                },
            ) if frequency_index == self.frequency_index()
                && crystal_selector == self.request.parameters.crystal_selector =>
            {
                Step::FrequencyStartDelay
            }
            (
                Step::FrequencyStartDelay,
                PhyChipChannelCompletion::DelayElapsed {
                    phase: PhyChipChannelDelay::FrequencyStartPulse,
                    micros: 1,
                },
            ) => Step::ClearFrequencySwitch,
            (Step::ClearFrequencySwitch, PhyChipChannelCompletion::FrequencySwitchCleared) => {
                Step::FrequencySettleDelay
            }
            (
                Step::FrequencySettleDelay,
                PhyChipChannelCompletion::DelayElapsed {
                    phase: PhyChipChannelDelay::FrequencySettle,
                    micros: 10,
                },
            ) => Step::AwaitFrequencyReady { samples: 0 },
            (
                Step::AwaitFrequencyReady { samples },
                PhyChipChannelCompletion::FrequencyReadyObserved {
                    address: PHY_FREQUENCY_READY_ADDRESS,
                    value,
                },
            ) => {
                if value & PHY_FREQUENCY_READY_MASK != 0 {
                    Step::ConfigureNrxFirst
                } else {
                    Step::AwaitFrequencyReady {
                        samples: samples.wrapping_add(1),
                    }
                }
            }
            (
                Step::AwaitFrequencyReady { samples },
                PhyChipChannelCompletion::FrequencyReadyTimedOut,
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::FrequencyReadyTimedOut { samples },
            )),
            (
                Step::ConfigureNrxFirst,
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == self.frequency_mhz => Step::ConfigureBssCbw,
            (Step::ConfigureBssCbw, PhyChipChannelCompletion::BssCbwConfigured { cbw })
                if cbw == self.request.cbw =>
            {
                Step::ConfigureRxCompensationFirst
            }
            (
                Step::ConfigureRxCompensationFirst,
                PhyChipChannelCompletion::RxCompensationConfigured { pass: 0 },
            ) => Step::ProgramTxCap,
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cWriteCompleted {
                    phase: PhyChipChannelI2cPhase::ProgramTxCap,
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) if value == tx_cap_value(self.channel, self.request.parameters.tx_capacitance) => {
                Step::ConfigureNrxSecond
            }
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cTimedOut {
                    phase: PhyChipChannelI2cPhase::ProgramTxCap,
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cDeadlineExceeded {
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::ConfigureNrxSecond,
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == self.frequency_mhz => Step::CalculateTxGain,
            (
                Step::CalculateTxGain,
                PhyChipChannelCompletion::TxGainCalculated { request, mut image },
            ) if request == self.tx_gain_request() => {
                image.seed = self.request.parameters.tx_gain_seed;
                image.config = self.request.parameters.tx_gain_config;
                if self.request.parameters.tx_gain_skip_publication {
                    Step::CaptureTxCap
                } else {
                    Step::PublishTxGain(image)
                }
            }
            (Step::PublishTxGain(_), PhyChipChannelCompletion::TxGainPublished) => {
                Step::CaptureTxCap
            }
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cReadCompleted {
                    phase: PhyChipChannelI2cPhase::CaptureTxCap,
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) => Step::PublishTxCapCommandMemory { value },
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cTimedOut {
                    phase: PhyChipChannelI2cPhase::CaptureTxCap,
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cDeadlineExceeded {
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::PublishTxCapCommandMemory { value },
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value: completed },
            ) if value == completed => Step::ConfigureChannelCbw,
            (Step::ConfigureChannelCbw, PhyChipChannelCompletion::ChannelCbwConfigured { cbw })
                if cbw == self.request.cbw =>
            {
                Step::ConfigureRxCompensationSecond
            }
            (
                Step::ConfigureRxCompensationSecond,
                PhyChipChannelCompletion::RxCompensationConfigured { pass: 1 },
            ) => Step::DisableBbpll(CleanupContinuation::Complete(self.outcome())),
            (
                Step::DisableBbpll(continuation),
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled: false },
            ) => Step::ClearDcMemory(continuation),
            (Step::ClearDcMemory(continuation), PhyChipChannelCompletion::DcMemoryCleared) => {
                Step::EnableAgc(continuation)
            }
            (Step::EnableAgc(continuation), PhyChipChannelCompletion::AgcSet { enabled: true }) => {
                match continuation {
                    CleanupContinuation::Complete(outcome) => Step::Complete(outcome),
                    CleanupContinuation::Failed(failure) => Step::Failed(failure),
                }
            }
            (Step::Complete(_) | Step::Failed(_), _) => {
                return Err(PhyChipChannelTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyChipChannelTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelBindingError {
    NotDirectMmio,
}

/// Non-cloneable token for one finite channel-specific MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelMmioBinding {
    action: PhyChipChannelAction,
}

impl PhyChipChannelMmioBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelBindingError> {
        match action {
            PhyChipChannelAction::SetAgc { .. }
            | PhyChipChannelAction::SetBbpllCalibration { .. }
            | PhyChipChannelAction::StartFrequencySwitch { .. }
            | PhyChipChannelAction::ClearFrequencySwitch
            | PhyChipChannelAction::AwaitFrequencyReadyEdge { .. }
            | PhyChipChannelAction::ConfigureNrx { .. }
            | PhyChipChannelAction::ConfigureBssCbw { .. }
            | PhyChipChannelAction::ConfigureRxCompensation { .. }
            | PhyChipChannelAction::PublishTxGain(_)
            | PhyChipChannelAction::PublishTxCapCommandMemory { .. }
            | PhyChipChannelAction::ConfigureChannelCbw { .. }
            | PhyChipChannelAction::ClearDcMemory => Ok(Self { action }),
            _ => Err(PhyChipChannelBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyChipChannelAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyChipChannelCompletion {
        match self.action {
            PhyChipChannelAction::SetAgc { enabled } => {
                crate::radio_hal::set_phy_channel_agc(registers, enabled);
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                crate::radio_hal::wifi_strict_phy_bbpll_cal(u32::from(enabled));
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => {
                crate::radio_hal::start_phy_channel_frequency_switch(
                    frequency_index,
                    crystal_selector,
                );
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                crate::radio_hal::clear_phy_channel_frequency_switch();
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { address, .. } => {
                PhyChipChannelCompletion::FrequencyReadyObserved {
                    address,
                    value: crate::radio_hal::sample_phy_channel_frequency_ready(),
                }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                crate::radio_hal::configure_phy_channel_nrx_frequency(frequency_mhz);
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                crate::radio_hal::configure_phy_channel_bss_cbw(cbw);
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation { pass } => {
                open_esp_radio_hal_esp32s31::phy_agc::configure_rx_compensation(registers);
                PhyChipChannelCompletion::RxCompensationConfigured { pass }
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                crate::radio_hal::publish_phy_tx_gain_memory(registers, false, image);
                PhyChipChannelCompletion::TxGainPublished
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                crate::radio_hal::publish_phy_tx_cap_command_memory(value);
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                crate::radio_hal::configure_phy_channel_cbw(cbw);
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => {
                crate::radio_hal::wifi_strict_phy_dc_mem_clr();
                PhyChipChannelCompletion::DcMemoryCleared
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable token for the pure Rust TX-gain calculation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainBinding {
    request: PhyWifiTxGainRequest,
}

impl PhyWifiTxGainBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelBindingError> {
        match action {
            PhyChipChannelAction::CalculateTxGain(request) => Ok(Self { request }),
            _ => Err(PhyChipChannelBindingError::NotDirectMmio),
        }
    }

    pub const fn request(&self) -> PhyWifiTxGainRequest {
        self.request
    }

    pub const fn execute(self) -> PhyChipChannelCompletion {
        PhyChipChannelCompletion::TxGainCalculated {
            request: self.request,
            image: calculate_wifi_tx_gain(self.request),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelTimerBinding {
    phase: PhyChipChannelDelay,
    micros: u32,
}

impl PhyChipChannelTimerBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        match action {
            PhyChipChannelAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyChipChannelExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyChipChannelCompletion {
        PhyChipChannelCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelI2cBinding {
    outer_action: PhyChipChannelAction,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl PhyChipChannelI2cBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        let request = match action {
            PhyChipChannelAction::WriteI2c { address, value, .. } => {
                crate::phy_cold::PhyColdI2cRequest::write_byte(address, value)
            }
            PhyChipChannelAction::ReadI2c { address, .. } => {
                crate::phy_cold::PhyColdI2cRequest::read_byte(address)
            }
            _ => return Err(PhyChipChannelExternalBindingError::UnsupportedAction),
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
        result: Result<u8, crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyChipChannelCompletion, PhyChipChannelExternalBindingError> {
        let crate::phy_cold::PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(PhyChipChannelExternalBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyChipChannelAction::WriteI2c {
                    phase,
                    address,
                    value,
                },
                crate::phy_cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(PhyChipChannelCompletion::I2cWriteCompleted {
                phase,
                address,
                value,
            }),
            (
                PhyChipChannelAction::ReadI2c { phase, address },
                crate::phy_cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(PhyChipChannelCompletion::I2cReadCompleted {
                phase,
                address,
                value,
            }),
            _ => Err(PhyChipChannelExternalBindingError::UnexpectedOutcome),
        }
    }

    pub const fn into_timeout_completion(self) -> PhyChipChannelCompletion {
        match self.outer_action {
            PhyChipChannelAction::WriteI2c { phase, address, .. }
            | PhyChipChannelAction::ReadI2c { phase, address } => {
                PhyChipChannelCompletion::I2cTimedOut { phase, address }
            }
            _ => unreachable!(),
        }
    }
}

/// Exhaustive lowering of every non-terminal channel action.
///
#[derive(Debug, Eq, PartialEq)]
pub enum PhyChipChannelExternalBinding {
    Mmio(PhyChipChannelMmioBinding),
    Temperature(crate::phy_temperature::PhyTemperatureExternalBinding),
    Timer(PhyChipChannelTimerBinding),
    I2c(PhyChipChannelI2cBinding),
    TxGain(PhyWifiTxGainBinding),
}

impl PhyChipChannelExternalBinding {
    pub fn lower(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        if let Ok(binding) = PhyChipChannelMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyChipChannelAction::Temperature(action) => {
                crate::phy_temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyChipChannelExternalBindingError::UnsupportedAction)
            }
            PhyChipChannelAction::DelayMicros { .. } => {
                PhyChipChannelTimerBinding::new(action).map(Self::Timer)
            }
            PhyChipChannelAction::WriteI2c { .. } | PhyChipChannelAction::ReadI2c { .. } => {
                PhyChipChannelI2cBinding::new(action).map(Self::I2c)
            }
            PhyChipChannelAction::CalculateTxGain(_) => PhyWifiTxGainBinding::new(action)
                .map(Self::TxGain)
                .map_err(|_| PhyChipChannelExternalBindingError::UnsupportedAction),
            PhyChipChannelAction::Complete(_) | PhyChipChannelAction::Failed(_) => {
                Err(PhyChipChannelExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyChipChannelExternalBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: PhyChipChannelParameters = PhyChipChannelParameters {
        frequency_offset: 0,
        crystal_selector: 3,
        channel_14_mic_enabled: false,
        dot11p_enabled: false,
        dot11p_config: 0,
        tx_gain_skip_publication: false,
        tx_gain_seed: [1, 2, 3, 4, 5, 6],
        tx_gain_config: 0x1234,
        tx_gain_curve: [7, 8, 9, 10, 11, 12],
        tx_gain_correction: -3,
        tx_gain_base: 20,
        tx_gain_delta: 2,
        tx_capacitance: [1, 2, 3, 4, 5, 6],
    };

    const REQUEST: PhyChipChannelRequest = PhyChipChannelRequest {
        channel_or_frequency: 11,
        cbw: 0,
        parameters: PARAMETERS,
    };

    #[test]
    fn rust_tx_gain_translation_matches_the_recovered_packed_layout() {
        let image = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
            channel: 11,
            calibration_curve: PARAMETERS.tx_gain_curve,
            correction: PARAMETERS.tx_gain_correction,
            base_and_delta: PARAMETERS
                .tx_gain_base
                .wrapping_add(PARAMETERS.tx_gain_delta) as i8,
        });
        assert_eq!(
            image.output_32,
            [
                0x3d41_4549,
                0x2d31_3539,
                0x1d21_2529,
                0x0d11_1519,
                0xfd01_0509,
                0xfaf8_f8f9,
                0xf8fc_fafe,
                0xff03_fafe,
            ]
        );
        assert_eq!(
            image.output_64,
            [
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x0100_0100,
                0x8000_8000,
                0x8000_8000,
                0x8000_8000,
                0x8000_8000,
            ]
        );
        assert_eq!(
            image.output_72,
            [
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x003f_003f,
                0x0037_003f,
                0x0027_002f,
                0x0027_0027,
                0x001f_001f,
                0x0017_0017,
                0x000f_000f,
                0,
                0,
            ]
        );
        assert_eq!(image.seed, [0; 6]);
        assert_eq!(image.config, 0);
    }

    fn temperature_completion(action: PhyTemperatureAction) -> PhyTemperatureCompletion {
        match action {
            PhyTemperatureAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => PhyTemperatureCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: 15,
            },
            PhyTemperatureAction::SampleCode { address, .. } => {
                PhyTemperatureCompletion::CodeSampled {
                    address,
                    value: 128,
                }
            }
            PhyTemperatureAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            } => PhyTemperatureCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
                value,
            },
            action => panic!("unexpected terminal temperature action: {action:?}"),
        }
    }

    fn direct_completion(action: PhyChipChannelAction, ready: bool) -> PhyChipChannelCompletion {
        match action {
            PhyChipChannelAction::SetAgc { enabled } => {
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::Temperature(action) => {
                PhyChipChannelCompletion::Temperature(temperature_completion(action))
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => PhyChipChannelCompletion::FrequencySwitchStarted {
                frequency_index,
                crystal_selector,
            },
            PhyChipChannelAction::DelayMicros { phase, micros } => {
                PhyChipChannelCompletion::DelayElapsed { phase, micros }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { address, mask, .. } => {
                PhyChipChannelCompletion::FrequencyReadyObserved {
                    address,
                    value: if ready { mask } else { 0 },
                }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation { pass } => {
                PhyChipChannelCompletion::RxCompensationConfigured { pass }
            }
            PhyChipChannelAction::WriteI2c {
                phase,
                address,
                value,
            } => PhyChipChannelCompletion::I2cWriteCompleted {
                phase,
                address,
                value,
            },
            PhyChipChannelAction::CalculateTxGain(request) => {
                PhyChipChannelCompletion::TxGainCalculated {
                    request,
                    image: PhyWifiTxGainImage {
                        seed: [0; 6],
                        output_32: [0x20; 8],
                        output_64: [0x40; 16],
                        output_72: [0x48; 18],
                        config: 0,
                    },
                }
            }
            PhyChipChannelAction::PublishTxGain(_) => PhyChipChannelCompletion::TxGainPublished,
            PhyChipChannelAction::ReadI2c { phase, address } => {
                PhyChipChannelCompletion::I2cReadCompleted {
                    phase,
                    address,
                    value: 0xc5,
                }
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => PhyChipChannelCompletion::DcMemoryCleared,
            action => panic!("unexpected terminal channel action: {action:?}"),
        }
    }

    #[test]
    fn pure_channel_frequency_helpers_match_24ghz_reference_edges() {
        assert_eq!(channel_to_frequency(1), 2_412);
        assert_eq!(channel_to_frequency(11), 2_462);
        assert_eq!(channel_to_frequency(14), 2_484);
        assert_eq!(channel_to_frequency(2_462), 2_462);
        assert_eq!(frequency_to_channel(2_412), 1);
        assert_eq!(frequency_to_channel(2_462), 11);
        assert_eq!(frequency_to_channel(2_484), 14);
    }

    #[test]
    fn cold_channel_eleven_traverses_the_complete_ordered_graph() {
        let mut transition = PhyChipChannelTransition::new(REQUEST);
        let mut actions = 0;
        let mut ready_samples = 0;
        let mut rx_comp_passes = [false; 2];
        let mut saw_gain_image = false;

        loop {
            actions += 1;
            assert!(actions < 80);
            let action = transition.action();
            match action {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. } => {
                    assert_eq!(samples, ready_samples);
                    ready_samples += 1;
                }
                PhyChipChannelAction::ConfigureRxCompensation { pass } => {
                    rx_comp_passes[pass as usize] = true;
                }
                PhyChipChannelAction::PublishTxGain(image) => {
                    saw_gain_image = true;
                    assert_eq!(image.seed, PARAMETERS.tx_gain_seed);
                    assert_eq!(image.config, PARAMETERS.tx_gain_config);
                }
                PhyChipChannelAction::Complete(outcome) => {
                    assert_eq!(outcome.channel, 11);
                    assert_eq!(outcome.frequency_mhz, 2_462);
                    assert_eq!(outcome.cbw, 0);
                    assert!(!outcome.init_complete);
                    break;
                }
                PhyChipChannelAction::Failed(failure) => {
                    panic!("channel transition failed: {failure:?}")
                }
                _ => {}
            }
            let completion = direct_completion(action, ready_samples == 3);
            transition.advance(completion).unwrap();
        }

        assert_eq!(ready_samples, 3);
        assert_eq!(rx_comp_passes, [true, true]);
        assert!(saw_gain_image);
    }

    #[test]
    fn frequency_timeout_runs_full_radio_cleanup() {
        let mut transition = PhyChipChannelTransition::new(REQUEST);
        loop {
            match transition.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples: 2, .. } => {
                    transition
                        .advance(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                        .unwrap();
                    break;
                }
                action => {
                    let completion = direct_completion(action, false);
                    transition.advance(completion).unwrap();
                }
            }
        }

        assert_eq!(
            transition.action(),
            PhyChipChannelAction::SetBbpllCalibration { enabled: false }
        );
        transition
            .advance(PhyChipChannelCompletion::BbpllCalibrationSet { enabled: false })
            .unwrap();
        assert_eq!(transition.action(), PhyChipChannelAction::ClearDcMemory);
        transition
            .advance(PhyChipChannelCompletion::DcMemoryCleared)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyChipChannelAction::SetAgc { enabled: true }
        );
        transition
            .advance(PhyChipChannelCompletion::AgcSet { enabled: true })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyChipChannelAction::Failed(PhyChipChannelFailure::FrequencyReadyTimedOut {
                samples: 2
            })
        );
    }

    #[test]
    fn unsupported_profile_fails_before_touching_radio() {
        let mut request = REQUEST;
        request.parameters.channel_14_mic_enabled = true;
        assert_eq!(
            PhyChipChannelTransition::new(request).action(),
            PhyChipChannelAction::Failed(PhyChipChannelFailure::Channel14MicEnabled)
        );

        request.parameters.channel_14_mic_enabled = false;
        request.channel_or_frequency = 14;
        assert_eq!(
            PhyChipChannelTransition::new(request).action(),
            PhyChipChannelAction::Failed(PhyChipChannelFailure::UnsupportedChannel(14))
        );
    }

    #[test]
    fn direct_binding_rejects_timer_i2c_and_terminal_actions() {
        assert_eq!(
            PhyChipChannelMmioBinding::new(PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencySettle,
                micros: 10,
            }),
            Err(PhyChipChannelBindingError::NotDirectMmio)
        );
        assert_eq!(
            PhyChipChannelMmioBinding::new(PhyChipChannelAction::WriteI2c {
                phase: PhyChipChannelI2cPhase::ProgramTxCap,
                address: TX_CAP_ADDRESS,
                value: 0xc1,
            }),
            Err(PhyChipChannelBindingError::NotDirectMmio)
        );
    }

    #[test]
    fn external_lowering_covers_each_channel_operation_class() {
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::SetAgc { enabled: false }),
            Ok(PhyChipChannelExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Temperature(
                PhyTemperatureTransition::new().action()
            )),
            Ok(PhyChipChannelExternalBinding::Temperature(_))
        ));
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencySettle,
                micros: 10,
            }),
            Ok(PhyChipChannelExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::WriteI2c {
                phase: PhyChipChannelI2cPhase::ProgramTxCap,
                address: TX_CAP_ADDRESS,
                value: 0xc1,
            }),
            Ok(PhyChipChannelExternalBinding::I2c(_))
        ));
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::CalculateTxGain(
                PhyWifiTxGainRequest {
                    channel: 1,
                    calibration_curve: [0; 6],
                    correction: 0,
                    base_and_delta: 0,
                }
            )),
            Ok(PhyChipChannelExternalBinding::TxGain(_))
        ));
        assert!(matches!(
            PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Failed(
                PhyChipChannelFailure::UnsupportedChannel(14)
            )),
            Err(PhyChipChannelExternalBindingError::UnsupportedAction)
        ));
    }
}
