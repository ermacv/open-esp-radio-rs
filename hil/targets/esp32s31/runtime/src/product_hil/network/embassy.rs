//! Released smoltcp and maintained owned-packet Embassy composition.
#[cfg(feature = "task-poll-telemetry")]
use super::{observation, progress};
use embassy_net::Config;
use open_esp_radio_esp32s31_embassy_wifi::{Esp32s31WifiDevice, Esp32s31WifiNetworkDevice};
use open_esp_radio_hil_protocol::{
    WifiNetworkInterface, WifiRxChecksumPolicy, WifiTxUdpChecksumPolicy,
};
#[cfg(feature = "task-poll-telemetry")]
type Device = progress::Device<Esp32s31WifiNetworkDevice>;
#[cfg(not(feature = "task-poll-telemetry"))]
type Device = Esp32s31WifiNetworkDevice;
#[cfg(feature = "owned-network")]
pub(crate) type Runner<'a> = embassy_net::Runner<'a>;
#[cfg(feature = "compat-network")]
pub(crate) type Runner<'a> = embassy_net::Runner<'a, Device>;
#[cfg(feature = "compat-network")]
pub(crate) type Resources = embassy_net::StackResources<16>;
#[cfg(feature = "owned-network")]
pub(crate) type Resources = embassy_net::StackResources<Device>;

mod ipv4;
pub(crate) use ipv4::{Iface, configure, info};
pub(crate) fn new(
    device: Esp32s31WifiDevice,
    resources: &'static mut Resources,
    settings: super::Settings,
    _role: WifiNetworkInterface,
) -> (Iface<'static>, Runner<'static>) {
    let device = device
        .with_software_ipv4_udp_rx_checksum_validation(
            settings.rx_checksum == WifiRxChecksumPolicy::Software,
        )
        .with_software_ipv4_udp_tx_checksum_generation(
            settings.tx_udp_checksum == WifiTxUdpChecksumPolicy::Software,
        );
    #[cfg(feature = "compat-network")]
    let device = device.into_compat();
    #[cfg(feature = "owned-network")]
    let (device, allocator) = device.into_owned();
    #[cfg(feature = "task-poll-telemetry")]
    let device = progress::Device::new(device, observation::counters(_role));
    let mut config = Config::default();
    config.ipv4 = ipv4::config(settings.ipv4);
    #[cfg(feature = "compat-network")]
    let (stack, runner) = embassy_net::new(device, config, resources, settings.seed);
    #[cfg(feature = "owned-network")]
    let (stack, mut runner) = embassy_net::new(device, config, resources, settings.seed, allocator);
    #[cfg(feature = "owned-network")]
    runner.set_poll_budget(embassy_net::PollBudget::new(32, 32));
    (Iface(stack), runner)
}
