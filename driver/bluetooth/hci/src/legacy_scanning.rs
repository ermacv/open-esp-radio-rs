//! Semantic HCI boundary for legacy passive scanning.
//!
//! Standard `bt-hci` parameter types are decoded into owned Controller intent.
//! This module does not retain configuration, start a Link Layer window,
//! publish advertising reports, or claim radio progress.

use bt_hci::{
    FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{LeSetScanEnable, LeSetScanEnableParams, LeSetScanParams, LeSetScanParamsParams},
    },
    event::{EventKind, le::LeAdvertisingReport, le::LeEventParams},
    param::{
        AddrKind, BdAddr, Error as HciError, LeAdvEventKind, LeScanKind, ScanningFilterPolicy,
        Status,
    },
};

use crate::{BootstrapPhase, HciCommandPacket, HciControllerResponse};

/// Complete Command Complete event size for this command family.
pub const LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;
/// Largest complete LE Advertising Report event emitted by this profile.
pub const LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY: usize = 45;

const LEGACY_SCAN_MIN_UNITS: u16 = 0x0004;
const LEGACY_SCAN_MAX_UNITS: u16 = 0x4000;
const LEGACY_ADVERTISING_REPORT_DATA_CAPACITY: usize = 31;

/// Closed identity of the standard legacy scanning commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningCommandKind {
    /// LE Set Scan Parameters.
    SetParameters,
    /// LE Set Scan Enable.
    SetEnable,
}

impl LeLegacyScanningCommandKind {
    /// Classify an opcode without decoding its parameters.
    pub const fn from_opcode(opcode: Opcode) -> Option<Self> {
        let raw = opcode.to_raw();
        if raw == LeSetScanParams::OPCODE.to_raw() {
            Some(Self::SetParameters)
        } else if raw == LeSetScanEnable::OPCODE.to_raw() {
            Some(Self::SetEnable)
        } else {
            None
        }
    }

    /// Exact standard HCI opcode.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::SetParameters => LeSetScanParams::OPCODE,
            Self::SetEnable => LeSetScanEnable::OPCODE,
        }
    }
}

/// Validated passive LE 1M scan timing supplied by the Host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyPassiveScanParameters {
    interval_units_625_us: u16,
    window_units_625_us: u16,
}

impl LeLegacyPassiveScanParameters {
    /// Start-to-start scan interval in 0.625 ms units.
    pub const fn interval_units_625_us(self) -> u16 {
        self.interval_units_625_us
    }

    /// Receive-window duration in 0.625 ms units.
    pub const fn window_units_625_us(self) -> u16 {
        self.window_units_625_us
    }
}

/// Host-selected report duplicate policy for one enabled scan session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningDuplicatePolicy {
    /// Publish every accepted report.
    ReportAll,
    /// Suppress exact duplicates within the enabled scan session.
    FilterDuplicates,
}

/// One decoded Set Scan Enable command awaiting lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningEnableCommand {
    enable: bool,
    duplicate_policy: LeLegacyScanningDuplicatePolicy,
}

impl LeLegacyScanningEnableCommand {
    /// Whether the Host requested entry into the enabled lifecycle.
    pub const fn enable(self) -> bool {
        self.enable
    }

    /// Duplicate policy selected for this Enable transaction.
    pub const fn duplicate_policy(self) -> LeLegacyScanningDuplicatePolicy {
        self.duplicate_policy
    }
}

/// One fully decoded standard command awaiting Controller policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningCommand {
    /// Replace passive scan parameters while the scanner is disabled.
    SetParameters(LeLegacyPassiveScanParameters),
    /// Start or stop passive scanning.
    SetEnable(LeLegacyScanningEnableCommand),
}

/// Configuration-only command which can complete without starting hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningConfigurationCommand {
    parameters: LeLegacyPassiveScanParameters,
}

impl LeLegacyScanningConfigurationCommand {
    /// Extract Set Parameters from the full scanning command family.
    pub fn from_command(command: LeLegacyScanningCommand) -> Result<Self, LeLegacyScanningCommand> {
        match command {
            LeLegacyScanningCommand::SetParameters(parameters) => Ok(Self { parameters }),
            command => Err(command),
        }
    }

