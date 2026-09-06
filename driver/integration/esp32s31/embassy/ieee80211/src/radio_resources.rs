//! Role-neutral network and aggregate-TX storage for the sole Wi-Fi runner.
//!
//! STA and AP own distinct RX/link/MAC endpoints and share one tagged physical
//! TX arena. All of that allocation belongs to the integration root, not to a
//! station `connected` transaction.

#[cfg(feature = "tx-psram-dma-probe")]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::resources::profile::{
    ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
    ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
};
#[cfg(feature = "owned-network")]
use crate::resources::profile::{
    ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH as NETWORK_OWNER_TX_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_PACKET_POOL_CAPACITY as NETWORK_PACKET_POOL_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_RX_PACKET_POOL_CAPACITY as NETWORK_RX_PACKET_POOL_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH as NETWORK_RX_QUEUE_DEPTH,
};
#[cfg(feature = "compat-network")]
use embassy_net_compat as embassy_net;
#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;
#[cfg(feature = "owned-network")]
use embassy_net_owned::{PacketBufAllocator, PacketPool, PacketPoolStorage};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(feature = "owned-network")]
use open_esp_radio_embassy_net::{OwnedEndpointResources, OwnedNetworkDevice, OwnedNetworkTxFrame};
#[cfg(feature = "compat-network")]
use open_esp_radio_embassy_net_compat::{
    Device as CompatibilityNetworkDevice, FrameStorage as CompatibilityFrameStorage,
    Resources as CompatibilityEndpointResources,
};
use open_esp_radio_esp32s31_wifi_dma::tx_ampdu_storage::AmpduDmaStorage;
#[cfg(feature = "owned-network")]
use open_esp_radio_esp32s31_wifi_embassy::datapath::network::DualOwnedDatapathNetwork;
use open_esp_radio_esp32s31_wifi_embassy::{
    datapath::tx::resources::AggregateTxResources,
    datapath::{PinnedTxConsumer, PinnedTxFrame, PinnedTxPool, PinnedTxResources},
};
#[cfg(feature = "compat-network")]
use open_esp_radio_esp32s31_wifi_embassy_compat::{
    CompatibilityTxFrame, DualCompatibilityDatapathNetwork,
};
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::{
    HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage,
    TX_AMPDU_METADATA_SIZE,
};
use open_esp_radio_wifi_embassy::station_network::{
    RunningStationNetwork, StationNetworkResources,
};
use static_cell::{ConstStaticCell, StaticCell};

pub(super) const NETWORK_TX_HEADROOM: usize =
    TX_AMPDU_METADATA_SIZE + open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
// The protected MPDU starts immediately after the aggregate metadata and must
// remain naturally aligned for the hardware TX path.
const _: () = assert!(TX_AMPDU_METADATA_SIZE.is_multiple_of(core::mem::align_of::<u32>()));
pub(super) const TX_AMPDU_BUFFER_SIZE: usize = 0;

/// Complete-frame software horizon for the unchanged Embassy driver.
///
/// This is deliberately smaller than the optimized owner pool. Payload slots
/// live in separate general-memory arenas and circulate through the hot
/// channels as unique leases; the compatibility track still publishes an
/// explicit bounded-RAM/extra-copy envelope rather than pretending to be the
/// fast path.
#[cfg(feature = "compat-network")]
pub const ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH: usize = 16;

#[cfg(feature = "compat-network")]
type CompatMonitor = open_esp_radio_embassy_net_compat::ResourceMonitor<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH,
>;
#[cfg(feature = "compat-network")]
static STATION_COMPAT_MONITOR: embassy_sync::blocking_mutex::Mutex<
    CriticalSectionRawMutex,
    core::cell::RefCell<Option<CompatMonitor>>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(None));

/// Observe station queue ownership even if radio service is awaiting capacity.
#[cfg(feature = "compat-network")]
pub fn station_compat_resources() -> Option<open_esp_radio_embassy_net_compat::ResourceSnapshot> {
    STATION_COMPAT_MONITOR.lock(|monitor| monitor.borrow().as_ref().map(CompatMonitor::snapshot))
}

#[cfg(feature = "owned-network")]
type NetworkResources = OwnedEndpointResources<
    CriticalSectionRawMutex,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
>;
#[cfg(feature = "compat-network")]
type NetworkResources = CompatibilityEndpointResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH,
>;
#[cfg(feature = "compat-network")]
type NetworkFrameStorage =
    CompatibilityFrameStorage<NETWORK_FRAME_CAPACITY, ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH>;
