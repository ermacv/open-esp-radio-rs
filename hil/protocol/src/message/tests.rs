use super::*;

#[test]
fn unknown_boot_allows_only_session_free_capability_discovery() {
    let mut discovery = Envelope::new(0, 1, 0, 1, Command::GetCapabilities);
    assert_eq!(discovery.validate_target(42), Ok(()));
    discovery.session_id = 9;
    assert_eq!(discovery.validate_target(42), Err(RejectReason::BootId));
    discovery.session_id = 0;
    discovery.protocol_version = PROTOCOL_VERSION + 1;
    assert_eq!(
        discovery.validate_target(42),
        Err(RejectReason::ProtocolVersion)
    );
    discovery.protocol_version = PROTOCOL_VERSION;
    for command in [
        Command::StopStation,
        Command::GetStatus,
        Command::QueryStackUsage,
        Command::AcknowledgeResult,
    ] {
        let mut request = Envelope::new(0, 1, 0, 1, command);
        assert_eq!(request.validate_target(42), Err(RejectReason::BootId));
        request.boot_id = 41;
        assert_eq!(request.validate_target(42), Err(RejectReason::BootId));
        request.boot_id = 42;
        assert_eq!(request.validate_target(42), Ok(()));
    }
    assert_eq!(discovery.validate_target(0), Err(RejectReason::BootId));
}

fn flow(flow_id: u8, address: [u8; 4], port: u16) -> SessionFlowConfig {
    let traffic = FlowConfig {
        payload_bytes: 1_472,
        offered_rate_bps: Some(60_000_000),
        pacing_group_datagrams: None,
    };
    SessionFlowConfig {
        flow_id,
        peer: Some(Ipv4Endpoint { address, port }),
        target_rx: Some(traffic),
        target_tx: Some(traffic),
    }
}

fn two_flow_session() -> SessionConfig {
    SessionConfig {
        network_interface: WifiNetworkInterface::AccessPoint,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(10_000),
        flows: [
            Some(flow(3, [192, 168, 4, 2], 9_002)),
            Some(flow(9, [192, 168, 4, 3], 9_003)),
        ],
        link_requirements: SessionLinkRequirements::NONE,
    }
}

#[test]
fn multi_flow_structure_requires_the_explicit_capability() {
    let session = two_flow_session();
    assert!(!session.structurally_valid(1_472, false));
    assert!(session.structurally_valid(1_472, true));
}

#[test]
fn per_flow_pacing_is_nonzero_and_owned_only_by_udp_target_tx() {
    let mut session = two_flow_session();
    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(2);
    assert!(session.structurally_valid(1_472, true));

    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(0);
    assert!(!session.structurally_valid(1_472, true));

    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = None;
    session.flows[1]
        .as_mut()
        .unwrap()
        .target_rx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(2);
    assert!(!session.structurally_valid(1_472, true));
}

#[test]
fn multi_flow_structure_rejects_ambiguous_identities() {
    let mut duplicate_id = two_flow_session();
    duplicate_id.flows[1].as_mut().unwrap().flow_id = 3;
    assert!(!duplicate_id.structurally_valid(1_472, true));

    let mut duplicate_peer = two_flow_session();
    duplicate_peer.flows[1].as_mut().unwrap().peer = duplicate_peer.flows[0].unwrap().peer;
    assert!(!duplicate_peer.structurally_valid(1_472, true));
}

#[test]
fn multi_flow_structure_rejects_missing_peer_or_direction() {
    let mut missing_peer = two_flow_session();
    missing_peer.flows[1].as_mut().unwrap().peer = None;
    assert!(!missing_peer.structurally_valid(1_472, true));

    let mut missing_rx = two_flow_session();
    missing_rx.flows[1].as_mut().unwrap().target_rx = None;
    assert!(!missing_rx.structurally_valid(1_472, true));
}
