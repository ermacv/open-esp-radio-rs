use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{
        Cmd, Opcode, OpcodeGroup, SyncCmd,
        controller_baseband::{
            HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask, SetEventMaskPage2,
        },
        info::ReadBdAddr,
        le::{
            LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
            LeSetAdvEnable, LeSetEventMask, LeSetRandomAddr, LeSetScanEnable, LeSetScanParams,
        },
    },
    controller::{Controller, ExternalController},
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{
        BdAddr, ControllerToHostFlowControl, Error as HciError, EventMask, EventMaskPage2,
        LeEventMask, Status,
    },
    transport::{PacketToController, Transport},
};
use embassy_futures::{
    block_on,
    join::{join, join3},
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use trouble_host::{BleHostError, Error as TroubleError, HostResources, Packet, PacketPool};

use crate::{
    HciCommandPacket, HostToControllerFrame, InProcessHciChannel, InProcessHciControllerEndpoint,
    InProcessHciHostTransport, LeControllerCommandClassification, classify_le_controller_command,
};

use super::{
    BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapConfigError, BootstrapHostBuffers,
    BootstrapPhase, LeControllerBootstrap, LeControllerBootstrapConfig, OwnedBootstrapCommand,
    command_error,
};

type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 32>;
type TestHost<'channel> = InProcessHciHostTransport<'channel, NoopRawMutex, 1, 1, 32>;
type TestController<'channel> = InProcessHciControllerEndpoint<'channel, NoopRawMutex, 1, 1, 32>;

struct TestPacket([u8; 64]);

impl AsRef<[u8]> for TestPacket {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for TestPacket {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Packet for TestPacket {}

struct TestPacketPool;

impl PacketPool for TestPacketPool {
    type Packet = TestPacket;

    const MTU: usize = 64;

    fn allocate() -> Option<Self::Packet> {
        Some(TestPacket([0; 64]))
    }

    fn capacity() -> usize {
        2
    }
}

#[test]
fn trouble_no_security_bootstrap_and_conservative_extensions_are_supported() {
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let config = LeControllerBootstrapConfig::new(public_address, 251, 4).unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let random_address = BdAddr::new([0xc6, 5, 4, 3, 2, 1]);
    let event_mask = EventMask::new()
        .enable_le_meta(true)
        .enable_hardware_error(true)
        .enable_disconnection_complete(true);
    let le_event_mask = LeEventMask::new()
        .enable_le_conn_complete(true)
        .enable_le_adv_report(true)
        .enable_le_conn_update_complete(true);

    block_on(async {
        assert_success(
            round_trip(&host, &controller, &mut bootstrap, &Reset::new()).await,
            &[],
        );
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeSetRandomAddr::new(random_address),
            )
            .await,
            &[],
        );
        assert_eq!(bootstrap.requested_random_address(), Some(random_address));

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &SetEventMask::new(event_mask),
            )
            .await,
            &[],
        );
        assert_eq!(bootstrap.event_mask(), event_mask);

        let unsupported_page_2 = round_trip(
            &host,
            &controller,
            &mut bootstrap,
            &SetEventMaskPage2::new(EventMaskPage2::new().enable_encryption_change_v2(true)),
        )
        .await;
        assert_eq!(unsupported_page_2.status, HciError::UNKNOWN_CMD.to_status());

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeSetEventMask::new(le_event_mask),
            )
            .await,
            &[],
        );
        assert_eq!(bootstrap.le_event_mask(), le_event_mask);

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeReadFilterAcceptListSize::new(),
            )
            .await,
            &[0],
        );
        assert_success(
            round_trip(&host, &controller, &mut bootstrap, &LeReadBufferSize::new()).await,
            &[251, 0, 4],
        );

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &HostBufferSize::new(255, 0, 1, 0),
            )
            .await,
            &[],
        );
        assert_eq!(
            bootstrap.host_buffers(),
            Some(BootstrapHostBuffers {
                acl_data_packet_length: 255,
                total_acl_data_packets: 1,
            })
        );

        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &SetControllerToHostFlowControl::new(ControllerToHostFlowControl::AclOnSyncOff),
            )
            .await,
            &[],
        );
        assert_eq!(
            bootstrap.controller_to_host_flow_control(),
            ControllerToHostFlowControl::AclOnSyncOff
        );

        assert_success(
            round_trip(&host, &controller, &mut bootstrap, &ReadBdAddr::new()).await,
            &[6, 5, 4, 3, 2, 1],
        );
        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeReadLocalSupportedFeatures::new(),
            )
            .await,
            &[0; 8],
        );

        let advertising = round_trip(
            &host,
            &controller,
            &mut bootstrap,
            &LeSetAdvEnable::new(true),
        )
        .await;
        assert_eq!(advertising.status, HciError::UNKNOWN_CMD.to_status());

        assert_success(
            round_trip(&host, &controller, &mut bootstrap, &Reset::new()).await,
            &[],
        );
        assert_eq!(bootstrap.event_mask(), EventMask::new());
        assert_eq!(bootstrap.le_event_mask(), LeEventMask::new());
        assert_eq!(bootstrap.requested_random_address(), None);
        assert_eq!(bootstrap.host_buffers(), None);
        assert_eq!(
            bootstrap.controller_to_host_flow_control(),
            ControllerToHostFlowControl::Off
        );
    });
}

