//! Complete ESP32-S31 station scan composition.
//!
//! Initial and reconnect scans use different RX owners, but their PHY,
//! active-probe, storage and finite service composition is identical.  This
//! module is the single ownership transaction for that common work: callers
//! provide one coherent resource set and receive every owner back together
//! with a value-only decision.

use open_esp_radio_esp32s31_hal::MacInterruptSetup;
use open_esp_radio_esp32s31_hal::{
    phy_i2c::PhyI2cMasterControl, phy_temperature::PhyTemperatureSystemControl,
    wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyState, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_mac::tx::TxHardware;
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaIdentity},
    channel::Esp32s31ScanPhy,
    control_tx::{ControlTxError, Esp32s31ControlTx},
    scan::{Esp32s31StaScanBackend, Esp32s31StaScanConfig, Esp32s31StaScanError},
    scan_tx::{Esp32s31RunningScanTx, Esp32s31ScanTxSummary},
};
use open_esp_radio_ieee80211::{
    scan::{ScanRecord, ScanTable},
    security::WifiSecurityMode,
    station::StaSequenceCounter,
};
use open_esp_radio_wifi_sta::request::{StationDiscovery, WifiSsid};
use open_esp_radio_wifi_sta::scan::{
    StaCandidateScanExit, StaCandidateScanService, StaScanPlanError, StaScanProgress,
};
use open_esp_radio_wifi_sta::station::StaFailureDisposition;
use open_esp_radio_wifi_sta::station::{StaAttemptFailure, StaAttemptOutcome, StaLifecycleStage};

use crate::{
    roles::scan::port::{
        Esp32s31ScanPhyPort, Esp32s31ScanPort, Esp32s31ScanPortError, Esp32s31ScanRadio,
        Esp32s31ScanReceivePort, Esp32s31ScanStation, Esp32s31ScanStorage, Esp32s31ScanTelemetry,
        Esp32s31ScanTimer,
    },
    roles::scan::rx::Esp32s31ScanFrameObserver,
};

use super::composer::Esp32s31StationInitialScanExit;

/// Qualified active-probe rate set used by the normal ESP32-S31 station
/// service. It belongs to the chip/runtime composition, not to board or HIL
/// policy.
pub const ESP32S31_STATION_PROBE_RATES: [u8; 12] = [
    0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
];

/// Descriptor admission bound for one default active probe publication.
pub const ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY: u32 = 88;

/// Owned, finite S31 scan plan derived from one portable discovery request.
///
/// Board firmware and HIL may supply a preferred first channel, but neither
/// can replace the selected set, dwell or SSID after this value is built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StationScanPlan {
    config: Esp32s31StaScanConfig,
    channels: [u8; 14],
    channel_count: u8,
    ssid: WifiSsid,
    security: WifiSecurityMode,
}

impl Esp32s31StationScanPlan {
    pub fn new(
        discovery: StationDiscovery,
        preferred_channel: Option<u8>,
        security: WifiSecurityMode,
    ) -> Self {
        let mut channels = [0; 14];
        let mut channel_count = 0_usize;
        for channel in discovery
            .scan()
            .channels()
            .primary_channels_preferred(preferred_channel)
        {
            channels[channel_count] = channel;
            channel_count += 1;
        }
        Self {
            config: Esp32s31StaScanConfig::new(discovery.scan().dwell_millis())
                .expect("StationScanPolicy guarantees a nonzero dwell"),
            channels,
            channel_count: channel_count as u8,
            ssid: discovery.ssid(),
            security,
        }
    }

    pub const fn channel_count(&self) -> u8 {
        self.channel_count
    }

    pub fn channels(&self) -> &[u8] {
        &self.channels[..usize::from(self.channel_count)]
    }

    pub fn target_ssid(&self) -> &[u8] {
        self.ssid.as_bytes()
    }

    pub fn request(&self, station_address: [u8; 6]) -> Esp32s31StationScanRequest<'_, '_, '_> {
        Esp32s31StationScanRequest::new(
            self.config,
            self.channels(),
            station_address,
            self.target_ssid(),
            &ESP32S31_STATION_PROBE_RATES,
            self.security,
        )
        .with_descriptor_capacity(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
    }
}

/// Value-only policy for one finite station candidate scan.
#[derive(Clone, Copy)]
pub struct Esp32s31StationScanRequest<'ssid, 'rates, 'channels> {
    pub config: Esp32s31StaScanConfig,
    pub channels: &'channels [u8],
    pub station_address: [u8; 6],
    pub target_ssid: &'ssid [u8],
    pub supported_rates: &'rates [u8],
    pub descriptor_capacity: Option<u32>,
    pub select_candidate: bool,
    pub security: WifiSecurityMode,
}

