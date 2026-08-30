//! Portable ordering authority for LE DTM command responses.
//!
//! Radio progress and Controller-to-Host capacity are deliberately independent:
//! a chip-specific runner may transform the retained radio owner while the exact
//! Command Complete remains pending. Only successful insertion into the matching
//! HCI epoch advances the order state to [`LeDtmResponsePublished`].

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    HciChannelError, HciControllerResponse, HciEpochIdentity, InProcessHciControllerEndpoint,
    LeDtmCommandCompleteEvent,
};

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

    use super::{LeDtmResponsePending, LeDtmResponsePublication, LeDtmResponsePublished};
    use crate::{
        HciChannelError, InProcessHciChannel, LE_RECEIVER_TEST_V1_OPCODE, LE_TEST_END_OPCODE,
        LeDtmCommand, LeReceiverTestV1Command, LeTestEndCommand,
    };

    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

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
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("DTM Test End response changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("response is a complete Command Complete");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("response contains standard status");
        assert_eq!(complete.status, Status::SUCCESS);
        assert_eq!(complete.return_params::<LeTestEnd>().unwrap(), 0x3412);
    }
}
