//! Event-driven ESP32-S31 RFPLL frequency programming.
//!
//! The primary reference is the complete rev0 ROM graph rooted at
//! `phy_set_rf_freq_offset` (`0x2f82_5c10`). The graph includes
//! `phy_set_rfpll_freq`, `phy_rfpll_set_freq`, `phy_write_rfpll_sdm`,
//! `phy_restart_cal`, `phy_wait_rfpll_cal_end`, `phy_read_pll_cap`,
//! `phy_write_pll_cap`, and `phy_rfpll_cap_init_cal`.
//!
//! ROM busy-waits through synchronous PHY-I2C calls, delays inside two loops,
//! prints after a missed lock deadline, and has a hardware-dependent
//! capacitor-search path which can fail to reach its equality bound. Rust
//! exposes every I2C transaction and timer interval as an identity-bound
//! external completion. A missed lock remains ordinary outcome data; the
//! non-terminating ROM search condition becomes a typed failure.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the ESP32-S31 body is one
/// `ret` and does not touch shared RFPLL state.
#[inline]
pub const fn phy_bbpll_en_usb() {}

/// Return the exact RF-calibration data version used by the pinned archive.
#[inline]
pub const fn phy_get_rf_cal_version() -> u32 {
    100
}

use crate::phy_i2c::{PhyI2cAddress, analog_registers};

const RFPLL_BLOCK: u8 = 0x62;
const SDM_BLOCK: u8 = 0x63;
const LOCK_ATTEMPTS: u8 = 100;
const CAP_SEARCH_LIMIT: u8 = 10;
const WIFI_CHANNEL_MAX: u16 = 14;

