//! Rust-owned slices of the ESP32-S31 baseband cold initializer.
//!
//! The pinned parent is `libphy.a[phy_init.o]::phy_bb_init`, size `0x16a`.
//! This module is intentionally built from independently completed child
//! transitions. It must not grow a generic "call vendor calibration" action:
//! every child becomes either pure Rust state or an explicit I2C, MMIO,
//! observation, timer, or interrupt edge before the parent can use it.
//!
//! The first complete child is
//! `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem`, size `0x76`, called by the
//! parent with exactly 32 entries. The reference reads the high byte of
//! `0x2010_0408` once, then performs four finite MMIO accesses per entry. It
//! has no callback, allocation, hidden software state, wait, delay, or
//! hardware-dependent exit.

pub const PHY_TX_CFR_ENTRY_COUNT: u8 = 32;
pub const PHY_TX_CFR_INDEX_SOURCE_ADDRESS: usize = 0x2010_0408;

const PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT: u8 = 10;
const PHY_TX_CFR_DATA_PREFIX_VALUE: u32 = 0x0000_0e13;
const PHY_TX_CFR_INDEX_FIELD_MASK: u32 = 0x0007_f800;
const PHY_TX_CFR_INDEX_FIELD_SHIFT: u8 = 11;
const PHY_GAIN_MEMORY_CONTROL_RETAIN_MASK: u32 = 0xfff0_0000;
const PHY_GAIN_MEMORY_WRITE_BIT: u32 = 0x0008_0000;
const PHY_GAIN_MEMORY_INDEX_SHIFT: u8 = 11;
pub const PHY_RX_TABLE_ENTRY_COUNT: u8 = 0x4f;

/// Exact four-word input of complete ROM leaf `phy_write_gain_mem`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyGainMemoryEntry {
    pub word0: u32,
    pub word1: u32,
    pub word2: u32,
    pub index: u8,
}

/// The two explicit `phy_param` bytes consumed by ROM `phy_reg_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterInitParameters {
    pub parameter_121: u8,
    pub parameter_120: u8,
}

/// Explicit inputs captured before the Rust owner publishes the RX table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxTableInitParameters {
    pub parameter_002: u8,
    pub parameter_121: u8,
}

/// Reproduce one of the 79 fixed-form `phy_rx_table_init` entries.
pub const fn phy_rx_table_gain_entry(
    parameters: PhyRxTableInitParameters,
    index: u8,
) -> PhyGainMemoryEntry {
    PhyGainMemoryEntry {
        word0: 0x4020_0000,
        word1: 0x0201_0080 | ((parameters.parameter_002 as u32) << 29),
        word2: ((parameters.parameter_002 >> 6) as u32) | 0x0000_00fc,
        index,
    }
}

