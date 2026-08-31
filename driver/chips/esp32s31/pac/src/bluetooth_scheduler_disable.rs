//! Affine, bounded ownership for the reviewed scheduler-disable transaction.

#![deny(unsafe_code)]

use super::{BluetoothInterruptOutputPrepared, BluetoothTaskRegisters, device_fence};

trait BluetoothSchedulerLifecycleRequest {
    fn publish_image_1(&mut self);
    fn fence(&mut self);
}

trait BluetoothSchedulerDisableStatus {
    fn scheduler_busy(&mut self) -> bool;
    fn fence(&mut self);
}

struct HardwareLifecycleRequest<'a> {
    controller: &'a super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerLifecycleRequest for HardwareLifecycleRequest<'_> {
    fn publish_image_1(&mut self) {
        super::svd::fixed_register_image::publish_bluetooth_scheduler_lifecycle_request_image_1(
            self.controller,
        );
    }

    fn fence(&mut self) {
        device_fence();
    }
}

struct HardwareDisableStatus<'a> {
    interrupts: &'a BluetoothInterruptOutputPrepared,
}

impl BluetoothSchedulerDisableStatus for HardwareDisableStatus<'_> {
    fn scheduler_busy(&mut self) -> bool {
        self.interrupts.scheduler_busy_after_routes()
    }

    fn fence(&mut self) {
        device_fence();
    }
}

fn execute_disable_begin(
    controller_time_latch_in_flight: bool,
    request: &mut impl BluetoothSchedulerLifecycleRequest,
) -> Result<(), BluetoothSchedulerDisableBeginError> {
    if controller_time_latch_in_flight {
        return Err(BluetoothSchedulerDisableBeginError::ControllerTimeLatchInFlight);
    }
    request.publish_image_1();
    request.fence();
    Ok(())
}

fn execute_disable_step(status: &mut impl BluetoothSchedulerDisableStatus) -> bool {
    let busy = status.scheduler_busy();
    if !busy {
        status.fence();
    }
    busy
}

/// Why the scheduler lifecycle request could not be published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerDisableBeginError {
    /// A controller-time latch request still belongs to the task owner.
    ControllerTimeLatchInFlight,
}

/// Failed scheduler-disable admission retaining the unique task owner.
///
/// Admission is checked before the first MMIO effect, so the returned owner is
/// the same logical capability supplied by the caller.
#[must_use = "a failed scheduler disable still owns the task partition"]
pub struct BluetoothSchedulerDisableBeginFailure {
    task: BluetoothTaskRegisters,
    error: BluetoothSchedulerDisableBeginError,
}

impl BluetoothSchedulerDisableBeginFailure {
    /// Return the finite admission failure reason.
    pub const fn error(&self) -> BluetoothSchedulerDisableBeginError {
        self.error
    }

    /// Recover the unchanged task owner and rejection reason.
    pub fn into_parts(self) -> (BluetoothTaskRegisters, BluetoothSchedulerDisableBeginError) {
        (self.task, self.error)
    }
}

impl core::fmt::Debug for BluetoothSchedulerDisableBeginFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerDisableBeginFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Scheduler-disable request published while the task owner remains affine.
///
/// This value is intentionally not a `Future`. Each [`Self::step`] performs
/// exactly one volatile status read and returns immediately. Current evidence
/// proves no wake source for BUSY clearing, so a busy observation is terminal
/// until a later interrupt/deadline bridge supplies an affine recheck permit.
///
/// The request cannot be polled twice after ownership has moved:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::{
///     BluetoothInterruptOutputPrepared, BluetoothSchedulerDisableRequest,
/// };
///
/// fn poll_twice(
///     request: BluetoothSchedulerDisableRequest,
///     interrupts: &mut BluetoothInterruptOutputPrepared,
/// ) {
///     let _first = request.step_after_cpu_routes_disabled(interrupts);
///     let _second = request.step_after_cpu_routes_disabled(interrupts);
/// }
/// ```
#[must_use = "the disable request admits exactly one bounded status observation"]
pub struct BluetoothSchedulerDisableRequest {
    task: BluetoothTaskRegisters,
}

