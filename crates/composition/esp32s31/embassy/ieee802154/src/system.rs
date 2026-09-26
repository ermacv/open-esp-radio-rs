//! Bring-up and teardown of the IEEE 802.15.4 client on the chip.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Instant, Timer};
use esp_hal::interrupt::{InterruptHandler, Priority};
use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154Clocked, Ieee802154Cold, Ieee802154FoundationCheckpoint, Ieee802154Powered,
        Ieee802154ReadbackError, Ieee802154ResetCheckpoint, Ieee802154TxPowerLevels,
        ll::Ieee802154MacOwners,
    },
    root::Ieee802154RadioPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadio,
        SharedRadioLease,
    },
};
use oer_esp32s31_ieee802154::{
    engine::{Ieee802154Engine, Ieee802154EngineBuffers},
    pib::Ieee802154PibDefaults,
};
use oer_esp32s31_ieee802154_esp_hal::{
    BoundEspHalIeee802154InterruptRoute, EspHalIeee802154InterruptRouteError, bind, now_micros,
};
use oer_esp32s31_ieee802154_runtime::{
    Ieee802154Platform, Ieee802154Runtime, Ieee802154RuntimeParts,
};
use oer_esp32s31_phy::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyTrackingError, ConcurrentRfError,
    ConcurrentTrackingTick, NoopPhyTargetObserver, PhyRegisterConfig, close_concurrent_rf,
    concurrent::{
        ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, MaintenancePolicy,
        evaluate_periodic_tracking,
    },
    ieee802154_client::{
        Ieee802154PhyClientError, Ieee802154PhyMembership, RegisteredIeee802154Operational,
        RegisteredIeee802154OperationalRoute, join_ieee802154, leave_ieee802154,
    },
    maintain_concurrent_phy, register_concurrent_phy,
    state::client::PhyModemClient,
    track_concurrent_phy, wake_concurrent_rf,
};
use oer_esp32s31_phy_runtime::EmbassyPhyTime;

use crate::maintenance::{
    Ieee802154PhyMaintenance, MAINTENANCE_PERIOD_MICROS, next_attempt_micros,
};

/// Events the runtime queues before the consumer takes them.
pub const IEEE802154_EVENT_CAPACITY: usize = 16;

/// The process-wide IEEE 802.15.4 runtime.
pub type Ieee802154SystemRuntime = Ieee802154Runtime<
    'static,
    CriticalSectionRawMutex,
    Ieee802154MacOwners,
    IEEE802154_EVENT_CAPACITY,
>;

static RUNTIME: Ieee802154SystemRuntime = Ieee802154Runtime::new();

/// Window a clocked client grants shared PHY tracking at bring-up. The
/// clocked owner starts no MAC operation while the proof lives, so the bound
/// only limits how long tracking may hold the shared PHY.
const TRACKING_WINDOW_MICROS: u64 = 1_000_000;

/// Retry period while another client holds the arbiter lease.
const LEASE_RETRY_MICROS: u64 = 100;

extern "C" fn ieee802154_interrupt() {
    RUNTIME.on_interrupt();
}

/// The IEEE 802.15.4 partition and MAC engine while the client is stopped.
pub struct Ieee802154Parked {
    /// The partition of the concurrent split.
    pub partition: Ieee802154RadioPartition,
    /// The MAC engine and its receive storage.
    pub engine: Ieee802154Engine<'static>,
}

impl Ieee802154Parked {
    /// Park a partition with a fresh engine over `buffers`, using the
    /// ESP32-S31 BTBB transmit-power levels.
    pub fn new(
        partition: Ieee802154RadioPartition,
        buffers: &'static mut Ieee802154EngineBuffers,
        defaults: Ieee802154PibDefaults,
    ) -> Self {
        Self {
            partition,
            engine: Ieee802154Engine::new(buffers, Ieee802154TxPowerLevels::ESP32S31, defaults),
        }
    }
}

/// Why bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154StartError {
    /// Common radio power could not be entered.
    Power(CommonRadioPowerError),
    /// The module clocks could not be enabled.
    Clocks(ModemClockError),
    /// The shared PHY domain could not be registered.
    PhyRegistration(ConcurrentPhyError),
    /// Registering the shared PHY failed after it started.
    PhyRegistrationFailed,
    /// Closed shared RF could not be woken.
    PhyWake(ConcurrentRfError),
    /// IEEE 802.15.4 could not join the shared PHY domain or BTBB.
    PhyClient(Ieee802154PhyClientError),
    /// Tracking due at the join was rejected before it started.
    Tracking(ConcurrentPhyError),
    /// Tracking due at the join failed after it started.
    TrackingFailed,
    /// A MAC reset line did not read back released.
    Reset(Ieee802154ReadbackError<Ieee802154ResetCheckpoint>),
    /// A MAC foundation field did not read back.
    Foundation(Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>),
    /// A runtime is already installed.
    AlreadyInstalled,
    /// The CPU route of modem source 132 could not be bound.
    Route(EspHalIeee802154InterruptRouteError),
}

