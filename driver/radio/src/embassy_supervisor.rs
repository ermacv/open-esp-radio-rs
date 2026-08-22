//! Embassy mailbox for the hardware-free radio controller.

use core::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};
use open_esp_radio_wifi_embassy::{await_stack_boundary, stack_boundary::stack_poll};

use crate::{
    AccessPointRequest, MonitorRequest, RadioController, StationAccessPointRequest, StationRequest,
    WifiIdle, WifiScanFailure, WifiScanReport, WifiScanRequest, WifiServicePlanningError,
    WifiServiceRequest, WifiStartFailure, WifiStartResult, WifiStopReport,
    WifiSupervisorConfiguration, WifiSupervisorPort,
};

/// Exactly one command is outstanding because the public controller requires
/// a mutable borrow for every operation.
const MAILBOX_CAPACITY: usize = 1;

/// Request transported to the sole owner-holding radio-supervisor task.
pub enum EmbassyWifiSupervisorCommand {
    Scan(WifiScanRequest),
    StartStation(StationRequest),
    StartAccessPoint(AccessPointRequest),
    StartStationAccessPoint(StationAccessPointRequest),
    StartMonitor(MonitorRequest),
    Stop,
}

/// Role requested while another Wi-Fi role graph is already active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyWifiStartKind {
    StandaloneScan,
    Station,
    AccessPoint,
    StationAccessPoint,
    StandaloneMonitor,
}

/// Result of one command received while the supervisor holds `WifiStopped`.
///
/// `Handled` means an idempotent stop or a rejected start was already answered
/// without moving hardware. `Start` carries a fully capability-checked,
/// owner-independent service request which the concrete actor may now
/// materialize by consuming its stopped owner.
pub enum EmbassyWifiStoppedDispatch {
    Handled,
    Start(WifiServiceRequest),
}

/// Complete result of one locally executed role epoch.
///
/// `NotStarted` means materialization rejected the request and already sent a
/// typed response, so the generation does not advance. `Stopped` means start
/// was acknowledged and the complete reusable owner returned. `Faulted`
/// retains the exact quarantined frontier and cannot re-enter stopped state.
pub enum EmbassyWifiRoleEpochOutcome<S, F> {
    NotStarted(S),
    Stopped(S),
    Faulted(F),
}

/// Concrete local-role composition used by the physical Embassy supervisor.
///
/// Unlike a `start() -> Running` backend, this contract encloses the complete
/// active epoch in one future. Implementations materialize a role, acknowledge
/// successful start, poll its `!Send` owner future alongside `endpoint`,
/// classify its terminal owner and complete a pending stop before returning.
/// No live owner may escape in a detached task or synchronized channel.
pub trait EmbassyWifiRoleEpochRunner<M: RawMutex> {
    type Stopped;
    type Faulted;
    type Error;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error;

    /// Translate a retained hardware fault into the public control-plane
    /// error. This classifies the state; it does not perform recovery.
    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error;

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, M, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: crate::RadioSubsystemGeneration,
    ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a;
}

/// Typed completion transported back to the application controller.
pub enum EmbassyWifiSupervisorResponse<E> {
    Scan(Result<WifiScanReport, WifiScanFailure<WifiScanRequest, E>>),
    Station(WifiStartResult<StationRequest, E>),
    AccessPoint(WifiStartResult<AccessPointRequest, E>),
    StationAccessPoint(WifiStartResult<StationAccessPointRequest, E>),
    Monitor(WifiStartResult<MonitorRequest, E>),
    Stop(Result<WifiStopReport, E>),
    SupervisorUnavailable,
}

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

/// Control transport failure distinct from a backend service error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyWifiSupervisorError<E> {
    SupervisorUnavailable,
    ResponseMismatch,
    Service(E),
}

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

/// Prepared owner-holding supervisor actor.
///
/// The application receives only the hardware-free [`RadioController`]. This
/// value retains the physical endpoint, reusable stopped frontier and role
/// runner together, so application code cannot detach or accidentally drive
/// an internal endpoint independently of the hardware owner.
pub struct EmbassyWifiSupervisorTask<'resources, M, R>
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    endpoint: EmbassyWifiSupervisorEndpoint<'resources, M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
}

impl<M, R> EmbassyWifiSupervisorTask<'_, M, R>
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    /// Run the sole physical radio actor forever.
    pub async fn run(self) -> ! {
        run_embassy_wifi_supervisor_actor(
            self.endpoint,
            self.configuration,
            self.runner,
            self.stopped,
        )
        .await
    }
}

