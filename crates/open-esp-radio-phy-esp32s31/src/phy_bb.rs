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
const PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT: u8 = 10;
const PHY_TX_CFR_DATA_PREFIX_VALUE: u32 = 0x0000_0e13;
pub const PHY_RX_TABLE_ENTRY_COUNT: u8 = 0x4f;
pub const PHY_WIFI_RX_GAIN_GENERATED_CAPACITY: usize = 0x55;

// Pinned `libphy.a[phy_rx_gain.o]` `.rodata` and stack initializers used by
// `phy_set_rx_gain_table(0x985, 0)`. Keep these byte-oriented: the ROM
// generator deliberately reads signed adjustment bytes from the latter two
// objects and aligned halfwords from the first two.
const PHY_RX_GAIN_BASE_BANK_0: [u16; 8] = [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f];
const PHY_RX_GAIN_BASE_BANK_1: [u16; 11] = [
    0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
];
const PHY_RX_GAIN_ADVANCE_BANK_0: [i8; 8] = [8, 8, 10, 8, 5, 7, 6, 0];
const PHY_RX_GAIN_ADVANCE_BANK_1: [i8; 11] = [6, 5, 5, 5, 7, 5, 7, 7, 5, 4, 0];
const PHY_RX_GAIN_THRESHOLD_BANK_0: [u8; 8] = [3, 5, 3, 9, 12, 12, 12, 12];
const PHY_RX_GAIN_THRESHOLD_BANK_1: [u8; 11] = [0; 11];
const PHY_RX_GAIN_LOW_FIELD: [u16; 7] = [0, 0x20, 0x30, 0x38, 0x3c, 0x3e, 0x3f];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainBank {
    Wifi,
    Shared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyGeneratedRxGainTable {
    pub words: [u32; PHY_WIFI_RX_GAIN_GENERATED_CAPACITY],
    /// Highest valid generated index, matching the return value of ROM
    /// `phy_gen_rx_gain_table`.
    pub last_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainMemoryParameters {
    pub parameter_002: u8,
    pub wifi_index_dc: [[u16; 2]; 8],
    pub wifi_dc_base: [u16; 2],
    pub shared_index_dc: [[u16; 2]; 11],
    pub rxbb_dc_adjustments: [[u16; 2]; 6],
    pub wifi_auxiliary: u16,
}

fn rfrx_gain_index(bank: PhyRxGainBank, encoded_gain: u16) -> usize {
    let bases = match bank {
        PhyRxGainBank::Wifi => &PHY_RX_GAIN_BASE_BANK_0[..],
        PhyRxGainBank::Shared => &PHY_RX_GAIN_BASE_BANK_1[..],
    };
    let mut index = 0;
    while index != bases.len() {
        if bases[index] == encoded_gain {
            return index;
        }
        index += 1;
    }
    bases.len()
}

const fn shared_rx_mixer_digital_gain(index: usize) -> u32 {
    match index {
        9 => 4,
        10 => 7,
        _ => 0,
    }
}

/// Reproduce one complete `phy_wr_rx_gain_mem_new` output record.
///
/// The two hardware-clock/PBus wrappers around the reference loop are
/// sequenced by its caller. This function contains the entire per-entry
/// arithmetic and every former `phy_param` dependency as an owned value.
pub fn phy_generated_rx_gain_memory_entry(
    parameters: PhyRxGainMemoryParameters,
    bank: PhyRxGainBank,
    table: &PhyGeneratedRxGainTable,
    index: u8,
) -> PhyGainMemoryEntry {
    let encoded = table.words[index as usize];
    let gain_index = rfrx_gain_index(bank, ((encoded >> 12) & 0xffff) as u16);
    let index_dc = match bank {
        PhyRxGainBank::Wifi => parameters.wifi_index_dc[gain_index],
        PhyRxGainBank::Shared => parameters.shared_index_dc[gain_index],
    };
    let mut selected_bits = 0_u8;
    let mut bit = 4_u8;
    while bit != 10 {
        selected_bits = selected_bits.wrapping_add(((encoded >> bit) & 1) as u8);
        bit += 1;
    }
    let adjustment = parameters.rxbb_dc_adjustments[if selected_bits > 5 {
        5
    } else {
        selected_bits as usize
    }];
    let dc_base = match bank {
        PhyRxGainBank::Wifi => parameters.wifi_dc_base,
        PhyRxGainBank::Shared => [0x100, 0x100],
    };
    let dc_i = dc_base[0].wrapping_add(adjustment[0]);
    let dc_q = dc_base[1].wrapping_add(adjustment[1]);
    let auxiliary = match bank {
        PhyRxGainBank::Wifi => parameters.wifi_auxiliary,
        PhyRxGainBank::Shared => 0,
    };
    let mixer_digital_gain = match bank {
        PhyRxGainBank::Wifi => 7,
        PhyRxGainBank::Shared => shared_rx_mixer_digital_gain(gain_index),
    };
    let memory_index = match bank {
        PhyRxGainBank::Wifi => index,
        PhyRxGainBank::Shared => index.wrapping_add(0x50),
    };
    PhyGainMemoryEntry {
        word0: (u32::from(dc_i) << 31)
            | (u32::from(dc_q) << 13)
            | (u32::from(index_dc[1]) << 22)
            | (u32::from(auxiliary) & 0x1fff),
        word1: (((encoded >> 4) & 0x7f) << 20)
            | ((encoded & 7) << 17)
            | (u32::from(index_dc[0]) << 8)
            | (u32::from(dc_i) >> 1)
            | (mixer_digital_gain << 29),
        word2: u32::from(parameters.parameter_002 >> 6)
            | (((encoded >> 15) & 7) << 5)
            | (((encoded >> 12) & 7) << 2),
        index: memory_index,
    }
}

/// Pure Rust replacement for complete ROM `phy_gen_rx_gain_table`, size 312.
///
/// The diagnostic argument of the ROM function only prints intermediate
/// values and is intentionally absent. The S31 cold parent uses fixed table
/// objects, so exposing the bank rather than five aliasable raw pointers also
/// makes every input and bound explicit.
pub fn generate_phy_rx_gain_table(bank: PhyRxGainBank) -> PhyGeneratedRxGainTable {
    let (maximum_gain, entry_count) = match bank {
        PhyRxGainBank::Wifi => (0x1c_u8, PHY_RX_GAIN_BASE_BANK_0.len()),
        PhyRxGainBank::Shared => (0x12_u8, PHY_RX_GAIN_BASE_BANK_1.len()),
    };
    let mut words = [0_u32; PHY_WIFI_RX_GAIN_GENERATED_CAPACITY];
    let mut gain = match bank {
        PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[0],
        PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[0],
    };
    let mut source_index = 0_usize;
    let mut output_index = 0_usize;
    loop {
        let advance = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_ADVANCE_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_ADVANCE_BANK_1[source_index],
        };
        let threshold = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[source_index],
        };
        if gain == threshold.wrapping_add_signed(advance) && source_index < entry_count - 1 {
            loop {
                source_index += 1;
                let next_advance = match bank {
                    PhyRxGainBank::Wifi => PHY_RX_GAIN_ADVANCE_BANK_0[source_index],
                    PhyRxGainBank::Shared => PHY_RX_GAIN_ADVANCE_BANK_1[source_index],
                };
                if next_advance != 0 || source_index >= entry_count - 1 {
                    break;
                }
            }
            gain = match bank {
                PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[source_index],
                PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[source_index],
            };
        }
        let base = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_BASE_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_BASE_BANK_1[source_index],
        };
        let low = PHY_RX_GAIN_LOW_FIELD[(gain / 6) as usize] & 0x0fff;
        words[output_index] =
            u32::from(base) * 0x1000 + u32::from(low) * 0x10 + u32::from(gain % 6);
        if maximum_gain < gain || output_index + 1 == PHY_WIFI_RX_GAIN_GENERATED_CAPACITY {
            return PhyGeneratedRxGainTable {
                words,
                last_index: output_index as u8,
            };
        }
        output_index += 1;
        gain = gain.wrapping_add(1);
    }
}

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
    ReadStartIndex,
    ProgramEntry(PhyTxCfrEntry),
    Complete(PhyTxCfrOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrCompletion {
    StartIndexRead { base_index: u8 },
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
            PhyTxCfrStep::ReadStartIndex => PhyTxCfrAction::ReadStartIndex,
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
            (PhyTxCfrStep::ReadStartIndex, PhyTxCfrCompletion::StartIndexRead { base_index }) => {
                PhyTxCfrStep::Entries {
                    start_index: base_index,
                    index: 0,
                }
            }
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
            PhyTxCfrAction::ReadStartIndex | PhyTxCfrAction::ProgramEntry(_) => Ok(Self { action }),
            PhyTxCfrAction::Complete(_) => Err(PhyTxCfrBindingError::TerminalAction),
        }
    }

    pub const fn action(&self) -> PhyTxCfrAction {
        self.action
    }

    /// Execute exactly one finite target transaction and consume its token.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyTxCfrCompletion {
        match self.action {
            PhyTxCfrAction::ReadStartIndex => PhyTxCfrCompletion::StartIndexRead {
                base_index: open_esp_radio_hal_esp32s31::phy_memory::read_table_memory_base_index(
                    registers,
                ),
            },
            PhyTxCfrAction::ProgramEntry(entry) => {
                open_esp_radio_hal_esp32s31::phy_memory::program_tx_cfr_entry(
                    registers,
                    entry.data,
                    entry.memory_index(),
                );
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
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyBbMmioCompletion {
        match self.action {
            PhyBbMmioAction::EnableBasebandInitialization => {
                crate::radio_hal::enable_phy_baseband_initialization()
            }
            PhyBbMmioAction::SetBasebandMode { mode } => {
                crate::radio_hal::set_phy_baseband_mode(mode.register_value())
            }
            PhyBbMmioAction::UpdateAgcRegisters => {
                crate::radio_hal::configure_phy_bb_agc_register_update(registers)
            }
            PhyBbMmioAction::UpdatePostInitRegisters => {
                crate::radio_hal::wifi_strict_phy_reg_update_new()
            }
            PhyBbMmioAction::EnableAgc => crate::radio_hal::enable_phy_agc(registers),
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
                open_esp_radio_hal_esp32s31::phy_memory::program_gain_memory_entry(
                    registers,
                    [entry.word0, entry.word1, entry.word2],
                    entry.index,
                )
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
                crate::radio_hal::configure_phy_registers(registers, parameters)
            }
            PhyBbMmioAction::ConfigureRxTable { parameters } => {
                crate::radio_hal::configure_phy_rx_table(registers, parameters)
            }
        }
        PhyBbMmioCompletion {
            action: self.action,
        }
    }
}

