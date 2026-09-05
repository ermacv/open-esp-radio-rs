//! Event-driven controller-time latch and scheduler-epoch projection.
//!
//! Every in-flight observation is a single decision. `Waiting` tells the
//! caller to return to its executor and arrange a later interrupt or bounded
//! timer recheck; no state here spins, allocates, stores a waker or depends on
//! an RTOS.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_pac::{BluetoothControllerLatchedTime, BluetoothControllerTimeScale};

/// One ordered controller-time sample from the always-awake latch path.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerTimeSample {
    latched_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerTimeSample {
    #[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
    const fn from_live_latch(latched_time: BluetoothControllerLatchedTime) -> Self {
        Self { latched_time }
    }

    #[cfg(test)]
    pub(crate) const fn for_validation(raw_ticks: u32) -> Self {
        Self::from_live_latch(BluetoothControllerLatchedTime::from_bits(raw_ticks))
    }

    /// Return the complete wrapping raw controller-tick image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_ticks(&self) -> u32 {
        self.latched_time.bits()
    }

    /// Borrow the typed hardware sample for one private descriptor update.
    ///
    /// This does not expose an integer image or duplicate scheduler-time
    /// authority. The returned PAC value can only enter a lower typed codec.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn latched_time(&self) -> BluetoothControllerLatchedTime {
        self.latched_time
    }
}

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
mod worker {
    use open_esp_radio_esp32s31_hal::{
        BluetoothControllerHal, BluetoothControllerTimeLatchBeginError,
        BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
    };

    use super::BluetoothControllerTimeSample;

    /// Crate-private hardware seam required by the controller-time worker.
    ///
    /// Keeping this boundary private prevents downstream code from manufacturing
    /// a supposedly live sample. Production has exactly one implementation: the
    /// finite borrow of the unique controller HAL owner.
    pub(crate) trait BluetoothControllerTimeHardware {
        /// Publish one fresh request without waiting for hardware.
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError>;

        /// Perform exactly one hardware observation and return immediately.
        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>;
    }

