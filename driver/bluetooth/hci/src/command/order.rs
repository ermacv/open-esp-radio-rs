//! Portable ordering authority for LE Controller command responses.
//!
//! Radio progress and Controller-to-Host capacity are deliberately independent:
//! a chip-specific runner may transform the retained owner while the exact
//! Command Complete remains pending. The resource-owned initial claim establishes
//! [`LeControllerCommandReady`]; thereafter only successful insertion into the
//! matching HCI epoch restores that authority.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use super::advertising::LeLegacyAdvertisingActiveEnableDisposition;
use super::scanning::{
    LeLegacyScanningActiveEnableDisposition, LeLegacyScanningIdleEnableDisposition,
};
use crate::{
    HciChannelError, HciClassifiedCommandIntake, HciControllerResponse, HciEpochBound,
    HciEpochIdentity, HostToControllerFrame, LeControllerCommandClassification,
    LeControllerCommandComplete, LeControllerCommandEndpoint, LeDtmActiveSessionDisposition,
    LeDtmCommand, LeDtmIdleSessionDisposition, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingIdleEnableDisposition, LeLegacyConnectableAdvertisingEnableRequest,
    LeLegacyNonconnectableAdvertisingEnableRequest, LeLegacyScanningEnableCommand,
    LeLegacyScanningEnableRequest, LeReceiverTestCommand, LeTestEndCommand,
    LeTransmitterTestCommand, OwnedBootstrapCommand,
};

/// A combined Controller endpoint does not match retained affine HCI authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerEndpointMismatch;

/// One consumed and classified Host command inseparable from next-command authority.
///
/// Only a matching [`LeControllerCommandEndpoint`] can construct this value.
/// Its classification is intentionally not exposed: an idle or active router
/// must consume the complete aggregate before another command can be accepted.
#[must_use = "route the classified command without separating its affine authority"]
pub struct LeControllerClassifiedCommand<'epoch, 'command, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: HciEpochBound<'command, LeControllerCommandClassification>,
}

impl<Owner> LeControllerClassifiedCommand<'_, '_, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }
}

/// One non-blocking command intake through the combined Controller endpoint.
///
/// A command result keeps its classification and affine authority opaque and
/// inseparable. Every branch that consumes no command returns the exact
/// authority for a lossless retry.
#[must_use = "route the command or retain the returned command-ready authority"]
pub enum LeControllerCommandIntake<'epoch, 'command, 'buffer, Owner> {
    /// One complete classified command plus reusable scratch storage.
    Command {
        /// Opaque classification and sole next-command authority.
        command: LeControllerClassifiedCommand<'epoch, 'command, Owner>,
        /// Scratch storage no longer borrowed by the owned classification.
        buffer: &'buffer mut [u8],
    },
    /// A readiness hint became stale before intake.
    Empty {
        /// Unchanged next-command authority.
        ready: LeControllerCommandReady<'epoch, Owner>,
        /// Scratch storage available for another wait.
        buffer: &'buffer mut [u8],
    },
    /// The authority belongs to another Controller epoch; no packet was consumed.
    EndpointMismatch {
        /// Unchanged next-command authority.
        ready: LeControllerCommandReady<'epoch, Owner>,
        /// Scratch storage available to the matching endpoint.
        buffer: &'buffer mut [u8],
    },
    /// A packet-boundary failure consumed no command authority.
    Channel {
        /// Unchanged next-command authority.
        ready: LeControllerCommandReady<'epoch, Owner>,
        /// Scratch storage available to the supervisor or a corrected retry.
        buffer: &'buffer mut [u8],
        /// Exact transport failure.
        error: HciChannelError,
    },
    /// The oldest Host packet was data rather than a command.
    NonCommand {
        /// Unchanged next-command authority for later command intake.
        ready: LeControllerCommandReady<'epoch, Owner>,
        /// Data frame retaining its source-epoch proof and buffer borrow.
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// One endpoint-validated receiver start retaining its response-order epoch.
///
/// The semantic command is available only as a borrowed projection. A chip
/// runner may transform or temporarily split the lifecycle owner, but the
/// command and response order remain together until hardware has actually
/// entered the started state.
#[must_use = "retain the deferred receiver start until hardware starts or rejects it"]
pub struct LeControllerDeferredReceiverStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeReceiverTestCommand,
}

impl<'epoch, Owner> LeControllerDeferredReceiverStart<'epoch, Owner> {
    /// Borrow the idle lifecycle owner without releasing command or order.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Borrow the validated receiver command without releasing response order.
    pub const fn command(&self) -> &LeReceiverTestCommand {
        &self.command
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredReceiverStart<'epoch, Next> {
        LeControllerDeferredReceiverStart {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate only the lifecycle owner while retaining command and order in
    /// an opaque unit continuation suitable for a chip-owned state machine.
    pub fn into_parts(self) -> (Owner, LeControllerDeferredReceiverStart<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredReceiverStart {
                ready,
                command: self.command,
            },
        )
    }

    /// Construct the exact receiver-start success selected by a hardware composition.
    ///
    /// This portable layer preserves semantics and order but cannot observe
    /// hardware `RUN`; the chip-owned state machine must enforce that proof.
    pub fn into_started_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_started_command_complete())
    }

    /// Construct the receiver-start Hardware Failure selected by a hardware composition.
    ///
    /// This portable layer consumes the semantic command and command-order
    /// authority without accepting a caller-selected status. Proving that
    /// hardware is recovered remains the chip-owned state machine's job.
    pub fn into_hardware_failure_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_hardware_failure_command_complete())
    }
}

