use bt_hci::{
    cmd::{
        Cmd,
        le::{LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetScanResponseData},
    },
    param::{
        AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration, Error as HciError,
        Status,
    },
};

use super::{
    LEGACY_ADVERTISING_INTERVAL_DEFAULT, LeLegacyAdvertisingCommand,
    LeLegacyAdvertisingCommandKind, LeLegacyAdvertisingConfiguration,
    LeLegacyAdvertisingConfigurationCommand, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingIdleEnableDisposition, LeLegacyAdvertisingOwnAddressKind,
    LeLegacyAdvertisingRole,
};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapPhase, HciCommandPacket, LeLegacyAdvertisingAddress,
};

#[test]
fn decodes_supported_nonconnectable_parameters() {
    let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
        ],
    ))
    .expect("the supported standard parameters decode");

    let LeLegacyAdvertisingCommand::SetParameters(parameters) = command else {
        panic!("parameters changed semantic command kind");
    };
    assert_eq!(parameters.interval().minimum_units_625_us(), 0x20);
    assert_eq!(parameters.interval().maximum_units_625_us(), 0x40);
    assert_eq!(parameters.role(), LeLegacyAdvertisingRole::Nonconnectable);
    assert_eq!(
        parameters.own_address_kind(),
        LeLegacyAdvertisingOwnAddressKind::Random
    );
    assert!(parameters.channels().channel_37());
    assert!(!parameters.channels().channel_38());
    assert!(parameters.channels().channel_39());
}

#[test]
fn advertising_and_scan_response_data_are_owned_and_length_bounded() {
    for opcode in [LeSetAdvData::OPCODE, LeSetScanResponseData::OPCODE] {
        let mut body = [0; 32];
        body[0] = 3;
        body[1..4].copy_from_slice(&[2, 1, 6]);
        let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(opcode, &body))
            .expect("the complete standard data command decodes");
        body.fill(0xff);
        match command {
            LeLegacyAdvertisingCommand::SetData(data) => {
                assert_eq!(opcode, LeSetAdvData::OPCODE);
                assert_eq!(data.as_bytes(), &[2, 1, 6]);
            }
            LeLegacyAdvertisingCommand::SetScanResponseData(data) => {
                assert_eq!(opcode, LeSetScanResponseData::OPCODE);
                assert_eq!(data.as_bytes(), &[2, 1, 6]);
            }
            _ => panic!("data changed semantic command kind"),
        }
    }
}

#[test]
fn rejects_malformed_invalid_and_unsupported_values_with_exact_status() {
    for (opcode, body, expected) in [
        (
            LeSetAdvEnable::OPCODE,
            &[2][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            &[0x20, 0, 0x40, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            &[0x20, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            &[0x20, 0, 0x40, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
    ] {
        let error = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(opcode, body))
            .expect_err("the rejected value cannot become a command token");
        let response = error
            .into_command_complete()
            .expect("the opcode belongs to this command family");
        assert_eq!(response.opcode(), opcode);
        assert_eq!(response.status(), expected);
    }
}

#[test]
fn rejects_every_directed_scannable_only_and_filtered_parameter_profile() {
    for unsupported_adv_kind in [1, 2, 4] {
        let mut body = [0; 15];
        body[..4].copy_from_slice(&[0x20, 0, 0x40, 0]);
        body[4] = unsupported_adv_kind;
        body[13] = 0x07;
        let error = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &body,
        ))
        .expect_err("directed and scannable-only roles remain unsupported");
        assert_eq!(
            error
                .into_command_complete()
                .expect("Set Advertising Parameters owns the rejection")
                .status(),
            HciError::UNSUPPORTED.to_status()
        );
    }

    for unsupported_filter_policy in [1, 2, 3] {
        let mut body = [0; 15];
        body[..4].copy_from_slice(&[0x20, 0, 0x40, 0]);
        body[4] = 0;
        body[13] = 0x07;
        body[14] = unsupported_filter_policy;
        let error = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
            LeSetAdvParams::OPCODE,
            &body,
        ))
        .expect_err("every filtered advertising profile remains unsupported");
        assert_eq!(
            error
                .into_command_complete()
                .expect("Set Advertising Parameters owns the rejection")
                .status(),
            HciError::UNSUPPORTED.to_status()
        );
    }
}

