//! The arbiter, its shared resources, the periodic tracking timer and the
//! coexistence schedule's phase timer.

use core::cell::Cell;
use core::{future::Future, task::Poll};
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex},
    mutex::{Mutex, MutexGuard},
    signal::Signal,
};

use embassy_time::{Instant, Timer};
use oer_esp32s31_coex::{
    CoexArbiterPorts, CoexClientRequest, CoexCore, CoexError, CoexEventId, CoexExpiry, CoexPhase,
    CoexPhaseChange, CoexPhaseTimer, CoexPti, CoexPtiTable, CoexSchedule, CoexScheduleExecutor,
    CoexStatusType, CoexTimerIndex,
};
use oer_esp32s31_hal::coex::{ExternalCoexConfig, ExternalCoexError};
use oer_esp32s31_hal::{
    ieee80211::client::WifiClocksOn,
    ieee802154::Ieee802154Clocked,
    root::{ConcurrentPartitions, RadioHardware},
    shared_radio::{PlatformClockProvider, SharedRadio, SharedRadioLease},
};
use oer_esp32s31_phy::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyRegistration, ConcurrentPhyTrackingError,
    ConcurrentRfError, ConcurrentTrackingTick, NoopPhyTargetObserver, PhyCalibrationCache,
    PhyCalibrationIdentity, PhyRegisterConfig, close_concurrent_rf,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    ieee802154_client::{
        Ieee802154PhyClientError, Ieee802154PhyLeaveFailure, Ieee802154PhyMembership,
        join_ieee802154, leave_ieee802154,
    },
    register_concurrent_phy,
    state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
    track_concurrent_phy, wake_concurrent_rf,
    wifi_client::{
        WifiPhyLeaveFailure, WifiPhyMembership, WifiPhySuspended, resume_wifi, suspend_wifi,
    },
};
use oer_esp32s31_phy_runtime::EmbassyPhyTime;

/// The external coexistence schedule status bits `ic_set_extern_coex` sets
/// and `ic_stop_extern_coex` clears (`coex_schm_status_bit_set(3, 1 | 2)`).
const EXTERNAL_COEX_STATUS: u16 = 0x01 | 0x02;

/// The IEEE 802.15.4 schedule status bit of the pinned libcoexist
/// `esp_coex_ieee802154_status_enable` and `_disable`
/// (`coex_schm_status_bit_set/clear(4, 1)`).
const IEEE802154_COEX_STATUS: u16 = 0x01;

/// The event whose priority Wi-Fi's power management reads (`coex_pti_get(0)`).
const BEACON_EVENT: CoexEventId = match CoexEventId::new(0) {
    Some(event) => event,
    None => unreachable!(),
};

/// The share of the schedule's first phase, or zero without one.
fn first_share(schedule: &CoexSchedule) -> u8 {
    schedule
        .phase_by_index(0)
        .map_or(0, |phase| phase.share_percent())
}

/// Retry period while another holder owns the arbiter lease.
const LEASE_RETRY_MICROS: u64 = 100;

/// The platform resources every shared-PHY transaction borrows.
pub struct RadioResources<P, C> {
    /// The integration token the PHY target port borrows.
    pub platform: P,
    /// The platform sources of the modem clocks.
    pub clocks: C,
    /// The retained cache the first registration validates and replays.
    retained_cache: Option<PhyCalibrationCache>,
    /// The coexistence request policy on the arbiter's timer bank.
    coex: CoexCore,
    /// The coexistence schedule and its phase timer state.
    schedule: CoexScheduleExecutor,
    /// Radios that enabled coexistence (`coex_core_enable` count).
    coex_clients: u8,
    /// The Wi-Fi channel `coex_wifi_channel_set` last recorded.
    wifi_channel: CoexWifiChannel,
}

/// The Wi-Fi channel the coexistence module records for Bluetooth.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoexWifiChannel {
    /// Primary channel.
    pub primary: u8,
    /// Secondary channel.
    pub secondary: u8,
}

/// The coexistence values Wi-Fi's power management reads on every beacon
/// and slice, as the vendor reads them live: `coex_status_get(WIFI)`, the
/// schedule's current and flexible period, its interval, the share of its
/// first phase and the priority of event 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiCoexView {
    /// [`RadioGuard::coex_active_for`] of Wi-Fi.
    pub active: bool,
    /// `coex_schm_curr_period_get`.
    pub current_period: u8,
    /// `coex_schm_flexible_period_get`.
    pub flexible_period: u8,
    /// `coex_schm_interval_get`.
    pub interval: u32,
    /// The share of `coex_schm_get_phase_by_idx(0)`, or zero without one.
    pub first_phase_share_percent: u8,
    /// `coex_pti_get(0)`.
    pub beacon_pti: CoexPti,
}

/// The latest [`WifiCoexView`], readable without the arbiter lease.
///
/// Every [`RadioGuard`] refreshes it when it is dropped, so it reflects each
/// completed coexistence change.
pub struct WifiCoexViewCell(BlockingMutex<CriticalSectionRawMutex, Cell<WifiCoexView>>);

impl WifiCoexViewCell {
    /// The view after the last released guard.
    pub fn get(&self) -> WifiCoexView {
        self.0.lock(Cell::get)
    }

    fn set(&self, view: WifiCoexView) {
        self.0.lock(|cell| cell.set(view));
    }
}

/// Why external coexistence could not stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalCoexStopError {
    /// The arbiter's external coexistence could not stop.
    External(ExternalCoexError),
    /// Disabling coexistence could not withdraw a request timer.
    Coex(CoexError),
}

/// When a Bluetooth preemption ends, as `coex_iso_end_int_handle` reports it
/// to Wi-Fi.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexPreemptionEnd {
    /// The preemption ends at this instant.
    At(Instant),
    /// Bluetooth reported no end time.
    Unknown,
}

