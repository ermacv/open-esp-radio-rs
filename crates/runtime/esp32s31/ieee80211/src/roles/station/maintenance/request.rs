//! Same-connection maintenance requests; no shared RF grant.
//!
//! One requester at a time may ask the connected-station supervisor for a
//! physical pause round trip. The supervisor opens availability for one
//! connected epoch, waits for explicit or automatic work, and completes
//! every admitted request exactly once.

use super::{
    automatic::{self, Report as TrackingReport, Status as TrackingStatus},
    policy::PauseError,
    timeline::PauseTimeline,
};
use core::cell::RefCell;
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    mutex::Mutex as AsyncMutex,
    signal::Signal,
};
use oer_esp32s31_phy::tracking::service::Config as TrackingConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseOperation {
    Access,
    /// Diagnostic absence, with no PHY algorithm. PM notification is optional
    /// for the matched control. This is not an automatic maintenance policy.
    Synthetic {
        duration_micros: u32,
        notify_ap: bool,
    },
    Tracking,
    Calibration,
    CommonCalibration,
    TxCalibration,
    Rfpll {
        maximum_age_micros: u64,
    },
    Operation(oer_esp32s31_phy::tracking::maintenance::Operation),
    /// Recheck the age of a completed sensor acquisition after RF admission.
    ObservedOperation {
        operation: oer_esp32s31_phy::tracking::maintenance::Operation,
        maximum_age_micros: u64,
    },
    Automatic(oer_esp32s31_phy::tracking::maintenance::Operation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PauseReport {
    /// Detailed observations exist only in diagnostic firmware. Keeping the
    /// payload out of the type removes it from the request signal and normal
    /// runtime futures when diagnostics are disabled.
    #[cfg(feature = "diagnostics")]
    pub timings: Option<oer_esp32s31_phy::tracking::observation::Report>,
    /// Complete composition handoffs, including TX drain and worker release.
    #[cfg(feature = "diagnostics")]
    pub timeline: Option<PauseTimeline>,
    /// Stop/resume plus optional PM exchanges; excludes requester queuing and prior TX drain.
    pub elapsed_micros: u64,
    pub tracking: Option<oer_esp32s31_phy::tracking::PhyParamTrackingOutcome>,
}

impl PauseReport {
    /// Return the full physical transaction timeline in diagnostic firmware.
    pub const fn timeline(&self) -> Option<&PauseTimeline> {
        #[cfg(feature = "diagnostics")]
        {
            self.timeline.as_ref()
        }
        #[cfg(not(feature = "diagnostics"))]
        {
            None
        }
    }

    /// Return detailed timing observations when this firmware includes them.
    ///
    /// Keeping this accessor available in every feature profile lets HIL and
    /// other generic consumers use one API without retaining the diagnostic
    /// payload in production request storage.
    pub const fn timings(&self) -> Option<&oer_esp32s31_phy::tracking::observation::Report> {
        #[cfg(feature = "diagnostics")]
        {
            self.timings.as_ref()
        }
        #[cfg(not(feature = "diagnostics"))]
        {
            None
        }
    }

    /// Discard observations when a caller replaces the measured interval with
    /// a wider aggregate interval. This is a no-op in compact production
    /// builds, where the diagnostic payload does not exist.
    pub fn discard_timings(&mut self) {
        #[cfg(feature = "diagnostics")]
        {
            self.timings = None;
            self.timeline = None;
        }
    }
}

struct State {
    available: bool,
    pending: bool,
}

pub struct Requests {
    pub automatic: automatic::Control,
    state: Mutex<CriticalSectionRawMutex, RefCell<State>>,
    client: AsyncMutex<CriticalSectionRawMutex, ()>,
    requested: Signal<CriticalSectionRawMutex, PauseOperation>,
    completed: Signal<CriticalSectionRawMutex, Result<PauseReport, PauseError>>,
}

impl Requests {
    pub const fn new() -> Self {
        Self {
            automatic: automatic::Control::new(),
            state: Mutex::new(RefCell::new(State {
                available: false,
                pending: false,
            })),
            client: AsyncMutex::new(()),
            requested: Signal::new(),
            completed: Signal::new(),
        }
    }

    pub async fn request(&self, operation: PauseOperation) -> Result<PauseReport, PauseError> {
        if matches!(operation, PauseOperation::Synthetic { duration_micros, .. } if duration_micros == 0 || duration_micros > 200_000)
        {
            return Err(PauseError::InvalidDuration);
        }
        let _client = self.client.try_lock().map_err(|_| PauseError::Busy)?;
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !state.available {
                return Err(PauseError::Unavailable);
            }
            if state.pending {
                return Err(PauseError::Busy);
            }
            self.completed.reset();
            state.pending = true;
            self.requested.signal(operation);
            Ok(())
        })?;
        // Cancellation releases the client lock but leaves the in-flight
        // request pending until the supervisor completes it or closes the epoch.
        self.completed.wait().await
    }

    pub fn open(&self, tracking: Option<TrackingConfig>) -> Availability<'_> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            assert!(!state.available && !state.pending);
            state.available = true;
        });
        self.automatic.configure(tracking);
        Availability(self)
    }

    pub async fn wait(&self) -> PauseOperation {
        self.requested.wait().await
    }

    /// Explicit work wins when both sources are ready. The automatic future
    /// may reserve service state when polled, so the losing source must not
    /// be polled after an explicit request has been accepted.
    pub async fn wait_next<T>(
        &self,
        automatic: impl core::future::Future<Output = T>,
    ) -> embassy_futures::select::Either<PauseOperation, T> {
        embassy_futures::select::select(self.wait(), automatic).await
    }

    /// Enable or disable observation-driven tracking for the open epoch.
    pub fn configure_tracking(&self, config: Option<TrackingConfig>) -> Result<(), PauseError> {
        self.state.lock(|state| {
            if !state.borrow().available {
                return Err(PauseError::Unavailable);
            }
            if config.is_some() && matches!(self.automatic.status(), TrackingStatus::Pending(_)) {
                return Err(PauseError::Busy);
            }
            self.automatic.configure(config);
            Ok(())
        })
    }

    /// Ask the enabled service to acquire a new PHY sensor observation.
    pub fn request_temperature_observation(&self) -> Result<(), PauseError> {
        self.state.lock(|state| {
            if !state.borrow().available {
                return Err(PauseError::Unavailable);
            }
            self.automatic.notify();
            Ok(())
        })
    }

    pub fn tracking_status(&self) -> TrackingStatus {
        self.automatic.status()
    }

    pub fn tracking_report(&self) -> TrackingReport {
        self.automatic.measurements()
    }

    pub fn finish(&self, result: Result<PauseReport, PauseError>) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.pending {
                state.pending = false;
                self.completed.signal(result);
            }
        });
    }
}

/// Open availability for one connected epoch; dropping it closes the epoch.
pub struct Availability<'a>(&'a Requests);
impl Drop for Availability<'_> {
    fn drop(&mut self) {
        self.0.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.available = false;
            self.0.automatic.end_epoch();
            self.0.requested.reset();
            if state.pending {
                state.pending = false;
                self.0.completed.signal(Err(PauseError::Interrupted));
            }
        });
    }
}

impl Default for Requests {
    fn default() -> Self {
        Self::new()
    }
}
