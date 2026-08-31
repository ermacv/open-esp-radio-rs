//! Conservative HCI bootstrap state below a Bluetooth Host.
//!
//! This module owns only the command subset needed to configure the initial
//! direct HCI boundary. It never starts advertising, scanning, a connection,
//! encryption or radio work. Unknown and Link-Layer commands remain unclaimed
//! for an outer router; they never fall through to guessed hardware behavior.

use core::num::NonZeroU16;

use bt_hci::{
    FromHciBytes,
    cmd::{
        Cmd, Opcode,
        controller_baseband::{
            HostBufferSize, HostBufferSizeParams, Reset, SetControllerToHostFlowControl,
            SetEventMask,
        },
        info::ReadBdAddr,
        le::{
            LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
            LeSetEventMask, LeSetRandomAddr,
        },
    },
    param::{
        BdAddr, ControllerToHostFlowControl, Error as HciError, EventMask, LeEventMask, Status,
    },
};

use crate::HciCommandPacket;

/// Maximum complete HCI Event body emitted by the bootstrap state machine.
///
/// This includes the two-byte Event header. The largest supported response is
/// LE Read Local Supported Features: six Command Complete bytes plus eight
/// conservative feature bytes.
pub const BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 14;

/// Invalid immutable parameters for the initial Host/Controller profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapConfigError {
    /// Trouble cannot transmit when the Controller reports a zero ACL size.
    ZeroAclDataPacketLength,
    /// Trouble cannot acquire link credits when the Controller reports zero.
    ZeroAclDataPacketCount,
}

/// Public Bluetooth device identity in canonical display order.
///
/// The first byte is the octet printed first in an EUI-48 address. HCI carries
/// `BD_ADDR` least-significant octet first, so keeping canonical bytes in a
/// separate type prevents a platform eFuse reader from leaking byte-order
/// policy into every Controller bootstrap caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPublicDeviceAddress([u8; 6]);

impl BluetoothPublicDeviceAddress {
    /// Construct an address from canonical EUI-48 bytes.
    pub const fn from_canonical_bytes(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    /// Return the canonical EUI-48 byte sequence unchanged.
    pub const fn canonical_bytes(self) -> [u8; 6] {
        self.0
    }

    fn hci_wire_address(self) -> BdAddr {
        let [byte_0, byte_1, byte_2, byte_3, byte_4, byte_5] = self.0;
        BdAddr::new([byte_5, byte_4, byte_3, byte_2, byte_1, byte_0])
    }
}

/// Immutable, non-radio values reported during HCI Host bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerBootstrapConfig {
    public_address: BluetoothPublicDeviceAddress,
    le_acl_data_packet_length: u16,
    total_num_le_acl_data_packets: u8,
}

impl LeControllerBootstrapConfig {
    /// Create a bounded initial LE Host profile.
    ///
    /// A zero public address is allowed for deployments which provide a random
    /// address. This value is a software HCI report and is not proof that an
    /// ESP32-S31 address or packet buffer has reached hardware.
    pub const fn new(
        public_address: BluetoothPublicDeviceAddress,
        le_acl_data_packet_length: u16,
        total_num_le_acl_data_packets: u8,
    ) -> Result<Self, BootstrapConfigError> {
        if le_acl_data_packet_length == 0 {
            return Err(BootstrapConfigError::ZeroAclDataPacketLength);
        }
        if total_num_le_acl_data_packets == 0 {
            return Err(BootstrapConfigError::ZeroAclDataPacketCount);
        }
        Ok(Self {
            public_address,
            le_acl_data_packet_length,
            total_num_le_acl_data_packets,
        })
    }

    /// Public address in canonical EUI-48 display order.
    pub const fn public_address(&self) -> BluetoothPublicDeviceAddress {
        self.public_address
    }

    /// Maximum Host-to-Controller LE ACL data payload reported to the Host.
    pub const fn le_acl_data_packet_length(&self) -> u16 {
        self.le_acl_data_packet_length
    }

    /// Initial number of Host-to-Controller LE ACL credits.
    pub const fn total_num_le_acl_data_packets(&self) -> u8 {
        self.total_num_le_acl_data_packets
    }

    /// Number of implemented filter accept list entries.
    ///
    /// The initial profile has no list owner and therefore reports zero.
    pub const fn filter_accept_list_size(&self) -> u8 {
        0
    }
}

/// HCI bootstrap lifecycle relative to the mandatory Reset command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapPhase {
    /// No valid Reset has established a fresh Host configuration epoch.
    AwaitingReset,
    /// Bootstrap commands may configure the current software HCI epoch.
    Configuring,
}

