//! Owner-independent stopped-state planning and typed rejection.

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_radio::wifi::{
    WifiScanFailure, WifiServicePlanningError, WifiServiceRequest, WifiStartFailure,
    WifiStopReport, WifiSupervisorConfiguration,
};

use super::message::{EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorResponse};
use super::transport::EmbassyWifiSupervisorEndpoint;

/// Result of one command received while the supervisor holds `WifiStopped`.
///
/// `Handled` means an idempotent stop or a rejected start was already answered
/// without moving hardware. `Start` carries a fully capability-checked,
/// owner-independent service request which the concrete actor may now
/// materialize by consuming its stopped owner.
#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc dispatch transfers the complete validated service request to the stopped owner"
)]
pub enum EmbassyWifiStoppedDispatch {
    Handled,
    Start(WifiServiceRequest),
    RestartRadio,
    CycleRetainedRadio,
}

/// Validate and dispatch one command while the concrete actor owns a reusable
/// stopped Wi-Fi frontier.
///
/// Planning happens before any PAC/DMA/IRQ owner moves. Rejected starts return
/// their exact request, and an already-stopped `Stop` is acknowledged
/// idempotently with the current generation.
pub async fn dispatch_embassy_wifi_stopped_command<M, E, PlanningError>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    configuration: WifiSupervisorConfiguration,
    generation: oer_radio::wifi::RadioSubsystemGeneration,
    mut planning_error: PlanningError,
) -> EmbassyWifiStoppedDispatch
where
    M: RawMutex,
    PlanningError: FnMut(WifiServicePlanningError) -> E,
{
    match endpoint.receive().await {
        EmbassyWifiSupervisorCommand::Scan(request) => match configuration.plan_scan(request) {
            Ok(service) => EmbassyWifiStoppedDispatch::Start(service),
            Err(failure) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected {
                            request: failure.request,
                            error: planning_error(failure.error),
                        },
                    )))
                    .await;
                EmbassyWifiStoppedDispatch::Handled
            }
        },
        EmbassyWifiSupervisorCommand::StartStation(request) => {
            match configuration.plan_station(request) {
                Ok(service) => EmbassyWifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Station(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    EmbassyWifiStoppedDispatch::Handled
                }
            }
        }
        EmbassyWifiSupervisorCommand::StartAccessPoint(request) => {
            match configuration.plan_access_point(request) {
                Ok(service) => EmbassyWifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    EmbassyWifiStoppedDispatch::Handled
                }
            }
        }
        EmbassyWifiSupervisorCommand::StartStationAccessPoint(request) => {
            match configuration.plan_station_access_point(request) {
                Ok(service) => EmbassyWifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    EmbassyWifiStoppedDispatch::Handled
                }
            }
        }
        EmbassyWifiSupervisorCommand::StartMonitor(request) => {
            match configuration.plan_monitor(request) {
                Ok(service) => EmbassyWifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    EmbassyWifiStoppedDispatch::Handled
                }
            }
        }
        EmbassyWifiSupervisorCommand::Stop => {
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                    WifiStopReport::new(generation),
                )))
                .await;
            EmbassyWifiStoppedDispatch::Handled
        }
        EmbassyWifiSupervisorCommand::RestartRadio => EmbassyWifiStoppedDispatch::RestartRadio,
        EmbassyWifiSupervisorCommand::CycleRetainedRadio => {
            EmbassyWifiStoppedDispatch::CycleRetainedRadio
        }
    }
}
