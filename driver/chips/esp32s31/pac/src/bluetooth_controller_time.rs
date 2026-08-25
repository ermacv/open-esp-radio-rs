//! Bounded transactions for the reviewed always-awake controller-time latch.
//!
//! Publication performs one fresh-read RMW. Every later call observes the
//! request bit exactly once and returns immediately; only the call which sees
//! the self-clear edge reads the first latched-time word. The PAC stores the
//! in-flight state beside the unique task owner, so cancelling a higher async
//! worker cannot accidentally authorize a second request.

#![deny(unsafe_code)]

const LATCH_REQUEST: u32 = 1 << 26;

/// A controller-time transaction has not yet been drained by software.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerTimeLatchBeginError {
    /// The previous request must be completed before another is published.
    AlreadyInFlight,
}

/// No controller-time request is available for one event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerTimeLatchStepError {
    /// A request must be published before its self-clear edge is observed.
    NotInFlight,
}

/// One controller-time latch request for the always-awake timer path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothControllerTimeLatchRequest;

impl BluetoothControllerTimeLatchRequest {
    /// Construct one request without touching MMIO.
    pub const fn new() -> Self {
        Self
    }

    /// Return the exact fresh-read OR image published to `SLEEP_TIMER_CONTROL`.
    pub const fn publication_image(self, fresh_control_read: u32) -> u32 {
        fresh_control_read | LATCH_REQUEST
    }
}

/// One fresh `SLEEP_TIMER_CONTROL` observation after request publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeLatchObservation(u32);

impl BluetoothControllerTimeLatchObservation {
    /// Retain the complete control-register image used by one decision.
    pub const fn from_control_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether hardware still owns the latch request.
    pub const fn pending(self) -> bool {
        self.0 & LATCH_REQUEST != 0
    }
}

/// First latched controller-time word read after hardware clears the request.
///
/// The value remains a wrapping positional `u32`: its physical unit and
/// effective counter width are not established by current evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerLatchedTime(u32);

impl BluetoothControllerLatchedTime {
    /// Retain one complete `SLEEP_TIMER_LATCHED_TIME_0` image.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete positional image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Result of exactly one live controller-time event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a waiting latch must be revisited after another event"]
pub enum BluetoothControllerTimeLatchStep {
    /// Hardware still owns the request; the caller must yield.
    Waiting,
    /// Hardware cleared the request and the first latched word was read once.
    Ready(BluetoothControllerLatchedTime),
}

/// Sticky software ownership retained beside the unique task-side PAC owner.
///
/// An async operation may disappear while hardware still owns a request. Drop
/// therefore performs no implicit MMIO and does not reset this state. The
/// durable upper task owner can continue with `step_controller_time_latch`; a
/// second begin fails closed until the original request reaches `Ready`.
pub(crate) struct BluetoothControllerTimeLatchOwnership {
    in_flight: bool,
}

impl BluetoothControllerTimeLatchOwnership {
    pub(crate) const fn new() -> Self {
        Self { in_flight: false }
    }

    fn begin(&mut self) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        if self.in_flight {
            return Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight);
        }
        self.in_flight = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn begin_without_mmio_for_test(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        self.begin()
    }

    fn require_in_flight(&self) -> Result<(), BluetoothControllerTimeLatchStepError> {
        if self.in_flight {
            Ok(())
        } else {
            Err(BluetoothControllerTimeLatchStepError::NotInFlight)
        }
    }

    fn complete(&mut self) {
        debug_assert!(self.in_flight);
        self.in_flight = false;
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.in_flight
    }
}

trait BluetoothControllerTimeLatchControl {
    fn publish_latch_request(&mut self, request: BluetoothControllerTimeLatchRequest);
    fn order_after_publication(&mut self);
    fn read_control(&mut self) -> u32;
    fn order_after_clear_observation(&mut self);
    fn read_latched_time_0(&mut self) -> u32;
}

struct HardwareControllerTimeLatchControl<'registers> {
    registers: &'registers super::svd::BluetoothControllerCore,
}

impl BluetoothControllerTimeLatchControl for HardwareControllerTimeLatchControl<'_> {
    fn publish_latch_request(&mut self, _request: BluetoothControllerTimeLatchRequest) {
        self.registers
            .sleep_timer_control()
            .modify(|_, writer| writer.latch_request().set_bit());
    }

    fn order_after_publication(&mut self) {
        super::device_fence();
    }

    fn read_control(&mut self) -> u32 {
        self.registers.sleep_timer_control().read().bits()
    }

    fn order_after_clear_observation(&mut self) {
        super::device_fence();
    }

    fn read_latched_time_0(&mut self) -> u32 {
        self.registers
            .sleep_timer_latched_time_0()
            .read()
            .image()
            .bits()
    }
}

fn execute_latch_publication(
    ownership: &mut BluetoothControllerTimeLatchOwnership,
    control: &mut impl BluetoothControllerTimeLatchControl,
    request: BluetoothControllerTimeLatchRequest,
) -> Result<(), BluetoothControllerTimeLatchBeginError> {
    ownership.begin()?;
    control.publish_latch_request(request);
    control.order_after_publication();
    Ok(())
}

