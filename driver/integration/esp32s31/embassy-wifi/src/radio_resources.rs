//! Role-neutral network and aggregate-TX storage for the sole Wi-Fi runner.
//!
//! STA and AP own distinct RX/link/MAC endpoints and share one tagged physical
//! TX arena. All of that allocation belongs to the integration root, not to a
//! station `connected` transaction.

#[cfg(feature = "tx-psram-dma-probe")]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio_embassy_net::{
    DefaultEgressControlPlane, DefaultEgressControlledNetwork, DefaultEgressNetworkScheduler,
    DefaultEgressNetworkState, DefaultEgressRadioScheduler, DualPinnedNetworkRunner,
    EgressPeerDirectory, EgressPeerIdentity, NetworkEndpointConfig, PinnedEndpointResources,
    PinnedNetworkTxFrame, PinnedTxFrame, PinnedTxPool, PinnedTxResources, SharedPinnedRxConsumer,
    SharedRxSplitPinnedDevice,
};
use open_esp_radio_esp32s31_wifi_dma::tx_ampdu_storage::AmpduDmaStorage;
use open_esp_radio_esp32s31_wifi_embassy::{
    composition::resources::{
        ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
        ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH as NETWORK_RX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT as RX_STAGE_SLOT_COUNT,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
    },
    datapath::tx::resources::AggregateTxResources,
};
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::{
    HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage,
    TX_AMPDU_METADATA_SIZE,
};
use open_esp_radio_wifi_embassy::station_network::{
    RunningStationNetwork, StationNetworkResources,
};
use open_esp_radio_wifi_ap::{
    AP_MAX_CLIENTS, AccessPointServiceStatus, ApPeerPhase,
};
use static_cell::{ConstStaticCell, StaticCell};

pub(super) const NETWORK_TX_HEADROOM: usize =
    TX_AMPDU_METADATA_SIZE + open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
// The protected MPDU starts immediately after the aggregate metadata and must
// remain naturally aligned for the hardware TX path.
const _: () = assert!(TX_AMPDU_METADATA_SIZE.is_multiple_of(core::mem::align_of::<u32>()));
pub(super) const TX_AMPDU_BUFFER_SIZE: usize = 0;

type NetworkResources = PinnedEndpointResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_RX_QUEUE_DEPTH,
>;
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

pub(super) type RadioTxBacking = PinnedTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
pub(super) type RadioNetworkTxBacking = PinnedNetworkTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type RadioAmpduRetention = RetainedAmpduDmaStorage<RadioTxBacking, TX_AMPDU_FRAME_COUNT>;

pub type Esp32s31WifiDevice = SharedRxSplitPinnedDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;

/// Permanent application-side devices for the two logical Wi-Fi interfaces.
/// Each device owns independent IP/link/RX state while both publish into the
/// one physical tagged TX fabric.
pub struct Esp32s31WifiDevices {
    pub station: Esp32s31WifiDevice,
    pub access_point: Esp32s31WifiDevice,
}
pub(super) type RadioNetworkRunner = DualPinnedNetworkRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ControlledRadioNetworkRunner = DefaultEgressControlledNetwork<
    'static,
    CriticalSectionRawMutex,
    &'static RadioNetworkRunner,
>;
pub(super) type NetworkRunner = &'static mut ControlledRadioNetworkRunner;
type AccessPointEgressNetworkScheduler =
    DefaultEgressNetworkScheduler<'static, CriticalSectionRawMutex>;
type AccessPointEgressRadioScheduler =
    DefaultEgressRadioScheduler<'static, CriticalSectionRawMutex>;
pub(super) type RadioAmpduStorage =
    AggregateTxResources<'static, RadioTxBacking, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
pub type WifiNetworkResources = StationNetworkResources<(), NetworkRunner, ()>;
pub(super) type RunningWifiNetwork = RunningStationNetwork<(), NetworkRunner>;

static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
static ACCESS_POINT_NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
static NETWORK_TX_RESOURCES: ConstStaticCell<NetworkTxResources> =
    ConstStaticCell::new(NetworkTxResources::new());
static NETWORK_RUNNER: StaticCell<RadioNetworkRunner> = StaticCell::new();
static CONTROLLED_NETWORK_RUNNER: StaticCell<ControlledRadioNetworkRunner> = StaticCell::new();
static AP_EGRESS_PEERS: EgressPeerDirectory<AP_MAX_CLIENTS> = EgressPeerDirectory::new();
static AP_EGRESS_CONTROL: DefaultEgressControlPlane<CriticalSectionRawMutex> =
    DefaultEgressControlPlane::new();
