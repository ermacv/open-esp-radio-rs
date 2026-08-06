#![forbid(unsafe_code)]

use core::{future::Future, marker::PhantomData};

use embassy_futures::select::{Either, select};
use embassy_time::Timer;
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::station::Esp32s31StationCommand,
    wifi::sta::{
        scan::StaScanPlanError,
        station::{
            StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffOutcome,
            StaBackoffReason, StaFailureDisposition, StaLifecycleBackend, StaLifecycleStage,
        },
    },
};
use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationFailureStage, StationLifecycleEvent,
};

use super::RadioHilStaLifecycleOwner;
use crate::{
    console::emergency_log,
    radio_hil::{
        RadioHilRunningScanPortError, RadioHilStationCommandReceiver, run_initial_station_attempt,
        run_reconnected_station_attempt, run_running_scan_attempt,
    },
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

pub(in crate::radio_hil) struct RadioHilStaLifecycleBackend<'control, O> {
    station_control: RadioHilStationCommandReceiver<'control>,
    _owner: PhantomData<fn() -> O>,
}

impl<'control, O> RadioHilStaLifecycleBackend<'control, O> {
    pub(in crate::radio_hil) const fn new(
        station_control: RadioHilStationCommandReceiver<'control>,
    ) -> Self {
        Self {
            station_control,
            _owner: PhantomData,
        }
    }
}

impl<'control, 'fixture, 'security> StaLifecycleBackend
    for RadioHilStaLifecycleBackend<'control, RadioHilStaLifecycleOwner<'fixture, 'security>>
{
    type Owner = RadioHilStaLifecycleOwner<'fixture, 'security>;
    type Error = RadioHilStaLifecycleFailure;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
        async move {
            if let Some(command) = self.station_control.try_take() {
                match command {
                    Esp32s31StationCommand::Reconnect => {
                        let deferred = self.station_control.defer(command);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=OBSERVE \
                             stage=production-station-command command=reconnect \
                             action=deferred deferred={}",
                            u8::from(deferred),
                        ));
                    }
                    Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop => {
                        self.station_control.record_terminal(command);
                        return StaAttemptOutcome::Stopped { owner };
                    }
                }
            }
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
                    run_initial_station_attempt(
                        ready,
                        &mut self.station_control,
                        context.generation,
                    )
                    .await
                }
                RadioHilStaLifecycleOwner::RunningScan(ready) => {
                    if context.refresh_candidate {
                        run_running_scan_attempt(
                            ready,
                            &mut self.station_control,
                            context.generation,
                        )
                        .await
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
                    run_reconnected_station_attempt(
                        ready,
                        &mut self.station_control,
                        context.generation,
                    )
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

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
        async move {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE \
                 stage=production-sta-lifecycle-backoff delay_ms={delay_millis} \
                 reason={reason:?}"
            ));
            match select(
                Timer::after_millis(u64::from(delay_millis)),
                self.station_control.wait(),
            )
            .await
            {
                Either::First(()) => StaBackoffOutcome::Elapsed { owner },
                Either::Second(command @ Esp32s31StationCommand::Reconnect) => {
                    let _ = self.station_control.defer(command);
                    StaBackoffOutcome::Elapsed { owner }
                }
                Either::Second(
                    command @ (Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop),
                ) => {
                    self.station_control.record_terminal(command);
                    StaBackoffOutcome::Stopped { owner }
                }
            }
        }
    }
}