/// One endpoint-validated transmitter start retaining its response-order epoch.
#[must_use = "retain the deferred transmitter start until hardware starts or rejects it"]
pub struct LeControllerDeferredTransmitterStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeTransmitterTestCommand,
}

impl<'epoch, Owner> LeControllerDeferredTransmitterStart<'epoch, Owner> {
    /// Borrow the idle lifecycle owner without releasing command or order.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Borrow the validated transmitter command without releasing response order.
    pub const fn command(&self) -> &LeTransmitterTestCommand {
        &self.command
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredTransmitterStart<'epoch, Next> {
        LeControllerDeferredTransmitterStart {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate only the lifecycle owner while retaining command and order in
    /// an opaque unit continuation suitable for a chip-owned state machine.
    pub fn into_parts(self) -> (Owner, LeControllerDeferredTransmitterStart<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredTransmitterStart {
                ready,
                command: self.command,
            },
        )
    }

    /// Construct the exact transmitter-start success selected by a hardware composition.
    ///
    /// This portable layer preserves semantics and order but cannot observe
    /// hardware `RUN`; the chip-owned state machine must enforce that proof.
    pub fn into_started_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_started_command_complete())
    }

    /// Construct the transmitter-start Hardware Failure selected by a hardware composition.
    ///
    /// This portable layer consumes the semantic command and command-order
    /// authority without accepting a caller-selected status. Proving that
    /// hardware is recovered remains the chip-owned state machine's job.
    pub fn into_hardware_failure_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_hardware_failure_command_complete())
    }
}

/// One endpoint-validated nonconnectable advertising Enable retaining response order.
#[must_use = "retain the deferred nonconnectable start until hardware starts or rejects it"]
pub struct LeControllerDeferredLegacyNonconnectableAdvertisingStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyAdvertisingEnableCommand,
    request: LeLegacyNonconnectableAdvertisingEnableRequest,
}

/// One endpoint-validated connectable advertising Enable retaining response order.
#[must_use = "retain the deferred connectable start until hardware starts or rejects it"]
pub struct LeControllerDeferredLegacyConnectableAdvertisingStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyAdvertisingEnableCommand,
    request: LeLegacyConnectableAdvertisingEnableRequest,
}

/// One endpoint-validated advertising Disable retaining response order.
///
/// Success cannot be constructed until the chip-specific owner has stopped
/// publication, retired the running scheduler graph and recovered CPU-owned
/// Link Layer memory.
#[must_use = "retain advertising Disable until hardware is quiescent"]
pub struct LeControllerDeferredLegacyAdvertisingDisable<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyAdvertisingEnableCommand,
}

impl<'epoch, Owner> LeControllerDeferredLegacyAdvertisingDisable<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyAdvertisingDisable<'epoch, Next> {
        LeControllerDeferredLegacyAdvertisingDisable {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate the lifecycle owner from the opaque Disable/order continuation.
    pub fn into_parts(
        self,
    ) -> (
        Owner,
        LeControllerDeferredLegacyAdvertisingDisable<'epoch, ()>,
    ) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyAdvertisingDisable {
                ready,
                command: self.command,
            },
        )
    }

    /// Complete Disable only after the chip-specific lifecycle proves quiescence.
    pub fn into_stopped_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_stopped_command_complete())
    }
}

impl<'epoch, Owner> LeControllerDeferredLegacyNonconnectableAdvertisingStart<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Immutable configuration snapshot accepted at the Enable boundary.
    pub const fn request(&self) -> LeLegacyNonconnectableAdvertisingEnableRequest {
        self.request
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyNonconnectableAdvertisingStart<'epoch, Next> {
        LeControllerDeferredLegacyNonconnectableAdvertisingStart {
            ready: self.ready.map_owner(map),
            command: self.command,
            request: self.request,
        }
    }

    /// Separate only the lifecycle owner while retaining command and order.
    pub fn into_parts(
        self,
    ) -> (
        Owner,
        LeControllerDeferredLegacyNonconnectableAdvertisingStart<'epoch, ()>,
    ) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyNonconnectableAdvertisingStart {
                ready,
                command: self.command,
                request: self.request,
            },
        )
    }

    /// Complete Enable only after the chip runner proves hardware `RUN`.
    pub fn into_started_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_started_command_complete())
    }

    /// Complete with Hardware Failure only after the chip runner recovers idle ownership.
    pub fn into_hardware_failure_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_hardware_failure_command_complete())
    }
}

impl<'epoch, Owner> LeControllerDeferredLegacyConnectableAdvertisingStart<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Immutable connectable configuration snapshot accepted at Enable order.
    pub const fn request(&self) -> LeLegacyConnectableAdvertisingEnableRequest {
        self.request
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyConnectableAdvertisingStart<'epoch, Next> {
        LeControllerDeferredLegacyConnectableAdvertisingStart {
            ready: self.ready.map_owner(map),
            command: self.command,
            request: self.request,
        }
    }

    /// Separate only the lifecycle owner while retaining command and order.
    pub fn into_parts(
        self,
    ) -> (
        Owner,
        LeControllerDeferredLegacyConnectableAdvertisingStart<'epoch, ()>,
    ) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyConnectableAdvertisingStart {
                ready,
                command: self.command,
                request: self.request,
            },
        )
    }

    /// Complete Enable only after the chip runner proves hardware `RUN`.
    pub fn into_started_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_started_command_complete())
    }

    /// Complete with Hardware Failure only after the chip runner recovers idle ownership.
    pub fn into_hardware_failure_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_hardware_failure_command_complete())
    }
}