/// The coexistence schedule's wake-ups: its phase-timer command and the
/// phase events of the two radios the vendor schedule notifies.
struct CoexSignals {
    timer: Signal<CriticalSectionRawMutex, CoexPhaseTimer>,
    wifi: Signal<CriticalSectionRawMutex, CoexPhase>,
    bluetooth: Signal<CriticalSectionRawMutex, CoexPhase>,
    wifi_started: Signal<CriticalSectionRawMutex, ()>,
    bluetooth_started: Signal<CriticalSectionRawMutex, bool>,
    wifi_channel: Signal<CriticalSectionRawMutex, CoexWifiChannel>,
    preemption_end: Signal<CriticalSectionRawMutex, CoexPreemptionEnd>,
    wifi_view: WifiCoexViewCell,
}

impl CoexSignals {
    fn new() -> Self {
        Self {
            timer: Signal::new(),
            wifi: Signal::new(),
            bluetooth: Signal::new(),
            wifi_started: Signal::new(),
            bluetooth_started: Signal::new(),
            wifi_channel: Signal::new(),
            preemption_end: Signal::new(),
            wifi_view: WifiCoexViewCell(BlockingMutex::new(Cell::new(WifiCoexView {
                active: false,
                current_period: CoexSchedule::new().current_period(),
                flexible_period: 0,
                interval: 0,
                first_phase_share_percent: first_share(&CoexSchedule::new()),
                beacon_pti: CoexPtiTable::VENDOR.pti(BEACON_EVENT),
            }))),
        }
    }

    /// Program the phase timer and notify Wi-Fi, then Bluetooth, as the
    /// vendor phase change does after its schedule lock.
    fn publish(&self, change: CoexPhaseChange) {
        self.timer.signal(change.timer);
        if change.step.notify_wifi {
            self.wifi.signal(change.step.phase);
        }
        if change.step.notify_bluetooth {
            self.bluetooth.signal(change.step.phase);
        }
    }
}

/// The shared radio: the arbiter with its PHY domain and the platform
/// resources, taken together through [`Self::lock`].
pub struct RadioSystem<P, C> {
    radio: SharedRadio<ConcurrentPhy>,
    /// Taken only while the arbiter lease is held, so taking it never waits.
    resources: Mutex<CriticalSectionRawMutex, RadioResources<P, C>>,
    identity: PhyCalibrationIdentity,
    coex: CoexSignals,
}

/// The arbiter lease together with the platform resources.
///
/// Dropping it releases both. Hold it only for one radio transaction.
pub struct RadioGuard<'radio, P, C> {
    // Declared first so it drops before the lease that protects it.
    resources: MutexGuard<'radio, CriticalSectionRawMutex, RadioResources<P, C>>,
    lease: SharedRadioLease<'radio, ConcurrentPhy>,
    identity: PhyCalibrationIdentity,
    coex: &'radio CoexSignals,
}

/// How [`RadioGuard::prepare_phy`] made the shared PHY ready.
#[must_use = "a registration carries the new calibration cache and outcome"]
#[allow(
    clippy::large_enum_variant,
    reason = "the registration carries its calibration cache inline without allocation"
)]
pub enum RadioPhyPrepared {
    /// This call registered the domain; the registration reports its
    /// calibration path and carries the fresh cache.
    Registered(ConcurrentPhyRegistration),
    /// Closed RF was woken without calibration.
    Woken,
    /// The domain was registered with RF open already.
    AlreadyOpen,
}

/// Why the shared PHY could not be prepared for a joining client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPhyError {
    /// Registration was rejected before any register access.
    Registration(ConcurrentPhyError),
    /// Registration started and failed; the radio requires reset.
    RegistrationFailed,
    /// Closed RF could not be woken.
    Wake(ConcurrentRfError),
}

impl RadioPhyError {
    /// Whether a hardware transaction started, so the radio requires reset.
    pub const fn started(self) -> bool {
        match self {
            Self::Registration(_) => false,
            Self::RegistrationFailed => true,
            Self::Wake(error) => !matches!(error, ConcurrentRfError::Rejected(_)),
        }
    }
}

/// Why IEEE 802.15.4 could not join the shared PHY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154JoinError {
    /// The shared PHY could not be prepared ([`RadioPhyError::started`]).
    Phy(RadioPhyError),
    /// The PHY domain rejected the client; the client set is unchanged.
    Client(Ieee802154PhyClientError),
}

/// IEEE 802.15.4 after it left the shared PHY with
/// [`RadioGuard::leave_ieee802154`].
#[must_use = "closing RF after the last client may have failed"]
pub struct Ieee802154Left {
    /// Whether RF closed because no client remains, or why closing it
    /// failed; a [`ConcurrentRfError::Recoverable`] failure keeps RF open,
    /// any other started failure requires reset.
    pub rf_closed: Result<bool, ConcurrentRfError>,
}

/// Wi-Fi asleep after [`RadioGuard::suspend_wifi`].
#[must_use = "closing RF after the last client may have failed"]
pub struct WifiAsleep {
    /// The suspended client, to resume or leave.
    pub suspended: WifiPhySuspended,
    /// Whether RF closed because Wi-Fi was the last client, or why closing
    /// it failed; a [`ConcurrentRfError::Recoverable`] failure keeps RF open,
    /// any other started failure requires reset.
    pub rf_closed: Result<bool, ConcurrentRfError>,
}

/// Failed wake retaining the suspended Wi-Fi client.
#[must_use = "a failed wake still owns the suspended Wi-Fi client"]
pub struct WifiWakeFailure {
    /// Why Wi-Fi did not wake.
    pub error: WifiWakeError,
    /// The unchanged suspended client.
    pub suspended: WifiPhySuspended,
}