/// Successful completion of the complete Wi-Fi portion of
/// `libphy.a[phy_init.o]::phy_bb_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBbInitOutcome {
    pub calibration_performed: bool,
}

/// Terminal failures from the owned children of `phy_bb_init`.
///
/// `phy_bt_tx_gain_init` does not yet have a variant. Its complete relocation
/// graph reaches shared RFPLL, TXDC and PWDET calibration in addition to BT
/// gain publication, so callers must not treat the current omission as a
/// proved Wi-Fi-only optimization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitFailure {
    TxDc(crate::phy_txdc::PhyTxDcFailure),
    Pwdet(crate::phy_pwdet::PhyPwdetFailure),
    TxCap(crate::phy_tx_cal::PhyTxCapFailure),
    Temperature {
        pass: u8,
        failure: crate::phy_temperature::PhyTemperatureFailure,
    },
    TxPower(crate::phy_tx_power::PhyTxPowerFailure),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetFailure),
    Dcode(crate::phy_dcode::PhyDcodeFailure),
    TxIq(crate::phy_txiq::PhyTxIqInitFailure),
    RxIq(crate::phy_rxiq::PhyRxIqInitFailure),
    RxSaturation(crate::phy_rx_saturation::PhyRxSaturationOutcome),
    RxGain(crate::phy_rx_gain::PhyRxGainInitFailure),
    Channel(crate::phy_channel::PhyChipChannelFailure),
}