/// Owners retained after a failure that may have left hardware in an
/// unknown state. They are never returned; the chip must be reset.
#[must_use = "a fail-stop owner keeps the radio until the chip resets"]
pub struct Ieee802154FailStop {
    _owner: FailStopOwner,
}

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the retained owners are never read or moved again"
)]
enum FailStopOwner {
    Powered(Ieee802154Powered),
    Clocked(Ieee802154Clocked),
    Joined(Ieee802154Clocked, Ieee802154PhyMembership),
    Installed(RegisteredIeee802154Operational, Ieee802154Engine<'static>),
}

/// Failed bring-up.
#[must_use = "a failed start still owns the partition or keeps it fail-stop"]
pub struct Ieee802154StartFailure {
    /// The first error.
    pub error: Ieee802154StartError,
    /// The stopped client when every earlier step was rolled back, or the
    /// fail-stop owner otherwise.
    pub owner: Result<Ieee802154Parked, Ieee802154FailStop>,
}

/// Why teardown stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154StopError {
    /// The route of modem source 132 must be quiesced on its binding core.
    Route(EspHalIeee802154InterruptRouteError),
    /// The MAC foundation did not read back after the operational epoch.
    Foundation(Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>),
    /// IEEE 802.15.4 could not leave the shared PHY domain or BTBB.
    PhyClient(Ieee802154PhyClientError),
    /// Shared RF could not be closed after the last PHY client left.
    PhyClose(ConcurrentRfError),
    /// The module clocks could not be released.
    Clocks(ModemClockError),
    /// Common radio power could not be left.
    Power(CommonRadioPowerError),
}

/// Failed teardown.
#[must_use = "a failed stop still owns the system or keeps it fail-stop"]
pub struct Ieee802154StopFailure {
    /// The first error.
    pub error: Ieee802154StopError,
    /// The unchanged running system when nothing was torn down yet, or the
    /// fail-stop owner otherwise.
    pub owner: Result<Ieee802154System, Ieee802154FailStop>,
}

/// The running IEEE 802.15.4 client.
///
/// The MAC owners and the engine live in the runtime; this value keeps the
/// PHY membership and the bound CPU route until [`Self::stop`].
#[must_use = "the running IEEE 802.15.4 client must be stopped"]
pub struct Ieee802154System {
    route: RegisteredIeee802154OperationalRoute,
    /// Bound except while PHY maintenance holds the MAC paused, or after a
    /// failed rebind.
    bound: Option<BoundEspHalIeee802154InterruptRoute>,
}

/// Why PHY maintenance failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154MaintenanceError {
    /// The PHY domain rejected the evaluation or the maintenance before it
    /// started.
    Phy(ConcurrentPhyError),
    /// Tracking failed after it started; the domain is poisoned.
    TrackingFailed,
    /// The CPU route could not be bound again; the MAC receives no interrupt.
    Route(EspHalIeee802154InterruptRouteError),
}

async fn lease(radio: &SharedRadio<ConcurrentPhy>) -> SharedRadioLease<'_, ConcurrentPhy> {
    loop {
        if let Ok(lease) = radio.try_acquire() {
            return lease;
        }
        Timer::after_micros(LEASE_RETRY_MICROS).await;
    }
}

fn fail_stop(error: Ieee802154StartError, owner: FailStopOwner) -> Ieee802154StartFailure {
    Ieee802154StartFailure {
        error,
        owner: Err(Ieee802154FailStop { _owner: owner }),
    }
}

/// Roll a clocked client back to its partition.
fn unwind_clocked(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
    clocked: Ieee802154Clocked,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
) -> Ieee802154StartFailure {
    let powered = match clocked.disable_clocks(lease, clocks) {
        Ok(powered) => powered,
        Err(failure) => return fail_stop(error, FailStopOwner::Clocked(failure.into_owner())),
    };
    unwind_powered(lease, powered, engine, error)
}

