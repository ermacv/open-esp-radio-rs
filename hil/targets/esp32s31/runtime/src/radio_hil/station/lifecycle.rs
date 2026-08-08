#![forbid(unsafe_code)]

use core::{future::Future, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::wifi::sta::{
    scan::StaScanPlanError,
    station::{
        StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffReason,
        StaFailureDisposition, StaLifecycleStage,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::station::{
    Esp32s31StationAttemptRunner, Esp32s31StationCommand,
};
use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationFailureStage, StationLifecycleEvent,
};

use super::{
    RadioHilStaLifecycleOwner,
    attempts::{
        run_initial_station_attempt, run_reconnected_station_attempt, run_running_scan_attempt,
    },
};
use crate::{
    console::emergency_log,
    radio_hil::{RadioHilRunningScanPortError, RadioHilStationCommandReceiver},
};
use open_esp_radio::esp32s31::wifi::sta::{
    attempt::Esp32s31StaAttemptStage, scan::Esp32s31StaScanError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilStaLifecycleFailure {
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
    InvalidEpochOwner,
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
        RadioHilStaLifecycleFailure::RunningScanNoCandidate => {
            StationAttemptFailureReason::NoCandidate
        }
        RadioHilStaLifecycleFailure::Authentication
        | RadioHilStaLifecycleFailure::InitialJoin { .. }
        | RadioHilStaLifecycleFailure::StationAttempt(_) => {
            StationAttemptFailureReason::PeerProtocol
        }
        RadioHilStaLifecycleFailure::RunningScanTransaction(_)
        | RadioHilStaLifecycleFailure::ConnectedHardware => StationAttemptFailureReason::Hardware,
        RadioHilStaLifecycleFailure::CandidateRefreshContract
        | RadioHilStaLifecycleFailure::RunningScanPlan(_)
        | RadioHilStaLifecycleFailure::InvalidEpochOwner => {
            StationAttemptFailureReason::ContractViolation
        }
    }
}

pub(in crate::radio_hil) struct RadioHilStaAttemptRunner<O> {
    _owner: PhantomData<fn() -> O>,
}

impl<O> RadioHilStaAttemptRunner<O> {
    pub(in crate::radio_hil) const fn new() -> Self {
        Self {
            _owner: PhantomData,
        }
    }
}

impl<'fixture, 'security> Esp32s31StationAttemptRunner<CriticalSectionRawMutex>
    for RadioHilStaAttemptRunner<RadioHilStaLifecycleOwner<'fixture, 'security>>
{
    type Owner = RadioHilStaLifecycleOwner<'fixture, 'security>;
    type Error = RadioHilStaLifecycleFailure;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
        station_control: &'a mut RadioHilStationCommandReceiver<'_>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        async move {
            let phase = match &owner {
                RadioHilStaLifecycleOwner::Authenticate(_) => "authentication",
                RadioHilStaLifecycleOwner::RunningScan(_) => "running-scan",
                RadioHilStaLifecycleOwner::Reconnect(_) => "reconnect",
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE \
                 stage=production-sta-lifecycle-attempt generation={} attempt={} \
                 refresh_candidate={} phase={phase}",
                context.generation,
                context.attempt,
                u8::from(context.refresh_candidate),
            ));
            let outcome = match owner {
                RadioHilStaLifecycleOwner::Authenticate(ready) => {
                    run_initial_station_attempt(ready, station_control, context.generation).await
                }
                RadioHilStaLifecycleOwner::RunningScan(ready) => {
                    if context.refresh_candidate {
                        run_running_scan_attempt(ready, station_control, context.generation).await
                    } else {
                        StaAttemptOutcome::Failed {
                            owner: RadioHilStaLifecycleOwner::RunningScan(ready),
                            failure: StaAttemptFailure::new(
                                StaLifecycleStage::CandidateSelection,
                                StaFailureDisposition::Terminal,
                                RadioHilStaLifecycleFailure::CandidateRefreshContract,
                            ),
                        }
                    }
                }
                RadioHilStaLifecycleOwner::Reconnect(ready) => {
                    run_reconnected_station_attempt(ready, station_control, context.generation)
                        .await
                }
            };
            if let StaAttemptOutcome::Failed { failure, .. } = &outcome {
                crate::console::publish_station_lifecycle(StationLifecycleEvent::AttemptFailed {
                    generation: context.generation,
                    attempt: context.attempt,
                    stage: protocol_station_failure_stage(failure.stage),
                    reason: protocol_station_failure_reason(failure.error),
                })
                .await;
            }
            outcome
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
