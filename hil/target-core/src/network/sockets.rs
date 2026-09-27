//! Socket construction and API differences; traffic generation stays in `traffic`.
pub use embassy_net::tcp::TcpSocket;
pub use embassy_net::{IpEndpoint, Ipv4Address};
pub use embassy_net::{Stack, udp::UdpSocket};

mod xarxa;
pub use xarxa::*;

pub fn new_tcp<'a>(stack: Stack<'a>, rx: &'a mut [u8], tx: &'a mut [u8]) -> TcpSocket<'a> {
    TcpSocket::new(stack, rx, tx)
}

// Unidirectional HIL sockets allocate only the byte rings they use.
pub type UdpRxStorage = UdpStorage<UDP_RX_QUEUE_DEPTH, 0>;
// One RX slot serves the unmeasured reverse-flow challenge before Start.
pub type UdpTxStorage = UdpStorage<1, 16>;
