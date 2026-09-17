//! Bounded observations of the real physical transaction and guarded RUN.
//! Times include preemption and observation overhead; maxima are observations,
//! not WCET or thermal qualification. No observation grants RF authority.

use oer_esp32s31_bluetooth::le::peripheral::maintenance::PeripheralMaintenanceRun;
use oer_esp32s31_phy::tracking::{
    observation::{OPERATION_COUNT, Operation, Timing},
    parameters::PhyParamTrackingOutcome,
};

/// Boot-lifetime measurements; operation timings nest and must not be summed.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaintenanceMeasurements {
    pub transactions: u32,
    pub restored: u32,
    pub common_calibrations: u32,
    /// Quality of the latest completed RX DC product; light tracking retains it.
    pub latest_rx_quality: Option<oer_esp32s31_phy::rx::gain_calibration::PhyRxGainDcQuality>,
    pub bluetooth_calibrations: u32,
    pub maximum_execution_micros: u32,
    pub maximum_restoration_micros: u32,
    pub maximum_to_run_micros: u32,
    pub maximum_poll_micros: u32,
    pub admitted_at_micros: u64,
    pub execution_deadline_micros: Option<u64>,
    pub restoration_deadline_micros: Option<u64>,
    pub physical_finished_at_micros: Option<u64>,
    pub run_at_micros: Option<u64>,
    pub operations: [Timing; OPERATION_COUNT],
    pub invalid: bool,
}

