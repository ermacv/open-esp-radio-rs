use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{
        Cmd, Opcode, OpcodeGroup, SyncCmd,
        controller_baseband::{
            HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask, SetEventMaskPage2,
        },
        info::{ReadBdAddr, ReadLocalSupportedCmds},
        le::{
            LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
            LeSetAdvEnable, LeSetEventMask, LeSetRandomAddr, LeSetScanEnable, LeSetScanParams,
        },
    },
    controller::{Controller, ExternalController},
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{
        BdAddr, CmdMask, ControllerToHostFlowControl, Error as HciError, EventMask, EventMaskPage2,
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
    HciCommandPacket, HostToControllerFrame, InProcessHciChannel, InProcessHciControllerTransport,
    InProcessHciHostTransport, LeControllerCommandClassification, classify_le_controller_command,
};

use super::state::{default_event_mask, default_le_event_mask};
use super::{
    BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapConfigError, BootstrapHostBuffers,
    BootstrapPhase, LeControllerBootstrap, LeControllerBootstrapConfig, OwnedBootstrapCommand,
    command_error,
};

type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 80>;
type TestHost<'channel> = InProcessHciHostTransport<'channel, NoopRawMutex, 1, 1, 80>;
type TestController<'channel> = InProcessHciControllerTransport<'channel, NoopRawMutex, 1, 1, 80>;

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
                &ReadLocalSupportedCmds::new(),
            )
            .await,
            &super::le_controller_supported_commands(),
        );
        assert_success(
            round_trip(
                &host,
                &controller,
                &mut bootstrap,
                &LeReadLocalSupportedFeatures::new(),
            )
            .await,
            &[(1 << 0) | (1 << 3) | (1 << 4), 1 << 6, 0, 0, 0, 0, 0, 0],
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
        // Reset restores the specification defaults.
        assert_eq!(bootstrap.event_mask(), default_event_mask());
        assert!(!bootstrap.event_mask().is_le_meta_enabled());
        assert!(bootstrap.event_mask().is_disconnection_complete_enabled());
        assert_eq!(bootstrap.le_event_mask(), default_le_event_mask());
        assert!(bootstrap.le_event_mask().is_le_adv_report_enabled());
        assert!(
            bootstrap
                .le_event_mask()
                .is_le_long_term_key_request_enabled()
        );
        assert!(
            !bootstrap
                .le_event_mask()
                .is_le_phy_update_complete_enabled()
        );
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
            .dispatch(OwnedBootstrapCommand::Reset, false)
            .status(),
        Status::SUCCESS
    );
    let response = bootstrap.dispatch(OwnedBootstrapCommand::ReadBdAddr, false);
    assert_eq!(response.status(), Status::SUCCESS);
    assert_eq!(&response.as_bytes()[6..], &[6, 5, 4, 3, 2, 1]);
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
            let mut command_buffer = [0; 80];
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
    // Trouble's security feature requests entropy during Runner startup;
    // workspace feature unification can enable it for this test too.
    // Deterministic entropy is a host-test input, never a production RNG.
    let entropy = BootstrapTestEntropy(core::sync::atomic::AtomicUsize::new(0));
    let mut hci = crate::LeControllerHciResources::<NoopRawMutex, 4, 1, 255>::new(config).unwrap();
    let crate::LeControllerHciEndpoints { host, controller } = hci.split();
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
            drive_bootstrap_until(&controller, &mut bootstrap, &entropy, &stop),
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

struct BootstrapTestEntropy(core::sync::atomic::AtomicUsize);

impl crate::LeRandomSource for BootstrapTestEntropy {
    fn random_bytes(&self) -> Result<[u8; 8], crate::LeRandomUnavailable> {
        let sequence = self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
        Ok((sequence as u64).to_le_bytes())
    }
}