    impl BluetoothControllerTimeHardware for BluetoothControllerHal<'_> {
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
            BluetoothControllerHal::begin_controller_time_latch(self)
        }

        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>
        {
            BluetoothControllerHal::step_controller_time_latch(self)
        }
    }

    /// Opaque identity of one logical controller-time request.
    ///
    /// A cancellation path must return the identity it received from `request`.
    /// A late identity cannot silently abandon a newer request: it faults the
    /// private worker before any additional hardware observation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[must_use = "the request identity must be completed or abandoned"]
    pub(crate) struct BluetoothControllerTimeRequest(u64);

    impl BluetoothControllerTimeRequest {
        #[cfg(test)]
        pub(crate) const fn for_validation(generation: u64) -> Self {
            Self(generation)
        }
    }

    /// Durable logical phase retained between executor events.
    #[cfg(test)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum BluetoothControllerTimeWorkerPhase {
        /// No logical or hardware request belongs to this worker.
        Idle,
        /// This worker published the request and may return its completed sample.
        Requested,
        /// An abandoned request must be drained without becoming a sample for a
        /// later logical caller.
        DrainingOrphan,
        /// Hardware and logical ownership disagreed; no further MMIO is allowed.
        Faulted,
    }

    /// Why a fresh logical controller-time request was not published.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum BluetoothControllerTimeRequestError {
        /// Another logical request or orphan drain is active; no MMIO occurred.
        Busy,
        /// The lower owner already held a request although the durable worker was
        /// idle. The worker entered fail-stop instead of stealing that request.
        OwnershipCollision,
        /// The non-repeating request generation space was exhausted before MMIO.
        GenerationExhausted,
        /// A prior ownership mismatch put the worker into fail-stop.
        Faulted,
    }

    /// Result of exactly one controller-time recheck event.
    #[derive(Debug, Eq, PartialEq)]
    #[must_use = "the worker event outcome must drive the next controller action"]
    pub(crate) enum BluetoothControllerTimeEventStep {
        /// No transaction was active; no MMIO occurred.
        Idle,
        /// Hardware still owns the request; arrange one later recheck event.
        Waiting,
        /// The request owned by this worker produced one ordered live sample.
        Sample {
            /// Identity of the logical request which owns this result.
            request: BluetoothControllerTimeRequest,
            /// Ordered sample read by the live HAL path.
            sample: BluetoothControllerTimeSample,
        },
        /// An abandoned request was drained without relabelling its latched word
        /// as a new logical sample.
        OrphanDrained,
    }

    /// Fail-stop result of a controller-time recheck event.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum BluetoothControllerTimeEventError {
        /// The caller did not present the identity owned by the active request.
        RequestMismatch,
        /// The lower sticky owner disappeared while the worker was active.
        OwnershipLost,
        /// A previous ownership mismatch already stopped this worker.
        Faulted,
    }

    /// Executor-neutral logical owner of controller-time sampling.
    ///
    /// The sole controller runner must retain this state beside its task resources,
    /// while every method receives only a short HAL borrow. Each recheck performs
    /// at most one hardware observation and contains no loop, await, waker, timer,
    /// allocator or RTOS binding.
    pub(crate) struct BluetoothControllerTimeWorker {
        state: BluetoothControllerTimeWorkerState,
        last_generation: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum BluetoothControllerTimeWorkerState {
        Idle,
        Requested(BluetoothControllerTimeRequest),
        DrainingOrphan(BluetoothControllerTimeRequest),
        Faulted,
    }

    impl BluetoothControllerTimeWorker {
        /// Construct the sole worker while splitting a proven-cold task owner.
        ///
        /// This constructor is intentionally private to the crate ownership
        /// transition. Moving later lifecycle typestates carries this value; they
        /// never reconstruct it from a borrowed HAL or clear a fault by dropping
        /// the worker.
        pub(crate) const fn new_idle() -> Self {
            Self {
                state: BluetoothControllerTimeWorkerState::Idle,
                last_generation: 0,
            }
        }

        /// Current durable logical phase.
        #[cfg(test)]
        pub(crate) const fn phase(&self) -> BluetoothControllerTimeWorkerPhase {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {
                    BluetoothControllerTimeWorkerPhase::Idle
                }
                BluetoothControllerTimeWorkerState::Requested(_) => {
                    BluetoothControllerTimeWorkerPhase::Requested
                }
                BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    BluetoothControllerTimeWorkerPhase::DrainingOrphan
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    BluetoothControllerTimeWorkerPhase::Faulted
                }
            }
        }

        /// Whether an external durable wake or bounded timer must recheck hardware.
        #[cfg(test)]
        pub(crate) const fn needs_recheck(&self) -> bool {
            matches!(
                self.state,
                BluetoothControllerTimeWorkerState::Requested(_)
                    | BluetoothControllerTimeWorkerState::DrainingOrphan(_)
            )
        }

        /// Whether the complete task owner may return to the cold ownership state.
        #[cfg(test)]
        pub(crate) const fn is_reunitable(&self) -> bool {
            matches!(self.state, BluetoothControllerTimeWorkerState::Idle)
        }

        #[cfg(test)]
        pub(crate) fn exhaust_generation_for_validation(&mut self) {
            self.last_generation = u64::MAX;
        }

        /// Publish one fresh logical sample request.
        ///
        /// Non-idle phases return without MMIO. A lower-layer collision proves
        /// desynchronized ownership and permanently faults this worker.
        pub(crate) fn request(
            &mut self,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {}
                BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    return Err(BluetoothControllerTimeRequestError::Busy);
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeRequestError::Faulted);
                }
            }

            let Some(generation) = self.last_generation.checked_add(1) else {
                self.state = BluetoothControllerTimeWorkerState::Faulted;
                return Err(BluetoothControllerTimeRequestError::GenerationExhausted);
            };
            let request = BluetoothControllerTimeRequest(generation);

            match hardware.begin_controller_time_latch() {
                Ok(()) => {
                    self.last_generation = generation;
                    self.state = BluetoothControllerTimeWorkerState::Requested(request);
                    Ok(request)
                }
                Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeRequestError::OwnershipCollision)
                }
            }
        }

        /// Abandon the current logical caller without clearing hardware ownership.
        ///
        /// A later event must still drain the request, but its latched value is no
        /// longer allowed to satisfy any logical sample request.
        pub(crate) fn cancel_owned(
            &mut self,
            request: BluetoothControllerTimeRequest,
        ) -> Result<(), BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Requested(owned) if owned == request => {
                    self.state = BluetoothControllerTimeWorkerState::DrainingOrphan(request);
                    Ok(())
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    Err(BluetoothControllerTimeEventError::Faulted)
                }
                BluetoothControllerTimeWorkerState::Idle
                | BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::RequestMismatch)
                }
            }
        }

        /// Recheck exactly the request whose affine owner is presented.
        pub(crate) fn recheck_owned(
            &mut self,
            request: BluetoothControllerTimeRequest,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Requested(owned) if owned == request => {}
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeEventError::Faulted);
                }
                BluetoothControllerTimeWorkerState::Idle
                | BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    return Err(BluetoothControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(BluetoothControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(latched_time)) => {
                    self.state = BluetoothControllerTimeWorkerState::Idle;
                    Ok(BluetoothControllerTimeEventStep::Sample {
                        request,
                        sample: BluetoothControllerTimeSample::from_live_latch(latched_time),
                    })
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::OwnershipLost)
                }
            }
        }

        /// Drain one abandoned hardware request without creating a sample.
        pub(crate) fn drain_orphan(
            &mut self,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {
                    return Ok(BluetoothControllerTimeEventStep::Idle);
                }
                BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {}
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeEventError::Faulted);
                }
                BluetoothControllerTimeWorkerState::Requested(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    return Err(BluetoothControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(BluetoothControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(_)) => {
                    self.state = BluetoothControllerTimeWorkerState::Idle;
                    Ok(BluetoothControllerTimeEventStep::OrphanDrained)
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::OwnershipLost)
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) use worker::BluetoothControllerTimeHardware;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use worker::BluetoothControllerTimeRequest;
#[cfg(test)]
pub(crate) use worker::BluetoothControllerTimeWorkerPhase;
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) use worker::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequestError, BluetoothControllerTimeWorker,
};

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) trait BluetoothControllerTimePendingOwner {
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError>;

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError>;

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError>;
}

