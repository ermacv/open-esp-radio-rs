//! Event-driven ESP32-S31 PHY cold-initialization operations.
//!
//! Unique typed software state lives in [`crate::phy_state::PhyState`]. This
//! module contains only the hardware-operation graph and its bound external
//! completions.

/// Return the exact pinned ESP32-S31 RF-data record count.
///
/// The compiled vendor archive returns this independent value verbatim.
#[inline]
pub const fn phy_get_rfdata_num() -> u32 {
    0x20c
}

/// Required pinned `libphy.a::phy_internal_delay` vendor-ABI no-op leaf.
#[inline]
pub const fn phy_internal_delay() -> u32 {
    0
}

use crate::{
    phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateRequest,
        PhyDcIqReadinessSnapshot,
    },
    phy_frequency::{
        PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion, PhyFrequencyI2cAction,
        PhyFrequencyI2cCompletion, PhyFrequencyTableAction, PhyFrequencyTableCompletion,
    },
    phy_i2c::{
        AdcRateAction, AdcRateCompletion, FilterDcapAction, FilterDcapCompletion, I2cInit1Action,
        I2cInit1Completion, OpenI2cXpdAction, OpenI2cXpdCompletion, PhyI2cAddress, PhyI2cError,
        PhyI2cField, PhyRfInitPrefixAction, PhyRfInitPrefixCompletion, PhyRfInitPrefixOutcome,
        PhyRfInitPrefixTransition, PhyRfInitPrefixTransitionError, RcCalibrationAction,
        RcCalibrationCompletion, RfpllChargePumpAction, RfpllChargePumpCompletion,
    },
    phy_pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion},
    phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
        PhySignalPowerRequest,
    },
    phy_xtal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyPassAction,
        XtalDutyPassCompletion, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
        XtalDutyRestoreAction, XtalDutyRestoreCompletion, XtalDutySearchAction,
        XtalDutySearchCompletion,
    },
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::SharedPhyAccess;

pub use crate::phy_state::{
    PhyDot11pConfiguration, PhyRegisterTemperatureControl, PhyState, PhyTemperatureTrackingDebug,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cRequest {
    ReadByte { address: PhyI2cAddress },
    ReadField { field: PhyI2cField },
    WriteByte { address: PhyI2cAddress, value: u8 },
    WriteField { field: PhyI2cField, value: u8 },
}

impl PhyColdI2cRequest {
    pub const fn read_byte(address: PhyI2cAddress) -> Self {
        Self::ReadByte { address }
    }

    pub const fn read_field(field: PhyI2cField) -> Self {
        Self::ReadField { field }
    }

    pub const fn write_byte(address: PhyI2cAddress, value: u8) -> Self {
        Self::WriteByte { address, value }
    }

    pub const fn write_field(field: PhyI2cField, value: u8) -> Self {
        Self::WriteField { field, value }
    }

    pub const fn address(self) -> PhyI2cAddress {
        match self {
            Self::ReadByte { address } | Self::WriteByte { address, .. } => address,
            Self::ReadField { field } | Self::WriteField { field, .. } => field.address(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cAction {
    StartRead { address: PhyI2cAddress },
    AwaitReadCompletionEdge { address: PhyI2cAddress },
    StartWrite { address: PhyI2cAddress, value: u8 },
    AwaitWriteCompletionEdge { address: PhyI2cAddress },
    Complete(PhyColdI2cOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cOutcome {
    Read { address: PhyI2cAddress, value: u8 },
    Written { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cObservation {
    /// The externally delivered edge arrived before the peripheral completed.
    ///
    /// The transaction remains unchanged and does not arrange another wake.
    /// Only a new hardware edge or an outer deadline may call the observation
    /// method again.
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cError {
    BusyAtStart,
    WrongEdge,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyColdI2cPhase {
    StartRead,
    AwaitRead,
    StartWrite(u8),
    AwaitWrite,
    Complete(PhyColdI2cOutcome),
}

/// One nonblocking PHY-I2C transaction, including masked read/modify/write.
///
/// Start and completion are different states.  Observing `Busy` after an
/// externally delivered edge leaves the state at `Await*` and returns
/// [`PhyColdI2cObservation::StillPending`]; it does not spin, retry, register a
/// waker, or request an executor poll.  A separate owner must provide either a
/// later hardware edge or a deadline.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdI2cTransaction {
    request: PhyColdI2cRequest,
    phase: PhyColdI2cPhase,
}

impl PhyColdI2cTransaction {
    pub const fn new(request: PhyColdI2cRequest) -> Self {
        let phase = match request {
            PhyColdI2cRequest::ReadByte { .. }
            | PhyColdI2cRequest::ReadField { .. }
            | PhyColdI2cRequest::WriteField { .. } => PhyColdI2cPhase::StartRead,
            PhyColdI2cRequest::WriteByte { value, .. } => PhyColdI2cPhase::StartWrite(value),
        };
        Self { request, phase }
    }

    pub const fn action(&self) -> PhyColdI2cAction {
        let address = self.request.address();
        match self.phase {
            PhyColdI2cPhase::StartRead => PhyColdI2cAction::StartRead { address },
            PhyColdI2cPhase::AwaitRead => PhyColdI2cAction::AwaitReadCompletionEdge { address },
            PhyColdI2cPhase::StartWrite(value) => PhyColdI2cAction::StartWrite { address, value },
            PhyColdI2cPhase::AwaitWrite => PhyColdI2cAction::AwaitWriteCompletionEdge { address },
            PhyColdI2cPhase::Complete(outcome) => PhyColdI2cAction::Complete(outcome),
        }
    }

    pub fn read_started(&mut self) -> Result<(), PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::StartRead {
            return Err(self.phase_error());
        }
        self.phase = PhyColdI2cPhase::AwaitRead;
        Ok(())
    }

    pub fn write_started(&mut self) -> Result<(), PhyColdI2cError> {
        if !matches!(self.phase, PhyColdI2cPhase::StartWrite(_)) {
            return Err(self.phase_error());
        }
        self.phase = PhyColdI2cPhase::AwaitWrite;
        Ok(())
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::AwaitRead {
            return Err(self.phase_error());
        }
        let value = match result {
            Ok(value) => value,
            Err(PhyI2cError::Busy) => return Ok(PhyColdI2cObservation::StillPending),
        };

        let address = self.request.address();
        self.phase = match self.request {
            PhyColdI2cRequest::ReadByte { .. } => {
                PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Read { address, value })
            }
            PhyColdI2cRequest::ReadField { field } => {
                PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Read {
                    address,
                    value: field.extract(value),
                })
            }
            PhyColdI2cRequest::WriteField {
                field,
                value: field_value,
            } => PhyColdI2cPhase::StartWrite(field.replace(value, field_value)),
            PhyColdI2cRequest::WriteByte { .. } => return Err(PhyColdI2cError::WrongEdge),
        };
        Ok(PhyColdI2cObservation::EdgeConsumed)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::AwaitWrite {
            return Err(self.phase_error());
        }
        match result {
            Ok(()) => {
                self.phase = PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Written {
                    address: self.request.address(),
                });
                Ok(PhyColdI2cObservation::EdgeConsumed)
            }
            Err(PhyI2cError::Busy) => Ok(PhyColdI2cObservation::StillPending),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), PhyColdI2cError> {
        match self.action() {
            PhyColdI2cAction::StartRead { address } => {
                crate::phy_i2c::try_start_read(platform, address)
                    .map_err(|PhyI2cError::Busy| PhyColdI2cError::BusyAtStart)?;
                self.read_started()
            }
            PhyColdI2cAction::StartWrite { address, value } => {
                crate::phy_i2c::try_start_write(platform, address, value)
                    .map_err(|PhyI2cError::Busy| PhyColdI2cError::BusyAtStart)?;
                self.write_started()
            }
            PhyColdI2cAction::Complete(_) => Err(PhyColdI2cError::AlreadyComplete),
            _ => Err(PhyColdI2cError::WrongEdge),
        }
    }

    /// Consume exactly one independently delivered target completion edge.
    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        &mut self,
        platform: &P,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        match self.action() {
            PhyColdI2cAction::AwaitReadCompletionEdge { address } => {
                self.observe_read_result(crate::phy_i2c::try_finish_read(platform, address))
            }
            PhyColdI2cAction::AwaitWriteCompletionEdge { address } => {
                self.observe_write_result(crate::phy_i2c::try_finish_write(platform, address))
            }
            PhyColdI2cAction::Complete(_) => Err(PhyColdI2cError::AlreadyComplete),
            _ => Err(PhyColdI2cError::WrongEdge),
        }
    }

    const fn phase_error(&self) -> PhyColdI2cError {
        if matches!(self.phase, PhyColdI2cPhase::Complete(_)) {
            PhyColdI2cError::AlreadyComplete
        } else {
            PhyColdI2cError::WrongEdge
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdLoweringError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
    HardwareRestoreInvariant,
}

/// Identity-bound lowering of one RF-init action to one PHY-I2C transaction.
///
/// The original action remains part of the binding until the transaction is
/// complete. This prevents a completion from being reused for a later action
/// which happens to address the same PHY-I2C register.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdI2cBinding {
    outer_action: PhyRfInitPrefixAction,
    transaction: PhyColdI2cTransaction,
}

impl PhyColdI2cBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let request = lower_prefix_i2c_request(outer_action)
            .ok_or(PhyColdLoweringError::UnsupportedAction)?;
        Ok(Self {
            outer_action,
            transaction: PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn action(&self) -> PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        let PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        };
        lower_prefix_i2c_completion(self.outer_action, outcome)
            .ok_or(PhyColdLoweringError::UnexpectedOutcome)
    }
}

/// Non-cloneable identity-bound owner of a PAC-owned PHY-I²C configuration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdI2cConfigurationBinding {
    outer_action: PhyRfInitPrefixAction,
    transaction: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationTransaction,
}

impl PhyColdI2cConfigurationBinding {
    pub const fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let operation = match outer_action {
            PhyRfInitPrefixAction::ConfigureBiasRegisters => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::BiasRegisters
            }
            PhyRfInitPrefixAction::ConfigureI2cBbpll => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::EnableBbpllCalibration
            }
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureI2c { rate }) => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::ConfigureAdcRate(rate)
            }
            PhyRfInitPrefixAction::ConfigureRcCalibrationSettings => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::RcCalibrationSettings
            }
            PhyRfInitPrefixAction::ConfigureSar2 => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::Sar2Initialization
            }
            PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Configure(parameters)) => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::FilterDcap(
                    parameters.pac_inputs(),
                )
            }
            PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Configure(parameters)) => {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationOperation::InitializationStageOne(
                    parameters.pac_initialization_stage_one_inputs(),
                )
            }
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            transaction: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationTransaction::new(
                operation,
            ),
        })
    }

    pub const fn action(&self) -> open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationAction {
        self.transaction.action()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        platform: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<(), PhyColdI2cError> {
        open_esp_radio_esp32s31_hal::phy_i2c::start_configuration(&mut self.transaction, platform)
            .map_err(|error| match error {
                open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationError::BusyAtStart => {
                    PhyColdI2cError::BusyAtStart
                }
                _ => PhyColdI2cError::WrongEdge,
            })
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        platform: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        open_esp_radio_esp32s31_hal::phy_i2c::observe_configuration(&mut self.transaction, platform)
            .map(|observation| {
                match observation {
            open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationObservation::StillPending => {
                PhyColdI2cObservation::StillPending
            }
            open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationObservation::EdgeConsumed => {
                PhyColdI2cObservation::EdgeConsumed
            }
        }
            })
            .map_err(|_| PhyColdI2cError::WrongEdge)
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.action()
            != open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationAction::Complete
        {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        }
        match self.outer_action {
            PhyRfInitPrefixAction::ConfigureBiasRegisters => {
                Ok(PhyRfInitPrefixCompletion::BiasRegistersConfigured)
            }
            PhyRfInitPrefixAction::ConfigureI2cBbpll => {
                Ok(PhyRfInitPrefixCompletion::I2cBbpllConfigured)
            }
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureI2c { .. }) => Ok(
                PhyRfInitPrefixCompletion::AdcRate(AdcRateCompletion::I2cConfigured),
            ),
            PhyRfInitPrefixAction::ConfigureRcCalibrationSettings => {
                Ok(PhyRfInitPrefixCompletion::RcCalibrationSettingsConfigured)
            }
            PhyRfInitPrefixAction::ConfigureSar2 => Ok(PhyRfInitPrefixCompletion::Sar2Configured),
            PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Configure(_)) => Ok(
                PhyRfInitPrefixCompletion::FilterDcap(FilterDcapCompletion::Configured),
            ),
            PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Configure(_)) => Ok(
                PhyRfInitPrefixCompletion::I2cInit1(I2cInit1Completion::Configured),
            ),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }
}

