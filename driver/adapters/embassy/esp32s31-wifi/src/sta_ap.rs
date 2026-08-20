//! Shared physical owners for one same-channel STA plus SoftAP composition.
//!
//! These types describe resources that exist once in hardware even though two
//! logical interfaces consume them. Role-local protocol state must not create
//! another physical owner.

use core::future::Future;

use embassy_sync::channel::{Channel, Receiver, Sender};
use open_esp_radio_dma::TaggedStableDmaBacking;
use open_esp_radio_embassy_net::{NetworkInterfaceId, PinnedRxPublisher, RawMutex};
use open_esp_radio_esp32s31_wifi_mac::MacInterface;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxError, RxIngressConfig, RxSegment, view_normalized_rx_frame,
};
use open_esp_radio_esp32s31_wifi_mac::rx_ampdu::RxBlockAckSessions;
use open_esp_radio_esp32s31_wifi_mac::rx_pool::{
    VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
};
use open_esp_radio_ieee80211::vif::{StaApRxAddresses, StaApRxRoute, StaApVif, classify_sta_ap_rx};

use crate::connected_rx_protocol::Esp32s31StagedRxFrame;
use crate::{
    embassy_irq::EmbassyMacIrqRuntime,
    wdev::{WdevInterfaceScope, WdevNetwork, WdevNetworkRxEndpoints, WdevRunner, WdevServices},
};

mod control;
mod rx_service;
mod station_role;
mod station_rx_role;

pub use control::{
    Esp32s31StaApAccessPointControlProgress, Esp32s31StaApAccessPointControlRole,
    Esp32s31StaApControlArbiter, Esp32s31StaApControlError, Esp32s31StaApControlExit,
    Esp32s31StaApStationControlRole,
};
pub use rx_service::{
    Esp32s31StaApAccessPointRxRole, Esp32s31StaApRxService, Esp32s31StaApRxServiceError,
    Esp32s31StaApStationRxRole,
};
pub use station_role::{
    Esp32s31StaApStationControlError, Esp32s31StaApStationFinishFailure,
    Esp32s31StaApStationFinishReason, Esp32s31StaApStationPrepareFailure,
    Esp32s31StaApStationPrepared, Esp32s31StaApStationRole, Esp32s31StaApStationTxError,
    Esp32s31StaApStationTxOwnershipError, finish_sta_ap_station, park_sta_ap_station_role,
    prepare_sta_ap_station,
};
pub use station_rx_role::{Esp32s31StaApStationRxError, Esp32s31StaApStationRxSink};

pub const STA_NETWORK_INTERFACE_ID: NetworkInterfaceId = NetworkInterfaceId::new(0);
pub const AP_NETWORK_INTERFACE_ID: NetworkInterfaceId = NetworkInterfaceId::new(1);

/// The one ordinary/A-MPDU hardware owner shared by both logical roles.
/// Role-local state may hold these resources only while this owner records
/// the matching lent role.
pub type Esp32s31StaApPhysicalTx<Ordinary, Aggregate> =
    crate::wdev::paired::WdevPairedPhysicalTx<Ordinary, Aggregate>;

/// Complete paired WDEV services graph before it is joined to the permanent
/// dual-interface network runner.
pub type Esp32s31StaApWdevServices<H, PhysicalTx, Rx, Station, AccessPoint> =
    crate::wdev::paired::WdevPairedServiceSet<
        H,
        PhysicalTx,
        Rx,
        Station,
        AccessPoint,
        Esp32s31StaApControlArbiter,
    >;

/// Compose the only supported station-plus-SoftAP role ordering.
///
/// Callers supply unique owners but cannot swap endpoint identities or attach
/// an unrelated control scheduler. Capability publication remains the
/// responsibility of the production runtime after hardware validation.
pub fn compose_sta_ap_wdev_services<H, PhysicalTx, Rx, Station, AccessPoint>(
    hardware: H,
    physical_tx: PhysicalTx,
    rx: Rx,
    station: Station,
    access_point: AccessPoint,
) -> Esp32s31StaApWdevServices<H, PhysicalTx, Rx, Station, AccessPoint> {
    crate::wdev::paired::WdevPairedServiceSet::new(
        STA_NETWORK_INTERFACE_ID,
        AP_NETWORK_INTERFACE_ID,
        hardware,
        physical_tx,
        rx,
        station,
        access_point,
        Esp32s31StaApControlArbiter::new(),
    )
}

