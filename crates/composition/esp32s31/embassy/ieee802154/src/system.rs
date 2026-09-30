//! Bring-up and teardown of the IEEE 802.15.4 client on the chip.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Instant, Timer};
use esp_hal::interrupt::{InterruptHandler, Priority};
use oer_esp32s31_coex::CoexError;
use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154Clocked, Ieee802154Cold, Ieee802154FoundationCheckpoint, Ieee802154Powered,
        Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
        coex::{Ieee802154CoexConfig, Ieee802154Coexistence, resolve_priorities},
        ll::Ieee802154MacOwners,
    },
    root::Ieee802154RadioPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};
use oer_esp32s31_ieee802154_esp_hal::{
    BoundEspHalIeee802154InterruptRoute, EspHalIeee802154InterruptRouteError, bind, now_micros,
    random,
};
use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentRfError, ConcurrentTrackingTick, NoopPhyTargetObserver,
    concurrent::{
        ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, MaintenancePolicy,
        evaluate_periodic_tracking,
    },
    ieee802154_client::{
        Ieee802154PhyClientError, Ieee802154PhyMembership, RegisteredIeee802154Operational,
        RegisteredIeee802154OperationalRoute, leave_ieee802154,
    },
    maintain_concurrent_phy,
    state::client::PhyModemClient,
};
use oer_esp32s31_phy_runtime::EmbassyPhyTime;
use oer_esp32s31_radio_runtime::{Ieee802154JoinError, RadioGuard, RadioPhyError, RadioSystem};
use oer_espressif_ieee802154_engine::{
    engine::{Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces},
    pib::Ieee802154PibDefaults,
};
use oer_espressif_ieee802154_runtime::{
    Ieee802154Platform, Ieee802154Runtime, Ieee802154RuntimeError, Ieee802154RuntimeParts,
};
use static_cell::ConstStaticCell;

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

extern "C" fn ieee802154_interrupt() {
    RUNTIME.on_interrupt();
}

/// The engine's DMA frames. The MAC DMA reaches internal SRAM alone
/// ([`IEEE802154_DMA_WINDOW`](oer_esp32s31_hal::ieee802154::IEEE802154_DMA_WINDOW)), so they
/// live in the platform's DMA-visible section whatever the image's data
/// placement; one engine takes them.
#[allow(
    unsafe_code,
    reason = "the linker must retain the MAC's DMA frames in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_ieee802154_engine")]
static ENGINE_BUFFERS: ConstStaticCell<Ieee802154EngineBuffers> =
    ConstStaticCell::new(Ieee802154EngineBuffers::new());

/// The IEEE 802.15.4 partition and MAC engine while the client is stopped.
pub struct Ieee802154Parked {
    /// The partition of the concurrent split.
    pub partition: Ieee802154RadioPartition,
    /// The MAC engine and its receive storage.
    pub engine: Ieee802154Engine<'static>,
}

impl Ieee802154Parked {
    /// Park a partition with a fresh engine over the composition's
    /// DMA-visible frames, using the ESP32-S31 BTBB transmit-power levels.
    /// `None` once an engine took the frames.
    pub fn new(
        partition: Ieee802154RadioPartition,
        defaults: Ieee802154PibDefaults,
    ) -> Option<Self> {
        let buffers = ENGINE_BUFFERS.try_take()?;
        Some(Self {
            partition,
            engine: Ieee802154Engine::new(
                buffers,
                oer_esp32s31_hal::ieee802154::ESP32S31_TX_POWER_LEVELS,
                defaults,
            ),
        })
    }

    /// Park a partition with a fresh multi-PAN engine of `interfaces`
    /// interfaces (`CONFIG_IEEE802154_MULTI_PAN_ENABLE`,
    /// `CONFIG_IEEE802154_INTERFACE_NUM`): the runtime's radio then accepts
    /// interface settings and transmissions of each interface. `None` once
    /// an engine took the composition's DMA-visible frames.
    pub fn multipan(
        partition: Ieee802154RadioPartition,
        defaults: Ieee802154PibDefaults,
        interfaces: Ieee802154Interfaces,
    ) -> Option<Self> {
        let buffers = ENGINE_BUFFERS.try_take()?;
        Some(Self {
            partition,
            engine: Ieee802154Engine::new_multipan(
                buffers,
                oer_esp32s31_hal::ieee802154::ESP32S31_TX_POWER_LEVELS,
                defaults,
                interfaces,
            ),
        })
    }
}

