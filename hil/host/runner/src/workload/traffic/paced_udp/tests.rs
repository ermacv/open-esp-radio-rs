use super::*;

#[test]
fn packet_interval_matches_requested_payload_rate() {
    assert_eq!(
        packet_interval(1_200, 80_000_000).unwrap(),
        Duration::from_micros(120)
    );
    assert_eq!(
        packet_interval(1_200, 10_000_000).unwrap(),
        Duration::from_micros(960)
    );
}

#[test]
fn terminal_marker_is_redundant_and_bounded() {
    let receiver = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let sender = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();

    send_terminal_markers(&sender).unwrap();

    let mut marker = [0_u8; 4];
    for _ in 0..TERMINAL_MARKERS {
        let (length, source) = receiver.recv_from(&mut marker).unwrap();
        assert_eq!(length, marker.len());
        assert_eq!(source, sender.local_addr().unwrap());
        assert_eq!(i32::from_be_bytes(marker), -1);
    }
}
