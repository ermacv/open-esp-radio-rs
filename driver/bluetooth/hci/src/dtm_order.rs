//! Portable ordering authority for LE DTM command responses.
//!
//! Radio progress and Controller-to-Host capacity are deliberately independent:
//! a chip-specific runner may transform the retained radio owner while the exact
//! Command Complete remains pending. Only successful insertion into the matching
//! HCI epoch advances the order state to [`LeDtmResponsePublished`].

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    HciChannelError, HciControllerResponse, HciEpochBound, HciEpochIdentity,
    InProcessHciControllerEndpoint, LeDtmActiveSessionDisposition, LeDtmCommand,
    LeDtmCommandCompleteEvent, LeDtmIdleSessionDisposition, LeReceiverTestV1Command,
    LeTestEndCommand, LeTransmitterTestV1Command,
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
                LeDtmIdleCommandRoute::ResponsePending(LeDtmResponsePending::new(
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
    ResponsePending(LeDtmResponsePending<'epoch, Owner>),
    /// Either idle owner or command belongs to another live Controller epoch.
    EndpointMismatch {
        /// Unchanged idle owner.
        owner: Owner,
        /// Unchanged semantic command retaining its source-epoch proof.
        command: HciEpochBound<'command, LeDtmCommand>,
    },
}

/// A radio owner paired with one exact not-yet-published DTM response.
///
/// The radio owner remains available by shared reference and may be transformed
/// with [`Self::map_radio`] without exposing or rebuilding the response. Queue
/// backpressure, an endpoint mismatch, and every other transport error return
/// this complete owner unchanged.
#[must_use = "the radio owner and exact DTM response must remain retained"]
pub struct LeDtmResponsePending<'epoch, Radio> {
    radio: Radio,
    response: LeDtmCommandCompleteEvent,
    hci_epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Radio> LeDtmResponsePending<'epoch, Radio> {
    /// Bind one radio owner to the response and HCI epoch which admitted it.
    pub fn new(
        radio: Radio,
        response: LeDtmCommandCompleteEvent,
        hci_epoch: HciEpochIdentity<'epoch>,
    ) -> Self {
        Self {
            radio,
            response,
            hci_epoch,
        }
    }

    /// Borrow the independently progressing radio axis.
    pub const fn radio(&self) -> &Radio {
        &self.radio
    }

    /// Separate the radio from a unit response-order marker for typed composition.
    ///
    /// This is a consuming decomposition: the exact response bytes and epoch
    /// remain in `LeDtmResponsePending<()>`, and neither output can be recreated
    /// from the other. Chip-specific session aggregates immediately reunite
    /// both outputs around their independently progressing radio axis.
    pub fn into_parts(self) -> (Radio, LeDtmResponsePending<'epoch, ()>) {
        (
            self.radio,
            LeDtmResponsePending {
                radio: (),
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

    /// Transform only the radio axis while retaining the exact response authority.
    pub fn map_radio<Next>(
        self,
        map: impl FnOnce(Radio) -> Next,
    ) -> LeDtmResponsePending<'epoch, Next> {
        LeDtmResponsePending {
            radio: map(self.radio),
            response: self.response,
            hci_epoch: self.hci_epoch,
        }
    }

    /// Attempt the sole durable publication through the matching HCI epoch.
    ///
    /// Capacity is not reserved. `Full` therefore returns `Pending` for an exact
    /// later retry. No error releases the radio owner or response bytes.
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
    ) -> LeDtmResponsePublication<'epoch, Radio> {
        if !self.matches_endpoint(controller) {
            return LeDtmResponsePublication::EndpointMismatch(self);
        }

        match controller.try_publish(self.response.kind(), self.response.as_bytes()) {
            Ok(()) => LeDtmResponsePublication::Published(LeDtmResponsePublished {
                radio: self.radio,
                hci_epoch: self.hci_epoch,
            }),
            Err(HciChannelError::Full) => LeDtmResponsePublication::Pending(self),
            Err(error) => LeDtmResponsePublication::Fault {
                pending: self,
                error,
            },
        }
    }
}

/// A radio owner whose DTM response entered its matching HCI epoch.
///
/// This state contains no response bytes and exposes no publication operation,
/// making a duplicate insertion unrepresentable. The retained epoch marker
/// authorizes later command intake and the next ordered response transaction.
#[must_use = "the active radio owner and its HCI affinity must remain retained"]
pub struct LeDtmResponsePublished<'epoch, Radio> {
    radio: Radio,
    hci_epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Radio> LeDtmResponsePublished<'epoch, Radio> {
    /// Borrow the independently progressing radio axis.
    pub const fn radio(&self) -> &Radio {
        &self.radio
    }

    /// Release the retained owner after the response was durably published.
    ///
    /// The response authority is absent from this typestate, so consuming the
    /// owner cannot enable duplicate publication. Terminal session transitions
    /// use this edge before restoring their hardware graph.
    pub fn into_radio(self) -> Radio {
        self.radio
    }

    /// Separate the radio from its published unit order proof for typed composition.
    pub fn into_parts(self) -> (Radio, LeDtmResponsePublished<'epoch, ()>) {
        (
            self.radio,
            LeDtmResponsePublished {
                radio: (),
                hci_epoch: self.hci_epoch,
            },
        )
    }

    /// Transform only the radio axis while retaining the published order proof.
    pub fn map_radio<Next>(
        self,
        map: impl FnOnce(Radio) -> Next,
    ) -> LeDtmResponsePublished<'epoch, Next> {
        LeDtmResponsePublished {
            radio: map(self.radio),
            hci_epoch: self.hci_epoch,
        }
    }

    /// Begin the next ordered DTM response in this same live HCI epoch.
    ///
    /// The previous response authority has already been consumed by successful
    /// publication. A higher session layer must retain the semantic command and
    /// radio owner needed to construct `response`; this transition supplies
    /// only ordering and endpoint affinity.
    pub fn begin_next_response(
        self,
        response: LeDtmCommandCompleteEvent,
    ) -> LeDtmResponsePending<'epoch, Radio> {
        LeDtmResponsePending {
            radio: self.radio,
            response,
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

    /// Route one endpoint-bound command under the portable active-DTM policy.
    ///
    /// The published response order and command must both belong to `controller`.
    /// A second start is converted into the standard Controller Busy response
    /// while retaining `Radio` inside the next pending transaction. Test End
    /// retains the published order proof beside its semantic command so a chip
    /// runner can quiesce exactly that radio before beginning its response.
    pub fn route_active_command<
        'command,
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
        command: HciEpochBound<'command, LeDtmCommand>,
    ) -> LeDtmActiveCommandRoute<'epoch, 'command, Radio> {
        if !self.accepts_endpoint(controller) {
            return LeDtmActiveCommandRoute::EndpointMismatch {
                published: self,
                command,
            };
        }
        match command.try_into_for_endpoint(controller) {
            Ok(command) => match command.into_active_session_disposition() {
                LeDtmActiveSessionDisposition::RejectControllerBusy(response) => {
                    LeDtmActiveCommandRoute::BusyResponsePending(self.begin_next_response(response))
                }
                LeDtmActiveSessionDisposition::End(command) => LeDtmActiveCommandRoute::TestEnd {
                    published: self,
                    command,
                },
            },
            Err(command) => LeDtmActiveCommandRoute::EndpointMismatch {
                published: self,
                command,
            },
        }
    }
}

