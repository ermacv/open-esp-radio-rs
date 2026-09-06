use super::*;
use embassy_net::udp::PacketMetadata;
pub const UDP_RX_QUEUE_DEPTH: usize = 16;
const CAPACITY: usize = 1472;
/// Per-socket datagram storage, retained statically outside executor futures.
pub struct UdpStorage<const RX: usize, const TX: usize> {
    rx_meta: [PacketMetadata; RX],
    tx_meta: [PacketMetadata; TX],
    rx: [[u8; CAPACITY]; RX],
    tx: [[u8; CAPACITY]; TX],
}
impl<const RX: usize, const TX: usize> UdpStorage<RX, TX> {
    pub const fn new() -> Self {
        Self {
            rx_meta: [PacketMetadata::EMPTY; RX],
            tx_meta: [PacketMetadata::EMPTY; TX],
            rx: [[0; CAPACITY]; RX],
            tx: [[0; CAPACITY]; TX],
        }
    }
}
pub fn new_udp<'a, const RX: usize, const TX: usize>(
    stack: Stack<'a>,
    storage: &'a mut UdpStorage<RX, TX>,
) -> UdpSocket<'a> {
    UdpSocket::new(
        stack,
        &mut storage.rx_meta,
        storage.rx.as_flattened_mut(),
        &mut storage.tx_meta,
        storage.tx.as_flattened_mut(),
    )
}
pub async fn recv_from_with<R>(
    socket: &mut UdpSocket<'_>,
    f: impl FnOnce(&[u8], embassy_net::udp::UdpMetadata) -> R,
) -> Result<R, embassy_net::udp::RecvError> {
    Ok(socket.recv_from_with(f).await)
}
// The released API starts listening in accept(), on the socket itself.
pub fn listen(_stack: Stack<'_>, port: u16) -> u16 {
    port
}
pub async fn accept(
    port: &mut u16,
    socket: &mut TcpSocket<'_>,
) -> Result<(), embassy_net::tcp::AcceptError> {
    socket.accept(*port).await
}