/// One endpoint-validated passive scanning Enable retaining response order.
#[must_use = "retain the deferred scanner start until hardware starts or rejects it"]
pub struct LeControllerDeferredLegacyScanningStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyScanningEnableCommand,
    request: LeLegacyScanningEnableRequest,
}

impl<'epoch, Owner> LeControllerDeferredLegacyScanningStart<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Immutable passive scanner configuration captured at Enable order.
    pub const fn request(&self) -> LeLegacyScanningEnableRequest {
        self.request
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyScanningStart<'epoch, Next> {
        LeControllerDeferredLegacyScanningStart {
            ready: self.ready.map_owner(map),
            command: self.command,
            request: self.request,
        }
    }

    /// Separate the lifecycle owner from the opaque Enable/order continuation.
    pub fn into_parts(self) -> (Owner, LeControllerDeferredLegacyScanningStart<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyScanningStart {
                ready,
                command: self.command,
                request: self.request,
            },
        )
    }

    /// Complete Enable only after hardware proves entry into `RUN`.
    pub fn into_started_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_started_command_complete())
    }

    /// Reject Enable after a failed start and recovered hardware owner.
    pub fn into_hardware_failure_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_hardware_failure_command_complete())
    }
}

/// One endpoint-validated passive scanning Disable retaining response order.
#[must_use = "retain scanner Disable until hardware is quiescent"]
pub struct LeControllerDeferredLegacyScanningDisable<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyScanningEnableCommand,
}

impl<'epoch, Owner> LeControllerDeferredLegacyScanningDisable<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyScanningDisable<'epoch, Next> {
        LeControllerDeferredLegacyScanningDisable {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate the lifecycle owner from the opaque Disable/order continuation.
    pub fn into_parts(self) -> (Owner, LeControllerDeferredLegacyScanningDisable<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyScanningDisable {
                ready,
                command: self.command,
            },
        )
    }

    /// Complete Disable only after scanner publication and hardware are quiescent.
    pub fn into_stopped_response(self) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_stopped_command_complete())
    }
}

/// One endpoint-validated DTM command retaining next-command authority.
///
/// The command is intentionally not exposed separately. Active-session policy
/// consumes this aggregate so a semantic command from one epoch cannot be
/// paired with command-ready authority from another epoch.
#[must_use = "route the retained DTM command under one session policy"]
pub struct LeControllerDeferredDtmCommand<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeDtmCommand,
}

impl<'epoch, Owner> LeControllerDeferredDtmCommand<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Apply active-session DTM policy without separating command and order.
    pub fn into_active_session_route(self) -> LeControllerActiveDtmCommandRoute<'epoch, Owner> {
        match self.command.into_active_session_disposition() {
            LeDtmActiveSessionDisposition::RejectControllerBusy(response) => {
                LeControllerActiveDtmCommandRoute::ResponsePending(
                    self.ready.begin_next_response(response),
                )
            }
            LeDtmActiveSessionDisposition::End(command) => {
                LeControllerActiveDtmCommandRoute::TestEnd(LeControllerDeferredTestEnd {
                    ready: self.ready,
                    command,
                })
            }
        }
    }
}

/// Portable active-session disposition of one endpoint-validated DTM command.
#[must_use = "publish Controller Busy or retain Test End through hardware quiescence"]
pub enum LeControllerActiveDtmCommandRoute<'epoch, Owner> {
    /// A second RX/TX start became the fixed Controller Busy response.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// Test End retains command and order until the chip supplies its packet count.
    TestEnd(LeControllerDeferredTestEnd<'epoch, Owner>),
}