impl MaintenanceMeasurements {
    fn add(value: &mut u32, amount: u32, invalid: &mut bool) {
        if let Some(sum) = value.checked_add(amount) {
            *value = sum;
        } else {
            *invalid = true;
        }
    }
    fn duration(&mut self, start: u64, end: u64) -> Option<u32> {
        let duration = end.checked_sub(start).and_then(|v| u32::try_from(v).ok());
        self.invalid |= duration.is_none();
        duration
    }
    fn begin(&mut self, admitted: u64, execution: Option<u64>, restoration: Option<u64>) {
        // A previous guarded physical return cannot disappear before RUN.
        self.invalid |= self.restoration_deadline_micros.is_some()
            && self.physical_finished_at_micros.is_some()
            && self.run_at_micros.is_none();
        self.admitted_at_micros = admitted;
        self.execution_deadline_micros = execution;
        self.restoration_deadline_micros = restoration;
        self.physical_finished_at_micros = None;
        self.run_at_micros = None;
    }
    fn physical(
        &mut self,
        at: u64,
        outcome: Option<PhyParamTrackingOutcome>,
        report: oer_esp32s31_phy::tracking::observation::Report,
    ) {
        self.invalid |= report.invalid || self.physical_finished_at_micros.is_some();
        if let Some(elapsed) = self.duration(self.admitted_at_micros, at) {
            self.maximum_execution_micros = self.maximum_execution_micros.max(elapsed);
        }
        self.invalid |= self.execution_deadline_micros.is_some_and(|end| at >= end);
        self.physical_finished_at_micros = Some(at);
        if let Some(outcome) = outcome {
            Self::add(&mut self.transactions, 1, &mut self.invalid);
            Self::add(
                &mut self.common_calibrations,
                u32::from(outcome.calibration.common),
                &mut self.invalid,
            );
            Self::add(
                &mut self.bluetooth_calibrations,
                u32::from(outcome.calibration.bluetooth_ieee802154),
                &mut self.invalid,
            );
        }
        if let Some(quality) = report
            .rx_gain_execution
            .and_then(|execution| execution.quality)
        {
            self.latest_rx_quality = Some(quality);
        }
        for (sum, measurement) in self.operations.iter_mut().zip(report.timings) {
            for (value, amount) in [
                (&mut sum.started, measurement.started),
                (&mut sum.completed, measurement.completed),
                (&mut sum.failed, measurement.failed),
            ] {
                if let Some(next) = value.checked_add(amount) {
                    *value = next;
                } else {
                    self.invalid = true;
                }
            }
            Self::add(
                &mut sum.elapsed_micros,
                measurement.elapsed_micros,
                &mut self.invalid,
            );
            sum.maximum_micros = sum.maximum_micros.max(measurement.maximum_micros);
        }
        for operation in [Operation::Dcode, Operation::RxGain, Operation::TxDcPwdet] {
            if let Some(poll) = report.poll_timing(operation) {
                self.maximum_poll_micros = self.maximum_poll_micros.max(poll.maximum_micros);
            }
        }
    }
    fn restored(&mut self, run: PeripheralMaintenanceRun) {
        let Some(physical) = self.physical_finished_at_micros else {
            self.invalid = true;
            return;
        };
        if run.admitted_at_micros != self.admitted_at_micros
            || Some(run.restoration_deadline_micros) != self.restoration_deadline_micros
            || run.run_at_micros >= run.restoration_deadline_micros
            || self.run_at_micros.is_some()
        {
            self.invalid = true;
            return;
        }
        if let Some(elapsed) = self.duration(physical, run.run_at_micros) {
            self.maximum_restoration_micros = self.maximum_restoration_micros.max(elapsed);
        }
        if let Some(elapsed) = self.duration(self.admitted_at_micros, run.run_at_micros) {
            self.maximum_to_run_micros = self.maximum_to_run_micros.max(elapsed);
        }
        self.run_at_micros = Some(run.run_at_micros);
        Self::add(&mut self.restored, 1, &mut self.invalid);
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use super::*;
    use oer_esp32s31_phy::{
        PhyTargetObserver,
        tracking::observation::{Event, Recorder},
    };
    struct State {
        measurements: MaintenanceMeasurements,
        recorder: Recorder,
    }
    static STATE: embassy_sync::blocking_mutex::Mutex<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        core::cell::RefCell<Option<State>>,
    > = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(None));
    #[inline(never)]
    pub(crate) fn begin(admitted: u64, execution: Option<u64>, restoration: Option<u64>) {
        STATE.lock(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot.get_or_insert_with(|| State {
                measurements: Default::default(),
                recorder: Default::default(),
            });
            state.measurements.begin(admitted, execution, restoration);
            state.recorder = Default::default();
        });
    }
    #[inline(never)]
    pub(crate) fn physical(outcome: Option<PhyParamTrackingOutcome>) {
        let now = embassy_time::Instant::now().as_micros();
        STATE.lock(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot.as_mut().expect("maintenance observation started");
            state
                .measurements
                .physical(now, outcome, state.recorder.report());
        });
    }
    pub(crate) fn restored(run: PeripheralMaintenanceRun) {
        STATE.lock(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.measurements.restored(run);
            }
        });
    }
    pub fn snapshot() -> Option<MaintenanceMeasurements> {
        STATE.lock(|slot| slot.borrow().as_ref().map(|state| state.measurements))
    }
    pub(crate) struct Observer;
    impl PhyTargetObserver for Observer {
        fn rx_gain_execution(
            &mut self,
            execution: oer_esp32s31_phy::tracking::observation::RxGainExecution,
        ) {
            STATE.lock(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    state.recorder.observe_rx_gain_execution(execution);
                }
            });
        }
        #[inline(never)]
        fn tracking_operation(&mut self, operation: Operation, event: Event) {
            let now = embassy_time::Instant::now().as_micros();
            STATE.lock(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    state.recorder.observe(operation, event, now);
                }
            });
        }
    }
}
#[cfg(target_arch = "riscv32")]
pub use target::snapshot;
#[cfg(target_arch = "riscv32")]
pub(crate) use target::{Observer, begin, physical, restored};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn light_tracking_retains_the_latest_rx_product_quality() {
        let quality = oer_esp32s31_phy::rx::gain_calibration::PhyRxGainDcQuality::default();
        let mut m = MaintenanceMeasurements::default();
        m.begin(100, None, None);
        let report = oer_esp32s31_phy::tracking::observation::Report {
            rx_gain_execution: Some(oer_esp32s31_phy::tracking::observation::RxGainExecution {
                quality: Some(quality),
                ..Default::default()
            }),
            ..Default::default()
        };
        m.physical(110, None, report);
        assert_eq!(m.latest_rx_quality, Some(quality));
        m.begin(200, None, None);
        m.physical(210, None, Default::default());
        assert_eq!(m.latest_rx_quality, Some(quality));
        assert!(!m.invalid);
    }

    #[test]
    fn physical_return_is_not_run_and_original_window_is_required() {
        let mut m = MaintenanceMeasurements::default();
        m.begin(100, Some(120), Some(130));
        m.physical(110, None, Default::default());
        assert_eq!(m.restored, 0);
        assert_eq!(m.maximum_execution_micros, 10);
        m.restored(PeripheralMaintenanceRun {
            admitted_at_micros: 100,
            run_at_micros: 125,
            restoration_deadline_micros: 130,
            event_counter: 3,
        });
        assert!(!m.invalid);
        assert_eq!(
            (
                m.restored,
                m.maximum_restoration_micros,
                m.maximum_to_run_micros
            ),
            (1, 15, 25)
        );
    }
    #[test]
    fn new_work_cannot_hide_missing_restoration() {
        let mut m = MaintenanceMeasurements::default();
        m.begin(100, Some(120), Some(130));
        m.physical(110, None, Default::default());
        m.begin(200, Some(220), Some(230));
        assert!(m.invalid);
    }
    #[test]
    fn late_or_wrong_epoch_run_is_not_counted() {
        for (start, at) in [(99, 125), (100, 130)] {
            let mut m = MaintenanceMeasurements::default();
            m.begin(100, Some(120), Some(130));
            m.physical(110, None, Default::default());
            m.restored(PeripheralMaintenanceRun {
                admitted_at_micros: start,
                run_at_micros: at,
                restoration_deadline_micros: 130,
                event_counter: 3,
            });
            assert!(m.invalid);
            assert_eq!(m.restored, 0);
        }
    }
}