type NetworkTxResources = PinnedTxResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkTxPool = PinnedTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type PhysicalTxConsumer = PinnedTxConsumer<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;

pub(super) type RadioTxBacking = PinnedTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
#[cfg(feature = "owned-network")]
pub(super) type RadioNetworkTxBacking = OwnedNetworkTxFrame;
#[cfg(feature = "compat-network")]
pub(super) type RadioNetworkTxBacking = CompatibilityTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH,
>;
type RadioAmpduRetention = RetainedAmpduDmaStorage<RadioTxBacking, TX_AMPDU_FRAME_COUNT>;

#[cfg(feature = "owned-network")]
pub type Esp32s31WifiNetworkDevice = OwnedNetworkDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
>;
#[cfg(feature = "compat-network")]
pub type Esp32s31WifiNetworkDevice = CompatibilityNetworkDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH,
>;

/// Application-side Wi-Fi device plus the general packet allocator consumed
/// exactly once when Embassy constructs its Xarxa stack.
pub struct Esp32s31WifiDevice {
    pub(crate) inner: Esp32s31WifiNetworkDevice,
    #[cfg(feature = "owned-network")]
    pub(crate) packet_allocator: PacketBufAllocator,
}

#[cfg(feature = "compat-network")]
impl Esp32s31WifiDevice {
    /// Transfer the unique released Embassy device to application-owned stack composition.
    pub fn into_compat(self) -> Esp32s31WifiNetworkDevice {
        self.inner
    }
}
#[cfg(feature = "owned-network")]
impl Esp32s31WifiDevice {
    /// Transfer the unique owned-packet device and its matching allocator together.
    pub fn into_owned(self) -> (Esp32s31WifiNetworkDevice, PacketBufAllocator) {
        (self.inner, self.packet_allocator)
    }
}

#[cfg(not(feature = "upstream-network"))]
impl Esp32s31WifiDevice {
    /// Select software IPv4/UDP validation for received packets.
    pub fn with_software_ipv4_udp_rx_checksum_validation(mut self, enabled: bool) -> Self {
        use embassy_net::driver::{Checksum, ChecksumCapabilities};

        let mut checksum = ChecksumCapabilities::default();
        if !enabled {
            checksum.ipv4 = Checksum::Tx;
            checksum.udp = Checksum::Tx;
        }
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }

    /// Select software generation of IPv4 UDP checksums.
    pub fn with_software_ipv4_udp_tx_checksum_generation(mut self, enabled: bool) -> Self {
        use embassy_net::driver::{Checksum, Driver as _};

        let mut checksum = self.inner.capabilities().checksum;
        let validate_rx = matches!(checksum.udp, Checksum::Both | Checksum::Rx);
        checksum.udp = match (validate_rx, enabled) {
            (true, true) => Checksum::Both,
            (true, false) => Checksum::Rx,
            (false, true) => Checksum::Tx,
            (false, false) => Checksum::None,
        };
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }
}

#[cfg(feature = "owned-network")]
pub type Esp32s31WifiStackResources = embassy_net::StackResources<Esp32s31WifiNetworkDevice>;
#[cfg(feature = "compat-network")]
pub type Esp32s31WifiStackResources = embassy_net::StackResources<16>;

/// Permanent application-side devices for the two logical Wi-Fi interfaces.
/// Each device owns independent IP/link/RX state while both publish into the
/// one physical tagged TX fabric.
pub struct Esp32s31WifiDevices {
    pub station: Esp32s31WifiDevice,
    pub access_point: Esp32s31WifiDevice,
}
#[cfg(feature = "owned-network")]
pub(super) type RadioNetworkRunner = DualOwnedDatapathNetwork<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
#[cfg(feature = "compat-network")]
pub(super) type RadioNetworkRunner = DualCompatibilityDatapathNetwork<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    ESP32S31_COMPAT_NETWORK_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
pub(super) type NetworkRunner = &'static mut RadioNetworkRunner;
pub(super) type RadioAmpduStorage =
    AggregateTxResources<'static, RadioTxBacking, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
pub type WifiNetworkResources = StationNetworkResources<(), NetworkRunner, ()>;
pub(super) type RunningWifiNetwork = RunningStationNetwork<(), NetworkRunner>;

#[cfg(not(feature = "upstream-network"))]
static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
#[cfg(not(feature = "upstream-network"))]
static ACCESS_POINT_NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
#[cfg(feature = "compat-network")]
#[allow(
    unsafe_code,
    reason = "compatibility payload storage is explicitly placed in the general PSRAM tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_compat_station_rx")]
