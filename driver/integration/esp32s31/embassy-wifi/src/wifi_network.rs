//! One production network scheduler shared by every Wi-Fi role.

use crate::Esp32s31WifiDevice;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct Esp32s31WifiNetworkRunner<'resources> {
    inner: embassy_net::Runner<'resources, Esp32s31WifiDevice>,
}

impl Esp32s31WifiNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        self.inner.run_work_conserving().await
    }

    #[cfg(feature = "qualification")]
    pub async fn run_observed(mut self, observer: fn(embassy_net::CooperativePollReport)) -> ! {
        self.inner.run_work_conserving_observed(observer).await
    }
}

/// Construct the role-neutral IP stack and its sole production runner.
///
/// IP policy and socket capacity remain application choices. The same stack
/// owner is used by station and access-point epochs.
pub fn new_wifi_network<'resources, const SOCKETS: usize>(
    device: Esp32s31WifiDevice,
    config: embassy_net::Config,
    resources: &'resources mut embassy_net::StackResources<SOCKETS>,
    random_seed: u64,
) -> (
    embassy_net::Stack<'resources>,
    Esp32s31WifiNetworkRunner<'resources>,
) {
    let (stack, inner) = embassy_net::new(device, config, resources, random_seed);
    (stack, Esp32s31WifiNetworkRunner { inner })
}
