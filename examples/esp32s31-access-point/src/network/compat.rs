use crate::embassy_net;
pub use embassy_net::{Stack, tcp::TcpSocket, udp::UdpSocket};
pub struct UdpStorage {
    rx_metadata: [embassy_net::udp::PacketMetadata; 4],
    tx_metadata: [embassy_net::udp::PacketMetadata; 4],
    rx: [u8; 4 * 1472],
    tx: [u8; 4 * 1472],
}
impl UdpStorage {
    pub const fn new() -> Self {
        Self {
            rx_metadata: [embassy_net::udp::PacketMetadata::EMPTY; 4],
            tx_metadata: [embassy_net::udp::PacketMetadata::EMPTY; 4],
            rx: [0; 4 * 1472],
            tx: [0; 4 * 1472],
        }
    }
}

pub fn new_udp(stack: Stack<'static>, storage: &'static mut UdpStorage) -> UdpSocket<'static> {
    UdpSocket::new(
        stack,
        &mut storage.rx_metadata,
        &mut storage.rx,
        &mut storage.tx_metadata,
        &mut storage.tx,
    )
}
pub fn new_tcp<'a>(stack: Stack<'static>, rx: &'a mut [u8], tx: &'a mut [u8]) -> TcpSocket<'a> {
    TcpSocket::new(stack, rx, tx)
}
// Released Embassy starts listening through TcpSocket::accept; there is no
// independent listener owner. The application retains only the port here.
pub fn listen(_stack: Stack<'static>, port: u16) -> u16 {
    port
}
pub async fn accept(port: &mut u16, socket: &mut TcpSocket<'_>) -> Result<(), ()> {
    socket.accept(*port).await.map_err(|_| ())
}
pub fn remote_endpoint(meta: embassy_net::udp::UdpMetadata) -> embassy_net::IpEndpoint {
    meta.endpoint
}

#[cfg(target_arch = "riscv32")]
pub async fn run<F: core::future::Future>(
    device: open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiDevice,
    seed: u64,
    application: impl FnOnce(Stack<'static>) -> F,
) -> ! {
    use embassy_net::{Config, Ipv4Address, Ipv4Cidr, StaticConfigV4};
    use open_esp_radio_esp32s31_embassy_wifi::{
        Esp32s31WifiNetworkRunner, Esp32s31WifiStackResources,
    };
    static STORAGE: static_cell::StaticCell<Esp32s31WifiStackResources> =
        static_cell::StaticCell::new();
    let config = Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    let (stack, runner) = Esp32s31WifiNetworkRunner::new(
        device,
        config,
        STORAGE.init(Esp32s31WifiStackResources::new()),
        seed,
    );
    embassy_futures::join::join(application(stack), runner.run()).await;
    unreachable!()
}

impl Default for UdpStorage {
    fn default() -> Self {
        Self::new()
    }
}