/// Complete paired production WDEV owner.
///
/// The two network publishers are derived here from the permanent network
/// owner. Callers cannot attach an AP protocol role to the STA endpoint (or
/// vice versa), and there is no single-interface fallback in this graph.
pub type Esp32s31StaApWdevRunner<
    'resources,
    'irq,
    M,
    N,
    Services,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> = WdevRunner<
    'resources,
    'irq,
    M,
    N,
    Services,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    RX_QUEUE_DEPTH,
    TX_QUEUE_DEPTH,
    WdevNetworkRxEndpoints<
        PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    >,
>;

/// Bind a complete paired services graph to the one permanent tagged network
/// owner and the one MAC interrupt runtime.
///
/// This is the production composition boundary. Building a service set alone
/// does not publish either logical interface and therefore cannot be mistaken
/// for an active STA+AP capability.
pub fn compose_sta_ap_wdev_runner<
    'resources,
    'irq,
    M,
    N,
    Services,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>(
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: N,
    services: Services,
) -> Esp32s31StaApWdevRunner<
    'resources,
    'irq,
    M,
    N,
    Services,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    RX_QUEUE_DEPTH,
    TX_QUEUE_DEPTH,
>
where
    M: RawMutex + 'resources,
    N: WdevNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    Services: WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    let endpoints = WdevNetworkRxEndpoints::new(
        STA_NETWORK_INTERFACE_ID,
        network.rx_publisher(STA_NETWORK_INTERFACE_ID),
        AP_NETWORK_INTERFACE_ID,
        network.rx_publisher(AP_NETWORK_INTERFACE_ID),
    );
    WdevRunner::new_with_scope(
        irq,
        network,
        WdevInterfaceScope::pair(STA_NETWORK_INTERFACE_ID, AP_NETWORK_INTERFACE_ID),
        endpoints,
        services,
    )
}

/// One upstream station peer plus every peer admitted by the SoftAP policy.
pub const STA_AP_RX_BLOCK_ACK_PEER_CAPACITY: usize =
    open_esp_radio_esp32s31_wifi_ap::protocol::AP_MAX_CLIENTS + 1;

/// Unique software owner of the eight physical ordinary RX BlockAck banks.
///
/// The larger peer table tracks logical identities; it does not manufacture
/// more hardware banks. Every operation remains keyed by its MAC interface.
pub type Esp32s31StaApRxBlockAck = RxBlockAckSessions<STA_AP_RX_BLOCK_ACK_PEER_CAPACITY>;

/// Lower the role-neutral logical-interface identity only at the chip edge.
pub const fn lower_sta_ap_vif(interface: StaApVif) -> MacInterface {
    match interface {
        StaApVif::Station => MacInterface::Station,
        StaApVif::AccessPoint => MacInterface::AccessPoint,
    }
}

pub const fn network_interface_id(interface: StaApVif) -> NetworkInterfaceId {
    match interface {
        StaApVif::Station => STA_NETWORK_INTERFACE_ID,
        StaApVif::AccessPoint => AP_NETWORK_INTERFACE_ID,
    }
}

/// Decode only the two endpoint identities owned by this composition. An
/// unknown tag is never guessed from Ethernet contents or the active role.
pub const fn sta_ap_vif(interface: NetworkInterfaceId) -> Option<StaApVif> {
    if interface.value() == STA_NETWORK_INTERFACE_ID.value() {
        Some(StaApVif::Station)
    } else if interface.value() == AP_NETWORK_INTERFACE_ID.value() {
        Some(StaApVif::AccessPoint)
    } else {
        None
    }
}

/// Ownership-preserving TX dispatch result for the shared physical queue.
/// Unknown tags retain their exact lease and must be rejected explicitly by
/// the composition root; no role encoder may guess from Ethernet contents.
pub enum StaApTxDispatch<B> {
    Station(TaggedStableDmaBacking<NetworkInterfaceId, B>),
    AccessPoint(TaggedStableDmaBacking<NetworkInterfaceId, B>),
    Unknown(TaggedStableDmaBacking<NetworkInterfaceId, B>),
}

