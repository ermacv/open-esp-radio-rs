//! One-slot application/owner transport and cancellation reconciliation.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};

use oer_radio::wifi::{
    AccessPointRequest, MonitorRequest, RadioController, StationAccessPointRequest, StationRequest,
    WifiIdle, WifiRadioRestartReport, WifiRadioRetainedCycleReport, WifiScanFailure,
    WifiScanReport, WifiScanRequest, WifiStartFailure, WifiStartResult, WifiStopReport,
    WifiSupervisorPort,
};

use super::message::{
    EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorError, EmbassyWifiSupervisorResponse,
};

/// One outstanding controller command and one bounded completion.
const MAILBOX_CAPACITY: usize = 1;

/// Static storage for one application endpoint and one supervisor endpoint.
///
/// It contains only owned requests, value reports and wake state. PAC, DMA,
/// IRQ and protocol owners never enter this object.
pub struct EmbassyWifiSupervisorControlResources<M: RawMutex, E> {
    split: AtomicBool,
    supervisor_alive: AtomicBool,
    commands: Channel<M, EmbassyWifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Channel<M, EmbassyWifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
}

pub type EmbassyWifiSupervisorEndpoints<'resources, M, E> = (
    RadioController<EmbassyWifiSupervisorPort<'resources, M, E>>,
    EmbassyWifiSupervisorEndpoint<'resources, M, E>,
);

impl<M: RawMutex, E> EmbassyWifiSupervisorControlResources<M, E> {
    pub const fn new() -> Self {
        Self {
            split: AtomicBool::new(false),
            supervisor_alive: AtomicBool::new(false),
            commands: Channel::new(),
            responses: Channel::new(),
        }
    }

    /// Permanently split this mailbox for one firmware supervisor task.
    ///
    /// Recreating endpoints after either side disappears could consume stale
    /// requests or completions, so a second split is rejected until physical
    /// radio reset also reconstructs this storage.
    pub fn split(
        &self,
    ) -> Result<EmbassyWifiSupervisorEndpoints<'_, M, E>, EmbassyWifiSupervisorControlError> {
        if self
            .split
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(EmbassyWifiSupervisorControlError::InUse);
        }
        self.supervisor_alive.store(true, Ordering::Release);
        let port = EmbassyWifiSupervisorPort {
            commands: self.commands.sender(),
            responses: self.responses.receiver(),
            supervisor_alive: &self.supervisor_alive,
            completion_pending: false,
        };
        let endpoint = EmbassyWifiSupervisorEndpoint {
            commands: self.commands.receiver(),
            responses: self.responses.sender(),
            supervisor_alive: &self.supervisor_alive,
        };
        Ok((RadioController::new(WifiIdle::new(port)), endpoint))
    }

    #[cfg(test)]
    pub(super) fn no_pending_command_for_test(&self) -> bool {
        self.commands.try_receive().is_err()
    }
}

impl<M: RawMutex, E> Default for EmbassyWifiSupervisorControlResources<M, E> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyWifiSupervisorControlError {
    InUse,
}

/// Application-side mailbox transport. It owns no hardware capability.
pub struct EmbassyWifiSupervisorPort<'resources, M: RawMutex, E> {
    commands: Sender<'resources, M, EmbassyWifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Receiver<'resources, M, EmbassyWifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
    supervisor_alive: &'resources AtomicBool,
    completion_pending: bool,
}