/// Failed preparation with every movable owner retained.
pub struct EmbassyWifiSupervisorPrepareFailure<R, S> {
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: S,
}

impl<R, S> EmbassyWifiSupervisorPrepareFailure<R, S> {
    pub const fn error(&self) -> EmbassyWifiSupervisorControlError {
        EmbassyWifiSupervisorControlError::InUse
    }

    pub fn into_parts(self) -> (WifiSupervisorConfiguration, R, S) {
        (self.configuration, self.runner, self.stopped)
    }
}

/// Prepare the controller/actor pair as one operation.
///
/// Failure cannot lose the role runner or stopped hardware frontier.
pub fn prepare_embassy_wifi_supervisor<'resources, M, R>(
    control: &'resources EmbassyWifiSupervisorControlResources<M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
) -> Result<
    (
        RadioController<EmbassyWifiSupervisorPort<'resources, M, R::Error>>,
        EmbassyWifiSupervisorTask<'resources, M, R>,
    ),
    EmbassyWifiSupervisorPrepareFailure<R, R::Stopped>,
>
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    let (controller, endpoint) = match control.split() {
        Ok(endpoints) => endpoints,
        Err(EmbassyWifiSupervisorControlError::InUse) => {
            return Err(EmbassyWifiSupervisorPrepareFailure {
                configuration,
                runner,
                stopped,
            });
        }
    };
    Ok((
        controller,
        EmbassyWifiSupervisorTask {
            endpoint,
            configuration,
            runner,
            stopped,
        },
    ))
}

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

    async fn publish(&mut self, command: EmbassyWifiSupervisorCommand) {
        self.reconcile_cancelled_command().await;
        self.commands.send(command).await;
        self.completion_pending = true;
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
        self.publish(EmbassyWifiSupervisorCommand::Scan(request))
            .await;
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
        self.publish(EmbassyWifiSupervisorCommand::StartStation(request))
            .await;
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
        self.publish(EmbassyWifiSupervisorCommand::StartAccessPoint(request))
            .await;
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
        self.publish(EmbassyWifiSupervisorCommand::StartStationAccessPoint(
            request,
        ))
        .await;
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
        self.publish(EmbassyWifiSupervisorCommand::StartMonitor(request))
            .await;
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
        self.publish(EmbassyWifiSupervisorCommand::Stop).await;
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

/// Validate and dispatch one command while the concrete actor owns a reusable
/// stopped Wi-Fi frontier.
///
/// Planning happens before any PAC/DMA/IRQ owner moves. Rejected starts return
/// their exact request, and an already-stopped `Stop` is acknowledged
/// idempotently with the current generation.
pub async fn dispatch_embassy_wifi_stopped_command<M, E, PlanningError>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    configuration: WifiSupervisorConfiguration,
    generation: crate::RadioSubsystemGeneration,
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
    }
}

/// Run the physical Wi-Fi supervisor as one owner-holding local actor.
///
/// Between epochs this function owns `R::Stopped`. During an epoch that owner
/// is moved into `R::run_epoch`, whose future is awaited in this same task. A
/// reusable owner can therefore reappear only through the explicit
/// `NotStarted` or `Stopped` return variants.
pub async fn run_embassy_wifi_supervisor_actor<M, R>(
    mut endpoint: EmbassyWifiSupervisorEndpoint<'_, M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    mut runner: R,
    mut stopped: R::Stopped,
) -> !
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    let mut generation = crate::RadioSubsystemGeneration::INITIAL;
    loop {
        let service = loop {
            match dispatch_embassy_wifi_stopped_command(
                &mut endpoint,
                configuration,
                generation,
                |error| runner.planning_error(error),
            )
            .await
            {
                EmbassyWifiStoppedDispatch::Handled => {}
                EmbassyWifiStoppedDispatch::Start(service) => break service,
            }
        };

        let next_generation = generation.next();
        match await_stack_boundary!(runner.run_epoch(
            &mut endpoint,
            stopped,
            service,
            next_generation
        )) {
            EmbassyWifiRoleEpochOutcome::NotStarted(returned) => stopped = returned,
            EmbassyWifiRoleEpochOutcome::Stopped(returned) => {
                stopped = returned;
                generation = next_generation;
            }
            EmbassyWifiRoleEpochOutcome::Faulted(faulted) => {
                run_embassy_wifi_faulted_actor(&mut endpoint, &mut runner, faulted).await
            }
        }
    }
}

