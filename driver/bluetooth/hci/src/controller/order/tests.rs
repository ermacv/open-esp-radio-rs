use bt_hci::{
    ControllerToHostPacket, FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode, OpcodeGroup,
        controller_baseband::{Reset, SetEventMask},
        le::{
            LeReceiverTest, LeReceiverTestV2, LeSetAdvData, LeSetAdvEnable, LeSetAdvParams,
            LeSetRandomAddr, LeSetScanEnable, LeSetScanParams, LeSetScanResponseData, LeTestEnd,
            LeTransmitterTestV2,
        },
    },
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{
        AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration, Error as HciError,
        LeScanKind, ScanningFilterPolicy, Status,
    },
    transport::{PacketToController, Transport},
};
use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{
    LeControllerActiveDtmCommandRoute, LeControllerActiveLegacyAdvertisingCommandRoute,
    LeControllerActiveLegacyScanningCommandRoute, LeControllerClassifiedCommand,
    LeControllerCommandIntake, LeControllerCommandReady, LeControllerIdleClassifiedCommandRoute,
    LeControllerResetCompletion, LeControllerResponsePending, LeControllerResponsePublication,
};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, LE_RECEIVER_TEST_V1_OPCODE,
    LE_RECEIVER_TEST_V2_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE, LeControllerBootstrapConfig,
    LeControllerClassifiedCommandRoute, LeControllerCommandEndpoint, LeControllerCommandReadyClaim,
    LeControllerHciEndpoints, LeControllerHciResources, LeDtmModulationIndex, LeDtmPhy,
    LeLegacyAdvertisingAddress, LeLegacyAdvertisingRole,
};

#[derive(Debug, Eq, PartialEq)]
struct RadioOwner(u32);

#[derive(Debug, Eq, PartialEq)]
struct QuiescedOwner(u32);

type ControllerResources = LeControllerHciResources<NoopRawMutex, 1, 1, 45>;

fn controller_resources() -> ControllerResources {
    controller_resources_with_output_depth()
}

fn controller_resources_with_output_depth<const CONTROLLER_TO_HOST_DEPTH: usize>()
-> LeControllerHciResources<NoopRawMutex, 1, CONTROLLER_TO_HOST_DEPTH, 45> {
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
    let LeControllerCommandReadyClaim::Ready(ready) = controller.claim_initial_command_ready(owner)
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
    endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, CONTROLLER_TO_HOST_DEPTH, 45>,
    owner: Owner,
) -> LeControllerResponsePending<'epoch, Owner> {
    block_on(endpoints.host.write(&LeReceiverTest::new(7)))
        .expect("the receiver command enters the real Host queue");
    let mut command_buffer = [0; 45];
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
    endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, CONTROLLER_TO_HOST_DEPTH, 45>,
    owner: Owner,
) -> LeControllerCommandReady<'epoch, Owner> {
    let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 0x1f);
    block_on(endpoints.host.write(&RawCommand::new(opcode, &[])))
        .expect("the probe command enters the Host queue");
    let ready = claim_initial_ready(&mut endpoints.controller, owner);
    let mut command_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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

    let mut buffer = [0; 45];
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

    let mut buffer = [0; 45];
    let LeControllerCommandIntake::Empty {
        ready,
        buffer: returned,
    } = endpoints
        .controller
        .try_receive_classified_command_with_buffer(ready, &mut buffer)
    else {
        panic!("an empty queue returns exact authority and scratch storage");
    };
    assert_eq!(returned.len(), 45);

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
                required: 45,
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

    let LeControllerResponsePublication::Published(ready) = pending.try_publish(&first.controller)
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
    let mut command_buffer = [0; 45];
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

    let mut buffer = [0; 45];
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
    block_on(endpoints.host.write(&LeTestEnd::new())).expect("Test End enters the real Host queue");
    let mut command_buffer = [0; 45];
    let classified = intake_command(&endpoints.controller, started, &mut command_buffer);
    let LeControllerClassifiedCommandRoute::Dtm(deferred) =
        endpoints.controller.route_classified_command(classified)
    else {
        panic!("Test End remains in the portable DTM order aggregate");
    };
    let LeControllerActiveDtmCommandRoute::TestEnd(ending) = deferred.into_active_session_route()
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

    let mut buffer = [0; 45];
    assert_start_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
    assert_test_end_response(block_on(endpoints.host.read(&mut buffer)).unwrap());
}