impl<M: RawMutex, E> EmbassyWifiSupervisorPort<'_, M, E> {
    async fn response(&mut self) -> EmbassyWifiSupervisorResponse<E> {
        self.responses.receive().await
    }

    fn supervisor_available(&self) -> bool {
        self.supervisor_alive.load(Ordering::Acquire)
    }

    /// Consume the completion of a command whose caller future was dropped.
    ///
    /// The public role API consumes its capability, so this path principally
    /// protects direct internal users of the transport. The mailbox remains a
    /// strict one-command transaction: a later request is never paired with a
    /// stale completion.
    async fn reconcile_cancelled_command(&mut self) {
        if self.completion_pending {
            let _ = self.response().await;
            self.completion_pending = false;
        }
    }

    async fn publish(
        &mut self,
        command: EmbassyWifiSupervisorCommand,
    ) -> Result<(), EmbassyWifiSupervisorCommand> {
        self.reconcile_cancelled_command().await;
        // Reconciliation may have waited while the sole endpoint disappeared.
        // In that case its terminal response belongs to the cancelled command,
        // not to this still-owned, unpublished request.
        if !self.supervisor_available() {
            return Err(command);
        }
        self.commands.send(command).await;
        self.completion_pending = true;
        Ok(())
    }

    async fn completion(&mut self) -> EmbassyWifiSupervisorResponse<E> {
        let response = self.response().await;
        self.completion_pending = false;
        response
    }
}