fn lower_prefix_i2c_request(action: PhyRfInitPrefixAction) -> Option<PhyColdI2cRequest> {
    match action {
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteByte {
            address,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::WriteByte { address, value },
            ..
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::WriteByte { address, value },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteByte { address, value },
            )),
        )) => Some(PhyColdI2cRequest::write_byte(address, value)),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate { address, candidate }),
        )) => Some(PhyColdI2cRequest::write_byte(address, candidate)),
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ReadSdmSample { address })
        | PhyRfInitPrefixAction::ReadParameter18e { address }
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadByte { address })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::ReadByte {
            address,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::ReadByte { address },
            ..
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::ReadByte { address },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::ReadByte { address },
            )),
        )) => Some(PhyColdI2cRequest::read_byte(address)),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked { field, value })
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::WriteMasked {
            field,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
            field,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::WriteMasked { field, value },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::DisableCalibrationPath {
            field,
            value,
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::WriteMasked { field, value },
        ))
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::WriteMasked { field, value },
            ..
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteMasked { field, value },
            )),
        )) => Some(PhyColdI2cRequest::write_field(field, value)),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked { field })
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadMasked { field })
        | PhyRfInitPrefixAction::ReadMasked69 { field }
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty { field })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::ReadMasked { field },
            ..
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::ReadMasked { field },
            )),
        )) => Some(PhyColdI2cRequest::read_field(field)),
        _ => None,
    }
}

fn lower_prefix_i2c_completion(
    action: PhyRfInitPrefixAction,
    outcome: PhyColdI2cOutcome,
) -> Option<PhyRfInitPrefixCompletion> {
    match (action, outcome) {
        (
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ReadSdmSample { address }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::SdmSample(value),
        )),
        (
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked { .. }),
            PhyColdI2cOutcome::Written { .. },
        ) => Some(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Write,
        )),
        (
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked { .. }),
            PhyColdI2cOutcome::Read { value, .. },
        ) => Some(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Read(value),
        )),
        (
            PhyRfInitPrefixAction::ReadParameter18e { address },
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => {
            Some(PhyRfInitPrefixCompletion::Parameter18eRead { address, value })
        }
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::WriteMasked { .. }),
            PhyColdI2cOutcome::Written { .. },
        ) => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::Write,
        )),
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadMasked { .. }),
            PhyColdI2cOutcome::Read { value, .. },
        ) => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadMasked(value),
        )),
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadByte { address }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadByte { address, value },
        )),
        (PhyRfInitPrefixAction::ReadMasked69 { .. }, PhyColdI2cOutcome::Read { value, .. }) => {
            Some(PhyRfInitPrefixCompletion::Masked69Read(value))
        }
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
                field,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::MaskedWrite { field },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteByte {
                address,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::ByteWrite { address },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::ReadByte {
                address,
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::ByteRead { address, value },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::WriteMasked { field, .. },
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                field,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::WriteByte { address, .. },
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::ByteWrite {
                address,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::ReadMasked { field },
                ..
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::MaskedRead {
                field,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::ReadByte { address },
                ..
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::ByteRead {
                address,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::WriteMasked { field, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::MaskedWrite {
                field,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::ReadByte { address },
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::ByteRead {
                address,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty { field }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::InitialDutyRead { field, value },
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::DisableCalibrationPath {
                field,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::CalibrationPathDisabled { field },
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteMasked { field, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::MaskedWrite { field }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteByte { address, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::ByteWrite { address }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::WriteMasked { field, .. },
                )),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite { field }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::WriteByte { address, .. },
                )),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::ByteWrite { address }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::ReadMasked { field },
                )),
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if field.address() == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedRead {
                    field,
                    value,
                }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::ReadByte { address },
                )),
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::ByteRead {
                    address,
                    value,
                }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                    address,
                    candidate,
                }),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::CandidateWritten { address, candidate },
            )),
        )),
        _ => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdMmioBinding {
    outer_action: PhyRfInitPrefixAction,
}

