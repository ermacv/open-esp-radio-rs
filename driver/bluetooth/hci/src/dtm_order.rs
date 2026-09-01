//! Portable ordering authority for LE Controller command responses.
//!
//! Radio progress and Controller-to-Host capacity are deliberately independent:
//! a chip-specific runner may transform the retained owner while the exact
//! Command Complete remains pending. The resource-owned initial claim establishes
//! [`LeControllerCommandReady`]; thereafter only successful insertion into the
//! matching HCI epoch restores that authority.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    HciChannelError, HciClassifiedCommandIntake, HciControllerResponse, HciEpochBound,
    HciEpochIdentity, HostToControllerFrame, LeControllerCommandClassification,
    LeControllerCommandComplete, LeControllerCommandEndpoint, LeDtmActiveSessionDisposition,
    LeDtmCommand, LeDtmIdleSessionDisposition, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingEnableRequest, LeLegacyAdvertisingIdleEnableDisposition,
    LeReceiverTestCommand, LeTestEndCommand, LeTransmitterTestCommand, OwnedBootstrapCommand,
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

/// One endpoint-validated advertising Enable retaining response order.
#[must_use = "retain the deferred advertising start until hardware starts or rejects it"]
pub struct LeControllerDeferredLegacyAdvertisingStart<'epoch, Owner> {
    ready: LeControllerCommandReady<'epoch, Owner>,
    command: LeLegacyAdvertisingEnableCommand,
    request: LeLegacyAdvertisingEnableRequest,
}

