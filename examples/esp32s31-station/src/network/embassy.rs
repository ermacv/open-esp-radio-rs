use oer_esp32s31_embassy_wifi::WifiDevice;

use static_cell::StaticCell;
pub async fn run(device: WifiDevice, seed: u64) -> ! {
    use crate::embassy_net;

    use oer_esp32s31_embassy_wifi::{WifiNetworkRunner, WifiStackResources};
    static STORAGE: StaticCell<WifiStackResources> = StaticCell::new();
    let (stack, runner) = WifiNetworkRunner::new(
        device,
        embassy_net::Config::dhcpv4(Default::default()),
        STORAGE.init(WifiStackResources::new()),
        seed,
    );
    let application = async {
        stack.wait_config_up().await;
        esp_println::println!("open-radio: IPv4 ready {:?}", stack.config_v4());
        super::echo(stack).await
    };
    embassy_futures::join::join(application, runner.run()).await;
    unreachable!()
}
