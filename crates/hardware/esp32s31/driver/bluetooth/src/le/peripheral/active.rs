//! Contiguous Peripheral LE1M events with independently retained HCI order.
//!
//! Requires an explicit local-clock timing policy in the runtime configuration.
//! Central feature requests and unsupported optional LLCP requests enter a
//! bounded control-response queue. Active HCI commands share the radio wait;
//! Disconnect retains Command Status order through acknowledged LL termination,
//! and Reset retires the exact graph before bootstrap mutation. Host ACL packets
//! are retained as one credit, fragmented into 27-byte plaintext or 23-byte
//! encrypted legacy LL payloads, and completed only after final peer
//! acknowledgement. Accepted peer LL Data PDUs
//! enter a bounded Controller-to-Host ACL queue after Connection Complete.
//! Host Buffer Size and Host Number Of Completed Packets bound delivery for the
//! sole live handle. The special credit command remains serialized behind an
//! older pending normal response, but flow-controlled output keeps intake live
//! when the Host ACL owner is occupied. Central Connection Update and Channel Map
//! Update retain their exact instant transitions through scheduler admission.
//! Peer termination retires the unlinked graph and restores ordered idle HCI intake.
//! An unanswered initial transmit window recurs with its full WinSize; six
//! events without establishment retire the connection with reason `0x3e`.
//! A running event which exceeds its absolute completion budget enters the
//! common hardware stop sequence, then unlinks and recycles as an aborted
//! event; stop and post-unlink waits are independently finite.
//! Established supervision uses the independent hardware valid-RX time and
//! retires expired unlinked connections with reason `0x08`.
//! A guarded recurring window missed before RUN is cancelled and rebuilt at a
//! later established event; supervision bounds the jump, while a crossed
//! Connection Update or Channel Map Update instant closes with reason `0x28`.
//! Local termination arms `T_terminate` from fresh Controller time immediately
//! before the first PDU enters the TX graph and retires on acknowledgement or
//! expiry after the connection supervision timeout.
//! Version exchange requires a caller-supplied Controller implementation identity.
//! A disconnected handle is not reusable until every Host-owned Controller ACL
//! buffer from that connection has returned its flow-control credit.
//! Any radio fault or unsupported mandatory-control transition seals its owners.

#![forbid(unsafe_code)]

pub(crate) mod acl;
mod host_events;
mod radio;

use super::first_hci::{
    LegacyConnectablePeripheralFirstHciAxis as Axis,
    LegacyConnectablePeripheralFirstHciOrder as Order,
    LegacyConnectablePeripheralFirstHciOrderPublication as OrderPublication,
    LegacyConnectablePeripheralFirstHciResponsePublication as Publication,
    LegacyConnectablePeripheralFirstHciResponseWait as ResponseWait,
    LegacyConnectablePeripheralFirstHciRunning as FirstRunning,
    LegacyConnectablePeripheralFirstHciRunningOrder as RunningOrder,
};
use crate::controller::SchedulerRunInterruptStorage;
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActivePeripheralCommandRoute as HciCommandRoute,
    LeControllerActivePeripheralIntake as HciIntake, LeControllerClassifiedCommand,
    LeControllerCommandEndpoint, LeControllerEndpointMismatch, LeControllerResetBarrier,
};
pub use radio::{PeripheralConnectionActiveFaultCause, PeripheralConnectionActiveWait};

/// Outcome of publishing the next ordered Host-visible connection event.
#[must_use = "retain the returned active connection owner"]
pub enum PeripheralConnectionHostEventPublication<Session> {
    None(Session),
    Published(Session),
    Masked(Session),
    /// An older ordered command response still owns Controller output order.
    OrderedResponsePending(Session),
    Pending(Session),
    FlowControlled(Session),
    EndpointMismatch(Session),
    Fault {
        session: Session,
        error: HciChannelError,
    },
}

/// Sole owner of completion, successor preparation, and the ordered response.
#[must_use = "drive or retain the exact active connection owner"]
pub struct PeripheralConnectionActiveSession<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    order: Order<'a, radio::Radio<'a, S, N>>,
    control: oer_bluetooth_ll::control::LePeripheralControl,
    encryption: oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    supervision: Option<super::supervision::PeripheralSupervisionDeadline>,
    termination: Option<super::termination::PeripheralTerminationDeadline>,
    procedure: Option<super::procedure::PeripheralProcedureDeadline>,
    progress_deadline: super::progress::PeripheralConnectionProgressDeadline,
    host_events: host_events::PeripheralConnectionHostEvents,
    acl: acl::PeripheralConnectionAcl,
    disconnect: Option<oer_bluetooth_hci::LeDisconnectCommand>,
    read_remote_features_after_status: bool,
    read_remote_version_after_status: bool,
}

