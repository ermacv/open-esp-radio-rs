//! Upstream endpoint allocation. The global PacketBuf pool remains owned and
//! placed by Xarxa; only queue metadata and final Wi-Fi SRAM belong here.

use super::*;
use core::cell::Cell;
use embassy_sync::blocking_mutex::Mutex;
use open_esp_radio_xarxa_upstream::{Device, Resources};

const RX_DEPTH: usize = 16;
// One TX queue must not consume all 16 owners in the default shared pool.
// Selected frames, sockets and the other interface also count; this is queued
// admission headroom, not an exclusive RX reservation.
const TX_DEPTH: usize = 8;
type EndpointResources = Resources<CriticalSectionRawMutex, RX_DEPTH, TX_DEPTH>;
static STATION: ConstStaticCell<EndpointResources> = ConstStaticCell::new(Resources::new());
static ACCESS_POINT: ConstStaticCell<EndpointResources> = ConstStaticCell::new(Resources::new());

type Endpoint =
    open_esp_radio_xarxa_upstream::Endpoint<'static, CriticalSectionRawMutex, RX_DEPTH, TX_DEPTH>;
static STATION_MONITOR: Mutex<CriticalSectionRawMutex, Cell<Option<Endpoint>>> =
    Mutex::new(Cell::new(None));

/// Cumulative allocation refusals at the station's shared Xarxa pool boundary.
/// Returns `None` before network resources have been initialized. Reading this
/// counter does not require the radio executor to make progress.
pub fn station_rx_pool_drops() -> Option<u32> {
    STATION_MONITOR.lock(|monitor| monitor.get().map(|endpoint| endpoint.rx_pool_drops()))
}

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
    STATION_MONITOR.lock(|monitor| monitor.set(Some(station_endpoint)));
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
