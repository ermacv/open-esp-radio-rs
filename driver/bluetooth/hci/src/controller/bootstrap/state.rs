//! Reset-scoped Host configuration and bootstrap command admission.

use super::*;

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

    /// Dispatch one non-Reset bootstrap command while a radio role is active.
    ///
    /// The random address cannot change while advertising or scanning is
    /// enabled. Rejecting it here leaves the epoch's previously accepted
    /// address untouched. Active-session routers retain Reset separately until
    /// hardware quiescence and must not pass Reset through this helper.
    pub(crate) fn dispatch_owned_while_radio_active(
        &mut self,
        command: OwnedBootstrapCommand,
    ) -> BootstrapCommandCompleteEvent {
        if matches!(command, OwnedBootstrapCommand::LeSetRandomAddress(_)) {
            return command_error(command.opcode(), HciError::CMD_DISALLOWED);
        }
        self.dispatch_owned(command)
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
