//! HIL-owned upstream stack storage and per-role IPv4 configuration.
//!
//! Each role keeps its own stack and interface across radio restarts. Socket
//! workloads use the original Embassy API; radio ownership stays in the product.

#![forbid(unsafe_code)]

use embassy_net::{Runner, Stack, StackStorage, iface::Iface};
use open_esp_radio_esp32s31_embassy_wifi::{Esp32s31WifiDevice, Esp32s31WifiNetworkDevice};

pub(super) struct Settings {
    pub ipv4: Option<open_esp_radio_hil_protocol::NetworkIpv4Configuration>,
    pub seed: u64,
    pub rx_checksum: open_esp_radio_hil_protocol::WifiRxChecksumPolicy,
    pub tx_udp_checksum: open_esp_radio_hil_protocol::WifiTxUdpChecksumPolicy,
}

pub(super) struct Resources {
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

pub(super) fn new(
    device: Esp32s31WifiDevice,
    resources: &'static mut Resources,
    settings: Settings,
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
    configure(iface, settings.ipv4);
    (iface, runner)
}

mod checksum;
mod ipv4;
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod observation;
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod progress;
pub(super) use ipv4::{configure, info};