/// Exactly one externally completed operation emitted by the baseband
/// parent. No variant means "call a vendor child".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitAction {
    Mmio(PhyBbMmioAction),
    TxDc(crate::phy_txdc::PhyTxDcAction),
    Pwdet(crate::phy_pwdet::PhyPwdetAction),
    TxCap(crate::phy_tx_cal::PhyTxCapAction),
    Temperature(crate::phy_temperature::PhyTemperatureAction),
    TxPower(crate::phy_tx_power::PhyTxPowerAction),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetAction),
    Dcode(crate::phy_dcode::PhyDcodeAction),
    TxIq(crate::phy_txiq::PhyTxIqInitAction),
    TxCfr(PhyTxCfrAction),
    PbusMemory(crate::phy_pbus_memory::PhyPbusMemoryAction),
    RxIq(crate::phy_rxiq::PhyRxIqInitAction),
    RxSaturation(crate::phy_rx_saturation::PhyRxSaturationAction),
    RxGain(crate::phy_rx_gain::PhyRxGainInitAction),
    Channel(crate::phy_channel::PhyChipChannelAction),
}

/// Identity-bound completion for one [`PhyBbInitAction`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitCompletion {
    Mmio(PhyBbMmioCompletion),
    TxDc(crate::phy_txdc::PhyTxDcCompletion),
    Pwdet(crate::phy_pwdet::PhyPwdetCompletion),
    TxCap(crate::phy_tx_cal::PhyTxCapCompletion),
    Temperature(crate::phy_temperature::PhyTemperatureCompletion),
    TxPower(crate::phy_tx_power::PhyTxPowerCompletion),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetCompletion),
    Dcode(crate::phy_dcode::PhyDcodeCompletion),
    TxIq(crate::phy_txiq::PhyTxIqInitCompletion),
    TxCfr(PhyTxCfrCompletion),
    PbusMemory(crate::phy_pbus_memory::PhyPbusMemoryCompletion),
    RxIq(crate::phy_rxiq::PhyRxIqInitCompletion),
    RxSaturation(crate::phy_rx_saturation::PhyRxSaturationCompletion),
    RxGain(crate::phy_rx_gain::PhyRxGainInitCompletion),
    Channel(crate::phy_channel::PhyChipChannelCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbExternalBindingError {
    UnsupportedAction,
}

/// Exhaustive lowering of every non-terminal Wi-Fi `phy_bb_init` action.
///
/// This is the boundary intended for the source-only radio executor. It owns
/// no child state and exposes no vendor callback: each variant is a consumed
/// token for one finite MMIO command, one readiness observation, one PBus
/// transaction, or one externally scheduled Rust timer.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyBbExternalBinding {
    Mmio(PhyBbMmioBinding),
    TxDc(crate::phy_txdc::PhyTxDcExternalBinding),
    Pwdet(crate::phy_pwdet::PhyPwdetExternalBinding),
    TxCap(crate::phy_tx_cal::PhyTxCapExternalBinding),
    Temperature(crate::phy_temperature::PhyTemperatureExternalBinding),
    TxPower(crate::phy_tx_power::PhyTxPowerExternalBinding),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBinding),
    Dcode(crate::phy_dcode::PhyDcodeExternalBinding),
    TxIq(crate::phy_txiq::PhyTxIqInitExternalBinding),
    TxCfr(PhyTxCfrMmioBinding),
    PbusMemory(crate::phy_pbus_memory::PhyPbusMemoryMmioBinding),
    RxIq(crate::phy_rxiq::PhyRxIqInitExternalBinding),
    RxSaturation(crate::phy_rx_saturation::PhyRxSaturationExternalBinding),
    RxGain(crate::phy_rx_gain::PhyRxGainInitExternalBinding),
    Channel(crate::phy_channel::PhyChipChannelExternalBinding),
}