#[test]
fn accepts_standard_bt_hci_field_domains_without_reencoding_them() {
    let command = LeSetAdvParams::new(
        Duration::from_u16(0x20),
        Duration::from_u16(0x40),
        AdvKind::AdvNonconnInd,
        AddrKind::PUBLIC,
        AddrKind::PUBLIC,
        BdAddr::default(),
        AdvChannelMap::ALL,
        AdvFilterPolicy::Unfiltered,
    );
    let _ = command;
    assert_eq!(
        LeSetAdvParams::OPCODE,
        LeLegacyAdvertisingCommandKind::SetParameters.opcode()
    );
}

#[test]
fn configuration_is_reset_scoped_and_rejects_pre_reset_mutation() {
    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ))
    .expect("fixture parameters decode");
    let parameters = LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
        .expect("Set Parameters is software-only configuration");
    let mut configuration = LeLegacyAdvertisingConfiguration::new();
    let reset_defaults = configuration.parameters();
    assert_eq!(reset_defaults.role(), LeLegacyAdvertisingRole::Connectable);
    assert_eq!(
        reset_defaults.own_address_kind(),
        LeLegacyAdvertisingOwnAddressKind::Public
    );
    assert!(reset_defaults.channels().channel_37());
    assert!(reset_defaults.channels().channel_38());
    assert!(reset_defaults.channels().channel_39());

    let rejected = configuration.dispatch(BootstrapPhase::AwaitingReset, parameters);
    assert_eq!(rejected.status(), HciError::CMD_DISALLOWED.to_status());
    assert_eq!(configuration.parameters(), reset_defaults);

    let accepted = configuration.dispatch(BootstrapPhase::Configuring, parameters);
    assert_eq!(accepted.status(), Status::SUCCESS);
    assert_ne!(configuration.parameters(), reset_defaults);

    let mut body = [0; 32];
    body[0] = 3;
    body[1..4].copy_from_slice(&[2, 1, 6]);
    let data =
        LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(LeSetAdvData::OPCODE, &body))
            .expect("fixture data decode");
    let data = LeLegacyAdvertisingConfigurationCommand::from_command(data)
        .expect("Set Data is software-only configuration");
    assert_eq!(
        configuration
            .dispatch(BootstrapPhase::Configuring, data)
            .status(),
        Status::SUCCESS
    );
    assert_eq!(configuration.data().as_bytes(), &[2, 1, 6]);

    let scan_response = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetScanResponseData::OPCODE,
        &body,
    ))
    .expect("fixture scan-response data decode");
    let scan_response = LeLegacyAdvertisingConfigurationCommand::from_command(scan_response)
        .expect("Set Scan Response Data is software-only configuration");
    assert_eq!(
        configuration
            .dispatch(BootstrapPhase::Configuring, scan_response)
            .status(),
        Status::SUCCESS
    );
    assert_eq!(configuration.scan_response_data().as_bytes(), &[2, 1, 6]);

    configuration.reset();
    assert_eq!(configuration.parameters(), reset_defaults);
    assert!(configuration.data().is_empty());
    assert!(configuration.scan_response_data().is_empty());
}

#[test]
fn idle_enable_freezes_parameters_data_and_resolved_public_address() {
    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
        ],
    ))
    .expect("fixture parameters decode");
    let mut configuration = LeLegacyAdvertisingConfiguration::new();
    configuration.dispatch(
        BootstrapPhase::Configuring,
        LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
            .expect("the parameters command is configuration"),
    );

    let mut body = [0; 32];
    body[0] = 3;
    body[1..4].copy_from_slice(&[2, 1, 6]);
    let data =
        LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(LeSetAdvData::OPCODE, &body))
            .expect("fixture data decode");
    configuration.dispatch(
        BootstrapPhase::Configuring,
        LeLegacyAdvertisingConfigurationCommand::from_command(data)
            .expect("the data command is configuration"),
    );

    let enable = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvEnable::OPCODE,
        &[1],
    ))
    .expect("Enable decodes");
    let enable = LeLegacyAdvertisingEnableCommand::from_command(enable)
        .expect("Enable refines into its lifecycle token");
    let LeLegacyAdvertisingIdleEnableDisposition::StartNonconnectable(request) = configuration
        .dispatch_idle_enable(
            BootstrapPhase::Configuring,
            enable,
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            None,
        )
    else {
        panic!("complete configuration must defer a hardware start");
    };
    assert_eq!(request.data().as_bytes(), &[2, 1, 6]);
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Public(BluetoothPublicDeviceAddress::from_canonical_bytes([
            1, 2, 3, 4, 5, 6
        ]))
    );
    assert_eq!(
        request.parameters().role(),
        LeLegacyAdvertisingRole::Nonconnectable
    );
    assert_eq!(request.parameters().interval().minimum_units_625_us(), 0x20);
    assert_eq!(request.parameters().interval().maximum_units_625_us(), 0x40);
    assert!(request.parameters().channels().channel_37());
    assert!(!request.parameters().channels().channel_38());
    assert!(request.parameters().channels().channel_39());
}