#[test]
fn read_bd_addr_converts_canonical_identity_at_the_hci_boundary() {
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let config = LeControllerBootstrapConfig::new(public_address, 27, 1).unwrap();
    assert_eq!(
        config.public_address().canonical_bytes(),
        [1, 2, 3, 4, 5, 6]
    );

    let mut bootstrap = LeControllerBootstrap::new(config);
    assert_eq!(
        bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    let response = bootstrap.dispatch_owned(OwnedBootstrapCommand::ReadBdAddr);
    assert_eq!(response.status(), Status::SUCCESS);
    assert_eq!(&response.as_bytes()[6..], &[6, 5, 4, 3, 2, 1]);
}

#[test]
fn active_radio_rejects_random_address_without_replacing_epoch_state() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        27,
        1,
    )
    .unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let retained = BdAddr::new([0xc6, 5, 4, 3, 2, 1]);
    let rejected = BdAddr::new([0xc7, 6, 5, 4, 3, 2]);
    assert_eq!(
        bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        bootstrap
            .dispatch_owned(OwnedBootstrapCommand::LeSetRandomAddress(retained))
            .status(),
        Status::SUCCESS
    );

    let response = bootstrap
        .dispatch_owned_while_radio_active(OwnedBootstrapCommand::LeSetRandomAddress(rejected));
    assert_eq!(response.status(), HciError::CMD_DISALLOWED.to_status());
    assert_eq!(bootstrap.requested_random_address(), Some(retained));
}