fn execute_latch_step(
    ownership: &mut BluetoothControllerTimeLatchOwnership,
    control: &mut impl BluetoothControllerTimeLatchControl,
) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
    ownership.require_in_flight()?;
    let observation =
        BluetoothControllerTimeLatchObservation::from_control_bits(control.read_control());
    if observation.pending() {
        Ok(BluetoothControllerTimeLatchStep::Waiting)
    } else {
        control.order_after_clear_observation();
        let step = BluetoothControllerTimeLatchStep::Ready(
            BluetoothControllerLatchedTime::from_bits(control.read_latched_time_0()),
        );
        ownership.complete();
        Ok(step)
    }
}

impl super::BluetoothTaskRegisters {
    /// Publish one controller-time latch request.
    ///
    /// The transaction performs one fresh-read RMW which sets only
    /// `SLEEP_TIMER_CONTROL.LATCH_REQUEST`, followed by a device fence. It does
    /// not wait for hardware and cannot overlap an earlier request retained by
    /// this task owner.
    ///
    /// The surrounding powered lifecycle must first establish the reset and
    /// quiescent timer prerequisite. The ownership-only Bluetooth route does
    /// not inspect or clear a pre-existing hardware request bit.
    #[doc(hidden)]
    pub fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        let mut control = HardwareControllerTimeLatchControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_latch_publication(
            &mut self.controller_time_latch,
            &mut control,
            BluetoothControllerTimeLatchRequest::new(),
        )
    }

    /// Perform at most one observation of an in-flight latch request.
    ///
    /// `Waiting` performs only one control-register read. `Ready` performs that
    /// same read followed by exactly one read of
    /// `SLEEP_TIMER_LATCHED_TIME_0`, clears the sticky software ownership and
    /// returns the complete positional word. There is no polling loop.
    #[doc(hidden)]
    pub fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        let mut control = HardwareControllerTimeLatchControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_latch_step(&mut self.controller_time_latch, &mut control)
    }

    /// Whether cancellation left one hardware latch request in flight.
    #[doc(hidden)]
    pub const fn controller_time_latch_in_flight(&self) -> bool {
        self.controller_time_latch.in_flight()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{
        BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
        BluetoothControllerTimeLatchControl, BluetoothControllerTimeLatchObservation,
        BluetoothControllerTimeLatchOwnership, BluetoothControllerTimeLatchRequest,
        BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError, LATCH_REQUEST,
        execute_latch_publication, execute_latch_step,
    };
    use crate::RadioHardware;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Publish(u32),
        ReadControl,
        Fence,
        ReadLatchedTime0,
    }

    struct Recorder {
        control: u32,
        latched_time_0: u32,
        operations: Vec<Operation>,
    }

    impl BluetoothControllerTimeLatchControl for Recorder {
        fn publish_latch_request(&mut self, request: BluetoothControllerTimeLatchRequest) {
            self.control = request.publication_image(self.control);
            self.operations.push(Operation::Publish(self.control));
        }

        fn order_after_publication(&mut self) {
            self.operations.push(Operation::Fence);
        }

        fn read_control(&mut self) -> u32 {
            self.operations.push(Operation::ReadControl);
            self.control
        }

        fn order_after_clear_observation(&mut self) {
            self.operations.push(Operation::Fence);
        }

        fn read_latched_time_0(&mut self) -> u32 {
            self.operations.push(Operation::ReadLatchedTime0);
            self.latched_time_0
        }
    }

    #[test]
    fn latch_publication_preserves_every_non_request_bit() {
        let request = BluetoothControllerTimeLatchRequest::new();

        assert_eq!(request.publication_image(0xa123_4567), 0xa523_4567);
        assert_eq!(request.publication_image(0xa523_4567), 0xa523_4567);
    }

    #[test]
    fn only_latch_request_bit_controls_the_wait_decision() {
        assert!(!BluetoothControllerTimeLatchObservation::from_control_bits(0).pending());
        assert!(BluetoothControllerTimeLatchObservation::from_control_bits(0x8400_0000).pending());
        assert!(!BluetoothControllerTimeLatchObservation::from_control_bits(0x8000_0007).pending());
    }

    #[test]
    fn latched_time_retains_the_complete_wrapping_image() {
        assert_eq!(
            BluetoothControllerLatchedTime::from_bits(0xffff_fffe).bits(),
            0xffff_fffe
        );
    }

    #[test]
    fn publication_is_one_fresh_read_rmw_preserving_other_bits() {
        let mut ownership = BluetoothControllerTimeLatchOwnership::new();
        let mut recorder = Recorder {
            control: 0xa123_4567,
            latched_time_0: 0,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_latch_publication(
                &mut ownership,
                &mut recorder,
                BluetoothControllerTimeLatchRequest::new(),
            ),
            Ok(())
        );

        assert_eq!(recorder.control, 0xa523_4567);
        assert_eq!(
            recorder.operations,
            [Operation::Publish(0xa523_4567), Operation::Fence]
        );
        assert!(ownership.in_flight());

        assert_eq!(
            execute_latch_publication(
                &mut ownership,
                &mut recorder,
                BluetoothControllerTimeLatchRequest::new(),
            ),
            Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight)
        );
        assert_eq!(
            recorder.operations,
            [Operation::Publish(0xa523_4567), Operation::Fence]
        );
    }

    #[test]
    fn pending_step_reads_control_once_and_never_reads_latched_time() {
        let mut ownership = BluetoothControllerTimeLatchOwnership::new();
        assert_eq!(ownership.begin(), Ok(()));
        let mut recorder = Recorder {
            control: LATCH_REQUEST | 7,
            latched_time_0: 0xdead_beef,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_latch_step(&mut ownership, &mut recorder),
            Ok(BluetoothControllerTimeLatchStep::Waiting)
        );
        assert_eq!(recorder.operations, [Operation::ReadControl]);
        assert!(ownership.in_flight());
    }

    #[test]
    fn ready_step_reads_control_then_latched_time_exactly_once() {
        let mut ownership = BluetoothControllerTimeLatchOwnership::new();
        assert_eq!(ownership.begin(), Ok(()));
        let mut recorder = Recorder {
            control: 0x8000_0007,
            latched_time_0: 0xffff_fffe,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_latch_step(&mut ownership, &mut recorder),
            Ok(BluetoothControllerTimeLatchStep::Ready(
                BluetoothControllerLatchedTime::from_bits(0xffff_fffe,)
            ))
        );
        assert_eq!(
            recorder.operations,
            [
                Operation::ReadControl,
                Operation::Fence,
                Operation::ReadLatchedTime0
            ]
        );
        assert!(!ownership.in_flight());

        let operation_count = recorder.operations.len();
        assert_eq!(
            execute_latch_step(&mut ownership, &mut recorder),
            Err(BluetoothControllerTimeLatchStepError::NotInFlight)
        );
        assert_eq!(recorder.operations.len(), operation_count);
    }

    #[test]
    fn cancelled_worker_resumes_the_same_request_and_drains_it_once() {
        let mut ownership = BluetoothControllerTimeLatchOwnership::new();
        let mut publication = Recorder {
            control: 7,
            latched_time_0: 0,
            operations: Vec::new(),
        };
        assert_eq!(
            execute_latch_publication(
                &mut ownership,
                &mut publication,
                BluetoothControllerTimeLatchRequest::new(),
            ),
            Ok(())
        );

        let mut first_worker = Recorder {
            control: LATCH_REQUEST | 7,
            latched_time_0: 0,
            operations: Vec::new(),
        };
        assert_eq!(
            execute_latch_step(&mut ownership, &mut first_worker),
            Ok(BluetoothControllerTimeLatchStep::Waiting)
        );
        drop(first_worker);

        let mut replacement_worker = Recorder {
            control: 7,
            latched_time_0: 0x1234_5678,
            operations: Vec::new(),
        };
        assert_eq!(
            execute_latch_publication(
                &mut ownership,
                &mut replacement_worker,
                BluetoothControllerTimeLatchRequest::new(),
            ),
            Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight)
        );
        assert!(replacement_worker.operations.is_empty());
        assert_eq!(
            execute_latch_step(&mut ownership, &mut replacement_worker),
            Ok(BluetoothControllerTimeLatchStep::Ready(
                BluetoothControllerLatchedTime::from_bits(0x1234_5678)
            ))
        );
        assert_eq!(
            replacement_worker.operations,
            [
                Operation::ReadControl,
                Operation::Fence,
                Operation::ReadLatchedTime0
            ]
        );

        let operation_count = replacement_worker.operations.len();
        assert_eq!(
            execute_latch_step(&mut ownership, &mut replacement_worker),
            Err(BluetoothControllerTimeLatchStepError::NotInFlight)
        );
        assert_eq!(replacement_worker.operations.len(), operation_count);
    }

    #[test]
    fn idle_step_fails_without_any_register_access() {
        let mut ownership = BluetoothControllerTimeLatchOwnership::new();
        let mut recorder = Recorder {
            control: 0,
            latched_time_0: 0,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_latch_step(&mut ownership, &mut recorder),
            Err(BluetoothControllerTimeLatchStepError::NotInFlight)
        );
        assert!(recorder.operations.is_empty());
    }

    #[test]
    fn unfinished_latch_prevents_owner_reunion_without_mmio() {
        let cold = RadioHardware::for_validation().into_bluetooth();
        let (mut task, interrupts) = cold.separate_interrupt_owner();
        assert_eq!(task.controller_time_latch.begin(), Ok(()));

        let failure = match task.into_cold(interrupts) {
            Ok(_) => panic!("an unfinished latch must retain both owners"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            crate::BluetoothTaskReuniteError::ControllerTimeLatchInFlight
        );
        let (task, _interrupts, error) = failure.into_parts();
        assert_eq!(
            error,
            crate::BluetoothTaskReuniteError::ControllerTimeLatchInFlight
        );
        assert!(task.controller_time_latch_in_flight());
    }
}