impl PhyBbExternalBinding {
    pub fn lower(action: PhyBbInitAction) -> Result<Self, PhyBbExternalBindingError> {
        match action {
            PhyBbInitAction::Mmio(action) => Ok(Self::Mmio(PhyBbMmioBinding::new(action))),
            PhyBbInitAction::TxDc(action) => crate::phy_txdc::PhyTxDcExternalBinding::lower(action)
                .map(Self::TxDc)
                .map_err(|_| PhyBbExternalBindingError::UnsupportedAction),
            PhyBbInitAction::Pwdet(action) => {
                crate::phy_pwdet::PhyPwdetExternalBinding::lower(action)
                    .map(Self::Pwdet)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxCap(action) => {
                crate::phy_tx_cal::PhyTxCapExternalBinding::lower(action)
                    .map(Self::TxCap)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Temperature(action) => {
                crate::phy_temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxPower(action) => {
                crate::phy_tx_power::PhyTxPowerExternalBinding::lower(action)
                    .map(Self::TxPower)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxDcPwdet(action) => {
                crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBinding::lower(action)
                    .map(Self::TxDcPwdet)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Dcode(action) => {
                crate::phy_dcode::PhyDcodeExternalBinding::lower(action)
                    .map(Self::Dcode)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxIq(action) => {
                crate::phy_txiq::PhyTxIqInitExternalBinding::lower(action)
                    .map(Self::TxIq)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxCfr(action) => PhyTxCfrMmioBinding::new(action)
                .map(Self::TxCfr)
                .map_err(|_| PhyBbExternalBindingError::UnsupportedAction),
            PhyBbInitAction::PbusMemory(action) => {
                crate::phy_pbus_memory::PhyPbusMemoryMmioBinding::new(action)
                    .map(Self::PbusMemory)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxIq(action) => {
                crate::phy_rxiq::PhyRxIqInitExternalBinding::lower(action)
                    .map(Self::RxIq)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxSaturation(action) => {
                crate::phy_rx_saturation::PhyRxSaturationExternalBinding::lower(action)
                    .map(Self::RxSaturation)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxGain(action) => {
                crate::phy_rx_gain::PhyRxGainInitExternalBinding::lower(action)
                    .map(Self::RxGain)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Channel(action) => {
                crate::phy_channel::PhyChipChannelExternalBinding::lower(action)
                    .map(Self::Channel)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBbInitStep {
    EnableInitialization,
    SetCalibrationMode,
    TxDc(crate::phy_txdc::PhyTxDcTransition),
    Pwdet(crate::phy_pwdet::PhyPwdetTransition),
    TxCap(crate::phy_tx_cal::PhyTxCapTransition),
    TemperatureFirst(crate::phy_temperature::PhyTemperatureTransition),
    TxPower(crate::phy_tx_power::PhyTxPowerTransition),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetTransition),
    Dcode(crate::phy_dcode::PhyDcodeTransition),
    TxIq(crate::phy_txiq::PhyTxIqInitTransition),
    TxCfr(PhyTxCfrTransition),
    PbusMemory(crate::phy_pbus_memory::PhyPbusMemoryTransition),
    TemperatureSecond(crate::phy_temperature::PhyTemperatureTransition),
    RxIq(crate::phy_rxiq::PhyRxIqInitTransition),
    RxTable(PhyRxTableInitParameters),
    RxSaturationPrepare,
    RxSaturation(crate::phy_rx_saturation::PhyRxSaturationTransition),
    RxGain(crate::phy_rx_gain::PhyRxGainInitTransition),
    RxSaturationFinalize,
    RegisterInit(PhyRegisterInitParameters),
    UpdateAgc,
    UpdatePostInit,
    EnableAgc,
    Channel(crate::phy_channel::PhyChipChannelTransition),
    SetIdleMode,
    DisableWifi,
    ConfigureI2cTxRate,
    ConfigureTxPowerTracking,
    FailureSetIdle(PhyBbInitFailure),
    FailureEnableAgc(PhyBbInitFailure),
    Complete(PhyBbInitOutcome),
    Failed(PhyBbInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitLocalStep {
    /// One state-only terminal child result was committed. The caller may
    /// invoke `step_local` again or yield to another executor task.
    StateAdvanced,
    External(PhyBbInitAction),
    Complete(PhyBbInitOutcome),
    Failed(PhyBbInitFailure),
}

/// Unique Rust owner and exact parent sequencing for the Wi-Fi branch of
/// `phy_bb_init`.
///
/// `step_local` performs at most one local state transition. Every hardware
/// access, timer delay and readiness sample is returned to the outer radio
/// executor as one action. The type has no allocator, waker, retry loop,
/// pointer to `phy_param`, or ROM callback table.
pub struct PhyBbInitTransition {
    state: crate::phy_cold::PhyColdState,
    channel_or_frequency: u16,
    step: PhyBbInitStep,
    calibration_performed: bool,
}

impl PhyBbInitTransition {
    pub const fn new(state: crate::phy_cold::PhyColdState) -> Self {
        Self::new_on_channel(state, 11)
    }

    pub const fn new_on_channel(
        state: crate::phy_cold::PhyColdState,
        channel_or_frequency: u16,
    ) -> Self {
        Self {
            state,
            channel_or_frequency,
            step: PhyBbInitStep::EnableInitialization,
            calibration_performed: false,
        }
    }

    pub const fn state(&self) -> &crate::phy_cold::PhyColdState {
        &self.state
    }

    pub fn into_state(self) -> crate::phy_cold::PhyColdState {
        self.state
    }

    fn begin_failure(&mut self, failure: PhyBbInitFailure) {
        self.step = PhyBbInitStep::FailureSetIdle(failure);
    }

    fn channel_transition(&self) -> crate::phy_channel::PhyChipChannelTransition {
        crate::phy_channel::PhyChipChannelTransition::new(
            crate::phy_channel::PhyChipChannelRequest {
                channel_or_frequency: self.channel_or_frequency,
                cbw: 0,
                parameters: self.state.channel_parameters(),
            },
        )
    }

    pub fn step_local(&mut self) -> Result<PhyBbInitLocalStep, PhyBbInitTransitionError> {
        let local = match self.step {
            PhyBbInitStep::TxDc(transition) => match transition.action() {
                crate::phy_txdc::PhyTxDcAction::Complete(outcome) => {
                    self.state.apply_tx_dc_outcome(outcome);
                    self.step = PhyBbInitStep::Pwdet(crate::phy_pwdet::PhyPwdetTransition::new(
                        self.state.pwdet_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_txdc::PhyTxDcAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxDc(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxDc(action)),
            },
            PhyBbInitStep::Pwdet(transition) => match transition.action() {
                crate::phy_pwdet::PhyPwdetAction::Complete(outcome) => {
                    self.state.apply_pwdet_outcome(outcome);
                    self.step = PhyBbInitStep::TxCap(crate::phy_tx_cal::PhyTxCapTransition::new(
                        self.state.tx_cap_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_pwdet::PhyPwdetAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Pwdet(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Pwdet(action)),
            },
            PhyBbInitStep::TxCap(transition) => match transition.action() {
                crate::phy_tx_cal::PhyTxCapAction::Complete(outcome) => {
                    self.state.apply_tx_cap_outcome(outcome);
                    self.step = PhyBbInitStep::TemperatureFirst(
                        crate::phy_temperature::PhyTemperatureTransition::new(),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_tx_cal::PhyTxCapAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxCap(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxCap(action)),
            },
            PhyBbInitStep::TemperatureFirst(transition) => match transition.action() {
                crate::phy_temperature::PhyTemperatureAction::Complete(outcome) => {
                    self.state.apply_temperature_outcome(outcome);
                    self.step =
                        PhyBbInitStep::TxPower(crate::phy_tx_power::PhyTxPowerTransition::new(
                            self.state.tx_power_parameters(),
                        ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Temperature { pass: 1, failure });
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Temperature(action)),
            },
            PhyBbInitStep::TxPower(transition) => match transition.action() {
                crate::phy_tx_power::PhyTxPowerAction::Complete(outcome) => {
                    self.state.apply_tx_power_outcome(outcome);
                    self.step = PhyBbInitStep::TxDcPwdet(
                        crate::phy_txdc_pwdet::PhyTxDcPwdetTransition::new(
                            self.state.tx_dc_pwdet_parameters(),
                        ),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_tx_power::PhyTxPowerAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxPower(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxPower(action)),
            },
            PhyBbInitStep::TxDcPwdet(transition) => match transition.action() {
                crate::phy_txdc_pwdet::PhyTxDcPwdetAction::Complete(outcome) => {
                    self.state.apply_tx_dc_pwdet_outcome(outcome);
                    self.step = PhyBbInitStep::Dcode(crate::phy_dcode::PhyDcodeTransition::new(
                        self.state.dcode_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_txdc_pwdet::PhyTxDcPwdetAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxDcPwdet(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxDcPwdet(action)),
            },
            PhyBbInitStep::Dcode(transition) => match transition.action() {
                crate::phy_dcode::PhyDcodeAction::Complete(outcome) => {
                    self.state.apply_dcode_outcome(outcome);
                    self.step = PhyBbInitStep::TxIq(crate::phy_txiq::PhyTxIqInitTransition::new(
                        self.state.tx_iq_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_dcode::PhyDcodeAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Dcode(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Dcode(action)),
            },
            PhyBbInitStep::TxIq(transition) => match transition.action() {
                crate::phy_txiq::PhyTxIqInitAction::Complete(outcome) => {
                    self.state.apply_tx_iq_outcome(outcome);
                    self.state.mark_baseband_calibration_complete();
                    self.calibration_performed = true;
                    self.step = PhyBbInitStep::TxCfr(PhyTxCfrTransition::new());
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_txiq::PhyTxIqInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxIq(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxIq(action)),
            },
            PhyBbInitStep::TxCfr(transition) => match transition.action() {
                PhyTxCfrAction::Complete(_) => {
                    // The next vendor call is `phy_bt_tx_gain_init`. Its
                    // relocation graph includes shared RFPLL, TXDC and PWDET
                    // calibration, so this remains a known parent-graph gap
                    // until those operations have a Rust-owned transition.
                    self.step = PhyBbInitStep::PbusMemory(
                        crate::phy_pbus_memory::PhyPbusMemoryTransition::new(
                            self.state.pbus_memory_parameters(),
                        ),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxCfr(action)),
            },
            PhyBbInitStep::PbusMemory(transition) => match transition.action() {
                crate::phy_pbus_memory::PhyPbusMemoryAction::Complete(outcome) => {
                    self.state.apply_pbus_memory_outcome(outcome);
                    self.step = PhyBbInitStep::TemperatureSecond(
                        crate::phy_temperature::PhyTemperatureTransition::new(),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::PbusMemory(action)),
            },
            PhyBbInitStep::TemperatureSecond(transition) => match transition.action() {
                crate::phy_temperature::PhyTemperatureAction::Complete(outcome) => {
                    self.state.apply_temperature_outcome(outcome);
                    self.step = PhyBbInitStep::RxIq(crate::phy_rxiq::PhyRxIqInitTransition::new(
                        self.state.rx_iq_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Temperature { pass: 2, failure });
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Temperature(action)),
            },
            PhyBbInitStep::RxIq(transition) => match transition.action() {
                crate::phy_rxiq::PhyRxIqInitAction::Complete(outcome) => {
                    self.state.apply_rx_iq_outcome(outcome);
                    let parameters = self.state.prepare_rx_table_init();
                    self.step = PhyBbInitStep::RxTable(parameters);
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_rxiq::PhyRxIqInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::RxIq(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxIq(action)),
            },
            PhyBbInitStep::RxSaturation(transition) => match transition.action() {
                crate::phy_rx_saturation::PhyRxSaturationAction::Complete(outcome) => {
                    if self.state.apply_rx_saturation_outcome(outcome).is_ok() {
                        self.step = PhyBbInitStep::RxGain(
                            crate::phy_rx_gain::PhyRxGainInitTransition::new(
                                self.state.rx_gain_init_parameters(),
                            ),
                        );
                    } else {
                        self.begin_failure(PhyBbInitFailure::RxSaturation(outcome));
                    }
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxSaturation(action)),
            },
            PhyBbInitStep::RxGain(transition) => match transition.action() {
                crate::phy_rx_gain::PhyRxGainInitAction::Complete(outcome) => {
                    self.state.apply_rx_gain_init_outcome(outcome);
                    self.step = PhyBbInitStep::RxSaturationFinalize;
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_rx_gain::PhyRxGainInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::RxGain(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxGain(action)),
            },
            PhyBbInitStep::Channel(transition) => match transition.action() {
                crate::phy_channel::PhyChipChannelAction::Complete(outcome) => {
                    self.state.apply_channel_outcome(outcome);
                    self.step = PhyBbInitStep::SetIdleMode;
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::phy_channel::PhyChipChannelAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Channel(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Channel(action)),
            },
            PhyBbInitStep::Complete(outcome) => PhyBbInitLocalStep::Complete(outcome),
            PhyBbInitStep::Failed(failure) => PhyBbInitLocalStep::Failed(failure),
            step => PhyBbInitLocalStep::External(PhyBbInitAction::Mmio(step.mmio_action())),
        };
        Ok(local)
    }

    pub fn advance_external(
        &mut self,
        completion: PhyBbInitCompletion,
    ) -> Result<(), PhyBbInitTransitionError> {
        let step = match (self.step, completion) {
            (PhyBbInitStep::EnableInitialization, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableBasebandInitialization =>
            {
                PhyBbInitStep::SetCalibrationMode
            }
            (PhyBbInitStep::SetCalibrationMode, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Calibration,
                    }) =>
            {
                if self.state.baseband_calibration_complete() {
                    PhyBbInitStep::TxCfr(PhyTxCfrTransition::new())
                } else {
                    PhyBbInitStep::TxDc(crate::phy_txdc::PhyTxDcTransition::new(
                        self.state.tx_dc_parameters(),
                    ))
                }
            }
            (PhyBbInitStep::TxDc(mut transition), PhyBbInitCompletion::TxDc(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxDc(transition)
            }
            (PhyBbInitStep::Pwdet(mut transition), PhyBbInitCompletion::Pwdet(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Pwdet(transition)
            }
            (PhyBbInitStep::TxCap(mut transition), PhyBbInitCompletion::TxCap(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxCap(transition)
            }
            (
                PhyBbInitStep::TemperatureFirst(mut transition),
                PhyBbInitCompletion::Temperature(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TemperatureFirst(transition)
            }
            (PhyBbInitStep::TxPower(mut transition), PhyBbInitCompletion::TxPower(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxPower(transition)
            }
            (
                PhyBbInitStep::TxDcPwdet(mut transition),
                PhyBbInitCompletion::TxDcPwdet(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxDcPwdet(transition)
            }
            (PhyBbInitStep::Dcode(mut transition), PhyBbInitCompletion::Dcode(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Dcode(transition)
            }
            (PhyBbInitStep::TxIq(mut transition), PhyBbInitCompletion::TxIq(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxIq(transition)
            }
            (PhyBbInitStep::TxCfr(mut transition), PhyBbInitCompletion::TxCfr(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxCfr(transition)
            }
            (
                PhyBbInitStep::PbusMemory(mut transition),
                PhyBbInitCompletion::PbusMemory(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::PbusMemory(transition)
            }
            (
                PhyBbInitStep::TemperatureSecond(mut transition),
                PhyBbInitCompletion::Temperature(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TemperatureSecond(transition)
            }
            (PhyBbInitStep::RxIq(mut transition), PhyBbInitCompletion::RxIq(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxIq(transition)
            }
            (PhyBbInitStep::RxTable(parameters), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigureRxTable { parameters } =>
            {
                PhyBbInitStep::RxSaturationPrepare
            }
            (PhyBbInitStep::RxSaturationPrepare, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureRfRxSaturation {
                        phase: PhyRfRxSaturationPhase::PrepareCheck,
                    }) =>
            {
                PhyBbInitStep::RxSaturation(
                    crate::phy_rx_saturation::PhyRxSaturationTransition::new(
                        self.state.rx_saturation_parameter_002(),
                    ),
                )
            }
            (
                PhyBbInitStep::RxSaturation(mut transition),
                PhyBbInitCompletion::RxSaturation(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxSaturation(transition)
            }
            (PhyBbInitStep::RxGain(mut transition), PhyBbInitCompletion::RxGain(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxGain(transition)
            }
            (PhyBbInitStep::RxSaturationFinalize, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureRfRxSaturation {
                        phase: PhyRfRxSaturationPhase::Finalize,
                    }) =>
            {
                PhyBbInitStep::RegisterInit(self.state.register_init_parameters())
            }
            (PhyBbInitStep::RegisterInit(parameters), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigurePhyRegisters { parameters } =>
            {
                PhyBbInitStep::UpdateAgc
            }
            (PhyBbInitStep::UpdateAgc, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::UpdateAgcRegisters =>
            {
                PhyBbInitStep::UpdatePostInit
            }
            (PhyBbInitStep::UpdatePostInit, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::UpdatePostInitRegisters =>
            {
                PhyBbInitStep::EnableAgc
            }
            (PhyBbInitStep::EnableAgc, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableAgc =>
            {
                PhyBbInitStep::Channel(self.channel_transition())
            }
            (PhyBbInitStep::Channel(mut transition), PhyBbInitCompletion::Channel(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Channel(transition)
            }
            (PhyBbInitStep::SetIdleMode, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Idle,
                    }) =>
            {
                if self.state.disable_wifi_after_baseband_init() {
                    PhyBbInitStep::DisableWifi
                } else {
                    PhyBbInitStep::ConfigureI2cTxRate
                }
            }
            (PhyBbInitStep::DisableWifi, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::SetWifiEnabled { enabled: false } =>
            {
                PhyBbInitStep::ConfigureI2cTxRate
            }
            (PhyBbInitStep::ConfigureI2cTxRate, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigureI2cTxRate =>
            {
                PhyBbInitStep::ConfigureTxPowerTracking
            }
            (PhyBbInitStep::ConfigureTxPowerTracking, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true }) =>
            {
                PhyBbInitStep::Complete(PhyBbInitOutcome {
                    calibration_performed: self.calibration_performed,
                })
            }
            (PhyBbInitStep::FailureSetIdle(failure), PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Idle,
                    }) =>
            {
                PhyBbInitStep::FailureEnableAgc(failure)
            }
            (PhyBbInitStep::FailureEnableAgc(failure), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableAgc =>
            {
                PhyBbInitStep::Failed(failure)
            }
            (PhyBbInitStep::Complete(_), _) | (PhyBbInitStep::Failed(_), _) => {
                return Err(PhyBbInitTransitionError::AlreadyComplete)
            }
            _ => return Err(PhyBbInitTransitionError::WrongCompletion),
        };
        self.step = step;
        Ok(())
    }
}

impl PhyBbInitStep {
    const fn mmio_action(self) -> PhyBbMmioAction {
        match self {
            Self::EnableInitialization => PhyBbMmioAction::EnableBasebandInitialization,
            Self::SetCalibrationMode => PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Calibration,
            },
            Self::RxTable(parameters) => PhyBbMmioAction::ConfigureRxTable { parameters },
            Self::RxSaturationPrepare => PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::PrepareCheck,
            },
            Self::RxSaturationFinalize => PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::Finalize,
            },
            Self::RegisterInit(parameters) => PhyBbMmioAction::ConfigurePhyRegisters { parameters },
            Self::UpdateAgc => PhyBbMmioAction::UpdateAgcRegisters,
            Self::UpdatePostInit => PhyBbMmioAction::UpdatePostInitRegisters,
            Self::EnableAgc | Self::FailureEnableAgc(_) => PhyBbMmioAction::EnableAgc,
            Self::SetIdleMode | Self::FailureSetIdle(_) => PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Idle,
            },
            Self::DisableWifi => PhyBbMmioAction::SetWifiEnabled { enabled: false },
            Self::ConfigureI2cTxRate => PhyBbMmioAction::ConfigureI2cTxRate,
            Self::ConfigureTxPowerTracking => {
                PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true }
            }
            _ => panic!("non-MMIO baseband step"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        generate_phy_rx_gain_table, phy_generated_rx_gain_memory_entry, PhyBbBasebandMode,
        PhyBbMmioAction, PhyBbMmioBinding, PhyGainMemoryEntry, PhyRegisterInitParameters,
        PhyRfRxSaturationPhase, PhyRxGainBank, PhyRxGainMemoryParameters, PhyRxTableInitParameters,
        PhyTxCfrAction, PhyTxCfrBindingError, PhyTxCfrCompletion, PhyTxCfrEntry,
        PhyTxCfrMmioBinding, PhyTxCfrOutcome, PhyTxCfrTransition, PhyTxCfrTransitionError,
        PHY_TX_CFR_ENTRY_COUNT,
    };

    #[test]
    fn rx_gain_generator_reproduces_both_cold_parent_tables() {
        let wifi = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
        assert_eq!(wifi.last_index, 69);
        assert_eq!(wifi.words[0], 0x0004_0003);
        assert_eq!(wifi.words[68], 0x0007_f3c4);
        assert_eq!(wifi.words[69], 0x0007_f3c5);
        assert_eq!(wifi.words[70], 0);

        let shared = generate_phy_rx_gain_table(PhyRxGainBank::Shared);
        assert_eq!(shared.last_index, 75);
        assert_eq!(shared.words[0], 0x0004_0000);
        assert_eq!(shared.words[74], 0x0007_f380);
        assert_eq!(shared.words[75], 0x0007_f381);
        assert_eq!(shared.words[76], 0);
    }

    #[test]
    fn rx_gain_memory_entry_uses_only_copied_owner_state() {
        let mut wifi_index_dc = [[0_u16; 2]; 8];
        wifi_index_dc[0] = [3, 4];
        let parameters = PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc,
            wifi_dc_base: [10, 20],
            shared_index_dc: [[0; 2]; 11],
            rxbb_dc_adjustments: [[1, 2]; 6],
            wifi_auxiliary: 5,
        };
        let table = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
        assert_eq!(
            phy_generated_rx_gain_memory_entry(parameters, PhyRxGainBank::Wifi, &table, 0),
            PhyGainMemoryEntry {
                word0: 0x8102_c005,
                word1: 0xe006_0305,
                word2: 2,
                index: 0,
            }
        );
    }

    #[test]
    fn transition_reproduces_all_32_reference_entries() {
        let mut transition = PhyTxCfrTransition::new();
        assert_eq!(transition.action(), PhyTxCfrAction::ReadStartIndex);
        transition
            .advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0xfa })
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
    fn transition_rejects_foreign_or_late_completions() {
        let mut transition = PhyTxCfrTransition::new();
        transition
            .advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0x12 })
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
            transition.advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0 }),
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

    fn complete_parent_mmio(transition: &mut super::PhyBbInitTransition, action: PhyBbMmioAction) {
        transition
            .advance_external(super::PhyBbInitCompletion::Mmio(
                super::PhyBbMmioCompletion { action },
            ))
            .unwrap();
    }

    #[test]
    fn complete_parent_enters_or_skips_the_guarded_calibration_prefix() {
        let mut fresh = super::PhyBbInitTransition::new(crate::phy_cold::PhyColdState::new());
        assert_eq!(
            fresh.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
                PhyBbMmioAction::EnableBasebandInitialization
            ))
        );
        complete_parent_mmio(&mut fresh, PhyBbMmioAction::EnableBasebandInitialization);
        complete_parent_mmio(
            &mut fresh,
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Calibration,
            },
        );
        assert_eq!(
            fresh.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxDc(
                crate::phy_txdc::PhyTxDcAction::ConfigurePbusDebugMode
            ))
        );

        let mut image = *crate::phy_cold::PhyColdState::new().parameter_image();
        image[0x0a4] |= 0x08;
        let mut retained = super::PhyBbInitTransition::new(
            crate::phy_cold::PhyColdState::from_parameter_image(image),
        );
        complete_parent_mmio(&mut retained, PhyBbMmioAction::EnableBasebandInitialization);
        complete_parent_mmio(
            &mut retained,
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Calibration,
            },
        );
        assert_eq!(
            retained.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxCfr(
                PhyTxCfrAction::ReadStartIndex
            ))
        );
    }

    #[test]
    fn complete_parent_skips_only_the_bt_coexistence_child() {
        let mut transition = super::PhyBbInitTransition::new(crate::phy_cold::PhyColdState::new());
        transition.step = super::PhyBbInitStep::TxCfr(PhyTxCfrTransition::new());
        transition
            .advance_external(super::PhyBbInitCompletion::TxCfr(
                PhyTxCfrCompletion::StartIndexRead { base_index: 3 },
            ))
            .unwrap();
        let mut index = 0;
        while index != PHY_TX_CFR_ENTRY_COUNT {
            let action = match transition.step_local().unwrap() {
                super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxCfr(
                    PhyTxCfrAction::ProgramEntry(entry),
                )) => entry,
                other => panic!("unexpected CFR action: {other:?}"),
            };
            transition
                .advance_external(super::PhyBbInitCompletion::TxCfr(
                    PhyTxCfrCompletion::EntryProgrammed(action),
                ))
                .unwrap();
            index += 1;
        }
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::StateAdvanced
        );
        assert!(matches!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::PbusMemory(
                crate::phy_pbus_memory::PhyPbusMemoryAction::Program(_)
            ))
        ));
    }

    #[test]
    fn complete_parent_tail_preserves_conditional_disable_and_tracking_order() {
        let mut image = *crate::phy_cold::PhyColdState::new().parameter_image();
        image[0x196] = 1;
        let mut transition = super::PhyBbInitTransition::new(
            crate::phy_cold::PhyColdState::from_parameter_image(image),
        );
        transition.step = super::PhyBbInitStep::SetIdleMode;

        complete_parent_mmio(
            &mut transition,
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Idle,
            },
        );
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
                PhyBbMmioAction::SetWifiEnabled { enabled: false }
            ))
        );
        complete_parent_mmio(
            &mut transition,
            PhyBbMmioAction::SetWifiEnabled { enabled: false },
        );
        complete_parent_mmio(&mut transition, PhyBbMmioAction::ConfigureI2cTxRate);
        complete_parent_mmio(
            &mut transition,
            PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true },
        );
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::Complete(super::PhyBbInitOutcome {
                calibration_performed: false,
            })
        );
    }

    #[test]
    fn complete_parent_failure_always_restores_idle_mode_and_agc() {
        let state = crate::phy_cold::PhyColdState::new();
        let parameters = state.channel_parameters();
        let mut transition = super::PhyBbInitTransition::new(state);
        transition.step =
            super::PhyBbInitStep::Channel(crate::phy_channel::PhyChipChannelTransition::new(
                crate::phy_channel::PhyChipChannelRequest {
                    channel_or_frequency: 0,
                    cbw: 0,
                    parameters,
                },
            ));
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::StateAdvanced
        );
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
                PhyBbMmioAction::SetBasebandMode {
                    mode: PhyBbBasebandMode::Idle,
                }
            ))
        );
        complete_parent_mmio(
            &mut transition,
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Idle,
            },
        );
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
                PhyBbMmioAction::EnableAgc
            ))
        );
        complete_parent_mmio(&mut transition, PhyBbMmioAction::EnableAgc);
        assert_eq!(
            transition.step_local().unwrap(),
            super::PhyBbInitLocalStep::Failed(super::PhyBbInitFailure::Channel(
                crate::phy_channel::PhyChipChannelFailure::UnsupportedChannel(0)
            ))
        );
    }
}
