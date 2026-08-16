//! Role-neutral network and aggregate-TX storage for the sole Wi-Fi runner.
//!
//! STA and AP borrow the same physical arenas in disjoint lifecycle epochs.
//! Their allocation therefore belongs to the integration root, not to the
//! station `connected` transaction.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::esp32s31::wifi::{
    dma::tx_ampdu_storage::AmpduDmaStorage,
    mac::tx_ampdu::{
        HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage,
    },
};
use open_esp_radio_embassy_net::{
    PinnedTxFrame, PinnedTxPool, SharedPinnedRxConsumer, SharedRxSplitPinnedDevice,
    SplitPinnedRadioRunner, SplitPinnedResources,
};
use open_esp_radio_esp32s31_wifi_embassy::{
    ampdu_resources::AggregateTxResources,
    resource_profile::{
        ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
        ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH as NETWORK_RX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT as RX_STAGE_SLOT_COUNT,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
    },
};
use open_esp_radio_wifi_embassy::station_network::{RunningStationNetwork, StationNetworkResources};
use static_cell::ConstStaticCell;

pub(super) const NETWORK_TX_HEADROOM: usize =
    8 + open_esp_radio::wifi::ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
pub(super) const TX_AMPDU_BUFFER_SIZE: usize = 0;

type NetworkResources = SplitPinnedResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
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
pub(super) type NetworkRunner = SplitPinnedRadioRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
pub(super) type RadioAmpduStorage = AggregateTxResources<
    'static,
    RadioTxBacking,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
>;
pub type WifiNetworkResources = StationNetworkResources<(), NetworkRunner, ()>;
pub(super) type RunningWifiNetwork = RunningStationNetwork<(), NetworkRunner>;

static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
#[allow(
    unsafe_code,
    reason = "the linker must retain production network TX backing in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_network_tx")]
static NETWORK_TX_POOL: ConstStaticCell<NetworkTxPool> = ConstStaticCell::new(NetworkTxPool::new());

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
    shared: SharedPinnedRxConsumer<
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    >,
) -> (Esp32s31WifiDevice, WifiNetworkResources) {
    let network_resources = NETWORK_RESOURCES.take();
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.take());
    let (device, runner) = network_resources.split(tx_pool, station_address);
    (
        device.with_ingress_tx_reserve().with_shared_rx(shared),
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