/// Portable command policy while one legacy advertising set is active.
#[must_use = "publish the response or retain Disable/Reset through hardware quiescence"]
pub enum LeControllerActiveLegacyAdvertisingCommandRoute<'epoch, 'command, Owner> {
    /// A command completed without changing the active advertising lifecycle.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// Disable remains ordered until the exact active graph becomes CPU-owned.
    Disable(LeControllerDeferredLegacyAdvertisingDisable<'epoch, Owner>),
    /// Reset remains ordered and undispatched until the active graph is quiescent.
    ResetBarrier(LeControllerResetBarrier<'epoch, Owner>),
    /// The aggregate belongs to another endpoint and remains inseparable.
    EndpointMismatch(LeControllerClassifiedCommand<'epoch, 'command, Owner>),
}

/// Portable command policy while passive scanning owns the radio lifecycle.
#[must_use = "publish the response or retain Disable/Reset through hardware quiescence"]
pub enum LeControllerActiveLegacyScanningCommandRoute<'epoch, 'command, Owner> {
    /// A command completed without changing the active scanning lifecycle.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// Disable remains ordered until publication stops and hardware is quiescent.
    Disable(LeControllerDeferredLegacyScanningDisable<'epoch, Owner>),
    /// Reset remains ordered and undispatched until the active graph is quiescent.
    ResetBarrier(LeControllerResetBarrier<'epoch, Owner>),
    /// The aggregate belongs to another endpoint and remains inseparable.
    EndpointMismatch(LeControllerClassifiedCommand<'epoch, 'command, Owner>),
}

/// Endpoint-bound Test End retained until active hardware has quiesced.
#[must_use = "retain Test End until the exact terminal packet count is available"]
pub struct LeControllerDeferredTestEnd<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeTestEndCommand,
}

impl<'epoch, Owner> LeControllerDeferredTestEnd<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredTestEnd<'epoch, Next> {
        LeControllerDeferredTestEnd {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate only the lifecycle owner while retaining command and order.
    pub fn into_parts(self) -> (Owner, LeControllerDeferredTestEnd<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredTestEnd {
                ready,
                command: self.command,
            },
        )
    }

    /// Construct the exact Test End completion after hardware quiescence.
    pub fn into_ended_response(
        self,
        packet_count: u16,
    ) -> LeControllerResponsePending<'epoch, Owner> {
        self.ready
            .begin_next_response(self.command.into_ended_command_complete(packet_count))
    }
}

/// Portable result of routing one complete Controller classification while the
/// hardware session is idle.
#[must_use = "start hardware, publish the response, or retain the exact mismatch"]
pub enum LeControllerIdleClassifiedCommandRoute<'epoch, 'command, Owner> {
    /// A receiver start retains its semantic command and response order.
    StartReceiver(LeControllerDeferredReceiverStart<'epoch, Owner>),
    /// A transmitter start retains its semantic command and response order.
    StartTransmitter(LeControllerDeferredTransmitterStart<'epoch, Owner>),
    /// Nonconnectable advertising Enable retains its immutable configuration and order.
    StartLegacyNonconnectableAdvertising(
        LeControllerDeferredLegacyNonconnectableAdvertisingStart<'epoch, Owner>,
    ),
    /// Connectable advertising Enable retains response-capable configuration and order.
    StartLegacyConnectableAdvertising(
        LeControllerDeferredLegacyConnectableAdvertisingStart<'epoch, Owner>,
    ),
    /// Passive scanning Enable retains one immutable configuration and response order.
    StartLegacyScanning(LeControllerDeferredLegacyScanningStart<'epoch, Owner>),
    /// The classification completed synchronously into one ordered response.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// Reset remains ordered but undispatched until lifecycle quiescence.
    ResetBarrier(LeControllerResetBarrier<'epoch, Owner>),
    /// The aggregate belongs to another endpoint and remains inseparable.
    EndpointMismatch(LeControllerClassifiedCommand<'epoch, 'command, Owner>),
}

/// An owner paired with one exact not-yet-published Controller response.
///
/// The owner remains available by shared reference and may be transformed with
/// [`Self::map_owner`] without exposing or rebuilding the response. Queue
/// backpressure, an endpoint mismatch, and every other transport error return
/// this complete owner unchanged.
#[must_use = "the owner and exact Controller response must remain retained"]
pub struct LeControllerResponsePending<'epoch, Owner> {
    owner: Owner,
    response: LeControllerCommandComplete,
    hci_epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Owner> LeControllerResponsePending<'epoch, Owner> {
    /// Borrow the independently progressing owner axis.
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Separate the owner from a unit response-order marker for typed composition.
    ///
    /// This is a consuming decomposition: the exact response bytes and epoch
    /// remain in `LeControllerResponsePending<()>`, and neither output can be
    /// recreated from the other. Chip-specific session aggregates immediately
    /// reunite both outputs around their independently progressing owner axis.
    pub fn into_parts(self) -> (Owner, LeControllerResponsePending<'epoch, ()>) {
        (
            self.owner,
            LeControllerResponsePending {
                owner: (),
                response: self.response,
                hci_epoch: self.hci_epoch,
            },
        )
    }

    /// Whether an endpoint may publish this exact pending response.
    ///
    /// This is publication affinity only; unlike the published typestate it
    /// grants no authority to receive or execute the next HCI command.
    pub fn matches_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch
            .same_epoch(controller.transport().epoch_identity())
    }

    /// Transform only the owner axis while retaining the exact response authority.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerResponsePending<'epoch, Next> {
        LeControllerResponsePending {
            owner: map(self.owner),
            response: self.response,
            hci_epoch: self.hci_epoch,
        }
    }

    /// Attempt the sole durable publication through the matching HCI epoch.
    ///
    /// Capacity is not reserved. `Full` therefore returns `Pending` for an exact
    /// later retry. No error releases the owner or response bytes.
    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LeControllerResponsePublication<'epoch, Owner> {
        if !self.matches_endpoint(controller) {
            return LeControllerResponsePublication::EndpointMismatch(self);
        }

        match controller
            .transport()
            .try_publish(self.response.kind(), self.response.as_bytes())
        {
            Ok(()) => LeControllerResponsePublication::Published(LeControllerCommandReady {
                owner: self.owner,
                hci_epoch: self.hci_epoch,
            }),
            Err(HciChannelError::Full) => LeControllerResponsePublication::Pending(self),
            Err(error) => LeControllerResponsePublication::Fault {
                pending: self,
                error,
            },
        }
    }
}

/// An owner carrying the sole authority to accept the next command in one HCI epoch.
///
/// Resources mint the initial authority once, and each durable response
/// publication returns its successor. This state contains no response bytes
/// and exposes no publication operation, making duplicate insertion
/// unrepresentable.
#[must_use = "the owner and its HCI affinity must remain retained"]
pub struct LeControllerCommandReady<'epoch, Owner> {
    owner: Owner,
    hci_epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Owner> LeControllerCommandReady<'epoch, Owner> {
    pub(crate) const fn initial(owner: Owner, hci_epoch: HciEpochIdentity<'epoch>) -> Self {
        Self { owner, hci_epoch }
    }

    /// Borrow the independently progressing owner axis.
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Separate the owner from its unit next-command authority for typed composition.
    pub fn into_parts(self) -> (Owner, LeControllerCommandReady<'epoch, ()>) {
        (
            self.owner,
            LeControllerCommandReady {
                owner: (),
                hci_epoch: self.hci_epoch,
            },
        )
    }

    /// Transform only the owner axis while retaining next-command authority.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerCommandReady<'epoch, Next> {
        LeControllerCommandReady {
            owner: map(self.owner),
            hci_epoch: self.hci_epoch,
        }
    }