#[test]
fn connectable_enable_retains_scan_response_and_distinct_role() {
    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x01, 0x00,
        ],
    ))
    .expect("unfiltered ADV_IND parameters decode");
    let mut configuration = LeLegacyAdvertisingConfiguration::new();
    configuration.dispatch(
        BootstrapPhase::Configuring,
        LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
            .expect("the parameters command is configuration"),
    );

    let mut body = [0; 32];
    body[0] = 4;
    body[1..5].copy_from_slice(&[3, 3, 0xaa, 0xfe]);
    let scan_response = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetScanResponseData::OPCODE,
        &body,
    ))
    .expect("scan-response data decode");
    configuration.dispatch(
        BootstrapPhase::Configuring,
        LeLegacyAdvertisingConfigurationCommand::from_command(scan_response)
            .expect("scan-response data is configuration"),
    );

    let enable =
        LeLegacyAdvertisingEnableCommand::from_command(LeLegacyAdvertisingCommand::SetEnable(true))
            .expect("the fixture is Enable");
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let LeLegacyAdvertisingIdleEnableDisposition::StartConnectable(request) = configuration
        .dispatch_idle_enable(BootstrapPhase::Configuring, enable, public_address, None)
    else {
        panic!("ADV_IND must produce only the connectable start type");
    };

    assert_eq!(
        request.parameters().role(),
        LeLegacyAdvertisingRole::Connectable
    );
    assert!(request.data().is_empty());
    assert_eq!(request.scan_response_data().as_bytes(), &[3, 3, 0xaa, 0xfe]);
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Public(public_address)
    );
}

#[test]
fn idle_enable_uses_reset_defaults_and_requires_a_selected_random_address() {
    let enable =
        LeLegacyAdvertisingEnableCommand::from_command(LeLegacyAdvertisingCommand::SetEnable(true))
            .expect("the fixture is Enable");
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let mut configuration = LeLegacyAdvertisingConfiguration::new();

    let LeLegacyAdvertisingIdleEnableDisposition::Complete(response) = configuration
        .dispatch_idle_enable(BootstrapPhase::AwaitingReset, enable, public_address, None)
    else {
        panic!("Enable before the required Reset must fail closed");
    };
    assert_eq!(response.status(), HciError::CMD_DISALLOWED.to_status());

    let LeLegacyAdvertisingIdleEnableDisposition::StartConnectable(request) = configuration
        .dispatch_idle_enable(BootstrapPhase::Configuring, enable, public_address, None)
    else {
        panic!("the reset defaults must start connectable undirected advertising");
    };
    assert_eq!(
        request.parameters().role(),
        LeLegacyAdvertisingRole::Connectable
    );
    assert_eq!(
        request.parameters().own_address_kind(),
        LeLegacyAdvertisingOwnAddressKind::Public
    );
    assert_eq!(
        request.parameters().interval().minimum_units_625_us(),
        LEGACY_ADVERTISING_INTERVAL_DEFAULT
    );
    assert_eq!(
        request.parameters().interval().maximum_units_625_us(),
        LEGACY_ADVERTISING_INTERVAL_DEFAULT
    );
    assert!(request.parameters().channels().channel_37());
    assert!(request.parameters().channels().channel_38());
    assert!(request.parameters().channels().channel_39());
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Public(public_address)
    );

    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ))
    .expect("random-address parameters decode");
    configuration.dispatch(
        BootstrapPhase::Configuring,
        LeLegacyAdvertisingConfigurationCommand::from_command(parameters)
            .expect("the parameters command is configuration"),
    );
    let LeLegacyAdvertisingIdleEnableDisposition::Complete(response) = configuration
        .dispatch_idle_enable(BootstrapPhase::Configuring, enable, public_address, None)
    else {
        panic!("random advertising cannot start without LE Set Random Address");
    };
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );

    let LeLegacyAdvertisingIdleEnableDisposition::StartNonconnectable(request) = configuration
        .dispatch_idle_enable(
            BootstrapPhase::Configuring,
            enable,
            public_address,
            Some(BdAddr::new([9, 8, 7, 6, 5, 0xc4])),
        )
    else {
        panic!("the accepted random address must complete the start snapshot");
    };
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Random(BdAddr::new([9, 8, 7, 6, 5, 0xc4]))
    );
}
