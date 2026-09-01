//! Semantic HCI boundary for the supported legacy advertising command subset.
//!
//! This module decodes the standard `bt-hci` command types into owned values.
//! It deliberately does not retain advertising configuration, select an
//! advertiser address, start a Link Layer event, or claim radio progress.

use bt_hci::{
    FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{
            LeSetAdvData, LeSetAdvDataParams, LeSetAdvEnable, LeSetAdvParams, LeSetAdvParamsParams,
        },
    },
    param::{AddrKind, AdvKind, Error as HciError, Status},
};

use crate::{
    BluetoothPublicDeviceAddress, BootstrapPhase, HciCommandPacket, HciControllerResponse,
};

/// Largest legacy advertising data value accepted by the HCI command.
pub const LE_LEGACY_ADVERTISING_DATA_CAPACITY: usize = 31;
/// Complete Command Complete event size for this command family.
pub const LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;

const LEGACY_ADVERTISING_INTERVAL_MIN: u16 = 0x0020;
const LEGACY_ADVERTISING_INTERVAL_MAX: u16 = 0x4000;

/// Closed identity of the supported standard legacy advertising commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingCommandKind {
    SetParameters,
    SetData,
    SetEnable,
}

impl LeLegacyAdvertisingCommandKind {
    /// Classify an opcode without decoding its parameters.
    pub const fn from_opcode(opcode: Opcode) -> Option<Self> {
        let raw = opcode.to_raw();
        if raw == LeSetAdvParams::OPCODE.to_raw() {
            Some(Self::SetParameters)
        } else if raw == LeSetAdvData::OPCODE.to_raw() {
            Some(Self::SetData)
        } else if raw == LeSetAdvEnable::OPCODE.to_raw() {
            Some(Self::SetEnable)
        } else {
            None
        }
    }

    /// Exact standard HCI opcode.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::SetParameters => LeSetAdvParams::OPCODE,
            Self::SetData => LeSetAdvData::OPCODE,
            Self::SetEnable => LeSetAdvEnable::OPCODE,
        }
    }
}

/// Address source selected for the supported advertising role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingOwnAddressKind {
    Public,
    Random,
}

/// Non-empty HCI primary-channel selection with reserved bits rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingPrimaryChannels {
    channel_37: bool,
    channel_38: bool,
    channel_39: bool,
}

impl LeLegacyAdvertisingPrimaryChannels {
    /// Whether primary advertising channel 37 is selected.
    pub const fn channel_37(self) -> bool {
        self.channel_37
    }

    /// Whether primary advertising channel 38 is selected.
    pub const fn channel_38(self) -> bool {
        self.channel_38
    }

    /// Whether primary advertising channel 39 is selected.
    pub const fn channel_39(self) -> bool {
        self.channel_39
    }
}

/// Validated interval range supplied by LE Set Advertising Parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingIntervalRange {
    minimum_units_625_us: u16,
    maximum_units_625_us: u16,
}

impl LeLegacyAdvertisingIntervalRange {
    /// Minimum requested interval in 0.625 ms units.
    pub const fn minimum_units_625_us(self) -> u16 {
        self.minimum_units_625_us
    }

    /// Maximum requested interval in 0.625 ms units.
    pub const fn maximum_units_625_us(self) -> u16 {
        self.maximum_units_625_us
    }
}

/// Parameters for the currently closed `ADV_NONCONN_IND` role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyNonconnectableAdvertisingParameters {
    interval: LeLegacyAdvertisingIntervalRange,
    own_address_kind: LeLegacyAdvertisingOwnAddressKind,
    channels: LeLegacyAdvertisingPrimaryChannels,
}

impl LeLegacyNonconnectableAdvertisingParameters {
    /// Controller-selectable advertising interval range.
    pub const fn interval(self) -> LeLegacyAdvertisingIntervalRange {
        self.interval
    }

