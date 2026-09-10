//! Event-driven control for the connected Wi-Fi maintenance service.
use super::PauseError;
use core::cell::RefCell;
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};
use oer_esp32s31_phy::tracking::{
    maintenance::Operation,
    service::{Config, Suspension},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Disabled,
    Waiting {
        deadline_micros: u64,
    },
    Pending(Operation),
    Deferred(Operation),
    Suspended(Suspension),
    Completed {
        operation: Operation,
        elapsed_micros: u64,
    },
    Failed(PauseError),
}

/// Accumulated completed physical operations in the current service window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub operations: [u16; 6],
    pub deferred: u16,
    pub common_calibrated: u16,
    pub wifi_calibrated: u16,
    pub pause_micros: u64,
    pub maximum_pause_micros: u64,
    pub failed: bool,
    pub suspended: bool,
    pub invalid: bool,
}

struct State {
    report: Report,
    config: Option<Config>,
    observation_requested: bool,
    status: Status,
}
pub(crate) struct Control {
    state: Mutex<CriticalSectionRawMutex, RefCell<State>>,
    changed: Signal<CriticalSectionRawMutex, ()>,
}
impl Control {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(State {
                report: Report {
                    operations: [0; 6],
                    deferred: 0,
                    common_calibrated: 0,
                    wifi_calibrated: 0,
                    pause_micros: 0,
                    maximum_pause_micros: 0,
                    failed: false,
                    suspended: false,
                    invalid: false,
                },
                config: None,
                observation_requested: false,
                status: Status::Disabled,
            })),
            changed: Signal::new(),
        }
    }
    pub fn configure(&self, config: Option<Config>) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.config = config;
            if config.is_some() {
                state.report = Report::default();
            }
            state.observation_requested = config.is_some();
            // Disable stops future selection, not an admitted operation. Keep
            // Pending until completion so disable/re-enable cannot reset the
            // report or replace the configuration underneath that operation.
            if config.is_some() || !matches!(state.status, Status::Failed(_) | Status::Pending(_)) {
                state.status = Status::Disabled;
            }
        });
        self.changed.signal(());
    }
    pub fn notify(&self) {
        self.state
            .lock(|state| state.borrow_mut().observation_requested = true);
        self.changed.signal(());
    }
    /// The supervisor has ended this connected epoch; no selected operation
    /// can subsequently publish a completion into a new one.
    pub fn end_epoch(&self) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.config = None;
            state.observation_requested = false;
            if !matches!(state.status, Status::Failed(_)) {
                state.status = Status::Disabled;
            }
        });
        self.changed.signal(());
    }
    pub fn snapshot(&self) -> (Option<Config>, bool) {
        self.state.lock(|state| {
            let state = state.borrow();
            (state.config, state.observation_requested)
        })
    }
    pub fn try_begin(&self, config: Config, operation: Operation) -> bool {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            // Inspection borrows the PHY owner outside this lock. A caller
            // on the other core may disable or replace configuration meanwhile.
            if state.config != Some(config) || matches!(state.status, Status::Pending(_)) {
                return false;
            }
            if operation == Operation::Temperature {
                state.observation_requested = false;
            }
            state.status = Status::Pending(operation);
            true
        })
    }
    pub fn status(&self) -> Status {
        self.state.lock(|state| state.borrow().status)
    }
    pub fn report(&self, status: Status) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.config.is_none()
                && matches!(status, Status::Waiting { .. } | Status::Suspended(_))
            {
                return;
            }
            if matches!(status, Status::Failed(_)) {
                // Failure requires an explicit new configuration, not a
                // repeated automatic attempt against the same frontier.
                state.config = None;
                state.observation_requested = false;
            }
            state.status = status;
            state.report.failed |= matches!(status, Status::Failed(_));
            state.report.suspended |= matches!(status, Status::Suspended(_));
        });
    }
    pub fn measurements(&self) -> Report {
        self.state.lock(|state| state.borrow().report)
    }
    pub fn completed(
        &self,
        operation: Operation,
        elapsed_micros: u64,
        outcome: Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
    ) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let report = &mut state.report;
            if let Some(total) = report.pause_micros.checked_add(elapsed_micros) {
                report.pause_micros = total;
            } else {
                report.invalid = true;
            }
            report.maximum_pause_micros = report.maximum_pause_micros.max(elapsed_micros);
            let Some(outcome) = outcome else {
                if let Some(count) = report.deferred.checked_add(1) {
                    report.deferred = count;
                } else {
                    report.invalid = true;
                }
                state.status = Status::Deferred(operation);
                return;
            };
            let index = match operation {
                Operation::Temperature => 0,
                Operation::WifiPower => 1,
                Operation::WifiI2c => 2,
                Operation::CommonCalibration => 3,
                Operation::WifiTxCalibration => 4,
                Operation::Rfpll => 5,
            };
            if let Some(count) = report.operations[index].checked_add(1) {
                report.operations[index] = count;
            } else {
                report.invalid = true;
            }
            {
                for (counter, committed) in [
                    (&mut report.common_calibrated, outcome.calibration.common),
                    (&mut report.wifi_calibrated, outcome.calibration.wifi),
                ] {
                    if committed {
                        if let Some(next) = counter.checked_add(1) {
                            *counter = next;
                        } else {
                            report.invalid = true;
                        }
                    }
                }
            }
            state.status = Status::Completed {
                operation,
                elapsed_micros,
            };
        });
    }
    pub async fn changed(&self) {
        self.changed.wait().await;
    }
}
