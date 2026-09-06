use open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiDevice;
use static_cell::StaticCell;
pub async fn run(device: Esp32s31WifiDevice, seed: u64) -> ! {
    use crate::embassy_net;
    use open_esp_radio_esp32s31_embassy_wifi::{
        Esp32s31WifiNetworkRunner, Esp32s31WifiStackResources,
    };
    static STORAGE: StaticCell<Esp32s31WifiStackResources> = StaticCell::new();
    let (stack, runner) = Esp32s31WifiNetworkRunner::new(
        device,
        embassy_net::Config::dhcpv4(Default::default()),
        STORAGE.init(Esp32s31WifiStackResources::new()),
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