static AP_EGRESS_NETWORK_STATE: ConstStaticCell<DefaultEgressNetworkState> =
    ConstStaticCell::new(DefaultEgressNetworkState::new());
static AP_EGRESS_NETWORK_SCHEDULER: StaticCell<AccessPointEgressNetworkScheduler> =
    StaticCell::new();
static AP_EGRESS_RADIO_SCHEDULER: StaticCell<AccessPointEgressRadioScheduler> = StaticCell::new();

#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn access_point_egress_control_snapshot() -> open_esp_radio_embassy_net::EgressControlSnapshot
{
    AP_EGRESS_CONTROL.snapshot()
}
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

pub(super) fn initialize_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
    station_shared: SharedPinnedRxConsumer<
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    >,
    access_point_shared: SharedPinnedRxConsumer<
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    >,
) -> (Esp32s31WifiDevices, WifiNetworkResources) {
    let station_resources = NETWORK_RESOURCES.take();
    let access_point_resources = ACCESS_POINT_NETWORK_RESOURCES.take();
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
    let (tx_provider, tx_consumer) = network_tx_resources.split(tx_pool);
    let station_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID;
    let access_point_interface =
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::AP_NETWORK_INTERFACE_ID;
    let (station_device, station_rx) = station_resources.split(
        tx_provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, station_address),
    );
    let access_point_endpoint = NetworkEndpointConfig::associated_peers(
        access_point_interface,
        access_point_address,
        &AP_EGRESS_PEERS,
    );
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    let access_point_endpoint = access_point_endpoint.with_shadow_grant(
        open_esp_radio_esp32s31_wifi_embassy::roles::access_point::access_point_egress_shadow_grant(),
    );
    let (access_point_device, access_point_rx) =
        access_point_resources.split(tx_provider, access_point_endpoint);
    let (access_point_egress_network, access_point_egress_radio) = AP_EGRESS_CONTROL.split();
    let access_point_egress_network = AP_EGRESS_NETWORK_SCHEDULER.init(
        DefaultEgressNetworkScheduler::new(
            access_point_egress_network,
            AP_EGRESS_NETWORK_STATE.take(),
        ),
    );
    let access_point_device = access_point_device.with_egress_control(access_point_egress_network);
    let runner = DualPinnedNetworkRunner::new(
        station_interface,
        station_rx,
        access_point_interface,
        access_point_rx,
        tx_consumer,
    )
    .with_shared_rx_ordering(&station_shared, &access_point_shared);
    let runner = NETWORK_RUNNER.init(runner);
    let access_point_egress_radio = AP_EGRESS_RADIO_SCHEDULER.init(
        DefaultEgressRadioScheduler::new(access_point_egress_radio),
    );
    let runner = CONTROLLED_NETWORK_RUNNER.init(
        DefaultEgressControlledNetwork::with_egress_control(
        &*runner,
        access_point_egress_radio,
        ),
    );
    #[cfg(feature = "tx-staging-copy-probe")]
    let access_point_device = access_point_device.with_tx_staging_copy_probe_selection();
    (
        Esp32s31WifiDevices {
            station: station_device
                .with_ingress_tx_reserve()
                .with_shared_rx(station_shared),
            access_point: access_point_device
                .with_ingress_tx_reserve()
                .with_shared_rx(access_point_shared),
        },
        WifiNetworkResources::Unstarted { device: (), runner },
    )
}

pub(crate) fn publish_access_point_egress_peers(status: &AccessPointServiceStatus) {
    let mut peers = [None; AP_MAX_CLIENTS];
    for peer in status
        .peers
        .iter()
        .flatten()
        .filter(|peer| peer.phase == ApPeerPhase::Authorized)
    {
        let identity = EgressPeerIdentity::try_new(
            peer.address,
            peer.association_id,
            peer.association_epoch,
        )
        .expect("an authorized AP peer has a non-zero bounded identity");
        let index = usize::from(identity.slot().get()) - 1;
        let destination = peers
            .get_mut(index)
            .expect("an AP association ID fits the published peer directory");
        assert!(destination.is_none(), "an AP slot has one current peer");
        *destination = Some(identity);
    }
    AP_EGRESS_PEERS
        .replace(&peers)
        .expect("AP egress peer publication generation is not reusable");
}

pub(crate) fn clear_access_point_egress_peers() {
    AP_EGRESS_PEERS
        .clear()
        .expect("AP egress peer publication generation is not reusable");
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    open_esp_radio_esp32s31_wifi_embassy::roles::access_point::access_point_egress_shadow_grant()
        .clear()
        .expect("AP egress shadow-grant publication generation is not reusable");
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