pub fn dispatch_sta_ap_tx<B>(
    frame: TaggedStableDmaBacking<NetworkInterfaceId, B>,
) -> StaApTxDispatch<B> {
    match sta_ap_vif(*frame.tag()) {
        Some(StaApVif::Station) => StaApTxDispatch::Station(frame),
        Some(StaApVif::AccessPoint) => StaApTxDispatch::AccessPoint(frame),
        None => StaApTxDispatch::Unknown(frame),
    }
}

/// Normalize one hardware-completed S31 receive unit and route only its
/// public IEEE 802.11 header fields to a logical interface.
///
/// Hardware status validation remains in the MAC RX backend. Association,
/// authorization, key and BlockAck policy remain in the selected role
/// consumer; this boundary cannot turn a header match into protocol trust.
pub fn classify_sta_ap_segment(
    segment: &RxSegment<'_>,
    ingress: RxIngressConfig,
    addresses: StaApRxAddresses,
) -> Result<StaApRxRoute, RxError> {
    let normalized = view_normalized_rx_frame(segment, ingress)?;
    Ok(classify_sta_ap_rx(normalized.mpdu, addresses))
}

/// One ordered staged owner together with the fact-only VIF classification
/// made at the common physical RX boundary.
///
/// Every outcome retains the unique staging lease. In particular, malformed,
/// foreign, ambiguous and invalid units cannot disappear merely because no
/// role consumer accepted them. The common protocol dispatcher must consume
/// the result and account for its final disposition.
pub struct Esp32s31StaApStagedRxFrame<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    route: Result<StaApRxRoute, RxError>,
    frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApStagedRxFrame<'pool, CAPACITY, SLOTS>
{
    pub fn classify(
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        ingress: RxIngressConfig,
        addresses: StaApRxAddresses,
    ) -> Self {
        let route = classify_sta_ap_segment(&frame.segment(), ingress, addresses);
        Self { route, frame }
    }

    pub const fn route(&self) -> Result<StaApRxRoute, RxError> {
        self.route
    }

    pub const fn frame(&self) -> &Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS> {
        &self.frame
    }

    pub fn into_parts(
        self,
    ) -> (
        Result<StaApRxRoute, RxError>,
        Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) {
        (self.route, self.frame)
    }

    pub fn into_frame(self) -> Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS> {
        self.frame
    }
}

/// Single ordered handoff from the physical RX producer to the STA+AP
/// protocol dispatcher.
///
/// This is deliberately not split into one queue per VIF: doing that in the
/// DMA producer would make inter-interface ordering and ownership loss
/// dependent on queue capacity. One consumer owns the ordered stream and
/// delegates each retained lease to the selected role protocol.
pub struct Esp32s31StaApStagedRxQueue<
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: Channel<M, Esp32s31StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
}

/// Protocol-side ownership result from the common ordered RX stream.
pub enum Esp32s31StaApRxDispatch<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    Station(Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>),
    AccessPoint(Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>),
    Rejected {
        classification: Result<StaApRxRoute, RxError>,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    },
}

/// Ownership disposition returned by one role-specific processor.
///
/// `Deferred` retains the exact staging lease. It is not a request to copy or
/// reconstruct the frame and can therefore be restored to the ordered common
/// frontier without changing DMA ownership identity.
pub enum Esp32s31RoutedRxDisposition<F> {
    Processed,
    Deferred(F),
}

/// Result of one finite common protocol-dispatch turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApRxTurn<D> {
    Idle,
    Station(D),
    AccessPoint,
    DeferredAccessPoint,
    Rejected(Result<StaApRxRoute, RxError>),
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS>
{
    fn from_staged(frame: Esp32s31StaApStagedRxFrame<'pool, CAPACITY, SLOTS>) -> Self {
        let (classification, frame) = frame.into_parts();
        match classification {
            Ok(StaApRxRoute::Interface(StaApVif::Station)) => Self::Station(frame),
            Ok(StaApRxRoute::Interface(StaApVif::AccessPoint)) => Self::AccessPoint(frame),
            classification => Self::Rejected {
                classification,
                frame,
            },
        }
    }

    pub fn into_frame(self) -> Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS> {
        match self {
            Self::Station(frame) | Self::AccessPoint(frame) | Self::Rejected { frame, .. } => frame,
        }
    }
}

