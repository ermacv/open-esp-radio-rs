//! Owned bootstrap command decoding without mutable Controller state.

use super::*;

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

fn parse_complete<T: for<'packet> FromHciBytes<'packet>>(bytes: &[u8]) -> Option<T> {
    T::from_hci_bytes_complete(bytes).ok()
}