    /// Source of the advertiser address.
    pub const fn own_address_kind(self) -> LeLegacyAdvertisingOwnAddressKind {
        self.own_address_kind
    }

    /// Complete non-empty primary-channel selection.
    pub const fn channels(self) -> LeLegacyAdvertisingPrimaryChannels {
        self.channels
    }
}

/// Owned, length-checked advertising data from LE Set Advertising Data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingData {
    bytes: [u8; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
    length: u8,
}

impl LeLegacyAdvertisingData {
    /// Borrow only the Host-declared advertising-data prefix.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Number of meaningful advertising-data octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the Host selected an empty advertising-data value.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

/// One fully decoded standard command awaiting Controller policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingCommand {
    SetParameters(LeLegacyNonconnectableAdvertisingParameters),
    SetData(LeLegacyAdvertisingData),
    SetEnable(bool),
}

/// Configuration-only subset which can complete without starting hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingConfigurationCommand {
    SetParameters(LeLegacyNonconnectableAdvertisingParameters),
    SetData(LeLegacyAdvertisingData),
}

/// One decoded Set Advertising Enable command awaiting lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingEnableCommand {
    enable: bool,
}

impl LeLegacyAdvertisingEnableCommand {
    /// Separate Enable from the full command family without losing its value.
    pub fn from_command(
        command: LeLegacyAdvertisingCommand,
    ) -> Result<Self, LeLegacyAdvertisingCommand> {
        match command {
            LeLegacyAdvertisingCommand::SetEnable(enable) => Ok(Self { enable }),
            command => Err(command),
        }
    }

    /// Whether the Host requested entry into the enabled lifecycle.
    pub const fn enable(self) -> bool {
        self.enable
    }

    pub(crate) fn into_started_command_complete(self) -> LeLegacyAdvertisingCommandCompleteEvent {
        debug_assert!(self.enable);
        LeLegacyAdvertisingCommandCompleteEvent::new(
            LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
            Status::SUCCESS,
        )
    }

    pub(crate) fn into_hardware_failure_command_complete(
        self,
    ) -> LeLegacyAdvertisingCommandCompleteEvent {
        debug_assert!(self.enable);
        LeLegacyAdvertisingCommandCompleteEvent::new(
            LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }
}

/// Resolved advertiser address retained by an accepted Enable transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingAddress {
    wire_bytes: [u8; 6],
    kind: LeLegacyAdvertisingOwnAddressKind,
}

impl LeLegacyAdvertisingAddress {
    /// HCI/Link-Layer least-significant-octet-first address bytes.
    pub const fn wire_bytes(self) -> [u8; 6] {
        self.wire_bytes
    }

    /// Public or random TxAdd identity selected by the accepted parameters.
    pub const fn kind(self) -> LeLegacyAdvertisingOwnAddressKind {
        self.kind
    }
}

/// Immutable Host configuration snapshot retained from Enable until radio start.
///
/// This value owns all software inputs needed to create a portable Link Layer
/// set. It grants no scheduler, SRAM, radio, or response-completion authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingEnableRequest {
    parameters: LeLegacyNonconnectableAdvertisingParameters,
    data: LeLegacyAdvertisingData,
    advertiser: LeLegacyAdvertisingAddress,
}

impl LeLegacyAdvertisingEnableRequest {
    /// Exact accepted non-connectable parameter snapshot.
    pub const fn parameters(self) -> LeLegacyNonconnectableAdvertisingParameters {
        self.parameters
    }

    /// Exact accepted advertising-data snapshot.
    pub const fn data(self) -> LeLegacyAdvertisingData {
        self.data
    }

    /// Address resolved at the Enable ordering boundary.
    pub const fn advertiser(self) -> LeLegacyAdvertisingAddress {
        self.advertiser
    }
}

pub(crate) enum LeLegacyAdvertisingIdleEnableDisposition {
    Start(LeLegacyAdvertisingEnableRequest),
    Complete(LeLegacyAdvertisingCommandCompleteEvent),
}