static STATION_COMPAT_RX_STORAGE: ConstStaticCell<NetworkFrameStorage> =
    ConstStaticCell::new(NetworkFrameStorage::new());
#[cfg(feature = "compat-network")]
#[allow(
    unsafe_code,
    reason = "compatibility payload storage is explicitly placed in the general PSRAM tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_compat_station_tx")]
static STATION_COMPAT_TX_STORAGE: ConstStaticCell<NetworkFrameStorage> =
    ConstStaticCell::new(NetworkFrameStorage::new());
#[cfg(feature = "compat-network")]
#[allow(
    unsafe_code,
    reason = "compatibility payload storage is explicitly placed in the general PSRAM tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_compat_ap_rx")]
static ACCESS_POINT_COMPAT_RX_STORAGE: ConstStaticCell<NetworkFrameStorage> =
    ConstStaticCell::new(NetworkFrameStorage::new());
#[cfg(feature = "compat-network")]
#[allow(
    unsafe_code,
    reason = "compatibility payload storage is explicitly placed in the general PSRAM tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_compat_ap_tx")]
static ACCESS_POINT_COMPAT_TX_STORAGE: ConstStaticCell<NetworkFrameStorage> =
    ConstStaticCell::new(NetworkFrameStorage::new());
static NETWORK_TX_RESOURCES: ConstStaticCell<NetworkTxResources> =
    ConstStaticCell::new(NetworkTxResources::new());
static NETWORK_RUNNER: StaticCell<RadioNetworkRunner> = StaticCell::new();

#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "network packet payloads are explicitly placed in the PSRAM ownership tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_station_network_packets")]
static STATION_NETWORK_PACKET_STORAGE: ConstStaticCell<
    PacketPoolStorage<NETWORK_PACKET_POOL_CAPACITY>,
> = ConstStaticCell::new(PacketPoolStorage::new());
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "network packet payloads are explicitly placed in the PSRAM ownership tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_ap_network_packets")]
static ACCESS_POINT_NETWORK_PACKET_STORAGE: ConstStaticCell<
    PacketPoolStorage<NETWORK_PACKET_POOL_CAPACITY>,
> = ConstStaticCell::new(PacketPoolStorage::new());
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "RX packet payloads are explicitly separated from the physical DMA staging tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_station_rx_packets")]
static STATION_RX_PACKET_STORAGE: ConstStaticCell<
    PacketPoolStorage<NETWORK_RX_PACKET_POOL_CAPACITY>,
> = ConstStaticCell::new(PacketPoolStorage::new());
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "RX packet payloads are explicitly separated from the physical DMA staging tier"
)]
#[unsafe(link_section = ".psram.bss.open_radio_ap_rx_packets")]
static ACCESS_POINT_RX_PACKET_STORAGE: ConstStaticCell<
    PacketPoolStorage<NETWORK_RX_PACKET_POOL_CAPACITY>,
> = ConstStaticCell::new(PacketPoolStorage::new());

#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "hot packet-pool ownership metadata is explicitly retained in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_station_network_pool")]
static STATION_NETWORK_PACKET_POOL: StaticCell<PacketPool<NETWORK_PACKET_POOL_CAPACITY>> =
    StaticCell::new();
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "hot packet-pool ownership metadata is explicitly retained in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_ap_network_pool")]
static ACCESS_POINT_NETWORK_PACKET_POOL: StaticCell<PacketPool<NETWORK_PACKET_POOL_CAPACITY>> =
    StaticCell::new();
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "hot packet-pool ownership metadata is explicitly retained in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_station_rx_pool")]
static STATION_RX_PACKET_POOL: StaticCell<PacketPool<NETWORK_RX_PACKET_POOL_CAPACITY>> =
    StaticCell::new();
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "hot packet-pool ownership metadata is explicitly retained in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_ap_rx_pool")]
static ACCESS_POINT_RX_PACKET_POOL: StaticCell<PacketPool<NETWORK_RX_PACKET_POOL_CAPACITY>> =
    StaticCell::new();
#[allow(
    unsafe_code,
    reason = "the linker must retain production network TX backing in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_network_tx")]
static NETWORK_TX_POOL: ConstStaticCell<NetworkTxPool> = ConstStaticCell::new(NetworkTxPool::new());