#[test]
fn idle_router_defers_both_start_kinds_until_explicit_started_response() {
    let mut receiver_resources = controller_resources();
    let mut receiver = receiver_resources.split();
    block_on(receiver.host.write(&LeReceiverTestV2::new(13, 2, 1)))
        .expect("the receiver command enters the real Host queue");
    let mut receiver_buffer = [0; 45];
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
    let mut response_buffer = [0; 45];
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
    let mut transmitter_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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
    let mut response_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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

    let mut response_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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
    let mut response_buffer = [0; 45];
    assert_probe_response(
        block_on(endpoints.host.read(&mut response_buffer)).expect("the older response is drained"),
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
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(
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
    let mut command_buffer = [0; 45];
    let mut response_buffer = [0; 45];

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
    let LeControllerIdleClassifiedCommandRoute::StartLegacyNonconnectableAdvertising(start) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("Enable must remain deferred until hardware starts");
    };
    assert_eq!(start.owner(), &RadioOwner(101));
    assert_eq!(
        start.request().advertiser(),
        LeLegacyAdvertisingAddress::Public(BluetoothPublicDeviceAddress::from_canonical_bytes([
            2, 3, 5, 7, 11, 13
        ]))
    );
    assert!(start.request().data().is_empty());
    assert_eq!(
        start.request().parameters().role(),
        LeLegacyAdvertisingRole::Nonconnectable
    );
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
fn connectable_advertising_start_has_distinct_type_and_ordered_scan_response() {
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(103));
    let mut command_buffer = [0; 45];
    let mut response_buffer = [0; 45];

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

    let parameters = LeSetAdvParams::new(
        Duration::from_u16(0x20),
        Duration::from_u16(0x40),
        AdvKind::AdvInd,
        AddrKind::PUBLIC,
        AddrKind::PUBLIC,
        BdAddr::default(),
        AdvChannelMap::ALL,
        AdvFilterPolicy::Unfiltered,
    );
    block_on(endpoints.host.write(&parameters))
        .expect("connectable parameters enter the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("connectable parameters complete in software");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("parameter completion publishes");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvParams::OPCODE,
        Status::SUCCESS,
    );

    let mut scan_response = [0; 31];
    scan_response[..4].copy_from_slice(&[3, 3, 0xaa, 0xfe]);
    block_on(
        endpoints
            .host
            .write(&LeSetScanResponseData::new(4, scan_response)),
    )
    .expect("scan-response data enter the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("scan-response configuration completes in software");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("scan-response completion publishes");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanResponseData::OPCODE,
        Status::SUCCESS,
    );

    block_on(endpoints.host.write(&LeSetAdvEnable::new(true)))
        .expect("Enable enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerIdleClassifiedCommandRoute::StartLegacyConnectableAdvertising(start) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("ADV_IND must not enter the nonconnectable deferred start");
    };

    assert_eq!(start.owner(), &RadioOwner(103));
    assert_eq!(
        start.request().parameters().role(),
        LeLegacyAdvertisingRole::Connectable
    );
    assert_eq!(
        start.request().scan_response_data().as_bytes(),
        &[3, 3, 0xaa, 0xfe]
    );
    assert_eq!(
        start.request().advertiser(),
        LeLegacyAdvertisingAddress::Public(BluetoothPublicDeviceAddress::from_canonical_bytes([
            2, 3, 5, 7, 11, 13
        ]))
    );

    let pending = start.into_hardware_failure_response();
    assert_eq!(pending.owner(), &RadioOwner(103));
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("fail-closed connectable response publishes once");
    };
    assert_eq!(ready.owner(), &RadioOwner(103));
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvEnable::OPCODE,
        HciError::HARDWARE_FAILURE.to_status(),
    );
}