    /// Validated parameter value retained by this command.
    pub const fn parameters(self) -> LeLegacyPassiveScanParameters {
        self.parameters
    }

    pub(crate) fn into_active_session_command_complete(
        self,
    ) -> LeLegacyScanningCommandCompleteEvent {
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetParameters.opcode(),
            HciError::CMD_DISALLOWED.to_status(),
        )
    }
}

/// Immutable Host configuration snapshot retained from Enable until radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningEnableRequest {
    parameters: LeLegacyPassiveScanParameters,
    duplicate_policy: LeLegacyScanningDuplicatePolicy,
}

impl LeLegacyScanningEnableRequest {
    /// Exact accepted passive scan timing.
    pub const fn parameters(self) -> LeLegacyPassiveScanParameters {
        self.parameters
    }

    /// Duplicate policy selected by the accepted Enable command.
    pub const fn duplicate_policy(self) -> LeLegacyScanningDuplicatePolicy {
        self.duplicate_policy
    }
}

pub(crate) enum LeLegacyScanningIdleEnableDisposition {
    Start(LeLegacyScanningEnableRequest),
    Complete(LeLegacyScanningCommandCompleteEvent),
}

pub(crate) enum LeLegacyScanningActiveEnableDisposition {
    Disable(LeLegacyScanningEnableCommand),
    Complete(LeLegacyScanningCommandCompleteEvent),
}

impl LeLegacyScanningEnableCommand {
    pub(crate) fn into_started_command_complete(self) -> LeLegacyScanningCommandCompleteEvent {
        debug_assert!(self.enable);
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetEnable.opcode(),
            Status::SUCCESS,
        )
    }

    pub(crate) fn into_hardware_failure_command_complete(
        self,
    ) -> LeLegacyScanningCommandCompleteEvent {
        debug_assert!(self.enable);
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetEnable.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }

    pub(crate) fn into_active_session_disposition(self) -> LeLegacyScanningActiveEnableDisposition {
        if self.enable {
            LeLegacyScanningActiveEnableDisposition::Complete(
                LeLegacyScanningCommandCompleteEvent::new(
                    LeLegacyScanningCommandKind::SetEnable.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                ),
            )
        } else {
            LeLegacyScanningActiveEnableDisposition::Disable(self)
        }
    }

    pub(crate) fn into_stopped_command_complete(self) -> LeLegacyScanningCommandCompleteEvent {
        debug_assert!(!self.enable);
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetEnable.opcode(),
            Status::SUCCESS,
        )
    }
}

/// Reset-scoped software configuration for the passive scanner.
pub(crate) struct LeLegacyScanningConfiguration {
    parameters: Option<LeLegacyPassiveScanParameters>,
}

impl LeLegacyScanningConfiguration {
    pub(crate) const fn new() -> Self {
        Self { parameters: None }
    }

    pub(crate) fn dispatch(
        &mut self,
        phase: BootstrapPhase,
        command: LeLegacyScanningConfigurationCommand,
    ) -> LeLegacyScanningCommandCompleteEvent {
        if phase == BootstrapPhase::AwaitingReset {
            return LeLegacyScanningCommandCompleteEvent::new(
                LeLegacyScanningCommandKind::SetParameters.opcode(),
                HciError::CMD_DISALLOWED.to_status(),
            );
        }
        self.parameters = Some(command.parameters());
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetParameters.opcode(),
            Status::SUCCESS,
        )
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn dispatch_idle_enable(
        &self,
        phase: BootstrapPhase,
        command: LeLegacyScanningEnableCommand,
    ) -> LeLegacyScanningIdleEnableDisposition {
        if phase == BootstrapPhase::AwaitingReset {
            return LeLegacyScanningIdleEnableDisposition::Complete(
                LeLegacyScanningCommandCompleteEvent::new(
                    LeLegacyScanningCommandKind::SetEnable.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                ),
            );
        }
        if !command.enable() {
            return LeLegacyScanningIdleEnableDisposition::Complete(
                LeLegacyScanningCommandCompleteEvent::new(
                    LeLegacyScanningCommandKind::SetEnable.opcode(),
                    Status::SUCCESS,
                ),
            );
        }
        let Some(parameters) = self.parameters else {
            return LeLegacyScanningIdleEnableDisposition::Complete(
                LeLegacyScanningCommandCompleteEvent::new(
                    LeLegacyScanningCommandKind::SetEnable.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                ),
            );
        };
        LeLegacyScanningIdleEnableDisposition::Start(LeLegacyScanningEnableRequest {
            parameters,
            duplicate_policy: command.duplicate_policy(),
        })
    }