impl PhyColdMmioBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        if lower_prefix_mmio_completion(outer_action).is_none() {
            return Err(PhyColdLoweringError::UnsupportedAction);
        }
        Ok(Self { outer_action })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        lower_prefix_mmio_completion(self.outer_action)
            .ok_or(PhyColdLoweringError::UnsupportedAction)
    }

    /// Execute exactly one finite target MMIO transaction and consume its
    /// identity token.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<R: open_esp_radio_esp32s31_hal::SharedPhyAccess>(
        self,
        registers: &mut R,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::ConfigureFeBbClock => {
                registers.open_frontend_baseband_internal_clocks();
                open_esp_radio_esp32s31_hal::analog_i2c::enable_frontend_baseband_power(registers);
                return self.into_completion();
            }
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay) => {
                crate::phy_i2c::configure_open_i2c_pre_delay(registers);
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                open_esp_radio_esp32s31_hal::phy_power_detector::initialize_registers(registers)
                    .map_err(|_| PhyColdLoweringError::HardwareRestoreInvariant)?;
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                open_esp_radio_esp32s31_hal::phy_power_detector::configure_background(registers);
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                open_esp_radio_esp32s31_hal::phy_temperature::initialize(registers);
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled } => {
                open_esp_radio_esp32s31_hal::phy_i2c::configure_bbpll_calibration(
                    registers, enabled,
                );
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection } => {
                open_esp_radio_esp32s31_hal::phy_i2c::configure_clock_selection(
                    registers, selection,
                );
                return self.into_completion();
            }
            PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                open_esp_radio_esp32s31_hal::phy_i2c::configure_master_registers(registers);
                return self.into_completion();
            }
            _ => {}
        }

        match self.outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode) => {
                open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers)
            }
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkModePulse) => {
                open_esp_radio_esp32s31_hal::phy_agc::configure_pbus_work_mode_pulse(registers)
            }
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ClearWorkModePulse) => {
                open_esp_radio_esp32s31_hal::phy_agc::clear_pbus_work_mode_pulse(registers)
            }
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio { rate }) => {
                crate::phy_hardware::configure_phy_adc_rate(registers, rate)
            }
            PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                crate::phy_hardware::configure_phy_front_end_registers(registers)
            }
            PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { parameter } => {
                crate::phy_i2c::configure_i2c_master_command_memory(registers, parameter)
            }
            PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
                crate::phy_hardware::configure_phy_front_end_update(registers)
            }
            PhyRfInitPrefixAction::ChannelFrequency(
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override },
            ) => open_esp_radio_esp32s31_hal::phy_frequency::initialize_registers(
                registers,
                parameter_override,
            ),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Table(
                PhyFrequencyTableAction::WriteMemory {
                    address,
                    value,
                    mode,
                    ..
                },
            ))
            | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::WriteMemory {
                    address,
                    value,
                    mode,
                    ..
                },
            )) => open_esp_radio_esp32s31_hal::phy_frequency::write_memory(
                registers, address, value, mode,
            ),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::ConfigureNumberAddresses(image),
            )) => open_esp_radio_esp32s31_hal::phy_frequency::configure_i2c_number_addresses(
                registers,
                image.control_field,
                image.words,
            ),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                    enabled,
                    selector,
                    step,
                }),
            )) => crate::phy_hardware::configure_phy_calibration_tone(
                registers, enabled, selector, step,
            ),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureRxClock { enabled }),
            )) => open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureTxClock { enabled }),
            )) => open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigurePbusDebugMode),
            )) => open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::PrepareRxDcoControlRestore),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::PrepareRxDcoControlRestore,
                )),
            )) => open_esp_radio_esp32s31_hal::phy_rx_dco::prepare_control_restore(registers)
                .map_err(|_| PhyColdLoweringError::HardwareRestoreInvariant)?,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RestoreRxDcoControl),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::RestoreRxDcoControl,
                )),
            )) => open_esp_radio_esp32s31_hal::phy_rx_dco::restore_control(registers)
                .map_err(|_| PhyColdLoweringError::HardwareRestoreInvariant)?,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::Configure(request),
                ))),
            )) => crate::phy_dc_iq::configure_target(registers, request.control),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::SetEnable { phase, enabled, .. },
                ))),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::SetEstimatorEnable { phase, enabled, .. },
                )),
            )) => crate::phy_dc_iq::set_enable_target(registers, phase, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ConfigureClock { clock, enabled, .. },
                )),
            )) => match clock {
                crate::phy_signal_power::PhySignalPowerClock::Tx => {
                    open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled)
                }
                crate::phy_signal_power::PhySignalPowerClock::Rx => {
                    open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, enabled)
                }
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ConfigureEstimator { control, .. },
                )),
            )) => crate::phy_dc_iq::configure_target(registers, control),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureCalibrationTone {
                    enabled,
                    selector,
                    step,
                }),
            )) => crate::phy_hardware::configure_phy_calibration_tone(
                registers, enabled, selector, step,
            ),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureRxClock { enabled }),
            )) => open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureTxClock { enabled }),
            )) => open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
            )) => open_esp_radio_esp32s31_hal::phy_agc::configure_pbus_work_mode_pulse(registers),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ClearPbusWorkModePulse),
            )) => open_esp_radio_esp32s31_hal::phy_agc::clear_pbus_work_mode_pulse(registers),
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        }
        self.into_completion()
    }
}

