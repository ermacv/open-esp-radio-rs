//! One production network scheduler shared by applications and HIL.

use crate::Esp32s31WifiDevice;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
///
/// Its scheduling policy is fixed inside the pinned Embassy fork. Test
/// direction, traffic scenarios, and core placement cannot change it.
/// Split placement is the high-throughput topology: on one cooperative
/// executor a full egress budget can delay radio IRQ service even though the
/// scheduler remains bounded and directionally fair.
pub struct Esp32s31StationNetworkRunner<'resources> {
    inner: embassy_net::Runner<'resources, Esp32s31WifiDevice>,
}

impl Esp32s31StationNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        self.inner.run_work_conserving().await
    }

    /// Observe the unchanged production policy in qualification firmware.
    #[cfg(feature = "qualification")]
    pub async fn run_observed(mut self, observer: fn(embassy_net::CooperativePollReport)) -> ! {
        self.inner.run_work_conserving_observed(observer).await
    }
}

/// Construct the network stack and its sole production runner.
///
/// IP policy and socket capacity remain application choices. Driver queue
/// ownership and scheduling do not.
pub fn new_station_network<'resources, const SOCKETS: usize>(
    device: Esp32s31WifiDevice,
    config: embassy_net::Config,
    resources: &'resources mut embassy_net::StackResources<SOCKETS>,
    random_seed: u64,
) -> (
    embassy_net::Stack<'resources>,
    Esp32s31StationNetworkRunner<'resources>,
) {
    let (stack, inner) = embassy_net::new(device, config, resources, random_seed);
    (stack, Esp32s31StationNetworkRunner { inner })
}
