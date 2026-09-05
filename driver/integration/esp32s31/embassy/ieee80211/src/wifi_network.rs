//! One production network scheduler for one permanent logical Wi-Fi device.

#[cfg(feature = "compat-network")]
use embassy_net_compat as embassy_net;
#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;

use crate::{Esp32s31WifiDevice, Esp32s31WifiStackResources};

#[cfg(feature = "owned-network")]
type NetworkRunner<'resources> = embassy_net::Runner<'resources>;
#[cfg(feature = "compat-network")]
type NetworkRunner<'resources> =
    embassy_net::Runner<'resources, crate::radio_resources::Esp32s31WifiNetworkDevice>;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct Esp32s31WifiNetworkRunner<'resources> {
    inner: NetworkRunner<'resources>,
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
        resources: &'resources mut Esp32s31WifiStackResources,
        random_seed: u64,
    ) -> (embassy_net::Stack<'resources>, Self) {
        #[cfg(feature = "owned-network")]
        let (stack, mut inner) = embassy_net::new(
            device.inner,
            config,
            resources,
            random_seed,
            device.packet_allocator,
        );
        #[cfg(feature = "owned-network")]
        inner.set_poll_budget(embassy_net::PollBudget::new(32, 32));

        #[cfg(feature = "compat-network")]
        let (stack, inner) = embassy_net::new(device.inner, config, resources, random_seed);

        (stack, Self { inner })
    }
}
