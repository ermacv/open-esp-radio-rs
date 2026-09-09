//! One explicit same-connection access or due-tracking request; no shared RF grant.
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseOperation {
    Access,
    Tracking,
    Calibration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PauseReport {
    pub timings: Option<oer_esp32s31_phy::tracking::observation::Report>,
    /// Physical stop/resume; excludes requester queuing.
    pub elapsed_micros: u64,
    pub tracking: Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
}

struct State {
    available: bool,
    pending: bool,
}

pub(super) struct Requests {
    state: Mutex<CriticalSectionRawMutex, RefCell<State>>,
    client: AsyncMutex<CriticalSectionRawMutex, ()>,
    requested: Signal<CriticalSectionRawMutex, PauseOperation>,
    completed: Signal<CriticalSectionRawMutex, Result<PauseReport, PauseError>>,
}

impl Requests {
    pub const fn new() -> Self {
        Self {
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
