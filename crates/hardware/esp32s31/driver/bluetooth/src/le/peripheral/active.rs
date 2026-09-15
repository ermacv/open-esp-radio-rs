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
//!
//! Private HCI coordination owns endpoint-facing intake and Host publication,
//! while lifecycle coordination steps normal and Reset radio transitions.
//! Both implement this same affine session; the Embassy runtime owns executor
//! waits but never an independently returnable radio/HCI half.

#![forbid(unsafe_code)]

pub(crate) mod acl;
mod hci;
mod host_events;
mod lifecycle;
mod radio;

use super::first_hci::{
    LegacyConnectablePeripheralFirstHciResponsePublication as Publication,
    LegacyConnectablePeripheralFirstHciRunning as FirstRunning,
    LegacyConnectablePeripheralFirstHciRunningOrder as RunningOrder,
};
use super::hci_order::{
    LegacyConnectablePeripheralFirstHciAxis as Axis,
    LegacyConnectablePeripheralFirstHciOrder as Order,
    LegacyConnectablePeripheralFirstHciOrderPublication as OrderPublication,
    LegacyConnectablePeripheralFirstHciResponseWait as ResponseWait,
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

    /// Borrow readiness from the retained phase. `None` requires an immediate step.
    pub fn radio_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        self.order.owner().wait()
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
