//! Exercise the released stack's opt-in echo responder through the real adapter.
use embassy_net_compat::{Config, Ipv4Address, Ipv4Cidr, StackResources, StaticConfigV4};
use open_esp_radio_embassy_net_compat::{FrameStorage, LinkState, NoopRawMutex, Resources};
use std::{
    future::Future,
    task::{Context, Waker},
};

fn checksum(bytes: &[u8]) -> [u8; 2] {
    let mut sum: u32 = bytes
        .chunks(2)
        .map(|word| u32::from(u16::from_be_bytes([word[0], *word.get(1).unwrap_or(&0)])))
        .sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    (!(sum as u16)).to_be_bytes()
}

#[test]
fn released_stack_answers_icmp_echo_after_neighbor_resolution() {
    let local_mac = [2, 0, 0, 0, 0, 1];
    let peer_mac = [2, 0, 0, 0, 0, 2];
    let local_ip = [192, 0, 2, 1];
    let peer_ip = [192, 0, 2, 2];
    let resources = Box::leak(Box::new(Resources::<NoopRawMutex, 1536, 2>::new()));
    let rx = Box::leak(Box::new(FrameStorage::new()));
    let tx = Box::leak(Box::new(FrameStorage::new()));
    let (device, radio) = resources.split(local_mac, rx, tx);
    radio.set_link_state(LinkState::Up);
    let mut stack_resources = StackResources::<4>::new();
    let config = Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::from_octets(local_ip), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    let (_, mut runner) = embassy_net_compat::new(device, config, &mut stack_resources, 1);
    let mut network = std::pin::pin!(runner.run());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(network.as_mut().poll(&mut cx).is_pending());

    // Resolve the peer through a normal Ethernet/IPv4 ARP request first.
    let mut arp = [0u8; 60];
    arp[..6].fill(255);
    arp[6..12].copy_from_slice(&peer_mac);
    arp[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
    arp[22..28].copy_from_slice(&peer_mac);
    arp[28..32].copy_from_slice(&peer_ip);
    arp[38..42].copy_from_slice(&local_ip);
    radio.try_send_rx(&arp).unwrap();
    assert!(network.as_mut().poll(&mut cx).is_pending());
    let response = radio.try_receive_tx().expect("ARP reply reaches the radio");
    assert_eq!(&response.as_slice()[20..22], &[0, 2]);
    drop(response);

    let mut echo = [0u8; 60];
    echo[..6].copy_from_slice(&local_mac);
    echo[6..12].copy_from_slice(&peer_mac);
    echo[12..14].copy_from_slice(&[8, 0]);
    echo[14] = 0x45;
    echo[16..18].copy_from_slice(&32_u16.to_be_bytes());
    echo[22] = 64;
    echo[23] = 1;
    echo[26..30].copy_from_slice(&peer_ip);
    echo[30..34].copy_from_slice(&local_ip);
    let ip_checksum = checksum(&echo[14..34]);
    echo[24..26].copy_from_slice(&ip_checksum);
    echo[34] = 8;
    echo[38..46].copy_from_slice(&[0x12, 0x34, 0, 7, b'p', b'i', b'n', b'g']);
    let icmp_checksum = checksum(&echo[34..46]);
    echo[36..38].copy_from_slice(&icmp_checksum);
    radio.try_send_rx(&echo).unwrap();
    assert!(network.as_mut().poll(&mut cx).is_pending());
    let response = radio
        .try_receive_tx()
        .expect("ICMP echo reply reaches the radio");
    let response = response.as_slice();
    assert_eq!(&response[..6], &peer_mac);
    assert_eq!(&response[26..30], &local_ip);
    assert_eq!(&response[30..34], &peer_ip);
    assert_eq!(response[34], 0, "ICMP echo reply");
    assert_eq!(&response[38..46], &echo[38..46]);
    assert_eq!(checksum(&response[14..34]), [0, 0]);
    assert_eq!(checksum(&response[34..46]), [0, 0]);
}
