//! Shared physical Wi-Fi owners retained across role transitions.

use super::*;

type ProductionHaltedRx =
    open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>;
type ProductionLiveRx =
    open_esp_radio_esp32s31_wifi_mac::rx::RxRingLive<'static, RX_DESCRIPTOR_COUNT>;

pub(crate) enum ProductionRxRing {
    Halted(ProductionHaltedRx),
    Live(ProductionLiveRx),
}

impl ProductionRxRing {
    pub(super) fn into_scan(self, storage: &'static RxStorage) -> ProductionScanRx {
        match self {
            Self::Halted(ring) => Esp32s31ScanRx::from_halted(ring, storage),
            Self::Live(ring) => Esp32s31ScanRx::from_live(ring, storage),
        }
    }
}
type ProductionScanRx =
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
pub(super) struct ProductionWifiPhysicalResources {
    pub(super) dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    pub(super) rx_ring: Option<ProductionRxRing>,
    pub(super) tx: ProductionOrdinaryTxResources,
    pub(super) aggregate_tx: RadioAmpduStorage,
}

impl ProductionWifiPhysicalResources {
    pub(super) const fn new(
        dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        rx_ring: Option<ProductionRxRing>,
        tx: ProductionOrdinaryTxResources,
        aggregate_tx: RadioAmpduStorage,
    ) -> Self {
        Self {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        Option<ProductionRxRing>,
        ProductionOrdinaryTxResources,
        RadioAmpduStorage,
    ) {
        (self.dma, self.rx_ring, self.tx, self.aggregate_tx)
    }

    pub(super) fn take_rx_ring(self) -> (Self, Option<ProductionRxRing>) {
        let Self {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        } = self;
        (
            Self {
                dma,
                rx_ring: None,
                tx,
                aggregate_tx,
            },
            rx_ring,
        )
    }

    pub(super) fn restore_rx_ring(self, rx_ring: ProductionRxRing) -> Self {
        let Self {
            dma,
            rx_ring: previous,
            tx,
            aggregate_tx,
        } = self;
        assert!(previous.is_none(), "physical RX ring is already present");
        Self {
            dma,
            rx_ring: Some(rx_ring),
            tx,
            aggregate_tx,
        }
    }
}