/// Why sleeping Wi-Fi could not wake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiWakeError {
    /// The shared PHY could not be prepared ([`RadioPhyError::started`]).
    Phy(RadioPhyError),
    /// The PHY domain rejected the client; nothing changed.
    Client(ConcurrentPhyError),
}

impl<P, C: PlatformClockProvider> RadioSystem<P, C> {
    /// Split the radio for concurrent clients.
    ///
    /// `identity` registers the shared PHY domain when its first client
    /// joins. The partitions go to the protocol compositions.
    pub fn new(
        hardware: RadioHardware,
        platform: P,
        clocks: C,
        identity: PhyCalibrationIdentity,
    ) -> (Self, ConcurrentPartitions) {
        let (radio, partitions) = hardware.into_concurrent(ConcurrentPhy::new());
        (
            Self {
                radio,
                resources: Mutex::new(RadioResources {
                    platform,
                    clocks,
                    retained_cache: None,
                    coex: CoexCore::new(),
                    schedule: CoexScheduleExecutor::new(),
                    coex_clients: 0,
                    wifi_channel: CoexWifiChannel::default(),
                }),
                identity,
                coex: CoexSignals::new(),
            },
            partitions,
        )
    }

    /// Supply a retained calibration cache for the first registration, which
    /// validates it against the calibration identity and replays it instead
    /// of calibrating fully when it matches. Call it before any client
    /// prepares the PHY; a later registration of the same system starts
    /// without a cache.
    #[must_use]
    pub fn with_calibration_cache(mut self, cache: PhyCalibrationCache) -> Self {
        self.resources.get_mut().retained_cache = Some(cache);
        self
    }

