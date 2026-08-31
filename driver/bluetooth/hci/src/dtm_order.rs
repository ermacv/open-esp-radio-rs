//! Portable ordering authority for LE Controller command responses.
//!
//! Radio progress and Controller-to-Host capacity are deliberately independent:
//! a chip-specific runner may transform the retained owner while the exact
//! Command Complete remains pending. Only successful insertion into the matching
//! HCI epoch advances the order state to [`LeControllerResponsePublished`].

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    HciChannelError, HciControllerResponse, HciEpochBound, HciEpochIdentity,
    InProcessHciControllerEndpoint, LeControllerCommandClassification, LeControllerCommandComplete,
    LeControllerCommandEndpoint, LeDtmCommand, LeDtmIdleSessionDisposition,
    LeReceiverTestV1Command, LeTransmitterTestV1Command, OwnedBootstrapCommand,
};

/// Route one endpoint-bound command while no DTM test is active.
///
/// `owner_epoch` binds the caller's idle owner to its live HCI resources. Both
/// that affinity and the command's opaque origin must match `controller` before
/// the semantic command can be released. Idle Test End immediately becomes an
/// ordered zero-count response retaining `Owner` through backpressure.
pub fn route_idle_dtm_command<
    'epoch,
    'command,
    Owner,
    M: RawMutex,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>(
    owner: Owner,
    owner_epoch: HciEpochIdentity<'epoch>,
    controller: &InProcessHciControllerEndpoint<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    command: HciEpochBound<'command, LeDtmCommand>,
) -> LeDtmIdleCommandRoute<'epoch, 'command, Owner> {
    if !owner_epoch.same_epoch(controller.epoch_identity()) {
        return LeDtmIdleCommandRoute::EndpointMismatch { owner, command };
    }
    match command.try_into_for_endpoint(controller) {
        Ok(command) => match command.into_idle_session_disposition() {
            LeDtmIdleSessionDisposition::StartReceiver(command) => {
                LeDtmIdleCommandRoute::StartReceiver { owner, command }
            }
            LeDtmIdleSessionDisposition::StartTransmitter(command) => {
                LeDtmIdleCommandRoute::StartTransmitter { owner, command }
            }
            LeDtmIdleSessionDisposition::CompleteNoTest(response) => {
                LeDtmIdleCommandRoute::ResponsePending(LeControllerResponsePending::new(
                    owner,
                    response,
                    owner_epoch,
                ))
            }
        },
        Err(command) => LeDtmIdleCommandRoute::EndpointMismatch { owner, command },
    }
}

/// Portable result of routing one endpoint-bound command while DTM is idle.
#[must_use = "start hardware, publish idle Test End, or retain the epoch mismatch"]
pub enum LeDtmIdleCommandRoute<'epoch, 'command, Owner> {
    /// Begin one receiver test with the exact idle owner.
    StartReceiver {
        /// Unchanged idle owner whose epoch admitted the command.
        owner: Owner,
        /// Validated receiver command released after both epoch checks.
        command: LeReceiverTestV1Command,
    },
    /// Begin one transmitter test with the exact idle owner.
    StartTransmitter {
        /// Unchanged idle owner whose epoch admitted the command.
        owner: Owner,
        /// Validated transmitter command released after both epoch checks.
        command: LeTransmitterTestV1Command,
    },
    /// Idle Test End became the standard zero-count response.
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    /// Either idle owner or command belongs to another live Controller epoch.
    EndpointMismatch {
        /// Unchanged idle owner.
        owner: Owner,
        /// Unchanged semantic command retaining its source-epoch proof.
        command: HciEpochBound<'command, LeDtmCommand>,
    },
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
    /// Bind one owner to the response and HCI epoch which admitted it.
    pub fn new<Response>(
        owner: Owner,
        response: Response,
        hci_epoch: HciEpochIdentity<'epoch>,
    ) -> Self
    where
        Response: Into<LeControllerCommandComplete>,
    {
        Self {
            owner,
            response: response.into(),
            hci_epoch,
        }
    }

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
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch.same_epoch(controller.epoch_identity())
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
        controller: &InProcessHciControllerEndpoint<
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