    pub(crate) fn complete_enable_while_radio_unavailable(
        phase: BootstrapPhase,
        command: LeLegacyScanningEnableCommand,
    ) -> LeLegacyScanningCommandCompleteEvent {
        let status = if phase == BootstrapPhase::AwaitingReset || command.enable() {
            HciError::CMD_DISALLOWED.to_status()
        } else {
            Status::SUCCESS
        };
        LeLegacyScanningCommandCompleteEvent::new(
            LeLegacyScanningCommandKind::SetEnable.opcode(),
            status,
        )
    }

    #[cfg(test)]
    pub(crate) const fn parameters(&self) -> Option<LeLegacyPassiveScanParameters> {
        self.parameters
    }
}

impl LeLegacyScanningCommand {
    /// Decode one command without mutating HCI, Link Layer, or hardware state.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLegacyScanningDecodeError> {
        let Some(kind) = LeLegacyScanningCommandKind::from_opcode(command.opcode()) else {
            return Err(LeLegacyScanningDecodeError::UnsupportedOpcode {
                opcode: command.opcode(),
            });
        };

        match kind {
            LeLegacyScanningCommandKind::SetParameters => {
                let parameters = LeSetScanParamsParams::from_hci_bytes_complete(
                    command.parameters(),
                )
                .map_err(|_| LeLegacyScanningDecodeError::MalformedParameters { command: kind })?;
                Self::decode_parameters(parameters)
            }
            LeLegacyScanningCommandKind::SetEnable => {
                let parameters = LeSetScanEnableParams::from_hci_bytes_complete(
                    command.parameters(),
                )
                .map_err(|_| LeLegacyScanningDecodeError::MalformedParameters { command: kind })?;
                Ok(Self::SetEnable(LeLegacyScanningEnableCommand {
                    enable: parameters.enable,
                    duplicate_policy: if parameters.filter_duplicates {
                        LeLegacyScanningDuplicatePolicy::FilterDuplicates
                    } else {
                        LeLegacyScanningDuplicatePolicy::ReportAll
                    },
                }))
            }
        }
    }

    fn decode_parameters(
        parameters: LeSetScanParamsParams,
    ) -> Result<Self, LeLegacyScanningDecodeError> {
        let command = LeLegacyScanningCommandKind::SetParameters;
        let LeSetScanParamsParams {
            le_scan_kind,
            le_scan_interval,
            le_scan_window,
            own_addr_kind,
            scanning_filter_policy,
        } = parameters;

        if le_scan_kind != LeScanKind::Passive
            || own_addr_kind != AddrKind::PUBLIC
            || scanning_filter_policy != ScanningFilterPolicy::BasicUnfiltered
        {
            return Err(LeLegacyScanningDecodeError::UnsupportedFeature { command });
        }

        let interval_units_625_us = le_scan_interval.as_u16();
        let window_units_625_us = le_scan_window.as_u16();
        if !(LEGACY_SCAN_MIN_UNITS..=LEGACY_SCAN_MAX_UNITS).contains(&interval_units_625_us)
            || !(LEGACY_SCAN_MIN_UNITS..=LEGACY_SCAN_MAX_UNITS).contains(&window_units_625_us)
            || window_units_625_us > interval_units_625_us
        {
            return Err(LeLegacyScanningDecodeError::InvalidParameters { command });
        }

        Ok(Self::SetParameters(LeLegacyPassiveScanParameters {
            interval_units_625_us,
            window_units_625_us,
        }))
    }

    /// Exact command identity retained by this semantic token.
    pub const fn kind(&self) -> LeLegacyScanningCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyScanningCommandKind::SetParameters,
            Self::SetEnable(_) => LeLegacyScanningCommandKind::SetEnable,
        }
    }
}