const fn address(block: u8, register: u8) -> PhyI2cAddress {
    PhyI2cAddress::new_internal(block, register)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllSdmImage {
    bytes: [u8; 5],
}

impl RfpllSdmImage {
    pub const fn bytes(self) -> [u8; 5] {
        self.bytes
    }
}

/// Exact stateless body of rev0 ROM `phy_rfpll_set_freq`.
pub const fn calculate_rfpll_sdm(
    frequency_code: u16,
    crystal_selector: u8,
    offset: u8,
) -> RfpllSdmImage {
    let selector_index = crystal_selector.wrapping_sub(1);
    let divisor = match selector_index {
        0 => 0x1a_u64,
        1 => 0x20_u64,
        2 => 0x30_u64,
        _ => 0x28_u64,
    };

    let scaled_offset = ((offset as u32).wrapping_shl(18) as i32 / 1_000) as u32;
    let scaled_frequency = (frequency_code as u32).wrapping_shl(18);
    let scaled = scaled_offset.wrapping_add(scaled_frequency);
    let (shift, first_divisor) = if frequency_code > 4_000 {
        (5_u32, 27_u64)
    } else {
        (3_u32, 3_u64)
    };
    let encoded =
        ((((scaled as u64) << shift) / first_divisor / divisor) as u32).wrapping_add(0xff00_0000);

    RfpllSdmImage {
        bytes: [
            (encoded & 0x7) as u8,
            (encoded >> 3) as u8,
            (encoded >> 11) as u8,
            (encoded >> 19) as u8,
            ((encoded >> 27) & 1) as u8,
        ],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyRequest {
    pub crystal_selector: u8,
    pub frequency_code: u16,
    pub offset: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyOutcome {
    pub sdm: RfpllSdmImage,
    pub lock_observed: bool,
    pub initial_cap: u16,
    pub final_cap: u16,
    pub accepted_cap_samples: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyFailure {
    /// Defensive terminal retained for callers which persist this public
    /// outcome. The bounded rev0 ROM search completes both ten-sample phases,
    /// including the no-accepted-sample case, so the exact transition does
    /// not normally emit it.
    CapacitorSearchDeadlineExceeded {
        initial_cap: u16,
        accepted_samples: u8,
        offset: u8,
    },
    FrequencyReadyDeadlineExceeded {
        samples: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyAction {
    StartChannelSwitch {
        frequency_index: u8,
        crystal_selector: u8,
    },
    ClearChannelSwitch,
    ReadChannelReady {
        samples: u32,
    },
    ConfigureNrx {
        frequency_mhz: u16,
    },
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
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    DelayMicros(u32),
    Complete(RfpllFrequencyOutcome),
    Failed(RfpllFrequencyFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyCompletion {
    ChannelSwitchStarted {
        frequency_index: u8,
        crystal_selector: u8,
    },
    ChannelSwitchCleared,
    ChannelReadyObserved {
        ready: bool,
    },
    ChannelReadyTimedOut,
    NrxConfigured {
        frequency_mhz: u16,
    },
    MaskedWrite {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    ByteWrite {
        address: PhyI2cAddress,
    },
    MaskedRead {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    ByteRead {
        address: PhyI2cAddress,
        value: u8,
    },
    DelayElapsed(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapSearchPhase {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CapSearchState {
    initial: u16,
    phase: CapSearchPhase,
    offset: u8,
    phase_attempts: u8,
    accepted: u8,
    sum: u16,
    lock_observed: bool,
}

impl CapSearchState {
    const fn candidate(self) -> u16 {
        match self.phase {
            CapSearchPhase::Down => self.initial.wrapping_sub(self.offset as u16),
            CapSearchPhase::Up => self
                .initial
                .wrapping_add(1)
                .wrapping_add(self.offset as u16),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapWriteContinuation {
    Search(CapSearchState),
    Final {
        initial: u16,
        final_cap: u16,
        accepted: u8,
        lock_observed: bool,
    },
}

impl CapWriteContinuation {
    const fn value(self) -> u16 {
        match self {
            Self::Search(search) => search.candidate(),
            Self::Final { final_cap, .. } => final_cap,
        }
    }

    const fn programmed_value(self) -> u16 {
        let value = self.value();
        if value & 0x8000 == 0 { value } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfpllFrequencyStep {
    ChannelStart,
    ChannelStartDelay,
    ChannelClear,
    ChannelSettleDelay,
    ChannelReady { samples: u32 },
    ChannelNrx,
    InitialWrite(u8),
    SdmWrite(u8),
    RestartWrite(u8),
    LockDelay { attempt: u8 },
    LockRead { attempt: u8 },
    CapLowRead { lock_observed: bool },
    CapHighRead { low: u8, lock_observed: bool },
    EnableCapSearch { initial: u16, lock_observed: bool },
    CapWriteLow(CapWriteContinuation),
    CapWriteHigh(CapWriteContinuation),
    CapDelay(CapWriteContinuation),
    CapStatusRead(CapSearchState),
    Complete(RfpllFrequencyOutcome),
    Failed(RfpllFrequencyFailure),
}

/// Heap-free, caller-driven replacement for the complete RFPLL frequency
/// programming graph used by crystal-duty calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyTransition {
    request: RfpllFrequencyRequest,
    sdm: RfpllSdmImage,
    step: RfpllFrequencyStep,
}

impl RfpllFrequencyTransition {
    const fn channel_frequency(channel: u16) -> u16 {
        if channel == 14 {
            2_484
        } else {
            2_407_u16.wrapping_add(channel.wrapping_mul(5))
        }
    }

    const fn programmed_frequency(request: RfpllFrequencyRequest) -> u16 {
        if request.frequency_code <= WIFI_CHANNEL_MAX {
            Self::channel_frequency(request.frequency_code)
        } else {
            request.frequency_code
        }
    }

    pub const fn new(request: RfpllFrequencyRequest) -> Self {
        Self {
            request,
            sdm: calculate_rfpll_sdm(
                Self::programmed_frequency(request),
                request.crystal_selector,
                request.offset,
            ),
            step: if request.frequency_code <= WIFI_CHANNEL_MAX {
                RfpllFrequencyStep::ChannelStart
            } else {
                RfpllFrequencyStep::InitialWrite(0)
            },
        }
    }

    const fn initial_write(index: u8) -> RfpllFrequencyAction {
        let (register, high_bit, low_bit, value) = match index {
            0 => (0x0b, 6, 6, 0),
            1 => (0x02, 7, 7, 1),
            _ => (0x02, 5, 0, 0x3f),
        };
        RfpllFrequencyAction::WriteMasked {
            address: address(RFPLL_BLOCK, register),
            high_bit,
            low_bit,
            value,
        }
    }

    const fn sdm_write(self, index: u8) -> RfpllFrequencyAction {
        let bytes = self.sdm.bytes;
        match index {
            0 => RfpllFrequencyAction::WriteMasked {
                address: address(SDM_BLOCK, 0),
                high_bit: 3,
                low_bit: 3,
                value: 0,
            },
            1 => RfpllFrequencyAction::WriteByte {
                address: address(SDM_BLOCK, 3),
                value: bytes[3],
            },
            2 => RfpllFrequencyAction::WriteByte {
                address: address(SDM_BLOCK, 4),
                value: bytes[2],
            },
            3 => RfpllFrequencyAction::WriteByte {
                address: address(SDM_BLOCK, 5),
                value: bytes[1],
            },
            4 => RfpllFrequencyAction::WriteMasked {
                address: analog_registers::RFPLL_SDM_LOW.address,
                high_bit: analog_registers::RFPLL_SDM_LOW.high_bit,
                low_bit: analog_registers::RFPLL_SDM_LOW.low_bit,
                value: bytes[0],
            },
            _ => RfpllFrequencyAction::WriteMasked {
                address: address(SDM_BLOCK, 0),
                high_bit: 3,
                low_bit: 3,
                value: 1,
            },
        }
    }

    const fn restart_write(index: u8) -> RfpllFrequencyAction {
        let (high_bit, value) = match index {
            0 => (6, 0),
            1 => (5, 0),
            2 => (5, 1),
            _ => (6, 1),
        };
        RfpllFrequencyAction::WriteMasked {
            address: address(RFPLL_BLOCK, 0),
            high_bit,
            low_bit: high_bit,
            value,
        }
    }

    pub const fn action(self) -> RfpllFrequencyAction {
        match self.step {
            RfpllFrequencyStep::ChannelStart => RfpllFrequencyAction::StartChannelSwitch {
                frequency_index: Self::programmed_frequency(self.request).wrapping_sub(2_400) as u8,
                crystal_selector: self.request.crystal_selector,
            },
            RfpllFrequencyStep::ChannelStartDelay => RfpllFrequencyAction::DelayMicros(1),
            RfpllFrequencyStep::ChannelClear => RfpllFrequencyAction::ClearChannelSwitch,
            RfpllFrequencyStep::ChannelSettleDelay => RfpllFrequencyAction::DelayMicros(10),
            RfpllFrequencyStep::ChannelReady { samples } => {
                RfpllFrequencyAction::ReadChannelReady { samples }
            }
            RfpllFrequencyStep::ChannelNrx => RfpllFrequencyAction::ConfigureNrx {
                frequency_mhz: Self::programmed_frequency(self.request),
            },
            RfpllFrequencyStep::InitialWrite(index) => Self::initial_write(index),
            RfpllFrequencyStep::SdmWrite(index) => self.sdm_write(index),
            RfpllFrequencyStep::RestartWrite(index) => Self::restart_write(index),
            RfpllFrequencyStep::LockDelay { .. } => RfpllFrequencyAction::DelayMicros(20),
            RfpllFrequencyStep::LockRead { .. } => RfpllFrequencyAction::ReadMasked {
                address: address(RFPLL_BLOCK, 7),
                high_bit: 1,
                low_bit: 1,
            },
            RfpllFrequencyStep::CapLowRead { .. } => RfpllFrequencyAction::ReadByte {
                address: address(RFPLL_BLOCK, 5),
            },
            RfpllFrequencyStep::CapHighRead { .. } => RfpllFrequencyAction::ReadMasked {
                address: address(RFPLL_BLOCK, 7),
                high_bit: 2,
                low_bit: 2,
            },
            RfpllFrequencyStep::EnableCapSearch { .. } => RfpllFrequencyAction::WriteMasked {
                address: address(RFPLL_BLOCK, 0x0b),
                high_bit: 6,
                low_bit: 6,
                value: 1,
            },
            RfpllFrequencyStep::CapWriteLow(continuation) => RfpllFrequencyAction::WriteByte {
                address: analog_registers::RFPLL_CAPACITOR_LOW,
                value: continuation.programmed_value() as u8,
            },
            RfpllFrequencyStep::CapWriteHigh(continuation) => RfpllFrequencyAction::WriteMasked {
                address: analog_registers::RFPLL_CAPACITOR_HIGH.address,
                high_bit: analog_registers::RFPLL_CAPACITOR_HIGH.high_bit,
                low_bit: analog_registers::RFPLL_CAPACITOR_HIGH.low_bit,
                value: (continuation.programmed_value() >> 8) as u8,
            },
            RfpllFrequencyStep::CapDelay(_) => RfpllFrequencyAction::DelayMicros(5),
            RfpllFrequencyStep::CapStatusRead(_) => RfpllFrequencyAction::ReadByte {
                address: address(RFPLL_BLOCK, 0x0c),
            },
            RfpllFrequencyStep::Complete(outcome) => RfpllFrequencyAction::Complete(outcome),
            RfpllFrequencyStep::Failed(failure) => RfpllFrequencyAction::Failed(failure),
        }
    }

    fn matches_write(action: RfpllFrequencyAction, completion: RfpllFrequencyCompletion) -> bool {
        match (action, completion) {
            (
                RfpllFrequencyAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    ..
                },
                RfpllFrequencyCompletion::MaskedWrite {
                    address: completed,
                    high_bit: completed_high,
                    low_bit: completed_low,
                },
            ) => address == completed && high_bit == completed_high && low_bit == completed_low,
            (
                RfpllFrequencyAction::WriteByte { address, .. },
                RfpllFrequencyCompletion::ByteWrite { address: completed },
            ) => address == completed,
            _ => false,
        }
    }

    fn finish_cap_phase(&mut self, search: CapSearchState) {
        if search.phase == CapSearchPhase::Down {
            self.step =
                RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(CapSearchState {
                    phase: CapSearchPhase::Up,
                    phase_attempts: 0,
                    ..search
                }));
        } else {
            let final_cap = if search.accepted == 0 {
                search.initial
            } else {
                search.sum / u16::from(search.accepted)
            };
            self.step = RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Final {
                initial: search.initial,
                final_cap,
                accepted: search.accepted,
                lock_observed: search.lock_observed,
            });
        }
    }

    pub fn advance(
        &mut self,
        completion: RfpllFrequencyCompletion,
    ) -> Result<(), RfpllFrequencyTransitionError> {
        let action = self.action();
        self.step = match (self.step, completion) {
            (
                RfpllFrequencyStep::ChannelStart,
                RfpllFrequencyCompletion::ChannelSwitchStarted {
                    frequency_index,
                    crystal_selector,
                },
            ) if frequency_index
                == Self::programmed_frequency(self.request).wrapping_sub(2_400) as u8
                && crystal_selector == self.request.crystal_selector =>
            {
                RfpllFrequencyStep::ChannelStartDelay
            }
            (RfpllFrequencyStep::ChannelStartDelay, RfpllFrequencyCompletion::DelayElapsed(1)) => {
                RfpllFrequencyStep::ChannelClear
            }
            (RfpllFrequencyStep::ChannelClear, RfpllFrequencyCompletion::ChannelSwitchCleared) => {
                RfpllFrequencyStep::ChannelSettleDelay
            }
            (
                RfpllFrequencyStep::ChannelSettleDelay,
                RfpllFrequencyCompletion::DelayElapsed(10),
            ) => RfpllFrequencyStep::ChannelReady { samples: 0 },
            (
                RfpllFrequencyStep::ChannelReady { samples },
                RfpllFrequencyCompletion::ChannelReadyObserved { ready },
            ) => {
                if ready {
                    RfpllFrequencyStep::ChannelNrx
                } else {
                    RfpllFrequencyStep::ChannelReady {
                        samples: samples.wrapping_add(1),
                    }
                }
            }
            (
                RfpllFrequencyStep::ChannelReady { samples },
                RfpllFrequencyCompletion::ChannelReadyTimedOut,
            ) => {
                RfpllFrequencyStep::Failed(RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
                    samples,
                })
            }
            (
                RfpllFrequencyStep::ChannelNrx,
                RfpllFrequencyCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == Self::programmed_frequency(self.request) => {
                RfpllFrequencyStep::Complete(RfpllFrequencyOutcome {
                    sdm: self.sdm,
                    lock_observed: true,
                    initial_cap: 0,
                    final_cap: 0,
                    accepted_cap_samples: 0,
                })
            }
            (RfpllFrequencyStep::InitialWrite(index), _)
                if Self::matches_write(action, completion) =>
            {
                if index == 2 {
                    RfpllFrequencyStep::SdmWrite(0)
                } else {
                    RfpllFrequencyStep::InitialWrite(index + 1)
                }
            }
            (RfpllFrequencyStep::SdmWrite(index), _) if Self::matches_write(action, completion) => {
                if index == 5 {
                    RfpllFrequencyStep::RestartWrite(0)
                } else {
                    RfpllFrequencyStep::SdmWrite(index + 1)
                }
            }
            (RfpllFrequencyStep::RestartWrite(index), _)
                if Self::matches_write(action, completion) =>
            {
                if index == 3 {
                    RfpllFrequencyStep::LockDelay { attempt: 0 }
                } else {
                    RfpllFrequencyStep::RestartWrite(index + 1)
                }
            }
            (
                RfpllFrequencyStep::LockDelay { attempt },
                RfpllFrequencyCompletion::DelayElapsed(20),
            ) => RfpllFrequencyStep::LockRead { attempt },
            (
                RfpllFrequencyStep::LockRead { attempt },
                RfpllFrequencyCompletion::MaskedRead {
                    address: completed,
                    high_bit: 1,
                    low_bit: 1,
                    value,
                },
            ) if completed == address(RFPLL_BLOCK, 7) => {
                if value != 0 {
                    RfpllFrequencyStep::CapLowRead {
                        lock_observed: true,
                    }
                } else if attempt + 1 == LOCK_ATTEMPTS {
                    RfpllFrequencyStep::CapLowRead {
                        lock_observed: false,
                    }
                } else {
                    RfpllFrequencyStep::LockDelay {
                        attempt: attempt + 1,
                    }
                }
            }
            (
                RfpllFrequencyStep::CapLowRead { lock_observed },
                RfpllFrequencyCompletion::ByteRead {
                    address: completed,
                    value,
                },
            ) if completed == address(RFPLL_BLOCK, 5) => RfpllFrequencyStep::CapHighRead {
                low: value,
                lock_observed,
            },
            (
                RfpllFrequencyStep::CapHighRead { low, lock_observed },
                RfpllFrequencyCompletion::MaskedRead {
                    address: completed,
                    high_bit: 2,
                    low_bit: 2,
                    value,
                },
            ) if completed == address(RFPLL_BLOCK, 7) => RfpllFrequencyStep::EnableCapSearch {
                initial: u16::from(low) | (u16::from(value) << 8),
                lock_observed,
            },
            (
                RfpllFrequencyStep::EnableCapSearch {
                    initial,
                    lock_observed,
                },
                _,
            ) if Self::matches_write(action, completion) => {
                RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(CapSearchState {
                    initial,
                    phase: CapSearchPhase::Down,
                    offset: 0,
                    phase_attempts: 0,
                    accepted: 0,
                    sum: 0,
                    lock_observed,
                }))
            }
            (RfpllFrequencyStep::CapWriteLow(continuation), _)
                if Self::matches_write(action, completion) =>
            {
                RfpllFrequencyStep::CapWriteHigh(continuation)
            }
            (RfpllFrequencyStep::CapWriteHigh(continuation), _)
                if Self::matches_write(action, completion) =>
            {
                RfpllFrequencyStep::CapDelay(continuation)
            }
            (
                RfpllFrequencyStep::CapDelay(continuation),
                RfpllFrequencyCompletion::DelayElapsed(5),
            ) => match continuation {
                CapWriteContinuation::Search(search) => RfpllFrequencyStep::CapStatusRead(search),
                CapWriteContinuation::Final {
                    initial,
                    final_cap,
                    accepted,
                    lock_observed,
                } => RfpllFrequencyStep::Complete(RfpllFrequencyOutcome {
                    sdm: self.sdm,
                    lock_observed,
                    initial_cap: initial,
                    final_cap,
                    accepted_cap_samples: accepted,
                }),
            },
            (
                RfpllFrequencyStep::CapStatusRead(mut search),
                RfpllFrequencyCompletion::ByteRead {
                    address: completed,
                    value,
                },
            ) if completed == address(RFPLL_BLOCK, 0x0c) => {
                let status = (value >> 2) & 0x3;
                if status == 0 {
                    search.sum = search.sum.wrapping_add(search.candidate());
                    search.accepted = search.accepted.wrapping_add(1);
                    search.offset = search.offset.wrapping_add(1);
                    search.phase_attempts = search.phase_attempts.wrapping_add(1);
                    if search.phase_attempts == CAP_SEARCH_LIMIT {
                        self.finish_cap_phase(search);
                    } else {
                        self.step =
                            RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(search));
                    }
                } else if search.accepted != 0 {
                    self.finish_cap_phase(search);
                } else {
                    search.offset = search.offset.wrapping_add(1);
                    search.phase_attempts = search.phase_attempts.wrapping_add(1);
                    if search.phase_attempts == CAP_SEARCH_LIMIT {
                        self.finish_cap_phase(search);
                    } else {
                        self.step =
                            RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(search));
                    }
                }
                return Ok(());
            }
            (RfpllFrequencyStep::Complete(_), _) | (RfpllFrequencyStep::Failed(_), _) => {
                return Err(RfpllFrequencyTransitionError::AlreadyComplete);
            }
            _ => return Err(RfpllFrequencyTransitionError::WrongCompletion),
        };
        Ok(())
    }

    pub const fn request(self) -> RfpllFrequencyRequest {
        self.request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable owner of one RFPLL PHY-I2C operation.
///
/// The complete outer action is retained until the transaction finishes so
/// two adjacent writes to the same byte cannot exchange completions.
#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyI2cBinding {
    outer_action: RfpllFrequencyAction,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl RfpllFrequencyI2cBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        let request = match action {
            RfpllFrequencyAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            } => {
                crate::phy_cold::PhyColdI2cRequest::write_masked(address, high_bit, low_bit, value)
                    .ok_or(RfpllFrequencyBindingError::UnsupportedAction)?
            }
            RfpllFrequencyAction::WriteByte { address, value } => {
                crate::phy_cold::PhyColdI2cRequest::write_byte(address, value)
            }
            RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => crate::phy_cold::PhyColdI2cRequest::read_masked(address, high_bit, low_bit)
                .ok_or(RfpllFrequencyBindingError::UnsupportedAction)?,
            RfpllFrequencyAction::ReadByte { address } => {
                crate::phy_cold::PhyColdI2cRequest::read_byte(address)
            }
            RfpllFrequencyAction::StartChannelSwitch { .. }
            | RfpllFrequencyAction::ClearChannelSwitch
            | RfpllFrequencyAction::ReadChannelReady { .. }
            | RfpllFrequencyAction::ConfigureNrx { .. }
            | RfpllFrequencyAction::DelayMicros(_)
            | RfpllFrequencyAction::Complete(_)
            | RfpllFrequencyAction::Failed(_) => {
                return Err(RfpllFrequencyBindingError::UnsupportedAction);
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
    pub fn start_target<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &mut P,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<RfpllFrequencyCompletion, RfpllFrequencyBindingError> {
        let crate::phy_cold::PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(RfpllFrequencyBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                RfpllFrequencyAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    ..
                },
                crate::phy_cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            }),
            (
                RfpllFrequencyAction::WriteByte { address, .. },
                crate::phy_cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(RfpllFrequencyCompletion::ByteWrite { address }),
            (
                RfpllFrequencyAction::ReadMasked {
                    address,
                    high_bit,
                    low_bit,
                },
                crate::phy_cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value,
            }),
            (
                RfpllFrequencyAction::ReadByte { address },
                crate::phy_cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(RfpllFrequencyCompletion::ByteRead { address, value }),
            _ => Err(RfpllFrequencyBindingError::UnexpectedOutcome),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyTimerBinding {
    micros: u32,
}

impl RfpllFrequencyTimerBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        match action {
            RfpllFrequencyAction::DelayMicros(micros) => Ok(Self { micros }),
            _ => Err(RfpllFrequencyBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> RfpllFrequencyCompletion {
        RfpllFrequencyCompletion::DelayElapsed(self.micros)
    }
}

/// Non-cloneable owner of one finite fast-channel MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyMmioBinding {
    action: RfpllFrequencyAction,
}

impl RfpllFrequencyMmioBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        match action {
            RfpllFrequencyAction::StartChannelSwitch { .. }
            | RfpllFrequencyAction::ClearChannelSwitch
            | RfpllFrequencyAction::ReadChannelReady { .. }
            | RfpllFrequencyAction::ConfigureNrx { .. } => Ok(Self { action }),
            _ => Err(RfpllFrequencyBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> RfpllFrequencyAction {
        self.action
    }

    /// Execute exactly one MMIO edge from the ROM fast-channel path.
    ///
    /// # Safety
    ///
    /// The caller must hold the unique radio-register owner and must not run
    /// another PHY frequency transition until the returned completion has
    /// been consumed.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> RfpllFrequencyCompletion {
        match self.action {
            RfpllFrequencyAction::StartChannelSwitch {
                frequency_index,
                crystal_selector,
            } => {
                open_esp_radio_esp32s31_hal::phy_frequency::start_channel_switch(
                    registers,
                    frequency_index,
                );
                RfpllFrequencyCompletion::ChannelSwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            RfpllFrequencyAction::ClearChannelSwitch => {
                open_esp_radio_esp32s31_hal::phy_frequency::clear_channel_switch(registers);
                RfpllFrequencyCompletion::ChannelSwitchCleared
            }
            RfpllFrequencyAction::ReadChannelReady { .. } => {
                RfpllFrequencyCompletion::ChannelReadyObserved {
                    ready: open_esp_radio_esp32s31_hal::phy_frequency::sample_frequency_ready(
                        registers,
                    ),
                }
            }
            RfpllFrequencyAction::ConfigureNrx { frequency_mhz } => {
                open_esp_radio_esp32s31_hal::phy_frequency::configure_nrx_frequency(
                    registers,
                    u32::from(frequency_mhz),
                );
                RfpllFrequencyCompletion::NrxConfigured { frequency_mhz }
            }
            _ => unreachable!(),
        }
    }
}

/// Exhaustive lowering for every non-terminal RFPLL frequency action.
#[derive(Debug, Eq, PartialEq)]
pub enum RfpllFrequencyExternalBinding {
    Mmio(RfpllFrequencyMmioBinding),
    I2c(RfpllFrequencyI2cBinding),
    Timer(RfpllFrequencyTimerBinding),
}

impl RfpllFrequencyExternalBinding {
    pub fn lower(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        if let Ok(binding) = RfpllFrequencyMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = RfpllFrequencyI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = RfpllFrequencyTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(RfpllFrequencyBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CAP_SEARCH_LIMIT, RFPLL_BLOCK, RfpllFrequencyAction, RfpllFrequencyBindingError,
        RfpllFrequencyCompletion, RfpllFrequencyExternalBinding, RfpllFrequencyI2cBinding,
        RfpllFrequencyOutcome, RfpllFrequencyRequest, RfpllFrequencyTransition, address,
        calculate_rfpll_sdm,
    };

    fn complete_write(action: RfpllFrequencyAction) -> RfpllFrequencyCompletion {
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
            action => panic!("expected write action, got {action:?}"),
        }
    }

    fn advance_writes(transition: &mut RfpllFrequencyTransition, count: usize) {
        let mut index = 0;
        while index != count {
            let completion = complete_write(transition.action());
            transition.advance(completion).unwrap();
            index += 1;
        }
    }

    fn enter_cap_search(transition: &mut RfpllFrequencyTransition, low: u8, high: u8) {
        advance_writes(transition, 13);
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(20))
            .unwrap();
        let RfpllFrequencyAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        } = transition.action()
        else {
            panic!("expected lock read");
        };
        transition
            .advance(RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: 1,
            })
            .unwrap();

        let RfpllFrequencyAction::ReadByte { address } = transition.action() else {
            panic!("expected cap low read");
        };
        transition
            .advance(RfpllFrequencyCompletion::ByteRead {
                address,
                value: low,
            })
            .unwrap();
        let RfpllFrequencyAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        } = transition.action()
        else {
            panic!("expected cap high read");
        };
        transition
            .advance(RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: high,
            })
            .unwrap();
        let completion = complete_write(transition.action());
        transition.advance(completion).unwrap();
    }

    fn complete_cap_candidate(transition: &mut RfpllFrequencyTransition, status: u8) {
        advance_writes(transition, 2);
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(5));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(5))
            .unwrap();
        let RfpllFrequencyAction::ReadByte { address } = transition.action() else {
            panic!("expected cap status read");
        };
        transition
            .advance(RfpllFrequencyCompletion::ByteRead {
                address,
                value: status << 2,
            })
            .unwrap();
    }

    #[test]
    fn sdm_image_matches_the_actual_xtal_duty_request() {
        assert_eq!(
            calculate_rfpll_sdm(0x983, 0x31, 0).bytes(),
            [0x05, 0xaa, 0x2a, 0x31, 0x00]
        );
        assert_eq!(
            calculate_rfpll_sdm(0x0fa1, 1, 7).bytes(),
            [0x01, 0xe8, 0x30, 0x3b, 0x00]
        );
    }

    #[test]
    fn lock_deadline_is_one_hundred_external_delay_and_read_edges() {
        let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: 0x31,
            frequency_code: 0x983,
            offset: 0,
        });
        advance_writes(&mut transition, 13);

        let mut attempts = 0;
        while attempts != 100 {
            assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
            transition
                .advance(RfpllFrequencyCompletion::DelayElapsed(20))
                .unwrap();
            let RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } = transition.action()
            else {
                panic!("expected lock read");
            };
            transition
                .advance(RfpllFrequencyCompletion::MaskedRead {
                    address,
                    high_bit,
                    low_bit,
                    value: 0,
                })
                .unwrap();
            attempts += 1;
        }
        assert!(matches!(
            transition.action(),
            RfpllFrequencyAction::ReadByte { .. }
        ));
    }

    #[test]
    fn capacitor_search_preserves_shared_offset_sum_and_first_match_order() {
        let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: 0x31,
            frequency_code: 0x983,
            offset: 0,
        });
        enter_cap_search(&mut transition, 100, 0);

        complete_cap_candidate(&mut transition, 0);
        complete_cap_candidate(&mut transition, 0);
        complete_cap_candidate(&mut transition, 1);
        complete_cap_candidate(&mut transition, 0);
        complete_cap_candidate(&mut transition, 1);

        advance_writes(&mut transition, 2);
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(5))
            .unwrap();
        let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
            panic!("expected completion");
        };
        assert_eq!(outcome.initial_cap, 100);
        assert_eq!(outcome.final_cap, (100 + 99 + 103) / 3);
        assert_eq!(outcome.accepted_cap_samples, 3);
        assert!(outcome.lock_observed);
    }

    #[test]
    fn bounded_rom_cap_path_preserves_initial_when_no_sample_is_accepted() {
        let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: 0x31,
            frequency_code: 0x983,
            offset: 0,
        });
        enter_cap_search(&mut transition, 100, 0);
        let mut index = 0;
        while index != CAP_SEARCH_LIMIT * 2 {
            complete_cap_candidate(&mut transition, 1);
            index += 1;
        }
        advance_writes(&mut transition, 2);
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(5))
            .unwrap();
        let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
            panic!("expected completion");
        };
        assert_eq!(outcome.initial_cap, 100);
        assert_eq!(outcome.final_cap, 100);
        assert_eq!(outcome.accepted_cap_samples, 0);
    }

    #[test]
    fn wifi_channel_uses_the_rom_fast_switch_without_rfpll_i2c() {
        let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: 0,
            frequency_code: 1,
            offset: 0,
        });
        assert_eq!(
            transition.action(),
            RfpllFrequencyAction::StartChannelSwitch {
                frequency_index: 12,
                crystal_selector: 0,
            }
        );
        transition
            .advance(RfpllFrequencyCompletion::ChannelSwitchStarted {
                frequency_index: 12,
                crystal_selector: 0,
            })
            .unwrap();
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(1));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(1))
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllFrequencyAction::ClearChannelSwitch
        );
        transition
            .advance(RfpllFrequencyCompletion::ChannelSwitchCleared)
            .unwrap();
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(10));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(10))
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllFrequencyAction::ReadChannelReady { samples: 0 }
        );
        transition
            .advance(RfpllFrequencyCompletion::ChannelReadyObserved { ready: true })
            .unwrap();
        assert_eq!(
            transition.action(),
            RfpllFrequencyAction::ConfigureNrx {
                frequency_mhz: 2_412,
            }
        );
        transition
            .advance(RfpllFrequencyCompletion::NrxConfigured {
                frequency_mhz: 2_412,
            })
            .unwrap();
        let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
            panic!("expected fast-channel completion");
        };
        assert!(outcome.lock_observed);
        assert_eq!(outcome.accepted_cap_samples, 0);
    }

    #[test]
    fn external_lowering_covers_rfpll_mmio_i2c_and_timer_actions() {
        assert!(matches!(
            RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::StartChannelSwitch {
                frequency_index: 12,
                crystal_selector: 0,
            }),
            Ok(RfpllFrequencyExternalBinding::Mmio(_))
        ));
        let register = address(RFPLL_BLOCK, 7);
        assert!(matches!(
            RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::ReadMasked {
                address: register,
                high_bit: 1,
                low_bit: 1,
            }),
            Ok(RfpllFrequencyExternalBinding::I2c(_))
        ));
        assert!(matches!(
            RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::WriteByte {
                address: register,
                value: 3,
            }),
            Ok(RfpllFrequencyExternalBinding::I2c(_))
        ));
        let timer =
            RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::DelayMicros(20)).unwrap();
        let RfpllFrequencyExternalBinding::Timer(timer) = timer else {
            panic!("expected timer");
        };
        assert_eq!(timer.micros(), 20);
        assert_eq!(
            timer.into_completion(),
            RfpllFrequencyCompletion::DelayElapsed(20)
        );
        assert!(matches!(
            RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::Complete(
                RfpllFrequencyOutcome {
                    sdm: calculate_rfpll_sdm(0x983, 0x31, 0),
                    lock_observed: true,
                    initial_cap: 1,
                    final_cap: 1,
                    accepted_cap_samples: 1,
                }
            )),
            Err(RfpllFrequencyBindingError::UnsupportedAction)
        ));
    }

    #[test]
    fn rfpll_i2c_binding_preserves_the_masked_read_identity() {
        let register = address(RFPLL_BLOCK, 7);
        let mut binding = RfpllFrequencyI2cBinding::new(RfpllFrequencyAction::ReadMasked {
            address: register,
            high_bit: 2,
            low_bit: 1,
        })
        .unwrap();
        binding.read_started().unwrap();
        assert_eq!(
            binding.observe_read_result(Ok(0b1010)).unwrap(),
            crate::phy_cold::PhyColdI2cObservation::EdgeConsumed
        );
        assert_eq!(
            binding.into_completion().unwrap(),
            RfpllFrequencyCompletion::MaskedRead {
                address: register,
                high_bit: 2,
                low_bit: 1,
                value: 1,
            }
        );
    }
}