        match controller.try_publish(self.response.kind(), self.response.as_bytes()) {
            Ok(()) => LeControllerResponsePublication::Published(LeControllerResponsePublished {
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

/// An owner whose Controller response entered its matching HCI epoch.
///
/// This state contains no response bytes and exposes no publication operation,
/// making a duplicate insertion unrepresentable. The retained epoch marker
/// authorizes later command intake and the next ordered response transaction.
#[must_use = "the owner and its HCI affinity must remain retained"]
pub struct LeControllerResponsePublished<'epoch, Owner> {
    owner: Owner,
    hci_epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Owner> LeControllerResponsePublished<'epoch, Owner> {
    /// Borrow the independently progressing owner axis.
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Release the retained owner after the response was durably published.
    ///
    /// The response authority is absent from this typestate, so consuming the
    /// owner cannot enable duplicate publication. Terminal session transitions
    /// use this edge before restoring their hardware graph.
    pub fn into_owner(self) -> Owner {
        self.owner
    }

    /// Separate the owner from its published unit order proof for typed composition.
    pub fn into_parts(self) -> (Owner, LeControllerResponsePublished<'epoch, ()>) {
        (
            self.owner,
            LeControllerResponsePublished {
                owner: (),
                hci_epoch: self.hci_epoch,
            },
        )
    }

    /// Transform only the owner axis while retaining the published order proof.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerResponsePublished<'epoch, Next> {
        LeControllerResponsePublished {
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
    pub fn begin_next_response<Response>(
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

    /// Whether an endpoint belongs to the HCI epoch which accepted the response.
    ///
    /// Only the published state exposes this authority, so command intake cannot
    /// be enabled while the preceding response is still pending.
    pub fn accepts_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch.same_epoch(controller.epoch_identity())
    }
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    LeControllerCommandEndpoint<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Route one fully classified command through its complete Controller epoch.
    ///
    /// The published response order and command must both belong to this
    /// endpoint. Terminal classifications and non-Reset bootstrap commands
    /// immediately enter the ordered response axis. DTM remains semantic for
    /// radio-session policy, while Reset becomes an opaque barrier without
    /// changing bootstrap state.
    pub fn route_classified_command<'epoch, 'command, Owner>(
        &mut self,
        published: LeControllerResponsePublished<'epoch, Owner>,
        command: HciEpochBound<'command, LeControllerCommandClassification>,
    ) -> LeControllerClassifiedCommandRoute<'epoch, 'command, Owner> {
        if !published.accepts_endpoint(self.transport()) {
            return LeControllerClassifiedCommandRoute::EndpointMismatch { published, command };
        }
        match command.try_into_for_endpoint(self.transport()) {
            Ok(classification) => match classification {
                LeControllerCommandClassification::Bootstrap(command) if command.is_reset() => {
                    LeControllerClassifiedCommandRoute::ResetBarrier(LeControllerResetBarrier {
                        published,
                        command,
                    })
                }
                LeControllerCommandClassification::Bootstrap(command) => {
                    let response = self.dispatch_bootstrap_command(command);
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        published.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::MalformedBootstrap(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        published.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::Dtm(command) => {
                    LeControllerClassifiedCommandRoute::Dtm { published, command }
                }
                LeControllerCommandClassification::MalformedDtm(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        published.begin_next_response(response),
                    )
                }
                LeControllerCommandClassification::Unsupported(response) => {
                    LeControllerClassifiedCommandRoute::ResponsePending(
                        published.begin_next_response(response),
                    )
                }
            },
            Err(command) => {
                LeControllerClassifiedCommandRoute::EndpointMismatch { published, command }
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
        if !barrier.accepts_endpoint(self.transport()) {
            return LeControllerResetCompletion::EndpointMismatch(barrier);
        }

        let LeControllerResetBarrier { published, command } = barrier;
        let response = self.dispatch_bootstrap_command(command);
        LeControllerResetCompletion::ResponsePending(published.begin_next_response(response))
    }
}

/// An accepted Reset waiting for the outer lifecycle to quiesce active work.
///
/// The prior-response order proof and exact Reset token remain private.
/// Constructing this barrier never changes bootstrap state. Typed decomposition
/// may release only the lifecycle owner while a unit barrier retains all HCI
/// completion authority through external quiescence.
#[must_use = "the Reset barrier must remain owned until lifecycle quiescence"]
pub struct LeControllerResetBarrier<'epoch, Owner> {
    published: LeControllerResponsePublished<'epoch, Owner>,
    command: OwnedBootstrapCommand,
}

impl<'epoch, Owner> LeControllerResetBarrier<'epoch, Owner> {
    /// Borrow the independently progressing lifecycle owner.
    pub const fn owner(&self) -> &Owner {
        self.published.owner()
    }

    /// Whether this Reset barrier belongs to a Controller transport epoch.
    pub fn accepts_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.published.accepts_endpoint(controller)
    }

    /// Transform only the lifecycle owner while retaining Reset and order.
    pub fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LeControllerResetBarrier<'epoch, Next> {
        LeControllerResetBarrier {
            published: self.published.map_owner(map),
            command: self.command,
        }
    }

    /// Separate the lifecycle owner from an opaque unit Reset continuation.
    ///
    /// The Reset command and published order proof remain together in the unit
    /// barrier. A hardware-specific runner retains that barrier while advancing
    /// `Owner`, then reunites its proven quiescent owner through
    /// [`Self::map_owner`] before completion.
    pub fn into_parts(self) -> (Owner, LeControllerResetBarrier<'epoch, ()>) {
        let (owner, published) = self.published.into_parts();
        (
            owner,
            LeControllerResetBarrier {
                published,
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
    Dtm {
        /// Published prior-response owner retaining the caller's session state.
        published: LeControllerResponsePublished<'epoch, Owner>,
        /// Complete semantic DTM command released only after epoch validation.
        command: LeDtmCommand,
    },
    /// Reset remains ordered but undispatched until lifecycle quiescence.
    ResetBarrier(LeControllerResetBarrier<'epoch, Owner>),
    /// Either order or command belongs to another live Controller epoch.
    EndpointMismatch {
        /// Unchanged published owner.
        published: LeControllerResponsePublished<'epoch, Owner>,
        /// Original complete classification retaining its source-epoch proof.
        command: HciEpochBound<'command, LeControllerCommandClassification>,
    },
}

/// Result of one consuming Controller response publication attempt.
#[must_use = "retain the unchanged pending owner or the published order proof"]
pub enum LeControllerResponsePublication<'epoch, Owner> {
    /// The response entered the matching queue exactly once.
    Published(LeControllerResponsePublished<'epoch, Owner>),
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
        ControllerToHostPacket, FromHciBytes, HostToControllerPacket, PacketKind, WriteHci,
        cmd::{
            Cmd, Opcode, OpcodeGroup,
            controller_baseband::{Reset, SetEventMask},
            le::LeTestEnd,
        },
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{Error as HciError, Status},
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{
        LeControllerResetCompletion, LeControllerResponsePending, LeControllerResponsePublication,
        LeControllerResponsePublished, LeDtmIdleCommandRoute, route_idle_dtm_command,
    };
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, InProcessHciChannel,
        LE_RECEIVER_TEST_V1_OPCODE, LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE,
        LeControllerBootstrap, LeControllerBootstrapConfig, LeControllerClassifiedCommandRoute,
        LeControllerCommandClassification, LeControllerHciResources, LeDtmCommand,
        LeReceiverTestV1Command, LeTestEndCommand, OwnedBootstrapCommand,
    };

    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    #[derive(Debug, Eq, PartialEq)]
    struct RadioOwner(u32);

    #[derive(Debug, Eq, PartialEq)]
    struct QuiescedOwner(u32);

    type ControllerResources = LeControllerHciResources<NoopRawMutex, 1, 1, 16>;

    fn controller_resources() -> ControllerResources {
        ControllerResources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .expect("the test HCI profile is nonzero"),
        )
        .expect("the profile fits its source-owned storage")
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

    impl WriteHci for RawCommand<'_> {
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

    impl HostToControllerPacket for RawCommand<'_> {
        const KIND: PacketKind = PacketKind::Cmd;
    }

    fn receiver_command() -> LeReceiverTestV1Command {
        let LeDtmCommand::ReceiverTestV1(command) =
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[7])
                .expect("the reviewed receiver command is valid")
        else {
            panic!("receiver opcode changed semantic command kind");
        };
        command
    }

    fn test_end_command() -> LeTestEndCommand {
        let LeDtmCommand::TestEnd(command) = LeDtmCommand::decode_body(LE_TEST_END_OPCODE, &[])
            .expect("the reviewed Test End command is valid")
        else {
            panic!("Test End opcode changed semantic command kind");
        };
        command
    }

    #[test]
    fn full_queue_retains_the_transformed_radio_until_exact_publication() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("the output queue starts empty");

        let pending = LeControllerResponsePending::new(
            11_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        )
        .map_owner(|radio| u16::from(radio) + 20);

        let LeControllerResponsePublication::Pending(pending) = pending.try_publish(&controller)
        else {
            panic!("a full matching queue must retain the pending owner");
        };
        assert_eq!(*pending.owner(), 31);

        let mut buffer = [0; 16];
        let ControllerToHostPacket::Event(event) =
            block_on(host.read(&mut buffer)).expect("the Host drains the older event")
        else {
            panic!("the retained older packet changed kind");
        };
        assert_eq!(event.kind, EventKind::HardwareError);

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&controller)
        else {
            panic!("the retained response must publish after capacity returns");
        };
        assert_eq!(*published.owner(), 31);
        assert!(published.accepts_endpoint(&controller));
        assert_start_response(block_on(host.read(&mut buffer)).unwrap());
    }

    #[test]
    fn wrong_endpoint_retains_both_axes_and_published_affinity() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut first_channel = Channel::new();
        let (_first_host, first) = first_channel.split();
        let mut second_channel = Channel::new();
        let (_second_host, second) = second_channel.split();
        let pending = LeControllerResponsePending::new(
            37_u8,
            receiver_command().into_started_command_complete(),
            first.epoch_identity(),
        );

        let LeControllerResponsePublication::EndpointMismatch(pending) =
            pending.try_publish(&second)
        else {
            panic!("a foreign endpoint must retain the complete pending owner");
        };
        assert_eq!(*pending.owner(), 37);

        let LeControllerResponsePublication::Published(published) = pending.try_publish(&first)
        else {
            panic!("the original endpoint must accept the retained response");
        };
        assert!(published.accepts_endpoint(&first));
        assert!(!published.accepts_endpoint(&second));
    }

    #[test]
    fn non_capacity_fault_retains_the_unchanged_radio_owner() {
        type TinyChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 3>;

        let mut channel = TinyChannel::new();
        let (_host, controller) = channel.split();
        let pending = LeControllerResponsePending::new(
            41_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );

        let LeControllerResponsePublication::Fault { pending, error } =
            pending.try_publish(&controller)
        else {
            panic!("undersized packet storage must fail without releasing ownership");
        };
        assert_eq!(
            error,
            HciChannelError::PacketTooLong {
                length: 6,
                capacity: 3,
            }
        );
        assert_eq!(*pending.owner(), 41);
    }

    #[test]
    fn successful_publication_is_exact_once_and_preserves_existing_fifo_order() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 2, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("the first FIFO slot starts empty");
        let pending = LeControllerResponsePending::new(
            43_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&controller)
        else {
            panic!("the second FIFO slot must accept the start response");
        };
        assert_eq!(*published.owner(), 43);

        let mut buffer = [0; 16];
        let ControllerToHostPacket::Event(first) = block_on(host.read(&mut buffer)).unwrap() else {
            panic!("the older event changed packet kind");
        };
        assert_eq!(first.kind, EventKind::HardwareError);
        assert_start_response(block_on(host.read(&mut buffer)).unwrap());

        let published: LeControllerResponsePublished<'_, u16> =
            published.map_owner(|radio| u16::from(radio) + 1);
        assert_eq!(*published.owner(), 44);
        assert!(published.accepts_endpoint(&controller));
    }

    #[test]
    fn ordered_axis_retains_and_publishes_a_typed_bootstrap_completion() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("the output queue starts empty");
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .expect("the test HCI profile is nonzero");
        let mut bootstrap = LeControllerBootstrap::new(config);
        let response = bootstrap.dispatch_owned(OwnedBootstrapCommand::Reset);
        let pending =
            LeControllerResponsePending::new(RadioOwner(46), response, controller.epoch_identity());

        let LeControllerResponsePublication::Pending(pending) = pending.try_publish(&controller)
        else {
            panic!("bootstrap response must retain its owner across backpressure");
        };
        assert_eq!(pending.owner(), &RadioOwner(46));

        let mut buffer = [0; 16];
        let ControllerToHostPacket::Event(older) = block_on(host.read(&mut buffer)).unwrap() else {
            panic!("the older event changed packet kind");
        };
        assert_eq!(older.kind, EventKind::HardwareError);
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&controller)
        else {
            panic!("bootstrap response must publish after capacity returns");
        };
        assert_eq!(published.into_owner(), RadioOwner(46));