struct PeripheralConnectionState<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    radio: radio::Radio<'a, S, N>,
    control: oer_bluetooth_ll::control::LePeripheralControl,
    encryption: oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    supervision: Option<super::supervision::PeripheralSupervisionDeadline>,
    termination: Option<super::termination::PeripheralTerminationDeadline>,
    procedure: Option<super::procedure::PeripheralProcedureDeadline>,
    progress_deadline: super::progress::PeripheralConnectionProgressDeadline,
    host_events: host_events::PeripheralConnectionHostEvents,
    acl: acl::PeripheralConnectionAcl,
    disconnect: Option<oer_bluetooth_hci::LeDisconnectCommand>,
    read_remote_features_after_status: bool,
    read_remote_version_after_status: bool,
}

/// Reset plus the complete active connection graph awaiting quiescence.
#[must_use = "retain Reset and the active connection until quiescence"]
pub struct PeripheralConnectionResetBarrier<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    barrier: LeControllerResetBarrier<'a, PeripheralConnectionState<'a, S, N>>,
}

/// One finite Reset-quiescence transition.
#[must_use = "retain the stopping connection, idle Reset barrier, or sealed fault"]
pub enum PeripheralConnectionResetStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Continue(PeripheralConnectionResetBarrier<'a, S, N>),
    Ready(crate::controller::ControllerIdleResetBarrier<'a, S, N>),
    Fault(PeripheralConnectionResetFault<'a, S, N>),
}

/// Sealed Reset and lower radio owner after a quiescence failure.
#[must_use = "retain the Reset fault until the hardware is quarantined"]
pub struct PeripheralConnectionResetFault<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    radio: radio::Fault<'a, S, N>,
    _barrier: LeControllerResetBarrier<'a, ()>,
    _control: oer_bluetooth_ll::control::LePeripheralControl,
    _encryption: oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    _supervision: Option<super::supervision::PeripheralSupervisionDeadline>,
    _termination: Option<super::termination::PeripheralTerminationDeadline>,
    _procedure: Option<super::procedure::PeripheralProcedureDeadline>,
    _progress_deadline: super::progress::PeripheralConnectionProgressDeadline,
    _host_events: host_events::PeripheralConnectionHostEvents,
    _acl: acl::PeripheralConnectionAcl,
    _disconnect: Option<oer_bluetooth_hci::LeDisconnectCommand>,
    _read_remote_features_after_status: bool,
    _read_remote_version_after_status: bool,
}

impl<S: SchedulerRunInterruptStorage, const N: usize> PeripheralConnectionResetFault<'_, S, N> {
    pub const fn cause(&self) -> PeripheralConnectionActiveFaultCause {
        self.radio.cause
    }
}

/// Opaque mismatch after active peripheral command intake.
#[must_use = "retain the command and active connection owner"]
pub struct PeripheralConnectionCommandMismatch<
    'a,
    'command,
    S: SchedulerRunInterruptStorage,
    const N: usize,
> {
    _command: LeControllerClassifiedCommand<'a, 'command, PeripheralConnectionState<'a, S, N>>,
}

/// Routed command outcome for one active peripheral connection.
#[must_use = "retain the returned lifecycle owner"]
pub enum PeripheralConnectionCommandRoute<
    'a,
    'command,
    S: SchedulerRunInterruptStorage,
    const N: usize,