#[cfg(any(target_arch = "riscv32", test))]
impl<T> BluetoothControllerTimePendingOwner for &mut T
where
    T: BluetoothControllerTimePendingOwner + ?Sized,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        T::recheck_owned_controller_time(self, request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        T::cancel_owned_controller_time(self, request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        T::drain_orphan_controller_time(self)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) enum BluetoothControllerTimePendingOwnerStep {
    Waiting,
    Ready(BluetoothControllerTimeSample),
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothControllerTimePendingOrphanStep {
    Idle,
    Waiting,
    Drained,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) struct BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    owner: Option<O>,
    request: Option<BluetoothControllerTimeRequest>,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) enum BluetoothControllerTimePendingCoreStep<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    Waiting(BluetoothControllerTimePendingCore<O>),
    Ready {
        owner: O,
        sample: BluetoothControllerTimeSample,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) struct BluetoothControllerTimePendingCoreFailure<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    owner: O,
    error: BluetoothControllerTimeEventError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> BluetoothControllerTimePendingCoreFailure<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    pub(crate) fn into_parts(self) -> (O, BluetoothControllerTimeEventError) {
        (self.owner, self.error)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    pub(crate) const fn new(owner: O, request: BluetoothControllerTimeRequest) -> Self {
        Self {
            owner: Some(owner),
            request: Some(request),
        }
    }

    pub(crate) fn recheck(
        mut self,
    ) -> Result<
        BluetoothControllerTimePendingCoreStep<O>,
        BluetoothControllerTimePendingCoreFailure<O>,
    > {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.recheck_owned_controller_time(request) {
            Ok(BluetoothControllerTimePendingOwnerStep::Waiting) => Ok(
                BluetoothControllerTimePendingCoreStep::Waiting(Self::new(owner, request)),
            ),
            Ok(BluetoothControllerTimePendingOwnerStep::Ready(sample)) => {
                Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample })
            }
            Err(error) => Err(BluetoothControllerTimePendingCoreFailure { owner, error }),
        }
    }

    pub(crate) fn cancel(mut self) -> Result<O, BluetoothControllerTimePendingCoreFailure<O>> {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.cancel_owned_controller_time(request) {
            Ok(()) => Ok(owner),
            Err(error) => Err(BluetoothControllerTimePendingCoreFailure { owner, error }),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> Drop for BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    fn drop(&mut self) {
        if let (Some(mut owner), Some(request)) = (self.owner.take(), self.request.take()) {
            let _result = owner.cancel_owned_controller_time(request);
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) fn drain_controller_time_orphan(
    owner: &mut impl BluetoothControllerTimePendingOwner,
) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
    owner.drain_orphan_controller_time()
}

/// Raw-tick anchor paired with the BLE scheduler's microsecond epoch.
///
/// The projection exactly retains the current `r_sched_timer_convertTimeToUs`
/// branch geometry. Every reviewed S31 HAL configuration has a positive scale
/// image, so the helper's optional negative-side remainder is always zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerSchedulerEpoch {
    raw_tick_anchor: u32,
    micros_anchor: u32,
    scale: BluetoothControllerTimeScale,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerSchedulerEpoch {
    /// Establish the source-owned epoch from its first live raw-tick update.
    ///
    /// Scheduler initialization starts both reference images at zero. Every
    /// reviewed ESP32-S31 time scale is positive, so the forward conversion's
    /// optional remainder is zero: the raw anchor is the exact sample and the
    /// scheduler anchor is its wrapping scale projection. The constructor only
    /// borrows the sample so the caller can then consume that same affine value
    /// into [`BluetoothControllerSchedulerNow`].
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_first_live_update(
        sample: &BluetoothControllerTimeSample,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        let raw_tick_anchor = sample.raw_ticks();
        Self {
            raw_tick_anchor,
            micros_anchor: scale.micros_from_raw_ticks(raw_tick_anchor),
            scale,
        }
    }

    /// Bind arbitrary validation anchors without publishing that authority in
    /// production code.
    #[cfg(test)]
    pub(crate) const fn new(
        sample: BluetoothControllerTimeSample,
        micros_anchor: u32,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            raw_tick_anchor: sample.raw_ticks(),
            micros_anchor,
            scale,
        }
    }

    /// Project a later or earlier wrapping raw sample into the scheduler epoch.
    #[cfg(test)]
    pub(crate) const fn project(self, sample: BluetoothControllerTimeSample) -> u32 {
        self.project_without_reanchor(&sample)
    }

    /// Project one completed live sample while retaining the exact epoch.
    ///
    /// The post-enable timing observation is a separate scheduler-time read in
    /// the reviewed standalone flow. It must use the retained epoch, but unlike
    /// a task-run current-time update it does not advance either epoch anchor.
    pub(crate) const fn project_without_reanchor(
        self,
        sample: &BluetoothControllerTimeSample,
    ) -> u32 {
        self.project_raw_ticks(sample.raw_ticks())
    }

    /// Project the raw timestamp captured beside one completed LE RX packet.
    ///
    /// Packet capture is not a fresh controller-time sample and therefore
    /// cannot re-anchor the scheduler epoch. The opaque memory-layer value is
    /// consumed only by this conversion before PHY calibration.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn project_le_packet_capture(
        self,
        captured: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLePacketCapturedTime,
    ) -> u32 {
        self.project_raw_ticks(captured.wrapping_controller_ticks())
    }

    /// Project the hardware capture retained by one completed connection event.
    ///
    /// Like an RX-packet capture, this is not a fresh time sample and cannot
    /// advance the persistent scheduler epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn project_peripheral_connection_capture(
        self,
        captured: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionCapturedAnchorTime,
    ) -> u32 {
        self.project_raw_ticks(captured.wrapping_controller_ticks())
    }

    /// Advance the raw anchor while preserving this sample's scheduler image.
    ///
    /// The Controller does this after every live task-run reference update.
    /// Re-anchoring is required even when the forward image is unchanged:
    /// shifting scales have wrapping aliases that the inverse projection can
    /// distinguish only through the latest raw anchor.
    pub(crate) const fn reanchor(self, sample: &BluetoothControllerTimeSample) -> Self {
        let raw_tick_anchor = sample.raw_ticks();
        Self {
            raw_tick_anchor,
            micros_anchor: self.project_raw_ticks(raw_tick_anchor),
            scale: self.scale,
        }
    }

    const fn project_raw_ticks(self, raw_ticks: u32) -> u32 {
        let delta = raw_ticks.wrapping_sub(self.raw_tick_anchor);
        if delta as i32 >= 0 {
            self.micros_anchor
                .wrapping_add(self.scale.micros_from_raw_ticks(delta))
        } else {
            self.micros_anchor
                .wrapping_sub(self.scale.micros_from_raw_ticks(delta.wrapping_neg()))
        }
    }

    /// Project one scheduler microsecond position back into raw controller ticks.
    ///
    /// This retains the complete `r_sched_timer_convertTimeToTicks` branch
    /// geometry. Discarded low scheduler bits are truncated toward the epoch
    /// anchor on both sides, matching the reviewed inverse helper.
    pub const fn raw_ticks_for_micros(self, micros: u32) -> u32 {
        let delta = micros.wrapping_sub(self.micros_anchor);
        if delta as i32 >= 0 {
            self.raw_tick_anchor
                .wrapping_add(self.scale.raw_ticks_from_micros(delta).whole_ticks)
        } else {
            self.raw_tick_anchor.wrapping_sub(
                self.scale
                    .raw_ticks_from_micros(delta.wrapping_neg())
                    .whole_ticks,
            )
        }
    }

    /// Convert a duration without applying either scheduler epoch anchor.
    ///
    /// Descriptor intervals are deltas, not absolute scheduler positions.
    /// Keeping this operation on the retained epoch prevents callers from
    /// reconstructing a duration by subtracting two independently truncated
    /// absolute projections.
    pub(crate) const fn raw_duration_ticks_for_micros(self, micros: u32) -> u32 {
        self.scale.raw_ticks_from_micros(micros).whole_ticks
    }
}