/// Result of one bounded scheduler-disable observation.
#[must_use = "retain the resulting busy or idle terminal observation"]
pub enum BluetoothSchedulerDisableStep {
    /// One fresh read observed BUSY set. No public recheck exists until a
    /// separately proven event can supply an affine permit.
    BusyObserved(BluetoothSchedulerDisableBusyObserved),
    /// One fresh read observed the positional BUSY bit clear.
    IdleObserved(BluetoothSchedulerDisableIdleObserved),
}

/// Task ownership after one fresh read observed `SCHEDULER_STATE.BUSY` set.
///
/// This is deliberately terminal today: returning the request would permit a
/// busy-poll loop despite the absence of a proven wake source. A later
/// interrupt/deadline bridge may consume this value together with its affine
/// recheck permit.
#[must_use = "the busy observation retains the task until a proven recheck edge exists"]
pub struct BluetoothSchedulerDisableBusyObserved {
    _task: BluetoothTaskRegisters,
}

/// Task ownership after a fresh clear observation of `SCHEDULER_STATE.BUSY`.
///
/// This proves only the reviewed request/read edge. The hardware meaning of
/// the request remains unknown; CPU routes, packets, PHY, BTBB and clocks are
/// not proven quiescent by this value.
///
/// The retained task owner is intentionally not recoverable yet:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothSchedulerDisableIdleObserved;
///
/// fn bypass_remaining_teardown(observation: BluetoothSchedulerDisableIdleObserved) {
///     let _task = observation.into_task();
/// }
/// ```
#[must_use = "the idle observation must continue through verified teardown"]
pub struct BluetoothSchedulerDisableIdleObserved {
    _task: BluetoothTaskRegisters,
}

impl BluetoothTaskRegisters {
    /// Publish the scheduler lifecycle request for the disable transaction
    /// while the complete IRQ route epoch remains retained by the same
    /// lifecycle.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` `btdm_sched.c` symbol
    /// `r_sym_bt_74l62ZLsZuXg67pPHSd7`, reached from both `r_btdm_task_disable`
    /// and `r_btdm_task_shutdown`. It writes complete image one to
    /// `0x2010_1004`, then waits while `SCHEDULER_STATE.BUSY` is set.
    ///
    /// Current evidence does not prove a race-free read through storage shared
    /// with a live ISR. The later status steps therefore accept only the
    /// controller-output owner after both CPU routes and shared ISR access have
    /// ended; live-route teardown must first produce that state.
    pub fn begin_scheduler_disable(
        self,
    ) -> Result<BluetoothSchedulerDisableRequest, BluetoothSchedulerDisableBeginFailure> {
        begin_scheduler_disable(self)
    }
}

fn begin_scheduler_disable(
    task: BluetoothTaskRegisters,
) -> Result<BluetoothSchedulerDisableRequest, BluetoothSchedulerDisableBeginFailure> {
    let controller_time_latch_in_flight = task.controller_time_latch.in_flight();
    let mut request = HardwareLifecycleRequest {
        controller: &task.bluetooth.bluetooth_controller_core,
    };
    if let Err(error) = execute_disable_begin(controller_time_latch_in_flight, &mut request) {
        return Err(BluetoothSchedulerDisableBeginFailure { task, error });
    }
    Ok(BluetoothSchedulerDisableRequest { task })
}

fn step_scheduler_disable(
    request: BluetoothSchedulerDisableRequest,
    busy: bool,
) -> BluetoothSchedulerDisableStep {
    if busy {
        BluetoothSchedulerDisableStep::BusyObserved(BluetoothSchedulerDisableBusyObserved {
            _task: request.task,
        })
    } else {
        BluetoothSchedulerDisableStep::IdleObserved(BluetoothSchedulerDisableIdleObserved {
            _task: request.task,
        })
    }
}