impl LeLegacyAdvertisingConfigurationCommand {
    /// Extract the software-only configuration commands from the full family.
    pub fn from_command(
        command: LeLegacyAdvertisingCommand,
    ) -> Result<Self, LeLegacyAdvertisingCommand> {
        match command {
            LeLegacyAdvertisingCommand::SetParameters(parameters) => {
                Ok(Self::SetParameters(parameters))
            }
            LeLegacyAdvertisingCommand::SetData(data) => Ok(Self::SetData(data)),
            command @ LeLegacyAdvertisingCommand::SetEnable(_) => Err(command),
        }
    }

    /// Exact standard HCI command identity.
    pub const fn kind(&self) -> LeLegacyAdvertisingCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyAdvertisingCommandKind::SetParameters,
            Self::SetData(_) => LeLegacyAdvertisingCommandKind::SetData,
        }
    }
}

/// Reset-scoped software configuration for one legacy advertising set.
///
/// This state contains only accepted Host intent. It grants no Link Layer,
/// scheduler, SRAM, or radio ownership and has no enabled state.
pub(crate) struct LeLegacyAdvertisingConfiguration {
    parameters: Option<LeLegacyNonconnectableAdvertisingParameters>,
    data: LeLegacyAdvertisingData,
}

impl LeLegacyAdvertisingConfiguration {
    pub(crate) const fn new() -> Self {
        Self {
            parameters: None,
            data: LeLegacyAdvertisingData {
                bytes: [0; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
                length: 0,
            },
        }
    }

    pub(crate) fn dispatch(
        &mut self,
        phase: BootstrapPhase,
        command: LeLegacyAdvertisingConfigurationCommand,
    ) -> LeLegacyAdvertisingCommandCompleteEvent {
        let kind = command.kind();
        if phase == BootstrapPhase::AwaitingReset {
            return LeLegacyAdvertisingCommandCompleteEvent::new(
                kind.opcode(),
                HciError::CMD_DISALLOWED.to_status(),
            );
        }
        match command {
            LeLegacyAdvertisingConfigurationCommand::SetParameters(parameters) => {
                self.parameters = Some(parameters);
            }
            LeLegacyAdvertisingConfigurationCommand::SetData(data) => {
                self.data = data;
            }
        }
        LeLegacyAdvertisingCommandCompleteEvent::new(kind.opcode(), Status::SUCCESS)
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn dispatch_idle_enable(
        &self,
        phase: BootstrapPhase,
        command: LeLegacyAdvertisingEnableCommand,
        public_address: BluetoothPublicDeviceAddress,
        requested_random_address: Option<bt_hci::param::BdAddr>,
    ) -> LeLegacyAdvertisingIdleEnableDisposition {
        if phase == BootstrapPhase::AwaitingReset {
            return LeLegacyAdvertisingIdleEnableDisposition::Complete(
                LeLegacyAdvertisingCommandCompleteEvent::new(
                    LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                ),
            );
        }
        if !command.enable() {
            return LeLegacyAdvertisingIdleEnableDisposition::Complete(
                LeLegacyAdvertisingCommandCompleteEvent::new(
                    LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
                    Status::SUCCESS,
                ),
            );
        }
        let Some(parameters) = self.parameters else {
            return LeLegacyAdvertisingIdleEnableDisposition::Complete(
                LeLegacyAdvertisingCommandCompleteEvent::new(
                    LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                ),
            );
        };
        let advertiser = match parameters.own_address_kind() {
            LeLegacyAdvertisingOwnAddressKind::Public => LeLegacyAdvertisingAddress {
                wire_bytes: public_address.hci_wire_bytes(),
                kind: LeLegacyAdvertisingOwnAddressKind::Public,
            },
            LeLegacyAdvertisingOwnAddressKind::Random => {
                let Some(address) = requested_random_address else {
                    return LeLegacyAdvertisingIdleEnableDisposition::Complete(
                        LeLegacyAdvertisingCommandCompleteEvent::new(
                            LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
                            HciError::CMD_DISALLOWED.to_status(),
                        ),
                    );
                };
                let mut wire_bytes = [0; 6];
                wire_bytes.copy_from_slice(address.raw());
                LeLegacyAdvertisingAddress {
                    wire_bytes,
                    kind: LeLegacyAdvertisingOwnAddressKind::Random,
                }
            }
        };
        LeLegacyAdvertisingIdleEnableDisposition::Start(LeLegacyAdvertisingEnableRequest {
            parameters,
            data: self.data,
            advertiser,
        })
    }

    pub(crate) fn complete_enable_while_radio_unavailable(
        phase: BootstrapPhase,
        command: LeLegacyAdvertisingEnableCommand,
    ) -> LeLegacyAdvertisingCommandCompleteEvent {
        let status = if phase == BootstrapPhase::AwaitingReset || command.enable() {
            HciError::CMD_DISALLOWED.to_status()
        } else {
            Status::SUCCESS
        };
        LeLegacyAdvertisingCommandCompleteEvent::new(
            LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
            status,
        )
    }

    #[cfg(test)]
    pub(crate) const fn parameters(&self) -> Option<LeLegacyNonconnectableAdvertisingParameters> {
        self.parameters
    }

    #[cfg(test)]
    pub(crate) const fn data(&self) -> LeLegacyAdvertisingData {
        self.data
    }
}

impl LeLegacyAdvertisingCommand {
    /// Decode one command without mutating HCI, Link Layer, or hardware state.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLegacyAdvertisingDecodeError> {
        let Some(kind) = LeLegacyAdvertisingCommandKind::from_opcode(command.opcode()) else {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedOpcode {
                opcode: command.opcode(),
            });
        };

        match kind {
            LeLegacyAdvertisingCommandKind::SetParameters => {
                let parameters =
                    LeSetAdvParamsParams::from_hci_bytes_complete(command.parameters()).map_err(
                        |_| LeLegacyAdvertisingDecodeError::MalformedParameters { command: kind },
                    )?;
                Self::decode_parameters(parameters)
            }
            LeLegacyAdvertisingCommandKind::SetData => {
                let parameters = LeSetAdvDataParams::from_hci_bytes_complete(command.parameters())
                    .map_err(|_| LeLegacyAdvertisingDecodeError::MalformedParameters {
                        command: kind,
                    })?;
                if usize::from(parameters.data_len) > LE_LEGACY_ADVERTISING_DATA_CAPACITY {
                    return Err(LeLegacyAdvertisingDecodeError::InvalidParameters {
                        command: kind,
                    });
                }
                Ok(Self::SetData(LeLegacyAdvertisingData {
                    bytes: parameters.data,
                    length: parameters.data_len,
                }))
            }
            LeLegacyAdvertisingCommandKind::SetEnable => {
                let enable = bool::from_hci_bytes_complete(command.parameters()).map_err(|_| {
                    LeLegacyAdvertisingDecodeError::MalformedParameters { command: kind }
                })?;
                Ok(Self::SetEnable(enable))
            }
        }
    }