/// Host buffer declaration accepted for the LE-only initial profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapHostBuffers {
    /// Maximum Controller-to-Host ACL packet length offered by the Host.
    pub acl_data_packet_length: u16,
    /// Number of Controller-to-Host ACL packet slots offered by the Host.
    pub total_acl_data_packets: u16,
}

/// Commands implemented by the software-only bootstrap state machine.
///
/// This is the source-owned capability table. Absence from this enum means the
/// command receives Unknown HCI Command; it does not fall through to guessed
/// hardware or Link-Layer behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapCommand {
    /// HCI Reset.
    Reset,
    /// Set Event Mask.
    SetEventMask,
    /// Set Controller To Host Flow Control.
    SetControllerToHostFlowControl,
    /// Host Buffer Size.
    HostBufferSize,
    /// Read BD_ADDR.
    ReadBdAddr,
    /// LE Set Event Mask.
    LeSetEventMask,
    /// LE Read Buffer Size.
    LeReadBufferSize,
    /// LE Read Local Supported Features.
    LeReadLocalSupportedFeatures,
    /// LE Set Random Address.
    LeSetRandomAddress,
    /// LE Read Filter Accept List Size.
    LeReadFilterAcceptListSize,
}

impl BootstrapCommand {
    /// Classify one opcode against the closed bootstrap capability table.
    pub const fn from_opcode(opcode: Opcode) -> Option<Self> {
        let raw = opcode.to_raw();
        if raw == Reset::OPCODE.to_raw() {
            Some(Self::Reset)
        } else if raw == SetEventMask::OPCODE.to_raw() {
            Some(Self::SetEventMask)
        } else if raw == SetControllerToHostFlowControl::OPCODE.to_raw() {
            Some(Self::SetControllerToHostFlowControl)
        } else if raw == HostBufferSize::OPCODE.to_raw() {
            Some(Self::HostBufferSize)
        } else if raw == ReadBdAddr::OPCODE.to_raw() {
            Some(Self::ReadBdAddr)
        } else if raw == LeSetEventMask::OPCODE.to_raw() {
            Some(Self::LeSetEventMask)
        } else if raw == LeReadBufferSize::OPCODE.to_raw() {
            Some(Self::LeReadBufferSize)
        } else if raw == LeReadLocalSupportedFeatures::OPCODE.to_raw() {
            Some(Self::LeReadLocalSupportedFeatures)
        } else if raw == LeSetRandomAddr::OPCODE.to_raw() {
            Some(Self::LeSetRandomAddress)
        } else if raw == LeReadFilterAcceptListSize::OPCODE.to_raw() {
            Some(Self::LeReadFilterAcceptListSize)
        } else {
            None
        }
    }

    /// Whether the closed bootstrap table contains an opcode.
    pub const fn supports(opcode: Opcode) -> bool {
        Self::from_opcode(opcode).is_some()
    }

    /// Exact HCI opcode represented by this command kind.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::Reset => Reset::OPCODE,
            Self::SetEventMask => SetEventMask::OPCODE,
            Self::SetControllerToHostFlowControl => SetControllerToHostFlowControl::OPCODE,
            Self::HostBufferSize => HostBufferSize::OPCODE,
            Self::ReadBdAddr => ReadBdAddr::OPCODE,
            Self::LeSetEventMask => LeSetEventMask::OPCODE,
            Self::LeReadBufferSize => LeReadBufferSize::OPCODE,
            Self::LeReadLocalSupportedFeatures => LeReadLocalSupportedFeatures::OPCODE,
            Self::LeSetRandomAddress => LeSetRandomAddr::OPCODE,
            Self::LeReadFilterAcceptListSize => LeReadFilterAcceptListSize::OPCODE,
        }
    }
}

/// One fully decoded, owned command in the software bootstrap subset.
///
/// Classification constructs this value without observing or changing a
/// bootstrap epoch. A session owner may therefore retain a Reset until active
/// radio work is quiescent, or apply another session-aware policy, before
/// handing it to the bootstrap state owner.
#[derive(Debug, Eq, PartialEq)]
pub enum OwnedBootstrapCommand {
    /// HCI Reset.
    Reset,
    /// Replace the base HCI event mask.
    SetEventMask(EventMask),
    /// Select the requested Controller-to-Host flow-control mode.
    SetControllerToHostFlowControl(ControllerToHostFlowControl),
    /// Publish the Host's bounded ACL buffer declaration.
    HostBufferSize {
        /// Maximum Controller-to-Host ACL packet length offered by the Host.
        acl_data_packet_length: NonZeroU16,
        /// Number of Controller-to-Host ACL packet slots offered by the Host.
        total_acl_data_packets: NonZeroU16,
    },
    /// Read the configured public device address.
    ReadBdAddr,
    /// Replace the LE Meta event mask.
    LeSetEventMask(LeEventMask),
    /// Read the configured LE ACL buffer profile.
    LeReadBufferSize,
    /// Read the conservative LE feature set.
    LeReadLocalSupportedFeatures,
    /// Retain a requested random address for the current software epoch.
    LeSetRandomAddress(BdAddr),
    /// Read the implemented filter accept list capacity.
    LeReadFilterAcceptListSize,
}

