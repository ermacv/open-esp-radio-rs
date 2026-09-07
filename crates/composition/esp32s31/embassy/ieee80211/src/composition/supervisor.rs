#![expect(
    clippy::type_complexity,
    reason = "the application root returns the statically typed controller and sole owner task together"
)]

//! ESP32-S31 local-role bindings for the physical radio supervisor.

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_radio::{
    runtime::embassy::{
        EmbassyWifiActiveRoleControl, EmbassyWifiActiveRoleExit, EmbassyWifiRoleEpochOutcome,
        EmbassyWifiRoleEpochRunner, EmbassyWifiRoleFrontier, EmbassyWifiStartKind,
        EmbassyWifiSupervisorControlResources, EmbassyWifiSupervisorEndpoint,
        EmbassyWifiSupervisorPort, EmbassyWifiSupervisorPrepareFailure,
        EmbassyWifiSupervisorResponse, EmbassyWifiSupervisorTask,
        drive_embassy_wifi_active_role_pinned, finish_embassy_wifi_active_role,
        prepare_embassy_wifi_supervisor,
    },
    wifi::{
        RadioController, RadioSubsystemGeneration, StationRequest, WifiStartFailure,
        WifiStartReport, WifiSupervisorConfiguration,
    },
};

use oer_esp32s31_hal::owner::MacInterruptSetup;

use oer_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver};

use oer_esp32s31_wifi_embassy::roles::{
    monitor::{MonitorController, MonitorStopped, MonitorTask, MonitorTaskExit},
    station::{StationAttemptRunner, StationController, StationExit, StationTask},
};

use oer_esp32s31_wifi_mac::{irq::MacInterruptRoute, rx::RxPhyInfo};

use oer_wifi_embassy::await_stack_boundary;

use oer_wifi_softmac::{MonitorChannelPolicy, MonitorSink};

/// Application-facing ESP32-S31 radio actor. Internal supervisor endpoints
/// never escape this value.
pub type RadioSupervisorTask<'resources, M, R> = EmbassyWifiSupervisorTask<'resources, M, R>;

/// Failed ESP32-S31 actor preparation with its runner and stopped owner
/// retained for explicit board-level handling.
pub type RadioSupervisorPrepareFailure<R, S> = EmbassyWifiSupervisorPrepareFailure<R, S>;

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
        RadioSupervisorTask<'resources, M, R>,
    ),
    RadioSupervisorPrepareFailure<R, R::Stopped>,
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
pub struct WifiSupervisorStopped<W, H, S, A, M> {
    pub wifi: W,
    pub physical: H,
    pub station: S,
    pub access_point: A,
    pub monitor: M,
}

impl<W, H, S, A, M> WifiSupervisorStopped<W, H, S, A, M> {
    pub const fn new(wifi: W, physical: H, station: S, access_point: A, monitor: M) -> Self {
        Self {
            wifi,
            physical,
            station,
            access_point,
            monitor,
        }
    }

    pub fn into_parts(self) -> (W, H, S, A, M) {
        (
            self.wifi,
            self.physical,
            self.station,
            self.access_point,
            self.monitor,
        )
    }
}

struct StationActiveRoleControl<'borrow, 'control, M: RawMutex> {
    inner: &'borrow mut StationController<'control, M>,
}

impl<M: RawMutex> EmbassyWifiActiveRoleControl for StationActiveRoleControl<'_, '_, M> {
    fn request_stop(&mut self) {
        self.inner.request_stop();
    }
}

struct MonitorActiveRoleControl<'borrow, 'control, M: RawMutex> {
    inner: &'borrow mut MonitorController<'control, M>,
}

impl<M: RawMutex> EmbassyWifiActiveRoleControl for MonitorActiveRoleControl<'_, '_, M> {
    fn request_stop(&mut self) {
        self.inner.request_stop();
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
    controller: &mut StationController<'control, M>,
    task: StationTask<'control, M, R>,
    reject_while_active: Reject,
) -> EmbassyWifiActiveRoleExit<StationExit<R::Owner, R, R::Error, R::Fault>>
where
    M: RawMutex + 'control,
    R: StationAttemptRunner<M>,
    Reject: FnMut(EmbassyWifiStartKind) -> E,
{
    let mut task = task;
    let role = task.run();
    let mut role = core::pin::pin!(role);
    let mut control = StationActiveRoleControl { inner: controller };
    await_stack_boundary!(drive_embassy_wifi_active_role_pinned(
        endpoint,
        &mut control,
        role.as_mut(),
        reject_while_active,
    ))
}

/// Owned input for one station role epoch.
pub struct StationSupervisorEpoch<S> {
    stopped: S,
    request: StationRequest,
    generation: RadioSubsystemGeneration,
}

impl<S> StationSupervisorEpoch<S> {
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
pub struct StationSupervisorHooks<C, R, F> {
    classify: C,
    reject_while_active: R,
    fault_error: F,
}

impl<C, R, F> StationSupervisorHooks<C, R, F> {
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
    epoch: StationSupervisorEpoch<S>,
    prepare: Prepare,
    hooks: StationSupervisorHooks<Classify, RejectActive, FaultError>,
) -> EmbassyWifiRoleEpochOutcome<S, F>
where
    M: RawMutex + 'control,
    R: StationAttemptRunner<M> + 'control,
    Prepare: FnOnce(
        S,
        StationRequest,
    )
        -> Result<(StationController<'control, M>, StationTask<'control, M, R>), F>,
    Classify: FnOnce(StationExit<R::Owner, R, R::Error, R::Fault>) -> EmbassyWifiRoleFrontier<S, F>,
    RejectActive: FnMut(EmbassyWifiStartKind) -> E,
    FaultError: FnMut(&F) -> E,
{
    let StationSupervisorEpoch {
        stopped,
        request,
        generation,
    } = epoch;
    let StationSupervisorHooks {
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
    D,
    O,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, E>,
    controller: &mut MonitorController<'runtime, M>,
    task: MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    channel_policy: MonitorChannelPolicy,
    _delay: D,
    observer: &mut O,
    reject_while_active: Reject,
) -> EmbassyWifiActiveRoleExit<
    MonitorTaskExit<
        MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        R::Error,
    >,
>
where
    P: Sized,
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    M: RawMutex,
    S: MonitorSink<RxPhyInfo>,
    Reject: FnMut(EmbassyWifiStartKind) -> E,
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let role = task.run_channel_policy_to_exit::<D, O>(channel_policy, observer);
    let mut role = core::pin::pin!(role);
    let mut control = MonitorActiveRoleControl { inner: controller };
    await_stack_boundary!(drive_embassy_wifi_active_role_pinned(
        endpoint,
        &mut control,
        role.as_mut(),
        reject_while_active,
    ))
}
