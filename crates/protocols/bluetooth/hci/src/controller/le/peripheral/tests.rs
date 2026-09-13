use bt_hci::{
    FromHciBytes,
    event::{Event, le::LeEvent},
    param::{
        AddrKind, BdAddr, ClockAccuracy, ConnHandle, Duration, Error as HciError, LeConnRole,
        Status,
    },
};

use super::{
    LeDisconnectionCompleteEvent, LePeripheralConnectionCompleteEvent,
    LePeripheralConnectionCompleteEventError,
};

#[test]
fn peripheral_connection_complete_roundtrips_through_bt_hci() {
    let event = LePeripheralConnectionCompleteEvent::new(
        ConnHandle::new(0x0abc),
        AddrKind::RANDOM,
        BdAddr::new([1, 2, 3, 4, 5, 0xc6]),
        Duration::from_u16(24),
        3,
        Duration::from_u16(200),
        ClockAccuracy::Ppm75,
    )
    .expect("the legacy peer address is representable");

    let Event::Le(LeEvent::LeConnectionComplete(decoded)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the complete standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.role, LeConnRole::Peripheral);
    assert_eq!(decoded.peer_addr_kind, AddrKind::RANDOM);
    assert_eq!(decoded.peer_addr, BdAddr::new([1, 2, 3, 4, 5, 0xc6]));
    assert_eq!(decoded.conn_interval, Duration::from_u16(24));
    assert_eq!(decoded.peripheral_latency, 3);
    assert_eq!(decoded.supervision_timeout, Duration::from_u16(200));
    assert_eq!(decoded.central_clock_accuracy, ClockAccuracy::Ppm75);
}

#[test]
fn peripheral_connection_complete_rejects_non_legacy_peer_address_kind() {
    assert_eq!(
        LePeripheralConnectionCompleteEvent::new(
            ConnHandle::new(1),
            AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            BdAddr::default(),
            Duration::from_u16(24),
            0,
            Duration::from_u16(200),
            ClockAccuracy::Ppm500,
        ),
        Err(
            LePeripheralConnectionCompleteEventError::UnsupportedPeerAddressKind(
                AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            )
        )
    );
}

#[test]
fn failed_connection_complete_has_no_allocated_connection() {
    let status = HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status();
    let event = LePeripheralConnectionCompleteEvent::failed(status)
        .expect("a non-success status represents failed establishment");

    let Event::Le(LeEvent::LeConnectionComplete(decoded)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the failed standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, status);
    assert_eq!(decoded.handle, ConnHandle::new(0));
    assert_eq!(decoded.conn_interval, Duration::from_u16(0));
    assert_eq!(decoded.supervision_timeout, Duration::from_u16(0));

    assert_eq!(
        LePeripheralConnectionCompleteEvent::failed(Status::SUCCESS),
        Err(LePeripheralConnectionCompleteEventError::SuccessfulFailureStatus)
    );
}

#[test]
fn disconnection_complete_roundtrips_through_bt_hci() {
    let reason = HciError::CONN_TIMEOUT.to_status();
    let event = LeDisconnectionCompleteEvent::new(ConnHandle::new(0x0abc), reason);

    let Event::DisconnectionComplete(decoded) = Event::from_hci_bytes_complete(event.as_bytes())
        .expect("bt-hci decodes the complete standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.reason, reason);
}