#[test]
fn passive_scanner_enable_and_disable_follow_hardware_lifecycle() {
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(111));
    let mut command_buffer = [0; 45];
    let mut response_buffer = [0; 45];

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

    let parameters = LeSetScanParams::new(
        LeScanKind::Passive,
        Duration::from_u16(0x20),
        Duration::from_u16(0x10),
        AddrKind::PUBLIC,
        ScanningFilterPolicy::BasicUnfiltered,
    );
    block_on(endpoints.host.write(&parameters)).expect("Set Scan Parameters enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("accepted scan parameters complete in software");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the parameter response publishes");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanParams::OPCODE,
        Status::SUCCESS,
    );

    block_on(endpoints.host.write(&LeSetScanEnable::new(true, true)))
        .expect("Enable enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerIdleClassifiedCommandRoute::StartLegacyScanning(start) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("Enable must remain deferred until scanner hardware starts");
    };
    assert_eq!(start.owner(), &RadioOwner(111));
    assert_eq!(start.request().parameters().interval_units_625_us(), 0x20);
    assert_eq!(start.request().parameters().window_units_625_us(), 0x10);
    assert_eq!(
        start.request().duplicate_policy(),
        crate::LeLegacyScanningDuplicatePolicy::FilterDuplicates
    );
    let pending = start
        .map_owner(|RadioOwner(owner)| RadioOwner(owner + 1))
        .into_started_response();
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the started response publishes exactly once");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanEnable::OPCODE,
        Status::SUCCESS,
    );

    block_on(
        endpoints
            .host
            .write(&LeSetRandomAddr::new(BdAddr::new([0xc6, 5, 4, 3, 2, 1]))),
    )
    .expect("LE Set Random Address enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyScanningCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_scanning_classified_command(command)
    else {
        panic!("active scanner must reject random-address replacement in command order");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the random-address rejection publishes");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetRandomAddr::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    block_on(endpoints.host.write(&LeSetScanEnable::new(true, false)))
        .expect("repeated Enable enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyScanningCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_scanning_classified_command(command)
    else {
        panic!("repeated Enable must not create a second scanner owner");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the repeated Enable response publishes");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanEnable::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    block_on(endpoints.host.write(&LeSetScanEnable::new(false, false)))
        .expect("Disable enters the Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyScanningCommandRoute::Disable(disable) = endpoints
        .controller
        .route_active_legacy_scanning_classified_command(command)
    else {
        panic!("Disable must remain deferred until scanner quiescence");
    };
    let pending = disable
        .map_owner(|RadioOwner(owner)| RadioOwner(owner + 1))
        .into_stopped_response();
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the stopped response publishes exactly once");
    };
    assert_eq!(ready.owner(), &RadioOwner(113));
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanEnable::OPCODE,
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
        let mut command_buffer = [0; 45];
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
        let mut response_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
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
    let mut buffer = [0; 45];
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
    block_on(endpoints.host.write(&LeTestEnd::new())).expect("Test End enters the real Host queue");
    let mut command_buffer = [0; 45];
    let classified = intake_command(&endpoints.controller, started, &mut command_buffer);
    let LeControllerClassifiedCommandRoute::Dtm(deferred) =
        endpoints.controller.route_classified_command(classified)
    else {
        panic!("Test End must remain semantic for the caller's session policy");
    };
    let LeControllerActiveDtmCommandRoute::TestEnd(ending) = deferred.into_active_session_route()
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
    let mut response_buffer = [0; 45];
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
fn active_advertising_disable_retains_command_order_until_quiescence() {
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    block_on(
        endpoints
            .host
            .write(&RawCommand::new(LeSetAdvEnable::OPCODE, &[0])),
    )
    .expect("Disable enters the real Host queue");
    let mut command_buffer = [0; 45];
    let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(107));
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::Disable(disable) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active Disable must remain deferred through hardware quiescence");
    };
    assert_eq!(disable.owner(), &RadioOwner(107));

    let pending = disable
        .map_owner(|RadioOwner(owner)| QuiescedOwner(owner + 1))
        .into_stopped_response();
    assert_eq!(pending.owner(), &QuiescedOwner(108));
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the completed Disable");
    };
    assert_eq!(ready.owner(), &QuiescedOwner(108));
    let mut response_buffer = [0; 45];
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvEnable::OPCODE,
        Status::SUCCESS,
    );
}