impl BluetoothSchedulerDisableRequest {
    /// Perform one fresh scheduler-state observation through the controller
    /// output owner after CPU routes and shared ISR access have ended. This method
    /// never spins, blocks, allocates or registers a waker.
    pub fn step_after_cpu_routes_disabled(
        self,
        interrupts: &mut BluetoothInterruptOutputPrepared,
    ) -> BluetoothSchedulerDisableStep {
        let busy = {
            let mut status = HardwareDisableStatus { interrupts };
            execute_disable_step(&mut status)
        };
        step_scheduler_disable(self, busy)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{
        BluetoothSchedulerDisableBeginError, BluetoothSchedulerDisableStatus,
        BluetoothSchedulerLifecycleRequest, execute_disable_begin, execute_disable_step,
    };
    use crate::RadioHardware;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PublishLifecycleRequestImage1,
        ReadBusy(bool),
        Fence,
    }

    struct Recorder {
        states: [bool; 1],
        next_state: usize,
        operations: Vec<Operation>,
    }

    impl BluetoothSchedulerLifecycleRequest for Recorder {
        fn publish_image_1(&mut self) {
            self.operations
                .push(Operation::PublishLifecycleRequestImage1);
        }

        fn fence(&mut self) {
            self.operations.push(Operation::Fence);
        }
    }

    impl BluetoothSchedulerDisableStatus for Recorder {
        fn scheduler_busy(&mut self) -> bool {
            let busy = self.states[self.next_state];
            self.next_state += 1;
            self.operations.push(Operation::ReadBusy(busy));
            busy
        }

        fn fence(&mut self) {
            self.operations.push(Operation::Fence);
        }
    }

    #[test]
    fn lifecycle_request_publication_precedes_one_idle_observation() {
        let mut recorder = Recorder {
            states: [false],
            next_state: 0,
            operations: Vec::new(),
        };

        execute_disable_begin(false, &mut recorder).expect("idle preflight admits request");
        assert!(!execute_disable_step(&mut recorder));

        assert_eq!(
            recorder.operations,
            [
                Operation::PublishLifecycleRequestImage1,
                Operation::Fence,
                Operation::ReadBusy(false),
                Operation::Fence,
            ]
        );
    }

    #[test]
    fn busy_observation_is_one_read_without_an_invented_recheck() {
        let mut recorder = Recorder {
            states: [true],
            next_state: 0,
            operations: Vec::new(),
        };

        execute_disable_begin(false, &mut recorder).expect("idle preflight admits request");
        assert!(execute_disable_step(&mut recorder));

        assert_eq!(
            recorder.operations,
            [
                Operation::PublishLifecycleRequestImage1,
                Operation::Fence,
                Operation::ReadBusy(true),
            ]
        );
    }

    #[test]
    fn in_flight_time_latch_rejects_before_request_or_fence() {
        let mut recorder = Recorder {
            states: [false],
            next_state: 0,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_disable_begin(true, &mut recorder),
            Err(BluetoothSchedulerDisableBeginError::ControllerTimeLatchInFlight)
        );
        assert!(recorder.operations.is_empty());
    }

    #[test]
    fn preflight_failure_returns_the_unchanged_task_owner() {
        let bluetooth = RadioHardware::for_validation().into_bluetooth();
        let (mut task, interrupts) = bluetooth.separate_interrupt_owner();
        assert_eq!(
            task.controller_time_latch.begin_without_mmio_for_test(),
            Ok(())
        );
        let failure = match task.begin_scheduler_disable() {
            Ok(_) => panic!("an in-flight time latch must reject before MMIO"),
            Err(failure) => failure,
        };
        let (task, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothSchedulerDisableBeginError::ControllerTimeLatchInFlight
        );
        let _retained_owners = (task, interrupts);
    }
}