impl<'epoch, Owner> LeControllerDeferredLegacyAdvertisingStart<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.ready.owner()
    }

    /// Immutable configuration snapshot accepted at the Enable boundary.
    pub const fn request(&self) -> LeLegacyAdvertisingEnableRequest {
        self.request
    }

    /// Transform only the independently progressing hardware owner.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerDeferredLegacyAdvertisingStart<'epoch, Next> {
        LeControllerDeferredLegacyAdvertisingStart {
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
        LeControllerDeferredLegacyAdvertisingStart<'epoch, ()>,
    ) {
        let (owner, ready) = self.ready.into_parts();
        (
            owner,
            LeControllerDeferredLegacyAdvertisingStart {
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
    /// Advertising Enable retains one immutable configuration and response order.
    StartLegacyAdvertising(LeControllerDeferredLegacyAdvertisingStart<'epoch, Owner>),
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
                    LeLegacyAdvertisingIdleEnableDisposition::Start(request) => {
                        LeControllerIdleClassifiedCommandRoute::StartLegacyAdvertising(
                            LeControllerDeferredLegacyAdvertisingStart {
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
                LeControllerCommandClassification::Unsupported(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        ready.begin_next_response(response),
                    )
                }
            },
            Err(_) => unreachable!("aggregate affinity was checked above"),
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
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes, PacketKind,
        cmd::{
            Cmd, Opcode, OpcodeGroup,
            controller_baseband::{Reset, SetEventMask},
            le::{
                LeReceiverTest, LeReceiverTestV2, LeSetAdvEnable, LeSetAdvParams, LeTestEnd,
                LeTransmitterTestV2,
            },
        },
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{Error as HciError, Status},
        transport::{PacketToController, Transport},
    };
    use embassy_futures::{
        block_on,
        select::{Either, select},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{
        LeControllerActiveDtmCommandRoute, LeControllerClassifiedCommand,
        LeControllerCommandIntake, LeControllerCommandReady,
        LeControllerIdleClassifiedCommandRoute, LeControllerResetCompletion,
        LeControllerResponsePending, LeControllerResponsePublication,
    };
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, LE_RECEIVER_TEST_V1_OPCODE,
        LE_RECEIVER_TEST_V2_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE, LeControllerBootstrapConfig,
        LeControllerClassifiedCommandRoute, LeControllerCommandEndpoint,
        LeControllerCommandReadyClaim, LeControllerHciEndpoints, LeControllerHciResources,
        LeDtmModulationIndex, LeDtmPhy,
    };

    #[derive(Debug, Eq, PartialEq)]
    struct RadioOwner(u32);

    #[derive(Debug, Eq, PartialEq)]
    struct QuiescedOwner(u32);

    type ControllerResources = LeControllerHciResources<NoopRawMutex, 1, 1, 16>;

    fn controller_resources() -> ControllerResources {
        controller_resources_with_output_depth()
    }

    fn controller_resources_with_output_depth<const CONTROLLER_TO_HOST_DEPTH: usize>()
    -> LeControllerHciResources<NoopRawMutex, 1, CONTROLLER_TO_HOST_DEPTH, 16> {
        LeControllerHciResources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .expect("the test HCI profile is nonzero"),
        )
        .expect("the profile fits its source-owned storage")
    }

    fn claim_initial_ready<
        'epoch,
        Owner,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        controller: &mut LeControllerCommandEndpoint<
            'epoch,
            NoopRawMutex,
            1,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        owner: Owner,
    ) -> LeControllerCommandReady<'epoch, Owner> {
        let LeControllerCommandReadyClaim::Ready(ready) =
            controller.claim_initial_command_ready(owner)
        else {
            panic!("the test epoch exposes its sole initial command authority");
        };
        ready
    }

    fn intake_command<
        'epoch,
        'resources,
        Owner,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        controller: &LeControllerCommandEndpoint<
            'resources,
            NoopRawMutex,
            1,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        ready: LeControllerCommandReady<'epoch, Owner>,
        buffer: &mut [u8],
    ) -> LeControllerClassifiedCommand<'epoch, 'resources, Owner> {
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, .. } => command,
            LeControllerCommandIntake::Empty { .. } => panic!("the queued command disappeared"),
            LeControllerCommandIntake::EndpointMismatch { .. } => {
                panic!("the command-ready authority belongs to another endpoint")
            }
            LeControllerCommandIntake::Channel { error, .. } => {
                panic!("command intake failed: {error:?}")
            }
            LeControllerCommandIntake::NonCommand { .. } => {
                panic!("the queued command changed packet kind")
            }
        }
    }

    struct RawCommand<'parameters> {
        opcode: Opcode,
        parameters: &'parameters [u8],
    }

    impl<'parameters> RawCommand<'parameters> {
        fn new(opcode: Opcode, parameters: &'parameters [u8]) -> Self {
            assert!(parameters.len() <= usize::from(u8::MAX));
            Self { opcode, parameters }
        }

        fn header(&self) -> [u8; 3] {
            let opcode = self.opcode.to_raw().to_le_bytes();
            [opcode[0], opcode[1], self.parameters.len() as u8]
        }
    }

    impl PacketToController for RawCommand<'_> {
        const KIND: PacketKind = PacketKind::Cmd;

        fn size(&self) -> usize {
            3 + self.parameters.len()
        }

        fn write_hci<W: embedded_io::Write>(&self, mut writer: W) -> Result<(), W::Error> {
            embedded_io::Write::write_all(&mut writer, &self.header())?;
            embedded_io::Write::write_all(&mut writer, self.parameters)
        }

        async fn write_hci_async<W: embedded_io_async::Write>(
            &self,
            mut writer: W,
        ) -> Result<(), W::Error> {
            embedded_io_async::Write::write_all(&mut writer, &self.header()).await?;
            embedded_io_async::Write::write_all(&mut writer, self.parameters).await
        }
    }

    fn receiver_start_pending<'epoch, Owner, const CONTROLLER_TO_HOST_DEPTH: usize>(
        endpoints: &mut LeControllerHciEndpoints<
            'epoch,
            NoopRawMutex,
            1,
            CONTROLLER_TO_HOST_DEPTH,
            16,
        >,
        owner: Owner,
    ) -> LeControllerResponsePending<'epoch, Owner> {
        block_on(endpoints.host.write(&LeReceiverTest::new(7)))
            .expect("the receiver command enters the real Host queue");
        let mut command_buffer = [0; 16];
        let ready = claim_initial_ready(&mut endpoints.controller, owner);
        let classified = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartReceiver(start) = endpoints
            .controller
            .route_idle_classified_command(classified)
        else {
            panic!("the receiver command becomes one deferred start");
        };
        start.into_started_response()
    }

    fn publish_probe_response<'epoch, Owner, const CONTROLLER_TO_HOST_DEPTH: usize>(
        endpoints: &mut LeControllerHciEndpoints<
            'epoch,
            NoopRawMutex,
            1,
            CONTROLLER_TO_HOST_DEPTH,
            16,
        >,
        owner: Owner,
    ) -> LeControllerCommandReady<'epoch, Owner> {
        let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 0x1f);
        block_on(endpoints.host.write(&RawCommand::new(opcode, &[])))
            .expect("the probe command enters the Host queue");
        let ready = claim_initial_ready(&mut endpoints.controller, owner);
        let mut command_buffer = [0; 16];
        let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("the unsupported probe becomes an ordered response");
        };
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the empty response queue accepts the probe");
        };
        ready
    }

    fn assert_probe_response(packet: ControllerToHostPacket<'_>) {
        assert_command_status(
            packet,
            Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 0x1f),
            HciError::UNKNOWN_CMD.to_status(),
        );
    }

    #[test]
    fn full_queue_retains_the_transformed_radio_until_exact_publication() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let ready = publish_probe_response(&mut endpoints, 11_u8);
        block_on(endpoints.host.write(&LeReceiverTest::new(7)))
            .expect("the receiver command enters the Host queue");
        let mut command_buffer = [0; 16];
        let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartReceiver(start) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("the receiver command becomes one deferred start");
        };
        let pending = start
            .into_started_response()
            .map_owner(|radio| u16::from(radio) + 20);

        let cancelled = block_on(select(
            async {},
            endpoints.controller.wait_response_capacity(&pending),
        ));
        assert!(matches!(cancelled, Either::First(())));

        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("a full matching queue must retain the pending owner");
        };
        assert_eq!(*pending.owner(), 31);

        let mut buffer = [0; 16];
        assert_probe_response(
            block_on(endpoints.host.read(&mut buffer)).expect("the Host drains the older response"),
        );
        block_on(endpoints.controller.wait_response_capacity(&pending))
            .expect("capacity wait accepts the retained matching response");

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the retained response must publish after capacity returns");
        };
        assert_eq!(*published.owner(), 31);
        assert!(published.accepts_endpoint(&endpoints.controller));
        assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
    }

    #[test]
    fn combined_intake_wait_and_retry_preserve_authority_and_buffer() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(19));

        let cancelled = block_on(select(
            async {},
            endpoints.controller.wait_command_available(&ready),
        ));
        assert!(matches!(cancelled, Either::First(())));

        let mut buffer = [0; 16];
        let LeControllerCommandIntake::Empty {
            ready,
            buffer: returned,
        } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(ready, &mut buffer)
        else {
            panic!("an empty queue returns exact authority and scratch storage");
        };
        assert_eq!(returned.len(), 16);

        block_on(endpoints.host.write(&LeTestEnd::new()))
            .expect("Test End enters the Host queue after the cancelled wait");
        block_on(endpoints.controller.wait_command_available(&ready))
            .expect("the matching authority observes the queued command");
        let mut short = [0; 15];
        let LeControllerCommandIntake::Channel {
            ready,
            buffer: returned,
            error:
                HciChannelError::DestinationTooSmall {
                    required: 16,
                    available: 15,
                },
        } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(ready, &mut short)
        else {
            panic!("a short buffer retains command authority, storage and queued packet");
        };
        assert_eq!(returned.len(), 15);

        let command = intake_command(&endpoints.controller, ready, &mut buffer);
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("the retained packet remains available to a corrected retry");
        };
        assert_eq!(pending.owner(), &RadioOwner(19));
    }

    #[test]
    fn wrong_endpoint_retains_both_axes_and_command_ready_affinity() {
        let mut first_resources = controller_resources();
        let mut first = first_resources.split();
        let mut second_resources = controller_resources();
        let second = second_resources.split();
        let pending = receiver_start_pending(&mut first, 37_u8);

        let LeControllerResponsePublication::EndpointMismatch(pending) =
            pending.try_publish(&second.controller)
        else {
            panic!("a foreign endpoint must retain the complete pending owner");
        };
        assert_eq!(*pending.owner(), 37);

        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&first.controller)
        else {
            panic!("the original endpoint must accept the retained response");
        };
        assert!(ready.accepts_endpoint(&first.controller));
        assert!(!ready.accepts_endpoint(&second.controller));
    }

    #[test]
    fn successful_publication_is_exact_once_and_preserves_existing_fifo_order() {
        let mut resources = controller_resources_with_output_depth::<2>();
        let mut endpoints = resources.split();
        let ready = publish_probe_response(&mut endpoints, 43_u8);
        block_on(endpoints.host.write(&LeReceiverTest::new(7)))
            .expect("the receiver command enters the Host queue");
        let mut command_buffer = [0; 16];
        let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartReceiver(start) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("the receiver command becomes one deferred start");
        };
        let pending = start.into_started_response();

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the second FIFO slot must accept the start response");
        };
        assert_eq!(*published.owner(), 43);

        let mut buffer = [0; 16];
        assert_probe_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
        assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());

        let published: LeControllerCommandReady<'_, u16> =
            published.map_owner(|radio| u16::from(radio) + 1);
        assert_eq!(*published.owner(), 44);
        assert!(published.accepts_endpoint(&endpoints.controller));
    }

    #[test]
    fn published_response_orders_the_next_dtm_completion() {
        let mut resources = controller_resources_with_output_depth::<2>();
        let mut endpoints = resources.split();
        let start = receiver_start_pending(&mut endpoints, 47_u8);
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue must accept the start response");
        };
        block_on(endpoints.host.write(&LeTestEnd::new()))
            .expect("Test End enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = intake_command(&endpoints.controller, started, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::Dtm(deferred) =
            endpoints.controller.route_classified_command(classified)
        else {
            panic!("Test End remains in the portable DTM order aggregate");
        };
        let LeControllerActiveDtmCommandRoute::TestEnd(ending) =
            deferred.into_active_session_route()
        else {
            panic!("active Test End remains deferred until its packet count exists");
        };
        let ending = ending.into_ended_response(0x3412);
        let LeControllerResponsePublication::Published(ended) =
            ending.try_publish(&endpoints.controller)
        else {
            panic!("the second slot must accept Test End after the start response");
        };
        assert_eq!(ended.owner(), &47);

        let mut buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
        assert_test_end_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
    }

    #[test]
    fn idle_router_defers_both_start_kinds_until_explicit_started_response() {
        let mut receiver_resources = controller_resources();
        let mut receiver = receiver_resources.split();
        block_on(receiver.host.write(&LeReceiverTestV2::new(13, 2, 1)))
            .expect("the receiver command enters the real Host queue");
        let mut receiver_buffer = [0; 16];
        let ready = claim_initial_ready(&mut receiver.controller, RadioOwner(71));
        let classified = intake_command(&receiver.controller, ready, &mut receiver_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartReceiver(start) = receiver
            .controller
            .route_idle_classified_command(classified)
        else {
            panic!("idle receiver start becomes one deferred transaction");
        };
        assert_eq!(start.owner(), &RadioOwner(71));
        assert_eq!(start.command().channel().index(), 13);
        assert_eq!(start.command().phy(), LeDtmPhy::Le2M);
        assert_eq!(
            start.command().modulation_index(),
            LeDtmModulationIndex::Stable
        );
        let (owner, continuation) = start.into_parts();
        assert_eq!(owner, RadioOwner(71));
        let pending = continuation
            .map_owner(|()| RadioOwner(72))
            .into_started_response();
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&receiver.controller)
        else {
            panic!("explicit receiver start completion publishes once");
        };
        assert_eq!(published.owner(), &RadioOwner(72));
        let mut response_buffer = [0; 16];
        assert_command_status(
            block_on(receiver.host.read(&mut response_buffer)).unwrap(),
            LE_RECEIVER_TEST_V2_OPCODE,
            Status::SUCCESS,
        );

        let mut transmitter_resources = controller_resources();
        let mut transmitter = transmitter_resources.split();
        block_on(
            transmitter
                .host
                .write(&LeTransmitterTestV2::new(17, 23, 2, 4)),
        )
        .expect("the transmitter command enters the real Host queue");
        let mut transmitter_buffer = [0; 16];
        let ready = claim_initial_ready(&mut transmitter.controller, RadioOwner(73));
        let classified = intake_command(&transmitter.controller, ready, &mut transmitter_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartTransmitter(start) = transmitter
            .controller
            .route_idle_classified_command(classified)
        else {
            panic!("idle transmitter start becomes one deferred transaction");
        };
        assert_eq!(start.owner(), &RadioOwner(73));
        assert_eq!(start.command().channel().index(), 17);
        assert_eq!(start.command().payload_length(), 23);
        assert_eq!(start.command().payload_pattern().hci_parameter(), 2);
        assert_eq!(start.command().phy(), LeDtmPhy::LeCodedS2);
        let pending = start
            .map_owner(|RadioOwner(owner)| RadioOwner(owner + 1))
            .into_started_response();
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&transmitter.controller)
        else {
            panic!("explicit transmitter start completion publishes once");
        };
        assert_eq!(published.owner(), &RadioOwner(74));
        assert_command_status(
            block_on(transmitter.host.read(&mut response_buffer)).unwrap(),
            LE_TRANSMITTER_TEST_V2_OPCODE,
            Status::SUCCESS,
        );
    }

    #[test]
    fn hardware_failure_status_preserves_backpressure_and_order() {
        let mut receiver_resources = controller_resources();
        let mut receiver = receiver_resources.split();
        let ready = publish_probe_response(&mut receiver, RadioOwner(91));
        block_on(receiver.host.write(&LeReceiverTestV2::new(13, 3, 0))).unwrap();
        let mut command_buffer = [0; 16];
        let command = intake_command(&receiver.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartReceiver(start) =
            receiver.controller.route_idle_classified_command(command)
        else {
            panic!("receiver start must remain deferred");
        };
        let pending = start
            .map_owner(|RadioOwner(owner)| RadioOwner(owner + 1))
            .into_hardware_failure_response();
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&receiver.controller)
        else {
            panic!("the older response must backpressure the portable failure");
        };
        let mut response_buffer = [0; 16];
        assert_probe_response(block_on(receiver.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&receiver.controller)
        else {
            panic!("the retained receiver failure publishes after capacity returns");
        };
        assert_eq!(ready.owner(), &RadioOwner(92));
        assert_command_status(
            block_on(receiver.host.read(&mut response_buffer)).unwrap(),
            LE_RECEIVER_TEST_V2_OPCODE,
            HciError::HARDWARE_FAILURE.to_status(),
        );
    }

    #[test]
    fn idle_router_retains_zero_count_test_end_through_backpressure() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let ready = publish_probe_response(&mut endpoints, RadioOwner(67));
        block_on(endpoints.host.write(&LeTestEnd::new())).expect("Test End enters the Host queue");
        let mut command_buffer = [0; 16];
        let classified = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_idle_classified_command(classified)
        else {
            panic!("idle Test End becomes a zero-count response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the full queue retains response and idle owner");
        };
        assert_eq!(pending.owner(), &RadioOwner(67));

        let mut response_buffer = [0; 16];
        assert_probe_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Test End publishes after capacity returns");
        };
        assert_eq!(published.owner(), &RadioOwner(67));
        assert_test_end_packet_count(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            0,
        );
    }

    #[test]
    fn idle_router_barriers_reset_and_dispatches_non_reset_exactly_once() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let ready = publish_probe_response(&mut endpoints, RadioOwner(81));
        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the Host queue");
        let mut command_buffer = [0; 16];
        let reset = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
            endpoints.controller.route_idle_classified_command(reset)
        else {
            panic!("idle Reset becomes an opaque lifecycle barrier");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        let LeControllerResetCompletion::ResponsePending(pending) = endpoints
            .controller
            .complete_reset_after_quiescence(barrier)
        else {
            panic!("the matching endpoint completes Reset after external quiescence");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Reset completion is backpressured");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );
        let mut response_buffer = [0; 16];
        assert_probe_response(
            block_on(endpoints.host.read(&mut response_buffer))
                .expect("the older response is drained"),
        );
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Reset completion publishes once");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            Reset::OPCODE,
            Status::SUCCESS,
        );

        let requested_mask = bt_hci::param::EventMask::new().enable_hardware_error(true);
        block_on(endpoints.host.write(&SetEventMask::new(requested_mask)))
            .expect("Set Event Mask enters the Host queue");
        let command = intake_command(&endpoints.controller, published, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("non-Reset bootstrap dispatches into one response");
        };
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Set Event Mask completion publishes");
        };
        assert_eq!(published.owner(), &RadioOwner(81));
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            SetEventMask::OPCODE,
            Status::SUCCESS,
        );
    }

    #[test]
    fn idle_advertising_enable_retains_snapshot_and_order_until_started() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 40>::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .expect("the test HCI profile is nonzero"),
        )
        .expect("the advertising commands fit the transport");
        let mut endpoints = resources.split();
        let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(101));
        let mut command_buffer = [0; 40];
        let mut response_buffer = [0; 40];

        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the Host queue");
        let reset = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
            endpoints.controller.route_idle_classified_command(reset)
        else {
            panic!("Reset must preserve lifecycle order");
        };
        let LeControllerResetCompletion::ResponsePending(pending) = endpoints
            .controller
            .complete_reset_after_quiescence(barrier)
        else {
            panic!("the matching endpoint completes Reset");
        };
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the empty response queue accepts Reset");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            Reset::OPCODE,
            Status::SUCCESS,
        );

        let parameters = [
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
        ];
        block_on(
            endpoints
                .host
                .write(&RawCommand::new(LeSetAdvParams::OPCODE, &parameters)),
        )
        .expect("Set Advertising Parameters enters the Host queue");
        let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("accepted parameters complete in software");
        };
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the empty response queue accepts parameters");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            LeSetAdvParams::OPCODE,
            Status::SUCCESS,
        );

        block_on(
            endpoints
                .host
                .write(&RawCommand::new(LeSetAdvEnable::OPCODE, &[1])),
        )
        .expect("Enable enters the Host queue");
        let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::StartLegacyAdvertising(start) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("Enable must remain deferred until hardware starts");
        };
        assert_eq!(start.owner(), &RadioOwner(101));
        assert_eq!(
            start.request().advertiser().wire_bytes(),
            [13, 11, 7, 5, 3, 2]
        );
        assert!(start.request().data().is_empty());
        assert_eq!(
            start
                .request()
                .parameters()
                .interval()
                .minimum_units_625_us(),
            0x20
        );
        assert!(start.request().parameters().channels().channel_37());
        assert!(!start.request().parameters().channels().channel_38());
        assert!(start.request().parameters().channels().channel_39());

        let (owner, continuation) = start.into_parts();
        assert_eq!(owner, RadioOwner(101));
        let pending = continuation
            .map_owner(|()| RadioOwner(102))
            .into_started_response();
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("started Enable response publishes exactly once");
        };
        assert_eq!(ready.owner(), &RadioOwner(102));
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            LeSetAdvEnable::OPCODE,
            Status::SUCCESS,
        );
    }

    #[test]
    fn idle_router_orders_malformed_and_unsupported_classifications() {
        let unsupported_opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
        for (owner, opcode, parameters, expected) in [
            (
                91,
                SetEventMask::OPCODE,
                &[0; 7][..],
                HciError::INVALID_HCI_PARAMETERS.to_status(),
            ),
            (
                92,
                LE_RECEIVER_TEST_V1_OPCODE,
                &[][..],
                HciError::INVALID_HCI_PARAMETERS.to_status(),
            ),
            (
                93,
                unsupported_opcode,
                &[][..],
                HciError::UNKNOWN_CMD.to_status(),
            ),
        ] {
            let mut resources = controller_resources();
            let mut endpoints = resources.split();
            let ready = publish_probe_response(&mut endpoints, RadioOwner(owner));
            block_on(endpoints.host.write(&RawCommand::new(opcode, parameters)))
                .expect("the command enters the real Host queue");
            let mut command_buffer = [0; 16];
            let classified = intake_command(&endpoints.controller, ready, &mut command_buffer);
            let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) = endpoints
                .controller
                .route_idle_classified_command(classified)
            else {
                panic!("terminal idle classification becomes a response");
            };
            assert_eq!(
                endpoints.controller.bootstrap_phase(),
                BootstrapPhase::AwaitingReset
            );
            let LeControllerResponsePublication::Pending(pending) =
                pending.try_publish(&endpoints.controller)
            else {
                panic!("the full queue retains the exact terminal response");
            };
            assert_eq!(pending.owner(), &RadioOwner(owner));
            let mut response_buffer = [0; 16];
            assert_probe_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
            let LeControllerResponsePublication::Published(published) =
                pending.try_publish(&endpoints.controller)
            else {
                panic!("the terminal response publishes after capacity returns");
            };
            assert_eq!(published.owner(), &RadioOwner(owner));
            assert_command_status(
                block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
                opcode,
                expected,
            );
        }
    }

    #[test]
    fn idle_router_cross_epoch_mismatch_retains_owner_order_and_full_classification() {
        let mut first_resources = controller_resources();
        let mut first = first_resources.split();
        let mut second_resources = controller_resources();
        let mut second = second_resources.split();
        block_on(second.host.write(&Reset::new())).expect("foreign Reset enters its Host queue");
        let mut command_buffer = [0; 16];
        let second_ready = claim_initial_ready(&mut second.controller, RadioOwner(97));
        let command = intake_command(&second.controller, second_ready, &mut command_buffer);
        let LeControllerIdleClassifiedCommandRoute::EndpointMismatch(command) =
            first.controller.route_idle_classified_command(command)
        else {
            panic!("foreign aggregate must remain inseparable and unchanged");
        };
        assert_eq!(command.owner(), &RadioOwner(97));
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        assert_eq!(
            second.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(_) =
            second.controller.route_idle_classified_command(command)
        else {
            panic!("the source endpoint must still route the retained Reset aggregate");
        };

        block_on(first.host.write(&Reset::new())).expect("the first Reset enters its Host queue");
        let first_ready = claim_initial_ready(&mut first.controller, RadioOwner(101));
        let LeControllerCommandIntake::EndpointMismatch {
            ready: first_ready,
            buffer,
        } = second
            .controller
            .try_receive_classified_command_with_buffer(first_ready, &mut command_buffer)
        else {
            panic!("foreign authority must fail before consuming any command");
        };
        assert_eq!(first_ready.owner(), &RadioOwner(101));
        let command = intake_command(&first.controller, first_ready, buffer);
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(_) =
            first.controller.route_idle_classified_command(command)
        else {
            panic!("mismatched intake must leave the source command available");
        };
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        assert_eq!(
            second.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
    }

    #[test]
    fn classified_router_rejects_both_active_start_kinds_through_owned_order() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = receiver_start_pending(&mut endpoints, RadioOwner(53));
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue must accept the start response");
        };

        block_on(endpoints.host.write(&LeReceiverTestV2::new(11, 2, 0)))
            .expect("the receiver command enters the real Host queue");
        let mut command_buffer = [0; 16];
        let receiver = intake_command(&endpoints.controller, started, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::Dtm(deferred) =
            endpoints.controller.route_classified_command(receiver)
        else {
            panic!("the combined router must hand the receiver command to session policy");
        };
        assert_eq!(deferred.owner(), &RadioOwner(53));
        let LeControllerActiveDtmCommandRoute::ResponsePending(pending) =
            deferred.into_active_session_route()
        else {
            panic!("a second receiver start must become Controller Busy");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued start response must backpressure Controller Busy");
        };
        let mut buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Controller Busy publishes after capacity returns");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut buffer)).unwrap(),
            LE_RECEIVER_TEST_V2_OPCODE,
            HciError::CONTROLLER_BUSY.to_status(),
        );

        block_on(
            endpoints
                .host
                .write(&LeTransmitterTestV2::new(17, 23, 2, 3)),
        )
        .expect("the transmitter command enters the real Host queue");
        let transmitter = intake_command(&endpoints.controller, ready, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::Dtm(deferred) =
            endpoints.controller.route_classified_command(transmitter)
        else {
            panic!("the combined router must hand the transmitter command to session policy");
        };
        let LeControllerActiveDtmCommandRoute::ResponsePending(pending) =
            deferred.into_active_session_route()
        else {
            panic!("a second transmitter start must become Controller Busy");
        };
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue accepts transmitter Controller Busy");
        };
        assert_eq!(ready.owner(), &RadioOwner(53));
        assert_command_status(
            block_on(endpoints.host.read(&mut buffer)).unwrap(),
            LE_TRANSMITTER_TEST_V2_OPCODE,
            HciError::CONTROLLER_BUSY.to_status(),
        );
    }

    #[test]
    fn classified_router_hands_test_end_to_session_policy_with_published_order() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = receiver_start_pending(&mut endpoints, RadioOwner(59));
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue must accept the start response");
        };
        block_on(endpoints.host.write(&LeTestEnd::new()))
            .expect("Test End enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = intake_command(&endpoints.controller, started, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::Dtm(deferred) =
            endpoints.controller.route_classified_command(classified)
        else {
            panic!("Test End must remain semantic for the caller's session policy");
        };
        let LeControllerActiveDtmCommandRoute::TestEnd(ending) =
            deferred.into_active_session_route()
        else {
            panic!("Test End must remain deferred until quiescence");
        };
        assert_eq!(ending.owner(), &RadioOwner(59));
        let pending = ending.into_ended_response(0x1234);
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued start response must backpressure Test End");
        };
        let mut response_buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(ready) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Test End publishes after capacity returns");
        };
        assert_eq!(ready.owner(), &RadioOwner(59));
        assert_test_end_packet_count(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            0x1234,
        );
    }

    #[test]
    fn reset_completion_is_exact_once_and_retained_through_backpressure() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = receiver_start_pending(&mut endpoints, RadioOwner(60));
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue must accept the start response");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = intake_command(&endpoints.controller, started, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) =
            endpoints.controller.route_classified_command(classified)
        else {
            panic!("Reset must become an opaque lifecycle barrier");
        };
        let (active, continuation) = barrier.into_parts();
        assert_eq!(active, RadioOwner(60));
        assert_eq!(continuation.owner(), &());
        assert!(continuation.accepts_endpoint(&endpoints.controller));
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        let barrier = continuation.map_owner(|()| QuiescedOwner(61));
        let LeControllerResetCompletion::ResponsePending(pending) = endpoints
            .controller
            .complete_reset_after_quiescence(barrier)
        else {
            panic!("the matching endpoint must apply the quiesced Reset");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(61));
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the probe event must backpressure the exact Reset completion");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(61));

        let mut response_buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the retained Reset completion must publish after capacity returns");
        };
        assert_eq!(published.owner(), &QuiescedOwner(61));
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            Reset::OPCODE,
            Status::SUCCESS,
        );
    }

    #[test]
    fn reset_completion_cross_epoch_rejection_retains_barrier_without_mutation() {
        let mut first_resources = controller_resources();
        let mut first = first_resources.split();
        let start = receiver_start_pending(&mut first, RadioOwner(71));
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&first.controller)
        else {
            panic!("the first endpoint must publish its start response");
        };
        block_on(first.host.write(&Reset::new())).expect("Reset enters the first Host transport");
        let mut command_buffer = [0; 16];
        let classified = intake_command(&first.controller, started, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) =
            first.controller.route_classified_command(classified)
        else {
            panic!("Reset must become a lifecycle barrier");
        };
        let barrier = barrier.map_owner(|RadioOwner(owner)| QuiescedOwner(owner + 1));

        let mut second_resources = controller_resources();
        let mut second = second_resources.split();
        let LeControllerResetCompletion::EndpointMismatch(barrier) =
            second.controller.complete_reset_after_quiescence(barrier)
        else {
            panic!("the foreign endpoint must retain the exact Reset barrier");
        };
        assert_eq!(barrier.owner(), &QuiescedOwner(72));
        assert!(barrier.accepts_endpoint(&first.controller));
        assert!(!barrier.accepts_endpoint(&second.controller));
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        assert_eq!(
            second.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        let LeControllerResetCompletion::ResponsePending(pending) =
            first.controller.complete_reset_after_quiescence(barrier)
        else {
            panic!("the original endpoint must apply the retained Reset");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(72));
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );
        assert_eq!(
            second.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&first.controller)
        else {
            panic!("the queued start response must retain Reset completion");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(72));

        let mut response_buffer = [0; 16];
        assert_start_response(block_on(first.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&first.controller)
        else {
            panic!("the original endpoint must publish after capacity returns");
        };
        assert_eq!(published.owner(), &QuiescedOwner(72));
        assert_command_status(
            block_on(first.host.read(&mut response_buffer)).unwrap(),
            Reset::OPCODE,
            Status::SUCCESS,
        );
    }

    #[test]
    fn classified_router_orders_malformed_and_unsupported_responses_through_backpressure() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = receiver_start_pending(&mut endpoints, RadioOwner(62));
        let LeControllerResponsePublication::Published(active) =
            start.try_publish(&endpoints.controller)
        else {
            panic!("the empty queue must accept the start response");
        };
        let mut command_buffer = [0; 16];
        let mut response_buffer = [0; 16];

        block_on(
            endpoints
                .host
                .write(&RawCommand::new(SetEventMask::OPCODE, &[0; 7])),
        )
        .expect("the malformed bootstrap command enters the real Host queue");
        let malformed_bootstrap =
            intake_command(&endpoints.controller, active, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_classified_command(malformed_bootstrap)
        else {
            panic!("malformed bootstrap must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued start response must backpressure malformed bootstrap");
        };
        assert_start_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(active) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("malformed bootstrap must publish after capacity returns");
        };

        block_on(
            endpoints
                .host
                .write(&RawCommand::new(LE_RECEIVER_TEST_V1_OPCODE, &[])),
        )
        .expect("the malformed DTM command enters the real Host queue");
        let malformed_dtm = intake_command(&endpoints.controller, active, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_classified_command(malformed_dtm)
        else {
            panic!("malformed DTM must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued bootstrap error must backpressure malformed DTM");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            SetEventMask::OPCODE,
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        );
        let LeControllerResponsePublication::Published(active) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("malformed DTM must publish after capacity returns");
        };

        let unsupported_opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
        block_on(
            endpoints
                .host
                .write(&RawCommand::new(unsupported_opcode, &[])),
        )
        .expect("the unsupported command enters the real Host queue");
        let unsupported = intake_command(&endpoints.controller, active, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_classified_command(unsupported)
        else {
            panic!("unsupported command must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued DTM error must backpressure Unknown Command");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            LE_RECEIVER_TEST_V1_OPCODE,
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        );
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("Unknown Command must publish after capacity returns");
        };
        assert_eq!(published.owner(), &RadioOwner(62));
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            unsupported_opcode,
            HciError::UNKNOWN_CMD.to_status(),
        );
    }

    #[test]
    fn classified_router_cross_epoch_rejection_retains_both_exact_owners() {
        let mut first_resources = controller_resources();
        let mut first = first_resources.split();
        let start = receiver_start_pending(&mut first, RadioOwner(61));
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(&first.controller)
        else {
            panic!("the first endpoint must publish its start response");
        };

        let mut second_resources = controller_resources();
        let mut second = second_resources.split();
        block_on(second.host.write(&LeTestEnd::new()))
            .expect("the foreign DTM command enters its own Host queue");
        let mut command_buffer = [0; 16];
        let second_ready = claim_initial_ready(&mut second.controller, RadioOwner(63));
        let classified = intake_command(&second.controller, second_ready, &mut command_buffer);
        let LeControllerClassifiedCommandRoute::EndpointMismatch(classified) =
            first.controller.route_classified_command(classified)
        else {
            panic!("a foreign aggregate must remain intact");
        };
        assert_eq!(started.owner(), &RadioOwner(61));
        assert_eq!(classified.owner(), &RadioOwner(63));
        let LeControllerClassifiedCommandRoute::Dtm(deferred) =
            second.controller.route_classified_command(classified)
        else {
            panic!("the source endpoint must retain the aggregate's DTM semantics");
        };
        let LeControllerActiveDtmCommandRoute::TestEnd(test_end) =
            deferred.into_active_session_route()
        else {
            panic!("the retained Test End must remain semantic");
        };
        assert_eq!(test_end.owner(), &RadioOwner(63));
    }

    fn assert_start_response(packet: ControllerToHostPacket<'_>) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("DTM start response changed packet kind");
        };
        assert_eq!(event.kind, EventKind::CommandComplete);
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("response is a complete Command Complete");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("response contains standard status");
        assert_eq!(complete.cmd_opcode, LE_RECEIVER_TEST_V1_OPCODE);
        assert_eq!(complete.status, Status::SUCCESS);
    }

    fn assert_command_status(packet: ControllerToHostPacket<'_>, opcode: Opcode, status: Status) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Controller response changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("response is a complete Command Complete");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("response contains standard status");
        assert_eq!(complete.cmd_opcode, opcode);
        assert_eq!(complete.status, status);
    }

    fn assert_test_end_response(packet: ControllerToHostPacket<'_>) {
        assert_test_end_packet_count(packet, 0x3412);
    }

    fn assert_test_end_packet_count(packet: ControllerToHostPacket<'_>, packet_count: u16) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("DTM Test End response changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("response is a complete Command Complete");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("response contains standard status");
        assert_eq!(complete.status, Status::SUCCESS);
        assert_eq!(complete.return_params::<LeTestEnd>().unwrap(), packet_count);
    }
}