    /// Take the arbiter lease and the platform resources, waiting while
    /// another holder owns the lease, as ESP-IDF waits for its PHY lock.
    pub async fn lock(&self) -> RadioGuard<'_, P, C> {
        let lease = loop {
            match self.radio.try_acquire() {
                Ok(lease) => break lease,
                Err(_) => Timer::after_micros(LEASE_RETRY_MICROS).await,
            }
        };
        // Every holder of the resources holds the lease, so this is free.
        let resources = self.resources.lock().await;
        RadioGuard {
            resources,
            lease,
            identity: self.identity,
            coex: &self.coex,
        }
    }

    /// One tick of the vendor `phy_track_pll`, under its own lease.
    ///
    /// # Errors
    ///
    /// As [`RadioGuard::track`].
    pub async fn track(&self) -> Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError> {
        self.lock().await.track().await
    }

    /// ESP-IDF's periodic `phy_track_pll` timer for the shared domain.
    ///
    /// Every tracking period it runs one [`Self::track`] tick. A domain that
    /// is not registered or has RF closed is skipped until the next period.
    /// Run it for the lifetime of the radio, for example as its own task.
    ///
    /// # Errors
    ///
    /// Returns only when a tick fails or the domain is poisoned; the radio
    /// then requires reset.
    ///
    /// # Cancellation
    ///
    /// Cancel it only while it waits between ticks; cancelling a running
    /// tracking transaction requires reset.
    pub async fn run_tracking(&self) -> ConcurrentPhyTrackingError {
        self.run_tracking_observed(|_| {}).await
    }

    /// [`Self::run_tracking`] reporting every tick's result to `observe`
    /// before the loop decides whether to continue.
    ///
    /// # Errors
    ///
    /// As [`Self::run_tracking`].
    ///
    /// # Cancellation
    ///
    /// As [`Self::run_tracking`].
    pub async fn run_tracking_observed(
        &self,
        observe: impl FnMut(&Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>),
    ) -> ConcurrentPhyTrackingError {
        match self
            .run_tracking_until(core::future::pending::<()>(), observe)
            .await
        {
            Err(error) => error,
            Ok(()) => unreachable!("a pending stop never completes"),
        }
    }

    /// [`Self::run_tracking_observed`] that ends when `stop` completes.
    ///
    /// `stop` is polled only while the timer waits between ticks; a started
    /// tick always runs to its terminal result first. Stopping therefore
    /// never leaves a tracking transaction half done, and the owner may end
    /// the timer without a reset.
    ///
    /// # Errors
    ///
    /// A tick failed or the domain is poisoned; the radio then requires
    /// reset. `Ok(())` means `stop` completed.
    ///
    /// # Cancellation
    ///
    /// Dropping this future itself follows [`Self::run_tracking`]: do it only
    /// while it waits; prefer `stop`.
    pub async fn run_tracking_until(
        &self,
        stop: impl Future<Output = ()>,
        mut observe: impl FnMut(&Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>),
    ) -> Result<(), ConcurrentPhyTrackingError> {
        let mut stop = core::pin::pin!(stop);
        loop {
            let mut period = Timer::after_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS);
            let stopped = core::future::poll_fn(|context| {
                if stop.as_mut().poll(context).is_ready() {
                    return Poll::Ready(true);
                }
                if core::pin::Pin::new(&mut period).poll(context).is_ready() {
                    return Poll::Ready(false);
                }
                Poll::Pending
            })
            .await;
            if stopped {
                return Ok(());
            }
            let tick = self.track().await;
            observe(&tick);
            match tick {
                Ok(ConcurrentTrackingTick::Unavailable(ConcurrentPhyError::Poisoned)) => {
                    return Err(ConcurrentPhyTrackingError::Rejected(
                        ConcurrentPhyError::Poisoned,
                    ));
                }
                Ok(_) => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// The coexistence values Wi-Fi reads without the arbiter lease; the
    /// cell borrows no generic of the system.
    pub fn wifi_coex_view(&self) -> &WifiCoexViewCell {
        &self.coex.wifi_view
    }

    /// The next coexistence phase that notifies Wi-Fi, as the vendor phase
    /// change calls Wi-Fi's phase handler. A phase published while nobody
    /// waits is kept until the next wait; a later one replaces it.
    pub async fn wifi_coex_phase(&self) -> CoexPhase {
        self.coex.wifi.wait().await
    }

    /// The next coexistence phase that notifies Bluetooth, after Wi-Fi's.
    /// Unread phases are replaced as in [`Self::wifi_coex_phase`].
    pub async fn bluetooth_coex_phase(&self) -> CoexPhase {
        self.coex.bluetooth.wait().await
    }

    /// The next start of coexistence, as the vendor calls the Wi-Fi start
    /// callback (`coex_register_start_cb`): a second radio enabled it.
    pub async fn wifi_coex_started(&self) {
        self.coex.wifi_started.wait().await;
    }

    /// The next coexistence start (`true`) or stop (`false`), as the vendor
    /// calls the two BLE callbacks of `coex_register_ble_cb`. Only the latest
    /// unread change is kept.
    pub async fn bluetooth_coex_started(&self) -> bool {
        self.coex.bluetooth_started.wait().await
    }

    /// The next Wi-Fi channel change, as the vendor calls the Bluetooth
    /// callback of `coex_register_wifi_channel_change_callback`. Only the
    /// latest unread channel is kept.
    pub async fn bluetooth_wifi_channel(&self) -> CoexWifiChannel {
        self.coex.wifi_channel.wait().await
    }

    /// The next end of a Bluetooth preemption, as the vendor calls the Wi-Fi
    /// callback of `esp_coex_configure_iso_end_wifi_cb`. Only the latest
    /// unread end is kept.
    pub async fn wifi_coex_preemption_end(&self) -> CoexPreemptionEnd {
        self.coex.preemption_end.wait().await
    }

    /// The coexistence schedule's phase timer, the vendor `esp_timer` whose
    /// callback runs `coex_schm_timeout_process`. Run it for the lifetime of
    /// the radio, for example as its own task.
    ///
    /// A phase change under [`RadioGuard`] programs the timer; its expiry
    /// steps the schedule under the arbiter lease and publishes the next
    /// phase. An expiry overtaken by a later phase change steps nothing.
    ///
    /// # Cancellation
    ///
    /// Cancelling it stops the phases where they are; an expiry that has
    /// taken the lease completes its step first.
    pub async fn run_coex_schedule(&self) -> ! {
        self.run_coex_schedule_until(core::future::pending()).await;
        unreachable!("a pending stop never completes")
    }

    /// [`Self::run_coex_schedule`] that ends when `stop` completes. `stop` is
    /// polled only while the timer waits.
    pub async fn run_coex_schedule_until(&self, stop: impl Future<Output = ()>) {
        let mut stop = core::pin::pin!(stop);
        let mut timer = CoexPhaseTimer::Disarm;
        loop {
            let generation = match timer {
                CoexPhaseTimer::Disarm => None,
                CoexPhaseTimer::Arm { generation, .. } => Some(generation),
            };
            let mut expiry = match timer {
                CoexPhaseTimer::Arm { micros, .. } => Some(Timer::after_micros(u64::from(micros))),
                CoexPhaseTimer::Disarm => None,
            };
            let mut command = core::pin::pin!(self.coex.timer.wait());
            let event = core::future::poll_fn(|context| {
                if stop.as_mut().poll(context).is_ready() {
                    return Poll::Ready(CoexTimerEvent::Stop);
                }
                if let Poll::Ready(next) = command.as_mut().poll(context) {
                    return Poll::Ready(CoexTimerEvent::Command(next));
                }
                if let Some(expiry) = expiry.as_mut()
                    && core::pin::Pin::new(expiry).poll(context).is_ready()
                {
                    return Poll::Ready(CoexTimerEvent::Expired);
                }
                Poll::Pending
            })
            .await;
            timer = match event {
                CoexTimerEvent::Stop => return,
                CoexTimerEvent::Command(next) => next,
                CoexTimerEvent::Expired => {
                    if let Some(generation) = generation {
                        // A phase change publishes its own timer command,
                        // which the next iteration reads.
                        self.lock().await.expire_coex_phase(generation);
                    }
                    CoexPhaseTimer::Disarm
                }
            };
        }
    }
}

/// What woke the coexistence phase timer.
enum CoexTimerEvent {
    Stop,
    Command(CoexPhaseTimer),
    Expired,
}

impl<P, C> Drop for RadioGuard<'_, P, C> {
    fn drop(&mut self) {
        let schedule = self.resources.schedule.schedule();
        self.coex.wifi_view.set(WifiCoexView {
            active: self.resources.coex_clients >= 2
                && schedule.status().others_publish(CoexStatusType::Wifi),
            current_period: schedule.current_period(),
            flexible_period: schedule.flexible_period(),
            interval: schedule.interval(),
            first_phase_share_percent: first_share(schedule),
            beacon_pti: self.lease.coex_pti(BEACON_EVENT),
        });
    }
}

