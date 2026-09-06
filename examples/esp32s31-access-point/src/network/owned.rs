use crate::embassy_net;
pub use embassy_net::{
    Stack,
    tcp::{TcpListener, TcpSocket},
    udp::UdpSocket,
};
pub fn new_udp(stack: Stack<'static>) -> UdpSocket<'static> {
    UdpSocket::new(stack)
}
pub fn new_tcp<'a>(stack: Stack<'static>, rx: &'a mut [u8], tx: &'a mut [u8]) -> TcpSocket<'a> {
    TcpSocket::new(stack, rx, tx)
}
pub fn new_listener(stack: Stack<'static>) -> TcpListener<'static> {
    TcpListener::new(stack)
}
pub async fn accept(
    listener: &mut TcpListener<'static>,
    socket: &mut TcpSocket<'_>,
) -> Result<(), ()> {
    listener.accept(socket).await.map_err(|_| ())
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
