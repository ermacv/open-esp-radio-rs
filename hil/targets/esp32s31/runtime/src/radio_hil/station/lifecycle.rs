#![forbid(unsafe_code)]

use core::{future::Future, marker::PhantomData, num::NonZeroU16};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::{
    esp32s31::hal::RadioRegisters,
    wifi::sta::{
        request::{
            StationDiscovery, StationScanChannels, StationScanPolicy, WifiSsid, WifiSsidError,
        },
        scan::StaScanPlanError,
        station::{StaAttemptContext, StaAttemptOutcome, StaBackoffReason, StaLifecycleStage},
    },
};
use open_esp_radio_esp32s31_wifi_embassy::station::{
    Esp32s31StationCommand, Esp32s31StationEngine, Esp32s31StationEngineObserver,
    Esp32s31StationEnginePort, Esp32s31StationInitialJoinPhase, Esp32s31StationInitialScanExit,
    Esp32s31StationInitialScanPhase, Esp32s31StationReconnectedPhase,
    Esp32s31StationRunningScanExit, Esp32s31StationRunningScanPhase,
    Esp32s31StationServicePhaseKind,
};
use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationFailureStage, StationLifecycleEvent,
};

use super::attempts::{
    run_initial_station_attempt, run_reconnected_station_attempt, run_running_scan_attempt,
};
use super::run_initial_station_scan_attempt;
use crate::{
    console::emergency_log,
    radio_hil::{
        RadioHilConnectedTaskFixture, RadioHilDisconnectedEpoch, RadioHilJoinRx,
        RadioHilReconnectedEpoch, RadioHilRunningScanPortError, RadioHilStaLifecycleOwner,
        RadioHilStaNetwork, RadioHilStationCommandReceiver, SCAN_DWELL_MS, ScanRx,
    },
};
use open_esp_radio::esp32s31::wifi::sta::{
    attempt::Esp32s31StaAttemptStage, scan::Esp32s31StaScanError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilStaLifecycleFailure {
    InitialScanNoCandidate,
    InitialScanTransaction,
    InitialScanPlan,
    InitialScanReceiveHandoff,
    Authentication,
    InitialJoin {
        associated: bool,
        message1: bool,
        message3: bool,
    },
    CandidateRefreshContract,
    RunningScanNoCandidate,
    RunningScanTransaction(Esp32s31StaScanError<RadioHilRunningScanPortError>),
    RunningScanPlan(StaScanPlanError),
    MissingReconnectReceiveOwner,
    StationAttempt(Esp32s31StaAttemptStage),
    ConnectedHardware,
}

pub(in crate::radio_hil) const fn protocol_station_failure_stage(
    stage: StaLifecycleStage,
) -> StationFailureStage {
    match stage {
        StaLifecycleStage::CandidateSelection => StationFailureStage::CandidateSelection,
        StaLifecycleStage::Authentication => StationFailureStage::Authentication,
        StaLifecycleStage::Association => StationFailureStage::Association,
        StaLifecycleStage::Security => StationFailureStage::Security,
        StaLifecycleStage::Connected => StationFailureStage::Connected,
        StaLifecycleStage::Hardware => StationFailureStage::Hardware,
    }
}

pub(in crate::radio_hil) const fn protocol_station_failure_reason(
    error: RadioHilStaLifecycleFailure,
) -> StationAttemptFailureReason {
    match error {
        RadioHilStaLifecycleFailure::InitialScanNoCandidate
        | RadioHilStaLifecycleFailure::RunningScanNoCandidate => {
            StationAttemptFailureReason::NoCandidate
        }
        RadioHilStaLifecycleFailure::Authentication
        | RadioHilStaLifecycleFailure::InitialJoin { .. }
        | RadioHilStaLifecycleFailure::StationAttempt(_) => {
            StationAttemptFailureReason::PeerProtocol
        }
        RadioHilStaLifecycleFailure::InitialScanTransaction
        | RadioHilStaLifecycleFailure::RunningScanTransaction(_)
        | RadioHilStaLifecycleFailure::ConnectedHardware => StationAttemptFailureReason::Hardware,
        RadioHilStaLifecycleFailure::InitialScanPlan
        | RadioHilStaLifecycleFailure::InitialScanReceiveHandoff
        | RadioHilStaLifecycleFailure::CandidateRefreshContract
        | RadioHilStaLifecycleFailure::RunningScanPlan(_)
        | RadioHilStaLifecycleFailure::MissingReconnectReceiveOwner => {
            StationAttemptFailureReason::ContractViolation
        }
    }
}

pub(in crate::radio_hil) struct RadioHilStationEnginePort<'fixture> {
    scan_qualified: bool,
    _fixture: PhantomData<&'fixture mut ()>,
}

impl RadioHilStationEnginePort<'_> {
    pub(in crate::radio_hil) const fn new() -> Self {
        Self {
            scan_qualified: false,
            _fixture: PhantomData,
        }
    }

    pub(in crate::radio_hil) const fn scan_qualified(&self) -> bool {
        self.scan_qualified
    }
}