impl<'radio, P, C: PlatformClockProvider> RadioGuard<'radio, P, C> {
    /// The arbiter lease.
    pub fn lease(&mut self) -> &mut SharedRadioLease<'radio, ConcurrentPhy> {
        &mut self.lease
    }

    /// The arbiter lease with the platform token and clock sources.
    pub fn parts(&mut self) -> (&mut SharedRadioLease<'radio, ConcurrentPhy>, &mut P, &mut C) {
        let RadioResources {
            platform, clocks, ..
        } = &mut *self.resources;
        (&mut self.lease, platform, clocks)
    }

    /// The first-client half of `esp_phy_enable`: register the shared PHY
    /// domain once, with the retained calibration cache when one was
    /// supplied, or wake its closed RF. A registered, open domain is left
    /// unchanged. The domain stays registered across RF close and wake, as
    /// ESP-IDF's calibrated PHY does, so only the first preparation
    /// registers.
    ///
    /// # Errors
    ///
    /// Registration or wake was rejected before any register access, or
    /// started and failed ([`RadioPhyError::started`]).
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn prepare_phy(&mut self) -> Result<RadioPhyPrepared, RadioPhyError> {
        let identity = self.identity;
        let RadioResources {
            platform,
            clocks,
            retained_cache,
            ..
        } = &mut *self.resources;
        let lease = &mut self.lease;
        if lease.attachment().rf_closed() {
            return wake_concurrent_rf::<EmbassyPhyTime>(lease, clocks)
                .await
                .map(|()| RadioPhyPrepared::Woken)
                .map_err(RadioPhyError::Wake);
        }
        if lease.attachment().client_snapshot().is_some() {
            return Ok(RadioPhyPrepared::AlreadyOpen);
        }
        let config = match retained_cache.take() {
            Some(cache) => PhyRegisterConfig::new(identity).with_calibration_cache(cache),
            None => PhyRegisterConfig::new(identity),
        };
        match register_concurrent_phy::<P, EmbassyPhyTime, _>(
            lease,
            platform,
            clocks,
            config,
            NoopPhyTargetObserver,
        )
        .await
        {
            Ok(registration) => Ok(RadioPhyPrepared::Registered(registration)),
            Err(ConcurrentPhyRegisterFailure::Rejected(ConcurrentPhyError::AlreadyRegistered)) => {
                Ok(RadioPhyPrepared::AlreadyOpen)
            }
            Err(ConcurrentPhyRegisterFailure::Rejected(error)) => {
                Err(RadioPhyError::Registration(error))
            }
            Err(_) => Err(RadioPhyError::RegistrationFailed),
        }
    }

    /// Capture the registered domain's current calibration state as a cache
    /// for the next cold registration, with the system's calibration
    /// identity: the final-state handoff after runtime tracking.
    ///
    /// # Errors
    ///
    /// The domain is not registered, awaits tracking or is poisoned.
    pub fn calibration_cache(&self) -> Result<PhyCalibrationCache, ConcurrentPhyError> {
        self.lease.attachment().calibration_cache(self.identity)
    }

    /// The last-client half of `esp_phy_disable`: close RF when the domain
    /// is open and no client remains. Returns whether RF was closed.
    ///
    /// # Errors
    ///
    /// RF close failed; a [`ConcurrentRfError::Recoverable`] failure keeps
    /// RF open, any other started failure requires reset.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn close_phy_if_idle(&mut self) -> Result<bool, ConcurrentRfError> {
        let (lease, platform, clocks) = self.parts();
        let idle = !lease.attachment().rf_closed()
            && lease
                .attachment()
                .client_snapshot()
                .is_some_and(|clients| clients.is_empty());
        if !idle {
            return Ok(false);
        }
        close_concurrent_rf::<P, EmbassyPhyTime>(lease, platform, clocks)
            .await
            .map(|()| true)
    }

    /// Join IEEE 802.15.4 to the shared PHY, as the vendor
    /// `esp_ieee802154_enable` does through `esp_phy_enable` and
    /// `esp_btbb_enable`: prepare the shared PHY - registering the domain
    /// when no client did, or waking closed RF - then take the BTBB
    /// reference and enter the PHY client set. A returned
    /// [`ConcurrentAcquire::TrackingDue`] means the domain must run its
    /// tracking, within the client's quiescence, before the MAC uses RF.
    ///
    /// # Errors
    ///
    /// Preparing the shared PHY failed ([`RadioPhyError::started`] tells
    /// whether hardware work began), or the domain rejected the client.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn join_ieee802154(
        &mut self,
        clocked: &Ieee802154Clocked,
    ) -> Result<(Ieee802154PhyMembership, ConcurrentAcquire), Ieee802154JoinError> {
        // The client needs RF open, not the registration report.
        runtime_trace(0x6001);
        trace_rate();
        let _prepared = self.prepare_phy().await.map_err(Ieee802154JoinError::Phy)?;
        runtime_trace(0x6002);
        trace_rate();
        let joined = join_ieee802154(self.lease(), clocked, &mut EmbassyPhyTime)
            .map_err(Ieee802154JoinError::Client);
        runtime_trace(0x6003);
        trace_rate();
        joined
    }

    /// Leave the shared PHY, as the vendor `esp_ieee802154_disable` does
    /// through `esp_phy_disable` and `esp_btbb_disable`: release the PHY
    /// client and the BTBB reference, then close RF when no client remains.
    ///
    /// The caller must have stopped the MAC first.
    ///
    /// # Errors
    ///
    /// The domain rejected the release; nothing changed and the membership
    /// is returned. An RF close failure after the release is reported in
    /// [`Ieee802154Left::rf_closed`].
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn leave_ieee802154(
        &mut self,
        clocked: &Ieee802154Clocked,
        membership: Ieee802154PhyMembership,
    ) -> Result<Ieee802154Left, Ieee802154PhyLeaveFailure> {
        runtime_trace(0x4001);
        trace_rate();
        let last = leave_ieee802154(self.lease(), clocked, membership)?;
        runtime_trace(0x4002);
        trace_rate();
        let rf_closed = if last {
            self.close_phy_if_idle().await
        } else {
            Ok(false)
        };
        runtime_trace(0x4003);
        trace_rate();
        Ok(Ieee802154Left { rf_closed })
    }

    /// One tick of the vendor `phy_track_pll` under this guard: evaluate
    /// the clients' tracking period and run due tracking under the domain's
    /// admission policy.
    ///
    /// # Errors
    ///
    /// The tracking clock was rejected, or tracking started and failed; the
    /// domain is then poisoned.
    ///
    /// # Cancellation
    ///
    /// Once tracking starts, drive this future to a terminal result.
    pub async fn track(&mut self) -> Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError> {
        let (lease, platform, _) = self.parts();
        track_concurrent_phy::<P, EmbassyPhyTime, _>(
            lease,
            platform,
            &mut EmbassyPhyTime,
            NoopPhyTargetObserver,
        )
        .await
    }

    /// Enable coexistence for one radio, as `coex_enable` does. Each radio
    /// enables it once; the first enables requests, and every later one
    /// starts coexistence: Wi-Fi's start wait and Bluetooth's start wait
    /// wake, as the vendor calls its start callbacks.
    pub fn enable_coex(&mut self) {
        let clients = self.resources.coex_clients.saturating_add(1);
        self.resources.coex_clients = clients;
        if clients == 1 {
            // `coex_pre_init` selects the policy timers' clock before any
            // radio can request the air.
            CoexArbiterPorts::new(&mut self.lease).configure_timer_clock();
            self.resources.coex.enable();
        } else {
            self.coex.wifi_started.signal(());
            self.coex.bluetooth_started.signal(true);
        }
    }

    /// Disable coexistence for one radio, as `coex_disable` does. When one
    /// radio remains, coexistence stops and Bluetooth's start wait reports
    /// the stop; when none remains, every programmed request is withdrawn
    /// and requests are disabled. Without an enabled radio nothing changes.
    ///
    /// # Errors
    ///
    /// A timer could not be disabled; the core keeps it as uncertain.
    pub fn disable_coex(&mut self) -> Result<(), CoexError> {
        let Some(clients) = self.resources.coex_clients.checked_sub(1) else {
            return Ok(());
        };
        self.resources.coex_clients = clients;
        match clients {
            0 => {
                let ports = CoexArbiterPorts::new(&mut self.lease);
                let (mut timer, _) = ports.ports();
                self.resources.coex.disable(&mut timer)
            }
            1 => {
                self.coex.bluetooth_started.signal(false);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Whether coexistence is active for `radio`, as `coex_status_get` with
    /// that radio's bitmap answers it: coexistence started (a second radio
    /// enabled it) and another radio publishes schedule status.
    pub fn coex_active_for(&self, radio: CoexStatusType) -> bool {
        self.resources.coex_clients >= 2
            && self
                .resources
                .schedule
                .schedule()
                .status()
                .others_publish(radio)
    }

    /// Enter IEEE 802.15.4 into coexistence, as ESP-IDF's
    /// `esp_coex_wifi_i154_enable` does: enable coexistence for the radio
    /// ([`Self::enable_coex`]) and publish its schedule status, the bit
    /// `esp_coex_ieee802154_status_enable` sets.
    pub fn enable_ieee802154_coex(&mut self) {
        self.enable_coex();
        self.set_coex_status_bits(CoexStatusType::Ieee802154, IEEE802154_COEX_STATUS);
    }

    /// Withdraw IEEE 802.15.4's schedule status, the bit
    /// `esp_coex_ieee802154_status_disable` clears, and disable coexistence
    /// for the radio ([`Self::disable_coex`]).
    ///
    /// # Errors
    ///
    /// As [`Self::disable_coex`].
    pub fn disable_ieee802154_coex(&mut self) -> Result<(), CoexError> {
        self.clear_coex_status_bits(CoexStatusType::Ieee802154, IEEE802154_COEX_STATUS);
        self.disable_coex()
    }

    /// Start external coexistence, as ESP-IDF's
    /// `esp_enable_extern_coex_gpio_pin` does after the platform routed the
    /// external signals: the arbiter programs the coexistence module clock,
    /// the work mode, grant and priorities and enables the block; then
    /// coexistence is enabled for the external radio and its schedule status
    /// bits 1 and 2 are published, as `esp_coex_external_set` does.
    ///
    /// # Errors
    ///
    /// External coexistence already runs, or the module clock failed; nothing
    /// changed.
    pub fn start_external_coex(
        &mut self,
        config: ExternalCoexConfig,
    ) -> Result<(), ExternalCoexError> {
        let (lease, _, clocks) = self.parts();
        lease.start_external_coex(config, clocks)?;
        self.enable_coex();
        self.set_coex_status_bits(CoexStatusType::ExternalCoex, EXTERNAL_COEX_STATUS);
        Ok(())
    }

    /// Stop external coexistence, as `esp_coex_external_stop` does: withdraw
    /// the schedule status, disable coexistence for the external radio, then
    /// clear the priorities, disable the block and release its module clock.
    /// The caller releases the routed signals afterwards.
    ///
    /// # Errors
    ///
    /// External coexistence does not run, a coexistence timer could not be
    /// withdrawn, or releasing the module clock failed.
    pub fn stop_external_coex(&mut self) -> Result<(), ExternalCoexStopError> {
        if self.lease.external_coex().is_none() {
            return Err(ExternalCoexStopError::External(
                ExternalCoexError::NotActive,
            ));
        }
        self.clear_coex_status_bits(CoexStatusType::ExternalCoex, EXTERNAL_COEX_STATUS);
        self.disable_coex().map_err(ExternalCoexStopError::Coex)?;
        let (lease, _, clocks) = self.parts();
        lease
            .stop_external_coex(clocks)
            .map_err(ExternalCoexStopError::External)
    }

    /// Record the Wi-Fi channel, as `coex_wifi_channel_set` does, and wake
    /// Bluetooth's channel wait.
    pub fn set_coex_wifi_channel(&mut self, channel: CoexWifiChannel) {
        self.resources.wifi_channel = channel;
        self.coex.wifi_channel.signal(channel);
    }

    /// The Wi-Fi channel last recorded, as `coex_wifi_channel_get` reads it.
    pub fn coex_wifi_channel(&self) -> CoexWifiChannel {
        self.resources.wifi_channel
    }

    /// Report the end of a Bluetooth preemption `remaining_micros` from now,
    /// or `None` without an end time, as `coex_iso_end_int_handle` does; it
    /// wakes Wi-Fi's preemption-end wait.
    pub fn end_bluetooth_preemption(&mut self, remaining_micros: Option<u32>) {
        let end = match remaining_micros {
            Some(micros) => CoexPreemptionEnd::At(
                Instant::now() + embassy_time::Duration::from_micros(u64::from(micros)),
            ),
            None => CoexPreemptionEnd::Unknown,
        };
        self.coex.preemption_end.signal(end);
    }

    /// Program a Wi-Fi coexistence request, as `coex_wifi_request` does,
    /// on the event's timer with the arbiter's current event priority.
    ///
    /// # Errors
    ///
    /// Requests are disabled, the event has no policy timer, a failed request
    /// awaits cleanup, or the clock or a timer write failed.
    pub fn request_wifi_coex(
        &mut self,
        request: CoexClientRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        let ports = CoexArbiterPorts::new(&mut self.lease);
        let (mut timer, mut clock) = ports.ports();
        self.resources
            .coex
            .request_wifi(&mut timer, &mut clock, request)
    }

    /// Program a Bluetooth coexistence request, as `coex_bt_request` does.
    ///
    /// # Errors
    ///
    /// As [`Self::request_wifi_coex`].
    pub fn request_bluetooth_coex(
        &mut self,
        request: CoexClientRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        let ports = CoexArbiterPorts::new(&mut self.lease);
        let (mut timer, mut clock) = ports.ports();
        self.resources
            .coex
            .request_bluetooth(&mut timer, &mut clock, request)
    }

    /// Withdraw the request of `event` by disabling its timer, as
    /// `coex_wifi_release` and `coex_bt_release` do.
    ///
    /// # Errors
    ///
    /// The event has no policy timer, or the timer write failed.
    pub fn release_coex(&mut self, event: CoexEventId) -> Result<CoexTimerIndex, CoexError> {
        let ports = CoexArbiterPorts::new(&mut self.lease);
        let (mut timer, _) = ports.ports();
        self.resources.coex.release(&mut timer, event)
    }

    /// The coexistence schedule: the radios' status, the selected scheme
    /// and the current phase.
    pub fn coex_schedule(&self) -> &CoexSchedule {
        self.resources.schedule.schedule()
    }

    /// Publish coexistence status bits of one radio, as
    /// `coex_schm_status_bit_set` does. When the change restarts the
    /// phases, the first phase is published and the phase timer programmed.
    pub fn set_coex_status_bits(&mut self, kind: CoexStatusType, bits: u16) {
        if let Some(change) = self.resources.schedule.set_status_bits(kind, bits) {
            self.coex.publish(change);
        }
    }

    /// Withdraw coexistence status bits, as `coex_schm_status_bit_clear`
    /// does. The phase and its timer stay as they are.
    pub fn clear_coex_status_bits(&mut self, kind: CoexStatusType, bits: u16) {
        self.resources.schedule.clear_status_bits(kind, bits);
    }

    /// Set the schedule interval, as `coex_schm_interval_set` does; Wi-Fi
    /// sets it from its beacon interval. It takes effect at the next phase.
    pub fn set_coex_interval(&mut self, interval: u32) {
        self.resources.schedule.set_interval(interval);
    }

    /// Set the flexible period, as `coex_schm_flexible_period_set` does.
    pub fn set_coex_flexible_period(&mut self, period: u8) {
        self.resources.schedule.set_flexible_period(period);
    }

    /// Begin the phases again at phase 0, as `coex_schm_process_restart`
    /// does; a connected Wi-Fi restarts them at each beacon.
    pub fn restart_coex_phases(&mut self) {
        let change = self.resources.schedule.restart();
        self.coex.publish(change);
    }

    /// Step the schedule for an expiry of the phase timer of `generation`.
    fn expire_coex_phase(&mut self, generation: u32) {
        if let CoexExpiry::Changed(change) = self.resources.schedule.expire(generation) {
            self.coex.publish(change);
        }
    }

    /// Put Wi-Fi's RF to sleep, as the vendor modem sleep does through
    /// `wifi_rf_phy_disable`: leave the PHY client set, keeping the
    /// registration and calibration, then close RF when no client remains.
    ///
    /// The caller must have stopped its MAC use of RF and cleared the Wi-Fi
    /// receive path first.
    ///
    /// # Errors
    ///
    /// The domain rejected the release; nothing changed and the membership
    /// is returned. An RF close failure after the release is reported in
    /// [`WifiAsleep::rf_closed`].
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn suspend_wifi(
        &mut self,
        clocks: &WifiClocksOn,
        membership: WifiPhyMembership,
    ) -> Result<WifiAsleep, WifiPhyLeaveFailure> {
        let (suspended, last) = suspend_wifi(self.lease(), clocks, membership)?;
        let rf_closed = if last {
            self.close_phy_if_idle().await
        } else {
            Ok(false)
        };
        Ok(WifiAsleep {
            suspended,
            rf_closed,
        })
    }

    /// Wake Wi-Fi's RF, as the vendor modem wake does through
    /// `wifi_rf_phy_enable`: wake closed RF, then re-enter the PHY client
    /// set. A returned [`ConcurrentAcquire::TrackingDue`] means the domain
    /// must run its tracking before the MAC uses RF.
    ///
    /// # Errors
    ///
    /// Preparing the shared PHY failed, or the domain rejected the client;
    /// the suspended client is returned.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn resume_wifi(
        &mut self,
        clocks: &WifiClocksOn,
        suspended: WifiPhySuspended,
    ) -> Result<(WifiPhyMembership, ConcurrentAcquire), WifiWakeFailure> {
        if let Err(error) = self.prepare_phy().await {
            return Err(WifiWakeFailure {
                error: WifiWakeError::Phy(error),
                suspended,
            });
        }
        resume_wifi(self.lease(), clocks, suspended, &mut EmbassyPhyTime).map_err(|failure| {
            WifiWakeFailure {
                error: WifiWakeError::Client(failure.error()),
                suspended: failure.into_suspended(),
            }
        })
    }
}

/// Debug trace: the last value and a ring of the last sixteen.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn runtime_trace(value: u32) {
    use core::sync::atomic::{AtomicU32, Ordering::SeqCst};
    #[unsafe(link_section = ".rtc_fast.persistent")]
    #[unsafe(no_mangle)]
    static OER_RUNTIME_TRACE: AtomicU32 = AtomicU32::new(0);
    #[unsafe(link_section = ".rtc_fast.persistent")]
    #[unsafe(no_mangle)]
    static OER_TRACE_RING: [AtomicU32; 65] = [const { AtomicU32::new(0) }; 65];
    OER_RUNTIME_TRACE.store(value, SeqCst);
    if value == 0x6104 {
        TRANSMIT_ARMED.store(true, SeqCst);
    }
    let index = OER_TRACE_RING[64].load(SeqCst) % 64;
    OER_TRACE_RING[index as usize].store(value, SeqCst);
    OER_TRACE_RING[64].store(index + 1, SeqCst);
}

