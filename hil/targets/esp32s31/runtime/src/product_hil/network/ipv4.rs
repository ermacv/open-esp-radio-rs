//! Per-endpoint HIL IPv4 policy using the original public interface API.
#![forbid(unsafe_code)]
use embassy_net::{
    iface::Iface,
    wire::{IpAddress, IpCidr, Ipv4Address, Ipv4Cidr},
};
use open_esp_radio_hil_protocol::{NetworkInfo, NetworkIpv4Configuration, WifiNetworkInterface};
pub(crate) fn configure(iface: Iface<'_>, config: Option<NetworkIpv4Configuration>) {
    iface.set_dhcpv4(None);
    iface.set_ip_addrs([]).expect("empty address list fits");
    iface.stack().routes().clear();
    match config {
        None => {}
        Some(NetworkIpv4Configuration::Dhcp) => iface.set_dhcpv4(Some(Default::default())),
        Some(NetworkIpv4Configuration::Static {
            address,
            prefix_length,
            gateway,
        }) => {
            iface
                .add_ip_addr(IpCidr::Ipv4(Ipv4Cidr::new(
                    Ipv4Address::from_octets(address),
                    prefix_length,
                )))
                .expect("one HIL IPv4 address fits");
            if let Some(gateway) = gateway {
                iface
                    .stack()
                    .routes()
                    .add_default_ipv4_route(Ipv4Address::from_octets(gateway), iface.handle())
                    .expect("one HIL gateway fits");
            }
        }
    }
}

pub(crate) fn info(
    iface: Iface<'_>,
    network_interface: WifiNetworkInterface,
) -> Option<NetworkInfo> {
    let address = iface.ip_addrs().first()?.cidr;
    let IpCidr::Ipv4(address) = address;
    let gateway = iface
        .stack()
        .routes()
        .iter()
        .find(|route| route.iface == iface.handle() && route.is_ipv4_gateway())
        .map(|route| {
            let IpAddress::Ipv4(address) = route.via_router;
            address.octets()
        });
    Some(NetworkInfo {
        network_interface,
        address: address.address().octets(),
        prefix_length: address.prefix_len(),
        gateway,
    })
}