/// Sole protocol-side consumer of the ordered STA+AP stream.
///
/// The consumer chooses a role before handing ownership onward. Role-specific
/// tasks therefore cannot race, scan past one another or reorder leases in
/// separate queues.
pub struct Esp32s31StaApRxConsumer<
    'pool,
    'queue,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: Receiver<'queue, M, Esp32s31StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    deferred: Option<Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS>>,
}

impl<'pool, 'queue, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApRxConsumer<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>
{
    pub fn try_receive(&mut self) -> Option<Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS>> {
        if let Some(frame) = self.deferred.take() {
            return Some(frame);
        }
        self.frames
            .try_receive()
            .ok()
            .map(Esp32s31StaApRxDispatch::from_staged)
    }

    pub async fn receive(&mut self) -> Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS> {
        if let Some(frame) = self.deferred.take() {
            return frame;
        }
        Esp32s31StaApRxDispatch::from_staged(self.frames.receive().await)
    }

    /// Restore a frame that the selected role could not process without
    /// violating its current TX/publication ordering edge.
    ///
    /// Only one ordered head may be deferred. Returning the exact input on a
    /// second attempt makes accidental overwrite impossible.
    pub fn defer(
        &mut self,
        frame: Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS>,
    ) -> Result<(), Esp32s31StaApRxDispatch<'pool, CAPACITY, SLOTS>> {
        if self.deferred.is_some() {
            return Err(frame);
        }
        self.deferred = Some(frame);
        Ok(())
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len() + usize::from(self.deferred.is_some())
    }

    pub fn discard_queued(&mut self) -> usize {
        let mut discarded = usize::from(self.deferred.take().is_some());
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    /// Consume at most one ordered staged owner and delegate it to the
    /// already-selected role processor.
    ///
    /// Station processing may await final network capacity. AP processing is
    /// deliberately finite: hardware/control actions remain in the common
    /// WDEV transaction domain, and an unsafe ordering edge returns the exact
    /// owner through [`Esp32s31RoutedRxDisposition::Deferred`]. No closure can
    /// observe a frame classified for the other role.
    pub async fn service_next<Station, StationFuture, StationDispatch, AccessPoint, Error>(
        &mut self,
        station: Station,
        access_point: AccessPoint,
    ) -> Result<Esp32s31StaApRxTurn<StationDispatch>, Error>
    where
        Station: FnOnce(Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>) -> StationFuture,
        StationFuture: Future<Output = StationDispatch>,
        AccessPoint: FnOnce(
            Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        ) -> Result<
            Esp32s31RoutedRxDisposition<Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>>,
            Error,
        >,
    {
        let Some(dispatch) = self.try_receive() else {
            return Ok(Esp32s31StaApRxTurn::Idle);
        };
        match dispatch {
            Esp32s31StaApRxDispatch::Station(frame) => {
                Ok(Esp32s31StaApRxTurn::Station(station(frame).await))
            }
            Esp32s31StaApRxDispatch::AccessPoint(frame) => match access_point(frame)? {
                Esp32s31RoutedRxDisposition::Processed => Ok(Esp32s31StaApRxTurn::AccessPoint),
                Esp32s31RoutedRxDisposition::Deferred(frame) => {
                    // `try_receive` removed the only possible deferred head,
                    // and neither closure can borrow this consumer. Restoring
                    // the exact owner therefore cannot overwrite another one.
                    debug_assert!(self.deferred.is_none());
                    self.deferred = Some(Esp32s31StaApRxDispatch::AccessPoint(frame));
                    Ok(Esp32s31StaApRxTurn::DeferredAccessPoint)
                }
            },
            Esp32s31StaApRxDispatch::Rejected {
                classification,
                frame,
            } => {
                drop(frame);
                Ok(Esp32s31StaApRxTurn::Rejected(classification))
            }
        }
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApStagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "STA+AP staged RX queue must not be empty");
        assert!(
            DEPTH <= SLOTS,
            "STA+AP staged RX queue cannot outgrow its ownership pool"
        );
        Self {
            frames: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        Sender<'_, M, Esp32s31StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        Esp32s31StaApRxConsumer<'pool, '_, M, DEPTH, CAPACITY, SLOTS>,
    ) {
        (
            self.frames.sender(),
            Esp32s31StaApRxConsumer {
                frames: self.frames.receiver(),
                deferred: None,
            },
        )
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize> Default
    for Esp32s31StaApStagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    const STA: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
    const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
    const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];
    const ADDRESSES: StaApRxAddresses = StaApRxAddresses {
        station: STA,
        station_bssid: UPLINK,
        access_point: AP,
    };

    fn segment(
        storage: &mut [u8; 128],
        frame_control: u16,
        receiver: [u8; 6],
        transmitter: [u8; 6],
        third: [u8; 6],
    ) -> RxSegment<'_> {
        const MPDU_LENGTH: usize = 24;
        const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
        const TAIL_OFFSET: usize = 0x38;
        const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
        const RECEIVED: usize = FRAME_OFFSET + MPDU_LENGTH;
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        storage[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&frame_control.to_le_bytes());
        storage[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&receiver);
        storage[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&transmitter);
        storage[FRAME_OFFSET + 16..FRAME_OFFSET + 22].copy_from_slice(&third);
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: storage,
            next_descriptor_address: 0,
        }
    }

    const INGRESS: RxIngressConfig = RxIngressConfig {
        ring_entry_limit: 1,
        csi_config: 0,
        flags: 0,
    };

    #[test]
    fn logical_vifs_lower_to_their_exact_hardware_contexts() {
        assert_eq!(lower_sta_ap_vif(StaApVif::Station), MacInterface::Station);
        assert_eq!(
            lower_sta_ap_vif(StaApVif::AccessPoint),
            MacInterface::AccessPoint
        );
        assert_ne!(
            network_interface_id(StaApVif::Station),
            network_interface_id(StaApVif::AccessPoint)
        );
        assert_eq!(
            sta_ap_vif(STA_NETWORK_INTERFACE_ID),
            Some(StaApVif::Station)
        );
        assert_eq!(sta_ap_vif(NetworkInterfaceId::new(2)), None);
    }

    #[test]
    fn normalized_dma_boundary_routes_each_data_direction_without_role_inference() {
        let mut storage = [0; 128];
        let from_ds = segment(&mut storage, 0x0208, STA, UPLINK, PEER);
        assert_eq!(
            classify_sta_ap_segment(&from_ds, INGRESS, ADDRESSES),
            Ok(StaApRxRoute::Interface(StaApVif::Station))
        );

        let mut storage = [0; 128];
        let to_ds = segment(&mut storage, 0x0108, AP, PEER, STA);
        assert_eq!(
            classify_sta_ap_segment(&to_ds, INGRESS, ADDRESSES),
            Ok(StaApRxRoute::Interface(StaApVif::AccessPoint))
        );
    }

    #[test]
    fn invalid_hardware_unit_fails_before_vif_classification() {
        let mut storage = [0; 128];
        let mut invalid = segment(&mut storage, 0x0108, AP, PEER, STA);
        invalid.descriptor_word0 &= !BIT_31;
        assert_eq!(
            classify_sta_ap_segment(&invalid, INGRESS, ADDRESSES),
            Err(RxError::Invalid)
        );
    }

    #[test]
    fn shared_tx_dispatch_retains_the_exact_owner_for_every_tag() {
        let station = TaggedStableDmaBacking::new(STA_NETWORK_INTERFACE_ID, 11_u8);
        let access_point = TaggedStableDmaBacking::new(AP_NETWORK_INTERFACE_ID, 22_u8);
        let unknown = TaggedStableDmaBacking::new(NetworkInterfaceId::new(9), 33_u8);

        assert!(matches!(
            dispatch_sta_ap_tx(station),
            StaApTxDispatch::Station(_)
        ));
        assert!(matches!(
            dispatch_sta_ap_tx(access_point),
            StaApTxDispatch::AccessPoint(_)
        ));
        let StaApTxDispatch::Unknown(owner) = dispatch_sta_ap_tx(unknown) else {
            panic!("unknown interface must fail closed")
        };
        assert_eq!(*owner.tag(), NetworkInterfaceId::new(9));
        assert_eq!(*owner, 33);
    }
}