impl<'ssid, 'rates, 'channels> Esp32s31StationScanRequest<'ssid, 'rates, 'channels> {
    pub const fn new(
        config: Esp32s31StaScanConfig,
        channels: &'channels [u8],
        station_address: [u8; 6],
        target_ssid: &'ssid [u8],
        supported_rates: &'rates [u8],
        security: WifiSecurityMode,
    ) -> Self {
        Self {
            config,
            channels,
            station_address,
            target_ssid,
            supported_rates,
            descriptor_capacity: None,
            select_candidate: true,
            security,
        }
    }

    pub const fn with_descriptor_capacity(mut self, capacity: u32) -> Self {
        self.descriptor_capacity = Some(capacity);
        self
    }

    /// Collect observations without selecting a peer for association.
    pub const fn without_candidate_selection(mut self) -> Self {
        self.select_candidate = false;
        self
    }
}

/// Exact owner set consumed by one initial or reconnect scan.
#[allow(clippy::type_complexity)]
pub struct Esp32s31StationScanResources<
    'radio,
    'storage,
    'sequence,
    'slot,
    'interrupt,
    P,
    Q,
    D,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub phy: &'radio mut PhyState,
    pub platform: &'radio mut P,
    pub phy_observer: Q,
    pub phy_delay: D,
    pub hardware: H,
    pub receive: R,
    pub control: Esp32s31ControlTx<'slot, X, E, T, TX_BUFFER_SIZE>,
    pub interrupt_setup: &'interrupt MacInterruptSetup,
    pub table: &'storage mut ScanTable<RECORDS>,
    pub frame: &'storage mut [u8],
    pub scan_observer: O,
    pub sequence: &'sequence mut StaSequenceCounter,
    pub timer: W,
}

/// Owners returned after the scan transaction has stopped RX.
///
/// The role-specific caller decides whether `receive` becomes an initial
/// halted ring or a stopped connected RX epoch.  Keeping that conversion
/// outside this common transaction prevents the two distinct owner frontiers
/// from being conflated.
#[allow(clippy::type_complexity)]
pub struct Esp32s31StationScanReturned<
    'storage,
    'sequence,
    'slot,
    P,
    D,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub hardware: H,
    pub receive: R,
    pub control: Esp32s31ControlTx<'slot, X, E, T, TX_BUFFER_SIZE>,
    pub phy_observer: P,
    pub phy_delay: D,
    pub scan_observer: O,
    pub timer: W,
    pub table: &'storage mut ScanTable<RECORDS>,
    pub frame: &'storage mut [u8],
    pub sequence: &'sequence mut StaSequenceCounter,
    pub telemetry: Esp32s31ScanTelemetry,
    pub transmit: Esp32s31ScanTxSummary,
}

/// Value-only result of one finite candidate scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationScanDecision<E> {
    Selected {
        candidate: ScanRecord,
        progress: StaScanProgress,
    },
    NoCandidate {
        progress: StaScanProgress,
    },
    Stopped {
        progress: StaScanProgress,
    },
    Failed {
        error: E,
        progress: StaScanProgress,
    },
    InvalidPlan {
        error: StaScanPlanError,
        progress: StaScanProgress,
    },
}

/// Complete finite result: no decision is observable without all owners.
pub struct Esp32s31StationScanOutcome<R, E> {
    pub returned: R,
    pub decision: Esp32s31StationScanDecision<E>,
}

/// Outer lifecycle owners returned by a completely stopped initial scan.
///
/// Observation-only values stay outside this bundle. These are exactly the
/// movable owners needed either to enter the first join or reconstruct the
/// `InitialScan` station phase after a finite exit.
pub struct Esp32s31StationInitialScanReturned<'security, R, H, S, N> {
    pub runtime: R,
    pub hardware: H,
    pub receive: S,
    pub network: N,
    pub identity: Esp32s31StaIdentity,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

/// Consumer-specific diagnostic errors for the four initial-scan failures.
///
/// Their lifecycle severity is deliberately absent: the common completion
/// transaction below owns that safety policy. HIL and ordinary firmware may
/// use different error vocabularies without choosing different retry rules.
pub struct Esp32s31StationInitialScanFailures<E> {
    pub no_candidate: E,
    pub receive_handoff: E,
    pub transaction: E,
    pub invalid_plan: E,
}

