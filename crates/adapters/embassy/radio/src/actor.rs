//! One local physical-owner actor, role epochs and fault retention.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_wifi_embassy::await_stack_boundary;

use oer_radio::wifi::{
    PhyRegistrationGeneration, RadioController, WifiRadioCalibrationPath, WifiRadioRestartReport,
    WifiRadioRetainedCycleReport, WifiScanFailure, WifiServicePlanningError, WifiServiceRequest,
    WifiStartFailure, WifiSupervisorConfiguration,
};

use super::dispatch::{EmbassyWifiStoppedDispatch, dispatch_embassy_wifi_stopped_command};
use super::message::{EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorResponse};
use super::transport::{
    EmbassyWifiSupervisorControlError, EmbassyWifiSupervisorControlResources,
    EmbassyWifiSupervisorEndpoint, EmbassyWifiSupervisorPort,
};

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
/// The returned future covers the complete active epoch, including any child
/// execution in the same local ownership domain. Implementations acknowledge
/// start, drive the role alongside `endpoint`, recover the exact terminal
/// owner and complete a pending stop before returning. A controlled child must
/// return its owner through a local rendezvous before quiescence; detached
/// tasks and synchronized channels must not acquire independent live owners.
pub trait EmbassyWifiRoleEpochRunner<M: RawMutex> {
    type Stopped;
    type Faulted;
    /// Fail-stop owner returned by a role-neutral whole-radio transaction.
    ///
    /// Implementations may use a compact handle to fault storage kept outside
    /// the supervisor frame. Active-role faults can retain a much larger owner
    /// graph through `Faulted` independently.
    type LifecycleFaulted;
    type Error;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error;

    /// Translate a retained hardware fault into the public control-plane
    /// error. This classifies the state; it does not perform recovery.
    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error;

    fn lifecycle_fault_error(&mut self, faulted: &Self::LifecycleFaulted) -> Self::Error;

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, M, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a;

    /// Close and cold-start the complete physical radio in the actor's stopped
    /// slot. Success must repopulate the slot; failure leaves it empty and
    /// retains the exact physical owner in `Faulted`.
    fn restart_radio<'a>(
        &'a mut self,
        stopped: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<WifiRadioCalibrationPath, Self::LifecycleFaulted>> + 'a;

    /// Close and restore RF without retiring the registered PHY epoch.
    fn cycle_retained_radio<'a>(
        &'a mut self,
        stopped: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<(), Self::LifecycleFaulted>> + 'a;
}

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
        await_stack_boundary!(run_embassy_wifi_supervisor_actor(
            self.endpoint,
            self.configuration,
            self.runner,
            self.stopped,
        ))
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

/// Controller and sole owner-holding actor prepared against the same resources.
pub type EmbassyWifiSupervisorPrepared<'resources, M, R> = (
    RadioController<
        EmbassyWifiSupervisorPort<'resources, M, <R as EmbassyWifiRoleEpochRunner<M>>::Error>,
    >,
    EmbassyWifiSupervisorTask<'resources, M, R>,
);

