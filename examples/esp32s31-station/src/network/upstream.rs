use oer_esp32s31_embassy_wifi::WifiDevice;

use static_cell::StaticCell;
pub async fn run(device: WifiDevice, seed: u64) -> ! {
    use embassy_net_upstream::{Stack, StackStorage};

    use oer_esp32s31_embassy_wifi::WifiNetworkDevice;
    static STORAGE: StaticCell<StackStorage<'static>> = StaticCell::new();
    static DRIVER: StaticCell<WifiNetworkDevice> = StaticCell::new();
    let (stack, mut runner) = Stack::new(STORAGE.init(StackStorage::new()), seed);
    let iface = stack
        .add_iface(DRIVER.init(device.into_upstream()))
        .expect("one station interface fits");
    iface.set_dhcpv4(Some(Default::default()));
    let sockets = async {
        iface.wait_config_v4_up().await;
        esp_println::println!("open-radio: Xarxa IPv4 ready {:?}", iface.ip_addrs());
        super::echo(stack).await
    };
    embassy_futures::join::join(runner.run(), sockets).await;
    unreachable!()
}