fn unwind_powered(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    powered: Ieee802154Powered,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
) -> Ieee802154StartFailure {
    match powered.power_down(lease) {
        Ok(cold) => Ieee802154StartFailure {
            error,
            owner: Ok(Ieee802154Parked {
                partition: cold.into_partition(),
                engine,
            }),
        },
        Err(failure) => fail_stop(error, FailStopOwner::Powered(failure.into_owner())),
    }
}

/// Make the shared PHY domain ready for a new client: register it once,
/// or wake its closed RF.
async fn prepare_phy<P>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &mut impl PlatformClockProvider,
    config: PhyRegisterConfig,
) -> Result<(), (Ieee802154StartError, bool)> {
    if lease.attachment().rf_closed() {
        return match wake_concurrent_rf::<EmbassyPhyTime>(lease, clocks).await {
            Ok(()) => Ok(()),
            Err(ConcurrentRfError::Rejected(error)) => Err((
                Ieee802154StartError::PhyWake(ConcurrentRfError::Rejected(error)),
                false,
            )),
            Err(error) => Err((Ieee802154StartError::PhyWake(error), true)),
        };
    }
    if lease.attachment().client_snapshot().is_some() {
        return Ok(());
    }
    match register_concurrent_phy::<P, EmbassyPhyTime, _>(
        lease,
        platform,
        clocks,
        config,
        NoopPhyTargetObserver,
    )
    .await
    {
        Ok(_registration) => Ok(()),
        Err(ConcurrentPhyRegisterFailure::Rejected(ConcurrentPhyError::AlreadyRegistered)) => {
            Ok(())
        }
        Err(ConcurrentPhyRegisterFailure::Rejected(error)) => {
            Err((Ieee802154StartError::PhyRegistration(error), false))
        }
        Err(_) => Err((Ieee802154StartError::PhyRegistrationFailed, true)),
    }
}