> {
    ResponsePending(PeripheralConnectionActiveSession<'a, S, N>),
    ResetBarrier(PeripheralConnectionResetBarrier<'a, S, N>),
    EndpointMismatch(PeripheralConnectionCommandMismatch<'a, 'command, S, N>),
}

/// One non-blocking command intake through the connection's affine HCI authority.
#[must_use = "route the command or retain the returned active connection"]
pub enum PeripheralConnectionCommandIntake<
    'a,
    'command,
    'buffer,
    S: SchedulerRunInterruptStorage,
    const N: usize,
> {
    Routed {
        route: PeripheralConnectionCommandRoute<'a, 'command, S, N>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    Acl {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        buffer: &'buffer mut [u8],
    },
    HostCompletedPackets {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        buffer: &'buffer mut [u8],
    },
    NonCommand {
        session: PeripheralConnectionActiveSession<'a, S, N>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// One finite radio transition; only `Published` represents a new scheduler RUN.
#[must_use = "retain the returned session or sealed failure"]
pub enum PeripheralConnectionActiveStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    /// Termination or failed establishment returned the ordered idle command owner.
    Stopped {
        task: crate::controller::ControllerIdleCommandTask<'a, S, N>,
        reason: u8,
    },
    Continue(PeripheralConnectionActiveSession<'a, S, N>),
    Published(PeripheralConnectionActiveSession<'a, S, N>),
    Fault(PeripheralConnectionActiveFault<'a, S, N>),
}

/// Sealed radio transaction and HCI authority; does not authorize reclamation.
#[must_use = "retain the fault until the hardware is quarantined"]
pub struct PeripheralConnectionActiveFault<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    radio: radio::Fault<'a, S, N>,
    _order: Order<'a, ()>,
    _control: oer_bluetooth_ll::control::LePeripheralControl,
    _encryption: oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    _supervision: Option<super::supervision::PeripheralSupervisionDeadline>,
    _termination: Option<super::termination::PeripheralTerminationDeadline>,
    _procedure: Option<super::procedure::PeripheralProcedureDeadline>,
    _progress_deadline: super::progress::PeripheralConnectionProgressDeadline,
    _host_events: host_events::PeripheralConnectionHostEvents,
    _acl: acl::PeripheralConnectionAcl,
    _disconnect: Option<oer_bluetooth_hci::LeDisconnectCommand>,
    _read_remote_features_after_status: bool,
    _read_remote_version_after_status: bool,
}

impl<S: SchedulerRunInterruptStorage, const N: usize> PeripheralConnectionActiveFault<'_, S, N> {
    pub const fn cause(&self) -> PeripheralConnectionActiveFaultCause {
        self.radio.cause
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionActiveSession<'a, S, N>
{
    pub fn from_first(first: FirstRunning<'a, S, N>) -> Self {
        let local_version = first.local_version_information();
        let (running, order) = first.into_parts();
        let order = match order {
            RunningOrder::CommandReady(order) => Order::CommandReady(order),
            RunningOrder::ResponsePending(order) => Order::ResponsePending(order),
        };
        Self {
            order: order.map_owner(|()| radio::Radio::Running(running)),
            control: oer_bluetooth_ll::control::LePeripheralControl::new()
                .with_local_version(local_version),
            encryption: oer_bluetooth_ll::security::LePeripheralEncryptionProcedure::new(),
            supervision: None,
            termination: None,
            procedure: None,
            progress_deadline: super::progress::PeripheralConnectionProgressDeadline::new(
                S::monotonic_micros(),
            ),
            host_events: host_events::PeripheralConnectionHostEvents::new(),
            acl: acl::PeripheralConnectionAcl::new(),
            disconnect: None,
            read_remote_features_after_status: false,
            read_remote_version_after_status: false,
        }
    }

    pub const fn hci_axis(&self) -> Axis {
        self.order.axis()
    }

    /// Wait until the matching Host queue may contain another command.
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        match &self.order {
            Order::CommandReady(ready) => controller.wait_command_available(ready).await,
            Order::ResponsePending(_) => {
                unreachable!("a response-pending connection cannot accept a command")
            }
        }
    }

    /// Consume and route at most one command while preserving both active axes.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
    ) -> PeripheralConnectionCommandIntake<'a, 'command, 'buffer, S, N> {
        let Self {
            order,
            control,
            encryption,
            supervision,
            termination,
            procedure,
            progress_deadline,
            host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        let Order::CommandReady(ready) = order else {
            unreachable!("a response-pending connection cannot route a command")
        };
        let live_handle = host_events.live_handle();
        let local_procedure_busy =
            control.local_procedure_pending() || encryption.blocks_unrelated_transmission();
        let local_version_available = control.remote_version_request_available();
        let long_term_key_request_pending = encryption.long_term_key_request().is_some();
        let ready = ready.map_owner(|radio| PeripheralConnectionState {
            radio,
            control,
            encryption,
            supervision,
            termination,
            procedure,
            progress_deadline,
            host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        });
        match controller.try_receive_active_peripheral_with_buffer(
            ready,
            live_handle,
            buffer,
            |mut state, packet| {
                state.acl.accept_host_packet(packet);
                let completed = state.acl.take_completed_host_packets();
                state.host_events.observe_acl_completed(completed);
                state
            },
        ) {
            HciIntake::Command { command, buffer } => {
                let route = match controller.route_active_peripheral_classified_command(
                    command,
                    live_handle,
                    local_procedure_busy,
                    local_version_available,
                    long_term_key_request_pending,
                ) {
                    HciCommandRoute::ResponsePending(pending) => {
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::Disconnect(disconnect) => {
                        let pending = disconnect.into_accepted_status().map_owner(|accepted| {
                            let (mut state, command) = accepted.into_parts();
                            state.disconnect = Some(command);
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ReadRemoteFeatures(request) => {
                        let pending = request.into_accepted_status().map_owner(|mut state| {
                            use oer_bluetooth_ll::control::LeRemoteFeaturesAdmission;
                            assert!(matches!(
                                state.control.admit_remote_feature_request(),
                                LeRemoteFeaturesAdmission::Admitted
                                    | LeRemoteFeaturesAdmission::Cached(_)
                            ));
                            state.read_remote_features_after_status = true;
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ReadRemoteVersionInformation(request) => {
                        let pending = request.into_accepted_status().map_owner(|mut state| {
                            use oer_bluetooth_ll::control::LeRemoteVersionAdmission;
                            assert!(matches!(
                                state.control.admit_remote_version_request(),
                                LeRemoteVersionAdmission::Admitted
                                    | LeRemoteVersionAdmission::Cached(_)
                            ));
                            state.read_remote_version_after_status = true;
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::LongTermKeyReply(reply) => {
                        let pending = reply.into_accepted_complete().map_owner(|accepted| {
                            let (mut state, long_term_key) = accepted.into_parts();
                            state
                                .encryption
                                .provide_long_term_key(
                                    oer_bluetooth_ll::security::LeLongTermKey::new(long_term_key),
                                )
                                .unwrap_or_else(|_| {
                                    unreachable!(
                                        "the HCI router admitted only a pending LTK request"
                                    )
                                });
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::LongTermKeyNegativeReply(reply) => {
                        let pending = reply.into_accepted_complete().map_owner(|mut state| {
                            state.encryption.reject_long_term_key().unwrap_or_else(|_| {
                                unreachable!("the HCI router admitted only a pending LTK request")
                            });
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ResetBarrier(barrier) => {
                        PeripheralConnectionCommandRoute::ResetBarrier(
                            PeripheralConnectionResetBarrier { barrier },
                        )
                    }
                    HciCommandRoute::EndpointMismatch(command) => {
                        PeripheralConnectionCommandRoute::EndpointMismatch(
                            PeripheralConnectionCommandMismatch { _command: command },
                        )
                    }
                };
                PeripheralConnectionCommandIntake::Routed { route, buffer }
            }
            HciIntake::Acl { ready, buffer } => PeripheralConnectionCommandIntake::Acl {
                session: Self::from_ready(ready),
                buffer,
            },
            HciIntake::HostCompletedPackets {
                ready,
                command,
                buffer,
            } => {
                let (mut state, ready) = ready.into_parts();
                let result = command.and_then(|command| {
                    let credit_handle = state.acl.credit_handle(live_handle);
                    state
                        .acl
                        .accept_host_completed_packets(command, credit_handle)
                });
                match result {
                    Ok(()) => PeripheralConnectionCommandIntake::HostCompletedPackets {
                        session: state.into_session(Order::CommandReady(ready)),
                        buffer,
                    },
                    Err(response) => {
                        let pending = ready
                            .map_owner(|()| state)
                            .begin_host_completed_packets_error(response);
                        PeripheralConnectionCommandIntake::Routed {
                            route: PeripheralConnectionCommandRoute::ResponsePending(
                                Self::from_pending(pending),
                            ),
                            buffer,
                        }
                    }
                }
            }
            HciIntake::Empty { ready, buffer } => PeripheralConnectionCommandIntake::Empty {
                session: Self::from_ready(ready),
                buffer,
            },
            HciIntake::EndpointMismatch { ready, buffer } => {
                PeripheralConnectionCommandIntake::EndpointMismatch {
                    session: Self::from_ready(ready),
                    buffer,
                }
            }
            HciIntake::Channel {
                ready,
                buffer,
                error,
            } => PeripheralConnectionCommandIntake::Channel {
                session: Self::from_ready(ready),
                buffer,
                error,
            },
            HciIntake::NonCommand { ready, frame } => {
                PeripheralConnectionCommandIntake::NonCommand {
                    session: Self::from_ready(ready),
                    frame,
                }
            }
        }
    }

    fn from_ready(
        ready: oer_bluetooth_hci::LeControllerCommandReady<'a, PeripheralConnectionState<'a, S, N>>,
    ) -> Self {
        let (state, ready) = ready.into_parts();
        state.into_session(Order::CommandReady(ready))
    }

    fn from_pending(
        pending: oer_bluetooth_hci::LeControllerResponsePending<
            'a,
            PeripheralConnectionState<'a, S, N>,
        >,
    ) -> Self {
        let (state, pending) = pending.into_parts();
        state.into_session(Order::ResponsePending(pending))
    }

    /// Borrow readiness from the retained phase. `None` requires an immediate step.
    pub fn radio_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        self.order.owner().wait()
    }

    /// Whether an established/failed connection or disconnection event awaits HCI publication.
    pub const fn has_pending_host_event(&self) -> bool {
        self.host_events.has_pending() || self.acl.has_controller_packet()
    }

    /// Whether command/data intake has room for another owned Host ACL packet.
    pub const fn can_accept_host_packet(&self) -> bool {
        self.acl.can_accept_host_packet()
    }

    /// Whether the next retained Controller ACL packet awaits a Host credit.
    pub fn host_event_is_flow_controlled<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> bool {
        if !self.host_events.connection_result_published() || !self.acl.has_controller_packet() {
            return false;
        }
        let profile = controller.controller_to_host_acl_profile();
        self.acl.controller_packet_is_flow_controlled(
            profile.is_flow_controlled(),
            profile.total_packets(),
        )
    }

    /// Wait for possible capacity without moving the retained connection event.
    pub async fn wait_host_event_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        if !self.order.accepts_endpoint(controller) {
            return Err(LeControllerEndpointMismatch);
        }
        controller.wait_peripheral_connection_event_capacity().await;
        Ok(())
    }

    /// Try the next ordered unsolicited event after older command completion.
    pub fn try_publish_host_event<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        mut self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> PeripheralConnectionHostEventPublication<Self> {
        use host_events::PeripheralConnectionHostEventPublication as Publication;
        if !self.order.accepts_endpoint(controller) {
            return PeripheralConnectionHostEventPublication::EndpointMismatch(self);
        }
        if !matches!(&self.order, Order::CommandReady(_)) {
            return PeripheralConnectionHostEventPublication::OrderedResponsePending(self);
        }
        observe_encryption_host_events(&mut self.encryption, &mut self.host_events);
        match self.host_events.try_publish(controller) {
            Publication::Published => {
                return PeripheralConnectionHostEventPublication::Published(self);
            }
            Publication::Masked => {
                if self.host_events.take_long_term_key_request_masked() {
                    self.encryption.reject_long_term_key().unwrap_or_else(|_| {
                        unreachable!("only the retained LTK request can be masked")
                    });
                }
                return PeripheralConnectionHostEventPublication::Masked(self);
            }
            Publication::Pending => {
                return PeripheralConnectionHostEventPublication::Pending(self);
            }
            Publication::Fault(error) => {
                return PeripheralConnectionHostEventPublication::Fault {
                    session: self,
                    error,
                };
            }
            Publication::None => {}
        }
        if self.host_events.connection_result_published() {
            let profile = controller.controller_to_host_acl_profile();
            match self.acl.try_publish_controller_packet(
                controller,
                profile.maximum_payload(),
                profile.is_flow_controlled(),
                profile.total_packets(),
            ) {
                acl::ControllerAclPublication::Published => {
                    return PeripheralConnectionHostEventPublication::Published(self);
                }
                acl::ControllerAclPublication::Pending => {
                    return PeripheralConnectionHostEventPublication::Pending(self);
                }
                acl::ControllerAclPublication::FlowControlled => {
                    return PeripheralConnectionHostEventPublication::FlowControlled(self);
                }
                acl::ControllerAclPublication::Fault(error) => {
                    return PeripheralConnectionHostEventPublication::Fault {
                        session: self,
                        error,
                    };
                }
                acl::ControllerAclPublication::None => {}
            }
        }
        PeripheralConnectionHostEventPublication::None(self)
    }

    // Do not merge the affine radio transition's temporaries into the much
    // larger controller dispatch future's stack frame.
    #[inline(never)]
    pub fn step_radio(
        self,
        random: &mut impl super::PeripheralEncryptionRandomSource,
    ) -> PeripheralConnectionActiveStep<'a, S, N> {
        let Self {
            order,
            mut control,
            mut encryption,
            mut supervision,
            mut termination,
            mut procedure,
            mut progress_deadline,
            mut host_events,
            mut acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        let (radio, order) = order.into_parts();
        if let Some(deadline_phase) = radio.deadline_phase()
            && progress_deadline.expired(S::monotonic_micros())
        {
            if matches!(deadline_phase, radio::DeadlinePhase::SchedulerCompletion) {
                match radio.begin_completion_abort() {
                    Ok(radio) => {
                        return PeripheralConnectionActiveStep::Continue(Self {
                            order: order.map_owner(|()| radio),
                            control,
                            encryption,
                            supervision,
                            termination,
                            procedure,
                            progress_deadline:
                                super::progress::PeripheralConnectionProgressDeadline::for_stop(
                                    S::monotonic_micros(),
                                ),
                            host_events,
                            acl,
                            disconnect,
                            read_remote_features_after_status,
                            read_remote_version_after_status,
                        });
                    }
                    Err(radio) => {
                        return PeripheralConnectionActiveStep::Fault(
                            PeripheralConnectionActiveFault {
                                radio,
                                _order: order,
                                _control: control,
                                _encryption: encryption,
                                _supervision: supervision,
                                _termination: termination,
                                _procedure: procedure,
                                _progress_deadline: progress_deadline,
                                _host_events: host_events,
                                _acl: acl,
                                _disconnect: disconnect,
                                _read_remote_features_after_status:
                                    read_remote_features_after_status,
                                _read_remote_version_after_status: read_remote_version_after_status,
                            },
                        );
                    }
                }
            }
            let cause = match deadline_phase {
                radio::DeadlinePhase::SchedulerStop => {
                    PeripheralConnectionActiveFaultCause::CompletionAbortDeadlineExpired
                }
                radio::DeadlinePhase::PostUnlink => {
                    PeripheralConnectionActiveFaultCause::UnlinkDeadlineExpired
                }
                radio::DeadlinePhase::ControllerTime => {
                    PeripheralConnectionActiveFaultCause::ControllerTimeDeadlineExpired
                }
                radio::DeadlinePhase::SchedulerCompletion => unreachable!(),
            };
            return PeripheralConnectionActiveStep::Fault(PeripheralConnectionActiveFault {
                radio: radio.expire_deadline(cause),
                _order: order,
                _control: control,
                _encryption: encryption,
                _supervision: supervision,
                _termination: termination,
                _procedure: procedure,
                _progress_deadline: progress_deadline,
                _host_events: host_events,
                _acl: acl,
                _disconnect: disconnect,
                _read_remote_features_after_status: read_remote_features_after_status,
                _read_remote_version_after_status: read_remote_version_after_status,
            });
        }
        if matches!(&radio, radio::Radio::Stopped { .. }) {
            control.close_remote_feature_request();
            control.close_remote_version_request();
        }
        observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
        observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
        observe_encryption_host_events(&mut encryption, &mut host_events);
        if encryption.termination_reason() == Some(0x06) {
            control.request_local_termination(0x06);
        }
        let (radio, order) = match (radio, order) {
            (radio::Radio::Stopped { task, reason }, Order::CommandReady(ready))
                if super::progress::retirement_barrier_is_ready(
                    host_events.ready_to_restore_idle(),
                    !acl.has_controller_packet(),
                    acl.controller_credits_settled(),
                ) =>
            {
                return PeripheralConnectionActiveStep::Stopped {
                    task: crate::controller::ControllerIdleCommandTask::from_parts(task, ready),
                    reason,
                };
            }
            pair => pair,
        };
        let step = radio.step(
            &mut control,
            &mut encryption,
            &mut acl,
            radio::Deadlines {
                supervision: &mut supervision,
                termination: &mut termination,
                procedure: &mut procedure,
            },
            &mut host_events,
            random,
        );
        observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
        observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
        observe_encryption_host_events(&mut encryption, &mut host_events);
        match step {
            radio::Step::Continue(radio) => PeripheralConnectionActiveStep::Continue(Self {
                order: order.map_owner(|()| radio),
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            }),
            radio::Step::Published(radio) => {
                progress_deadline = super::progress::PeripheralConnectionProgressDeadline::new(
                    S::monotonic_micros(),
                );
                PeripheralConnectionActiveStep::Published(Self {
                    order: order.map_owner(|()| radio),
                    control,
                    encryption,
                    supervision,
                    termination,
                    procedure,
                    progress_deadline,
                    host_events,
                    acl,
                    disconnect,
                    read_remote_features_after_status,
                    read_remote_version_after_status,
                })
            }
            radio::Step::Fault(radio) => {
                PeripheralConnectionActiveStep::Fault(PeripheralConnectionActiveFault {
                    radio,
                    _order: order,
                    _control: control,
                    _encryption: encryption,
                    _supervision: supervision,
                    _termination: termination,
                    _procedure: procedure,
                    _progress_deadline: progress_deadline,
                    _host_events: host_events,
                    _acl: acl,
                    _disconnect: disconnect,
                    _read_remote_features_after_status: read_remote_features_after_status,
                    _read_remote_version_after_status: read_remote_version_after_status,
                })
            }
        }
    }

    /// Cancellation leaves the exact response and radio phase in this session.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<ResponseWait, LeControllerEndpointMismatch> {
        self.order.wait_response_capacity(controller).await
    }

    pub fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Publication<Self> {
        let Self {
            order,
            mut control,
            encryption,
            supervision,
            termination,
            mut procedure,
            progress_deadline,
            mut host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        match order.try_publish_response(controller) {
            OrderPublication::CommandReady(order) => Publication::CommandReady(Self {
                order,
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            }),
            OrderPublication::Published(order) => {
                if let Some(command) = disconnect {
                    control.request_host_termination(command.reason());
                }
                if read_remote_features_after_status {
                    if matches!(order.owner(), radio::Radio::Stopped { .. }) {
                        control.close_remote_feature_request();
                    } else if let Some(features) = control.activate_remote_feature_request() {
                        host_events.observe_remote_features(
                            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Success(features),
                        );
                    }
                    observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
                }
                if read_remote_version_after_status {
                    if matches!(order.owner(), radio::Radio::Stopped { .. }) {
                        control.close_remote_version_request();
                    } else if let Some(version) = control.activate_remote_version_request() {
                        host_events.observe_remote_version(
                            oer_bluetooth_ll::control::LeRemoteVersionResult::Success(version),
                        );
                    }
                    observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
                }
                Publication::Published(Self {
                    order,
                    control,
                    encryption,
                    supervision,
                    termination,
                    procedure,
                    progress_deadline,
                    host_events,
                    acl,
                    disconnect: None,
                    read_remote_features_after_status: false,
                    read_remote_version_after_status: false,
                })
            }
            OrderPublication::Pending(order) => Publication::Pending(Self {
                order,
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            }),
            OrderPublication::EndpointMismatch(order) => Publication::EndpointMismatch(Self {
                order,
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            }),
            OrderPublication::Fault { order, error } => Publication::Fault {
                state: Self {
                    order,
                    control,
                    encryption,
                    supervision,
                    termination,
                    procedure,
                    progress_deadline,
                    host_events,
                    acl,
                    disconnect,
                    read_remote_features_after_status,
                    read_remote_version_after_status,
                },
                error,
            },
        }
    }
}

fn observe_remote_feature_result(
    control: &mut oer_bluetooth_ll::control::LePeripheralControl,
    procedure: &mut Option<super::procedure::PeripheralProcedureDeadline>,
    host_events: &mut host_events::PeripheralConnectionHostEvents,
) {
    if let Some(result) = control.take_remote_features_result() {
        *procedure = None;
        host_events.observe_remote_features(result);
    }
}

fn observe_encryption_host_events(
    encryption: &mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    host_events: &mut host_events::PeripheralConnectionHostEvents,
) {
    if let Some(request) = encryption.take_long_term_key_request() {
        host_events.observe_long_term_key_request(request);
    }
    if encryption.take_encryption_enabled() {
        host_events.observe_encryption_enabled();
    }
    if encryption.take_encryption_refreshed() {
        host_events.observe_encryption_refreshed();
    }
}

fn observe_remote_version_result(
    control: &mut oer_bluetooth_ll::control::LePeripheralControl,
    procedure: &mut Option<super::procedure::PeripheralProcedureDeadline>,
    host_events: &mut host_events::PeripheralConnectionHostEvents,
) {
    if let Some(result) = control.take_remote_version_result() {
        *procedure = None;
        host_events.observe_remote_version(result);
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize> PeripheralConnectionState<'a, S, N> {
    fn into_session(self, order: Order<'a, ()>) -> PeripheralConnectionActiveSession<'a, S, N> {
        PeripheralConnectionActiveSession {
            order: order.map_owner(|()| self.radio),
            control: self.control,
            encryption: self.encryption,
            supervision: self.supervision,
            termination: self.termination,
            procedure: self.procedure,
            progress_deadline: self.progress_deadline,
            host_events: self.host_events,
            acl: self.acl,
            disconnect: self.disconnect,
            read_remote_features_after_status: self.read_remote_features_after_status,
            read_remote_version_after_status: self.read_remote_version_after_status,
        }
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionResetBarrier<'a, S, N>
{
    /// Borrow the current radio wait while retaining Reset ownership.
    pub fn radio_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        self.barrier.owner().radio.reset_wait()
    }

    /// Retire the connection graph before exposing the idle Reset barrier.
    pub fn step(
        self,
        random: &mut impl super::PeripheralEncryptionRandomSource,
    ) -> PeripheralConnectionResetStep<'a, S, N> {
        let (mut state, barrier) = self.barrier.into_parts();
        if let Some(deadline_phase) = state.radio.deadline_phase()
            && state.progress_deadline.expired(S::monotonic_micros())
        {
            let PeripheralConnectionState {
                radio,
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            } = state;
            if matches!(deadline_phase, radio::DeadlinePhase::SchedulerCompletion) {
                match radio.begin_completion_abort() {
                    Ok(radio) => {
                        return PeripheralConnectionResetStep::Continue(Self {
                            barrier: barrier.map_owner(|()| PeripheralConnectionState {
                                radio,
                                control,
                                encryption,
                                supervision,
                                termination,
                                procedure,
                                progress_deadline:
                                    super::progress::PeripheralConnectionProgressDeadline::for_stop(
                                        S::monotonic_micros(),
                                    ),
                                host_events,
                                acl,
                                disconnect,
                                read_remote_features_after_status,
                                read_remote_version_after_status,
                            }),
                        });
                    }
                    Err(radio) => {
                        return PeripheralConnectionResetStep::Fault(
                            PeripheralConnectionResetFault {
                                radio,
                                _barrier: barrier,
                                _control: control,
                                _encryption: encryption,
                                _supervision: supervision,
                                _termination: termination,
                                _procedure: procedure,
                                _progress_deadline: progress_deadline,
                                _host_events: host_events,
                                _acl: acl,
                                _disconnect: disconnect,
                                _read_remote_features_after_status:
                                    read_remote_features_after_status,
                                _read_remote_version_after_status: read_remote_version_after_status,
                            },
                        );
                    }
                }
            }
            let cause = match deadline_phase {
                radio::DeadlinePhase::SchedulerStop => {
                    PeripheralConnectionActiveFaultCause::CompletionAbortDeadlineExpired
                }
                radio::DeadlinePhase::PostUnlink => {
                    PeripheralConnectionActiveFaultCause::UnlinkDeadlineExpired
                }
                radio::DeadlinePhase::ControllerTime => {
                    PeripheralConnectionActiveFaultCause::ControllerTimeDeadlineExpired
                }
                radio::DeadlinePhase::SchedulerCompletion => unreachable!(),
            };
            return PeripheralConnectionResetStep::Fault(PeripheralConnectionResetFault {
                radio: radio.expire_deadline(cause),
                _barrier: barrier,
                _control: control,
                _encryption: encryption,
                _supervision: supervision,
                _termination: termination,
                _procedure: procedure,
                _progress_deadline: progress_deadline,
                _host_events: host_events,
                _acl: acl,
                _disconnect: disconnect,
                _read_remote_features_after_status: read_remote_features_after_status,
                _read_remote_version_after_status: read_remote_version_after_status,
            });
        }
        match state.radio.step_reset(
            &mut state.control,
            &mut state.encryption,
            &mut state.acl,
            radio::Deadlines {
                supervision: &mut state.supervision,
                termination: &mut state.termination,
                procedure: &mut state.procedure,
            },
            &mut state.host_events,
            random,
        ) {
            radio::ResetStep::Continue(radio) => {
                state.radio = radio;
                PeripheralConnectionResetStep::Continue(Self {
                    barrier: barrier.map_owner(|()| state),
                })
            }
            radio::ResetStep::Quiesced(task) => PeripheralConnectionResetStep::Ready(
                crate::controller::ControllerIdleResetBarrier::new(barrier.map_owner(|()| task)),
            ),
            radio::ResetStep::Fault(radio) => {
                PeripheralConnectionResetStep::Fault(PeripheralConnectionResetFault {
                    radio,
                    _barrier: barrier,
                    _control: state.control,
                    _encryption: state.encryption,
                    _supervision: state.supervision,
                    _termination: state.termination,
                    _procedure: state.procedure,
                    _progress_deadline: state.progress_deadline,
                    _host_events: state.host_events,
                    _acl: state.acl,
                    _disconnect: state.disconnect,
                    _read_remote_features_after_status: state.read_remote_features_after_status,
                    _read_remote_version_after_status: state.read_remote_version_after_status,
                })
            }
        }
    }
}
