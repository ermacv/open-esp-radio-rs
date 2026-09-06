//! Original Xarxa stack composition, also used with the minimal source patch.
use super::checksum;
#[cfg(feature = "task-poll-telemetry")]
use super::{observation, progress};
pub(crate) use embassy_net::{Runner, iface::Iface};
use embassy_net::{Stack, StackStorage};
use open_esp_radio_esp32s31_embassy_wifi::{Esp32s31WifiDevice, Esp32s31WifiNetworkDevice};
pub(crate) struct Resources {
    stack: StackStorage<'static>,
    driver: Option<NetworkDevice>,
}

#[cfg(not(feature = "task-poll-telemetry"))]
type NetworkDevice = checksum::Device<Esp32s31WifiNetworkDevice>;
#[cfg(feature = "task-poll-telemetry")]
type NetworkDevice = progress::Device<checksum::Device<Esp32s31WifiNetworkDevice>>;

impl Resources {
    pub const fn new() -> Self {
        Self {
            stack: StackStorage::new(),
            driver: None,
        }
    }
}

pub(crate) fn new(
    device: Esp32s31WifiDevice,
    resources: &'static mut Resources,
    settings: super::Settings,
    _role: open_esp_radio_hil_protocol::WifiNetworkInterface,
) -> (Iface<'static>, Runner<'static>) {
    let (stack, runner) = Stack::new(&mut resources.stack, settings.seed);
    let driver = checksum::Device::new(
        device.into_upstream(),
        settings.rx_checksum,
        settings.tx_udp_checksum,
    );
    #[cfg(feature = "task-poll-telemetry")]
    let driver = progress::Device::new(driver, observation::counters(_role));
    let iface = stack
        .add_iface(resources.driver.insert(driver))
        .expect("one HIL interface per stack");
    super::configure(iface, settings.ipv4);
    (iface, runner)
}