/// `esp_ieee802154_enable` on the shared radio.
///
/// `platform` is the integration token the PHY target port borrows;
/// `clocks` supplies the platform sources of the modem clocks; `config`
/// registers the shared PHY domain when no client registered it yet.
///
/// # Errors
///
/// A step failed. The failure returns the parked client when every earlier
/// step was rolled back, or a fail-stop owner when a started PHY, clock or
/// power transaction failed.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure returns the parked partition and engine"
)]
pub async fn start<P>(
    radio: &SharedRadio<ConcurrentPhy>,
    parked: Ieee802154Parked,
    platform: &mut P,
    clocks: &mut impl PlatformClockProvider,
    config: PhyRegisterConfig,
    defaults: Ieee802154PibDefaults,
) -> Result<Ieee802154System, Ieee802154StartFailure> {
    let Ieee802154Parked { partition, engine } = parked;
    let mut lease = lease(radio).await;

    let powered = match Ieee802154Cold::from_partition(partition).power_up(&mut lease) {
        Ok(powered) => powered,
        Err(failure) => {
            return Err(Ieee802154StartFailure {
                error: Ieee802154StartError::Power(failure.error()),
                owner: Ok(Ieee802154Parked {
                    partition: failure.into_owner().into_partition(),
                    engine,
                }),
            });
        }
    };
    let mut clocked = match powered.enable_clocks(&mut lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            let error = Ieee802154StartError::Clocks(failure.error());
            if failure.error() == ModemClockError::Poisoned {
                return Err(fail_stop(
                    error,
                    FailStopOwner::Powered(failure.into_owner()),
                ));
            }
            return Err(unwind_powered(
                &mut lease,
                failure.into_owner(),
                engine,
                error,
            ));
        }
    };

    if let Err((error, started)) = prepare_phy(&mut lease, platform, clocks, config).await {
        if started {
            return Err(fail_stop(error, FailStopOwner::Clocked(clocked)));
        }
        return Err(unwind_clocked(&mut lease, clocks, clocked, engine, error));
    }

    let (membership, acquired) = match join_ieee802154(&mut lease, &clocked, &mut EmbassyPhyTime) {
        Ok(joined) => joined,
        Err(error) => {
            return Err(unwind_clocked(
                &mut lease,
                clocks,
                clocked,
                engine,
                Ieee802154StartError::PhyClient(error),
            ));
        }
    };
    if acquired == ConcurrentAcquire::TrackingDue {
        let issued_at = Instant::now().as_micros();
        let tracked = match clocked.quiescence(issued_at, issued_at + TRACKING_WINDOW_MICROS) {
            Ok(proof) => {
                maintain_concurrent_phy::<P, EmbassyPhyTime, _>(
                    &mut lease,
                    platform,
                    &[proof],
                    NoopPhyTargetObserver,
                )
                .await
            }
            Err(_) => Err(ConcurrentPhyTrackingError::Rejected(
                ConcurrentPhyError::WindowClosed,
            )),
        };
        match tracked {
            Ok(_outcome) => {}
            Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                // Nothing started, but the domain still waits for tracking,
                // so the client cannot leave it.
                return Err(fail_stop(
                    Ieee802154StartError::Tracking(error),
                    FailStopOwner::Joined(clocked, membership),
                ));
            }
            Err(ConcurrentPhyTrackingError::Failed(_)) => {
                return Err(fail_stop(
                    Ieee802154StartError::TrackingFailed,
                    FailStopOwner::Joined(clocked, membership),
                ));
            }
        }
    }

    let reset = match clocked.reset_mac(&mut lease) {
        Ok(reset) => reset,
        Err(failure) => {
            let error = Ieee802154StartError::Reset(failure.error());
            return Err(leave_and_unwind(
                &mut lease,
                clocks,
                failure.into_clocked(),
                membership,
                engine,
                error,
            ));
        }
    };
    let foundation = match reset.configure_foundation() {
        Ok(foundation) => foundation,
        Err(failure) => {
            let error = Ieee802154StartError::Foundation(failure.error());
            return Err(leave_and_unwind(
                &mut lease,
                clocks,
                failure.into_reset().into_clocked(),
                membership,
                engine,
                error,
            ));
        }
    };
    drop(lease);

    let RegisteredIeee802154Operational {
        mut task,
        interrupts,
        route,
    } = RegisteredIeee802154Operational::new(foundation, membership);
    let interrupts = interrupts.activate(&mut task);
    let parts = Ieee802154RuntimeParts {
        engine,
        hardware: Ieee802154MacOwners::new(task, interrupts),
    };
    let platform_services = Ieee802154Platform {
        now_micros,
        enhanced_ack: None,
    };
    if let Err(parts) = RUNTIME.install(parts, platform_services, defaults) {
        let (mut task, interrupts) = parts.hardware.into_parts();
        let interrupts = interrupts.deactivate(&mut task);
        return Err(fail_stop(
            Ieee802154StartError::AlreadyInstalled,
            FailStopOwner::Installed(
                RegisteredIeee802154Operational {
                    task,
                    interrupts,
                    route,
                },
                parts.engine,
            ),
        ));
    }
    match bind(InterruptHandler::new(
        ieee802154_interrupt,
        Priority::Priority1,
    )) {
        Ok(bound) => Ok(Ieee802154System {
            route,
            bound: Some(bound),
        }),
        Err(error) => {
            let parts = RUNTIME
                .uninstall()
                .expect("the runtime installed above is still installed");
            let (mut task, interrupts) = parts.hardware.into_parts();
            let interrupts = interrupts.deactivate(&mut task);
            Err(fail_stop(
                Ieee802154StartError::Route(error),
                FailStopOwner::Installed(
                    RegisteredIeee802154Operational {
                        task,
                        interrupts,
                        route,
                    },
                    parts.engine,
                ),
            ))
        }
    }
}

fn leave_and_unwind(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
    clocked: Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
) -> Ieee802154StartFailure {
    if leave_ieee802154(lease, &clocked, membership).is_err() {
        return fail_stop(error, FailStopOwner::Clocked(clocked));
    }
    unwind_clocked(lease, clocks, clocked, engine, error)
}