fn lower_prefix_mmio_completion(
    action: PhyRfInitPrefixAction,
) -> Option<PhyRfInitPrefixCompletion> {
    match action {
        PhyRfInitPrefixAction::ConfigureFeBbClock => {
            Some(PhyRfInitPrefixCompletion::FeBbClockConfigured)
        }
        PhyRfInitPrefixAction::ConfigureBbpllCalibration { .. } => {
            Some(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
        }
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay) => Some(
            PhyRfInitPrefixCompletion::OpenI2cXpd(OpenI2cXpdCompletion::PreDelayConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::DebugModeConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkModePulse) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::WorkModePulseConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ClearWorkModePulse) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::WorkModePulseCleared),
        ),
        PhyRfInitPrefixAction::ConfigureI2cClockSelection { .. } => {
            Some(PhyRfInitPrefixCompletion::I2cClockSelectionConfigured)
        }
        PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio { .. }) => Some(
            PhyRfInitPrefixCompletion::AdcRate(AdcRateCompletion::MmioConfigured),
        ),
        PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
            Some(PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
            Some(PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
            Some(PhyRfInitPrefixCompletion::FrontEndRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
            Some(PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured)
        }
        PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
            Some(PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured)
        }
        PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { .. } => {
            Some(PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured)
        }
        PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
            Some(PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured)
        }
        PhyRfInitPrefixAction::ChannelFrequency(
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override },
        ) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured { parameter_override },
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Table(
            PhyFrequencyTableAction::WriteMemory {
                entry_index,
                word_index,
                address,
                ..
            },
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Table(PhyFrequencyTableCompletion {
                entry_index,
                word_index,
                address,
            }),
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::WriteMemory {
                descriptor_index,
                copy_index,
                address,
                ..
            },
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::MemoryWrite {
                descriptor_index,
                copy_index,
                address,
            }),
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::ConfigureNumberAddresses(image),
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(image),
            ),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::CalibrationToneConfigured {
                    enabled,
                    selector,
                    step,
                },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureRxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureTxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::TxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigurePbusDebugMode),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::PbusDebugModeConfigured,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::PrepareRxDcoControlRestore),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                PhyRxDcoAction::PrepareRxDcoControlRestore,
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::RxDcoControlRestorePrepared),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RestoreRxDcoControl),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDcoControlRestored,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                PhyRxDcoAction::RestoreRxDcoControl,
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::RxDcoControlRestored),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::Configure(request),
            ))),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::Configured(request),
                )),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::SetEnable {
                    request,
                    phase,
                    enabled,
                },
            ))),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::EnableSet {
                        request,
                        phase,
                        enabled,
                    },
                )),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureClock {
                    request,
                    clock,
                    enabled,
                },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::ClockConfigured {
                    request,
                    clock,
                    enabled,
                }),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::SetEstimatorEnable {
                    request,
                    phase,
                    enabled,
                },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(
                    PhySignalPowerCompletion::EstimatorEnableSet {
                        request,
                        phase,
                        enabled,
                    },
                ),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureEstimator { request, control },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(
                    PhySignalPowerCompletion::EstimatorConfigured { request, control },
                ),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::CalibrationToneConfigured {
                    enabled,
                    selector,
                    step,
                },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureRxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::RxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureTxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::TxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModePulseConfigured,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ClearPbusWorkModePulse),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModePulseCleared,
            )),
        )),
        _ => None,
    }
}

/// One timer edge belonging to one exact RF-init action.
///
/// The value owns no timer implementation and cannot wake itself. The outer
/// Rust executor arms its timer from [`micros`](Self::micros), then consumes
/// this binding only when that timer reports expiry.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdTimerBinding {
    outer_action: PhyRfInitPrefixAction,
    micros: u32,
}

impl PhyColdTimerBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let micros = match outer_action {
            PhyRfInitPrefixAction::DelayMicros(micros)
            | PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::DelayMicros(micros),
                ..
            }) => micros,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::DelayMicros(micros),
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::DelayMicros { micros, .. },
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::DelayMicros { micros, .. },
                ))),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros { micros, .. }),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::DelayMicros { micros, .. },
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(micros)),
            )) => micros,
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            micros,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub fn into_elapsed_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::DelayMicros(_) => Ok(PhyRfInitPrefixCompletion::DelayElapsed),
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::OpenI2cXpd(OpenI2cXpdCompletion::DelayElapsed),
            ),
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::DelayElapsed),
            ),
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::RcCalibration(RcCalibrationCompletion::Delay),
            ),
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::RfpllChargePump(RfpllChargePumpCompletion::Delay),
            ),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::DelayMicros(micros),
                ..
            }) => Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(
                    micros,
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::DelayMicros(micros),
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(
                        micros,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::DelayMicros { iteration, micros },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DelayElapsed {
                        iteration,
                        micros,
                    }),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::DelayMicros {
                        request,
                        phase,
                        micros,
                    },
                ))),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::DelayElapsed {
                            request,
                            phase,
                            micros,
                        },
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros { candidate, .. }),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::DelayElapsed { candidate },
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::DelayMicros {
                        request,
                        phase,
                        micros,
                    },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::DelayElapsed {
                        request,
                        phase,
                        micros,
                    }),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(micros)),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::DelayElapsed { micros },
                )),
            )),
            _ => Err(PhyColdLoweringError::UnsupportedAction),
        }
    }
}

/// Exactly one lowered external operation owned by the cold-init executor.
///
/// Unsupported nested actions are rejected during construction; there is no
/// generic vendor callback or synchronous fallback variant.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyColdExternalBinding {
    I2cConfiguration(PhyColdI2cConfigurationBinding),
    I2c(PhyColdI2cBinding),
    Mmio(PhyColdMmioBinding),
    Observation(PhyColdObservationBinding),
    Pbus(PhyColdPbusBinding),
    Timer(PhyColdTimerBinding),
}

