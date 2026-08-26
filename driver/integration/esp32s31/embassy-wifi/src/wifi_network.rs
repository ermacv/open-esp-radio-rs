//! One production network scheduler for one permanent logical Wi-Fi device.

use crate::Esp32s31WifiDevice;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct Esp32s31WifiNetworkRunner<'resources> {
    inner: embassy_net::Runner<'resources, Esp32s31WifiDevice>,
}

impl Esp32s31WifiNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        // The device already bounds RX epochs and TX credits. Keep xarxa's
        // native batched poll here: wrapping its single-packet primitives in
        // a second scheduler measurably lowers the saturated RX ceiling.
        self.inner.run().await
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
