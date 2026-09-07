//! Reusable finite join composition for initial and reconnected STA epochs.
//!
//! The caller owns the outer role phase and the eventual connected service.
//! This transaction alone assembles the concrete channel/join/WPA2 target,
//! runs it, and returns only value/role owners after every temporary hardware
//! borrow has ended.

use crate::{
    datapath::rx::{
        dma::ReceiveDmaStorage,
        frontier::{ReceiveFrontier, RxFrontierDelay},
    },
    roles::station::attempt::{
        StaAttemptChannel, StaAttemptRadio, StaAttemptStorage, StaAttemptTargetError,
        StaAttemptTargetOwner, StaAttemptTargetPort,
    },
};

use oer_esp32s31_phy::{PhyAsyncDelay, PhyState, PhyTargetObserver};

use oer_esp32s31_wifi_mac::{
    crypto::CcmpKeyHardware,
    he::He20PeerHardware,
    init::{MacRuntimeStopHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate::control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};

use oer_esp32s31_wifi_sta::{
    attempt::{
        AssociationAttemptOutcome, StaAttempt, StaAttemptObserver, StaAttemptProgress,
        StaAttemptReport, StaAttemptSecurity, StaAttemptStage, StaAttemptStation,
        StaInstalledSecurity,
    },
    hardware::channel::ScanPhy,
    join::{StaJoinObserver, StaJoinTransmit},
    peer::{ConnectedStaPeer, StaPeerTransmit},
    wpa2::HandshakeTransmit,
};

use oer_wifi_sta::station::StaFailureDisposition;

/// Concrete primitive error returned by the shared join transaction.
pub type StationJoinError<H, T> =
    StaAttemptTargetError<<T as StaJoinTransmit<H>>::Error, <T as HandshakeTransmit<H>>::Error>;

/// Role values returned after every temporary join borrow has ended.
pub struct StationJoinReturned<
    'storage,
    'security,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    pub receive: ReceiveFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub station: StaAttemptStation,
    pub security: StaAttemptSecurity<'security>,
}

/// Finite result of the common channel/authentication/association/WPA2 join.
#[expect(
    clippy::large_enum_variant,
    reason = "successful join transfers peer and security state inline; neither branch may allocate or discard returned resources"
)]
pub enum StationJoinOutcome<
    'storage,
    'security,
    D,
    E,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    Connected {
        returned: StationJoinReturned<'storage, 'security, D, COUNT, DMA_BUFFER_SIZE>,
        peer: ConnectedStaPeer,
        installed_security: StaInstalledSecurity,
        report: StaAttemptReport,
        progress: StaAttemptProgress,
    },
    Failed {
        returned: StationJoinReturned<'storage, 'security, D, COUNT, DMA_BUFFER_SIZE>,
        report: StaAttemptReport,
        stage: StaAttemptStage,
        disposition: StaFailureDisposition,
        error: E,
        progress: StaAttemptProgress,
    },
}

/// Complete owner bundle consumed by one finite station join transaction.
///
/// Keeping the radio, receive, TX and protocol scratch frontiers in one value
/// prevents composition roots from encoding the join contract as a positional
/// argument list. The transaction only borrows the hardware-facing owners;
/// the receive and station/security owners are returned in every outcome.
pub struct StationJoinResources<
    'hardware,
    'state,
    'storage,
    'transmit,
    'scratch,
    'security,
    H,
    P,
    PO,
    D,
    T,
    AO,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    pub hardware: &'hardware mut H,
    pub phy: &'state mut PhyState,
    pub platform: &'state mut P,
    pub phy_observer: PO,
    pub receive: ReceiveFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub rx_storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: &'transmit mut T,
    pub frame: &'scratch mut [u8],
    pub station: StaAttemptStation,
    pub listen_interval: oer_wifi_sta::request::StationListenInterval,
    pub security: StaAttemptSecurity<'security>,
    pub attempt_observer: AO,
}

/// Run the one production join transaction used by both normal firmware and
/// HIL. Observers receive value-only evidence and cannot replace any policy or
/// hardware transition.
#[allow(clippy::type_complexity)]
pub async fn run_esp32s31_station_join<
    'hardware,
    'state,
    'storage,
    'transmit,
    'scratch,
    'security,
    H,
    P,
    PO,
    PD,
    D,
    T,
    J,
    AO,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    resources: StationJoinResources<
        'hardware,
        'state,
        'storage,
        'transmit,
        'scratch,
        'security,
        H,
        P,
        PO,
        D,
        T,
        AO,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
) -> StationJoinOutcome<'storage, 'security, D, StationJoinError<H, T>, COUNT, DMA_BUFFER_SIZE>
where
    H: RxDma
        + TxHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + CcmpKeyHardware
        + MacRuntimeStopHardware
        + 'hardware,
    PO: PhyTargetObserver,
    PD: PhyAsyncDelay,
    D: RxFrontierDelay,
    T: StaJoinTransmit<H> + HandshakeTransmit<H> + StaPeerTransmit + 'transmit,
    J: StaJoinObserver + Default,
    AO: StaAttemptObserver,
    ScanPhy<'state, P, PO, PD>: StaAttemptChannel<H>,
{
    let StationJoinResources {
        hardware,
        phy,
        platform,
        phy_observer,
        receive,
        rx_storage,
        transmit,
        frame,
        station,
        listen_interval,
        security,
        attempt_observer,
    } = resources;
    type Channel<'a, P, O, D> = ScanPhy<'a, P, O, D>;
    let owner = StaAttemptTargetOwner::<
        '_,
        '_,
        '_,
        '_,
        '_,
        H,
        Channel<'_, P, PO, PD>,
        D,
        T,
        J,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >::new(
        StaAttemptRadio::new(
            hardware,
            ScanPhy::<P, PO, PD>::new(phy, platform, phy_observer),
            receive,
            rx_storage,
            transmit,
        ),
        StaAttemptStorage::new(frame),
        station,
        listen_interval,
        security,
    );
    let mut attempt = StaAttempt::with_observer(StaAttemptTargetPort::new(), attempt_observer);
    match attempt.run(owner).await {
        AssociationAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, progress) = failure.into_parts();
            let report = owner.report();
            let (radio, _storage, station, security) = owner.into_parts();
            let StaAttemptRadio {
                channel, receive, ..
            } = radio;
            let _ = channel.into_parts();
            StationJoinOutcome::Failed {
                returned: StationJoinReturned {
                    receive,
                    station,
                    security,
                },
                report,
                stage,
                disposition,
                error,
                progress,
            }
        }
        AssociationAttemptOutcome::Connected {
            connected,
            progress,
        } => {
            let mut owner = connected.into_owner();
            let report = owner.report();
            let peer = owner
                .take_connected_peer()
                .expect("a connected station attempt owns its peer");
            let installed_security = owner
                .take_installed_security()
                .expect("a connected station attempt owns its selected security frontier");
            let (radio, _storage, station, security) = owner.into_parts();
            let StaAttemptRadio {
                channel, receive, ..
            } = radio;
            let _ = channel.into_parts();
            StationJoinOutcome::Connected {
                returned: StationJoinReturned {
                    receive,
                    station,
                    security,
                },
                peer,
                installed_security,
                report,
                progress,
            }
        }
    }
}