    fn decode_parameters(
        parameters: LeSetAdvParamsParams,
    ) -> Result<Self, LeLegacyAdvertisingDecodeError> {
        let command = LeLegacyAdvertisingCommandKind::SetParameters;
        let LeSetAdvParamsParams {
            adv_interval_min,
            adv_interval_max,
            adv_kind,
            own_addr_kind,
            peer_addr_kind: _,
            peer_addr: _,
            adv_channel_map,
            adv_filter_policy: _,
        } = parameters;
        if adv_kind != AdvKind::AdvNonconnInd {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedFeature { command });
        }

        let own_address_kind = if own_addr_kind == AddrKind::PUBLIC {
            LeLegacyAdvertisingOwnAddressKind::Public
        } else if own_addr_kind == AddrKind::RANDOM {
            LeLegacyAdvertisingOwnAddressKind::Random
        } else {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedFeature { command });
        };

        let minimum_units_625_us = adv_interval_min.as_u16();
        let maximum_units_625_us = adv_interval_max.as_u16();
        if minimum_units_625_us < LEGACY_ADVERTISING_INTERVAL_MIN
            || maximum_units_625_us > LEGACY_ADVERTISING_INTERVAL_MAX
            || minimum_units_625_us > maximum_units_625_us
        {
            return Err(LeLegacyAdvertisingDecodeError::InvalidParameters { command });
        }

        let channel_bits = adv_channel_map.into_inner();
        if channel_bits == 0 || channel_bits & !0x07 != 0 {
            return Err(LeLegacyAdvertisingDecodeError::InvalidParameters { command });
        }

        Ok(Self::SetParameters(
            LeLegacyNonconnectableAdvertisingParameters {
                interval: LeLegacyAdvertisingIntervalRange {
                    minimum_units_625_us,
                    maximum_units_625_us,
                },
                own_address_kind,
                channels: LeLegacyAdvertisingPrimaryChannels {
                    channel_37: adv_channel_map.is_channel_37_enabled(),
                    channel_38: adv_channel_map.is_channel_38_enabled(),
                    channel_39: adv_channel_map.is_channel_39_enabled(),
                },
            },
        ))
    }

    /// Exact command identity retained by this semantic token.
    pub const fn kind(&self) -> LeLegacyAdvertisingCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyAdvertisingCommandKind::SetParameters,
            Self::SetData(_) => LeLegacyAdvertisingCommandKind::SetData,
            Self::SetEnable(_) => LeLegacyAdvertisingCommandKind::SetEnable,
        }
    }
}