static TRANSMIT_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Debug: park at the first transmission after each 802.15.4 start.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn oer_debug_before_transmit() {
    if TRANSMIT_ARMED.swap(false, core::sync::atomic::Ordering::SeqCst) {
        runtime_trace(0x7a7a);
        snapshot();
        oer_debug_park();
    }
}

/// Debug: an openocd hardware breakpoint target.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn oer_debug_park() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    core::hint::black_box(());
}

const SNAPSHOT_RANGES: [(usize, usize); 8] = [
    (0x2010_0000, 1024),
    (0x2010_2000, 16),
    (0x2010_2c00, 320),
    (0x2010_9c00, 64),
    (0x2010_f000, 64),
    (0x2010_f400, 64),
    (0x2010_f800, 16),
    (0x2070_4000, 128),
];
const SNAPSHOT_BLOCKS: [u8; 9] = [0x61, 0x62, 0x63, 0x66, 0x67, 0x69, 0x6a, 0x6b, 0x6d];
const SNAPSHOT_IMAGE: usize = 209;
const SNAPSHOT_WORDS: usize = 4 + SNAPSHOT_IMAGE + SNAPSHOT_BLOCKS.len() * 16 + 2000;

#[allow(unsafe_code)]
#[unsafe(link_section = ".rtc_fast.persistent")]
#[unsafe(no_mangle)]
static mut OER_SNAPSHOT: [[u32; SNAPSHOT_WORDS]; 3] = [[0; SNAPSHOT_WORDS]; 3];
#[allow(unsafe_code)]
#[unsafe(link_section = ".rtc_fast.persistent")]
#[unsafe(no_mangle)]
static OER_SNAPSHOT_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[allow(unsafe_code)]
fn snapshot() {
    use core::sync::atomic::Ordering::SeqCst;
    let seq = OER_SNAPSHOT_SEQ.load(SeqCst).wrapping_add(1);
    OER_SNAPSHOT_SEQ.store(seq, SeqCst);
    let slot = unsafe { &mut (*(&raw mut OER_SNAPSHOT))[(seq % 3) as usize] };
    slot[0] = 0x5eed_0000;
    slot[1] = seq;
    let mut registers = oer_esp32s31_pac::validation::shared_radio_registers();
    let phy = registers.radio_phy_mut();
    let mut at = 4;
    for index in 0..SNAPSHOT_IMAGE {
        slot[at] = if matches!(index, 118 | 175 | 176 | 177) {
            0xdead_0000
        } else {
            phy.register_image(index).unwrap_or(0xdead_beef)
        };
        at += 1;
    }
    for block in SNAPSHOT_BLOCKS {
        for register in 0..64u8 {
            let address = oer_esp32s31_pac::phy::i2c::PhyI2cAddress::debug(block, register);
            let mut polls = 0;
            while phy.try_start_phy_i2c_read(address).is_err() && polls < 10_000 {
                polls += 1;
            }
            let value = loop {
                match phy.try_finish_phy_i2c_read(address) {
                    Ok(value) => break value,
                    Err(_) if polls < 20_000 => polls += 1,
                    Err(_) => break 0xee,
                }
            };
            let word = at + usize::from(register / 4);
            slot[word] = (slot[word] & !(0xff << (8 * (register % 4)))) | (u32::from(value) << (8 * (register % 4)));
        }
        at += 16;
    }
    for (base, words) in SNAPSHOT_RANGES {
        for word in 0..words {
            slot[at] = unsafe { core::ptr::read_volatile((base + 4 * word) as *const u32) };
            at += 1;
        }
    }
    slot[2] = at as u32;
    slot[3] = 0x5eed_ffff;
}

/// Debug: trace PHY_BASEBAND_CONFIG_ORACLE.I2C_TX_RATE_CONTROL.
#[allow(unsafe_code)]
pub fn trace_rate() {
    runtime_trace(unsafe { core::ptr::read_volatile(0x2010_448c as *const u32) });
}