        let ControllerToHostPacket::Event(event) = block_on(host.read(&mut buffer)).unwrap() else {
            panic!("bootstrap completion changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("bootstrap response is a complete bt-hci event");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("Reset returns a standard status");
        assert_eq!(complete.cmd_opcode, Reset::OPCODE);
        assert_eq!(complete.status, Status::SUCCESS);
    }

    #[test]
    fn published_response_orders_the_next_dtm_completion() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 2, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        let start = LeControllerResponsePending::new(
            47_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) = start.try_publish(&controller)
        else {
            panic!("the empty queue must accept the start response");
        };
        let ending =
            started.begin_next_response(test_end_command().into_ended_command_complete(0x3412));
        let LeControllerResponsePublication::Published(ended) = ending.try_publish(&controller)
        else {
            panic!("the second slot must accept Test End after the start response");
        };
        assert_eq!(ended.into_owner(), 47);

        let mut buffer = [0; 16];
        assert_start_response(block_on(host.read(&mut buffer)).unwrap());
        assert_test_end_response(block_on(host.read(&mut buffer)).unwrap());
    }

    #[test]
    fn idle_route_retains_zero_count_response_and_owner_through_backpressure() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("the output queue starts empty");
        block_on(host.write(&LeTestEnd::new())).expect("Test End enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = match controller.try_receive_classified_command(&mut command_buffer) {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify Test End"),
        };
        let command = match classified.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("Test End must refine to DTM"),
        };

        let LeDtmIdleCommandRoute::ResponsePending(pending) = route_idle_dtm_command(
            RadioOwner(67),
            controller.epoch_identity(),
            &controller,
            command,
        ) else {
            panic!("idle Test End must produce the standard zero-count response");
        };
        let LeControllerResponsePublication::Pending(pending) = pending.try_publish(&controller)
        else {
            panic!("the full queue must retain the exact idle owner and response");
        };
        assert_eq!(pending.owner(), &RadioOwner(67));

        let mut response_buffer = [0; 16];
        let ControllerToHostPacket::Event(older) =
            block_on(host.read(&mut response_buffer)).expect("the Host drains the older event")
        else {
            panic!("the retained older packet changed kind");
        };
        assert_eq!(older.kind, EventKind::HardwareError);
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&controller)
        else {
            panic!("idle Test End must publish once capacity returns");
        };
        assert_eq!(published.into_owner(), RadioOwner(67));
        assert_test_end_packet_count(
            block_on(host.read(&mut response_buffer)).expect("Test End response remains queued"),
            0,
        );
    }

    #[test]
    fn idle_route_releases_both_validated_start_kinds_only_to_the_matching_endpoint() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut receiver_channel = Channel::new();
        let (receiver_host, receiver_controller) = receiver_channel.split();
        block_on(receiver_host.write(&RawCommand::new(LE_RECEIVER_TEST_V1_OPCODE, &[13])))
            .expect("the receiver command enters the real Host queue");
        let mut receiver_buffer = [0; 16];
        let receiver =
            match receiver_controller.try_receive_classified_command(&mut receiver_buffer) {
                Ok(classified) => classified,
                Err(_) => panic!("the receiver endpoint must classify its command"),
            };
        let receiver = match receiver.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the receiver command must refine to DTM"),
        };
        let LeDtmIdleCommandRoute::StartReceiver { owner, command } = route_idle_dtm_command(
            RadioOwner(71),
            receiver_controller.epoch_identity(),
            &receiver_controller,
            receiver,
        ) else {
            panic!("the matching receiver command must be released for hardware start");
        };
        assert_eq!(owner, RadioOwner(71));
        assert_eq!(command.channel().index(), 13);

        let mut transmitter_channel = Channel::new();
        let (transmitter_host, transmitter_controller) = transmitter_channel.split();
        block_on(transmitter_host.write(&RawCommand::new(
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[17, 23, 2],
        )))
        .expect("the transmitter command enters the real Host queue");
        let mut transmitter_buffer = [0; 16];
        let transmitter =
            match transmitter_controller.try_receive_classified_command(&mut transmitter_buffer) {
                Ok(classified) => classified,
                Err(_) => panic!("the transmitter endpoint must classify its command"),
            };
        let transmitter = match transmitter.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the transmitter command must refine to DTM"),
        };
        let LeDtmIdleCommandRoute::StartTransmitter { owner, command } = route_idle_dtm_command(
            RadioOwner(73),
            transmitter_controller.epoch_identity(),
            &transmitter_controller,
            transmitter,
        ) else {
            panic!("the matching transmitter command must be released for hardware start");
        };
        assert_eq!(owner, RadioOwner(73));
        assert_eq!(command.channel().index(), 17);
        assert_eq!(command.payload_length(), 23);
        assert_eq!(command.payload_pattern().hci_parameter(), 2);
    }

    #[test]
    fn idle_route_cross_epoch_rejections_retain_the_exact_owner_and_command() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut first_channel = Channel::new();
        let (_first_host, first_controller) = first_channel.split();
        let mut second_channel = Channel::new();
        let (second_host, second_controller) = second_channel.split();
        block_on(second_host.write(&LeTestEnd::new()))
            .expect("the foreign command enters its own Host queue");
        let mut command_buffer = [0; 16];
        let classified = match second_controller.try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the foreign endpoint must classify its command"),
        };
        let command = match classified.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the foreign command must retain its endpoint proof"),
        };

        let LeDtmIdleCommandRoute::EndpointMismatch { owner, command } = route_idle_dtm_command(
            RadioOwner(79),
            first_controller.epoch_identity(),
            &first_controller,
            command,
        ) else {
            panic!("a foreign command must not consume the idle owner");
        };
        assert_eq!(owner, RadioOwner(79));
        assert!(command.originates_from(&second_controller));
        assert!(!command.originates_from(&first_controller));

        block_on(second_host.write(&LeTestEnd::new()))
            .expect("the second foreign command enters its own Host queue");
        let classified = match second_controller.try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the foreign endpoint must classify its second command"),
        };
        let command = match classified.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the second command must retain its endpoint proof"),
        };
        let LeDtmIdleCommandRoute::EndpointMismatch { owner, command } = route_idle_dtm_command(
            RadioOwner(83),
            first_controller.epoch_identity(),
            &second_controller,
            command,
        ) else {
            panic!("a foreign owner epoch must retain both axes");
        };
        assert_eq!(owner, RadioOwner(83));
        assert!(command.originates_from(&second_controller));
    }

    #[test]
    fn classified_router_hands_both_dtm_start_kinds_to_session_policy() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = LeControllerResponsePending::new(
            RadioOwner(53),
            receiver_command().into_started_command_complete(),
            endpoints.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(endpoints.controller.transport())
        else {
            panic!("the empty queue must accept the start response");
        };

        block_on(
            endpoints
                .host
                .write(&RawCommand::new(LE_RECEIVER_TEST_V1_OPCODE, &[11])),
        )
        .expect("the receiver command enters the real Host queue");
        let mut command_buffer = [0; 16];
        let receiver = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify the queued command"),
        };
        let LeControllerClassifiedCommandRoute::Dtm { published, command } = endpoints
            .controller
            .route_classified_command(started, receiver)
        else {
            panic!("the combined router must hand the receiver command to session policy");
        };
        let LeDtmCommand::ReceiverTestV1(receiver) = command else {
            panic!("the receiver command changed semantic DTM kind");
        };
        assert_eq!(receiver.channel().index(), 11);
        assert_eq!(published.owner(), &RadioOwner(53));

        block_on(endpoints.host.write(&RawCommand::new(
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[17, 23, 2],
        )))
        .expect("the transmitter command enters the real Host queue");
        let transmitter = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify the transmitter command"),
        };
        let LeControllerClassifiedCommandRoute::Dtm { published, command } = endpoints
            .controller
            .route_classified_command(published, transmitter)
        else {
            panic!("the combined router must hand the transmitter command to session policy");
        };
        let LeDtmCommand::TransmitterTestV1(transmitter) = command else {
            panic!("the transmitter command changed semantic DTM kind");
        };
        assert_eq!(transmitter.channel().index(), 17);
        assert_eq!(transmitter.payload_length(), 23);
        assert_eq!(transmitter.payload_pattern().hci_parameter(), 2);

        let mut buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
        assert_eq!(published.into_owner(), RadioOwner(53));
    }

    #[test]
    fn classified_router_hands_test_end_to_session_policy_with_published_order() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = LeControllerResponsePending::new(
            RadioOwner(59),
            receiver_command().into_started_command_complete(),
            endpoints.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(endpoints.controller.transport())
        else {
            panic!("the empty queue must accept the start response");
        };
        block_on(endpoints.host.write(&LeTestEnd::new()))
            .expect("Test End enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify Test End"),
        };
        let LeControllerClassifiedCommandRoute::Dtm { published, command } = endpoints
            .controller
            .route_classified_command(started, classified)
        else {
            panic!("Test End must remain semantic for the caller's session policy");
        };
        assert_eq!(published.owner(), &RadioOwner(59));
        assert!(published.accepts_endpoint(endpoints.controller.transport()));
        let LeDtmCommand::TestEnd(command) = command else {
            panic!("Test End changed semantic DTM kind");
        };
        let response = command.into_ended_command_complete(0x1234);
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0x1234);
    }

    #[test]
    fn reset_completion_is_exact_once_and_retained_through_backpressure() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let start = LeControllerResponsePending::new(
            RadioOwner(60),
            receiver_command().into_started_command_complete(),
            endpoints.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(endpoints.controller.transport())
        else {
            panic!("the empty queue must accept the start response");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify Reset"),
        };
        let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) = endpoints
            .controller
            .route_classified_command(started, classified)
        else {
            panic!("Reset must become an opaque lifecycle barrier");
        };
        let (active, continuation) = barrier.into_parts();
        assert_eq!(active, RadioOwner(60));
        assert_eq!(continuation.owner(), &());
        assert!(continuation.accepts_endpoint(endpoints.controller.transport()));
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        let mut response_buffer = [0; 16];
        assert_start_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
        endpoints
            .controller
            .transport()
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("Reset produced no completion before lifecycle quiescence");

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
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the probe event must backpressure the exact Reset completion");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(61));

        let ControllerToHostPacket::Event(event) =
            block_on(endpoints.host.read(&mut response_buffer)).unwrap()
        else {
            panic!("the retained probe event changed kind");
        };
        assert_eq!(event.kind, EventKind::HardwareError);

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the retained Reset completion must publish after capacity returns");
        };
        assert_eq!(published.into_owner(), QuiescedOwner(61));
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
        let start = LeControllerResponsePending::new(
            RadioOwner(71),
            receiver_command().into_started_command_complete(),
            first.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(first.controller.transport())
        else {
            panic!("the first endpoint must publish its start response");
        };
        block_on(first.host.write(&Reset::new())).expect("Reset enters the first Host transport");
        let mut command_buffer = [0; 16];
        let classified = match first
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the first endpoint must classify Reset"),
        };
        let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) = first
            .controller
            .route_classified_command(started, classified)
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
        assert!(barrier.accepts_endpoint(first.controller.transport()));
        assert!(!barrier.accepts_endpoint(second.controller.transport()));
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
            pending.try_publish(first.controller.transport())
        else {
            panic!("the queued start response must retain Reset completion");
        };
        assert_eq!(pending.owner(), &QuiescedOwner(72));

        let mut response_buffer = [0; 16];
        assert_start_response(block_on(first.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(first.controller.transport())
        else {
            panic!("the original endpoint must publish after capacity returns");
        };
        assert_eq!(published.into_owner(), QuiescedOwner(72));
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
        let start = LeControllerResponsePending::new(
            RadioOwner(62),
            receiver_command().into_started_command_complete(),
            endpoints.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(active) =
            start.try_publish(endpoints.controller.transport())
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
        let malformed_bootstrap = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify malformed bootstrap"),
        };
        assert!(matches!(
            malformed_bootstrap.value(),
            LeControllerCommandClassification::MalformedBootstrap(_)
        ));
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_classified_command(active, malformed_bootstrap)
        else {
            panic!("malformed bootstrap must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the queued start response must backpressure malformed bootstrap");
        };
        assert_start_response(block_on(endpoints.host.read(&mut response_buffer)).unwrap());
        let LeControllerResponsePublication::Published(active) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("malformed bootstrap must publish after capacity returns");
        };

        block_on(
            endpoints
                .host
                .write(&RawCommand::new(LE_RECEIVER_TEST_V1_OPCODE, &[])),
        )
        .expect("the malformed DTM command enters the real Host queue");
        let malformed_dtm = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify malformed DTM"),
        };
        assert!(matches!(
            malformed_dtm.value(),
            LeControllerCommandClassification::MalformedDtm(_)
        ));
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_classified_command(active, malformed_dtm)
        else {
            panic!("malformed DTM must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the queued bootstrap error must backpressure malformed DTM");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            SetEventMask::OPCODE,
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        );
        let LeControllerResponsePublication::Published(active) =
            pending.try_publish(endpoints.controller.transport())
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
        let unsupported = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify unsupported command"),
        };
        assert!(matches!(
            unsupported.value(),
            LeControllerCommandClassification::Unsupported(_)
        ));
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_classified_command(active, unsupported)
        else {
            panic!("unsupported command must immediately become an ordered response");
        };
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the queued DTM error must backpressure Unknown Command");
        };
        assert_command_status(
            block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
            LE_RECEIVER_TEST_V1_OPCODE,
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        );
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("Unknown Command must publish after capacity returns");
        };
        assert_eq!(published.into_owner(), RadioOwner(62));
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
        let start = LeControllerResponsePending::new(
            RadioOwner(61),
            receiver_command().into_started_command_complete(),
            first.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(started) =
            start.try_publish(first.controller.transport())
        else {
            panic!("the first endpoint must publish its start response");
        };

        let mut second_resources = controller_resources();
        let second = second_resources.split();
        block_on(second.host.write(&LeTestEnd::new()))
            .expect("the foreign DTM command enters its own Host queue");
        let mut command_buffer = [0; 16];
        let classified = match second
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the foreign endpoint must classify its own command"),
        };
        let LeControllerClassifiedCommandRoute::EndpointMismatch { published, command } = first
            .controller
            .route_classified_command(started, classified)
        else {
            panic!("a foreign command must not consume the first published owner");
        };
        assert_eq!(published.owner(), &RadioOwner(61));
        assert!(published.accepts_endpoint(first.controller.transport()));
        assert!(!published.accepts_endpoint(second.controller.transport()));
        assert!(command.originates_from(second.controller.transport()));
        assert!(!command.originates_from(first.controller.transport()));
        assert!(matches!(
            command.value(),
            LeControllerCommandClassification::Dtm(LeDtmCommand::TestEnd(_))
        ));

        let second_start = LeControllerResponsePending::new(
            RadioOwner(63),
            receiver_command().into_started_command_complete(),
            second.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(second_started) =
            second_start.try_publish(second.controller.transport())
        else {
            panic!("the second endpoint must publish its own start response");
        };
        block_on(second.host.write(&LeTestEnd::new()))
            .expect("the second endpoint accepts another command");
        let second_classified = match second
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the second endpoint must classify its next command"),
        };
        let LeControllerClassifiedCommandRoute::EndpointMismatch { published, command } = first
            .controller
            .route_classified_command(second_started, second_classified)
        else {
            panic!("a foreign published order must retain both exact owners");
        };
        assert_eq!(published.owner(), &RadioOwner(63));
        assert!(published.accepts_endpoint(second.controller.transport()));
        assert!(!published.accepts_endpoint(first.controller.transport()));
        assert!(command.originates_from(second.controller.transport()));
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

    fn parse_command_complete(bytes: &[u8]) -> CommandCompleteWithStatus<'_> {
        let (packet, remaining) =
            ControllerToHostPacket::from_hci_bytes_with_kind(PacketKind::Event, bytes).unwrap();
        assert!(remaining.is_empty());
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Command Complete changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
        complete.try_into().unwrap()
    }
}