async fn run_embassy_wifi_faulted_actor<M, R>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, R::Error>,
    runner: &mut R,
    faulted: R::Faulted,
) -> !
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    loop {
        let response = match endpoint.receive().await {
            EmbassyWifiSupervisorCommand::Scan(request) => {
                EmbassyWifiSupervisorResponse::Scan(Err(WifiScanFailure::Rejected {
                    request,
                    error: runner.fault_error(&faulted),
                }))
            }
            EmbassyWifiSupervisorCommand::StartStation(request) => {
                EmbassyWifiSupervisorResponse::Station(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartAccessPoint(request) => {
                EmbassyWifiSupervisorResponse::AccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartStationAccessPoint(request) => {
                EmbassyWifiSupervisorResponse::StationAccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartMonitor(request) => {
                EmbassyWifiSupervisorResponse::Monitor(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::Stop => {
                EmbassyWifiSupervisorResponse::Stop(Err(runner.fault_error(&faulted)))
            }
        };
        endpoint.respond(response).await;
    }
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
    generation: crate::RadioSubsystemGeneration,
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

#[cfg(test)]
mod tests {
    use core::{
        cell::Cell,
        future::Future,
        num::NonZeroU16,
        sync::atomic::{AtomicU8, Ordering},
    };
    use std::rc::Rc;

    use embassy_futures::select::{Either, select};
    use embassy_futures::{block_on, join::join};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_sync::signal::Signal;
    use open_esp_radio_ieee80211::{channel::WifiChannel, station::StaAssociationPreference};
    use open_esp_radio_wifi_sta::station::StaReconnectPolicy;
    use open_esp_radio_wpa2::Pmk;

    use crate::{
        MonitorCapturePolicy, StationScanChannels, StationScanPolicy, StationSecurity,
        WIFI_SCAN_RESULT_CAPACITY, WifiMacAddress, WifiMonitorConfig, WifiScanResult, WifiSsid,
        WifiStartReport, WifiStationConfig,
    };

    use super::*;

    fn station_request() -> StationRequest {
        StationRequest::new(
            WifiSsid::new(b"mailbox").unwrap(),
            StationSecurity::wpa2_personal(Pmk::derive(b"password", b"mailbox").unwrap()),
            StaReconnectPolicy::new(2, 10, 100, 10).unwrap(),
            StationScanPolicy::new(
                StationScanChannels::CHANNELS_1_TO_13,
                NonZeroU16::new(10).unwrap(),
                StaAssociationPreference::Automatic,
            ),
        )
    }

    fn scan_request() -> WifiScanRequest {
        WifiScanRequest::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(10).unwrap(),
        )
    }

    fn run<F: Future>(future: F) -> F::Output {
        block_on(future)
    }

    struct TestRoleControl<'a> {
        stop: &'a Signal<NoopRawMutex, ()>,
        stage: &'a AtomicU8,
    }

    impl EmbassyWifiActiveRoleControl for TestRoleControl<'_> {
        fn request_stop(&mut self) {
            self.stage.store(1, Ordering::Release);
            self.stop.signal(());
        }
    }

    struct FakeLocalEpochRunner;

    impl EmbassyWifiRoleEpochRunner<NoopRawMutex> for FakeLocalEpochRunner {
        type Stopped = Rc<Cell<u8>>;
        type Faulted = Rc<Cell<u8>>;
        type Error = &'static str;

        fn planning_error(&mut self, _error: WifiServicePlanningError) -> Self::Error {
            "planning"
        }

        fn fault_error(&mut self, _faulted: &Self::Faulted) -> Self::Error {
            "faulted"
        }

        fn run_epoch<'a>(
            &'a mut self,
            endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, NoopRawMutex, Self::Error>,
            stopped: Self::Stopped,
            service: WifiServiceRequest,
            generation: crate::RadioSubsystemGeneration,
        ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a
        {
            async move {
                stopped.set(stopped.get() + 1);
                if service.scan_request().is_some() {
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Scan(Ok(
                            WifiScanReport::new(
                                generation,
                                [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY],
                                0,
                                0,
                                0,
                            ),
                        )))
                        .await;
                    return EmbassyWifiRoleEpochOutcome::Stopped(stopped);
                }
                let response = if service.station_request().is_some() {
                    EmbassyWifiSupervisorResponse::Station(Ok(WifiStartReport::new(generation)))
                } else if service.monitor_request().is_some() {
                    EmbassyWifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(generation)))
                } else {
                    return EmbassyWifiRoleEpochOutcome::Faulted(stopped);
                };
                endpoint.respond(response).await;

                match endpoint.receive().await {
                    EmbassyWifiSupervisorCommand::Stop => {
                        // In a real runner this response is emitted through
                        // `finish_embassy_wifi_active_role` after classifying
                        // the concrete owner-bearing task exit.
                        endpoint
                            .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                                WifiStopReport::new(generation),
                            )))
                            .await;
                        EmbassyWifiRoleEpochOutcome::Stopped(stopped)
                    }
                    _ => EmbassyWifiRoleEpochOutcome::Faulted(stopped),
                }
            }
        }
    }

    #[test]
    fn controller_and_supervisor_exchange_typed_station_completion() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let (radio, mut endpoint) = resources.split().unwrap();

        let application = async {
            radio
                .into_wifi()
                .start_station(station_request())
                .await
                .unwrap()
        };
        let supervisor = async {
            let EmbassyWifiSupervisorCommand::StartStation(request) = endpoint.receive().await
            else {
                panic!("expected station request")
            };
            assert_eq!(request.ssid().as_bytes(), b"mailbox");
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                    WifiStartReport::new(crate::RadioSubsystemGeneration::INITIAL),
                )))
                .await;
        };
        let (report, ()) = run(join(application, supervisor));
        assert_eq!(
            report.generation(),
            crate::RadioSubsystemGeneration::INITIAL
        );
    }

    #[test]
    fn stopped_dispatch_validates_before_returning_a_start_transaction() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let configuration = WifiSupervisorConfiguration::new(
            open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
        )
        .with_station(WifiStationConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap(),
        ));

        let application = async {
            radio
                .into_wifi()
                .start_station(station_request())
                .await
                .unwrap()
        };
        let supervisor = async {
            let dispatch = dispatch_embassy_wifi_stopped_command(
                &mut endpoint,
                configuration,
                crate::RadioSubsystemGeneration::INITIAL,
                |_| (),
            )
            .await;
            let EmbassyWifiStoppedDispatch::Start(service) = dispatch else {
                panic!("valid provisioned station request must be dispatched")
            };
            assert_eq!(
                service.station_request().unwrap().ssid().as_bytes(),
                b"mailbox"
            );
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                    WifiStartReport::new(crate::RadioSubsystemGeneration::INITIAL.next()),
                )))
                .await;
        };
        let (report, ()) = run(join(application, supervisor));
        assert_eq!(report.generation().value(), 1);
    }

    #[test]
    fn stopped_dispatch_rejects_unprovisioned_start_without_moving_a_request() {
        let resources =
            EmbassyWifiSupervisorControlResources::<NoopRawMutex, WifiServicePlanningError>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let configuration = WifiSupervisorConfiguration::new(
            open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
        );

        let application = async { radio.into_wifi().start_station(station_request()).await };
        let supervisor = async {
            let dispatch = dispatch_embassy_wifi_stopped_command(
                &mut endpoint,
                configuration,
                crate::RadioSubsystemGeneration::INITIAL,
                |error| error,
            )
            .await;
            assert!(matches!(dispatch, EmbassyWifiStoppedDispatch::Handled));
        };
        let (result, ()) = run(join(application, supervisor));
        match result {
            Err(crate::WifiRoleStartFailure::Rejected {
                wifi: _,
                request,
                error:
                    EmbassyWifiSupervisorError::Service(WifiServicePlanningError::StationNotProvisioned),
            }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
            _ => panic!("planning rejection must return the exact station request"),
        }
    }

    #[test]
    fn supervisor_actor_keeps_a_non_send_owner_across_role_epochs() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let configuration = WifiSupervisorConfiguration::new(
            open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
        )
        .with_station(WifiStationConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap(),
        ))
        .with_standalone_scan()
        .with_standalone_monitor();
        let owner = Rc::new(Cell::new(0));
        let observed_owner = Rc::clone(&owner);
        let (radio, task) = match prepare_embassy_wifi_supervisor(
            &resources,
            configuration,
            FakeLocalEpochRunner,
            owner,
        ) {
            Ok(prepared) => prepared,
            Err(_) => panic!("fresh supervisor resources must prepare once"),
        };

        let application = async {
            let scan = radio.into_wifi().scan(scan_request()).await.unwrap();
            let scan_generation = scan.report.generation().value();
            let station = scan.wifi.start_station(station_request()).await.unwrap();
            let station_generation = station.generation().value();
            let wifi = station.stop().await.unwrap();
            let monitor = wifi
                .start_monitor(MonitorRequest::new(
                    WifiChannel::mhz20(6).unwrap(),
                    WifiMonitorConfig::normalized(),
                ))
                .await
                .unwrap();
            let monitor_generation = monitor.generation().value();
            let _wifi = monitor.stop().await.unwrap();
            (scan_generation, station_generation, monitor_generation)
        };
        let Either::First(generations) = run(select(application, task.run()));
        assert_eq!(generations, (1, 2, 3));
        assert_eq!(observed_owner.get(), 3);
    }

    #[test]
    fn supervisor_prepare_failure_returns_runner_and_stopped_owner() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let configuration = WifiSupervisorConfiguration::new(
            open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
        );
        let (_radio, _task) = prepare_embassy_wifi_supervisor(
            &resources,
            configuration,
            FakeLocalEpochRunner,
            Rc::new(Cell::new(1)),
        )
        .unwrap_or_else(|_| panic!("fresh supervisor resources must prepare once"));

        let retained = Rc::new(Cell::new(7));
        let failure = match prepare_embassy_wifi_supervisor(
            &resources,
            configuration,
            FakeLocalEpochRunner,
            Rc::clone(&retained),
        ) {
            Ok(_) => panic!("one static control domain cannot prepare two actors"),
            Err(failure) => failure,
        };
        assert_eq!(failure.error(), EmbassyWifiSupervisorControlError::InUse);
        let (returned_configuration, _runner, returned_owner) = failure.into_parts();
        assert_eq!(returned_configuration, configuration);
        assert!(Rc::ptr_eq(&returned_owner, &retained));
    }

    #[test]
    fn disappearing_supervisor_wakes_an_inflight_controller() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let application = async { radio.into_wifi().start_station(station_request()).await };
        let supervisor = async {
            let _ = endpoint.receive().await;
            drop(endpoint);
        };
        let (result, ()) = run(join(application, supervisor));
        assert!(matches!(
            result,
            Err(crate::WifiRoleStartFailure::Faulted {
                error: EmbassyWifiSupervisorError::SupervisorUnavailable
            })
        ));
    }

    #[test]
    fn cancelled_transport_future_cannot_poison_the_next_command() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let mut port = radio.into_wifi().into_port();
        let cancel = Signal::<NoopRawMutex, ()>::new();
        let cancelled = Signal::<NoopRawMutex, ()>::new();

        let application = async {
            let first = port.start_station(station_request());
            assert!(matches!(
                select(first, cancel.wait()).await,
                Either::Second(())
            ));
            cancelled.signal(());

            let result = port
                .start_monitor(MonitorRequest::new(
                    WifiChannel::mhz20(6).unwrap(),
                    WifiMonitorConfig::normalized(),
                ))
                .await;
            assert!(result.is_ok());
        };
        let supervisor = async {
            assert!(matches!(
                endpoint.receive().await,
                EmbassyWifiSupervisorCommand::StartStation(_)
            ));
            cancel.signal(());
            cancelled.wait().await;
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                    WifiStartReport::new(crate::RadioSubsystemGeneration::INITIAL),
                )))
                .await;

            assert!(matches!(
                endpoint.receive().await,
                EmbassyWifiSupervisorCommand::StartMonitor(_)
            ));
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                    WifiStartReport::new(crate::RadioSubsystemGeneration::INITIAL.next()),
                )))
                .await;
        };

        run(join(application, supervisor));
    }

    #[test]
    fn monitor_policy_remains_an_owned_typed_command() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let request = MonitorRequest::new(
            WifiChannel::mhz20(11).unwrap(),
            WifiMonitorConfig::normalized(),
        )
        .with_capture_policy(MonitorCapturePolicy::truncate_at(
            NonZeroU16::new(256).unwrap(),
        ));
        let application = async { radio.into_wifi().start_monitor(request).await };
        let supervisor = async {
            let EmbassyWifiSupervisorCommand::StartMonitor(request) = endpoint.receive().await
            else {
                panic!("expected monitor request")
            };
            assert_eq!(request.channel().primary(), 11);
            assert_eq!(request.capture_policy().snapshot_length(), Some(256));
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                    WifiStartReport::new(crate::RadioSubsystemGeneration::INITIAL),
                )))
                .await;
        };
        let (result, ()) = run(join(application, supervisor));
        assert!(result.is_ok());
    }

    #[test]
    fn active_role_stop_is_acknowledged_only_after_owner_future_returns() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let stop = Signal::<NoopRawMutex, ()>::new();
        let stage = AtomicU8::new(0);
        let mut control = TestRoleControl {
            stop: &stop,
            stage: &stage,
        };

        let application = async {
            let mut port = radio.into_wifi().into_port();
            port.stop().await.unwrap()
        };
        let supervisor = async {
            let role = async {
                stop.wait().await;
                stage.store(2, Ordering::Release);
                "returned-owner"
            };
            let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |_| {
                panic!("no start command expected")
            })
            .await;
            assert!(exit.stop_requested());
            assert_eq!(stage.load(Ordering::Acquire), 2);

            let frontier = finish_embassy_wifi_active_role(
                &mut endpoint,
                crate::RadioSubsystemGeneration::INITIAL,
                exit,
                EmbassyWifiRoleFrontier::<_, ()>::Stopped,
                |_| unreachable!("the test role returned a stopped owner"),
            )
            .await;
            assert!(matches!(
                frontier,
                EmbassyWifiRoleFrontier::Stopped("returned-owner")
            ));
        };
        let (report, ()) = run(join(application, supervisor));
        assert_eq!(
            report.generation(),
            crate::RadioSubsystemGeneration::INITIAL
        );
        assert_eq!(stage.load(Ordering::Acquire), 2);
    }

    #[test]
    fn faulted_role_never_produces_a_successful_stop_response() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let stop = Signal::<NoopRawMutex, ()>::new();
        let stage = AtomicU8::new(0);
        let mut control = TestRoleControl {
            stop: &stop,
            stage: &stage,
        };

        let application = async {
            let mut port = radio.into_wifi().into_port();
            port.stop().await
        };
        let supervisor = async {
            let role = async {
                stop.wait().await;
                "retained-faulted-owner"
            };
            let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |_| {
                unreachable!("no start command expected")
            })
            .await;
            finish_embassy_wifi_active_role(
                &mut endpoint,
                crate::RadioSubsystemGeneration::INITIAL,
                exit,
                EmbassyWifiRoleFrontier::<(), _>::Faulted,
                |owner| {
                    assert_eq!(*owner, "retained-faulted-owner");
                    "faulted"
                },
            )
            .await
        };

        let (result, frontier) = run(join(application, supervisor));
        assert_eq!(result, Err(EmbassyWifiSupervisorError::Service("faulted")));
        assert!(matches!(
            frontier,
            EmbassyWifiRoleFrontier::Faulted("retained-faulted-owner")
        ));
    }

    #[test]
    fn active_role_rejects_a_new_start_with_the_untouched_request() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let (radio, mut endpoint) = resources.split().unwrap();
        let stop = Signal::<NoopRawMutex, ()>::new();
        let finish = Signal::<NoopRawMutex, ()>::new();
        let stage = AtomicU8::new(0);
        let mut control = TestRoleControl {
            stop: &stop,
            stage: &stage,
        };

        let application = async { radio.into_wifi().start_station(station_request()).await };
        let supervisor = async {
            let role = async {
                finish.wait().await;
                "returned-owner"
            };
            let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |kind| {
                assert_eq!(kind, EmbassyWifiStartKind::Station);
                finish.signal(());
                "already-running"
            })
            .await;
            assert_eq!(exit.output(), &"returned-owner");
            assert!(!exit.stop_requested());
        };

        let (result, ()) = run(join(application, supervisor));
        match result {
            Err(crate::WifiRoleStartFailure::Rejected {
                wifi: _,
                request,
                error: EmbassyWifiSupervisorError::Service("already-running"),
            }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
            _ => panic!("active role must return the exact rejected request"),
        }
    }

    #[test]
    fn mailbox_storage_cannot_be_split_twice() {
        let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
        let _ = resources.split().unwrap();
        assert!(matches!(
            resources.split(),
            Err(EmbassyWifiSupervisorControlError::InUse)
        ));
    }
}
