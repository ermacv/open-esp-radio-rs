use crate::embassy_net;
pub use embassy_net::{Stack, tcp::TcpListener, udp::UdpSocket};
pub type TcpSocket<'a> = embassy_net::tcp::TcpSocket<'a, 'static>;
pub fn new_udp(stack: Stack<'static>) -> UdpSocket<'static> {
    UdpSocket::new(stack).expect("UDP socket capacity")
}
pub fn new_tcp<'a>(stack: Stack<'static>, rx: &'a mut [u8], tx: &'a mut [u8]) -> TcpSocket<'a> {
    TcpSocket::new(stack, rx, tx).expect("TCP socket capacity")
}
pub fn new_listener(stack: Stack<'static>) -> TcpListener<'static> {
    TcpListener::new(stack).expect("TCP listener capacity")
}
pub async fn accept(
    listener: &mut TcpListener<'static>,
    socket: &mut TcpSocket<'_>,
) -> Result<(), ()> {
    let token = listener.accept().await.map_err(|_| ())?;
    socket.accept(token).await.map_err(|_| ())
}
pub fn remote_endpoint(meta: embassy_net::udp::UdpMetadata) -> embassy_net::wire::IpEndpoint {
    meta.endpoint
}

#[cfg(target_arch = "riscv32")]
pub async fn run<F: core::future::Future>(
    device: open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiDevice,
    seed: u64,
    application: impl FnOnce(Stack<'static>) -> F,
) -> ! {
    use embassy_net::{
        StackStorage,
        wire::{IpAddress, IpCidr},
    };
    use open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiNetworkDevice;
    static STORAGE: static_cell::StaticCell<StackStorage<'static>> = static_cell::StaticCell::new();
    static DRIVER: static_cell::StaticCell<Esp32s31WifiNetworkDevice> =
        static_cell::StaticCell::new();
    let (stack, mut runner) = Stack::new(STORAGE.init(StackStorage::new()), seed);
    let iface = stack
        .add_iface(DRIVER.init(device.into_upstream()))
        .expect("one AP interface fits");
    iface
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 168, 4, 1), 24)])
        .expect("one static AP address fits");
    embassy_futures::join::join(application(stack), runner.run()).await;
    unreachable!()
}
