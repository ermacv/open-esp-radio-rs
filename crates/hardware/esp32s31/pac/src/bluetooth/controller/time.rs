//! Bounded transactions for the reviewed always-awake controller-time latch.
//!
//! Publication performs one fresh-read RMW. Every later call observes the
//! request bit exactly once and returns immediately; only the call which sees
//! the self-clear edge reads the first latched-time word. The PAC stores the
//! in-flight state beside the unique task owner, so cancelling a higher async
//! worker cannot accidentally authorize a second request.

#![deny(unsafe_code)]

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
    fn latch_request_pending(&mut self) -> bool;
    fn order_after_clear_observation(&mut self);
    fn read_latched_time_0(&mut self) -> u32;
}

struct HardwareControllerTimeLatchControl<'registers> {
    registers: &'registers crate::svd::BluetoothControllerCore,
}

impl BluetoothControllerTimeLatchControl for HardwareControllerTimeLatchControl<'_> {
    fn publish_latch_request(&mut self, _request: BluetoothControllerTimeLatchRequest) {
        crate::generated::request_bluetooth_controller_time_latch(self.registers);
    }

    fn order_after_publication(&mut self) {
        crate::device_fence();
    }

    fn latch_request_pending(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_controller_time_latch_request(self.registers)
    }

    fn order_after_clear_observation(&mut self) {
        crate::device_fence();
    }

    fn read_latched_time_0(&mut self) -> u32 {
        crate::svd::field_read::observe_bluetooth_controller_latched_time(self.registers)
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
    if control.latch_request_pending() {
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

impl crate::BluetoothTaskRegisters {
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
mod tests;
