use super::*;
use std::net::{Ipv4Addr, SocketAddrV4};

#[test]
fn qualification_socket_reads_back_at_least_the_requested_capacity() {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let actual = configure_qualification_receive_buffer(&socket).unwrap();
    assert!(actual >= QUALIFICATION_RECEIVE_BUFFER_BYTES);
}
