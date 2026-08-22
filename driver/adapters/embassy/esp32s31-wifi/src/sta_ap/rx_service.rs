//! Finite protocol service over the sole same-channel STA+AP RX producer.

use core::future::Future;

use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDma, RxError},
    rx_pool::{
        RxStageTransactionError, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
    },
};
use open_esp_radio_ieee80211::vif::StaApRxRoute;

use super::{Esp32s31RoutedRxDisposition, Esp32s31StaApRxConsumer, Esp32s31StaApRxTurn};
use crate::{
    connected_rx_protocol::Esp32s31StagedRxFrame,
    embassy_rx::RxDmaObservationDelay,
    rx_dma_service::Esp32s31StagedRxEpoch,
    wdev::{
        WdevNetworkRx, WdevRxProgress, WdevRxServiceContext,
        paired::{WdevPairRole, WdevPairedRxProgress, WdevPairedRxService},
    },
};

const SATURATED_DMA_REFILL_QUANTUM_FRAMES: usize = 4;
/// Protocol leases consumed between physical-TX completion checks.
const ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES: usize = 4;

/// Connected-station protocol authority accepted by the common RX owner.
///
/// The role receives only an already-classified staging lease and its own
/// addressed network endpoint. It cannot inspect the AP endpoint or reclaim
/// DMA descriptors.
pub trait Esp32s31StaApStationRxRole<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>
{
    type Dispatch;
    type Error;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn WdevNetworkRx,
    ) -> Result<WdevRxProgress, Self::Error>;

    fn service_station_rx<'a>(
        &'a mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        network: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a;

    fn has_pending_rx(&self) -> bool;
}

/// SoftAP protocol authority accepted by the common RX owner.
///
/// AP publication can retain a decoded Ethernet batch independently of the
/// staging lease. The explicit publication edge lets the common scheduler
/// stop at network backpressure without scanning past the ordered head.
pub trait Esp32s31StaApAccessPointRxRole<
    'pool,
    H,
    PhysicalTx,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>
{
    type Error;

    fn publish_pending_rx(
        &mut self,
        physical_tx: &mut PhysicalTx,
        network: &mut dyn WdevNetworkRx,
    ) -> Result<WdevRxProgress, Self::Error>;

    fn service_access_point_rx(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<
        Esp32s31RoutedRxDisposition<Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>>,
        Self::Error,
    >;

    /// Consume protected AP data without borrowing the physical TX or MMIO
    /// owners. Any resulting hardware/control request remains value-only in
    /// the role mailbox until the active transaction reaches a terminal edge.
    fn service_access_point_rx_during_tx(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<
        Esp32s31RoutedRxDisposition<Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>>,
        Self::Error,
    >;

    fn has_pending_rx(&self) -> bool;

    /// True only while this AP role owns a hardware TX transaction created by
    /// its RX protocol step.
    fn tx_pending(&self) -> bool;
}

/// Origin-preserving failure from one common RX service turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApRxServiceError<StationError, AccessPointError> {
    Dma(RxStageTransactionError),
    Station(StationError),
    AccessPoint(AccessPointError),
}

/// Fact-only outcomes observed at the common STA+AP RX demultiplexer.
///
/// The vendor retains an interface-context pointer on each packet and routes
/// it by comparing that pointer with the active STA context. The open driver
/// has no reviewed equivalent of that private pointer, so it classifies the
/// public IEEE 802.11 header instead. These counters make that replacement
/// boundary observable without assigning protocol meaning to rejected data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31StaApRxRouteReport {
    pub station: u64,
    pub access_point: u64,
    pub foreign: u64,
    pub ambiguous: u64,
    pub malformed: u64,
    pub hardware_error: u64,
}

impl Esp32s31StaApRxRouteReport {
    pub const fn new() -> Self {
        Self {
            station: 0,
            access_point: 0,
            foreign: 0,
            ambiguous: 0,
            malformed: 0,
            hardware_error: 0,
        }
    }

    pub const fn total(self) -> u64 {
        self.station
            .saturating_add(self.access_point)
            .saturating_add(self.foreign)
            .saturating_add(self.ambiguous)
            .saturating_add(self.malformed)
            .saturating_add(self.hardware_error)
    }