impl OwnedBootstrapCommand {
    /// Closed bootstrap command identity.
    pub const fn kind(&self) -> BootstrapCommand {
        match self {
            Self::Reset => BootstrapCommand::Reset,
            Self::SetEventMask(_) => BootstrapCommand::SetEventMask,
            Self::SetControllerToHostFlowControl(_) => {
                BootstrapCommand::SetControllerToHostFlowControl
            }
            Self::HostBufferSize { .. } => BootstrapCommand::HostBufferSize,
            Self::ReadBdAddr => BootstrapCommand::ReadBdAddr,
            Self::LeSetEventMask(_) => BootstrapCommand::LeSetEventMask,
            Self::LeReadBufferSize => BootstrapCommand::LeReadBufferSize,
            Self::LeReadLocalSupportedFeatures => BootstrapCommand::LeReadLocalSupportedFeatures,
            Self::LeSetRandomAddress(_) => BootstrapCommand::LeSetRandomAddress,
            Self::LeReadFilterAcceptListSize => BootstrapCommand::LeReadFilterAcceptListSize,
        }
    }

    /// Exact HCI opcode represented by this semantic command.
    pub const fn opcode(&self) -> Opcode {
        self.kind().opcode()
    }

    /// Whether this command starts a new bootstrap epoch.
    pub const fn is_reset(&self) -> bool {
        matches!(self, Self::Reset)
    }

