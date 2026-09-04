//! One production network scheduler for one permanent logical Wi-Fi device.

use crate::Esp32s31WifiDevice;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct Esp32s31WifiNetworkRunner<'resources> {
    inner: embassy_net::Runner<'resources>,
}

impl Esp32s31WifiNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        // The IP runner and socket owners share one executor. Bound each
        // ingress/egress turn so a continuously replenished device queue
        // cannot starve UDP/TCP consumers in one unbounded poll.
        self.inner.run().await
    }
}

impl<'resources> Esp32s31WifiNetworkRunner<'resources> {
    /// Construct the role-neutral IP stack and its sole production runner.
    ///
    /// IP policy and socket capacity remain application choices. The same
    /// stack owner is used by station and access-point epochs.
    pub fn new(
        device: Esp32s31WifiDevice,
        config: embassy_net::Config,
        resources: &'resources mut embassy_net::StackResources<
            crate::radio_resources::Esp32s31WifiNetworkDevice,
        >,
        random_seed: u64,
    ) -> (embassy_net::Stack<'resources>, Self) {
        let (stack, mut inner) = embassy_net::new(
            device.inner,
            config,
            resources,
            random_seed,
            device.packet_allocator,
        );
        inner.set_poll_budget(embassy_net::PollBudget::new(32, 32));
        (stack, Self { inner })
    }
}