impl Ieee802154System {
    /// The runtime that accepts commands and yields events.
    pub fn runtime(&self) -> &'static Ieee802154SystemRuntime {
        &RUNTIME
    }

    /// Run one shared PHY tracking tick under the domain's maintenance
    /// policy, as one tick of ESP-IDF's periodic `phy_track_pll`.
    ///
    /// Under [`MaintenancePolicy::Vendor`] the tick runs with IEEE 802.15.4
    /// running, as the vendor does under its PHY lock; the arbiter owner's
    /// [`run_concurrent_phy_tracking`](oer_esp32s31_phy::run_concurrent_phy_tracking)
    /// is the periodic loop of this policy. Under
    /// [`MaintenancePolicy::Quiesced`] every active client must prove
    /// quiescence first: when IEEE 802.15.4 is the only active client this
    /// pauses the runtime (leaving receive mode), closes the CPU route,
    /// issues the proof, runs the tracking transaction within it, and then
    /// resumes the runtime and binds the route again; with another client
    /// active it reports `AwaitingOtherClients`. Run it on the core that
    /// started the client.
    ///
    /// # Errors
    ///
    /// The domain rejected the evaluation or the maintenance, tracking failed
    /// after it started, or the route could not be bound again.
    pub async fn maintain_phy<P>(
        &mut self,
        radio: &SharedRadio<ConcurrentPhy>,
        platform: &mut P,
    ) -> Result<Ieee802154PhyMaintenance, Ieee802154MaintenanceError> {
        let mut lease = lease(radio).await;
        if lease.attachment().maintenance_policy() == MaintenancePolicy::Vendor {
            let mut clock = EmbassyPhyTime;
            let tick = core::pin::pin!(track_concurrent_phy::<P, EmbassyPhyTime, _>(
                &mut lease,
                platform,
                &mut clock,
                NoopPhyTargetObserver,
            ));
            return match tick.await {
                Ok(ConcurrentTrackingTick::NotDue) => Ok(Ieee802154PhyMaintenance::NotDue),
                Ok(ConcurrentTrackingTick::Tracked(_)) => Ok(Ieee802154PhyMaintenance::Tracked),
                Ok(ConcurrentTrackingTick::Unavailable(error)) => {
                    Err(Ieee802154MaintenanceError::Phy(error))
                }
                // The policy was read under this lease.
                Ok(ConcurrentTrackingTick::AwaitingQuiescence) => {
                    Ok(Ieee802154PhyMaintenance::AwaitingOtherClients)
                }
                Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                    Err(Ieee802154MaintenanceError::Phy(error))
                }
                Err(ConcurrentPhyTrackingError::Failed(_)) => {
                    Err(Ieee802154MaintenanceError::TrackingFailed)
                }
            };
        }
        let due = lease.attachment().tracking_pending()
            || evaluate_periodic_tracking(&mut lease, &mut EmbassyPhyTime)
                .map_err(Ieee802154MaintenanceError::Phy)?;
        if !due {
            return Ok(Ieee802154PhyMaintenance::NotDue);
        }
        let others = lease.attachment().client_snapshot().is_some_and(|clients| {
            clients.contains(PhyModemClient::Wifi) || clients.contains(PhyModemClient::Bluetooth)
        });
        if others {
            return Ok(Ieee802154PhyMaintenance::AwaitingOtherClients);
        }
        let mut paused = match RUNTIME.pause() {
            Ok(paused) => paused,
            Err(_) => return Ok(Ieee802154PhyMaintenance::Busy),
        };
        if let Some(bound) = self.bound.take()
            && let Err((error, bound)) = bound.quiesce()
        {
            self.bound = Some(bound);
            let _ = RUNTIME.resume(paused);
            return Err(Ieee802154MaintenanceError::Route(error));
        }

        let issued_at = Instant::now().as_micros();
        let tracked = match paused
            .hardware_mut()
            .task_mut()
            .quiescence(issued_at, issued_at + TRACKING_WINDOW_MICROS)
        {
            Ok(proof) => {
                maintain_concurrent_phy::<P, EmbassyPhyTime, _>(
                    &mut lease,
                    platform,
                    &[proof],
                    NoopPhyTargetObserver,
                )
                .await
            }
            Err(_) => Err(ConcurrentPhyTrackingError::Rejected(
                ConcurrentPhyError::WindowClosed,
            )),
        };
        drop(lease);

        if RUNTIME.resume(paused).is_err() {
            unreachable!("the paused runtime has no other radio");
        }
        match bind(InterruptHandler::new(
            ieee802154_interrupt,
            Priority::Priority1,
        )) {
            Ok(bound) => self.bound = Some(bound),
            Err(error) => return Err(Ieee802154MaintenanceError::Route(error)),
        }
        match tracked {
            Ok(_outcome) => Ok(Ieee802154PhyMaintenance::Tracked),
            Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                Err(Ieee802154MaintenanceError::Phy(error))
            }
            Err(ConcurrentPhyTrackingError::Failed(_)) => {
                Err(Ieee802154MaintenanceError::TrackingFailed)
            }
        }
    }

    /// Keep the shared PHY tracked until `stop` completes: one
    /// [`Self::maintain_phy`] tick per tracking period, retried promptly
    /// while the MAC is busy. This is the periodic loop of the
    /// [`MaintenancePolicy::Quiesced`] policy while IEEE 802.15.4 is the only
    /// client; under the default vendor policy the arbiter owner runs
    /// `run_concurrent_phy_tracking` instead. `observe` receives each tick's
    /// outcome. Run it on the core that started the client and call
    /// [`Self::stop`] after it returns.
    ///
    /// # Errors
    ///
    /// A maintenance attempt failed; tracking stops being attempted.
    pub async fn maintain_phy_until<P>(
        &mut self,
        radio: &SharedRadio<ConcurrentPhy>,
        platform: &mut P,
        stop: impl Future<Output = ()>,
        mut observe: impl FnMut(Ieee802154PhyMaintenance),
    ) -> Result<(), Ieee802154MaintenanceError> {
        let mut stop = core::pin::pin!(stop);
        let mut delay = MAINTENANCE_PERIOD_MICROS;
        loop {
            match select(Timer::after_micros(delay), stop.as_mut()).await {
                Either::First(()) => {}
                Either::Second(()) => return Ok(()),
            }
            let outcome = {
                let maintained = core::pin::pin!(self.maintain_phy(radio, platform));
                maintained.await?
            };
            observe(outcome);
            delay = next_attempt_micros(outcome);
        }
    }

    /// `esp_ieee802154_disable`: quiesce the CPU route, take the MAC owners
    /// out of the runtime, prove the foundation again, leave the shared PHY
    /// and BTBB (closing RF after the last PHY client), release the module
    /// clocks and leave common power. Must run on the core that started the
    /// client.
    ///
    /// # Errors
    ///
    /// A step failed. The failure returns the unchanged system when the
    /// route could not be quiesced, or a fail-stop owner otherwise.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure returns the running system"
    )]
    pub async fn stop<P>(
        self,
        radio: &SharedRadio<ConcurrentPhy>,
        platform: &mut P,
        clocks: &mut impl PlatformClockProvider,
    ) -> Result<Ieee802154Parked, Ieee802154StopFailure> {
        let Self { route, bound } = self;
        if let Some(bound) = bound
            && let Err((error, bound)) = bound.quiesce()
        {
            return Err(Ieee802154StopFailure {
                error: Ieee802154StopError::Route(error),
                owner: Ok(Self {
                    route,
                    bound: Some(bound),
                }),
            });
        }
        let parts = RUNTIME
            .uninstall()
            .expect("a running system keeps its runtime installed");
        let engine = parts.engine;
        let (mut task, interrupts) = parts.hardware.into_parts();
        let interrupts = interrupts.deactivate(&mut task);
        let (foundation, membership) = match route.into_foundation(task, interrupts) {
            Ok(parts) => parts,
            Err(failure) => {
                let error = Ieee802154StopError::Foundation(failure.failure().error());
                let (failure, membership) = failure.into_parts();
                return Err(stop_fail_stop(
                    error,
                    FailStopOwner::Joined(failure.into_reset().into_clocked(), membership),
                ));
            }
        };
        let clocked = foundation.into_clocked();

        let mut lease = lease(radio).await;
        let last = match leave_ieee802154(&mut lease, &clocked, membership) {
            Ok(last) => last,
            Err(failure) => {
                let error = Ieee802154StopError::PhyClient(failure.error());
                return Err(stop_fail_stop(
                    error,
                    FailStopOwner::Joined(clocked, failure.into_membership()),
                ));
            }
        };
        if last
            && let Err(error) =
                close_concurrent_rf::<P, EmbassyPhyTime>(&mut lease, platform, clocks).await
        {
            return Err(stop_fail_stop(
                Ieee802154StopError::PhyClose(error),
                FailStopOwner::Clocked(clocked),
            ));
        }
        let powered = match clocked.disable_clocks(&mut lease, clocks) {
            Ok(powered) => powered,
            Err(failure) => {
                return Err(stop_fail_stop(
                    Ieee802154StopError::Clocks(failure.error()),
                    FailStopOwner::Clocked(failure.into_owner()),
                ));
            }
        };
        match powered.power_down(&mut lease) {
            Ok(cold) => Ok(Ieee802154Parked {
                partition: cold.into_partition(),
                engine,
            }),
            Err(failure) => Err(stop_fail_stop(
                Ieee802154StopError::Power(failure.error()),
                FailStopOwner::Powered(failure.into_owner()),
            )),
        }
    }
}

fn stop_fail_stop(error: Ieee802154StopError, owner: FailStopOwner) -> Ieee802154StopFailure {
    Ieee802154StopFailure {
        error,
        owner: Err(Ieee802154FailStop { _owner: owner }),
    }
}
