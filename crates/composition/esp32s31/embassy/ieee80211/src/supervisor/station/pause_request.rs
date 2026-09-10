#[path = "pause_request/automatic.rs"]
mod automatic;
pub use automatic::{Report as TrackingReport, Status as TrackingStatus};
pub use oer_esp32s31_phy::tracking::service::Config as TrackingConfig;
// Same-connection maintenance requests; no shared RF grant.
use core::cell::RefCell;
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    mutex::Mutex as AsyncMutex,
    signal::Signal,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseError {
    Unavailable,
    Busy,
    Interrupted,
    MacStop,
    RxBusy,
    RxPause,
    IrqPause,
    RxResume,
    IrqResume,
    RegisterReclaim,
    PhyAdmission,
    PhyRelease,
    RegisterRepublish,
    PhyTracking,
    MacRestoration,
    ReceivePolicyChanged,
    PeerNotification,
    InvalidDuration,
}

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
    pub timings: Option<oer_esp32s31_phy::tracking::observation::Report>,
    /// Stop/resume plus optional PM exchanges; excludes requester queuing and prior TX drain.
    pub elapsed_micros: u64,
    pub tracking: Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
}

struct State {
    available: bool,
    pending: bool,
}

pub(super) struct Requests {
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

    pub fn open(&self) -> Availability<'_> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            assert!(!state.available && !state.pending);
            state.available = true;
        });
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

pub(super) struct Availability<'a>(&'a Requests);
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

pub(super) static REQUESTS: Requests = Requests::new();

/// Stop and resume the standalone connected datapath without reassociation.
/// Access only checks the physical handoff. Tracking also executes currently due
/// PHY work; a missing tracking outcome means no work was due. Neither operation
/// grants shared RF access. Only one request may run in a connected standalone STA.
pub async fn station_pause_round_trip(
    operation: PauseOperation,
) -> Result<PauseReport, PauseError> {
    REQUESTS.request(operation).await
}

/// Enable or disable observation-driven tracking for this connected STA epoch.
/// Each selected operation still performs the full physical pause/restoration.
/// No configuration survives leaving the connected role. None is the default.
pub fn configure_station_tracking(config: Option<TrackingConfig>) -> Result<(), PauseError> {
    REQUESTS.state.lock(|state| {
        if !state.borrow().available {
            return Err(PauseError::Unavailable);
        }
        if config.is_some() && matches!(REQUESTS.automatic.status(), TrackingStatus::Pending(_)) {
            return Err(PauseError::Busy);
        }
        REQUESTS.automatic.configure(config);
        Ok(())
    })
}

/// Ask the enabled service to acquire a new PHY sensor observation. This is
/// an event, not a supplied temperature, calibration completion or RF grant.
pub fn request_station_temperature_observation() -> Result<(), PauseError> {
    REQUESTS.state.lock(|state| {
        if !state.borrow().available {
            return Err(PauseError::Unavailable);
        }
        REQUESTS.automatic.notify();
        Ok(())
    })
}

pub fn station_tracking_status() -> TrackingStatus {
    REQUESTS.automatic.status()
}

pub fn station_tracking_report() -> TrackingReport {
    REQUESTS.automatic.measurements()
}