#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_PROBE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_PREPARES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_LAST_ADDRESS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "tx-psram-dma-probe")]
#[allow(
    unsafe_code,
    reason = "the diagnostic pool must occupy the cached PSRAM aperture before its explicit cache writeback"
)]
#[unsafe(link_section = ".psram.bss.open_radio_network_tx_dma_probe")]
static PSRAM_NETWORK_TX_POOL: ConstStaticCell<NetworkTxPool> =
    ConstStaticCell::new(NetworkTxPool::new());

/// Select a same-image experiment where Wi-Fi A-MPDU descriptors reference
/// PSRAM packet buffers directly. Descriptors remain in internal SRAM.
#[cfg(feature = "tx-psram-dma-probe")]
pub fn configure_direct_psram_tx_dma_probe(enabled: bool) {
    DIRECT_PSRAM_TX_DMA_PREPARES.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_PROBE.store(enabled, Ordering::Release);
}

#[cfg(feature = "tx-psram-dma-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectPsramTxDmaProbeObservation {
    pub prepares: u32,
    pub first_address: u32,
    pub last_address: u32,
}

#[cfg(feature = "tx-psram-dma-probe")]
pub fn direct_psram_tx_dma_probe_observation() -> DirectPsramTxDmaProbeObservation {
    DirectPsramTxDmaProbeObservation {
        prepares: DIRECT_PSRAM_TX_DMA_PREPARES.load(Ordering::Acquire),
        first_address: DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.load(Ordering::Acquire),
        last_address: DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.load(Ordering::Acquire),
    }
}

#[cfg(feature = "tx-psram-dma-probe")]
fn prepare_psram_for_wifi_dma_read(storage: &mut [u8]) {
    let address = storage.as_ptr() as usize;
    let end = address
        .checked_add(storage.len())
        .expect("direct Wi-Fi TX DMA probe range must not wrap");
    assert!(
        address >= 0x5000_0000 && end <= 0x5400_0000,
        "direct Wi-Fi TX DMA probe backing must reside in PSRAM"
    );
    // The diagnostic pool isolates every slot on 64-byte cache-line
    // boundaries. The ownership callback writes dirty CPU data back and
    // writes those complete lines back before they become DMA-owned.
    open_esp_radio_esp32s31_platform_pac::writeback_psram_for_dma_read(storage)
        .expect("direct Wi-Fi TX DMA probe cache writeback must accept its PSRAM slot");
    let address = address as u32;
    let _ = DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.compare_exchange(
        0,
        address,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.store(address, Ordering::Release);
    DIRECT_PSRAM_TX_DMA_PREPARES.fetch_add(1, Ordering::Relaxed);
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
}

static TX_AMPDU_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
#[allow(
    unsafe_code,
    reason = "the linker must retain production A-MPDU descriptors in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu_descriptors")]
static TX_AMPDU_DMA_STORAGE: ConstStaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    ConstStaticCell::new(AmpduDmaStorage::new());
static TX_AMPDU_RETENTION: ConstStaticCell<RadioAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());

static TX_AMPDU_STANDBY_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
#[allow(
    unsafe_code,
    reason = "the linker must retain standby A-MPDU descriptors in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu_standby_descriptors")]
static TX_AMPDU_STANDBY_DMA_STORAGE: ConstStaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    ConstStaticCell::new(AmpduDmaStorage::new());
static TX_AMPDU_STANDBY_RETENTION: ConstStaticCell<RadioAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());

fn initialize_physical_tx() -> PhysicalTxConsumer {
    let network_tx_resources = NETWORK_TX_RESOURCES.take();
    #[cfg(feature = "tx-psram-dma-probe")]
    let tx_pool = if DIRECT_PSRAM_TX_DMA_PROBE.load(Ordering::Acquire) {
        NetworkTxPool::pin_static_with_dma_read_prepare(
            PSRAM_NETWORK_TX_POOL.take(),
            prepare_psram_for_wifi_dma_read,
        )
    } else {
        NetworkTxPool::pin_static(NETWORK_TX_POOL.take())
    };
    #[cfg(not(feature = "tx-psram-dma-probe"))]
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.take());
    network_tx_resources.split(tx_pool)
}