#[test]
fn external_controller_exec_completes_from_bootstrap_dispatch() {
    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
        27,
        1,
    )
    .unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let external = ExternalController::<_, 1>::new(host);

    block_on(async {
        let reset = Reset::new();
        let mut event_buffer = external.alloc_buf().unwrap();
        let worker = async {
            let mut command_buffer = [0; 32];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut command_buffer).await.unwrap()
            else {
                panic!("Reset changed packet kind");
            };
            let response = dispatch_test_packet(&mut bootstrap, command);
            controller
                .publish(bt_hci::PacketKind::Event, response.as_bytes())
                .await
                .unwrap();
            controller
                .publish(bt_hci::PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
        };

        let (completed, observed, ()) = join3(
            reset.exec(&external),
            external.read(&mut event_buffer),
            worker,
        )
        .await;
        completed.unwrap();
        assert!(matches!(
            observed.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    });
}

#[test]
fn real_trouble_runner_reaches_initialized_over_the_source_owned_hci_boundary() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap();
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let external = ExternalController::<_, 2>::new(host);
    let mut resources = HostResources::<TestPacketPool, 1, 1>::new();
    let stack = trouble_host::new(external, &mut resources).build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let stop = Signal::<NoopRawMutex, ()>::new();

    block_on(async {
        let initialized_probe = async {
            let result = peripheral.set_filter_accept_list(&[]).await;
            stop.signal(());
            result
        };

        // This public Trouble operation cannot emit its command until the
        // Runner has completed its initial ACL/mask bootstrap and published
        // the internal initialized state. The conservative bootstrap then
        // rejects the operational command because no filter-list owner or
        // Link Layer exists yet.
        let controller_and_probe = join(
            drive_bootstrap_until(&controller, &mut bootstrap, &stop),
            initialized_probe,
        );
        match select(runner.run(), controller_and_probe).await {
            Either::First(result) => {
                panic!("Trouble Runner stopped during bootstrap: {result:?}")
            }
            Either::Second(((), probe_result)) => {
                assert!(matches!(
                    probe_result,
                    Err(BleHostError::BleHost(TroubleError::Hci(
                        HciError::UNKNOWN_CMD
                    )))
                ));
            }
        }
    });

    assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    assert_eq!(
        bootstrap.host_buffers(),
        Some(BootstrapHostBuffers {
            acl_data_packet_length: 255,
            total_acl_data_packets: 1,
        })
    );
}

async fn drive_bootstrap_until(
    controller: &TestController<'_>,
    bootstrap: &mut LeControllerBootstrap,
    stop: &Signal<NoopRawMutex, ()>,
) {
    let mut command_buffer = [0; 32];
    loop {
        let command = match select(stop.wait(), controller.receive(&mut command_buffer)).await {
            Either::First(()) => return,
            Either::Second(Ok(HostToControllerFrame::Command(command))) => command,
            Either::Second(Ok(frame)) => {
                panic!("bootstrap received unsupported {:?} packet", frame.kind())
            }
            Either::Second(Err(error)) => panic!("bootstrap receive failed: {error:?}"),
        };
        let response = dispatch_test_packet(bootstrap, command);
        controller
            .publish(bt_hci::PacketKind::Event, response.as_bytes())
            .await
            .expect("bootstrap response enters the raw Controller endpoint");
    }
}

#[test]
fn known_commands_are_disallowed_before_reset_and_malformed_input_never_mutates() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
        27,
        1,
    )
    .unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);

    let before_reset =
        bootstrap.dispatch_owned(OwnedBootstrapCommand::SetEventMask(EventMask::new()));
    assert_eq!(before_reset.status(), HciError::CMD_DISALLOWED.to_status());
    assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

    let malformed_reset = dispatch_test_packet(
        &mut bootstrap,
        HciCommandPacket::for_test(Reset::OPCODE, &[0]),
    );
    assert_eq!(
        malformed_reset.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

    assert_eq!(
        bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    let malformed_mask = dispatch_test_packet(
        &mut bootstrap,
        HciCommandPacket::for_test(SetEventMask::OPCODE, &[0; 7]),
    );
    assert_eq!(
        malformed_mask.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.event_mask(), EventMask::new());

    let sync_host_buffers = [0xff, 0x00, 1, 1, 0, 1, 0];
    assert_eq!(
        dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::for_test(HostBufferSize::OPCODE, &sync_host_buffers),
        )
        .status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.host_buffers(), None);

    assert_eq!(
        dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::for_test(SetControllerToHostFlowControl::OPCODE, &[2]),
        )
        .status(),
        HciError::UNSUPPORTED.to_status()
    );
    assert_eq!(
        bootstrap.controller_to_host_flow_control(),
        ControllerToHostFlowControl::Off
    );

    let unknown = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 1);
    assert_eq!(
        dispatch_test_packet(&mut bootstrap, HciCommandPacket::for_test(unknown, &[]),).status(),
        HciError::UNKNOWN_CMD.to_status()
    );
}