/// One exact live sample bound to the retained Controller scheduler epoch.
///
/// This aggregate is deliberately affine: it cannot duplicate or detach the
/// sample from the epoch used to project it. Hardware ownership is not implied,
/// and the projected image is not RF-ready authority.
#[must_use = "the epoch-bound live scheduler sample must be consumed by scheduling"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerSchedulerNow {
    epoch: BluetoothControllerSchedulerEpoch,
    sample: BluetoothControllerTimeSample,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerSchedulerNow {
    /// Bind one exact sample to an already retained source-owned epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_retained_epoch(
        epoch: BluetoothControllerSchedulerEpoch,
        sample: BluetoothControllerTimeSample,
    ) -> Self {
        Self { epoch, sample }
    }

    /// Retained epoch used for this exact projection.
    pub(crate) const fn epoch(&self) -> BluetoothControllerSchedulerEpoch {
        self.epoch
    }

    /// Exact raw sample retained by this projection.
    #[cfg(test)]
    pub(crate) const fn sample(&self) -> &BluetoothControllerTimeSample {
        &self.sample
    }

    /// Wrapping microsecond image projected from the retained sample and epoch.
    pub(crate) const fn micros(&self) -> u32 {
        self.epoch.project_raw_ticks(self.sample.raw_ticks())
    }
}

#[cfg(test)]
mod tests;