/// Portable result of routing one endpoint-bound command while DTM is active.
#[must_use = "publish Busy, run Test End, or retain the exact epoch mismatch"]
pub enum LeDtmActiveCommandRoute<'epoch, 'command, Radio> {
    /// A second RX/TX start became an ordered Controller Busy response.
    BusyResponsePending(LeDtmResponsePending<'epoch, Radio>),
    /// Test End retains the exact active radio, order proof and semantic command.
    TestEnd {
        /// Published prior-response owner retaining the active radio.
        published: LeDtmResponsePublished<'epoch, Radio>,
        /// Semantic Test End command released only after epoch validation.
        command: LeTestEndCommand,
    },
    /// Either order or command belongs to another live Controller epoch.
    EndpointMismatch {
        /// Unchanged published owner retaining the exact radio.
        published: LeDtmResponsePublished<'epoch, Radio>,
        /// Unchanged semantic command retaining its source-epoch proof.
        command: HciEpochBound<'command, LeDtmCommand>,
    },
}

/// Result of one consuming DTM response publication attempt.
#[must_use = "retain the unchanged pending owner or the published order proof"]
pub enum LeDtmResponsePublication<'epoch, Radio> {
    /// The response entered the matching queue exactly once.
    Published(LeDtmResponsePublished<'epoch, Radio>),
    /// The matching queue is full; the complete owner is unchanged.
    Pending(LeDtmResponsePending<'epoch, Radio>),
    /// The supplied endpoint belongs to another live HCI epoch.
    EndpointMismatch(LeDtmResponsePending<'epoch, Radio>),
    /// A non-capacity transport failure retained the complete owner.
    Fault {
        /// Unchanged radio and response authority.
        pending: LeDtmResponsePending<'epoch, Radio>,
        /// Exact validation or transport failure.
        error: HciChannelError,
    },
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes, PacketKind,
        cmd::le::LeTestEnd,
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::Status,
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{
        LeDtmIdleCommandRoute, LeDtmResponsePending, LeDtmResponsePublication,
        LeDtmResponsePublished, route_idle_dtm_command,
    };
    use crate::{
        HciChannelError, InProcessHciChannel, LE_RECEIVER_TEST_V1_OPCODE, LE_TEST_END_OPCODE,
        LE_TRANSMITTER_TEST_V1_OPCODE, LeDtmActiveCommandRoute, LeDtmCommand,
        LeReceiverTestV1Command, LeTestEndCommand,
    };

    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    #[derive(Debug, Eq, PartialEq)]
    struct RadioOwner(u32);

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

        let pending = LeDtmResponsePending::new(
            11_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        )
        .map_radio(|radio| u16::from(radio) + 20);

        let LeDtmResponsePublication::Pending(pending) = pending.try_publish(&controller) else {
            panic!("a full matching queue must retain the pending owner");
        };
        assert_eq!(*pending.radio(), 31);

        let mut buffer = [0; 16];
        let ControllerToHostPacket::Event(event) =
            block_on(host.read(&mut buffer)).expect("the Host drains the older event")
        else {
            panic!("the retained older packet changed kind");
        };
        assert_eq!(event.kind, EventKind::HardwareError);

        let LeDtmResponsePublication::Published(published) = pending.try_publish(&controller)
        else {
            panic!("the retained response must publish after capacity returns");
        };
        assert_eq!(*published.radio(), 31);
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
        let pending = LeDtmResponsePending::new(
            37_u8,
            receiver_command().into_started_command_complete(),
            first.epoch_identity(),
        );

        let LeDtmResponsePublication::EndpointMismatch(pending) = pending.try_publish(&second)
        else {
            panic!("a foreign endpoint must retain the complete pending owner");
        };
        assert_eq!(*pending.radio(), 37);

        let LeDtmResponsePublication::Published(published) = pending.try_publish(&first) else {
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
        let pending = LeDtmResponsePending::new(
            41_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );

        let LeDtmResponsePublication::Fault { pending, error } = pending.try_publish(&controller)
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
        assert_eq!(*pending.radio(), 41);
    }

    #[test]
    fn successful_publication_is_exact_once_and_preserves_existing_fifo_order() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 2, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .expect("the first FIFO slot starts empty");
        let pending = LeDtmResponsePending::new(
            43_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );

        let LeDtmResponsePublication::Published(published) = pending.try_publish(&controller)
        else {
            panic!("the second FIFO slot must accept the start response");
        };
        assert_eq!(*published.radio(), 43);

        let mut buffer = [0; 16];
        let ControllerToHostPacket::Event(first) = block_on(host.read(&mut buffer)).unwrap() else {
            panic!("the older event changed packet kind");
        };
        assert_eq!(first.kind, EventKind::HardwareError);
        assert_start_response(block_on(host.read(&mut buffer)).unwrap());

        let published: LeDtmResponsePublished<'_, u16> =
            published.map_radio(|radio| u16::from(radio) + 1);
        assert_eq!(*published.radio(), 44);
        assert!(published.accepts_endpoint(&controller));
    }

    #[test]
    fn published_response_orders_the_next_dtm_completion() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 2, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        let start = LeDtmResponsePending::new(
            47_u8,
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );
        let LeDtmResponsePublication::Published(started) = start.try_publish(&controller) else {
            panic!("the empty queue must accept the start response");
        };
        let ending =
            started.begin_next_response(test_end_command().into_ended_command_complete(0x3412));
        let LeDtmResponsePublication::Published(ended) = ending.try_publish(&controller) else {
            panic!("the second slot must accept Test End after the start response");
        };
        assert_eq!(ended.into_radio(), 47);

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
        let LeDtmResponsePublication::Pending(pending) = pending.try_publish(&controller) else {
            panic!("the full queue must retain the exact idle owner and response");
        };
        assert_eq!(pending.radio(), &RadioOwner(67));

        let mut response_buffer = [0; 16];
        let ControllerToHostPacket::Event(older) =
            block_on(host.read(&mut response_buffer)).expect("the Host drains the older event")
        else {
            panic!("the retained older packet changed kind");
        };
        assert_eq!(older.kind, EventKind::HardwareError);
        let LeDtmResponsePublication::Published(published) = pending.try_publish(&controller)
        else {
            panic!("idle Test End must publish once capacity returns");
        };
        assert_eq!(published.into_radio(), RadioOwner(67));
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
        block_on(receiver_host.write(&LeTestEnd::new()))
            .expect("the epoch source enters the real Host queue");
        let mut receiver_buffer = [0; 16];
        let receiver =
            match receiver_controller.try_receive_classified_command(&mut receiver_buffer) {
                Ok(classified) => classified,
                Err(_) => panic!("the receiver endpoint must classify its command"),
            };
        let receiver = match receiver.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the receiver epoch source must refine to DTM"),
        }
        .map(|_| {
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[13])
                .expect("the reviewed receiver request is valid")
        });
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
        block_on(transmitter_host.write(&LeTestEnd::new()))
            .expect("the epoch source enters the real Host queue");
        let mut transmitter_buffer = [0; 16];
        let transmitter =
            match transmitter_controller.try_receive_classified_command(&mut transmitter_buffer) {
                Ok(classified) => classified,
                Err(_) => panic!("the transmitter endpoint must classify its command"),
            };
        let transmitter = match transmitter.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the transmitter epoch source must refine to DTM"),
        }
        .map(|_| {
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[17, 23, 2])
                .expect("the reviewed transmitter request is valid")
        });
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
    fn active_route_retains_radio_and_fifo_through_busy_backpressure() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        let start = LeDtmResponsePending::new(
            RadioOwner(53),
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );
        let LeDtmResponsePublication::Published(started) = start.try_publish(&controller) else {
            panic!("the empty queue must accept the start response");
        };

        block_on(host.write(&LeTestEnd::new()))
            .expect("the epoch source command enters the real Host queue");
        let mut command_buffer = [0; 16];
        let classified = match controller.try_receive_classified_command(&mut command_buffer) {
            Ok(classified) => classified,
            Err(_) => panic!("the real Controller endpoint must classify the queued command"),
        };
        let command = match classified.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the typed epoch source must refine to DTM"),
        }
        .map(|_| {
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[11])
                .expect("the routed receiver request is semantically valid")
        });
        let LeDtmActiveCommandRoute::BusyResponsePending(busy) =
            started.route_active_command(&controller, command)
        else {
            panic!("the portable active route must produce Controller Busy");
        };
        let LeDtmResponsePublication::Pending(busy) = busy.try_publish(&controller) else {
            panic!("the queued start response must backpressure Controller Busy");
        };
        assert_eq!(busy.radio(), &RadioOwner(53));

        let mut buffer = [0; 16];
        assert_start_response(block_on(host.read(&mut buffer)).unwrap());
        let LeDtmResponsePublication::Published(published) = busy.try_publish(&controller) else {
            panic!("Controller Busy must publish once capacity returns");
        };
        assert_eq!(published.into_radio(), RadioOwner(53));
        assert_controller_busy(block_on(host.read(&mut buffer)).unwrap());
    }

    #[test]
    fn active_route_retains_test_end_radio_command_and_published_order() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut channel = Channel::new();
        let (host, controller) = channel.split();
        let start = LeDtmResponsePending::new(
            RadioOwner(59),
            receiver_command().into_started_command_complete(),
            controller.epoch_identity(),
        );
        let LeDtmResponsePublication::Published(started) = start.try_publish(&controller) else {
            panic!("the empty queue must accept the start response");
        };
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

        let LeDtmActiveCommandRoute::TestEnd { published, command } =
            started.route_active_command(&controller, command)
        else {
            panic!("Test End must remain semantic until hardware quiescence");
        };
        assert_eq!(published.radio(), &RadioOwner(59));
        assert!(published.accepts_endpoint(&controller));
        let response = command.into_ended_command_complete(0x1234);
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0x1234);
    }

    #[test]
    fn active_route_cross_epoch_rejection_retains_both_exact_owners() {
        type Channel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

        let mut first_channel = Channel::new();
        let (_first_host, first_controller) = first_channel.split();
        let start = LeDtmResponsePending::new(
            RadioOwner(61),
            receiver_command().into_started_command_complete(),
            first_controller.epoch_identity(),
        );
        let LeDtmResponsePublication::Published(started) = start.try_publish(&first_controller)
        else {
            panic!("the first endpoint must publish its start response");
        };

        let mut second_channel = Channel::new();
        let (second_host, second_controller) = second_channel.split();
        block_on(second_host.write(&LeTestEnd::new()))
            .expect("the foreign DTM command enters its own Host queue");
        let mut command_buffer = [0; 16];
        let classified = match second_controller.try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the foreign endpoint must classify its own command"),
        };
        let command = match classified.try_into_dtm() {
            Ok(command) => command,
            Err(_) => panic!("the foreign DTM command must retain its endpoint proof"),
        };

        let LeDtmActiveCommandRoute::EndpointMismatch { published, command } =
            started.route_active_command(&first_controller, command)
        else {
            panic!("a foreign command must not consume the first active owner");
        };
        assert_eq!(published.radio(), &RadioOwner(61));
        assert!(published.accepts_endpoint(&first_controller));
        assert!(!published.accepts_endpoint(&second_controller));
        assert!(command.originates_from(&second_controller));
        assert!(!command.originates_from(&first_controller));
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

    fn assert_controller_busy(packet: ControllerToHostPacket<'_>) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Controller Busy response changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("response is a complete Command Complete");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("response contains standard status");
        assert_eq!(complete.cmd_opcode, LE_RECEIVER_TEST_V1_OPCODE);
        assert_eq!(
            complete.status,
            bt_hci::param::Error::CONTROLLER_BUSY.to_status()
        );
        assert!(complete.return_param_bytes.is_empty());
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
