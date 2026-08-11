//! ESP32-S31 local-role bindings for the physical radio supervisor.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_pac::MacInterruptSetup;
use open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiStopped;
use open_esp_radio_esp32s31_wifi_embassy::{
    monitor::{
        Esp32s31MonitorController, Esp32s31MonitorStopped, Esp32s31MonitorTask,
        Esp32s31MonitorTaskExit,
    },
    station::{
        Esp32s31StationAttemptRunner, Esp32s31StationController, Esp32s31StationExit,
        Esp32s31StationTask,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{irq::MacInterruptRoute, rx::RxPhyInfo};
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wifi_softmac::MonitorSink;

use crate::{
    RadioController, RadioSubsystemGeneration, StationRequest, WifiStartFailure, WifiStartReport,
    WifiSupervisorConfiguration,
    embassy_supervisor::{
        EmbassyWifiActiveRoleControl, EmbassyWifiActiveRoleExit, EmbassyWifiRoleEpochOutcome,
        EmbassyWifiRoleEpochRunner, EmbassyWifiRoleFrontier, EmbassyWifiStartKind,
        EmbassyWifiSupervisorControlResources, EmbassyWifiSupervisorEndpoint,
        EmbassyWifiSupervisorPort, EmbassyWifiSupervisorPrepareFailure,
        EmbassyWifiSupervisorResponse, EmbassyWifiSupervisorTask,
        drive_embassy_wifi_active_role_pinned, finish_embassy_wifi_active_role,
        prepare_embassy_wifi_supervisor,
    },
};

/// Application-facing ESP32-S31 radio actor. Internal supervisor endpoints
/// never escape this value.
pub type Esp32s31RadioSupervisorTask<'resources, M, R> =
    EmbassyWifiSupervisorTask<'resources, M, R>;

/// Failed ESP32-S31 actor preparation with its runner and stopped owner
/// retained for explicit board-level handling.
pub type Esp32s31RadioSupervisorPrepareFailure<R, S> = EmbassyWifiSupervisorPrepareFailure<R, S>;

/// Prepare the hardware-free controller and sole owner-holding ESP32-S31
/// radio actor together.
pub fn prepare_esp32s31_radio_supervisor<'resources, M, R>(
    control: &'resources EmbassyWifiSupervisorControlResources<M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
) -> Result<
    (
        RadioController<EmbassyWifiSupervisorPort<'resources, M, R::Error>>,
        Esp32s31RadioSupervisorTask<'resources, M, R>,
    ),
    Esp32s31RadioSupervisorPrepareFailure<R, R::Stopped>,
>
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    prepare_embassy_wifi_supervisor(control, configuration, runner, stopped)
}

/// Role-neutral stopped Wi-Fi plus independently reusable role resources.
///
/// The physical supervisor stores this aggregate only between active role
/// epochs. Starting a role consumes the corresponding resource graph together
/// with `wifi`; a clean exit must return both before this value can exist
/// again.
pub struct Esp32s31WifiSupervisorStopped<P, S, A, M> {
    pub wifi: Esp32s31WifiStopped<P>,
    pub station: S,
    pub access_point: A,
    pub monitor: M,
}

impl<P, S, A, M> Esp32s31WifiSupervisorStopped<P, S, A, M> {
    pub const fn new(
        wifi: Esp32s31WifiStopped<P>,
        station: S,
        access_point: A,
        monitor: M,
    ) -> Self {
        Self {
            wifi,
            station,
            access_point,
            monitor,
        }
    }

    pub fn into_parts(self) -> (Esp32s31WifiStopped<P>, S, A, M) {
        (self.wifi, self.station, self.access_point, self.monitor)
    }
}

impl<M: RawMutex> EmbassyWifiActiveRoleControl for Esp32s31StationController<'_, M> {
    fn request_stop(&mut self) {
        Esp32s31StationController::request_stop(self);
    }
}

impl<M: RawMutex> EmbassyWifiActiveRoleControl for Esp32s31MonitorController<'_, M> {
    fn request_stop(&mut self) {
        Esp32s31MonitorController::request_stop(self);
    }
}

/// Drive one complete ESP32-S31 station owner locally inside the physical
/// supervisor actor.
///
/// `task` owns the live station graph, while `controller` remains beside its
/// future solely to publish cooperative commands. The returned exit carries
/// the exact hardware frontier and must be classified before a pending stop is
/// acknowledged.
pub async fn drive_esp32s31_station_role<'control, M, R, E, Reject>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    controller: &mut Esp32s31StationController<'control, M>,
    task: Esp32s31StationTask<'control, M, R>,
    reject_while_active: Reject,
) -> EmbassyWifiActiveRoleExit<Esp32s31StationExit<R::Owner, R, R::Error, R::Fault>>
where
    M: RawMutex + 'control,
    R: Esp32s31StationAttemptRunner<M>,
    Reject: FnMut(EmbassyWifiStartKind) -> E,
{
    let mut task = task;
    let role = task.run();
    let mut role = core::pin::pin!(role);
    await_stack_boundary!(drive_embassy_wifi_active_role_pinned(
        endpoint,
        controller,
        role.as_mut(),
        reject_while_active,
    ))
}

