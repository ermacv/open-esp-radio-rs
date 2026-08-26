//! Reusable finite join composition for initial and reconnected STA epochs.
//!
//! The caller owns the outer role phase and the eventual connected service.
//! This transaction alone assembles the concrete channel/join/WPA2 target,
//! runs it, and returns only value/role owners after every temporary hardware
//! borrow has ended.

use open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl;
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyState, PhyTargetObserver};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::CcmpKeyHardware,
    he::He20PeerHardware,
    init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate_control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{
        Esp32s31StaAttempt, Esp32s31StaAttemptObserver, Esp32s31StaAttemptOutcome,
        Esp32s31StaAttemptProgress, Esp32s31StaAttemptReport, Esp32s31StaAttemptSecurity,
        Esp32s31StaAttemptStage, Esp32s31StaAttemptStation, Esp32s31StaInstalledSecurity,
    },
    channel::Esp32s31ScanPhy,
    join::{Esp32s31StaJoinObserver, Esp32s31StaJoinTransmit},
    peer::{Esp32s31ConnectedStaPeer, Esp32s31StaPeerTransmit},
    wpa2::Esp32s31Wpa2Transmit,
};
use open_esp_radio_wifi_sta::station::StaFailureDisposition;

use crate::{
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{Esp32s31RxFrontier, Esp32s31RxFrontierDelay},
    roles::station::attempt::{
        Esp32s31StaAttemptChannel, Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage,
        Esp32s31StaAttemptTargetError, Esp32s31StaAttemptTargetOwner, Esp32s31StaAttemptTargetPort,
    },
};

/// Concrete primitive error returned by the shared join transaction.
pub type Esp32s31StationJoinError<H, T> = Esp32s31StaAttemptTargetError<
    <T as Esp32s31StaJoinTransmit<H>>::Error,
    <T as Esp32s31Wpa2Transmit<H>>::Error,
>;

/// Role values returned after every temporary join borrow has ended.
pub struct Esp32s31StationJoinReturned<
    'storage,
    'security,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    pub receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub station: Esp32s31StaAttemptStation,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

/// Finite result of the common channel/authentication/association/WPA2 join.
pub enum Esp32s31StationJoinOutcome<
    'storage,
    'security,
    D,
    E,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    Connected {
        returned: Esp32s31StationJoinReturned<'storage, 'security, D, COUNT, DMA_BUFFER_SIZE>,
        peer: Esp32s31ConnectedStaPeer,
        installed_security: Esp32s31StaInstalledSecurity,
        report: Esp32s31StaAttemptReport,
        progress: Esp32s31StaAttemptProgress,
    },
    Failed {
        returned: Esp32s31StationJoinReturned<'storage, 'security, D, COUNT, DMA_BUFFER_SIZE>,
        report: Esp32s31StaAttemptReport,
        stage: Esp32s31StaAttemptStage,
        disposition: StaFailureDisposition,
        error: E,
        progress: Esp32s31StaAttemptProgress,
    },
}

/// Complete owner bundle consumed by one finite station join transaction.
///
/// Keeping the radio, receive, TX and protocol scratch frontiers in one value
/// prevents composition roots from encoding the join contract as a positional
/// argument list. The transaction only borrows the hardware-facing owners;
/// the receive and station/security owners are returned in every outcome.
pub struct Esp32s31StationJoinResources<
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
    pub receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: &'transmit mut T,
    pub frame: &'scratch mut [u8],
    pub station: Esp32s31StaAttemptStation,
    pub listen_interval: open_esp_radio_wifi_sta::request::StationListenInterval,
    pub security: Esp32s31StaAttemptSecurity<'security>,
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
    resources: Esp32s31StationJoinResources<
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
) -> Esp32s31StationJoinOutcome<
    'storage,
    'security,
    D,
    Esp32s31StationJoinError<H, T>,
    COUNT,
    DMA_BUFFER_SIZE,
>
where
    H: RxDma
        + TxHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + CcmpKeyHardware
        + 'hardware,
    P: PhyI2cMasterControl,
    PO: PhyTargetObserver,
    PD: PhyAsyncDelay,
    D: Esp32s31RxFrontierDelay,
    T: Esp32s31StaJoinTransmit<H> + Esp32s31Wpa2Transmit<H> + Esp32s31StaPeerTransmit + 'transmit,
    J: Esp32s31StaJoinObserver + Default,
    AO: Esp32s31StaAttemptObserver,
    Esp32s31ScanPhy<'state, P, PO, PD>: Esp32s31StaAttemptChannel<H>,
{
    let Esp32s31StationJoinResources {
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
    type Channel<'a, P, O, D> = Esp32s31ScanPhy<'a, P, O, D>;
    let owner = Esp32s31StaAttemptTargetOwner::<
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
        Esp32s31StaAttemptRadio::new(
            hardware,
            Esp32s31ScanPhy::<P, PO, PD>::new(phy, platform, phy_observer),
            receive,
            rx_storage,
            transmit,
        ),
        Esp32s31StaAttemptStorage::new(frame),
        station,
        listen_interval,
        security,
    );
    let mut attempt =
        Esp32s31StaAttempt::with_observer(Esp32s31StaAttemptTargetPort::new(), attempt_observer);
    match attempt.run(owner).await {
        Esp32s31StaAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, progress) = failure.into_parts();
            let report = owner.report();
            let (radio, _storage, station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                channel, receive, ..
            } = radio;
            let _ = channel.into_parts();
            Esp32s31StationJoinOutcome::Failed {
                returned: Esp32s31StationJoinReturned {
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
        Esp32s31StaAttemptOutcome::Connected {
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
            let Esp32s31StaAttemptRadio {
                channel, receive, ..
            } = radio;
            let _ = channel.into_parts();
            Esp32s31StationJoinOutcome::Connected {
                returned: Esp32s31StationJoinReturned {
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