/// Why a packet could not become an owned semantic scanning command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningDecodeError {
    /// The opcode does not belong to this command family.
    UnsupportedOpcode {
        /// Unclaimed opcode.
        opcode: Opcode,
    },
    /// `bt-hci` rejected the standard parameter encoding.
    MalformedParameters {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
    /// A standard value violated the Core scan timing constraints.
    InvalidParameters {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
    /// A valid standard command selected a role not closed by this Controller.
    UnsupportedFeature {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
}

impl LeLegacyScanningDecodeError {
    /// Convert a rejection for a known opcode into the required completion.
    pub fn into_command_complete(self) -> Result<LeLegacyScanningCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::MalformedParameters { command } | Self::InvalidParameters { command } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedFeature { command } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeLegacyScanningCommandCompleteEvent::new(
            command.opcode(),
            status,
        ))
    }
}

/// Owned Command Complete for one legacy scanning command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningCommandCompleteEvent {
    bytes: [u8; LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl LeLegacyScanningCommandCompleteEvent {
    pub(crate) fn new(opcode: Opcode, status: Status) -> Self {
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                0x0e,
                0x04,
                0x01,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
            ],
            opcode,
            status,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Exact consumed command opcode.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Result selected by the semantic/controller boundary.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeLegacyScanningCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Why one portable report cannot be represented by the legacy HCI event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingReportEventError {
    /// Legacy advertising data cannot exceed 31 octets.
    DataTooLong {
        /// Supplied advertising data length.
        length: usize,
    },
    /// The legacy report supports only public and random advertiser addresses.
    UnsupportedAddressKind(AddrKind),
}

/// One owned standard LE Advertising Report event containing exactly one report.
///
/// `bt-hci` 0.10 exposes parsing but not construction for Controller events.
/// This bounded owner therefore accepts only `bt-hci` field-domain types and
/// is regression-decoded through its standard event model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingReportEvent {
    bytes: [u8; LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY],
    length: u8,
}

impl LeLegacyAdvertisingReportEvent {
    /// Build one complete LE Meta event without an H4 packet indicator.
    pub fn new(
        event_kind: LeAdvEventKind,
        address_kind: AddrKind,
        address: BdAddr,
        data: &[u8],
        rssi_dbm: i8,
    ) -> Result<Self, LeLegacyAdvertisingReportEventError> {
        if data.len() > LEGACY_ADVERTISING_REPORT_DATA_CAPACITY {
            return Err(LeLegacyAdvertisingReportEventError::DataTooLong { length: data.len() });
        }
        if address_kind != AddrKind::PUBLIC && address_kind != AddrKind::RANDOM {
            return Err(LeLegacyAdvertisingReportEventError::UnsupportedAddressKind(
                address_kind,
            ));
        }

        let length = 14 + data.len();
        let parameter_length = length - 2;
        let mut bytes = [0; LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = parameter_length as u8;
        bytes[2] = LeAdvertisingReport::SUBEVENT_CODE;
        bytes[3] = 1;
        bytes[4] = event_kind as u8;
        bytes[5] = address_kind.as_raw();
        bytes[6..12].copy_from_slice(address.raw());
        bytes[12] = data.len() as u8;
        bytes[13..13 + data.len()].copy_from_slice(data);
        bytes[13 + data.len()] = rssi_dbm.to_le_bytes()[0];
        Ok(Self {
            bytes,
            length: length as u8,
        })
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

impl HciControllerResponse for LeLegacyAdvertisingReportEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        FromHciBytes, WriteHci,
        cmd::{
            Cmd,
            le::{LeSetScanEnable, LeSetScanParams},
        },
        event::{CommandComplete, CommandCompleteWithStatus, Event, le::LeEvent},
        param::{
            AddrKind, BdAddr, Duration, Error as HciError, LeAdvEventKind, LeScanKind,
            ScanningFilterPolicy, Status,
        },
    };

    use super::{
        LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError,
        LeLegacyScanningCommand, LeLegacyScanningConfiguration,
        LeLegacyScanningConfigurationCommand, LeLegacyScanningDecodeError,
        LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableCommand,
        LeLegacyScanningIdleEnableDisposition,
    };
    use crate::{BootstrapPhase, HciCommandPacket};

    fn decode<C>(command: &C) -> Result<LeLegacyScanningCommand, LeLegacyScanningDecodeError>
    where
        C: Cmd,
    {
        let mut encoded = [0_u8; 16];
        let length = command.params().size();
        command
            .params()
            .write_hci(&mut &mut encoded[..length])
            .expect("the standard parameters fit their declared size");
        LeLegacyScanningCommand::decode(HciCommandPacket::for_test(C::OPCODE, &encoded[..length]))
    }

    #[test]
    fn standard_passive_parameters_become_owned_timing() {
        let command = LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(0x20),
            Duration::from_u16(0x10),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::BasicUnfiltered,
        );
        let LeLegacyScanningCommand::SetParameters(parameters) =
            decode(&command).expect("the supported standard parameters decode")
        else {
            panic!("parameters changed semantic command kind");
        };
        assert_eq!(parameters.interval_units_625_us(), 0x20);
        assert_eq!(parameters.window_units_625_us(), 0x10);
    }

    #[test]
    fn standard_enable_retains_duplicate_policy() {
        for (filter_duplicates, expected) in [
            (false, LeLegacyScanningDuplicatePolicy::ReportAll),
            (true, LeLegacyScanningDuplicatePolicy::FilterDuplicates),
        ] {
            let LeLegacyScanningCommand::SetEnable(enable) =
                decode(&LeSetScanEnable::new(true, filter_duplicates))
                    .expect("the standard Enable command decodes")
            else {
                panic!("Enable changed semantic command kind");
            };
            assert!(enable.enable());
            assert_eq!(enable.duplicate_policy(), expected);
        }
    }

    #[test]
    fn unsupported_profiles_and_invalid_timing_fail_closed() {
        for command in [
            LeSetScanParams::new(
                LeScanKind::Active,
                Duration::from_u16(0x20),
                Duration::from_u16(0x10),
                AddrKind::PUBLIC,
                ScanningFilterPolicy::BasicUnfiltered,
            ),
            LeSetScanParams::new(
                LeScanKind::Passive,
                Duration::from_u16(0x20),
                Duration::from_u16(0x10),
                AddrKind::RANDOM,
                ScanningFilterPolicy::BasicUnfiltered,
            ),
            LeSetScanParams::new(
                LeScanKind::Passive,
                Duration::from_u16(0x20),
                Duration::from_u16(0x10),
                AddrKind::PUBLIC,
                ScanningFilterPolicy::BasicFiltered,
            ),
        ] {
            let response = decode(&command)
                .expect_err("the unsupported role must not become Controller intent")
                .into_command_complete()
                .expect("the opcode belongs to scanning");
            assert_eq!(response.status(), HciError::UNSUPPORTED.to_status());
        }

        for command in [
            LeSetScanParams::new(
                LeScanKind::Passive,
                Duration::from_u16(3),
                Duration::from_u16(3),
                AddrKind::PUBLIC,
                ScanningFilterPolicy::BasicUnfiltered,
            ),
            LeSetScanParams::new(
                LeScanKind::Passive,
                Duration::from_u16(8),
                Duration::from_u16(9),
                AddrKind::PUBLIC,
                ScanningFilterPolicy::BasicUnfiltered,
            ),
        ] {
            let response = decode(&command)
                .expect_err("invalid scan timing must not become Controller intent")
                .into_command_complete()
                .expect("the opcode belongs to scanning");
            assert_eq!(
                response.status(),
                HciError::INVALID_HCI_PARAMETERS.to_status()
            );
        }
    }

    #[test]
    fn malformed_standard_parameter_body_is_rejected_by_bt_hci() {
        let error = LeLegacyScanningCommand::decode(HciCommandPacket::for_test(
            LeSetScanEnable::OPCODE,
            &[1],
        ))
        .expect_err("a truncated standard command must fail closed");
        let response = error
            .into_command_complete()
            .expect("the opcode belongs to scanning");
        assert_eq!(
            response.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
    }

    #[test]
    fn rejection_completion_roundtrips_through_bt_hci() {
        let response = LeLegacyScanningCommand::decode(HciCommandPacket::for_test(
            LeSetScanEnable::OPCODE,
            &[2, 0],
        ))
        .expect_err("bt-hci rejects an invalid bool")
        .into_command_complete()
        .expect("the opcode belongs to scanning");
        let complete = CommandComplete::from_hci_bytes_complete(&response.as_bytes()[2..])
            .expect("the event parameters decode through bt-hci");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("the completion carries a status");
        assert_eq!(complete.cmd_opcode, LeSetScanEnable::OPCODE);
        assert_eq!(
            complete.status,
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert_eq!(response.status(), complete.status);
    }

    #[test]
    fn reset_scoped_configuration_freezes_an_enable_snapshot() {
        let parameters = decode(&LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(0x20),
            Duration::from_u16(0x10),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::BasicUnfiltered,
        ))
        .expect("the fixture parameters decode");
        let parameters = LeLegacyScanningConfigurationCommand::from_command(parameters)
            .expect("Set Parameters is configuration");
        let mut configuration = LeLegacyScanningConfiguration::new();

        assert_eq!(
            configuration
                .dispatch(BootstrapPhase::AwaitingReset, parameters)
                .status(),
            HciError::CMD_DISALLOWED.to_status()
        );
        assert_eq!(configuration.parameters(), None);
        assert_eq!(
            configuration
                .dispatch(BootstrapPhase::Configuring, parameters)
                .status(),
            Status::SUCCESS
        );

        let enable = LeLegacyScanningEnableCommand {
            enable: true,
            duplicate_policy: LeLegacyScanningDuplicatePolicy::FilterDuplicates,
        };
        let LeLegacyScanningIdleEnableDisposition::Start(request) =
            configuration.dispatch_idle_enable(BootstrapPhase::Configuring, enable)
        else {
            panic!("configured Enable must retain a hardware start");
        };
        assert_eq!(request.parameters(), parameters.parameters());
        assert_eq!(
            request.duplicate_policy(),
            LeLegacyScanningDuplicatePolicy::FilterDuplicates
        );

        configuration.reset();
        assert_eq!(configuration.parameters(), None);
    }

    #[test]
    fn single_report_event_roundtrips_through_bt_hci() {
        let event = LeLegacyAdvertisingReportEvent::new(
            LeAdvEventKind::AdvNonconnInd,
            AddrKind::RANDOM,
            BdAddr::new([1, 2, 3, 4, 5, 0xc6]),
            &[2, 1, 6],
            -71,
        )
        .expect("the legacy report fits its standard event");
        let Event::Le(LeEvent::LeAdvertisingReport(event)) =
            Event::from_hci_bytes_complete(event.as_bytes())
                .expect("bt-hci decodes the emitted complete event")
        else {
            panic!("the emitted event changed standard kind");
        };
        assert_eq!(event.reports.len(), 1);
        let report = event
            .reports
            .iter()
            .next()
            .expect("one report was declared")
            .expect("the report fields decode");
        assert_eq!(report.event_kind, LeAdvEventKind::AdvNonconnInd);
        assert_eq!(report.addr_kind, AddrKind::RANDOM);
        assert_eq!(report.addr, BdAddr::new([1, 2, 3, 4, 5, 0xc6]));
        assert_eq!(report.data, [2, 1, 6]);
        assert_eq!(report.rssi, -71);
    }

    #[test]
    fn report_event_rejects_unrepresentable_legacy_fields() {
        assert_eq!(
            LeLegacyAdvertisingReportEvent::new(
                LeAdvEventKind::AdvInd,
                AddrKind::PUBLIC,
                BdAddr::default(),
                &[0; 32],
                0,
            ),
            Err(LeLegacyAdvertisingReportEventError::DataTooLong { length: 32 })
        );
        assert_eq!(
            LeLegacyAdvertisingReportEvent::new(
                LeAdvEventKind::AdvInd,
                AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
                BdAddr::default(),
                &[],
                0,
            ),
            Err(LeLegacyAdvertisingReportEventError::UnsupportedAddressKind(
                AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            ))
        );
    }
}