/// Why a packet could not become an owned semantic advertising command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingDecodeError {
    UnsupportedOpcode {
        opcode: Opcode,
    },
    MalformedParameters {
        command: LeLegacyAdvertisingCommandKind,
    },
    InvalidParameters {
        command: LeLegacyAdvertisingCommandKind,
    },
    UnsupportedFeature {
        command: LeLegacyAdvertisingCommandKind,
    },
}

impl LeLegacyAdvertisingDecodeError {
    /// Convert a rejection for a known opcode into the required completion.
    ///
    /// An unclaimed opcode remains available to the next command family.
    pub fn into_command_complete(self) -> Result<LeLegacyAdvertisingCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::MalformedParameters { command } | Self::InvalidParameters { command } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedFeature { command } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeLegacyAdvertisingCommandCompleteEvent::new(
            command.opcode(),
            status,
        ))
    }
}

/// Owned Command Complete for one legacy advertising command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingCommandCompleteEvent {
    bytes: [u8; LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl LeLegacyAdvertisingCommandCompleteEvent {
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

impl HciControllerResponse for LeLegacyAdvertisingCommandCompleteEvent {
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
        cmd::{
            Cmd,
            le::{LeSetAdvData, LeSetAdvEnable, LeSetAdvParams},
        },
        param::{
            AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration, Error as HciError,
            Status,
        },
    };

    use super::{
        LeLegacyAdvertisingCommand, LeLegacyAdvertisingCommandKind,
        LeLegacyAdvertisingConfiguration, LeLegacyAdvertisingConfigurationCommand,
        LeLegacyAdvertisingEnableCommand, LeLegacyAdvertisingIdleEnableDisposition,
        LeLegacyAdvertisingOwnAddressKind,
    };
    use crate::{BluetoothPublicDeviceAddress, BootstrapPhase, HciCommandPacket};

    #[test]
    fn decodes_supported_nonconnectable_parameters() {
        let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &[
                0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
            ],
        ))
        .expect("the supported standard parameters decode");

        let LeLegacyAdvertisingCommand::SetParameters(parameters) = command else {
            panic!("parameters changed semantic command kind");
        };
        assert_eq!(parameters.interval().minimum_units_625_us(), 0x20);
        assert_eq!(parameters.interval().maximum_units_625_us(), 0x40);
        assert_eq!(
            parameters.own_address_kind(),
            LeLegacyAdvertisingOwnAddressKind::Random
        );
        assert!(parameters.channels().channel_37());
        assert!(!parameters.channels().channel_38());
        assert!(parameters.channels().channel_39());
    }

    #[test]
    fn data_is_owned_and_bounded_by_declared_length() {
        let mut body = [0; 32];
        body[0] = 3;
        body[1..4].copy_from_slice(&[2, 1, 6]);
        let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvData::OPCODE,
            &body,
        ))
        .expect("the complete standard data command decodes");
        let LeLegacyAdvertisingCommand::SetData(data) = command else {
            panic!("data changed semantic command kind");
        };
        body.fill(0xff);
        assert_eq!(data.as_bytes(), &[2, 1, 6]);
    }

    #[test]
    fn rejects_malformed_invalid_and_unsupported_values_with_exact_status() {
        for (opcode, body, expected) in [
            (
                LeSetAdvEnable::OPCODE,
                &[2][..],
                HciError::INVALID_HCI_PARAMETERS.to_status(),
            ),
            (
                LeSetAdvParams::OPCODE,
                &[0x20, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0][..],
                HciError::UNSUPPORTED.to_status(),
            ),
            (
                LeSetAdvParams::OPCODE,
                &[0x20, 0, 0x40, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
                HciError::INVALID_HCI_PARAMETERS.to_status(),
            ),
        ] {
            let error =
                LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(opcode, body))
                    .expect_err("the rejected value cannot become a command token");
            let response = error
                .into_command_complete()
                .expect("the opcode belongs to this command family");
            assert_eq!(response.opcode(), opcode);
            assert_eq!(response.status(), expected);
        }
    }

    #[test]
    fn accepts_standard_bt_hci_field_domains_without_reencoding_them() {
        let command = LeSetAdvParams::new(
            Duration::from_u16(0x20),
            Duration::from_u16(0x40),
            AdvKind::AdvNonconnInd,
            AddrKind::PUBLIC,
            AddrKind::PUBLIC,
            BdAddr::default(),
            AdvChannelMap::ALL,
            AdvFilterPolicy::Unfiltered,
        );
        let _ = command;
        assert_eq!(
            LeSetAdvParams::OPCODE,
            LeLegacyAdvertisingCommandKind::SetParameters.opcode()
        );
    }

    #[test]
    fn configuration_is_reset_scoped_and_rejects_pre_reset_mutation() {
        let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &[
                0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
            ],
        ))
        .expect("fixture parameters decode");
        let parameters = LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
            .expect("Set Parameters is software-only configuration");
        let mut configuration = LeLegacyAdvertisingConfiguration::new();

        let rejected = configuration.dispatch(BootstrapPhase::AwaitingReset, parameters);
        assert_eq!(rejected.status(), HciError::CMD_DISALLOWED.to_status());
        assert_eq!(configuration.parameters(), None);

        let accepted = configuration.dispatch(BootstrapPhase::Configuring, parameters);
        assert_eq!(accepted.status(), Status::SUCCESS);
        assert!(configuration.parameters().is_some());

        let mut body = [0; 32];
        body[0] = 3;
        body[1..4].copy_from_slice(&[2, 1, 6]);
        let data = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvData::OPCODE,
            &body,
        ))
        .expect("fixture data decode");
        let data = LeLegacyAdvertisingConfigurationCommand::from_command(data)
            .expect("Set Data is software-only configuration");
        assert_eq!(
            configuration
                .dispatch(BootstrapPhase::Configuring, data)
                .status(),
            Status::SUCCESS
        );
        assert_eq!(configuration.data().as_bytes(), &[2, 1, 6]);

        configuration.reset();
        assert_eq!(configuration.parameters(), None);
        assert!(configuration.data().is_empty());
    }

    #[test]
    fn idle_enable_freezes_parameters_data_and_resolved_public_address() {
        let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &[
                0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
            ],
        ))
        .expect("fixture parameters decode");
        let mut configuration = LeLegacyAdvertisingConfiguration::new();
        configuration.dispatch(
            BootstrapPhase::Configuring,
            LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
                .expect("the parameters command is configuration"),
        );

        let mut body = [0; 32];
        body[0] = 3;
        body[1..4].copy_from_slice(&[2, 1, 6]);
        let data = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvData::OPCODE,
            &body,
        ))
        .expect("fixture data decode");
        configuration.dispatch(
            BootstrapPhase::Configuring,
            LeLegacyAdvertisingConfigurationCommand::from_command(data)
                .expect("the data command is configuration"),
        );

        let enable = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvEnable::OPCODE,
            &[1],
        ))
        .expect("Enable decodes");
        let enable = LeLegacyAdvertisingEnableCommand::from_command(enable)
            .expect("Enable refines into its lifecycle token");
        let LeLegacyAdvertisingIdleEnableDisposition::Start(request) = configuration
            .dispatch_idle_enable(
                BootstrapPhase::Configuring,
                enable,
                BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
                None,
            )
        else {
            panic!("complete configuration must defer a hardware start");
        };
        assert_eq!(request.data().as_bytes(), &[2, 1, 6]);
        assert_eq!(request.advertiser().wire_bytes(), [6, 5, 4, 3, 2, 1]);
        assert_eq!(
            request.advertiser().kind(),
            LeLegacyAdvertisingOwnAddressKind::Public
        );
        assert_eq!(request.parameters().interval().minimum_units_625_us(), 0x20);
        assert_eq!(request.parameters().interval().maximum_units_625_us(), 0x40);
        assert!(request.parameters().channels().channel_37());
        assert!(!request.parameters().channels().channel_38());
        assert!(request.parameters().channels().channel_39());
    }

    #[test]
    fn idle_enable_requires_reset_parameters_and_a_selected_random_address() {
        let enable = LeLegacyAdvertisingEnableCommand::from_command(
            LeLegacyAdvertisingCommand::SetEnable(true),
        )
        .expect("the fixture is Enable");
        let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
        let mut configuration = LeLegacyAdvertisingConfiguration::new();

        for phase in [BootstrapPhase::AwaitingReset, BootstrapPhase::Configuring] {
            let LeLegacyAdvertisingIdleEnableDisposition::Complete(response) =
                configuration.dispatch_idle_enable(phase, enable, public_address, None)
            else {
                panic!("Enable without an accepted parameter set must fail closed");
            };
            assert_eq!(response.status(), HciError::CMD_DISALLOWED.to_status());
        }

        let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &[
                0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
            ],
        ))
        .expect("random-address parameters decode");
        configuration.dispatch(
            BootstrapPhase::Configuring,
            LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
                .expect("the parameters command is configuration"),
        );
        let LeLegacyAdvertisingIdleEnableDisposition::Complete(response) = configuration
            .dispatch_idle_enable(BootstrapPhase::Configuring, enable, public_address, None)
        else {
            panic!("random advertising cannot start without LE Set Random Address");
        };
        assert_eq!(response.status(), HciError::CMD_DISALLOWED.to_status());

        let LeLegacyAdvertisingIdleEnableDisposition::Start(request) = configuration
            .dispatch_idle_enable(
                BootstrapPhase::Configuring,
                enable,
                public_address,
                Some(BdAddr::new([9, 8, 7, 6, 5, 0xc4])),
            )
        else {
            panic!("the accepted random address must complete the start snapshot");
        };
        assert_eq!(request.advertiser().wire_bytes(), [9, 8, 7, 6, 5, 0xc4]);
        assert_eq!(
            request.advertiser().kind(),
            LeLegacyAdvertisingOwnAddressKind::Random
        );
    }
}
