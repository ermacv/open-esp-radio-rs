//! Pinned role drive, cooperative stop and classified completion.

use core::future::Future;

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_wifi_embassy::stack_boundary::stack_poll;

use crate::wifi::{WifiScanFailure, WifiStartFailure, WifiStopReport};

use super::message::{
    EmbassyWifiStartKind, EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorResponse,
};
use super::transport::EmbassyWifiSupervisorEndpoint;

/// Role-local control retained beside an active owner future by the physical
/// supervisor actor.
///
/// Request publication is deliberately synchronous. Waiting for completion
/// here would deadlock because the same actor must continue polling the role
/// future which performs DMA/IRQ quiescence.
pub trait EmbassyWifiActiveRoleControl {
    fn request_stop(&mut self);
}

/// Terminal active-role observation returned to the owner-holding supervisor.
///
/// `stop_requested` reports that the application is still waiting for its
/// `Stop` response. It does not classify `output` as quiescent: the concrete
/// supervisor must first inspect that owner-bearing output and reconstruct a
/// stopped or faulted frontier.
pub struct EmbassyWifiActiveRoleExit<O> {
    output: O,
    stop_requested: bool,
}

impl<O> EmbassyWifiActiveRoleExit<O> {
    pub fn into_parts(self) -> (O, bool) {
        (self.output, self.stop_requested)
    }

    pub const fn output(&self) -> &O {
        &self.output
    }

    pub const fn stop_requested(&self) -> bool {
        self.stop_requested
    }
}

/// Owner frontier retained by the physical supervisor after a role future
/// terminates.
///
/// Only `Stopped` is reusable. `Faulted` intentionally keeps the exact
/// quarantined owner instead of erasing it into an error code.
pub enum EmbassyWifiRoleFrontier<S, F> {
    Stopped(S),
    Faulted(F),
}

/// Poll one `!Send` owner future locally while continuing to service the
/// hardware-free supervisor mailbox.
///
/// The future is pinned inside this call and never enters `Channel`, `Signal`
/// or a detached executor task. A `Stop` command only publishes cooperative
/// stop through `control`; no response is emitted. After the role returns, the
/// caller must classify `output`, retain the returned owner frontier and only
/// then respond to the pending stop.
///
/// Start commands received while the role is active are rejected here with
/// their untouched request. `active_start_error` only constructs the service
/// error, so a caller cannot accidentally return a station response to a
/// monitor command or otherwise violate the mailbox protocol.
pub async fn drive_embassy_wifi_active_role<M, E, C, F, R>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    control: &mut C,
    role: F,
    active_start_error: R,
) -> EmbassyWifiActiveRoleExit<F::Output>
where
    M: RawMutex,
    C: EmbassyWifiActiveRoleControl,
    F: Future,
    R: FnMut(EmbassyWifiStartKind) -> E,
{
    let mut role = core::pin::pin!(role);
    drive_embassy_wifi_active_role_pinned(endpoint, control, role.as_mut(), active_start_error)
        .await
}

/// Borrowed variant for callers which already store a large role future in
/// their own async state. This avoids moving that future through another
/// owner future solely to service the supervisor mailbox.
pub async fn drive_embassy_wifi_active_role_pinned<M, E, C, F, R>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    control: &mut C,
    mut role: core::pin::Pin<&mut F>,
    mut active_start_error: R,
) -> EmbassyWifiActiveRoleExit<F::Output>
where
    M: RawMutex,
    C: EmbassyWifiActiveRoleControl,
    F: Future,
    R: FnMut(EmbassyWifiStartKind) -> E,
{
    let mut stop_requested = false;
    loop {
        match select(stack_poll(role.as_mut()), endpoint.receive()).await {
            Either::First(output) => {
                return EmbassyWifiActiveRoleExit {
                    output,
                    stop_requested,
                };
            }
            Either::Second(EmbassyWifiSupervisorCommand::Stop) => {
                if !stop_requested {
                    control.request_stop();
                    stop_requested = true;
                }
            }
            Either::Second(EmbassyWifiSupervisorCommand::RestartRadio) => {
                let error = active_start_error(EmbassyWifiStartKind::WholeRadioRestart);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::RestartRadio(Err(error)))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::CycleRetainedRadio) => {
                let error = active_start_error(EmbassyWifiStartKind::WholeRadioRetainedCycle);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::CycleRetainedRadio(Err(
                        error,
                    )))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::Scan(request)) => {
                let error = active_start_error(EmbassyWifiStartKind::StandaloneScan);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected { request, error },
                    )))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::StartStation(request)) => {
                let error = active_start_error(EmbassyWifiStartKind::Station);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Station(Err(
                        WifiStartFailure::rejected(request, error),
                    )))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::StartAccessPoint(request)) => {
                let error = active_start_error(EmbassyWifiStartKind::AccessPoint);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::rejected(request, error),
                    )))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::StartStationAccessPoint(request)) => {
                let error = active_start_error(EmbassyWifiStartKind::StationAccessPoint);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::rejected(request, error),
                    )))
                    .await;
            }
            Either::Second(EmbassyWifiSupervisorCommand::StartMonitor(request)) => {
                let error = active_start_error(EmbassyWifiStartKind::StandaloneMonitor);
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                        WifiStartFailure::rejected(request, error),
                    )))
                    .await;
            }
        }
    }
}

/// Classify a returned role owner and complete a pending application stop.
///
/// This is the only generic helper which emits the successful stop response.
/// Classification happens first. A reusable frontier produces `Ok`, while a
/// faulted frontier remains owned by the caller and produces a service error
/// derived without consuming that owner.
pub async fn finish_embassy_wifi_active_role<M, E, O, S, F, Classify, FaultError>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    generation: crate::wifi::RadioSubsystemGeneration,
    exit: EmbassyWifiActiveRoleExit<O>,
    classify: Classify,
    fault_error: FaultError,
) -> EmbassyWifiRoleFrontier<S, F>
where
    M: RawMutex,
    Classify: FnOnce(O) -> EmbassyWifiRoleFrontier<S, F>,
    FaultError: FnOnce(&F) -> E,
{
    let (output, stop_requested) = exit.into_parts();
    let frontier = classify(output);
    if stop_requested {
        let result = match &frontier {
            EmbassyWifiRoleFrontier::Stopped(_) => Ok(WifiStopReport::new(generation)),
            EmbassyWifiRoleFrontier::Faulted(faulted) => Err(fault_error(faulted)),
        };
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Stop(result))
            .await;
    }
    frontier
}