#[test]
fn active_advertising_reenable_is_noop_but_configuration_and_dtm_start_are_rejected() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            12,
            1,
        )
        .expect("the test HCI profile is nonzero"),
    )
    .expect("the profile fits its source-owned storage");
    let mut endpoints = resources.split();
    let ready = claim_initial_ready(&mut endpoints.controller, RadioOwner(109));
    let mut command_buffer = [0; 45];
    let mut response_buffer = [0; 45];

    block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active Reset must remain ordered through hardware quiescence");
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

    block_on(
        endpoints
            .host
            .write(&LeSetRandomAddr::new(BdAddr::new([0xc6, 5, 4, 3, 2, 1]))),
    )
    .expect("LE Set Random Address enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active random-address replacement must become an ordered rejection");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the random-address rejection");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetRandomAddr::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    let parameters = [
        0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
    ];
    block_on(
        endpoints
            .host
            .write(&RawCommand::new(LeSetAdvParams::OPCODE, &parameters)),
    )
    .expect("Set Parameters enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active configuration must become an ordered rejection");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the configuration rejection");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvParams::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    let mut data = [0; 31];
    data[..3].copy_from_slice(&[2, 1, 6]);
    block_on(endpoints.host.write(&LeSetAdvData::new(3, data)))
        .expect("Set Data enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active advertising data must become an ordered rejection");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the data rejection");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvData::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    let mut scan_response = [0; 31];
    scan_response[..4].copy_from_slice(&[3, 3, 0xaa, 0xfe]);
    block_on(
        endpoints
            .host
            .write(&LeSetScanResponseData::new(4, scan_response)),
    )
    .expect("Set Scan Response Data enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("active scan-response data must become an ordered rejection");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the scan-response rejection");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetScanResponseData::OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
    );

    block_on(
        endpoints
            .host
            .write(&RawCommand::new(LeSetAdvEnable::OPCODE, &[1])),
    )
    .expect("repeated Enable enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("repeated Enable must become an ordered no-op");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the Enable completion");
    };
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LeSetAdvEnable::OPCODE,
        Status::SUCCESS,
    );

    block_on(endpoints.host.write(&LeReceiverTestV2::new(11, 2, 0)))
        .expect("DTM start enters the real Host queue");
    let command = intake_command(&endpoints.controller, ready, &mut command_buffer);
    let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) = endpoints
        .controller
        .route_active_legacy_advertising_classified_command(command)
    else {
        panic!("DTM start cannot escape while advertising owns the radio");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue accepts the DTM rejection");
    };
    assert_eq!(ready.owner(), &RadioOwner(109));
    assert_command_status(
        block_on(endpoints.host.read(&mut response_buffer)).unwrap(),
        LE_RECEIVER_TEST_V2_OPCODE,
        HciError::CMD_DISALLOWED.to_status(),
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
    let mut command_buffer = [0; 45];
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

    let mut response_buffer = [0; 45];
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
    let LeControllerResponsePublication::Published(started) = start.try_publish(&first.controller)
    else {
        panic!("the first endpoint must publish its start response");
    };
    block_on(first.host.write(&Reset::new())).expect("Reset enters the first Host transport");
    let mut command_buffer = [0; 45];
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
    let LeControllerResponsePublication::Pending(pending) = pending.try_publish(&first.controller)
    else {
        panic!("the queued start response must retain Reset completion");
    };
    assert_eq!(pending.owner(), &QuiescedOwner(72));

    let mut response_buffer = [0; 45];
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
    let mut command_buffer = [0; 45];
    let mut response_buffer = [0; 45];

    block_on(
        endpoints
            .host
            .write(&RawCommand::new(SetEventMask::OPCODE, &[0; 7])),
    )
    .expect("the malformed bootstrap command enters the real Host queue");
    let malformed_bootstrap = intake_command(&endpoints.controller, active, &mut command_buffer);
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
    let LeControllerResponsePublication::Published(started) = start.try_publish(&first.controller)
    else {
        panic!("the first endpoint must publish its start response");
    };

    let mut second_resources = controller_resources();
    let mut second = second_resources.split();
    block_on(second.host.write(&LeTestEnd::new()))
        .expect("the foreign DTM command enters its own Host queue");
    let mut command_buffer = [0; 45];
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
    let LeControllerActiveDtmCommandRoute::TestEnd(test_end) = deferred.into_active_session_route()
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
