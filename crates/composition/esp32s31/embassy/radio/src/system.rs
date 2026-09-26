//! The arbiter, its shared resources and the periodic tracking timer.

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
    ConcurrentPhyRegisterFailure, ConcurrentPhyTrackingError, ConcurrentRfError,
    ConcurrentTrackingTick, NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig,
    close_concurrent_rf,
    concurrent::{ConcurrentPhy, ConcurrentPhyError},
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
                resources: Mutex::new(RadioResources { platform, clocks }),
                identity,
            },
            partitions,
        )
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
        mut observe: impl FnMut(&Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>),
    ) -> ConcurrentPhyTrackingError {
        loop {
            Timer::after_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS).await;
            let tick = self.track().await;
            observe(&tick);
            match tick {
                Ok(ConcurrentTrackingTick::Unavailable(ConcurrentPhyError::Poisoned)) => {
                    return ConcurrentPhyTrackingError::Rejected(ConcurrentPhyError::Poisoned);
                }
                Ok(_) => {}
                Err(error) => return error,
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
        let RadioResources { platform, clocks } = &mut *self.resources;
        (&mut self.lease, platform, clocks)
    }

    /// The first-client half of `esp_phy_enable`: register the shared PHY
    /// domain once, or wake its closed RF. A registered, open domain is left
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Registration or wake was rejected before any register access, or
    /// started and failed ([`RadioPhyError::started`]).
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result.
    pub async fn prepare_phy(&mut self) -> Result<(), RadioPhyError> {
        let identity = self.identity;
        let (lease, platform, clocks) = self.parts();
        if lease.attachment().rf_closed() {
            return wake_concurrent_rf::<EmbassyPhyTime>(lease, clocks)
                .await
                .map_err(RadioPhyError::Wake);
        }
        if lease.attachment().client_snapshot().is_some() {
            return Ok(());
        }
        match register_concurrent_phy::<P, EmbassyPhyTime, _>(
            lease,
            platform,
            clocks,
            PhyRegisterConfig::new(identity),
            NoopPhyTargetObserver,
        )
        .await
        {
            Ok(_registration) => Ok(()),
            Err(ConcurrentPhyRegisterFailure::Rejected(ConcurrentPhyError::AlreadyRegistered)) => {
                Ok(())
            }
            Err(ConcurrentPhyRegisterFailure::Rejected(error)) => {
                Err(RadioPhyError::Registration(error))
            }
            Err(_) => Err(RadioPhyError::RegistrationFailed),
        }
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