    fn record_rejected(&mut self, classification: Result<StaApRxRoute, RxError>) {
        match classification {
            Ok(StaApRxRoute::Foreign) => self.foreign = self.foreign.saturating_add(1),
            Ok(StaApRxRoute::Ambiguous) => self.ambiguous = self.ambiguous.saturating_add(1),
            Ok(StaApRxRoute::Malformed) => self.malformed = self.malformed.saturating_add(1),
            Ok(StaApRxRoute::Interface(_)) => {
                unreachable!("an addressed STA+AP frame cannot enter the rejected arm")
            }
            Err(_) => self.hardware_error = self.hardware_error.saturating_add(1),
        }
    }
}

/// Sole live DMA epoch plus the sole ordered STA/AP protocol consumer.
///
/// Role processors are deliberately not stored here. The outer paired WDEV
/// owns those processors together with their TX/control state and lends them
/// to one finite RX turn as disjoint mutable capabilities.
pub struct Esp32s31StaApRxService<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    dma: Esp32s31StagedRxEpoch<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    protocol: Esp32s31StaApRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    serviced_frames: u64,
    routes: Esp32s31StaApRxRouteReport,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31StaApRxService<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn new(
        dma: Esp32s31StagedRxEpoch<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        protocol: Esp32s31StaApRxConsumer<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
        >,
    ) -> Self {
        Self {
            dma,
            protocol,
            serviced_frames: 0,
            routes: Esp32s31StaApRxRouteReport::new(),
        }
    }

    pub const fn dma(
        &self,
    ) -> &Esp32s31StagedRxEpoch<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        &self.dma
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>
    where
        D: RxDmaObservationDelay,
    {
        self.dma.start(hardware).await
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>
    where
        D: RxDmaObservationDelay,
    {
        self.dma.stop(hardware)
    }

    pub const fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    pub const fn route_report(&self) -> Esp32s31StaApRxRouteReport {
        self.routes
    }

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31StagedRxEpoch<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        Esp32s31StaApRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        (self.dma, self.protocol)
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M,
    H,
    PhysicalTx,
    Station,
    AccessPoint,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> WdevPairedRxService<H, PhysicalTx, Station, AccessPoint>
    for Esp32s31StaApRxService<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    M: RawMutex,
    D: RxDmaObservationDelay,
    H: RxDma,
    Station: Esp32s31StaApStationRxRole<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    AccessPoint: Esp32s31StaApAccessPointRxRole<'pool, H, PhysicalTx, STAGE_CAPACITY, STAGE_SLOTS>,
{
    type Error = Esp32s31StaApRxServiceError<Station::Error, AccessPoint::Error>;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        station_network: &'a mut dyn WdevNetworkRx,
        access_point_network: &'a mut dyn WdevNetworkRx,
        context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevPairedRxProgress, Self::Error>> + 'a {
        async move {
            let dma_progress = self
                .dma
                .service(hardware)
                .await
                .map_err(Esp32s31StaApRxServiceError::Dma)?;

            if station
                .publish_pending_rx(station_network)
                .map_err(Esp32s31StaApRxServiceError::Station)?
                == WdevRxProgress::NetworkBackpressured
            {
                return Ok(WdevRxProgress::NetworkBackpressured.into());
            }
            if access_point
                .publish_pending_rx(physical_tx, access_point_network)
                .map_err(Esp32s31StaApRxServiceError::AccessPoint)?
                == WdevRxProgress::NetworkBackpressured
            {
                return Ok(WdevRxProgress::NetworkBackpressured.into());
            }

            let limit = context
                .maximum_protocol_frames
                .unwrap_or(STAGE_SLOTS)
                .max(1);
            // If the first producer pass filled the complete staging owner,
            // do not wait until the whole protocol batch and its cooperative
            // yield have finished before returning another descriptor to
            // hardware. A small frame quantum amortizes the cursor/reload
            // transaction without letting a complete negotiated 16-MPDU BA
            // window pass between refill opportunities. This preserves a
            // single DMA owner while closing the producer/consumer latency
            // gap that otherwise leaves the hardware ring as the only
            // remaining ingress reserve.
            let mut refill_saturated_dma = self.protocol.queued_frames() >= STAGE_SLOTS;
            let mut consumed_since_dma_refill = 0_usize;
            for _ in 0..limit {
                let turn = self
                    .protocol
                    .service_next(
                        |frame| station.service_station_rx(frame, station_network),
                        |frame| access_point.service_access_point_rx(hardware, physical_tx, frame),
                    )
                    .await
                    .map_err(Esp32s31StaApRxServiceError::AccessPoint)?;
                match turn {
                    Esp32s31StaApRxTurn::Idle => return Ok(dma_progress.into()),
                    Esp32s31StaApRxTurn::Station(dispatch) => {
                        dispatch.map_err(Esp32s31StaApRxServiceError::Station)?;
                        self.routes.station = self.routes.station.saturating_add(1);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                    }
                    Esp32s31StaApRxTurn::AccessPoint => {
                        self.routes.access_point = self.routes.access_point.saturating_add(1);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                    }
                    Esp32s31StaApRxTurn::DeferredAccessPoint => {
                        return Ok(if access_point.tx_pending() {
                            WdevPairedRxProgress::TxPending(WdevPairRole::Second)
                        } else {
                            WdevRxProgress::ProbePending.into()
                        });
                    }
                    Esp32s31StaApRxTurn::Rejected(classification) => {
                        self.routes.record_rejected(classification);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                    }
                }

                if station
                    .publish_pending_rx(station_network)
                    .map_err(Esp32s31StaApRxServiceError::Station)?
                    == WdevRxProgress::NetworkBackpressured
                {
                    return Ok(WdevRxProgress::NetworkBackpressured.into());
                }
                if access_point
                    .publish_pending_rx(physical_tx, access_point_network)
                    .map_err(Esp32s31StaApRxServiceError::AccessPoint)?
                    == WdevRxProgress::NetworkBackpressured
                {
                    return Ok(WdevRxProgress::NetworkBackpressured.into());
                }
                consumed_since_dma_refill = consumed_since_dma_refill.saturating_add(1);
                if refill_saturated_dma
                    && consumed_since_dma_refill >= SATURATED_DMA_REFILL_QUANTUM_FRAMES
                {
                    refill_saturated_dma = self
                        .dma
                        .service(hardware)
                        .await
                        .map_err(Esp32s31StaApRxServiceError::Dma)?
                        != WdevRxProgress::Drained;
                    consumed_since_dma_refill = 0;
                }
                if access_point.tx_pending() {
                    return Ok(WdevPairedRxProgress::TxPending(WdevPairRole::Second));
                }
            }
            Ok(WdevRxProgress::ProbePending.into())
        }
    }

    fn service_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        station_network: &'a mut dyn WdevNetworkRx,
        access_point_network: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            // First release the complete hardware frontier. Upper protocol
            // work below owns only staging leases and value-only mailboxes.
            let dma_progress = self
                .dma
                .service(hardware)
                .await
                .map_err(Esp32s31StaApRxServiceError::Dma)?;

            if station
                .publish_pending_rx(station_network)
                .map_err(Esp32s31StaApRxServiceError::Station)?
                == WdevRxProgress::NetworkBackpressured
                || access_point
                    .publish_pending_rx(physical_tx, access_point_network)
                    .map_err(Esp32s31StaApRxServiceError::AccessPoint)?
                    == WdevRxProgress::NetworkBackpressured
            {
                return Ok(WdevRxProgress::NetworkBackpressured);
            }

            for _ in 0..ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES {
                let turn = self
                    .protocol
                    .service_next(
                        |frame| station.service_station_rx(frame, station_network),
                        |frame| access_point.service_access_point_rx_during_tx(frame),
                    )
                    .await
                    .map_err(Esp32s31StaApRxServiceError::AccessPoint)?;
                match turn {
                    Esp32s31StaApRxTurn::Idle => return Ok(dma_progress),
                    Esp32s31StaApRxTurn::Station(dispatch) => {
                        dispatch.map_err(Esp32s31StaApRxServiceError::Station)?;
                        self.routes.station = self.routes.station.saturating_add(1);
                    }
                    Esp32s31StaApRxTurn::AccessPoint => {
                        self.routes.access_point = self.routes.access_point.saturating_add(1);
                    }
                    Esp32s31StaApRxTurn::DeferredAccessPoint => return Ok(dma_progress),
                    Esp32s31StaApRxTurn::Rejected(classification) => {
                        self.routes.record_rejected(classification);
                    }
                }
                self.serviced_frames = self.serviced_frames.saturating_add(1);
            }

            Ok(if self.protocol.queued_frames() == 0 {
                dma_progress
            } else {
                WdevRxProgress::ProbePending
            })
        }
    }

    fn has_work(&self, station: &Station, access_point: &AccessPoint) -> bool {
        self.protocol.queued_frames() != 0
            || station.has_pending_rx()
            || access_point.has_pending_rx()
    }

    fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }
}
