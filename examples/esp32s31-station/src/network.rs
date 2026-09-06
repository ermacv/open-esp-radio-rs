//! Application-owned IP configuration, stack storage and socket workload.

use open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiDevice;
use static_cell::StaticCell;

#[cfg(not(feature = "upstream-network"))]
pub async fn run(device: Esp32s31WifiDevice, seed: u64) -> ! {
    use crate::embassy_net;
    use open_esp_radio_esp32s31_embassy_wifi::{
        Esp32s31WifiNetworkRunner, Esp32s31WifiStackResources,
    };
    static STORAGE: StaticCell<Esp32s31WifiStackResources> = StaticCell::new();
    let (_stack, runner) = Esp32s31WifiNetworkRunner::new(
        device,
        embassy_net::Config::dhcpv4(Default::default()),
        STORAGE.init(Esp32s31WifiStackResources::new()),
        seed,
    );
    runner.run().await
}

#[cfg(feature = "upstream-network")]
pub async fn run(device: Esp32s31WifiDevice, seed: u64) -> ! {
    use embassy_net_upstream::{Stack, StackStorage, udp::UdpSocket};
    use open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiNetworkDevice;
    static STORAGE: StaticCell<StackStorage<'static>> = StaticCell::new();
    static DRIVER: StaticCell<Esp32s31WifiNetworkDevice> = StaticCell::new();
    let (stack, mut runner) = Stack::new(STORAGE.init(StackStorage::new()), seed);
    let iface = stack
        .add_iface(DRIVER.init(device.into_upstream()))
        .expect("one station interface fits");
    iface.set_dhcpv4(Some(Default::default()));
    let sockets = async {
        iface.wait_config_v4_up().await;
        esp_println::println!(
            "open-radio: Xarxa IPv4 ready {:?}",
            iface.ip_addrs()
        );
        let mut udp = UdpSocket::new(stack).expect("one UDP socket fits");
        udp.bind(4321).expect("UDP echo port is available");
        let mut payload = [0u8; 1472];
        loop {
            match udp.recv_from(&mut payload).await {
                Ok((length, metadata)) => {
                    if let Err(error) = udp.send_to(&payload[..length], metadata.endpoint).await {
                        esp_println::println!("open-radio: UDP echo send failed: {:?}", error);
                    }
                }
                Err(error) => {
                    esp_println::println!("open-radio: UDP echo receive failed: {:?}", error)
                }
            }
        }
    };
    embassy_futures::join::join(runner.run(), sockets).await;
    unreachable!()
}