/// Owned input for one station role epoch.
pub struct Esp32s31StationSupervisorEpoch<S> {
    stopped: S,
    request: StationRequest,
    generation: RadioSubsystemGeneration,
}

impl<S> Esp32s31StationSupervisorEpoch<S> {
    pub const fn new(
        stopped: S,
        request: StationRequest,
        generation: RadioSubsystemGeneration,
    ) -> Self {
        Self {
            stopped,
            request,
            generation,
        }
    }
}

/// Board-specific classification and public-error hooks around the common
/// owner-holding station supervisor protocol.
pub struct Esp32s31StationSupervisorHooks<C, R, F> {
    classify: C,
    reject_while_active: R,
    fault_error: F,
}

impl<C, R, F> Esp32s31StationSupervisorHooks<C, R, F> {
    pub const fn new(classify: C, reject_while_active: R, fault_error: F) -> Self {
        Self {
            classify,
            reject_while_active,
            fault_error,
        }
    }
}

/// Run one complete ESP32-S31 station epoch inside the physical supervisor.
///
/// The board binding prepares the concrete station owner and classifies its
/// terminal frontier. This function acknowledges start only after task
/// preparation, keeps the controller beside the owner future, and
/// acknowledges stop only after the returned owner is classified reusable.
pub async fn run_esp32s31_station_supervisor_epoch<
    'control,
    M,
    R,
    S,
    F,
    E,
    Prepare,
    Classify,
    RejectActive,
    FaultError,
>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    epoch: Esp32s31StationSupervisorEpoch<S>,
    prepare: Prepare,
    hooks: Esp32s31StationSupervisorHooks<Classify, RejectActive, FaultError>,
) -> EmbassyWifiRoleEpochOutcome<S, F>
where
    M: RawMutex + 'control,
    R: Esp32s31StationAttemptRunner<M> + 'control,
    Prepare: FnOnce(
        S,
        StationRequest,
    ) -> Result<
        (
            Esp32s31StationController<'control, M>,
            Esp32s31StationTask<'control, M, R>,
        ),
        F,
    >,
    Classify: FnOnce(
        Esp32s31StationExit<R::Owner, R, R::Error, R::Fault>,
    ) -> EmbassyWifiRoleFrontier<S, F>,
    RejectActive: FnMut(EmbassyWifiStartKind) -> E,
    FaultError: FnMut(&F) -> E,
{
    let Esp32s31StationSupervisorEpoch {
        stopped,
        request,
        generation,
    } = epoch;
    let Esp32s31StationSupervisorHooks {
        classify,
        reject_while_active,
        mut fault_error,
    } = hooks;

    let (mut controller, task) = match prepare(stopped, request) {
        Ok(prepared) => prepared,
        Err(faulted) => {
            let error = fault_error(&faulted);
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Station(Err(
                    WifiStartFailure::faulted(error),
                )))
                .await;
            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
        }
    };
    endpoint
        .respond(EmbassyWifiSupervisorResponse::Station(Ok(
            WifiStartReport::new(generation),
        )))
        .await;

    let exit = await_stack_boundary!(drive_esp32s31_station_role(
        endpoint,
        &mut controller,
        task,
        reject_while_active,
    ));
    match await_stack_boundary!(finish_embassy_wifi_active_role(
        endpoint,
        generation,
        exit,
        classify,
        fault_error,
    )) {
        EmbassyWifiRoleFrontier::Stopped(stopped) => EmbassyWifiRoleEpochOutcome::Stopped(stopped),
        EmbassyWifiRoleFrontier::Faulted(faulted) => EmbassyWifiRoleEpochOutcome::Faulted(faulted),
    }
}

/// Drive one complete standalone-monitor owner locally inside the physical
/// supervisor actor.
///
/// The returned exit preserves the distinction between a reusable stopped
/// owner and a task which still retains a quarantined live frontier. A pending
/// application stop may be acknowledged only for the former.
#[allow(clippy::type_complexity)]
pub async fn drive_esp32s31_monitor_role<
    'runtime,
    P,
    R,
    M,
    S,
    E,
    Reject,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    controller: &mut Esp32s31MonitorController<'runtime, M>,
    task: Esp32s31MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    reject_while_active: Reject,
) -> EmbassyWifiActiveRoleExit<
    Esp32s31MonitorTaskExit<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Esp32s31MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        R::Error,
    >,
>
where
    P: Sized,
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    M: RawMutex,
    S: MonitorSink<RxPhyInfo>,
    Reject: FnMut(EmbassyWifiStartKind) -> E,
{
    let role = task.run_to_exit();
    let mut role = core::pin::pin!(role);
    await_stack_boundary!(drive_embassy_wifi_active_role_pinned(
        endpoint,
        controller,
        role.as_mut(),
        reject_while_active,
    ))
}
