//! One production network scheduler for one permanent logical Wi-Fi device.

use crate::Esp32s31WifiDevice;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct Esp32s31WifiNetworkRunner<'resources> {
    inner: embassy_net::Runner<'resources, Esp32s31WifiDevice>,
}

impl Esp32s31WifiNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        // The IP runner and socket owners share one executor. Bound each
        // ingress/egress turn so a continuously replenished device queue
        // cannot starve UDP/TCP consumers in one unbounded poll.
        self.inner.run_work_conserving().await
    }

    /// Run the production scheduler while reporting one aggregate record per
    /// bounded stack poll. The observer is diagnostic-only; it cannot alter
    /// the scheduler decision or packet ownership.
    #[cfg(feature = "network-scheduler-telemetry")]
    pub async fn run_observed(
        mut self,
        observer: fn(embassy_net::CooperativePollReport),
    ) -> ! {
        self.inner.run_work_conserving_observed(observer).await
    }
}

impl<'resources> Esp32s31WifiNetworkRunner<'resources> {
    /// Construct the role-neutral IP stack and its sole production runner.
    ///
    /// IP policy and socket capacity remain application choices. The same
    /// stack owner is used by station and access-point epochs.
    pub fn new<const SOCKETS: usize>(
        device: Esp32s31WifiDevice,
        config: embassy_net::Config,
        resources: &'resources mut embassy_net::StackResources<SOCKETS>,
        random_seed: u64,
    ) -> (embassy_net::Stack<'resources>, Self) {
        let (stack, inner) = embassy_net::new(device, config, resources, random_seed);
        (stack, Self { inner })
    }
}
