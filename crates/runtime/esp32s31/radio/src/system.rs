//! The arbiter, its shared resources and the periodic tracking timer.

use core::{future::Future, task::Poll};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    mutex::{Mutex, MutexGuard},
};

use embassy_time::Timer;
use oer_esp32s31_hal::{
    root::{ConcurrentPartitions, RadioHardware},
    shared_radio::{PlatformClockProvider, SharedRadio, SharedRadioLease},
};
use oer_esp32s31_phy::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyRegistration, ConcurrentPhyTrackingError,
    ConcurrentRfError, ConcurrentTrackingTick, NoopPhyTargetObserver, PhyCalibrationCache,
    PhyCalibrationIdentity, PhyRegisterConfig, close_concurrent_rf,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    ieee802154_client::{
        Ieee802154PhyClientError, RegisteredIeee802154OperationalRoute,
        RegisteredIeee802154RouteFailure, RegisteredIeee802154SuspendedRoute,
    },
    register_concurrent_phy,
    state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
    track_concurrent_phy, wake_concurrent_rf,
};
use oer_esp32s31_phy_runtime::EmbassyPhyTime;

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
}

/// The shared radio: the arbiter with its PHY domain and the platform
/// resources, taken together through [`Self::lock`].
pub struct RadioSystem<P, C> {
    radio: SharedRadio<ConcurrentPhy>,
    /// Taken only while the arbiter lease is held, so taking it never waits.
    resources: Mutex<CriticalSectionRawMutex, RadioResources<P, C>>,
    identity: PhyCalibrationIdentity,
}

/// The arbiter lease together with the platform resources.
///
/// Dropping it releases both. Hold it only for one radio transaction.
pub struct RadioGuard<'radio, P, C> {
    // Declared first so it drops before the lease that protects it.
    resources: MutexGuard<'radio, CriticalSectionRawMutex, RadioResources<P, C>>,
    lease: SharedRadioLease<'radio, ConcurrentPhy>,
    identity: PhyCalibrationIdentity,
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

/// IEEE 802.15.4 asleep after [`RadioGuard::suspend_ieee802154`].
#[must_use = "the sleeping route must wake or reunite with its MAC owners"]
pub struct Ieee802154Asleep {
    /// The operational MAC route without a PHY client bit.
    pub route: RegisteredIeee802154SuspendedRoute,
    /// Whether RF closed because IEEE 802.15.4 was the last client, or why
    /// closing it failed; a [`ConcurrentRfError::Recoverable`] failure keeps
    /// RF open, any other started failure requires reset.
    pub rf_closed: Result<bool, ConcurrentRfError>,
}

/// Why a sleeping IEEE 802.15.4 route could not wake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154WakeError {
    /// The shared PHY could not be prepared ([`RadioPhyError::started`]).
    Phy(RadioPhyError),
    /// The PHY domain rejected the client; nothing changed.
    Client(Ieee802154PhyClientError),
}

/// Failed wake retaining the sleeping route.
#[must_use = "a failed wake still owns the sleeping route"]
pub struct Ieee802154WakeFailure {
    /// Why the route did not wake.
    pub error: Ieee802154WakeError,
    /// The unchanged sleeping route.
    pub route: RegisteredIeee802154SuspendedRoute,
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
                }),
                identity,
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

    /// Put IEEE 802.15.4 to sleep, as the vendor `ieee802154_sleep` does
    /// through `ieee802154_rf_disable`: leave the PHY client set while
    /// keeping the BTBB reference, then close RF when no client remains.
    ///
    /// The caller must have stopped the current MAC operation first.
    ///
    /// # Errors
    ///
    /// The domain rejected the release; nothing changed and the operational
    /// route is returned. An RF close failure after the release is reported
    /// in [`Ieee802154Asleep::rf_closed`].
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn suspend_ieee802154(
        &mut self,
        route: RegisteredIeee802154OperationalRoute,
    ) -> Result<
        Ieee802154Asleep,
        RegisteredIeee802154RouteFailure<RegisteredIeee802154OperationalRoute>,
    > {
        let (route, last) = route.suspend_rf(self.lease())?;
        let rf_closed = if last {
            self.close_phy_if_idle().await
        } else {
            Ok(false)
        };
        Ok(Ieee802154Asleep { route, rf_closed })
    }

    /// Wake IEEE 802.15.4 before its next MAC command, as the vendor
    /// `ieee802154_rf_enable` does: wake closed RF, then re-enter the PHY
    /// client set. A returned [`ConcurrentAcquire::TrackingDue`] means the
    /// domain must run its tracking before the MAC uses RF.
    ///
    /// # Errors
    ///
    /// Preparing the shared PHY failed, or the domain rejected the client;
    /// the sleeping route is returned.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn resume_ieee802154(
        &mut self,
        route: RegisteredIeee802154SuspendedRoute,
    ) -> Result<(RegisteredIeee802154OperationalRoute, ConcurrentAcquire), Ieee802154WakeFailure>
    {
        if let Err(error) = self.prepare_phy().await {
            return Err(Ieee802154WakeFailure {
                error: Ieee802154WakeError::Phy(error),
                route,
            });
        }
        route
            .resume_rf(self.lease(), &mut EmbassyPhyTime)
            .map_err(|failure| Ieee802154WakeFailure {
                error: Ieee802154WakeError::Client(failure.error()),
                route: failure.into_route(),
            })
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
}