/// Reproduce the final control-register value of `phy_write_gain_mem`.
pub const fn phy_gain_memory_control_word(current: u32, entry: PhyGainMemoryEntry) -> u32 {
    (current & PHY_GAIN_MEMORY_CONTROL_RETAIN_MASK)
        | ((entry.index as u32) << PHY_GAIN_MEMORY_INDEX_SHIFT)
        | PHY_GAIN_MEMORY_WRITE_BIT
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrEntry {
    pub index: u8,
    pub start_index: u8,
    pub data: u32,
}

impl PhyTxCfrEntry {
    pub const fn memory_index(self) -> u8 {
        self.start_index.wrapping_add(self.index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrOutcome {
    pub entries_written: u8,
    pub start_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrAction {
    ReadStartIndex { address: usize },
    ProgramEntry(PhyTxCfrEntry),
    Complete(PhyTxCfrOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrCompletion {
    StartIndexRead { address: usize, register_value: u32 },
    EntryProgrammed(PhyTxCfrEntry),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxCfrStep {
    ReadStartIndex,
    Entries { start_index: u8, index: u8 },
    Complete(PhyTxCfrOutcome),
}

/// Caller-driven replacement for the exact `phy_set_tx_cfr_mem(32)` child.
///
/// One call to [`Self::advance`] consumes one externally supplied completion.
/// The transition never samples hardware itself and never asks to be polled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrTransition {
    step: PhyTxCfrStep,
}

impl PhyTxCfrTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyTxCfrStep::ReadStartIndex,
        }
    }

    pub const fn action(self) -> PhyTxCfrAction {
        match self.step {
            PhyTxCfrStep::ReadStartIndex => PhyTxCfrAction::ReadStartIndex {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
            },
            PhyTxCfrStep::Entries { start_index, index } => {
                PhyTxCfrAction::ProgramEntry(PhyTxCfrEntry {
                    index,
                    start_index,
                    data: if index < PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT {
                        PHY_TX_CFR_DATA_PREFIX_VALUE
                    } else {
                        0
                    },
                })
            }
            PhyTxCfrStep::Complete(outcome) => PhyTxCfrAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCfrCompletion,
    ) -> Result<(), PhyTxCfrTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTxCfrStep::ReadStartIndex,
                PhyTxCfrCompletion::StartIndexRead {
                    address,
                    register_value,
                },
            ) if address == PHY_TX_CFR_INDEX_SOURCE_ADDRESS => PhyTxCfrStep::Entries {
                start_index: (register_value >> 24) as u8,
                index: 0,
            },
            (
                PhyTxCfrStep::Entries { start_index, index },
                PhyTxCfrCompletion::EntryProgrammed(completed),
            ) if completed
                == (PhyTxCfrEntry {
                    index,
                    start_index,
                    data: if index < PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT {
                        PHY_TX_CFR_DATA_PREFIX_VALUE
                    } else {
                        0
                    },
                }) =>
            {
                let next = index + 1;
                if next == PHY_TX_CFR_ENTRY_COUNT {
                    PhyTxCfrStep::Complete(PhyTxCfrOutcome {
                        entries_written: next,
                        start_index,
                    })
                } else {
                    PhyTxCfrStep::Entries {
                        start_index,
                        index: next,
                    }
                }
            }
            (PhyTxCfrStep::Complete(_), _) => return Err(PhyTxCfrTransitionError::AlreadyComplete),
            _ => return Err(PhyTxCfrTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyTxCfrTransition {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact control word written before the vendor commit-bit pulse.
pub const fn phy_tx_cfr_control_word(current: u32, entry: PhyTxCfrEntry) -> u32 {
    (current & !PHY_TX_CFR_INDEX_FIELD_MASK)
        | ((entry.memory_index() as u32) << PHY_TX_CFR_INDEX_FIELD_SHIFT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrBindingError {
    TerminalAction,
}

/// A non-cloneable identity token for one finite TX-CFR MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCfrMmioBinding {
    action: PhyTxCfrAction,
}

impl PhyTxCfrMmioBinding {
    pub fn new(action: PhyTxCfrAction) -> Result<Self, PhyTxCfrBindingError> {
        match action {
            PhyTxCfrAction::ReadStartIndex { .. } | PhyTxCfrAction::ProgramEntry(_) => {
                Ok(Self { action })
            }
            PhyTxCfrAction::Complete(_) => Err(PhyTxCfrBindingError::TerminalAction),
        }
    }

    pub const fn action(&self) -> PhyTxCfrAction {
        self.action
    }

    /// Execute exactly one finite target transaction and consume its token.
    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyTxCfrCompletion {
        match self.action {
            PhyTxCfrAction::ReadStartIndex { address } => PhyTxCfrCompletion::StartIndexRead {
                address,
                register_value: (address as *const u32).read_volatile(),
            },
            PhyTxCfrAction::ProgramEntry(entry) => {
                crate::radio_hal::program_phy_tx_cfr_entry(entry);
                PhyTxCfrCompletion::EntryProgrammed(entry)
            }
            PhyTxCfrAction::Complete(_) => unreachable!(),
        }
    }
}

/// Complete parent or child operations already proven to be finite MMIO.
///
/// This enum deliberately excludes every still-unported calibration child.
/// Adding a variant requires the complete body and all of its callees to have
/// been recovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbBasebandMode {
    Idle,
    Calibration,
}

impl PhyBbBasebandMode {
    const fn register_value(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Calibration => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfRxSaturationPhase {
    PrepareCheck,
    Finalize,
}

impl PhyRfRxSaturationPhase {
    const fn enabled(self) -> bool {
        match self {
            Self::PrepareCheck => false,
            Self::Finalize => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbMmioAction {
    EnableBasebandInitialization,
    SetBasebandMode {
        mode: PhyBbBasebandMode,
    },
    UpdateAgcRegisters,
    UpdatePostInitRegisters,
    EnableAgc,
    SetWifiEnabled {
        enabled: bool,
    },
    ConfigureTxPowerTracking {
        enabled: bool,
    },
    ConfigureRfRxSaturation {
        phase: PhyRfRxSaturationPhase,
    },
    ConfigureI2cTxRate,
    ProgramGainMemory(PhyGainMemoryEntry),
    EnableIqCorrection,
    SetWifiAgcSaturationGain {
        value: u32,
    },
    ConfigureBasebandWatchdog,
    EnableMacBaseband,
    ConfigureNoiseFloorAuto,
    ConfigureAntenna,
    ConfigureBtFilter,
    ConfigurePhyRegisters {
        parameters: PhyRegisterInitParameters,
    },
    ConfigureRxTable {
        parameters: PhyRxTableInitParameters,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBbMmioCompletion {
    pub action: PhyBbMmioAction,
}

/// A non-cloneable token for one recovered baseband MMIO transaction.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyBbMmioBinding {
    action: PhyBbMmioAction,
}

impl PhyBbMmioBinding {
    pub const fn new(action: PhyBbMmioAction) -> Self {
        Self { action }
    }

    pub const fn action(&self) -> PhyBbMmioAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyBbMmioCompletion {
        match self.action {
            PhyBbMmioAction::EnableBasebandInitialization => {
                crate::radio_hal::enable_phy_baseband_initialization()
            }
            PhyBbMmioAction::SetBasebandMode { mode } => {
                crate::radio_hal::set_phy_baseband_mode(mode.register_value())
            }
            PhyBbMmioAction::UpdateAgcRegisters => {
                crate::radio_hal::configure_phy_bb_agc_register_update()
            }
            PhyBbMmioAction::UpdatePostInitRegisters => {
                crate::radio_hal::wifi_strict_phy_reg_update_new()
            }
            PhyBbMmioAction::EnableAgc => crate::radio_hal::enable_phy_agc(),
            PhyBbMmioAction::SetWifiEnabled { enabled } => {
                crate::radio_hal::set_phy_wifi_enabled(enabled)
            }
            PhyBbMmioAction::ConfigureTxPowerTracking { enabled } => {
                crate::radio_hal::configure_phy_bb_tx_power_tracking(enabled)
            }
            PhyBbMmioAction::ConfigureRfRxSaturation { phase } => {
                crate::radio_hal::configure_phy_rf_rx_saturation(phase.enabled())
            }
            PhyBbMmioAction::ConfigureI2cTxRate => crate::radio_hal::configure_phy_i2c_tx_rate(),
            PhyBbMmioAction::ProgramGainMemory(entry) => {
                crate::radio_hal::program_phy_gain_memory_entry(entry)
            }
            PhyBbMmioAction::EnableIqCorrection => crate::radio_hal::enable_phy_iq_correction(),
            PhyBbMmioAction::SetWifiAgcSaturationGain { value } => {
                crate::radio_hal::set_phy_wifi_agc_saturation_gain(value)
            }
            PhyBbMmioAction::ConfigureBasebandWatchdog => {
                crate::radio_hal::configure_phy_baseband_watchdog()
            }
            PhyBbMmioAction::EnableMacBaseband => crate::radio_hal::enable_phy_mac_baseband(),
            PhyBbMmioAction::ConfigureNoiseFloorAuto => {
                crate::radio_hal::configure_phy_noise_floor_auto()
            }
            PhyBbMmioAction::ConfigureAntenna => crate::radio_hal::configure_phy_antenna(),
            PhyBbMmioAction::ConfigureBtFilter => crate::radio_hal::configure_phy_bt_filter(),
            PhyBbMmioAction::ConfigurePhyRegisters { parameters } => {
                crate::radio_hal::configure_phy_registers(parameters)
            }
            PhyBbMmioAction::ConfigureRxTable { parameters } => {
                crate::radio_hal::configure_phy_rx_table(parameters)
            }
        }
        PhyBbMmioCompletion {
            action: self.action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        phy_gain_memory_control_word, phy_tx_cfr_control_word, PhyBbBasebandMode, PhyBbMmioAction,
        PhyBbMmioBinding, PhyGainMemoryEntry, PhyRegisterInitParameters, PhyRfRxSaturationPhase,
        PhyRxTableInitParameters, PhyTxCfrAction, PhyTxCfrBindingError, PhyTxCfrCompletion,
        PhyTxCfrEntry, PhyTxCfrMmioBinding, PhyTxCfrOutcome, PhyTxCfrTransition,
        PhyTxCfrTransitionError, PHY_TX_CFR_ENTRY_COUNT, PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
    };

    #[test]
    fn transition_reproduces_all_32_reference_entries() {
        let mut transition = PhyTxCfrTransition::new();
        assert_eq!(
            transition.action(),
            PhyTxCfrAction::ReadStartIndex {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
            }
        );
        transition
            .advance(PhyTxCfrCompletion::StartIndexRead {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
                register_value: 0xfa00_0000,
            })
            .unwrap();

        for index in 0..PHY_TX_CFR_ENTRY_COUNT {
            let expected = PhyTxCfrEntry {
                index,
                start_index: 0xfa,
                data: if index < 10 { 0xe13 } else { 0 },
            };
            assert_eq!(transition.action(), PhyTxCfrAction::ProgramEntry(expected));
            transition
                .advance(PhyTxCfrCompletion::EntryProgrammed(expected))
                .unwrap();
        }

        assert_eq!(
            transition.action(),
            PhyTxCfrAction::Complete(PhyTxCfrOutcome {
                entries_written: 32,
                start_index: 0xfa,
            })
        );
    }

    #[test]
    fn memory_index_preserves_reference_byte_wrapping() {
        assert_eq!(
            PhyTxCfrEntry {
                index: 31,
                start_index: 0xfa,
                data: 0,
            }
            .memory_index(),
            0x19
        );
    }

    #[test]
    fn control_word_replaces_only_the_index_field() {
        let current = 0xa5a5_5a5a;
        let entry = PhyTxCfrEntry {
            index: 2,
            start_index: 0x40,
            data: 0xe13,
        };
        assert_eq!(
            phy_tx_cfr_control_word(current, entry),
            (current & !0x0007_f800) | (0x42 << 11)
        );
    }

    #[test]
    fn transition_rejects_foreign_or_late_completions() {
        let mut transition = PhyTxCfrTransition::new();
        assert_eq!(
            transition.advance(PhyTxCfrCompletion::StartIndexRead {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS + 4,
                register_value: 0,
            }),
            Err(PhyTxCfrTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyTxCfrCompletion::StartIndexRead {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
                register_value: 0x1200_0000,
            })
            .unwrap();
        assert_eq!(
            transition.advance(PhyTxCfrCompletion::EntryProgrammed(PhyTxCfrEntry {
                index: 1,
                start_index: 0x12,
                data: 0xe13,
            })),
            Err(PhyTxCfrTransitionError::WrongCompletion)
        );

        for index in 0..PHY_TX_CFR_ENTRY_COUNT {
            transition
                .advance(PhyTxCfrCompletion::EntryProgrammed(PhyTxCfrEntry {
                    index,
                    start_index: 0x12,
                    data: if index < 10 { 0xe13 } else { 0 },
                }))
                .unwrap();
        }
        assert_eq!(
            transition.advance(PhyTxCfrCompletion::StartIndexRead {
                address: PHY_TX_CFR_INDEX_SOURCE_ADDRESS,
                register_value: 0,
            }),
            Err(PhyTxCfrTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn binding_rejects_terminal_action_and_preserves_identity() {
        let entry = PhyTxCfrEntry {
            index: 7,
            start_index: 3,
            data: 0xe13,
        };
        let binding = PhyTxCfrMmioBinding::new(PhyTxCfrAction::ProgramEntry(entry)).unwrap();
        assert_eq!(binding.action(), PhyTxCfrAction::ProgramEntry(entry));
        assert_eq!(
            PhyTxCfrMmioBinding::new(PhyTxCfrAction::Complete(PhyTxCfrOutcome {
                entries_written: 32,
                start_index: 3,
            })),
            Err(PhyTxCfrBindingError::TerminalAction)
        );
    }

    #[test]
    fn finite_baseband_mmio_binding_preserves_dynamic_identity() {
        for action in [
            PhyBbMmioAction::EnableBasebandInitialization,
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Calibration,
            },
            PhyBbMmioAction::UpdateAgcRegisters,
            PhyBbMmioAction::UpdatePostInitRegisters,
            PhyBbMmioAction::EnableAgc,
            PhyBbMmioAction::SetWifiEnabled { enabled: false },
            PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true },
            PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::PrepareCheck,
            },
            PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::Finalize,
            },
            PhyBbMmioAction::ConfigureI2cTxRate,
            PhyBbMmioAction::ProgramGainMemory(PhyGainMemoryEntry {
                word0: 1,
                word1: 2,
                word2: 3,
                index: 4,
            }),
            PhyBbMmioAction::EnableIqCorrection,
            PhyBbMmioAction::SetWifiAgcSaturationGain { value: 0x0008_1825 },
            PhyBbMmioAction::ConfigureBasebandWatchdog,
            PhyBbMmioAction::EnableMacBaseband,
            PhyBbMmioAction::ConfigureNoiseFloorAuto,
            PhyBbMmioAction::ConfigureAntenna,
            PhyBbMmioAction::ConfigureBtFilter,
            PhyBbMmioAction::ConfigurePhyRegisters {
                parameters: PhyRegisterInitParameters {
                    parameter_121: 0x4f,
                    parameter_120: 0x4e,
                },
            },
            PhyBbMmioAction::ConfigureRxTable {
                parameters: PhyRxTableInitParameters {
                    parameter_002: 0xa5,
                    parameter_121: 0x4e,
                },
            },
        ] {
            assert_eq!(PhyBbMmioBinding::new(action).action(), action);
        }
    }

    #[test]
    fn gain_memory_control_word_matches_the_complete_rom_leaf() {
        assert_eq!(
            phy_gain_memory_control_word(
                0xabc5_4321,
                PhyGainMemoryEntry {
                    word0: 0x1111_1111,
                    word1: 0x2222_2222,
                    word2: 0x3333_3333,
                    index: 0x12,
                },
            ),
            0xabc8_9000
        );
    }

    #[test]
    fn rx_table_entry_transform_matches_both_parameter_extremes() {
        assert_eq!(
            super::phy_rx_table_gain_entry(
                PhyRxTableInitParameters {
                    parameter_002: 0,
                    parameter_121: 0x4e,
                },
                0,
            ),
            PhyGainMemoryEntry {
                word0: 0x4020_0000,
                word1: 0x0201_0080,
                word2: 0x0000_00fc,
                index: 0,
            }
        );
        assert_eq!(
            super::phy_rx_table_gain_entry(
                PhyRxTableInitParameters {
                    parameter_002: u8::MAX,
                    parameter_121: 0x4e,
                },
                0x4e,
            ),
            PhyGainMemoryEntry {
                word0: 0x4020_0000,
                word1: 0xe201_0080,
                word2: 0x0000_00ff,
                index: 0x4e,
            }
        );
    }
}