impl PhyColdExternalBinding {
    pub fn lower(action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        if let Ok(binding) = PhyColdI2cConfigurationBinding::new(action) {
            return Ok(Self::I2cConfiguration(binding));
        }
        if let Ok(binding) = PhyColdI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = PhyColdMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyColdPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyColdObservationBinding::new(action) {
            return Ok(Self::Observation(binding));
        }
        if let Ok(binding) = PhyColdTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyColdLoweringError::UnsupportedAction)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdObservationRequest {
    ConfigureOpenI2cPowerAndPulse,
    CheckOpenI2cSdmDeadline {
        started_at_cycle: u32,
        maximum_cycles: u32,
    },
    ConfigurePbusWorkMode,
    ReadRxDcoPbus {
        selector: u8,
        path: u8,
    },
    ObserveDcIqReadiness {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
        readiness_samples: u16,
    },
    ReadDcIqAccumulators(PhyDcIqEstimateRequest),
    ObserveSignalPowerReadiness {
        request: PhySignalPowerRequest,
        readiness_activity_edges: u16,
        readiness_samples: u16,
    },
    ReadSignalPowerAccumulators(PhySignalPowerRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdObservationResult {
    OpenI2cPowerAndPulse {
        started_at_cycle: u32,
    },
    OpenI2cSdmDeadline {
        expired: bool,
    },
    PbusWorkMode {
        settle_required: bool,
    },
    RxDcoPbusRead {
        selector: u8,
        path: u8,
        value: u32,
    },
    DcIqReadiness {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    DcIqAccumulators {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqAccumulatorSnapshot,
    },
    SignalPowerReadiness {
        request: PhySignalPowerRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    SignalPowerAccumulators {
        request: PhySignalPowerRequest,
        snapshot: PhySignalPowerAccumulatorSnapshot,
    },
}

/// One finite MMIO operation whose sampled value is part of the completion.
///
/// This is separate from [`PhyColdMmioBinding`] so a dynamic register sample
/// cannot be fabricated by constructing a fixed completion. Consuming the
/// binding returns the observation to exactly the parent action that requested
/// it.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdObservationBinding {
    outer_action: PhyRfInitPrefixAction,
    request: PhyColdObservationRequest,
}

impl PhyColdObservationBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let request = match outer_action {
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse) => {
                PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse
            }
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            }) => PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            },
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode)
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
            )) => PhyColdObservationRequest::ConfigurePbusWorkMode,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ReadPbus { selector, path },
                )),
            )) if selector == 1 && path == 2 => {
                PhyColdObservationRequest::ReadRxDcoPbus { selector, path }
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::AwaitReadinessEdge {
                        request,
                        readiness_activity_edges,
                        readiness_samples,
                    },
                ))),
            )) => PhyColdObservationRequest::ObserveDcIqReadiness {
                request,
                readiness_activity_edges,
                readiness_samples,
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::ReadAccumulators(request),
                ))),
            )) => PhyColdObservationRequest::ReadDcIqAccumulators(request),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::AwaitReadinessEdge {
                        request,
                        readiness_activity_edges,
                        readiness_samples,
                    },
                )),
            )) => PhyColdObservationRequest::ObserveSignalPowerReadiness {
                request,
                readiness_activity_edges,
                readiness_samples,
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ReadAccumulators(request),
                )),
            )) => PhyColdObservationRequest::ReadSignalPowerAccumulators(request),
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            request,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn request(&self) -> PhyColdObservationRequest {
        self.request
    }

    pub fn into_completion(
        self,
        result: PhyColdObservationResult,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match (self.outer_action, result) {
            (
                PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse),
                PhyColdObservationResult::OpenI2cPowerAndPulse { started_at_cycle },
            ) => Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured { started_at_cycle },
            )),
            (
                PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline { .. }),
                PhyColdObservationResult::OpenI2cSdmDeadline { expired },
            ) => Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired },
            )),
            (
                PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode),
                PhyColdObservationResult::PbusWorkMode { settle_required },
            ) => Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::WorkModeConfigured { settle_required },
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
                )),
                PhyColdObservationResult::PbusWorkMode { settle_required },
            ) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModeConfigured { settle_required },
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::ReadPbus { selector, path },
                    )),
                )),
                PhyColdObservationResult::RxDcoPbusRead {
                    selector: completed_selector,
                    path: completed_path,
                    value,
                },
            ) if selector == completed_selector && path == completed_path => {
                Ok(PhyRfInitPrefixCompletion::XtalDuty(
                    XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                        XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusRead {
                            selector,
                            path,
                            value,
                        }),
                    )),
                ))
            }
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::DcIq(PhyDcIqAction::AwaitReadinessEdge { request, .. }),
                    )),
                )),
                PhyColdObservationResult::DcIqReadiness {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessObserved { request, snapshot },
                    )),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::DcIq(PhyDcIqAction::ReadAccumulators(request)),
                    )),
                )),
                PhyColdObservationResult::DcIqAccumulators {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::AccumulatorsRead { request, snapshot },
                    )),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                        PhySignalPowerAction::AwaitReadinessEdge { request, .. },
                    )),
                )),
                PhyColdObservationResult::SignalPowerReadiness {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ReadinessObserved { request, snapshot },
                    ),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                        PhySignalPowerAction::ReadAccumulators(request),
                    )),
                )),
                PhyColdObservationResult::SignalPowerAccumulators {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::AccumulatorsRead { request, snapshot },
                    ),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }

    /// Consume an independently owned Rust deadline for a readiness action.
    ///
    /// Ordinary sampled observations cannot fabricate a timeout. Conversely,
    /// only the two readiness actions accept this completion; fixed MMIO
    /// samples and the open-I2C deadline fail closed.
    pub fn into_timeout_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::AwaitReadinessEdge { request, .. },
                ))),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessTimedOut(request),
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::AwaitReadinessEdge { request, .. },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ReadinessTimedOut(request),
                    ),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<R: open_esp_radio_esp32s31_hal::SharedPhyContext>(
        self,
        registers: &mut R,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.request == PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse {
            crate::phy_i2c::configure_open_i2c_power_and_pulse(registers);
            let started_at_cycle =
                open_esp_radio_esp32s31_hal::phy_prelude::sample_sdm_deadline_counter(registers);
            return self.into_completion(PhyColdObservationResult::OpenI2cPowerAndPulse {
                started_at_cycle,
            });
        }

        match self.request {
            // Consumed by the semantic shared-PHY branch above.
            PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse => {
                Err(PhyColdLoweringError::UnexpectedOutcome)
            }
            PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            } => {
                let current_cycle =
                    open_esp_radio_esp32s31_hal::phy_prelude::sample_sdm_deadline_counter(
                        registers,
                    );
                self.into_completion(PhyColdObservationResult::OpenI2cSdmDeadline {
                    expired: phy_sdm_deadline_expired(
                        started_at_cycle,
                        current_cycle,
                        maximum_cycles,
                    ),
                })
            }
            PhyColdObservationRequest::ConfigurePbusWorkMode => {
                let settle_required =
                    open_esp_radio_esp32s31_hal::pbus::configure_work_mode(registers);
                self.into_completion(PhyColdObservationResult::PbusWorkMode { settle_required })
            }
            PhyColdObservationRequest::ReadRxDcoPbus { selector, path } => {
                let value = u32::from(
                    open_esp_radio_esp32s31_hal::pbus::read_result(registers, selector, path)
                        .ok_or(PhyColdLoweringError::UnexpectedOutcome)?,
                );
                self.into_completion(PhyColdObservationResult::RxDcoPbusRead {
                    selector,
                    path,
                    value,
                })
            }
            PhyColdObservationRequest::ObserveDcIqReadiness { request, .. } => {
                let snapshot = crate::phy_dc_iq::sample_readiness_target(registers);
                self.into_completion(PhyColdObservationResult::DcIqReadiness { request, snapshot })
            }
            PhyColdObservationRequest::ReadDcIqAccumulators(request) => {
                let snapshot = crate::phy_dc_iq::read_accumulators_target(registers);
                self.into_completion(PhyColdObservationResult::DcIqAccumulators {
                    request,
                    snapshot,
                })
            }
            PhyColdObservationRequest::ObserveSignalPowerReadiness { request, .. } => {
                let snapshot = crate::phy_dc_iq::sample_readiness_target(registers);
                self.into_completion(PhyColdObservationResult::SignalPowerReadiness {
                    request,
                    snapshot,
                })
            }
            PhyColdObservationRequest::ReadSignalPowerAccumulators(request) => {
                let snapshot =
                    open_esp_radio_esp32s31_hal::phy_iq_estimator::read_signal_power(registers);
                self.into_completion(PhyColdObservationResult::SignalPowerAccumulators {
                    request,
                    snapshot: crate::phy_signal_power::PhySignalPowerAccumulatorSnapshot {
                        sum_i: snapshot.sum_i,
                        difference_i: snapshot.difference_i,
                        difference_q: snapshot.difference_q,
                        sum_q: snapshot.sum_q,
                    },
                })
            }
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
const fn phy_sdm_deadline_expired(
    started_at_cycle: u32,
    current_cycle: u32,
    maximum_cycles: u32,
) -> bool {
    current_cycle.wrapping_sub(started_at_cycle) > maximum_cycles
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusAction {
    Start(PhyPbusForceTest),
    AwaitCompletionEdge(PhyPbusForceTest),
    Complete(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusObservation {
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusHardwareResult {
    Busy,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusError {
    WrongEdge,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyColdPbusPhase {
    Start,
    AwaitCompletionEdge,
    Complete,
}

/// One uniquely owned PBus command and its independently delivered edge.
///
/// `Busy` after an observation preserves `AwaitCompletionEdge`; the binding
/// does not retry, poll, or arrange another wake. An outer deadline may
/// instead consume the binding through [`Self::into_timeout_completion`].
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdPbusBinding {
    outer_action: PhyRfInitPrefixAction,
    transaction: PhyPbusForceTest,
    phase: PhyColdPbusPhase,
}

impl PhyColdPbusBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let transaction = match outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction)) => {
                transaction
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) => transaction,
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            transaction,
            phase: PhyColdPbusPhase::Start,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn action(&self) -> PhyColdPbusAction {
        match self.phase {
            PhyColdPbusPhase::Start => PhyColdPbusAction::Start(self.transaction),
            PhyColdPbusPhase::AwaitCompletionEdge => {
                PhyColdPbusAction::AwaitCompletionEdge(self.transaction)
            }
            PhyColdPbusPhase::Complete => PhyColdPbusAction::Complete(self.transaction),
        }
    }

    pub fn started(&mut self) -> Result<(), PhyColdPbusError> {
        match self.phase {
            PhyColdPbusPhase::Start => {
                self.phase = PhyColdPbusPhase::AwaitCompletionEdge;
                Ok(())
            }
            PhyColdPbusPhase::AwaitCompletionEdge => Err(PhyColdPbusError::WrongEdge),
            PhyColdPbusPhase::Complete => Err(PhyColdPbusError::AlreadyComplete),
        }
    }

    pub fn observe_result(
        &mut self,
        result: PhyColdPbusHardwareResult,
    ) -> Result<PhyColdPbusObservation, PhyColdPbusError> {
        match self.phase {
            PhyColdPbusPhase::AwaitCompletionEdge
                if result == PhyColdPbusHardwareResult::Completed =>
            {
                self.phase = PhyColdPbusPhase::Complete;
                Ok(PhyColdPbusObservation::EdgeConsumed)
            }
            PhyColdPbusPhase::AwaitCompletionEdge => Ok(PhyColdPbusObservation::StillPending),
            PhyColdPbusPhase::Start => Err(PhyColdPbusError::WrongEdge),
            PhyColdPbusPhase::Complete => Err(PhyColdPbusError::AlreadyComplete),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<(), PhyColdPbusError> {
        if self.phase != PhyColdPbusPhase::Start {
            return Err(if self.phase == PhyColdPbusPhase::Complete {
                PhyColdPbusError::AlreadyComplete
            } else {
                PhyColdPbusError::WrongEdge
            });
        }
        open_esp_radio_esp32s31_hal::pbus::start_force_test(
            registers,
            self.transaction.selector(),
            self.transaction.path(),
            self.transaction.value(),
        );
        self.started()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyColdPbusObservation, PhyColdPbusError> {
        if self.phase != PhyColdPbusPhase::AwaitCompletionEdge {
            return Err(if self.phase == PhyColdPbusPhase::Complete {
                PhyColdPbusError::AlreadyComplete
            } else {
                PhyColdPbusError::WrongEdge
            });
        }
        match open_esp_radio_esp32s31_hal::pbus::try_finish_force_test(registers) {
            Ok(()) => self.observe_result(PhyColdPbusHardwareResult::Completed),
            Err(open_esp_radio_esp32s31_hal::pbus::PbusError::Busy) => {
                self.observe_result(PhyColdPbusHardwareResult::Busy)
            }
        }
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.phase != PhyColdPbusPhase::Complete {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        }
        match self.outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction))
                if transaction == self.transaction =>
            {
                Ok(PhyRfInitPrefixCompletion::PbusClear(
                    PhyPbusClearCompletion::ForceTestCompleted(transaction),
                ))
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceCompleted(transaction),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceCompleted(
                        transaction,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceCompleted(transaction),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }

    pub fn into_timeout_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.phase != PhyColdPbusPhase::AwaitCompletionEdge {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        }
        match self.outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction))
                if transaction == self.transaction =>
            {
                Ok(PhyRfInitPrefixCompletion::PbusClear(
                    PhyPbusClearCompletion::ForceTestTimedOut(transaction),
                ))
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceTimedOut(transaction),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceTimedOut(
                        transaction,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceTimedOut(transaction),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdLocalStep {
    /// One finite state-only action was applied.  The caller may consume
    /// another bounded action in the same executor dispatch or yield.
    StateAdvanced,
    /// Hardware, timer, or observation work must be completed externally.
    External(PhyRfInitPrefixAction),
    Complete(PhyRfInitPrefixOutcome),
}

/// Error from the single-owner composition around `phy_rf_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

impl From<PhyRfInitPrefixTransitionError> for PhyColdTransitionError {
    fn from(error: PhyRfInitPrefixTransitionError) -> Self {
        match error {
            PhyRfInitPrefixTransitionError::WrongCompletion => Self::WrongCompletion,
            PhyRfInitPrefixTransitionError::AlreadyComplete => Self::AlreadyComplete,
        }
    }
}

/// `phy_rf_init` transition and its only mutable software-state owner.
///
/// [`step_local`](Self::step_local) performs at most one finite state action.
/// It never loops, polls hardware, creates a waker, or retries a busy
/// transaction.  All non-local actions are returned verbatim to the target
/// executor and require one identity-bound external completion.
pub struct PhyRfColdInit {
    state: PhyState,
    transition: PhyRfInitPrefixTransition,
}

impl PhyRfColdInit {
    pub const fn new(state: PhyState) -> Self {
        Self {
            state,
            transition: PhyRfInitPrefixTransition::new(),
        }
    }

    pub const fn state(&self) -> &PhyState {
        &self.state
    }

    pub const fn action(&self) -> PhyRfInitPrefixAction {
        self.transition.action()
    }

    pub fn step_local(&mut self) -> Result<PhyColdLocalStep, PhyColdTransitionError> {
        let action = self.transition.action();
        let completion = match action {
            PhyRfInitPrefixAction::InspectRcCalibrationState => {
                PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
                    already_complete: self.state.rc_calibration_complete(),
                }
            }
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ApplyResult(result)) => {
                self.state.apply_rc_calibration(result);
                PhyRfInitPrefixCompletion::RcCalibration(RcCalibrationCompletion::Applied)
            }
            PhyRfInitPrefixAction::CaptureFilterDcapParameters => {
                PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(
                    self.state.filter_dcap_parameters(),
                )
            }
            PhyRfInitPrefixAction::CaptureXtalDutyParameters => {
                PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(
                    self.state.xtal_duty_parameters(),
                )
            }
            PhyRfInitPrefixAction::CaptureChannelFrequencyControl => {
                PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(
                    self.state.channel_frequency_control(),
                )
            }
            PhyRfInitPrefixAction::Complete(outcome) => {
                self.state.synchronize_success(outcome);
                return Ok(PhyColdLocalStep::Complete(outcome));
            }
            external => return Ok(PhyColdLocalStep::External(external)),
        };

        self.transition.advance(completion)?;
        Ok(PhyColdLocalStep::StateAdvanced)
    }

    pub fn advance_external(
        &mut self,
        completion: PhyRfInitPrefixCompletion,
    ) -> Result<(), PhyColdTransitionError> {
        self.transition.advance(completion)?;
        if let PhyRfInitPrefixAction::Complete(outcome) = self.transition.action() {
            self.state.synchronize_success(outcome);
        }
        Ok(())
    }

    pub fn into_state(self) -> PhyState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cBinding,
        PhyColdI2cConfigurationBinding, PhyColdI2cObservation, PhyColdI2cOutcome,
        PhyColdI2cRequest, PhyColdI2cTransaction, PhyColdLoweringError, PhyColdMmioBinding,
        PhyColdObservationBinding, PhyColdObservationRequest, PhyColdObservationResult,
        PhyColdPbusAction, PhyColdPbusBinding, PhyColdPbusHardwareResult, PhyColdPbusObservation,
        PhyColdTimerBinding, phy_sdm_deadline_expired,
    };
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqDelayPhase,
        PhyDcIqEnablePhase, PhyDcIqEstimateRequest, PhyDcIqReadinessSnapshot,
    };
    use crate::phy_frequency::{PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion};
    use crate::phy_i2c::{
        FilterDcapAction, FilterDcapParameters, I2cInit1Action, OpenI2cXpdAction,
        OpenI2cXpdCompletion, PhyI2cAddress, PhyI2cError, PhyRfInitParameterSnapshot,
        PhyRfInitPrefixAction, PhyRfInitPrefixCompletion, RcCalibrationAction,
        RcCalibrationCompletion,
    };
    use crate::phy_pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest};
    use crate::phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
    use crate::phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion};
    use crate::phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerClock,
        PhySignalPowerCompletion, PhySignalPowerRequest,
    };
    use crate::phy_xtal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyPassAction,
        XtalDutyPassCompletion, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
        XtalDutyRestoreAction, XtalDutyRestoreCompletion, XtalDutySearchAction,
        XtalDutySearchCompletion,
    };
    #[test]
    fn busy_observation_preserves_await_state_without_self_progress() {
        let address = PhyI2cAddress::new(0x66, 4).unwrap();
        let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::read_byte(address));
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::StartRead { address }
        );
        transaction.read_started().unwrap();
        let awaiting = PhyColdI2cAction::AwaitReadCompletionEdge { address };
        assert_eq!(transaction.action(), awaiting);

        assert_eq!(
            transaction.observe_read_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(transaction.action(), awaiting);

        assert_eq!(
            transaction.observe_read_result(Ok(0xa5)),
            Ok(PhyColdI2cObservation::EdgeConsumed)
        );
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
                address,
                value: 0xa5,
            })
        );
    }

    #[test]
    fn masked_write_needs_two_distinct_external_edges() {
        let field = crate::phy_i2c::analog_registers::RC_CALIBRATION_ENABLE;
        let address = field.address();
        let request = PhyColdI2cRequest::write_field(field, 1);
        let mut transaction = PhyColdI2cTransaction::new(request);

        transaction.read_started().unwrap();
        transaction.observe_read_result(Ok(0xc3)).unwrap();
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::StartWrite {
                address,
                value: 0xc3,
            }
        );

        transaction.write_started().unwrap();
        assert_eq!(
            transaction.observe_write_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::AwaitWriteCompletionEdge { address }
        );
        transaction.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Written { address })
        );
    }

    #[test]
    fn masked_outer_write_is_two_edges_but_one_identity_bound_completion() {
        let field = crate::phy_i2c::analog_registers::RC_CALIBRATION_ENABLE;
        let address = field.address();
        let outer_action = PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked {
            field,
            value: 1,
        });
        let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();

        binding.read_started().unwrap();
        binding.observe_read_result(Ok(0x83)).unwrap();
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::StartWrite {
                address,
                value: 0x83,
            }
        );
        binding.write_started().unwrap();
        assert_eq!(
            binding.observe_write_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::AwaitWriteCompletionEdge { address }
        );

        binding.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Write
            ))
        );
    }

    #[test]
    fn non_i2c_outer_action_is_rejected_instead_of_becoming_a_fallback() {
        assert_eq!(
            PhyColdI2cBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn finite_mmio_binding_preserves_dynamic_frequency_identity() {
        let outer_action = PhyRfInitPrefixAction::ChannelFrequency(
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                parameter_override: true,
            },
        );
        let binding = PhyColdMmioBinding::new(outer_action).unwrap();
        assert_eq!(binding.outer_action(), outer_action);
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override: true,
                }
            ))
        );

        assert_eq!(
            PhyColdMmioBinding::new(PhyRfInitPrefixAction::DelayMicros(10)),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn nested_calibration_mmio_keeps_every_parent_identity_field() {
        let tone_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled: true,
                selector: 0x80,
                step: 0,
            }),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(tone_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::CalibrationToneConfigured {
                        enabled: true,
                        selector: 0x80,
                        step: 0,
                    }
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 4,
            chain: 1,
            control: 0x1234,
            mode: 2,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::SetEnable {
                    request: dc_iq_request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ))),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(dc_iq_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::EnableSet {
                            request: dc_iq_request,
                            phase: PhyDcIqEnablePhase::Measurement,
                            enabled: true,
                        }
                    ))
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x3a7,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureClock {
                    request: signal_request,
                    clock: PhySignalPowerClock::Rx,
                    enabled: false,
                },
            )),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(signal_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ClockConfigured {
                            request: signal_request,
                            clock: PhySignalPowerClock::Rx,
                            enabled: false,
                        }
                    )
                ))
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(restore_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModePulseConfigured
                ))
            ))
        );
    }

    #[test]
    fn timer_binding_consumes_one_exact_delay_edge() {
        let outer_action =
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(100));
        let binding = PhyColdTimerBinding::new(outer_action).unwrap();
        assert_eq!(binding.outer_action(), outer_action);
        assert_eq!(binding.micros(), 100);
        assert_eq!(
            binding.into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Delay
            ))
        );

        assert_eq!(
            PhyColdTimerBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn nested_calibration_timers_preserve_every_parent_identity_field() {
        let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::DelayMicros(20),
            )),
        ));
        let rfpll = PhyColdTimerBinding::new(rfpll_action).unwrap();
        assert_eq!(rfpll.micros(), 20);
        assert_eq!(
            rfpll.into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(20))
                ))
            ))
        );

        let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                PhyRxDcoAction::DelayMicros {
                    iteration: 7,
                    micros: 10,
                },
            )),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(rx_dco_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DelayElapsed {
                        iteration: 7,
                        micros: 10,
                    })
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 7,
            chain: 1,
            control: 0x1234,
            mode: 2,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::DelayMicros {
                    request: dc_iq_request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ))),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(dc_iq_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::DelayElapsed {
                            request: dc_iq_request,
                            phase: PhyDcIqDelayPhase::Stop,
                            micros: 1,
                        }
                    ))
                ))
            ))
        );

        let search_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros {
                candidate: 0x3a,
                micros: 20,
            }),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(search_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::DelayElapsed { candidate: 0x3a }
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x3a7,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::DelayMicros {
                    request: signal_request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            )),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(signal_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::DelayElapsed {
                        request: signal_request,
                        phase: PhyDcIqDelayPhase::Start,
                        micros: 1,
                    })
                ))
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(2)),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(restore_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::DelayElapsed { micros: 2 }
                ))
            ))
        );
    }

    #[test]
    fn pbus_busy_result_preserves_one_owned_awaiting_edge() {
        let transaction = PhyPbusForceTest::new(4, 1, 0);
        let outer_action =
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
        let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
        assert_eq!(binding.action(), PhyColdPbusAction::Start(transaction));

        binding.started().unwrap();
        let awaiting = PhyColdPbusAction::AwaitCompletionEdge(transaction);
        assert_eq!(binding.action(), awaiting);
        assert_eq!(
            binding.observe_result(PhyColdPbusHardwareResult::Busy),
            Ok(PhyColdPbusObservation::StillPending)
        );
        assert_eq!(binding.action(), awaiting);

        assert_eq!(
            binding.observe_result(PhyColdPbusHardwareResult::Completed),
            Ok(PhyColdPbusObservation::EdgeConsumed)
        );
        assert_eq!(binding.action(), PhyColdPbusAction::Complete(transaction));
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::ForceTestCompleted(transaction)
            ))
        );
    }

    #[test]
    fn pbus_timeout_consumes_the_exact_awaiting_transaction() {
        let transaction = PhyPbusForceTest::new(3, 2, 0x100);
        let outer_action =
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
        let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
        binding.started().unwrap();
        assert_eq!(
            binding.into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::ForceTestTimedOut(transaction)
            ))
        );
    }

    #[test]
    fn nested_xtal_pbus_edges_return_to_the_exact_parent_transition() {
        let prepare_transaction = PhyPbusForceTest::new(0, 2, 0x42);
        let prepare_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(prepare_transaction)),
        ));
        let mut prepare = PhyColdPbusBinding::new(prepare_action).unwrap();
        prepare.started().unwrap();
        prepare
            .observe_result(PhyColdPbusHardwareResult::Completed)
            .unwrap();
        assert_eq!(
            prepare.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceCompleted(prepare_transaction)
                ))
            ))
        );

        let rx_dco_transaction = PhyPbusForceTest::new(3, 1, 0x1ff);
        let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ForcePbus(
                rx_dco_transaction,
            ))),
        ));
        let mut rx_dco = PhyColdPbusBinding::new(rx_dco_action).unwrap();
        rx_dco.started().unwrap();
        assert_eq!(
            rx_dco.into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceTimedOut(
                        rx_dco_transaction
                    ))
                ))
            ))
        );

        let restore_transaction = PhyPbusForceTest::new(1, 2, 0);
        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(restore_transaction)),
        ));
        let mut restore = PhyColdPbusBinding::new(restore_action).unwrap();
        restore.started().unwrap();
        restore
            .observe_result(PhyColdPbusHardwareResult::Completed)
            .unwrap();
        assert_eq!(
            restore.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceCompleted(restore_transaction)
                ))
            ))
        );
    }

    #[test]
    fn sampled_pbus_work_mode_is_bound_to_its_exact_parent() {
        let clear_action = PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode);
        let clear = PhyColdObservationBinding::new(clear_action).unwrap();
        assert_eq!(clear.outer_action(), clear_action);
        assert_eq!(
            clear.request(),
            PhyColdObservationRequest::ConfigurePbusWorkMode
        );
        assert_eq!(
            clear.into_completion(PhyColdObservationResult::PbusWorkMode {
                settle_required: true,
            }),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: true
                }
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
        ));
        let restore = PhyColdObservationBinding::new(restore_action).unwrap();
        assert_eq!(
            restore.into_completion(PhyColdObservationResult::PbusWorkMode {
                settle_required: false,
            }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                        settle_required: false
                    }
                ))
            ))
        );
    }

    #[test]
    fn open_i2c_deadline_keeps_one_epoch_and_the_inclusive_rom_bound() {
        assert!(!phy_sdm_deadline_expired(100, 10_099, 9_999));
        assert!(phy_sdm_deadline_expired(100, 10_100, 9_999));
        assert!(!phy_sdm_deadline_expired(0xffff_ff00, 0x0000_260f, 9_999));
        assert!(phy_sdm_deadline_expired(0xffff_ff00, 0x0000_2610, 9_999));

        let configure_action =
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse);
        let configure = PhyColdObservationBinding::new(configure_action).unwrap();
        assert_eq!(
            configure.request(),
            PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse
        );
        assert_eq!(
            configure.into_completion(PhyColdObservationResult::OpenI2cPowerAndPulse {
                started_at_cycle: 0xffff_ff00,
            }),
            Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured {
                    started_at_cycle: 0xffff_ff00
                }
            ))
        );

        let deadline_action =
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle: 0xffff_ff00,
                maximum_cycles: 9_999,
            });
        let deadline = PhyColdObservationBinding::new(deadline_action).unwrap();
        assert_eq!(
            deadline.request(),
            PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle: 0xffff_ff00,
                maximum_cycles: 9_999,
            }
        );
        assert_eq!(
            deadline
                .into_completion(PhyColdObservationResult::OpenI2cSdmDeadline { expired: false }),
            Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired: false }
            ))
        );
    }

    #[test]
    fn nested_external_edges_are_one_shot_semantic_bindings() {
        let prepare_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::PrepareRxDcoControlRestore),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(prepare_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDcoControlRestorePrepared
                ))
            ))
        );

        let pbus_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ReadPbus {
                selector: 1,
                path: 2,
            })),
        ));
        assert_eq!(
            PhyColdObservationBinding::new(pbus_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::RxDcoPbusRead {
                    selector: 1,
                    path: 2,
                    value: 0x1a5,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusRead {
                        selector: 1,
                        path: 2,
                        value: 0x1a5,
                    })
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 6,
            chain: 1,
            control: 0x0fa0,
            mode: 0,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::AwaitReadinessEdge {
                    request: dc_iq_request,
                    readiness_activity_edges: 3,
                    readiness_samples: 5,
                },
            ))),
        ));
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::DcIqReadiness {
                    request: dc_iq_request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: false,
                        activity: true,
                    },
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessObserved {
                            request: dc_iq_request,
                            snapshot: PhyDcIqReadinessSnapshot {
                                ready: false,
                                activity: true,
                            },
                        }
                    ))
                ))
            ))
        );
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_action)
                .unwrap()
                .into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessTimedOut(dc_iq_request)
                    ))
                ))
            ))
        );

        let dc_iq_accumulators = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::ReadAccumulators(dc_iq_request),
            ))),
        ));
        let dc_iq_snapshot = PhyDcIqAccumulatorSnapshot {
            i: -3,
            q: 7,
            power: 0x1234,
        };
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_accumulators)
                .unwrap()
                .into_completion(PhyColdObservationResult::DcIqAccumulators {
                    request: dc_iq_request,
                    snapshot: dc_iq_snapshot,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::AccumulatorsRead {
                            request: dc_iq_request,
                            snapshot: dc_iq_snapshot,
                        }
                    ))
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x25,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ReadAccumulators(signal_request),
            )),
        ));
        let signal_snapshot = PhySignalPowerAccumulatorSnapshot {
            sum_i: 10,
            difference_i: -20,
            difference_q: 30,
            sum_q: -40,
        };
        assert_eq!(
            PhyColdObservationBinding::new(signal_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::SignalPowerAccumulators {
                    request: signal_request,
                    snapshot: signal_snapshot,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::AccumulatorsRead {
                            request: signal_request,
                            snapshot: signal_snapshot,
                        }
                    )
                ))
            ))
        );
    }

    #[test]
    fn external_lowering_has_no_vendor_or_synchronous_fallback_variant() {
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::DelayMicros(10)),
            Ok(PhyColdExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureFrontEndRegisters),
            Ok(PhyColdExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureBiasRegisters),
            Ok(PhyColdExternalBinding::I2cConfiguration(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureRcCalibrationSettings),
            Ok(PhyColdExternalBinding::I2cConfiguration(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureSar2),
            Ok(PhyColdExternalBinding::I2cConfiguration(_))
        ));

        let address = PhyI2cAddress::new(0x62, 1).unwrap();
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ReadParameter18e { address }),
            Ok(PhyColdExternalBinding::I2c(_))
        ));
        let filter_parameters = FilterDcapParameters::new(1, 2, 3, 4, 5);
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::FilterDcap(
                FilterDcapAction::Configure(filter_parameters)
            )),
            Ok(PhyColdExternalBinding::I2cConfiguration(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::I2cInit1(
                I2cInit1Action::Configure(PhyRfInitParameterSnapshot::new(filter_parameters, 6))
            )),
            Ok(PhyColdExternalBinding::I2cConfiguration(_))
        ));
        let transaction = PhyPbusForceTest::new(4, 1, 0);
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
                PhyPbusClearAction::ForceTest(transaction)
            )),
            Ok(PhyColdExternalBinding::Pbus(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
                PhyPbusClearAction::ConfigureWorkMode
            )),
            Ok(PhyColdExternalBinding::Observation(_))
        ));
        assert_eq!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::CaptureFilterDcapParameters),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn i2c_configuration_binding_requires_the_complete_pac_transaction() {
        let parameters = FilterDcapParameters::new(1, 2, 3, 4, 5);
        let binding = PhyColdI2cConfigurationBinding::new(PhyRfInitPrefixAction::FilterDcap(
            FilterDcapAction::Configure(parameters),
        ))
        .unwrap();
        assert_eq!(
            binding.action(),
            open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cConfigurationAction::StartCommand
        );
        assert_eq!(
            binding.into_completion(),
            Err(PhyColdLoweringError::IncompleteTransaction)
        );
    }

    #[test]
    fn channel_frequency_i2c_completion_keeps_its_field_identity() {
        let field = crate::phy_i2c::analog_registers::RFPLL_SDM_LOW;
        let outer_action =
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
                field,
                value: 0x12,
            });
        let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();
        binding.read_started().unwrap();
        binding.observe_read_result(Ok(0x05)).unwrap();
        binding.write_started().unwrap();
        binding.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::MaskedWrite { field }
            ))
        );
    }

    #[test]
    fn xtal_and_rfpll_i2c_edges_keep_nested_identity() {
        let initial_field = crate::phy_i2c::analog_registers::XTAL_DUTY_INITIAL;
        let initial_action =
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
                field: initial_field,
            });
        let mut initial = PhyColdI2cBinding::new(initial_action).unwrap();
        initial.read_started().unwrap();
        initial.observe_read_result(Ok(0xeb)).unwrap();
        assert_eq!(
            initial.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::InitialDutyRead {
                    field: initial_field,
                    value: 0x2b,
                }
            ))
        );

        let rfpll_field = crate::phy_i2c::analog_registers::RFPLL_SDM_LOW;
        let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteMasked {
                    field: rfpll_field,
                    value: 0x12,
                },
            )),
        ));
        let mut rfpll = PhyColdI2cBinding::new(rfpll_action).unwrap();
        rfpll.read_started().unwrap();
        rfpll.observe_read_result(Ok(0x05)).unwrap();
        rfpll.write_started().unwrap();
        rfpll.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            rfpll.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                        field: rfpll_field,
                    })
                ))
            ))
        );

        let candidate_address = PhyI2cAddress::new(0x61, 0x0a).unwrap();
        let candidate_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                address: candidate_address,
                candidate: 0x3a,
            }),
        ));
        let mut candidate = PhyColdI2cBinding::new(candidate_action).unwrap();
        candidate.write_started().unwrap();
        candidate.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            candidate.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::CandidateWritten {
                        address: candidate_address,
                        candidate: 0x3a,
                    }
                ))
            ))
        );
    }
}