/// Classify a failed production scan using the shared owner-safety policy.
///
/// A failed probe publication or an RX-stop failure may leave a hardware
/// frontier that cannot safely be retried. Other finite scan failures retain
/// a stopped/recoverable owner and may refresh the candidate.
pub const fn esp32s31_station_scan_failure_disposition<P, R, T>(
    error: &Esp32s31StaScanError<Esp32s31ScanPortError<P, R, T>>,
) -> StaFailureDisposition {
    if matches!(
        error,
        Esp32s31StaScanError::ActiveProbe(Esp32s31ScanPortError::Transmit(_))
            | Esp32s31StaScanError::ReceiveStop(_)
    ) {
        StaFailureDisposition::Terminal
    } else {
        StaFailureDisposition::RefreshCandidate
    }
}

/// Complete the actor-owned initial scan and return one exact lifecycle edge.
///
/// `prepare_receive` is the only role-specific conversion: normal cold RX and
/// HIL may wrap the same halted ring differently. `restore_owner` only names
/// the consumer's concrete phase-owner alias; it cannot alter the common
/// disposition or candidate-selection rules.
#[allow(clippy::type_complexity)]
pub fn complete_esp32s31_station_initial_scan<'security, R, H, S, X, N, O, E, F, P, Q, T>(
    returned: Esp32s31StationInitialScanReturned<'security, R, H, S, N>,
    decision: Esp32s31StationScanDecision<Esp32s31StaScanError<Esp32s31ScanPortError<P, Q, T>>>,
    prepare_receive: impl FnOnce(S) -> Result<X, S>,
    restore_owner: impl FnOnce(R, H, S, N, Esp32s31StaIdentity, Esp32s31StaAttemptSecurity) -> O,
    failures: Esp32s31StationInitialScanFailures<E>,
) -> Esp32s31StationInitialScanExit<'security, R, H, X, N, O, E, F> {
    let Esp32s31StationInitialScanReturned {
        runtime,
        hardware,
        receive,
        network,
        identity,
        security,
    } = returned;
    let Esp32s31StationInitialScanFailures {
        no_candidate,
        receive_handoff,
        transaction,
        invalid_plan,
    } = failures;
    match decision {
        Esp32s31StationScanDecision::Selected { candidate, .. } => match prepare_receive(receive) {
            Ok(receive) => Esp32s31StationInitialScanExit::join_ready(
                runtime,
                hardware,
                receive,
                network,
                identity.select(candidate),
                security,
            ),
            Err(receive) => Esp32s31StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    receive_handoff,
                ),
            }),
        },
        Esp32s31StationScanDecision::NoCandidate { .. } => {
            Esp32s31StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    StaFailureDisposition::RefreshCandidate,
                    no_candidate,
                ),
            })
        }
        Esp32s31StationScanDecision::Stopped { .. } => {
            Esp32s31StationInitialScanExit::complete(StaAttemptOutcome::Stopped {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
            })
        }
        Esp32s31StationScanDecision::Failed { error, .. } => {
            let disposition = esp32s31_station_scan_failure_disposition(&error);
            Esp32s31StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    transaction,
                ),
            })
        }
        Esp32s31StationScanDecision::InvalidPlan { .. } => {
            Esp32s31StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    StaFailureDisposition::Terminal,
                    invalid_plan,
                ),
            })
        }
    }
}

/// Compose and run the common ESP32-S31 station candidate scan.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn run_esp32s31_station_scan<
    'radio,
    'storage,
    'sequence,
    'slot,
    'interrupt,
    'ssid,
    'rates,
    'channels,
    P,
    Q,
    D,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
>(
    resources: Esp32s31StationScanResources<
        'radio,
        'storage,
        'sequence,
        'slot,
        'interrupt,
        P,
        Q,
        D,
        H,
        R,
        X,
        E,
        T,
        O,
        W,
        RECORDS,
        TX_BUFFER_SIZE,
    >,
    request: Esp32s31StationScanRequest<'ssid, 'rates, 'channels>,
) -> Esp32s31StationScanOutcome<
    Esp32s31StationScanReturned<
        'storage,
        'sequence,
        'slot,
        Q,
        D,
        H,
        R,
        X,
        E,
        T,
        O,
        W,
        RECORDS,
        TX_BUFFER_SIZE,
    >,
    Esp32s31StaScanError<
        Esp32s31ScanPortError<
            PhyTargetPortError,
            <R as Esp32s31ScanReceivePort<H>>::Error,
            ControlTxError,
        >,
    >,
