//! Role-local configuration shared by released and owned Embassy APIs.
use embassy_net::{ConfigV4, Ipv4Address, Ipv4Cidr, Stack, StaticConfigV4};
use open_esp_radio_hil_protocol::{NetworkInfo, NetworkIpv4Configuration, WifiNetworkInterface};
#[derive(Clone, Copy)]
pub(crate) struct Iface<'a>(pub(super) Stack<'a>);
impl<'a> Iface<'a> {
    pub fn stack(self) -> Stack<'a> {
        self.0
    }
    pub async fn wait_config_v4_up(self) {
        self.0.wait_config_up().await;
    }
    pub async fn wait_config_v4_down(self) {
        self.0.wait_config_down().await;
    }
}
pub(crate) fn configure(iface: Iface<'_>, ipv4: Option<NetworkIpv4Configuration>) {
    iface.0.set_config_v4(config(ipv4));
}
pub(super) fn config(ipv4: Option<NetworkIpv4Configuration>) -> ConfigV4 {
    match ipv4 {
        None => ConfigV4::None,
        Some(NetworkIpv4Configuration::Dhcp) => ConfigV4::Dhcp(Default::default()),
        Some(NetworkIpv4Configuration::Static {
            address,
            prefix_length,
            gateway,
        }) => ConfigV4::Static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::from_octets(address), prefix_length),
            gateway: gateway.map(Ipv4Address::from_octets),
            dns_servers: Default::default(),
        }),
    }
}
pub(crate) fn info(
    iface: Iface<'_>,
    network_interface: WifiNetworkInterface,
) -> Option<NetworkInfo> {
    let config = iface.0.config_v4()?;
    Some(NetworkInfo {
        network_interface,
        address: config.address.address().octets(),
        prefix_length: config.address.prefix_len(),
        gateway: config.gateway.map(|a| a.octets()),
    })
}
