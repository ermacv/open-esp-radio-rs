//! Upstream endpoint allocation. The global PacketBuf pool remains owned and
//! placed by Xarxa; only queue metadata and final Wi-Fi SRAM belong here.

use super::*;
use open_esp_radio_xarxa_upstream::{Device, Resources};

const RX_DEPTH: usize = 16;
const TX_DEPTH: usize = 16;
type EndpointResources = Resources<CriticalSectionRawMutex, RX_DEPTH, TX_DEPTH>;
static STATION: ConstStaticCell<EndpointResources> = ConstStaticCell::new(Resources::new());
static ACCESS_POINT: ConstStaticCell<EndpointResources> = ConstStaticCell::new(Resources::new());

pub type Esp32s31WifiNetworkDevice = Device<'static, CriticalSectionRawMutex, RX_DEPTH, TX_DEPTH>;
pub(super) type RadioNetworkRunner = open_esp_radio_esp32s31_wifi_xarxa_upstream::Network<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    RX_DEPTH,
    TX_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;

pub(crate) fn initialize_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> (Esp32s31WifiDevices, WifiNetworkResources) {
    use open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::{
        AP_NETWORK_INTERFACE_ID, STA_NETWORK_INTERFACE_ID,
    };
    let (station, station_endpoint) = STATION
        .take()
        .split(STA_NETWORK_INTERFACE_ID, station_address);
    let (access_point, access_point_endpoint) = ACCESS_POINT
        .take()
        .split(AP_NETWORK_INTERFACE_ID, access_point_address);
    let runner = NETWORK_RUNNER.init(RadioNetworkRunner::new(
        station_endpoint,
        access_point_endpoint,
        initialize_physical_tx(),
    ));
    (
        Esp32s31WifiDevices {
            station: Esp32s31WifiDevice { inner: station },
            access_point: Esp32s31WifiDevice {
                inner: access_point,
            },
        },
        WifiNetworkResources::Unstarted { device: (), runner },
    )
}