/// A minimal bootstrap-only Controller over the raw transport.
async fn drive_bootstrap_until(
    controller: &InProcessHciControllerTransport<'_, NoopRawMutex, 4, 1, 255>,
    bootstrap: &mut LeControllerBootstrap,
    entropy: &BootstrapTestEntropy,
    stop: &Signal<NoopRawMutex, ()>,
) {
    use crate::LeRandomSource;
    let mut buffer = [0; 255];
    loop {
        match select(stop.wait(), controller.wait_receive_ready()).await {
            Either::First(()) => return,
            Either::Second(()) => {}
        }
        let Ok(HostToControllerFrame::Command(command)) = controller.try_receive(&mut buffer)
        else {
            panic!("Trouble bootstrap must submit an HCI command");
        };
        let response = match classify_le_controller_command(command) {
            LeControllerCommandClassification::Bootstrap(command) => {
                let response = bootstrap.dispatch(command, true);
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            LeControllerCommandClassification::Random(_) => {
                let response =
                    crate::LeRandCommandCompleteEvent::success(entropy.random_bytes().unwrap());
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            LeControllerCommandClassification::Unsupported(response) => {
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            _ => panic!("bootstrap must not start a radio role"),
        };
        response.unwrap();
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
        bootstrap.dispatch(OwnedBootstrapCommand::SetEventMask(EventMask::new()), false);
    assert_eq!(before_reset.status(), HciError::CMD_DISALLOWED.to_status());
    assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

    let malformed_reset =
        dispatch_test_packet(&mut bootstrap, HciCommandPacket::new(Reset::OPCODE, &[0]));
    assert_eq!(
        malformed_reset.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::Reset, false)
            .status(),
        Status::SUCCESS
    );
    let malformed_mask = dispatch_test_packet(
        &mut bootstrap,
        HciCommandPacket::new(SetEventMask::OPCODE, &[0; 7]),
    );
    assert_eq!(
        malformed_mask.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.event_mask(), default_event_mask());

    let sync_host_buffers = [0xff, 0x00, 1, 1, 0, 1, 0];
    assert_eq!(
        dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::new(HostBufferSize::OPCODE, &sync_host_buffers),
        )
        .status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.host_buffers(), None);

    assert_eq!(
        dispatch_test_packet(
            &mut bootstrap,
            HciCommandPacket::new(SetControllerToHostFlowControl::OPCODE, &[2]),
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
        dispatch_test_packet(&mut bootstrap, HciCommandPacket::new(unknown, &[]),).status(),
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
        ReadLocalSupportedCmds::OPCODE,
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
fn supported_commands_report_matches_the_closed_operational_inventory() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
        251,
        4,
    )
    .unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);
    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::Reset, false)
            .status(),
        Status::SUCCESS
    );

    let response = bootstrap.dispatch(OwnedBootstrapCommand::ReadLocalSupportedCommands, false);
    assert_eq!(response.opcode(), ReadLocalSupportedCmds::OPCODE);
    assert_eq!(response.status(), Status::SUCCESS);
    assert_eq!(response.as_bytes().len(), 70);
    let mask = <&CmdMask>::from_hci_bytes_complete(&response.as_bytes()[6..]).unwrap();

    assert!(mask.disconnect());
    assert!(mask.read_remote_version_information());
    assert!(mask.set_event_mask());
    assert!(mask.reset());
    assert!(mask.set_controller_to_host_flow_control());
    assert!(mask.host_buffer_size());
    assert!(mask.host_number_of_completed_packets());
    assert!(mask.read_bd_addr());
    assert!(mask.le_set_event_mask_v1());
    assert!(mask.le_read_buffer_size_v1());
    assert!(mask.le_read_local_supported_features());
    assert!(mask.le_set_random_addr());
    assert!(mask.le_set_adv_parameters());
    assert!(mask.le_set_adv_data());
    assert!(mask.le_set_scan_response_data());
    assert!(mask.le_set_adv_enable());
    assert!(mask.le_set_scan_parameters());
    assert!(mask.le_set_scan_enable());
    assert!(mask.le_read_filter_accept_list_size());
    assert!(mask.le_read_remote_features());
    assert!(mask.le_long_term_key_request_reply());
    assert!(mask.le_long_term_key_request_negative_reply());
    assert!(mask.le_receiver_test_v1());
    assert!(mask.le_transmitter_test_v1());
    assert!(mask.le_test_end());
    assert!(mask.le_receiver_test_v2());
    assert!(mask.le_transmitter_test_v2());

    assert!(!mask.inquiry());
    assert!(!mask.read_local_supported_features());
    assert!(!mask.le_read_adv_physical_channel_tx_power());
    assert!(!mask.le_create_conn());
    assert!(!mask.le_encrypt());
    assert!(!mask.le_read_phy());
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
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            252,
            1,
        ),
        Err(BootstrapConfigError::AclDataPacketLengthTooLarge {
            length: 252,
            maximum: 251,
        })
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObservedCommandComplete {
    opcode: Opcode,
    status: Status,
    parameters: [u8; 64],
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
    let mut command_buffer = [0; 80];
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

    let mut event_buffer = [0; 80];
    let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap() else {
        panic!("Command Complete changed packet kind");
    };
    assert_eq!(event.kind, EventKind::CommandComplete);
    let event = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
    let event: CommandCompleteWithStatus<'_> = event.try_into().unwrap();
    let mut parameters = [0; 64];
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
        LeControllerCommandClassification::Random(_)
        | LeControllerCommandClassification::MalformedRandom(_) => {
            panic!("LE Rand is a platform service, not software bootstrap")
        }
        LeControllerCommandClassification::Disconnect(_)
        | LeControllerCommandClassification::MalformedDisconnect(_)
        | LeControllerCommandClassification::ReadRemoteFeatures(_)
        | LeControllerCommandClassification::MalformedReadRemoteFeatures(_)
        | LeControllerCommandClassification::ReadRemoteVersionInformation(_)
        | LeControllerCommandClassification::MalformedReadRemoteVersionInformation(_)
        | LeControllerCommandClassification::LongTermKeyReply(_)
        | LeControllerCommandClassification::LongTermKeyNegativeReply(_)
        | LeControllerCommandClassification::MalformedLongTermKeyReply(_) => {
            command_error(crate::LeDisconnectCommand::OPCODE, HciError::UNKNOWN_CMD)
        }
        LeControllerCommandClassification::Bootstrap(command) => bootstrap.dispatch(command, false),
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