    /// Begin the next ordered Controller response in this same live HCI epoch.
    ///
    /// The previous response authority has already been consumed by successful
    /// publication. A higher session layer must retain the semantic command and
    /// owner needed to construct `response`; this transition supplies
    /// only ordering and endpoint affinity.
    fn begin_next_response<Response>(
        self,
        response: Response,
    ) -> LeControllerResponsePending<'epoch, Owner>
    where
        Response: Into<LeControllerCommandComplete>,
    {
        LeControllerResponsePending {
            owner: self.owner,
            response: response.into(),
            hci_epoch: self.hci_epoch,
        }
    }

    /// Whether an endpoint belongs to the HCI epoch carrying this authority.
    ///
    /// Only the command-ready state exposes this authority, so command intake cannot
    /// be enabled while the preceding response is still pending.
    pub fn accepts_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch
            .same_epoch(controller.transport().epoch_identity())
    }
}

impl<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    LeControllerCommandEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Wait until a Host packet is observed while borrowing command authority.
    ///
    /// The wait neither consumes nor reserves a packet and borrows `ready`, so
    /// cancellation leaves the sole affine authority in the caller. A foreign
    /// authority fails before observing this endpoint's queue.
    pub async fn wait_command_available<'epoch, Owner>(
        &self,
        ready: &LeControllerCommandReady<'epoch, Owner>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        if !ready.accepts_endpoint(self) {
            return Err(LeControllerEndpointMismatch);
        }
        self.transport().wait_receive_ready().await;
        Ok(())
    }

    /// Consume and classify at most one Host command under affine authority.
    ///
    /// On success classification and authority remain inseparable in one
    /// opaque aggregate accepted only by this crate's session routers. Every
    /// branch without a command returns the exact authority unchanged.
    pub fn try_receive_classified_command_with_buffer<'epoch, 'buffer, Owner>(
        &self,
        ready: LeControllerCommandReady<'epoch, Owner>,
        buffer: &'buffer mut [u8],
    ) -> LeControllerCommandIntake<'epoch, 'resources, 'buffer, Owner> {
        if !ready.accepts_endpoint(self) {
            return LeControllerCommandIntake::EndpointMismatch { ready, buffer };
        }

        match self
            .transport()
            .try_receive_classified_command_with_buffer(buffer)
        {
            HciClassifiedCommandIntake::Command { command, buffer } => {
                LeControllerCommandIntake::Command {
                    command: LeControllerClassifiedCommand { ready, command },
                    buffer,
                }
            }
            HciClassifiedCommandIntake::Empty { buffer } => {
                LeControllerCommandIntake::Empty { ready, buffer }
            }
            HciClassifiedCommandIntake::Channel { error, buffer } => {
                LeControllerCommandIntake::Channel {
                    ready,
                    buffer,
                    error,
                }
            }
            HciClassifiedCommandIntake::NonCommand(frame) => {
                LeControllerCommandIntake::NonCommand { ready, frame }
            }
        }
    }

    /// Wait until response capacity is observed while borrowing the response.
    ///
    /// The wait reserves no slot and borrows `pending`, so cancellation leaves
    /// exact response bytes and owner available for `try_publish`.
    pub async fn wait_response_capacity<'epoch, Owner>(
        &self,
        pending: &LeControllerResponsePending<'epoch, Owner>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        if !pending.matches_endpoint(self) {
            return Err(LeControllerEndpointMismatch);
        }
        self.transport().wait_publish_ready().await;
        Ok(())
    }

    /// Route one complete Controller classification while hardware is idle.
    ///
    /// The affine command-ready authority and consumed command must both belong to this
    /// endpoint before any bootstrap mutation or semantic release occurs. RX/TX
    /// starts retain an opaque response-order token until the chip runner proves
    /// hardware start. Every synchronous branch becomes one exact pending
    /// response, including idle Test End, non-Reset bootstrap commands,
    /// malformed known commands and unsupported opcodes.
    pub fn route_idle_classified_command<'epoch, 'command, Owner>(
        &mut self,
        command: LeControllerClassifiedCommand<'epoch, 'command, Owner>,
    ) -> LeControllerIdleClassifiedCommandRoute<'epoch, 'command, Owner> {
        if !command.ready.accepts_endpoint(self)
            || !command.command.originates_from(self.transport())
        {
            return LeControllerIdleClassifiedCommandRoute::EndpointMismatch(command);
        }

        let LeControllerClassifiedCommand { ready, command } = command;
        let classification = command
            .try_into_for_endpoint(self.transport())
            .unwrap_or_else(|_| unreachable!("aggregate affinity was checked above"));

        match classification {
            LeControllerCommandClassification::Dtm(command) => {
                match command.into_idle_session_disposition() {
                    LeDtmIdleSessionDisposition::StartReceiver(command) => {
                        LeControllerIdleClassifiedCommandRoute::StartReceiver(
                            LeControllerDeferredReceiverStart { ready, command },
                        )
                    }
                    LeDtmIdleSessionDisposition::StartTransmitter(command) => {
                        LeControllerIdleClassifiedCommandRoute::StartTransmitter(
                            LeControllerDeferredTransmitterStart { ready, command },
                        )
                    }
                    LeDtmIdleSessionDisposition::CompleteNoTest(response) => {
                        LeControllerIdleClassifiedCommandRoute::ResponsePending(
                            ready.begin_next_response(response),
                        )
                    }
                }
            }
            LeControllerCommandClassification::Bootstrap(command) if command.is_reset() => {
                LeControllerIdleClassifiedCommandRoute::ResetBarrier(LeControllerResetBarrier {
                    ready,
                    command,
                })
            }
            LeControllerCommandClassification::Bootstrap(command) => {
                let response = self.dispatch_bootstrap_command(command);
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedBootstrap(response) => {
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedDtm(response) => {
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
                let response = self.dispatch_legacy_advertising_configuration(command);
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingEnable(command) => {
                match self.dispatch_idle_legacy_advertising_enable(command) {
                    LeLegacyAdvertisingIdleEnableDisposition::StartNonconnectable(request) => {
                        LeControllerIdleClassifiedCommandRoute::StartLegacyNonconnectableAdvertising(
                            LeControllerDeferredLegacyNonconnectableAdvertisingStart {
                                ready,
                                command,
                                request,
                            },
                        )
                    }
                    LeLegacyAdvertisingIdleEnableDisposition::StartConnectable(request) => {
                        LeControllerIdleClassifiedCommandRoute::StartLegacyConnectableAdvertising(
                            LeControllerDeferredLegacyConnectableAdvertisingStart {
                                ready,
                                command,
                                request,
                            },
                        )
                    }
                    LeLegacyAdvertisingIdleEnableDisposition::Complete(response) => {
                        LeControllerIdleClassifiedCommandRoute::ResponsePending(
                            ready.begin_next_response(response),
                        )
                    }
                }
            }
            LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyScanningConfiguration(command) => {
                let response = self.dispatch_legacy_scanning_configuration(command);
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyScanningEnable(command) => {
                match self.dispatch_idle_legacy_scanning_enable(command) {
                    LeLegacyScanningIdleEnableDisposition::Start(request) => {
                        LeControllerIdleClassifiedCommandRoute::StartLegacyScanning(
                            LeControllerDeferredLegacyScanningStart {
                                ready,
                                command,
                                request,
                            },
                        )
                    }
                    LeLegacyScanningIdleEnableDisposition::Complete(response) => {
                        LeControllerIdleClassifiedCommandRoute::ResponsePending(
                            ready.begin_next_response(response),
                        )
                    }
                }
            }
            LeControllerCommandClassification::MalformedLegacyScanning(response) => {
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::Unsupported(response) => {
                LeControllerIdleClassifiedCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
        }
    }

    /// Route one fully classified command through its complete Controller epoch.
    ///
    /// The command-ready authority and command must both belong to this
    /// endpoint. Terminal classifications and non-Reset bootstrap commands
    /// immediately enter the ordered response axis. DTM remains semantic for
    /// radio-session policy, while Reset becomes an opaque barrier without
    /// changing bootstrap state.
    pub fn route_classified_command<'epoch, 'command, Owner>(
        &mut self,
        command: LeControllerClassifiedCommand<'epoch, 'command, Owner>,
    ) -> LeControllerClassifiedCommandRoute<'epoch, 'command, Owner> {
        if !command.ready.accepts_endpoint(self)
            || !command.command.originates_from(self.transport())
        {
            return LeControllerClassifiedCommandRoute::EndpointMismatch(command);
        }
        let LeControllerClassifiedCommand { ready, command } = command;
        match command.try_into_for_endpoint(self.transport()) {
            Ok(classification) => match classification {
                LeControllerCommandClassification::Bootstrap(command) if command.is_reset() => {
                    LeControllerClassifiedCommandRoute::ResetBarrier(LeControllerResetBarrier {
                        ready,
                        command,
                    })
                }
                LeControllerCommandClassification::Bootstrap(command) => {
                    let response = self.dispatch_bootstrap_command(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::MalformedBootstrap(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::Dtm(command) => {
                    LeControllerClassifiedCommandRoute::Dtm(LeControllerDeferredDtmCommand {
                        ready,
                        command,
                    })
                }
                LeControllerCommandClassification::MalformedDtm(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
                    let response = self.dispatch_legacy_advertising_configuration(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::LegacyAdvertisingEnable(command) => {
                    let response =
                        self.complete_legacy_advertising_enable_while_radio_unavailable(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::LegacyScanningConfiguration(command) => {
                    let response = self.dispatch_legacy_scanning_configuration(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::LegacyScanningEnable(command) => {
                    let response =
                        self.complete_legacy_scanning_enable_while_radio_unavailable(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::MalformedLegacyScanning(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::Unsupported(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
            },
            Err(_) => unreachable!("aggregate affinity was checked above"),
        }
    }

    /// Route one command while legacy advertising owns the radio lifecycle.
    ///
    /// Advertising configuration is immutable from accepted Enable through
    /// completed Disable. Repeated Enable is an ordered no-op, while Disable
    /// and Reset retain their exact command-order tokens until hardware
    /// quiescence.
    pub fn route_active_legacy_advertising_classified_command<'epoch, 'command, Owner>(
        &mut self,
        command: LeControllerClassifiedCommand<'epoch, 'command, Owner>,
    ) -> LeControllerActiveLegacyAdvertisingCommandRoute<'epoch, 'command, Owner> {
        if !command.ready.accepts_endpoint(self)
            || !command.command.originates_from(self.transport())
        {
            return LeControllerActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command);
        }
        let LeControllerClassifiedCommand { ready, command } = command;
        let classification = command
            .try_into_for_endpoint(self.transport())
            .unwrap_or_else(|_| unreachable!("aggregate affinity was checked above"));

        match classification {
            LeControllerCommandClassification::Bootstrap(command) if command.is_reset() => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResetBarrier(
                    LeControllerResetBarrier { ready, command },
                )
            }
            LeControllerCommandClassification::Bootstrap(command) => {
                let response = self.dispatch_bootstrap_command_while_radio_active(command);
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedBootstrap(response) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::Dtm(command) => {
                let response = match command.into_idle_session_disposition() {
                    LeDtmIdleSessionDisposition::CompleteNoTest(response) => response,
                    LeDtmIdleSessionDisposition::StartReceiver(command) => {
                        command.into_radio_unavailable_command_complete()
                    }
                    LeDtmIdleSessionDisposition::StartTransmitter(command) => {
                        command.into_radio_unavailable_command_complete()
                    }
                };
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedDtm(response) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(command.into_active_session_command_complete()),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingEnable(command) => {
                match command.into_active_session_disposition() {
                    LeLegacyAdvertisingActiveEnableDisposition::Disable(command) => {
                        LeControllerActiveLegacyAdvertisingCommandRoute::Disable(
                            LeControllerDeferredLegacyAdvertisingDisable { ready, command },
                        )
                    }
                    LeLegacyAdvertisingActiveEnableDisposition::Complete(response) => {
                        LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                            ready.begin_next_response(response),
                        )
                    }
                }
            }
            LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyScanningConfiguration(command) => {
                let response = self.dispatch_legacy_scanning_configuration(command);
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyScanningEnable(command) => {
                let response =
                    self.complete_legacy_scanning_enable_while_radio_unavailable(command);
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedLegacyScanning(response) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::Unsupported(response) => {
                LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
        }
    }

    /// Route one command while passive scanning owns the radio lifecycle.
    ///
    /// Scan parameters are immutable from accepted Enable through completed
    /// Disable. Repeated Enable is rejected, while Disable and Reset retain
    /// their exact command-order tokens until scanner quiescence.
    pub fn route_active_legacy_scanning_classified_command<'epoch, 'command, Owner>(
        &mut self,
        command: LeControllerClassifiedCommand<'epoch, 'command, Owner>,
    ) -> LeControllerActiveLegacyScanningCommandRoute<'epoch, 'command, Owner> {
        if !command.ready.accepts_endpoint(self)
            || !command.command.originates_from(self.transport())
        {
            return LeControllerActiveLegacyScanningCommandRoute::EndpointMismatch(command);
        }
        let LeControllerClassifiedCommand { ready, command } = command;
        let classification = command
            .try_into_for_endpoint(self.transport())
            .unwrap_or_else(|_| unreachable!("aggregate affinity was checked above"));

        match classification {
            LeControllerCommandClassification::Bootstrap(command) if command.is_reset() => {
                LeControllerActiveLegacyScanningCommandRoute::ResetBarrier(
                    LeControllerResetBarrier { ready, command },
                )
            }
            LeControllerCommandClassification::Bootstrap(command) => {
                let response = self.dispatch_bootstrap_command_while_radio_active(command);
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedBootstrap(response) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::Dtm(command) => {
                let response = match command.into_idle_session_disposition() {
                    LeDtmIdleSessionDisposition::CompleteNoTest(response) => response,
                    LeDtmIdleSessionDisposition::StartReceiver(command) => {
                        command.into_radio_unavailable_command_complete()
                    }
                    LeDtmIdleSessionDisposition::StartTransmitter(command) => {
                        command.into_radio_unavailable_command_complete()
                    }
                };
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedDtm(response) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
                let response = self.dispatch_legacy_advertising_configuration(command);
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyAdvertisingEnable(command) => {
                let response =
                    self.complete_legacy_advertising_enable_while_radio_unavailable(command);
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::LegacyScanningConfiguration(command) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(command.into_active_session_command_complete()),
                )
            }
            LeControllerCommandClassification::LegacyScanningEnable(command) => {
                match command.into_active_session_disposition() {
                    LeLegacyScanningActiveEnableDisposition::Disable(command) => {
                        LeControllerActiveLegacyScanningCommandRoute::Disable(
                            LeControllerDeferredLegacyScanningDisable { ready, command },
                        )
                    }
                    LeLegacyScanningActiveEnableDisposition::Complete(response) => {
                        LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                            ready.begin_next_response(response),
                        )
                    }
                }
            }
            LeControllerCommandClassification::MalformedLegacyScanning(response) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
            LeControllerCommandClassification::Unsupported(response) => {
                LeControllerActiveLegacyScanningCommandRoute::ResponsePending(
                    ready.begin_next_response(response),
                )
            }
        }
    }

    /// Apply one retained Reset after the outer lifecycle proves quiescence.
    ///
    /// Endpoint affinity is checked before bootstrap state changes. A mismatch
    /// returns the complete barrier unchanged. On success the exact Reset token
    /// is consumed once and only its ordered Command Complete remains pending,
    /// so publication backpressure cannot repeat dispatch.
    pub fn complete_reset_after_quiescence<'epoch, Owner>(
        &mut self,
        barrier: LeControllerResetBarrier<'epoch, Owner>,
    ) -> LeControllerResetCompletion<'epoch, Owner> {
        if !barrier.accepts_endpoint(self) {
            return LeControllerResetCompletion::EndpointMismatch(barrier);
        }

        let LeControllerResetBarrier { ready, command } = barrier;
        let response = self.dispatch_bootstrap_command(command);
        LeControllerResetCompletion::ResponsePending(ready.begin_next_response(response))
    }
}

/// An accepted Reset waiting for the outer lifecycle to quiesce active work.
///
/// The next-command authority and exact Reset token remain private.
/// Constructing this barrier never changes bootstrap state. Typed decomposition
/// may release only the lifecycle owner while a unit barrier retains all HCI
/// completion authority through external quiescence.
#[must_use = "the Reset barrier must remain owned until lifecycle quiescence"]
pub struct LeControllerResetBarrier<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: OwnedBootstrapCommand,
}

impl<'epoch, Owner> LeControllerResetBarrier<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Whether this Reset barrier belongs to a Controller transport epoch.
    pub fn accepts_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.ready.accepts_endpoint(controller)
    }

    /// Transform only the lifecycle owner while retaining Reset and order.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerResetBarrier<'epoch, Next> {
        LeControllerResetBarrier {
            ready: self.ready.map_owner(map),
            command: self.command,
        }
    }

    /// Separate the lifecycle owner from an opaque unit Reset continuation.
    ///
    /// The Reset command and next-command authority remain together in the unit
    /// barrier. A hardware-specific runner retains that barrier while advancing
    /// `Owner`, then reunites its proven quiescent owner through
    /// [`Self::map_owner`] before completion.
    pub fn into_parts(self) -> (Owner, LeControllerResetBarrier<'epoch, ()>) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerResetBarrier {
                ready,
                command: self.command,
            },
        )
    }
}

/// Result of applying one retained Reset through a combined Controller endpoint.
#[must_use = "publish the Reset completion or retain the exact endpoint mismatch"]
pub enum LeControllerResetCompletion<'epoch, Owner> {
    /// Bootstrap Reset was applied exactly once and its ordered response remains pending.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// The endpoint belongs to another HCI epoch; Reset and owner are unchanged.
    EndpointMismatch(LeControllerResetBarrier<'epoch, Owner>),
}

/// Portable result of routing one complete Controller classification.
#[must_use = "publish the response, route the semantic command, or retain the epoch mismatch"]
pub enum LeControllerClassifiedCommandRoute<'epoch, 'command, Owner> {
    /// A terminal classification became an ordered response.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// A valid DTM command remains untouched for session-specific policy.
    Dtm(LeControllerDeferredDtmCommand<'epoch, Owner>),
    /// Reset remains ordered but undispatched until lifecycle quiescence.
    ResetBarrier(LeControllerResetBarrier<'epoch, Owner>),
    /// The aggregate belongs to another endpoint and remains inseparable.
    EndpointMismatch(LeControllerClassifiedCommand<'epoch, 'command, Owner>),
}

/// Result of one consuming Controller response publication attempt.
#[must_use = "retain the unchanged pending owner or the command-ready authority"]
pub enum LeControllerResponsePublication<'epoch, Owner> {
    /// The response entered the matching queue exactly once.
    Published(LeControllerCommandReady<'epoch, Owner>),
    /// The matching queue is full; the complete owner is unchanged.
    Pending(LeControllerResponsePending<'epoch, Owner>),
    /// The supplied endpoint belongs to another live HCI epoch.
    EndpointMismatch(LeControllerResponsePending<'epoch, Owner>),
    /// A non-capacity transport failure retained the complete owner.
    Fault {
        /// Unchanged owner and response authority.
        pending: LeControllerResponsePending<'epoch, Owner>,
        /// Exact validation or transport failure.
        error: HciChannelError,
    },
}

#[cfg(test)]
mod tests;
