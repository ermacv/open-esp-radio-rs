//! Socket construction and API differences; traffic generation stays in `traffic`.
#[cfg(feature = "upstream-network")]
pub type TcpSocket<'a> = embassy_net::tcp::TcpSocket<'a, 'a>;
#[cfg(not(feature = "upstream-network"))]
pub use embassy_net::tcp::TcpSocket;
#[cfg(feature = "upstream-network")]
pub use embassy_net::wire::{IpEndpoint, Ipv4Address};
#[cfg(not(feature = "upstream-network"))]
pub use embassy_net::{IpEndpoint, Ipv4Address};
pub use embassy_net::{Stack, udp::UdpSocket};

#[cfg(feature = "compat-network")]
mod smoltcp;
#[cfg(feature = "compat-network")]
pub use smoltcp::*;
#[cfg(not(feature = "compat-network"))]
mod xarxa;
#[cfg(not(feature = "compat-network"))]
pub use xarxa::*;

pub fn new_tcp<'a>(stack: Stack<'a>, rx: &'a mut [u8], tx: &'a mut [u8]) -> TcpSocket<'a> {
    #[cfg(feature = "upstream-network")]
    return TcpSocket::new(stack, rx, tx).expect("HIL TCP socket capacity");
    #[cfg(not(feature = "upstream-network"))]
    TcpSocket::new(stack, rx, tx)
}

// Unidirectional HIL sockets allocate only the byte rings they use.
pub type UdpRxStorage = UdpStorage<UDP_RX_QUEUE_DEPTH, 0>;
pub type UdpTxStorage = UdpStorage<0, 16>;