/// Why bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154StartError {
    /// Common radio power could not be entered.
    Power(CommonRadioPowerError),
    /// The module clocks could not be enabled.
    Clocks(ModemClockError),
    /// The shared PHY domain could not be registered or woken.
    Phy(RadioPhyError),
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
    /// The engine's DMA frames lie outside the memory the MAC DMA reaches.
    BuffersNotDmaVisible,
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
    /// IEEE 802.15.4 still takes part in coexistence with Wi-Fi; disable it
    /// with [`Ieee802154System::disable_wifi_coexistence`] first.
    WifiCoexistenceEnabled,
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
// CAPABILITY: ieee802154-mac-operation-subset
pub struct Ieee802154System {
    route: RegisteredIeee802154OperationalRoute,
    /// Bound except while PHY maintenance holds the MAC paused, or after a
    /// failed rebind.
    bound: Option<BoundEspHalIeee802154InterruptRoute>,
    /// The scene levels the MAC's priorities were resolved with
    /// (`s_coex_config`).
    coex_config: Ieee802154CoexConfig,
    /// Whether IEEE 802.15.4 takes part in coexistence with Wi-Fi.
    wifi_coexistence: bool,
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

fn fail_stop(error: Ieee802154StartError, owner: FailStopOwner) -> Ieee802154StartFailure {
    Ieee802154StartFailure {
        error,
        owner: Err(Ieee802154FailStop { _owner: owner }),
    }
}

/// Roll a clocked client back to its partition.
fn unwind_clocked(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: Ieee802154Clocked,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
) -> Ieee802154StartFailure {
    let powered = match clocked.disable_clocks(lease) {
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

/// `esp_ieee802154_enable` on the shared radio.
///
/// `radio` supplies the arbiter with the platform token and clock sources,
/// and registers or wakes the shared PHY domain for this client.
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
// CAPABILITY: ieee802154-phy-and-rf-2-4-ghz-o-qpsk-250-kbit-s, ieee802154-security-power-and-coexistence-powered-lifecycle
pub async fn start<P, C: PlatformClockProvider>(
    radio: &RadioSystem<P, C>,
    parked: Ieee802154Parked,
    defaults: Ieee802154PibDefaults,
) -> Result<Ieee802154System, Ieee802154StartFailure> {
    // Frames the MAC DMA cannot reach leave the radio silent on air; refuse
    // before any hardware changes.
    if !parked
        .engine
        .buffers_dma_visible(&oer_esp32s31_hal::ieee802154::IEEE802154_DMA_WINDOW)
    {
        return Err(Ieee802154StartFailure {
            error: Ieee802154StartError::BuffersNotDmaVisible,
            owner: Ok(parked),
        });
    }
    let Ieee802154Parked {
        partition,
        mut engine,
    } = parked;
    let mut guard = radio.lock().await;
    let (lease, _, clocks) = guard.parts();

    let powered = match Ieee802154Cold::from_partition(partition).power_up(lease, clocks) {
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
    let mut clocked = match powered.enable_clocks(lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            let error = Ieee802154StartError::Clocks(failure.error());
            if failure.error() == ModemClockError::Poisoned {
                return Err(fail_stop(
                    error,
                    FailStopOwner::Powered(failure.into_owner()),
                ));
            }
            return Err(unwind_powered(lease, failure.into_owner(), engine, error));
        }
    };

    let joined = {
        let joined = core::pin::pin!(guard.join_ieee802154(&clocked));
        joined.await
    };
    let (membership, acquired) = match joined {
        Ok(joined) => joined,
        Err(Ieee802154JoinError::Phy(error)) if error.started() => {
            return Err(fail_stop(
                Ieee802154StartError::Phy(error),
                FailStopOwner::Clocked(clocked),
            ));
        }
        Err(error) => {
            let error = match error {
                Ieee802154JoinError::Phy(error) => Ieee802154StartError::Phy(error),
                Ieee802154JoinError::Client(error) => Ieee802154StartError::PhyClient(error),
            };
            let (lease, _, _) = guard.parts();
            return Err(unwind_clocked(lease, clocked, engine, error));
        }
    };
    let (lease, platform, _) = guard.parts();
    if acquired == ConcurrentAcquire::TrackingDue {
        let issued_at = Instant::now().as_micros();
        let tracked = match clocked.quiescence(issued_at, issued_at + TRACKING_WINDOW_MICROS) {
            Ok(proof) => {
                maintain_concurrent_phy::<P, EmbassyPhyTime, _>(
                    lease,
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

    let reset = match clocked.reset_mac(lease) {
        Ok(reset) => reset,
        Err(failure) => {
            let error = Ieee802154StartError::Reset(failure.error());
            return Err(leave_and_unwind(
                lease,
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
                lease,
                failure.into_reset().into_clocked(),
                membership,
                engine,
                error,
            ));
        }
    };
    engine.set_coexistence(coexistence(guard.lease(), Ieee802154CoexConfig::VENDOR));
    drop(guard);

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
    let platform_services = Ieee802154Platform { now_micros, random };
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
            coex_config: Ieee802154CoexConfig::VENDOR,
            wifi_coexistence: false,
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
    clocked: Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
) -> Ieee802154StartFailure {
    if leave_ieee802154(lease, &clocked, membership).is_err() {
        return fail_stop(error, FailStopOwner::Clocked(clocked));
    }
    unwind_clocked(lease, clocked, engine, error)
}

/// The shared radio is a software-coexistence build: the MAC publishes the
/// priorities of `config` read from the arbiter's current table.
fn coexistence(
    lease: &SharedRadioLease<'_, ConcurrentPhy>,
    config: Ieee802154CoexConfig,
) -> Ieee802154Coexistence {
    Ieee802154Coexistence::Software(resolve_priorities(config, &lease.coex_pti_table()))
}

impl Ieee802154System {
    /// The runtime that accepts commands and yields events.
    pub fn runtime(&self) -> &'static Ieee802154SystemRuntime {
        &RUNTIME
    }

    /// The live RSSI read of [`Ieee802154SystemRuntime::recent_rssi`] as a
    /// function, for synchronous callers that hold no system, such as
    /// OpenThread's `otPlatRadioGetRssi`. It reads `None` while no radio is
    /// installed.
    pub fn recent_rssi_reader(&self) -> fn() -> Option<i8> {
        || RUNTIME.recent_rssi().ok()
    }

    /// Read the arbiter's coexistence table again and publish the scene
    /// levels of `config` (`esp_ieee802154_set_coex_config`). Call it after
    /// the table or the levels change; the TX/RX priority applies from the
    /// next operation start, the ACK priority from the next start of the
    /// client, as in the vendor driver.
    ///
    /// # Errors
    ///
    /// The runtime holds no radio.
    pub async fn update_coexistence<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C>,
        config: Ieee802154CoexConfig,
    ) -> Result<(), Ieee802154RuntimeError> {
        let mut guard = radio.lock().await;
        RUNTIME.set_coexistence(coexistence(guard.lease(), config))?;
        self.coex_config = config;
        Ok(())
    }

    /// Take part in coexistence with Wi-Fi, as ESP-IDF's
    /// `esp_coex_wifi_i154_enable` does for a Thread border router: enable
    /// coexistence for IEEE 802.15.4 and publish its schedule status, so the
    /// coexistence schedule counts it (`RadioGuard::enable_ieee802154_coex`).
    /// The image must run `RadioSystem::run_coex_schedule` once, as its
    /// Wi-Fi composition does. Nothing happens when it already takes part.
    pub async fn enable_wifi_coexistence<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C>,
    ) {
        if self.wifi_coexistence {
            return;
        }
        radio.lock().await.enable_ieee802154_coex();
        self.wifi_coexistence = true;
    }

    /// Leave coexistence with Wi-Fi: withdraw the schedule status and
    /// disable coexistence for IEEE 802.15.4
    /// (`RadioGuard::disable_ieee802154_coex`). [`Self::stop`] requires it
    /// after [`Self::enable_wifi_coexistence`]. Nothing happens when it does
    /// not take part.
    ///
    /// # Errors
    ///
    /// A coexistence timer could not be disabled; IEEE 802.15.4 has left
    /// coexistence and the core keeps the timer as uncertain.
    pub async fn disable_wifi_coexistence<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C>,
    ) -> Result<(), CoexError> {
        if !self.wifi_coexistence {
            return Ok(());
        }
        self.wifi_coexistence = false;
        radio.lock().await.disable_ieee802154_coex()
    }

    /// Whether IEEE 802.15.4 takes part in coexistence with Wi-Fi.
    pub const fn wifi_coexistence(&self) -> bool {
        self.wifi_coexistence
    }

    /// The scene levels the MAC's priorities were last resolved with
    /// (`esp_ieee802154_get_coex_config`): the vendor default after start.
    pub const fn coex_config(&self) -> Ieee802154CoexConfig {
        self.coex_config
    }

    /// Run one shared PHY tracking tick under the domain's maintenance
    /// policy, as one tick of ESP-IDF's periodic `phy_track_pll`.
    ///
    /// Under the default [`MaintenancePolicy::Vendor`] this is one tick of
    /// the radio's periodic tracking: it runs with IEEE 802.15.4 receiving,
    /// as the vendor timer does, and needs no pause. Under
    /// [`MaintenancePolicy::Quiesced`] every active client must prove
    /// quiescence; when IEEE 802.15.4 is the only active client this pauses
    /// the runtime (leaving receive mode), closes the CPU route, issues the
    /// proof, runs the tracking transaction within it, and then resumes the
    /// runtime and binds the route again. Call it on the core that started
    /// the client.
    ///
    /// # Errors
    ///
    /// The domain rejected the evaluation or the maintenance, tracking failed
    /// after it started, or the route could not be bound again.
    pub async fn maintain_phy<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C>,
    ) -> Result<Ieee802154PhyMaintenance, Ieee802154MaintenanceError> {
        let mut guard = radio.lock().await;
        if guard.lease().attachment().maintenance_policy() == MaintenancePolicy::Vendor {
            return match guard.track().await {
                Ok(ConcurrentTrackingTick::NotDue) => Ok(Ieee802154PhyMaintenance::NotDue),
                Ok(ConcurrentTrackingTick::Tracked(_outcome)) => {
                    Ok(Ieee802154PhyMaintenance::Tracked)
                }
                Ok(ConcurrentTrackingTick::Unavailable(error)) => {
                    Err(Ieee802154MaintenanceError::Phy(error))
                }
                Ok(ConcurrentTrackingTick::AwaitingQuiescence) => {
                    // The policy was read under this guard.
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
        let lease = guard.lease();
        let due = lease.attachment().tracking_pending()
            || evaluate_periodic_tracking(lease, &mut EmbassyPhyTime)
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
        let tracked = core::pin::pin!(self.track_quiescent(&mut guard));
        tracked.await
    }

    /// Track the shared PHY within IEEE 802.15.4's quiescence: pause the
    /// runtime (leaving receive mode), close the CPU route, issue the
    /// proof, track within it, then resume and bind the route again.
    async fn track_quiescent<P, C: PlatformClockProvider>(
        &mut self,
        guard: &mut RadioGuard<'_, P, C>,
    ) -> Result<Ieee802154PhyMaintenance, Ieee802154MaintenanceError> {
        let (lease, platform, _) = guard.parts();
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
                    lease,
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
    /// client; under the default vendor policy the radio's
    /// [`RadioSystem::run_tracking`] is the periodic loop. `observe` receives each tick's
    /// outcome. Run it on the core that started the client and call
    /// [`Self::stop`] after it returns.
    ///
    /// # Errors
    ///
    /// A maintenance attempt failed; tracking stops being attempted.
    pub async fn maintain_phy_until<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C>,
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
                let maintained = core::pin::pin!(self.maintain_phy(radio));
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
    pub async fn stop<P, C: PlatformClockProvider>(
        self,
        radio: &RadioSystem<P, C>,
    ) -> Result<Ieee802154Parked, Ieee802154StopFailure> {
        let Self {
            route,
            bound,
            coex_config,
            wifi_coexistence,
        } = self;
        if wifi_coexistence {
            return Err(Ieee802154StopFailure {
                error: Ieee802154StopError::WifiCoexistenceEnabled,
                owner: Ok(Self {
                    route,
                    bound,
                    coex_config,
                    wifi_coexistence,
                }),
            });
        }
        if let Some(bound) = bound
            && let Err((error, bound)) = bound.quiesce()
        {
            return Err(Ieee802154StopFailure {
                error: Ieee802154StopError::Route(error),
                owner: Ok(Self {
                    route,
                    bound: Some(bound),
                    coex_config,
                    wifi_coexistence,
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
        let mut guard = radio.lock().await;
        let left = {
            let left = core::pin::pin!(guard.leave_ieee802154(&clocked, membership));
            left.await
        };
        let left = match left {
            Ok(left) => left,
            Err(failure) => {
                let error = Ieee802154StopError::PhyClient(failure.error());
                return Err(stop_fail_stop(
                    error,
                    FailStopOwner::Joined(clocked, failure.into_membership()),
                ));
            }
        };
        // RF closed after the last client; it was already closed, or stays
        // open for another client, otherwise.
        if let Err(error) = left.rf_closed {
            return Err(stop_fail_stop(
                Ieee802154StopError::PhyClose(error),
                FailStopOwner::Clocked(clocked),
            ));
        }
        let (lease, _, _) = guard.parts();
        let powered = match clocked.disable_clocks(lease) {
            Ok(powered) => powered,
            Err(failure) => {
                return Err(stop_fail_stop(
                    Ieee802154StopError::Clocks(failure.error()),
                    FailStopOwner::Clocked(failure.into_owner()),
                ));
            }
        };
        match powered.power_down(lease) {
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
