//! Exercise the target's IP policy against the real original upstream stack.
use embassy_net::{Stack, StackStorage, driver::Driver};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time as _;
use open_esp_radio_hil_protocol::{
    NetworkIpv4Configuration as Config, WifiNetworkInterface as Role, WifiRxChecksumPolicy as Rx,
    WifiTxUdpChecksumPolicy as Tx,
};
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources};

#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/checksum.rs"]
mod checksum;
#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/ipv4.rs"]
mod ipv4;

#[test]
fn role_reconfiguration_removes_old_addresses_and_routes_without_touching_peer() {
    let mut sta = Resources::<NoopRawMutex, 2, 2>::new();
    let mut ap = Resources::<NoopRawMutex, 2, 2>::new();
    let (mut sta_device, sta_radio) = sta.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]);
    let (mut ap_device, ap_radio) = ap.split(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 2]);
    sta_radio.link_controller().set_link_state(LinkState::Up);
    ap_radio.link_controller().set_link_state(LinkState::Up);
    let mut sta_storage = StackStorage::new();
    let mut ap_storage = StackStorage::new();
    let (sta_stack, _sta_runner) = Stack::new(&mut sta_storage, 1);
    let (ap_stack, _ap_runner) = Stack::new(&mut ap_storage, 2);
    let sta_iface = sta_stack.add_iface(&mut sta_device).unwrap();
    let ap_iface = ap_stack.add_iface(&mut ap_device).unwrap();
    ipv4::configure(
        sta_iface,
        Some(Config::Static {
            address: [192, 168, 1, 2],
            prefix_length: 24,
            gateway: Some([192, 168, 1, 1]),
        }),
    );
    ipv4::configure(
        ap_iface,
        Some(Config::Static {
            address: [10, 0, 0, 1],
            prefix_length: 24,
            gateway: None,
        }),
    );
    let sta_before = ipv4::info(sta_iface, Role::Station).unwrap();
    assert_eq!(sta_before.gateway, Some([192, 168, 1, 1]));
    ipv4::configure(ap_iface, None);
    assert!(ipv4::info(ap_iface, Role::AccessPoint).is_none());
    assert_eq!(ipv4::info(sta_iface, Role::Station).unwrap(), sta_before);
    ipv4::configure(sta_iface, Some(Config::Dhcp));
    assert!(ipv4::info(sta_iface, Role::Station).is_none());
    assert!(sta_stack.routes().is_empty());
    ipv4::configure(
        ap_iface,
        Some(Config::Static {
            address: [10, 1, 0, 1],
            prefix_length: 24,
            gateway: None,
        }),
    );
    assert_eq!(
        ipv4::info(ap_iface, Role::AccessPoint).unwrap().address,
        [10, 1, 0, 1]
    );
}

#[test]
fn checksum_experiment_keeps_ipv4_tx_and_tcp_checksums_enabled() {
    let mut resources = Resources::<NoopRawMutex, 2, 2>::new();
    let (device, _) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]);
    let default = checksum::Device::new(device, Rx::Software, Tx::Software);
    assert_eq!(default.capabilities().checksum, Default::default());
    let diagnostic =
        checksum::Device::new(default, Rx::AssumeValidDiagnostic, Tx::OmitIpv4Diagnostic);
    let caps = diagnostic.capabilities().checksum;
    assert!(caps.ipv4.rx && caps.udp.rx && caps.udp.tx);
    assert!(!caps.ipv4.tx && !caps.tcp.rx && !caps.tcp.tx);
}