#[test]
fn capability_table_excludes_link_layer_and_optional_page_two_commands() {
    for opcode in [
        Reset::OPCODE,
        SetEventMask::OPCODE,
        SetControllerToHostFlowControl::OPCODE,
        HostBufferSize::OPCODE,
        ReadBdAddr::OPCODE,
        LeSetEventMask::OPCODE,
        LeReadBufferSize::OPCODE,
        LeReadLocalSupportedFeatures::OPCODE,
        LeSetRandomAddr::OPCODE,
        LeReadFilterAcceptListSize::OPCODE,
    ] {
        assert!(BootstrapCommand::supports(opcode));
    }
    assert!(!BootstrapCommand::supports(SetEventMaskPage2::OPCODE));
    assert!(!BootstrapCommand::supports(LeSetAdvEnable::OPCODE));
}

#[test]
fn bootstrap_config_rejects_profiles_without_acl_capacity() {
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            0,
            1,
        ),
        Err(BootstrapConfigError::ZeroAclDataPacketLength)
    );
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            27,
            0,
        ),
        Err(BootstrapConfigError::ZeroAclDataPacketCount)
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObservedCommandComplete {
    opcode: Opcode,
    status: Status,
    parameters: [u8; 8],
    parameter_length: usize,
}

impl ObservedCommandComplete {
    fn parameters(&self) -> &[u8] {
        &self.parameters[..self.parameter_length]
    }
}

async fn round_trip<T: PacketToController>(
    host: &TestHost<'_>,
    controller: &TestController<'_>,
    bootstrap: &mut LeControllerBootstrap,
    command: &T,
) -> ObservedCommandComplete {
    host.write(command).await.unwrap();
    let mut command_buffer = [0; 32];
    let HostToControllerFrame::Command(command) =
        controller.receive(&mut command_buffer).await.unwrap()
    else {
        panic!("bootstrap command changed packet kind");
    };
    let response = dispatch_test_packet(bootstrap, command);
    controller
        .publish(bt_hci::PacketKind::Event, response.as_bytes())
        .await
        .unwrap();

    let mut event_buffer = [0; 32];
    let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap() else {
        panic!("Command Complete changed packet kind");
    };
    assert_eq!(event.kind, EventKind::CommandComplete);
    let event = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
    let event: CommandCompleteWithStatus<'_> = event.try_into().unwrap();
    let mut parameters = [0; 8];
    parameters[..event.return_param_bytes.len()].copy_from_slice(&event.return_param_bytes);
    ObservedCommandComplete {
        opcode: event.cmd_opcode,
        status: event.status,
        parameters,
        parameter_length: event.return_param_bytes.len(),
    }
}

fn assert_success(observed: ObservedCommandComplete, parameters: &[u8]) {
    assert_eq!(observed.status, Status::SUCCESS);
    assert_eq!(observed.parameters(), parameters);
}

fn dispatch_test_packet(
    bootstrap: &mut LeControllerBootstrap,
    command: HciCommandPacket<'_>,
) -> super::BootstrapCommandCompleteEvent {
    match classify_le_controller_command(command) {
        LeControllerCommandClassification::Bootstrap(command) => bootstrap.dispatch_owned(command),
        LeControllerCommandClassification::MalformedBootstrap(response) => response,
        LeControllerCommandClassification::Dtm(command) => {
            command_error(command.kind().opcode(), HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::MalformedDtm(response) => {
            command_error(response.opcode(), HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(command) => {
            command_error(command.kind().opcode(), HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::LegacyAdvertisingEnable(_) => {
            command_error(LeSetAdvEnable::OPCODE, HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::MalformedLegacyAdvertising(response) => {
            command_error(response.opcode(), HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::LegacyScanningConfiguration(_) => {
            command_error(LeSetScanParams::OPCODE, HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::LegacyScanningEnable(_) => {
            command_error(LeSetScanEnable::OPCODE, HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::MalformedLegacyScanning(response) => {
            command_error(response.opcode(), HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::Unsupported(response) => {
            command_error(response.opcode(), HciError::UNKNOWN_CMD)
        }
    }
}