#[cfg(feature = "owned-network")]
pub(crate) fn initialize_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> (Esp32s31WifiDevices, WifiNetworkResources) {
    let station_resources = NETWORK_RESOURCES.take();
    let access_point_resources = ACCESS_POINT_NETWORK_RESOURCES.take();
    let station_packet_allocator = STATION_NETWORK_PACKET_POOL
        .init(PacketPool::new(STATION_NETWORK_PACKET_STORAGE.take()))
        .allocator();
    let access_point_packet_allocator = ACCESS_POINT_NETWORK_PACKET_POOL
        .init(PacketPool::new(ACCESS_POINT_NETWORK_PACKET_STORAGE.take()))
        .allocator();
    let station_rx_allocator = STATION_RX_PACKET_POOL
        .init(PacketPool::new(STATION_RX_PACKET_STORAGE.take()))
        .allocator();
    let access_point_rx_allocator = ACCESS_POINT_RX_PACKET_POOL
        .init(PacketPool::new(ACCESS_POINT_RX_PACKET_STORAGE.take()))
        .allocator();
    let tx_consumer = initialize_physical_tx();
    let station_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID;
    let access_point_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::AP_NETWORK_INTERFACE_ID;
    let (station_device, station_runner) =
        station_resources.split(station_interface, station_address, station_rx_allocator);
    let (access_point_device, access_point_runner) = access_point_resources.split(
        access_point_interface,
        access_point_address,
        access_point_rx_allocator,
    );
    let runner = DualOwnedDatapathNetwork::new(station_runner, access_point_runner, tx_consumer);
    let runner = NETWORK_RUNNER.init(runner);
    (
        Esp32s31WifiDevices {
            station: Esp32s31WifiDevice {
                inner: station_device,
                packet_allocator: station_packet_allocator,
            },
            access_point: Esp32s31WifiDevice {
                inner: access_point_device,
                packet_allocator: access_point_packet_allocator,
            },
        },
        WifiNetworkResources::Unstarted { device: (), runner },
    )
}

#[cfg(feature = "compat-network")]
pub(crate) fn initialize_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> (Esp32s31WifiDevices, WifiNetworkResources) {
    let station_resources = NETWORK_RESOURCES.take();
    let access_point_resources = ACCESS_POINT_NETWORK_RESOURCES.take();
    let station_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID;
    let access_point_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::AP_NETWORK_INTERFACE_ID;
    let (station_device, station_runner) = station_resources.split(
        station_address,
        STATION_COMPAT_RX_STORAGE.take(),
        STATION_COMPAT_TX_STORAGE.take(),
    );
    STATION_COMPAT_MONITOR
        .lock(|monitor| *monitor.borrow_mut() = Some(station_runner.resource_monitor()));
    let (access_point_device, access_point_runner) = access_point_resources.split(
        access_point_address,
        ACCESS_POINT_COMPAT_RX_STORAGE.take(),
        ACCESS_POINT_COMPAT_TX_STORAGE.take(),
    );
    let runner = DualCompatibilityDatapathNetwork::new(
        station_interface,
        station_runner,
        access_point_interface,
        access_point_runner,
        initialize_physical_tx(),
    );
    let runner = NETWORK_RUNNER.init(runner);
    (
        Esp32s31WifiDevices {
            station: Esp32s31WifiDevice {
                inner: station_device,
            },
            access_point: Esp32s31WifiDevice {
                inner: access_point_device,
            },
        },
        WifiNetworkResources::Unstarted { device: (), runner },
    )
}

pub(super) fn initialize_ampdu() -> Result<RadioAmpduStorage, HtAmpduTxError> {
    Ok(AggregateTxResources::pipelined(
        HtAmpduTxResources::pin_static(TX_AMPDU_STORAGE.take(), TX_AMPDU_DMA_STORAGE.take())?,
        TX_AMPDU_RETENTION.take(),
        HtAmpduTxResources::pin_static(
            TX_AMPDU_STANDBY_STORAGE.take(),
            TX_AMPDU_STANDBY_DMA_STORAGE.take(),
        )?,
        TX_AMPDU_STANDBY_RETENTION.take(),
    ))
}

#[cfg(feature = "upstream-network")]
mod upstream;
#[cfg(feature = "upstream-network")]
use upstream::RadioNetworkRunner;
#[cfg(feature = "upstream-network")]
pub(crate) use upstream::initialize_network;
#[cfg(feature = "upstream-network")]
pub use upstream::{Esp32s31WifiNetworkDevice, station_rx_pool_drops};
#[cfg(feature = "upstream-network")]
pub(super) type RadioNetworkTxBacking =
    open_esp_radio_xarxa_upstream::TxFrame<'static, CriticalSectionRawMutex>;

#[cfg(feature = "upstream-network")]
impl Esp32s31WifiDevice {
    /// Transfer the original Xarxa driver to the application's upstream stack.
    /// The application owns stack storage, IP configuration and socket policy.
    pub fn into_upstream(self) -> Esp32s31WifiNetworkDevice {
        self.inner
    }
}