/// Materialize the same chip-neutral station discovery request consumed by
/// the production engine. HIL may choose scan ordering inside its port, but it
/// does not own an independent SSID, channel-set or dwell policy.
pub(in crate::radio_hil) fn radio_hil_station_discovery(
    target_ssid: &[u8],
) -> Result<StationDiscovery, WifiSsidError> {
    let dwell = NonZeroU16::new(SCAN_DWELL_MS).expect("the fixed HIL scan dwell is nonzero");
    Ok(StationDiscovery::new(
        WifiSsid::new(target_ssid)?,
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            dwell,
            crate::radio_hil::STA_ASSOCIATION_PREFERENCE,
        ),
    ))
}

pub(in crate::radio_hil) type RadioHilStationEngine<'fixture, 'security> = Esp32s31StationEngine<
    'security,
    RadioHilStationEnginePort<'fixture>,
    RadioHilStationEngineObserver,
>;

pub(in crate::radio_hil) struct RadioHilStationEngineObserver;

impl<'fixture, 'security> Esp32s31StationEnginePort<'security, CriticalSectionRawMutex>
    for RadioHilStationEnginePort<'fixture>
{
    type Runtime = RadioHilConnectedTaskFixture<'fixture>;
    type InitialHardware = RadioRegisters;
    type InitialScanRx = ScanRx;
    type PreconnectedRx = RadioHilJoinRx<'static>;
    type Network = RadioHilStaNetwork;
    type Disconnected = RadioHilDisconnectedEpoch;
    type Reconnected = RadioHilReconnectedEpoch;
    type Error = RadioHilStaLifecycleFailure;
    type Fault = core::convert::Infallible;

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut RadioHilStationCommandReceiver<'_>,
    ) -> impl Future<
        Output = Esp32s31StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::PreconnectedRx,
            Self::Network,
            RadioHilStaLifecycleOwner<'fixture, 'security>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            run_initial_station_scan_attempt(phase, discovery, &mut self.scan_qualified).await
        }
    }

    fn run_initial_join<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::PreconnectedRx,
            Self::Network,
        >,
        context: StaAttemptContext,
        control: &'a mut RadioHilStationCommandReceiver<'_>,
    ) -> impl Future<
        Output = StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, Self::Error>,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, hardware, receive, network, station, security) = phase.into_parts();
            run_initial_station_attempt(
                runtime,
                hardware,
                station,
                receive,
                network,
                security,
                control,
                context.generation,
            )
            .await
        }
    }

    fn run_running_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut RadioHilStationCommandReceiver<'_>,
    ) -> impl Future<
        Output = Esp32s31StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            RadioHilStaLifecycleOwner<'fixture, 'security>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, disconnected, station, security) = phase.into_parts();
            run_running_scan_attempt(runtime, station, disconnected, security, discovery).await
        }
    }

    fn run_reconnected<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
        >,
        context: StaAttemptContext,
        control: &'a mut RadioHilStationCommandReceiver<'_>,
    ) -> impl Future<
        Output = StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, Self::Error>,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, epoch, network, station, security) = phase.into_parts();
            run_reconnected_station_attempt(
                runtime,
                station,
                epoch,
                network,
                security,
                control,
                context.generation,
            )
            .await
        }
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        RadioHilStaLifecycleFailure::CandidateRefreshContract
    }
}

impl<'fixture, 'security>
    Esp32s31StationEngineObserver<
        'security,
        CriticalSectionRawMutex,
        RadioHilStationEnginePort<'fixture>,
    > for RadioHilStationEngineObserver
{
    fn attempt_started(
        &mut self,
        context: StaAttemptContext,
        phase: Esp32s31StationServicePhaseKind,
    ) {
        let phase = match phase {
            Esp32s31StationServicePhaseKind::InitialScan => "initial-scan",
            Esp32s31StationServicePhaseKind::InitialJoin => "authentication",
            Esp32s31StationServicePhaseKind::RunningScan => "running-scan",
            Esp32s31StationServicePhaseKind::Reconnected => "reconnect",
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=OBSERVE \
             stage=production-sta-lifecycle-attempt generation={} attempt={} \
             refresh_candidate={} phase={phase}",
            context.generation,
            context.attempt,
            u8::from(context.refresh_candidate),
        ));
    }

    fn attempt_finished<'a>(
        &'a mut self,
        context: StaAttemptContext,
        outcome: &'a StaAttemptOutcome<
            RadioHilStaLifecycleOwner<'fixture, 'security>,
            RadioHilStaLifecycleFailure,
        >,
    ) -> impl Future<Output = ()> + 'a
    where
        'security: 'a,
    {
        async move {
            if let StaAttemptOutcome::Failed { failure, .. } = outcome {
                crate::console::publish_station_lifecycle(StationLifecycleEvent::AttemptFailed {
                    generation: context.generation,
                    attempt: context.attempt,
                    stage: protocol_station_failure_stage(failure.stage),
                    reason: protocol_station_failure_reason(failure.error),
                })
                .await;
            }
        }
    }

    fn command_deferred(&mut self, command: Esp32s31StationCommand, accepted: bool) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=OBSERVE \
             stage=production-station-command command={command:?} \
             action=deferred deferred={}",
            u8::from(accepted),
        ));
    }

    fn backoff_started(&mut self, delay_millis: u32, reason: StaBackoffReason) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=OBSERVE \
             stage=production-sta-lifecycle-backoff delay_ms={delay_millis} \
             reason={reason:?}"
        ));
    }
}