    pub(crate) fn decode(
        command: HciCommandPacket<'_>,
    ) -> Result<Self, BootstrapCommandDecodeError> {
        let Some(kind) = BootstrapCommand::from_opcode(command.opcode()) else {
            return Err(BootstrapCommandDecodeError::Unsupported);
        };
        let parameters = command.parameters();
        let decoded = match kind {
            BootstrapCommand::Reset => {
                if !parameters.is_empty() {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::Reset
            }
            BootstrapCommand::SetEventMask => Self::SetEventMask(
                parse_complete(parameters).ok_or(BootstrapCommandDecodeError::Malformed(kind))?,
            ),
            BootstrapCommand::SetControllerToHostFlowControl => {
                Self::SetControllerToHostFlowControl(
                    parse_complete(parameters)
                        .ok_or(BootstrapCommandDecodeError::Malformed(kind))?,
                )
            }
            BootstrapCommand::HostBufferSize => {
                let host = parse_complete::<HostBufferSizeParams>(parameters)
                    .ok_or(BootstrapCommandDecodeError::Malformed(kind))?;
                let Some(acl_data_packet_length) = NonZeroU16::new(host.host_acl_data_packet_len)
                else {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                };
                let Some(total_acl_data_packets) =
                    NonZeroU16::new(host.host_total_acl_data_packets)
                else {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                };
                if host.host_sync_data_packet_len != 0 || host.host_total_sync_data_packets != 0 {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::HostBufferSize {
                    acl_data_packet_length,
                    total_acl_data_packets,
                }
            }
            BootstrapCommand::ReadBdAddr => {
                if !parameters.is_empty() {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::ReadBdAddr
            }
            BootstrapCommand::LeSetEventMask => Self::LeSetEventMask(
                parse_complete(parameters).ok_or(BootstrapCommandDecodeError::Malformed(kind))?,
            ),
            BootstrapCommand::LeReadBufferSize => {
                if !parameters.is_empty() {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::LeReadBufferSize
            }
            BootstrapCommand::LeReadLocalSupportedFeatures => {
                if !parameters.is_empty() {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::LeReadLocalSupportedFeatures
            }
            BootstrapCommand::LeSetRandomAddress => Self::LeSetRandomAddress(
                parse_complete(parameters).ok_or(BootstrapCommandDecodeError::Malformed(kind))?,
            ),
            BootstrapCommand::LeReadFilterAcceptListSize => {
                if !parameters.is_empty() {
                    return Err(BootstrapCommandDecodeError::Malformed(kind));
                }
                Self::LeReadFilterAcceptListSize
            }
        };
        Ok(decoded)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BootstrapCommandDecodeError {
    Unsupported,
    Malformed(BootstrapCommand),
}

/// Complete, validated Command Complete HCI Event emitted by bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapCommandCompleteEvent {
    bytes: [u8; BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY],
    length: usize,
    opcode: Opcode,
    status: Status,
}

impl BootstrapCommandCompleteEvent {
    /// Complete HCI Event bytes, without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Opcode copied into the Command Complete event.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// HCI status returned for the command.
    pub const fn status(&self) -> Status {
        self.status
    }

    fn new(opcode: Opcode, status: Status, return_parameters: &[u8]) -> Self {
        let length = 6 + return_parameters.len();
        assert!(
            length <= BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY,
            "bootstrap Command Complete exceeded its closed response profile"
        );
        let mut bytes = [0; BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY];
        bytes[0] = 0x0e;
        bytes[1] = (4 + return_parameters.len()) as u8;
        bytes[2] = 1;
        bytes[3..5].copy_from_slice(&opcode.to_raw().to_le_bytes());
        bytes[5] = status.into_inner();
        bytes[6..length].copy_from_slice(return_parameters);
        Self {
            bytes,
            length,
            opcode,
            status,
        }
    }
}

/// Pure software state for the conservative initial LE HCI command subset.
///
/// Successful setters update requested Host policy only. No field in this type
/// means that a mask, address, buffer or flow-control mode has reached the
/// ESP32-S31 Controller, Link Layer or radio.
pub struct LeControllerBootstrap {
    config: LeControllerBootstrapConfig,
    phase: BootstrapPhase,
    event_mask: EventMask,
    le_event_mask: LeEventMask,
    requested_random_address: Option<BdAddr>,
    host_buffers: Option<BootstrapHostBuffers>,
    controller_to_host_flow_control: ControllerToHostFlowControl,
}

impl LeControllerBootstrap {
    /// Construct cold bootstrap state which accepts only a valid Reset first.
    pub fn new(config: LeControllerBootstrapConfig) -> Self {
        Self {
            config,
            phase: BootstrapPhase::AwaitingReset,
            event_mask: EventMask::new(),
            le_event_mask: LeEventMask::new(),
            requested_random_address: None,
            host_buffers: None,
            controller_to_host_flow_control: ControllerToHostFlowControl::Off,
        }
    }

    /// Immutable values reported to the Host.
    pub const fn config(&self) -> LeControllerBootstrapConfig {
        self.config
    }

    /// Current Reset/configuration phase.
    pub const fn phase(&self) -> BootstrapPhase {
        self.phase
    }

    /// Whether no successful Reset has opened this bootstrap epoch.
    ///
    /// Every mutating command is rejected before Reset, so this phase also
    /// proves that no event mask, random address, Host buffer declaration or
    /// flow-control request has been accepted.
    pub const fn is_pristine(&self) -> bool {
        matches!(self.phase, BootstrapPhase::AwaitingReset)
    }

    /// Requested base HCI event mask in the current epoch.
    pub const fn event_mask(&self) -> EventMask {
        self.event_mask
    }

    /// Requested LE Meta event mask in the current epoch.
    pub const fn le_event_mask(&self) -> LeEventMask {
        self.le_event_mask
    }

    /// Requested random address, not a hardware-applied address.
    pub const fn requested_random_address(&self) -> Option<BdAddr> {
        self.requested_random_address
    }

    /// Host buffers declared for future Controller-to-Host ACL flow control.
    pub const fn host_buffers(&self) -> Option<BootstrapHostBuffers> {
        self.host_buffers
    }

    /// Requested Controller-to-Host flow-control mode.
    pub const fn controller_to_host_flow_control(&self) -> ControllerToHostFlowControl {
        self.controller_to_host_flow_control
    }

    /// Consume one classified bootstrap command under the current epoch policy.
    ///
    /// Classification is deliberately separate and immutable. A session owner
    /// decides when this method may run, in particular whether Reset must wait
    /// for active radio work to quiesce.
    pub(crate) fn dispatch_owned(
        &mut self,
        command: OwnedBootstrapCommand,
    ) -> BootstrapCommandCompleteEvent {
        let opcode = command.opcode();
        if !command.is_reset() && self.phase == BootstrapPhase::AwaitingReset {
            return command_error(opcode, HciError::CMD_DISALLOWED);
        }

        match command {
            OwnedBootstrapCommand::Reset => {
                self.reset_epoch();
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::SetEventMask(mask) => {
                self.event_mask = mask;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::SetControllerToHostFlowControl(mode) => {
                if !matches!(
                    mode,
                    ControllerToHostFlowControl::Off | ControllerToHostFlowControl::AclOnSyncOff
                ) {
                    return command_error(opcode, HciError::UNSUPPORTED);
                }
                self.controller_to_host_flow_control = mode;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::HostBufferSize {
                acl_data_packet_length,
                total_acl_data_packets,
            } => {
                self.host_buffers = Some(BootstrapHostBuffers {
                    acl_data_packet_length: acl_data_packet_length.get(),
                    total_acl_data_packets: total_acl_data_packets.get(),
                });
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::ReadBdAddr => {
                command_success(opcode, self.config.public_address.hci_wire_address().raw())
            }
            OwnedBootstrapCommand::LeSetEventMask(mask) => {
                self.le_event_mask = mask;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadBufferSize => {
                let mut response = [0; 3];
                response[..2].copy_from_slice(&self.config.le_acl_data_packet_length.to_le_bytes());
                response[2] = self.config.total_num_le_acl_data_packets;
                command_success(opcode, &response)
            }
            OwnedBootstrapCommand::LeReadLocalSupportedFeatures => {
                // The initial profile advertises no optional LE features. A
                // backend must close each independent feature before setting
                // its bit here.
                command_success(opcode, &[0; 8])
            }
            OwnedBootstrapCommand::LeSetRandomAddress(address) => {
                self.requested_random_address = Some(address);
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadFilterAcceptListSize => {
                command_success(opcode, &[self.config.filter_accept_list_size()])
            }
        }
    }

    fn reset_epoch(&mut self) {
        self.phase = BootstrapPhase::Configuring;
        self.event_mask = EventMask::new();
        self.le_event_mask = LeEventMask::new();
        self.requested_random_address = None;
        self.host_buffers = None;
        self.controller_to_host_flow_control = ControllerToHostFlowControl::Off;
    }
}

fn parse_complete<T: for<'packet> FromHciBytes<'packet>>(bytes: &[u8]) -> Option<T> {
    T::from_hci_bytes_complete(bytes).ok()
}

fn command_success(opcode: Opcode, parameters: &[u8]) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, Status::SUCCESS, parameters)
}

fn command_error(opcode: Opcode, error: HciError) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, error.to_status(), &[])
}

pub(crate) fn invalid_parameters(opcode: Opcode) -> BootstrapCommandCompleteEvent {
    command_error(opcode, HciError::INVALID_HCI_PARAMETERS)
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes,
        cmd::{
            Cmd, Opcode, OpcodeGroup, SyncCmd,
            controller_baseband::{
                HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask,
                SetEventMaskPage2,
            },
            info::ReadBdAddr,
            le::{
                LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
                LeSetAdvEnable, LeSetEventMask, LeSetRandomAddr,
            },
        },
        controller::{Controller, ExternalController},
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{
            BdAddr, ControllerToHostFlowControl, Error as HciError, EventMask, EventMaskPage2,
            LeEventMask, Status,
        },
        transport::{PacketToController, Transport},
    };
    use embassy_futures::{
        block_on,
        join::{join, join3},
        select::{Either, select},
    };
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
    use trouble_host::{BleHostError, Error as TroubleError, HostResources, Packet, PacketPool};

    use crate::{
        HciCommandPacket, HostToControllerFrame, InProcessHciChannel,
        InProcessHciControllerEndpoint, InProcessHciHostTransport,
        LeControllerCommandClassification, classify_le_controller_command,
    };

    use super::{
        BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapConfigError, BootstrapHostBuffers,
        BootstrapPhase, LeControllerBootstrap, LeControllerBootstrapConfig, OwnedBootstrapCommand,
        command_error,
    };

    type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 32>;
    type TestHost<'channel> = InProcessHciHostTransport<'channel, NoopRawMutex, 1, 1, 32>;
    type TestController<'channel> =
        InProcessHciControllerEndpoint<'channel, NoopRawMutex, 1, 1, 32>;

    struct TestPacket([u8; 64]);

    impl AsRef<[u8]> for TestPacket {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    impl AsMut<[u8]> for TestPacket {
        fn as_mut(&mut self) -> &mut [u8] {
            &mut self.0
        }
    }

    impl Packet for TestPacket {}

    struct TestPacketPool;

    impl PacketPool for TestPacketPool {
        type Packet = TestPacket;

        const MTU: usize = 64;

        fn allocate() -> Option<Self::Packet> {
            Some(TestPacket([0; 64]))
        }

        fn capacity() -> usize {
            2
        }
    }

    #[test]
    fn trouble_no_security_bootstrap_and_conservative_extensions_are_supported() {
        let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
        let config = LeControllerBootstrapConfig::new(public_address, 251, 4).unwrap();
        let mut bootstrap = LeControllerBootstrap::new(config);
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let random_address = BdAddr::new([0xc6, 5, 4, 3, 2, 1]);
        let event_mask = EventMask::new()
            .enable_le_meta(true)
            .enable_hardware_error(true)
            .enable_disconnection_complete(true);
        let le_event_mask = LeEventMask::new()
            .enable_le_conn_complete(true)
            .enable_le_adv_report(true)
            .enable_le_conn_update_complete(true);

        block_on(async {
            assert_success(
                round_trip(&host, &controller, &mut bootstrap, &Reset::new()).await,
                &[],
            );
            assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &LeSetRandomAddr::new(random_address),
                )
                .await,
                &[],
            );
            assert_eq!(bootstrap.requested_random_address(), Some(random_address));

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &SetEventMask::new(event_mask),
                )
                .await,
                &[],
            );
            assert_eq!(bootstrap.event_mask(), event_mask);

            let unsupported_page_2 = round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &SetEventMaskPage2::new(EventMaskPage2::new().enable_encryption_change_v2(true)),
            )
            .await;
            assert_eq!(unsupported_page_2.status, HciError::UNKNOWN_CMD.to_status());

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &LeSetEventMask::new(le_event_mask),
                )
                .await,
                &[],
            );
            assert_eq!(bootstrap.le_event_mask(), le_event_mask);

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &LeReadFilterAcceptListSize::new(),
                )
                .await,
                &[0],
            );
            assert_success(
                round_trip(&host, &controller, &mut bootstrap, &LeReadBufferSize::new()).await,
                &[251, 0, 4],
            );

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &HostBufferSize::new(255, 0, 1, 0),
                )
                .await,
                &[],
            );
            assert_eq!(
                bootstrap.host_buffers(),
                Some(BootstrapHostBuffers {
                    acl_data_packet_length: 255,
                    total_acl_data_packets: 1,
                })
            );

            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &SetControllerToHostFlowControl::new(ControllerToHostFlowControl::AclOnSyncOff),
                )
                .await,
                &[],
            );
            assert_eq!(
                bootstrap.controller_to_host_flow_control(),
                ControllerToHostFlowControl::AclOnSyncOff
            );

            assert_success(
                round_trip(&host, &controller, &mut bootstrap, &ReadBdAddr::new()).await,
                &[6, 5, 4, 3, 2, 1],
            );
            assert_success(
                round_trip(
                    &host,
                    &controller,
                    &mut bootstrap,
                    &LeReadLocalSupportedFeatures::new(),
                )
                .await,
                &[0; 8],
            );

            let advertising = round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeSetAdvEnable::new(true),
            )
            .await;
            assert_eq!(advertising.status, HciError::UNKNOWN_CMD.to_status());

            assert_success(
                round_trip(&host, &controller, &mut bootstrap, &Reset::new()).await,
                &[],
            );
            assert_eq!(bootstrap.event_mask(), EventMask::new());
            assert_eq!(bootstrap.le_event_mask(), LeEventMask::new());
            assert_eq!(bootstrap.requested_random_address(), None);
            assert_eq!(bootstrap.host_buffers(), None);
            assert_eq!(
                bootstrap.controller_to_host_flow_control(),
                ControllerToHostFlowControl::Off
            );
        });
    }

    #[test]
    fn read_bd_addr_converts_canonical_identity_at_the_hci_boundary() {
        let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
        let config = LeControllerBootstrapConfig::new(public_address, 27, 1).unwrap();
        assert_eq!(
            config.public_address().canonical_bytes(),
            [1, 2, 3, 4, 5, 6]
        );

        let mut bootstrap = LeControllerBootstrap::new(config);
        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::Reset)
                .status(),
            Status::SUCCESS
        );
        let response = bootstrap.dispatch_owned(OwnedBootstrapCommand::ReadBdAddr);
        assert_eq!(response.status(), Status::SUCCESS);
        assert_eq!(&response.as_bytes()[6..], &[6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn external_controller_exec_completes_from_bootstrap_dispatch() {
        const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            27,
            1,
        )
        .unwrap();
        let mut bootstrap = LeControllerBootstrap::new(config);
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let external = ExternalController::<_, 1>::new(host);

        block_on(async {
            let reset = Reset::new();
            let mut event_buffer = external.alloc_buf().unwrap();
            let worker = async {
                let mut command_buffer = [0; 32];
                let HostToControllerFrame::Command(command) =
                    controller.receive(&mut command_buffer).await.unwrap()
                else {
                    panic!("Reset changed packet kind");
                };
                let response = dispatch_test_packet(&mut bootstrap, command);
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
                    .unwrap();
                controller
                    .publish(bt_hci::PacketKind::Event, &HARDWARE_ERROR)
                    .await
                    .unwrap();
            };

            let (completed, observed, ()) = join3(
                reset.exec(&external),
                external.read(&mut event_buffer),
                worker,
            )
            .await;
            completed.unwrap();
            assert!(matches!(
                observed.unwrap(),
                ControllerToHostPacket::Event(_)
            ));
            assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
        });
    }

    #[test]
    fn real_trouble_runner_reaches_initialized_over_the_source_owned_hci_boundary() {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            251,
            4,
        )
        .unwrap();
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let mut bootstrap = LeControllerBootstrap::new(config);
        let external = ExternalController::<_, 2>::new(host);
        let mut resources = HostResources::<TestPacketPool, 1, 1>::new();
        let stack = trouble_host::new(external, &mut resources).build();
        let mut runner = stack.runner();
        let mut peripheral = stack.peripheral();
        let stop = Signal::<NoopRawMutex, ()>::new();

        block_on(async {
            let initialized_probe = async {
                let result = peripheral.set_filter_accept_list(&[]).await;
                stop.signal(());
                result
            };

            // This public Trouble operation cannot emit its command until the
            // Runner has completed its initial ACL/mask bootstrap and published
            // the internal initialized state. The conservative bootstrap then
            // rejects the operational command because no filter-list owner or
            // Link Layer exists yet.
            let controller_and_probe = join(
                drive_bootstrap_until(&controller, &mut bootstrap, &stop),
                initialized_probe,
            );
            match select(runner.run(), controller_and_probe).await {
                Either::First(result) => {
                    panic!("Trouble Runner stopped during bootstrap: {result:?}")
                }
                Either::Second(((), probe_result)) => {
                    assert!(matches!(
                        probe_result,
                        Err(BleHostError::BleHost(TroubleError::Hci(
                            HciError::UNKNOWN_CMD
                        )))
                    ));
                }
            }
        });

        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
        assert_eq!(
            bootstrap.host_buffers(),
            Some(BootstrapHostBuffers {
                acl_data_packet_length: 255,
                total_acl_data_packets: 1,
            })
        );
    }

    async fn drive_bootstrap_until(
        controller: &TestController<'_>,
        bootstrap: &mut LeControllerBootstrap,
        stop: &Signal<NoopRawMutex, ()>,
    ) {
        let mut command_buffer = [0; 32];
        loop {
            let command = match select(stop.wait(), controller.receive(&mut command_buffer)).await {
                Either::First(()) => return,
                Either::Second(Ok(HostToControllerFrame::Command(command))) => command,
                Either::Second(Ok(frame)) => {
                    panic!("bootstrap received unsupported {:?} packet", frame.kind())
                }
                Either::Second(Err(error)) => panic!("bootstrap receive failed: {error:?}"),
            };
            let response = dispatch_test_packet(bootstrap, command);
            controller
                .publish(bt_hci::PacketKind::Event, response.as_bytes())
                .await
                .expect("bootstrap response enters the raw Controller endpoint");
        }
    }

    #[test]
    fn known_commands_are_disallowed_before_reset_and_malformed_input_never_mutates() {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            27,
            1,
        )
        .unwrap();
        let mut bootstrap = LeControllerBootstrap::new(config);

        let before_reset =
            bootstrap.dispatch_owned(OwnedBootstrapCommand::SetEventMask(EventMask::new()));
        assert_eq!(before_reset.status(), HciError::CMD_DISALLOWED.to_status());
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

        let malformed_reset = dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::for_test(Reset::OPCODE, &[0]),
        );
        assert_eq!(
            malformed_reset.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::Reset)
                .status(),
            Status::SUCCESS
        );
        let malformed_mask = dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::for_test(SetEventMask::OPCODE, &[0; 7]),
        );
        assert_eq!(
            malformed_mask.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert_eq!(bootstrap.event_mask(), EventMask::new());

        let sync_host_buffers = [0xff, 0x00, 1, 1, 0, 1, 0];
        assert_eq!(
            dispatch_test_packet(
                &mut bootstrap,
                HciCommandPacket::for_test(HostBufferSize::OPCODE, &sync_host_buffers),
            )
            .status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert_eq!(bootstrap.host_buffers(), None);

        assert_eq!(
            dispatch_test_packet(
                &mut bootstrap,
                HciCommandPacket::for_test(SetControllerToHostFlowControl::OPCODE, &[2]),
            )
            .status(),
            HciError::UNSUPPORTED.to_status()
        );
        assert_eq!(
            bootstrap.controller_to_host_flow_control(),
            ControllerToHostFlowControl::Off
        );

        let unknown = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 1);
        assert_eq!(
            dispatch_test_packet(&mut bootstrap, HciCommandPacket::for_test(unknown, &[]),)
                .status(),
            HciError::UNKNOWN_CMD.to_status()
        );
    }

    #[test]
    fn capability_table_excludes_link_layer_and_optional_page_two_commands() {
        for opcode in [
            Reset::OPCODE,
            SetEventMask::OPCODE,
            SetControllerToHostFlowControl::OPCODE,
            HostBufferSize::OPCODE,
            ReadBdAddr::OPCODE,
            LeSetEventMask::OPCODE,
            LeReadBufferSize::OPCODE,
            LeReadLocalSupportedFeatures::OPCODE,
            LeSetRandomAddr::OPCODE,
            LeReadFilterAcceptListSize::OPCODE,
        ] {
            assert!(BootstrapCommand::supports(opcode));
        }
        assert!(!BootstrapCommand::supports(SetEventMaskPage2::OPCODE));
        assert!(!BootstrapCommand::supports(LeSetAdvEnable::OPCODE));
    }

    #[test]
    fn bootstrap_config_rejects_profiles_without_acl_capacity() {
        assert_eq!(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
                0,
                1,
            ),
            Err(BootstrapConfigError::ZeroAclDataPacketLength)
        );
        assert_eq!(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
                27,
                0,
            ),
            Err(BootstrapConfigError::ZeroAclDataPacketCount)
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ObservedCommandComplete {
        opcode: Opcode,
        status: Status,
        parameters: [u8; 8],
        parameter_length: usize,
    }

    impl ObservedCommandComplete {
        fn parameters(&self) -> &[u8] {
            &self.parameters[..self.parameter_length]
        }
    }

    async fn round_trip<T: PacketToController>(
        host: &TestHost<'_>,
        controller: &TestController<'_>,
        bootstrap: &mut LeControllerBootstrap,
        command: &T,
    ) -> ObservedCommandComplete {
        host.write(command).await.unwrap();
        let mut command_buffer = [0; 32];
        let HostToControllerFrame::Command(command) =
            controller.receive(&mut command_buffer).await.unwrap()
        else {
            panic!("bootstrap command changed packet kind");
        };
        let response = dispatch_test_packet(bootstrap, command);
        controller
            .publish(bt_hci::PacketKind::Event, response.as_bytes())
            .await
            .unwrap();

        let mut event_buffer = [0; 32];
        let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap()
        else {
            panic!("Command Complete changed packet kind");
        };
        assert_eq!(event.kind, EventKind::CommandComplete);
        let event = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
        let event: CommandCompleteWithStatus<'_> = event.try_into().unwrap();
        let mut parameters = [0; 8];
        parameters[..event.return_param_bytes.len()].copy_from_slice(&event.return_param_bytes);
        ObservedCommandComplete {
            opcode: event.cmd_opcode,
            status: event.status,
            parameters,
            parameter_length: event.return_param_bytes.len(),
        }
    }

    fn assert_success(observed: ObservedCommandComplete, parameters: &[u8]) {
        assert_eq!(observed.status, Status::SUCCESS);
        assert_eq!(observed.parameters(), parameters);
    }

    fn dispatch_test_packet(
        bootstrap: &mut LeControllerBootstrap,
        command: HciCommandPacket<'_>,
    ) -> super::BootstrapCommandCompleteEvent {
        match classify_le_controller_command(command) {
            LeControllerCommandClassification::Bootstrap(command) => {
                bootstrap.dispatch_owned(command)
            }
            LeControllerCommandClassification::MalformedBootstrap(response) => response,
            LeControllerCommandClassification::Dtm(command) => {
                command_error(command.kind().opcode(), HciError::UNKNOWN_CMD)
            }
            LeControllerCommandClassification::MalformedDtm(response) => {
                command_error(response.opcode(), HciError::UNKNOWN_CMD)
            }
            LeControllerCommandClassification::Unsupported(response) => {
                command_error(response.opcode(), HciError::UNKNOWN_CMD)
            }
        }
    }
}