impl<M: RawMutex, E> WifiSupervisorPort for EmbassyWifiSupervisorPort<'_, M, E> {
    type Error = EmbassyWifiSupervisorError<E>;

    async fn scan(
        &mut self,
        request: WifiScanRequest,
    ) -> Result<WifiScanReport, WifiScanFailure<WifiScanRequest, Self::Error>> {
        if !self.supervisor_available() {
            return Err(WifiScanFailure::Rejected {
                request,
                error: EmbassyWifiSupervisorError::SupervisorUnavailable,
            });
        }
        if let Err(EmbassyWifiSupervisorCommand::Scan(request)) = self
            .publish(EmbassyWifiSupervisorCommand::Scan(request))
            .await
        {
            return Err(WifiScanFailure::Rejected {
                request,
                error: EmbassyWifiSupervisorError::SupervisorUnavailable,
            });
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::Scan(result) => result.map_err(map_scan_failure),
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => Err(WifiScanFailure::Faulted {
                error: EmbassyWifiSupervisorError::SupervisorUnavailable,
            }),
            _ => Err(WifiScanFailure::Faulted {
                error: EmbassyWifiSupervisorError::ResponseMismatch,
            }),
        }
    }

    async fn start_station(
        &mut self,
        request: StationRequest,
    ) -> WifiStartResult<StationRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(EmbassyWifiSupervisorCommand::StartStation(request)) = self
            .publish(EmbassyWifiSupervisorCommand::StartStation(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::Station(result) => result.map_err(map_start_failure),
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_access_point(
        &mut self,
        request: AccessPointRequest,
    ) -> WifiStartResult<AccessPointRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(EmbassyWifiSupervisorCommand::StartAccessPoint(request)) = self
            .publish(EmbassyWifiSupervisorCommand::StartAccessPoint(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::AccessPoint(result) => result.map_err(map_start_failure),
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_station_access_point(
        &mut self,
        request: StationAccessPointRequest,
    ) -> WifiStartResult<StationAccessPointRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(EmbassyWifiSupervisorCommand::StartStationAccessPoint(request)) = self
            .publish(EmbassyWifiSupervisorCommand::StartStationAccessPoint(
                request,
            ))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::StationAccessPoint(result) => {
                result.map_err(map_start_failure)
            }
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_monitor(
        &mut self,
        request: MonitorRequest,
    ) -> WifiStartResult<MonitorRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(EmbassyWifiSupervisorCommand::StartMonitor(request)) = self
            .publish(EmbassyWifiSupervisorCommand::StartMonitor(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::Monitor(result) => result.map_err(map_start_failure),
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                EmbassyWifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn stop(&mut self) -> Result<WifiStopReport, Self::Error> {
        if !self.supervisor_available() {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        if self
            .publish(EmbassyWifiSupervisorCommand::Stop)
            .await
            .is_err()
        {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::Stop(result) => {
                result.map_err(EmbassyWifiSupervisorError::Service)
            }
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => {
                Err(EmbassyWifiSupervisorError::SupervisorUnavailable)
            }
            _ => Err(EmbassyWifiSupervisorError::ResponseMismatch),
        }
    }

    async fn restart_radio(&mut self) -> Result<WifiRadioRestartReport, Self::Error> {
        if !self.supervisor_available() {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        if self
            .publish(EmbassyWifiSupervisorCommand::RestartRadio)
            .await
            .is_err()
        {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::RestartRadio(result) => {
                result.map_err(EmbassyWifiSupervisorError::Service)
            }
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => {
                Err(EmbassyWifiSupervisorError::SupervisorUnavailable)
            }
            _ => Err(EmbassyWifiSupervisorError::ResponseMismatch),
        }
    }

    async fn cycle_retained_radio(&mut self) -> Result<WifiRadioRetainedCycleReport, Self::Error> {
        if !self.supervisor_available() {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        if self
            .publish(EmbassyWifiSupervisorCommand::CycleRetainedRadio)
            .await
            .is_err()
        {
            return Err(EmbassyWifiSupervisorError::SupervisorUnavailable);
        }
        match self.completion().await {
            EmbassyWifiSupervisorResponse::CycleRetainedRadio(result) => {
                result.map_err(EmbassyWifiSupervisorError::Service)
            }
            EmbassyWifiSupervisorResponse::SupervisorUnavailable => {
                Err(EmbassyWifiSupervisorError::SupervisorUnavailable)
            }
            _ => Err(EmbassyWifiSupervisorError::ResponseMismatch),
        }
    }
}

fn map_start_failure<R, E>(
    failure: WifiStartFailure<R, E>,
) -> WifiStartFailure<R, EmbassyWifiSupervisorError<E>> {
    match failure {
        WifiStartFailure::Rejected { request, error } => {
            WifiStartFailure::rejected(request, EmbassyWifiSupervisorError::Service(error))
        }
        WifiStartFailure::Faulted { error } => {
            WifiStartFailure::faulted(EmbassyWifiSupervisorError::Service(error))
        }
    }
}

fn map_scan_failure<R, E>(
    failure: WifiScanFailure<R, E>,
) -> WifiScanFailure<R, EmbassyWifiSupervisorError<E>> {
    match failure {
        WifiScanFailure::Rejected { request, error } => WifiScanFailure::Rejected {
            request,
            error: EmbassyWifiSupervisorError::Service(error),
        },
        WifiScanFailure::Returned { request, error } => WifiScanFailure::Returned {
            request,
            error: EmbassyWifiSupervisorError::Service(error),
        },
        WifiScanFailure::Faulted { error } => WifiScanFailure::Faulted {
            error: EmbassyWifiSupervisorError::Service(error),
        },
    }
}

/// Sole command endpoint held by the task which owns the radio state machine.
pub struct EmbassyWifiSupervisorEndpoint<'resources, M: RawMutex, E> {
    commands: Receiver<'resources, M, EmbassyWifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Sender<'resources, M, EmbassyWifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
    supervisor_alive: &'resources AtomicBool,
}

impl<M: RawMutex, E> EmbassyWifiSupervisorEndpoint<'_, M, E> {
    pub async fn receive(&mut self) -> EmbassyWifiSupervisorCommand {
        self.commands.receive().await
    }

    pub async fn respond(&mut self, response: EmbassyWifiSupervisorResponse<E>) {
        self.responses.send(response).await;
    }
}

impl<M: RawMutex, E> Drop for EmbassyWifiSupervisorEndpoint<'_, M, E> {
    fn drop(&mut self) {
        self.supervisor_alive.store(false, Ordering::Release);
        if let Err(TrySendError::Full(_)) = self
            .responses
            .try_send(EmbassyWifiSupervisorResponse::SupervisorUnavailable)
        {
            // One prior completion already wakes the only controller waiter.
            // Its next operation observes `supervisor_alive == false` before
            // publishing another command.
        }
    }
}