/// Prepare the controller/actor pair as one operation.
///
/// Failure cannot lose the role runner or stopped hardware frontier.
pub fn prepare_embassy_wifi_supervisor<'resources, M, R>(
    control: &'resources EmbassyWifiSupervisorControlResources<M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
) -> Result<
    EmbassyWifiSupervisorPrepared<'resources, M, R>,
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
    stopped: R::Stopped,
) -> !
where
    M: RawMutex,
    R: EmbassyWifiRoleEpochRunner<M>,
{
    let mut generation = oer_radio::wifi::RadioSubsystemGeneration::INITIAL;
    let mut phy_registration_generation = PhyRegistrationGeneration::INITIAL;
    let mut stopped = Some(stopped);
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
                EmbassyWifiStoppedDispatch::RestartRadio => {
                    let next_generation = generation.next();
                    let previous_phy_registration_generation = phy_registration_generation;
                    match await_stack_boundary!(runner.restart_radio(&mut stopped)) {
                        Ok(calibration_path) => {
                            generation = next_generation;
                            phy_registration_generation = phy_registration_generation.next();
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::RestartRadio(Ok(
                                    WifiRadioRestartReport::new(
                                        generation,
                                        previous_phy_registration_generation,
                                        phy_registration_generation,
                                        calibration_path,
                                    ),
                                )))
                                .await;
                        }
                        Err(faulted) => {
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::RestartRadio(Err(
                                    runner.lifecycle_fault_error(&faulted)
                                )))
                                .await;
                            run_embassy_wifi_lifecycle_faulted_actor(
                                &mut endpoint,
                                &mut runner,
                                faulted,
                            )
                            .await
                        }
                    }
                }
                EmbassyWifiStoppedDispatch::CycleRetainedRadio => {
                    let next_generation = generation.next();
                    let previous_phy_registration_generation = phy_registration_generation;
                    match await_stack_boundary!(runner.cycle_retained_radio(&mut stopped)) {
                        Ok(()) => {
                            generation = next_generation;
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::CycleRetainedRadio(Ok(
                                    WifiRadioRetainedCycleReport::new(
                                        generation,
                                        previous_phy_registration_generation,
                                        phy_registration_generation,
                                    ),
                                )))
                                .await;
                        }
                        Err(faulted) => {
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::CycleRetainedRadio(Err(
                                    runner.lifecycle_fault_error(&faulted),
                                )))
                                .await;
                            run_embassy_wifi_lifecycle_faulted_actor(
                                &mut endpoint,
                                &mut runner,
                                faulted,
                            )
                            .await
                        }
                    }
                }
                EmbassyWifiStoppedDispatch::Start(service) => break service,
            }
        };

        let next_generation = generation.next();
        match await_stack_boundary!(
            runner.run_epoch(
                &mut endpoint,
                stopped
                    .take()
                    .expect("stopped owner is present between supervisor epochs"),
                service,
                next_generation
            )
        ) {
            EmbassyWifiRoleEpochOutcome::NotStarted(returned) => stopped = Some(returned),
            EmbassyWifiRoleEpochOutcome::Stopped(returned) => {
                stopped = Some(returned);
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
            EmbassyWifiSupervisorCommand::RestartRadio => {
                EmbassyWifiSupervisorResponse::RestartRadio(Err(runner.fault_error(&faulted)))
            }
            EmbassyWifiSupervisorCommand::CycleRetainedRadio => {
                EmbassyWifiSupervisorResponse::CycleRetainedRadio(Err(runner.fault_error(&faulted)))
            }
        };
        endpoint.respond(response).await;
    }
}

async fn run_embassy_wifi_lifecycle_faulted_actor<M, R>(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, M, R::Error>,
    runner: &mut R,
    faulted: R::LifecycleFaulted,
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
                    error: runner.lifecycle_fault_error(&faulted),
                }))
            }
            EmbassyWifiSupervisorCommand::StartStation(request) => {
                EmbassyWifiSupervisorResponse::Station(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartAccessPoint(request) => {
                EmbassyWifiSupervisorResponse::AccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartStationAccessPoint(request) => {
                EmbassyWifiSupervisorResponse::StationAccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::StartMonitor(request) => {
                EmbassyWifiSupervisorResponse::Monitor(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            EmbassyWifiSupervisorCommand::Stop => {
                EmbassyWifiSupervisorResponse::Stop(Err(runner.lifecycle_fault_error(&faulted)))
            }
            EmbassyWifiSupervisorCommand::RestartRadio => {
                EmbassyWifiSupervisorResponse::RestartRadio(Err(
                    runner.lifecycle_fault_error(&faulted)
                ))
            }
            EmbassyWifiSupervisorCommand::CycleRetainedRadio => {
                EmbassyWifiSupervisorResponse::CycleRetainedRadio(Err(
                    runner.lifecycle_fault_error(&faulted)
                ))
            }
        };
        endpoint.respond(response).await;
    }
}