>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    Q: PhyTargetObserver,
    D: PhyAsyncDelay,
    H: TxHardware,
    R: Esp32s31ScanReceivePort<H>,
    X: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    O: Esp32s31ScanFrameObserver,
    W: Esp32s31ScanTimer,
    Esp32s31ScanPhy<'radio, P, Q, D>: Esp32s31ScanPhyPort<H, Error = PhyTargetPortError>,
{
    let Esp32s31StationScanResources {
        phy,
        platform,
        phy_observer,
        phy_delay,
        hardware,
        receive,
        control,
        interrupt_setup,
        table,
        frame,
        scan_observer,
        sequence,
        timer,
    } = resources;
    let station = match request.descriptor_capacity {
        Some(capacity) => Esp32s31ScanStation::new(
            request.station_address,
            request.target_ssid,
            request.supported_rates,
            request.security,
        )
        .with_descriptor_capacity(capacity),
        None => Esp32s31ScanStation::new(
            request.station_address,
            request.target_ssid,
            request.supported_rates,
            request.security,
        ),
    }
    .with_candidate_selection(request.select_candidate);
    let owner = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(
            Esp32s31ScanPhy::<_, _, D>::new(phy, platform, phy_observer),
            hardware,
            receive,
            Esp32s31RunningScanTx::new(control, interrupt_setup),
        ),
        Esp32s31ScanStorage::new(table, frame, scan_observer, sequence),
        station,
        timer,
    );
    let mut service = StaCandidateScanService::new(Esp32s31StaScanBackend::new(request.config));
    let (owner, decision) = match service.run(owner, request.channels).await {
        StaCandidateScanExit::Selected {
            owner,
            candidate,
            progress,
        } => (
            owner,
            Esp32s31StationScanDecision::Selected {
                candidate,
                progress,
            },
        ),
        StaCandidateScanExit::NoCandidate { owner, progress } => {
            (owner, Esp32s31StationScanDecision::NoCandidate { progress })
        }
        StaCandidateScanExit::Stopped { owner, progress } => {
            (owner, Esp32s31StationScanDecision::Stopped { progress })
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => (
            owner,
            Esp32s31StationScanDecision::Failed { error, progress },
        ),
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => (
            owner,
            Esp32s31StationScanDecision::InvalidPlan { error, progress },
        ),
    };
    let parts = owner.into_parts();
    let (_phy, _platform, phy_observer) = parts.phy.into_parts();
    let (control, transmit) = parts.tx.into_parts();
    Esp32s31StationScanOutcome {
        returned: Esp32s31StationScanReturned {
            hardware: parts.hardware,
            receive: parts.rx,
            control,
            phy_observer,
            phy_delay,
            scan_observer: parts.observer,
            timer: parts.timer,
            table: parts.table,
            frame: parts.frame,
            sequence: parts.sequence,
            telemetry: parts.telemetry,
            transmit,
        },
        decision,
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU16;

    use open_esp_radio_ieee80211::station::StaAssociationPreference;
    use open_esp_radio_wifi_sta::request::{StationScanChannels, StationScanPolicy};

    use super::*;

    #[test]
    fn owned_scan_plan_is_the_only_source_of_discovery_policy() {
        let discovery = StationDiscovery::new(
            WifiSsid::new(b"portable").unwrap(),
            StationScanPolicy::new(
                StationScanChannels::from_primary_channels(&[1, 6, 11]).unwrap(),
                NonZeroU16::new(40).unwrap(),
                StaAssociationPreference::Automatic,
            ),
        );
        let plan = Esp32s31StationScanPlan::new(discovery, Some(11));
        assert_eq!(plan.channels(), [11, 1, 6]);
        assert_eq!(plan.target_ssid(), b"portable");
        let request = plan.request([2, 0, 0, 0, 0, 1]);
        assert_eq!(request.config.dwell_ticks(), 40);
        assert_eq!(request.supported_rates, ESP32S31_STATION_PROBE_RATES);
        assert_eq!(
            request.descriptor_capacity,
            Some(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
        );
    }

    #[test]
    fn scan_failure_policy_is_terminal_only_at_unsafe_owner_frontiers() {
        assert_eq!(
            esp32s31_station_scan_failure_disposition(&Esp32s31StaScanError::ActiveProbe(
                Esp32s31ScanPortError::<u8, u8, u8>::Transmit(1),
            )),
            StaFailureDisposition::Terminal,
        );
        assert_eq!(
            esp32s31_station_scan_failure_disposition(&Esp32s31StaScanError::ReceiveStop(
                Esp32s31ScanPortError::<u8, u8, u8>::Receive(2),
            )),
            StaFailureDisposition::Terminal,
        );
        assert_eq!(
            esp32s31_station_scan_failure_disposition(&Esp32s31StaScanError::ChannelSwitch(
                Esp32s31ScanPortError::<u8, u8, u8>::ChannelSwitch(3),
            )),
            StaFailureDisposition::RefreshCandidate,
        );
    }
}
