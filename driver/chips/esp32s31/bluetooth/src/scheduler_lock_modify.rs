//! Event-driven controller model for one scheduler lock/modify transaction.
//!
//! Every transition consumes a fresh cross-owner observation. `Waiting` means
//! the caller should return to its executor and resume only after an interrupt
//! or another controller event supplies a new observation; no path spins.

#![forbid(unsafe_code)]

use core::{
    mem,
    sync::atomic::{AtomicU8, Ordering},
};

pub use open_esp_radio_esp32s31_hal::BluetoothSchedulerLockModifyInterruptObservation;
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation,
};

const EVENT_PENDING: u8 = 1 << 0;
const EVENT_BUSY: u8 = 1 << 1;

/// Result of evaluating one fresh scheduler event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "waiting state or completed transition must remain owned"]
pub enum BluetoothSchedulerLockModifyProgress<W, R> {
    /// Hardware still has both BUSY and START set.
    Waiting(W),
    /// The current phase can advance without polling.
    Ready(R),
}

/// Result of publishing one scheduler observation from the hard handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerLockModifyEventPublication {
    /// The cell changed from empty to pending; wake the sole controller worker.
    WakeWorker,
    /// A pending observation was replaced by the newer scheduler state.
    Coalesced,
}

/// One affine scheduler event removed from the ISR-to-worker handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler event must be processed or explicitly discarded"]
pub struct BluetoothSchedulerLockModifyEvent {
    interrupt: BluetoothSchedulerLockModifyInterruptObservation,
}

impl BluetoothSchedulerLockModifyEvent {
    /// Whether the newest interrupt-side scheduler observation was busy.
    pub const fn is_busy(&self) -> bool {
        self.interrupt.is_busy()
    }
}

/// Lock-free, allocation-free latest-value handoff from ISR to async worker.
///
/// The cell stores no count because every worker step performs fresh
/// task-owned observation and scheduler wait predicates are level-like. A
/// newer BUSY value replaces an older pending value. A publication racing
/// after [`Self::take`] creates a new pending epoch and therefore a new wake
/// edge; there is no RTOS queue or callback execution in interrupt context.
pub struct BluetoothSchedulerLockModifyEventCell {
    state: AtomicU8,
}

impl BluetoothSchedulerLockModifyEventCell {
    /// Construct an empty handoff.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    /// Publish one semantic BUSY observation from the unique interrupt owner.
    pub fn publish_from_interrupt(
        &self,
        observation: BluetoothSchedulerLockModifyInterruptObservation,
    ) -> BluetoothSchedulerLockModifyEventPublication {
        let value = EVENT_PENDING | if observation.is_busy() { EVENT_BUSY } else { 0 };
        let previous = self.state.swap(value, Ordering::AcqRel);
        if previous & EVENT_PENDING == 0 {
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        } else {
            BluetoothSchedulerLockModifyEventPublication::Coalesced
        }
    }

    /// Remove the newest pending observation in one atomic operation.
    pub fn take(&self) -> Option<BluetoothSchedulerLockModifyEvent> {
        let state = self.state.swap(0, Ordering::AcqRel);
        if state & EVENT_PENDING == 0 {
            None
        } else {
            Some(BluetoothSchedulerLockModifyEvent {
                interrupt: BluetoothSchedulerLockModifyInterruptObservation::from_busy(
                    state & EVENT_BUSY != 0,
                ),
            })
        }
    }

    /// Whether an event is currently pending for the sole worker.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & EVENT_PENDING != 0
    }
}

impl Default for BluetoothSchedulerLockModifyEventCell {
    fn default() -> Self {
        Self::new()
    }
}

/// A validated transaction waiting for permission to publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler request has not yet been published"]
pub struct BluetoothSchedulerLockModifyAwaitingPublication {
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyAwaitingPublication {
    /// Start a pure transaction without touching MMIO.
    pub const fn new(request: BluetoothSchedulerLockModifyRequest) -> Self {
        Self { request }
    }

    /// Evaluate the pre-publication wait predicate once.
    pub const fn observe(
        self,
        observation: BluetoothSchedulerLockModifyObservation,
    ) -> BluetoothSchedulerLockModifyProgress<Self, BluetoothSchedulerLockModifyPublication> {
        if observation.wait_active() {
            BluetoothSchedulerLockModifyProgress::Waiting(self)
        } else {
            BluetoothSchedulerLockModifyProgress::Ready(BluetoothSchedulerLockModifyPublication {
                request: self.request,
            })
        }
    }
}

/// Permission for the task owner to publish one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "publication must be performed or explicitly abandoned"]
pub struct BluetoothSchedulerLockModifyPublication {
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyPublication {
    /// Consume the permission through the unique HAL task borrow and begin
    /// awaiting the publication result.
    ///
    /// This is a finite, non-polling operation. The returned state contains
    /// the PAC proof that all ordered MMIO writes and the trailing device fence
    /// completed, so a pure state transition can no longer impersonate live
    /// publication.
    pub fn publish(
        self,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> BluetoothSchedulerLockModifyInFlight {
        self.publish_with(controller)
    }

    fn publish_with(
        self,
        backend: &mut impl BluetoothSchedulerLockModifyBackend,
    ) -> BluetoothSchedulerLockModifyInFlight {
        BluetoothSchedulerLockModifyInFlight {
            _publication: backend.publish(self.request),
            request: self.request,
        }
    }
}

trait BluetoothSchedulerLockModifyBackend {
    fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation;
    fn publish(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished;
}

impl BluetoothSchedulerLockModifyBackend for BluetoothControllerHal<'_> {
    fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation {
        self.capture_scheduler_lock_modify_task()
    }

    fn publish(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        self.publish_scheduler_lock_modify(request)
    }
}

/// One published lock/modify request awaiting its publication-result event.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the in-flight scheduler request still owns its publication result"]
pub struct BluetoothSchedulerLockModifyInFlight {
    _publication: BluetoothSchedulerLockModifyPublished,
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyInFlight {
    /// Evaluate the post-publication wait predicate once.
    pub const fn observe(
        self,
        observation: BluetoothSchedulerLockModifyObservation,
    ) -> BluetoothSchedulerLockModifyProgress<Self, BluetoothSchedulerLockModifyPublicationResult>
    {
        if observation.wait_active() {
            BluetoothSchedulerLockModifyProgress::Waiting(self)
        } else {
            BluetoothSchedulerLockModifyProgress::Ready(
                BluetoothSchedulerLockModifyPublicationResult {
                    code: observation.result_code_after_publication(),
                    request: self.request,
                },
            )
        }
    }
}

/// Positional result of the reviewed lock/modify publication path.
///
/// This value only ends the request-publication transaction. Hardware radio
/// completion and descriptor ownership return occur later through the
/// scheduler finished-item and recycle path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyPublicationResult {
    code: u8,
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyPublicationResult {
    /// Construct one opaque publication result for host-side ownership tests.
    ///
    /// The test seam deliberately does not repeat the generated PAC field
    /// width or validate register geometry in hand-written controller code.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn for_validation(code: u8, request: BluetoothSchedulerLockModifyRequest) -> Self {
        Self { code, request }
    }

    /// Return the exact item identity and hardware-list index carried by this
    /// completed publication transaction.
    ///
    /// Retaining the request prevents a later scheduler stage from combining
    /// a result with a different prepared graph. It does not turn the
    /// publication result into radio completion.
    pub const fn request(self) -> BluetoothSchedulerLockModifyRequest {
        self.request
    }

    /// Return zero for an idle scheduler or the reviewed request bits 30:27.
    ///
    /// The value remains positional: its higher-level success/error meanings
    /// are not established by the current archive.
    pub const fn code(self) -> u8 {
        self.code
    }
}

/// Why a durable scheduler worker rejected a new transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerLockModifyBeginError {
    /// A request is still waiting for publication or its result.
    AlreadyInFlight,
    /// The previous result has not yet been consumed.
    ResultPending,
}

/// Result of one bounded worker event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the executor must retain the worker outcome"]
pub enum BluetoothSchedulerLockModifyWorkerStep {
    /// No transaction owns this worker.
    Idle,
    /// A completed result is waiting for its consumer; the event was ignored.
    ResultPending,
    /// The current wait predicate remains active; return to the executor.
    Waiting,
    /// This step performed the finite PAC publication and now awaits an event.
    Published,
    /// This step stored the terminal publication result.
    Ready,
}

enum BluetoothSchedulerLockModifyWorkerState {
    Idle,
    Awaiting(BluetoothSchedulerLockModifyAwaitingPublication),
    InFlight(BluetoothSchedulerLockModifyInFlight),
    Complete(BluetoothSchedulerLockModifyPublicationResult),
}

/// Durable single-request worker state owned outside any individual future.
///
/// Dropping a future cannot discard an in-flight hardware request: the worker
/// retains the affine publication proof and continues from the next event.
/// Every [`Self::step`] consumes one value-only ISR event, performs one fresh
/// task observation and returns without polling.
pub struct BluetoothSchedulerLockModifyWorker {
    state: BluetoothSchedulerLockModifyWorkerState,
}

impl BluetoothSchedulerLockModifyWorker {
    /// Construct an idle worker.
    pub const fn new() -> Self {
        Self {
            state: BluetoothSchedulerLockModifyWorkerState::Idle,
        }
    }

    /// Whether this worker owns no admitted or completed request.
    pub const fn is_idle(&self) -> bool {
        matches!(self.state, BluetoothSchedulerLockModifyWorkerState::Idle)
    }

    /// Admit one validated request without touching hardware.
    pub fn begin(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> Result<(), BluetoothSchedulerLockModifyBeginError> {
        match self.state {
            BluetoothSchedulerLockModifyWorkerState::Idle => {
                self.state = BluetoothSchedulerLockModifyWorkerState::Awaiting(
                    BluetoothSchedulerLockModifyAwaitingPublication::new(request),
                );
                Ok(())
            }
            BluetoothSchedulerLockModifyWorkerState::Complete(_) => {
                Err(BluetoothSchedulerLockModifyBeginError::ResultPending)
            }
            BluetoothSchedulerLockModifyWorkerState::Awaiting(_)
            | BluetoothSchedulerLockModifyWorkerState::InFlight(_) => {
                Err(BluetoothSchedulerLockModifyBeginError::AlreadyInFlight)
            }
        }
    }

    /// Process exactly one interrupt event through the production HAL.
    pub fn step(
        &mut self,
        event: BluetoothSchedulerLockModifyEvent,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        self.step_with(event, controller)
    }

    fn step_with(
        &mut self,
        event: BluetoothSchedulerLockModifyEvent,
        backend: &mut impl BluetoothSchedulerLockModifyBackend,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        let state = mem::replace(
            &mut self.state,
            BluetoothSchedulerLockModifyWorkerState::Idle,
        );
        match state {
            BluetoothSchedulerLockModifyWorkerState::Idle => {
                BluetoothSchedulerLockModifyWorkerStep::Idle
            }
            BluetoothSchedulerLockModifyWorkerState::Complete(result) => {
                self.state = BluetoothSchedulerLockModifyWorkerState::Complete(result);
                BluetoothSchedulerLockModifyWorkerStep::ResultPending
            }
            BluetoothSchedulerLockModifyWorkerState::Awaiting(awaiting) => {
                let observation = BluetoothSchedulerLockModifyObservation::from_split(
                    event.interrupt,
                    backend.capture_task(),
                );
                match awaiting.observe(observation) {
                    BluetoothSchedulerLockModifyProgress::Waiting(awaiting) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::Awaiting(awaiting);
                        BluetoothSchedulerLockModifyWorkerStep::Waiting
                    }
                    BluetoothSchedulerLockModifyProgress::Ready(publication) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::InFlight(
                            publication.publish_with(backend),
                        );
                        BluetoothSchedulerLockModifyWorkerStep::Published
                    }
                }
            }
            BluetoothSchedulerLockModifyWorkerState::InFlight(in_flight) => {
                let observation = BluetoothSchedulerLockModifyObservation::from_split(
                    event.interrupt,
                    backend.capture_task(),
                );
                match in_flight.observe(observation) {
                    BluetoothSchedulerLockModifyProgress::Waiting(in_flight) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::InFlight(in_flight);
                        BluetoothSchedulerLockModifyWorkerStep::Waiting
                    }
                    BluetoothSchedulerLockModifyProgress::Ready(result) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::Complete(result);
                        BluetoothSchedulerLockModifyWorkerStep::Ready
                    }
                }
            }
        }
    }

    /// Consume the stored result and make the worker idle again.
    pub fn take_result(&mut self) -> Option<BluetoothSchedulerLockModifyPublicationResult> {
        let state = mem::replace(
            &mut self.state,
            BluetoothSchedulerLockModifyWorkerState::Idle,
        );
        match state {
            BluetoothSchedulerLockModifyWorkerState::Complete(result) => Some(result),
            other => {
                self.state = other;
                None
            }
        }
    }

    /// Whether publication or its result remains owned by the worker.
    pub const fn is_active(&self) -> bool {
        !matches!(self.state, BluetoothSchedulerLockModifyWorkerState::Idle)
    }
}

impl Default for BluetoothSchedulerLockModifyWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerSramAddress, BluetoothSchedulerLockModifyInterruptObservation,
        BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
        BluetoothSchedulerLockModifyRequest, BluetoothSchedulerLockModifyTaskObservation,
    };

    use super::{
        BluetoothSchedulerLockModifyAwaitingPublication, BluetoothSchedulerLockModifyBackend,
        BluetoothSchedulerLockModifyBeginError, BluetoothSchedulerLockModifyEventCell,
        BluetoothSchedulerLockModifyEventPublication, BluetoothSchedulerLockModifyInFlight,
        BluetoothSchedulerLockModifyProgress, BluetoothSchedulerLockModifyWorker,
        BluetoothSchedulerLockModifyWorkerStep,
    };

    fn request() -> BluetoothSchedulerLockModifyRequest {
        BluetoothSchedulerLockModifyRequest::new(
            BluetoothControllerSramAddress::new(0x2f00_0040)
                .expect("test address is representable"),
            open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareListIndex::new(6)
                .expect("test list index is representable"),
        )
    }

    #[test]
    fn each_busy_edge_returns_control_to_the_executor() {
        let waiting = BluetoothSchedulerLockModifyAwaitingPublication::new(request());
        let waiting = match waiting.observe(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, true, 0),
        ) {
            BluetoothSchedulerLockModifyProgress::Waiting(waiting) => waiting,
            BluetoothSchedulerLockModifyProgress::Ready(_) => panic!("busy request advanced"),
        };

        let publication = match waiting.observe(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 0),
        ) {
            BluetoothSchedulerLockModifyProgress::Ready(publication) => publication,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => panic!("ready request stalled"),
        };
        let _publication_requires_live_hal = publication;
    }

    #[test]
    fn published_request_yields_once_per_busy_event_before_result() {
        let request = request();
        let in_flight = BluetoothSchedulerLockModifyInFlight {
            _publication: BluetoothSchedulerLockModifyPublished::for_validation(),
            request,
        };
        let in_flight = match in_flight.observe(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, true, 0),
        ) {
            BluetoothSchedulerLockModifyProgress::Waiting(in_flight) => in_flight,
            BluetoothSchedulerLockModifyProgress::Ready(_) => panic!("busy request advanced"),
        };
        let result = match in_flight.observe(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 5),
        ) {
            BluetoothSchedulerLockModifyProgress::Ready(result) => result,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => panic!("ready result stalled"),
        };

        assert_eq!(result.code(), 5);
        assert_eq!(result.request(), request);
    }

    #[test]
    fn scheduler_idle_admits_publication_without_raw_register_images() {
        let publication = match BluetoothSchedulerLockModifyAwaitingPublication::new(request())
            .observe(
                BluetoothSchedulerLockModifyObservation::from_fields_for_validation(false, true, 0),
            ) {
            BluetoothSchedulerLockModifyProgress::Ready(publication) => publication,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => panic!("idle scheduler stalled"),
        };
        let _publication_requires_live_hal = publication;
    }

    #[test]
    fn event_handoff_keeps_the_latest_level_and_reopens_the_wake_epoch() {
        let cell = BluetoothSchedulerLockModifyEventCell::new();
        assert_eq!(
            cell.publish_from_interrupt(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(true)
            ),
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        );
        assert_eq!(
            cell.publish_from_interrupt(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(false)
            ),
            BluetoothSchedulerLockModifyEventPublication::Coalesced
        );
        assert!(cell.is_pending());
        assert!(
            !cell
                .take()
                .expect("latest event must remain pending")
                .interrupt
                .is_busy()
        );
        assert!(!cell.is_pending());
        assert_eq!(
            cell.publish_from_interrupt(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(true)
            ),
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        );
    }

    struct Backend {
        observations: [BluetoothSchedulerLockModifyTaskObservation; 4],
        next_observation: usize,
        published: usize,
    }

    impl BluetoothSchedulerLockModifyBackend for Backend {
        fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation {
            let observation = self.observations[self.next_observation];
            self.next_observation += 1;
            observation
        }

        fn publish(
            &mut self,
            _request: BluetoothSchedulerLockModifyRequest,
        ) -> BluetoothSchedulerLockModifyPublished {
            self.published += 1;
            BluetoothSchedulerLockModifyPublished::for_validation()
        }
    }

    fn event(
        cell: &BluetoothSchedulerLockModifyEventCell,
        busy: bool,
    ) -> super::BluetoothSchedulerLockModifyEvent {
        let _wake = cell.publish_from_interrupt(
            BluetoothSchedulerLockModifyInterruptObservation::from_busy(busy),
        );
        cell.take().expect("published event must be available")
    }

    #[test]
    fn durable_worker_publishes_once_and_retains_result_across_event_steps() {
        let cell = BluetoothSchedulerLockModifyEventCell::new();
        let mut backend = Backend {
            observations: [
                BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(true, 0),
                BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(false, 0),
                BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(true, 0),
                BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(false, 5),
            ],
            next_observation: 0,
            published: 0,
        };
        let mut worker = BluetoothSchedulerLockModifyWorker::new();
        worker.begin(request()).expect("idle worker admits request");

        assert_eq!(
            worker.step_with(event(&cell, true), &mut backend),
            BluetoothSchedulerLockModifyWorkerStep::Waiting
        );
        assert_eq!(
            worker.step_with(event(&cell, true), &mut backend),
            BluetoothSchedulerLockModifyWorkerStep::Published
        );
        assert_eq!(backend.published, 1);
        assert_eq!(
            worker.begin(request()),
            Err(BluetoothSchedulerLockModifyBeginError::AlreadyInFlight)
        );

        assert_eq!(
            worker.step_with(event(&cell, true), &mut backend),
            BluetoothSchedulerLockModifyWorkerStep::Waiting
        );
        assert_eq!(
            worker.step_with(event(&cell, true), &mut backend),
            BluetoothSchedulerLockModifyWorkerStep::Ready
        );
        assert_eq!(
            worker.begin(request()),
            Err(BluetoothSchedulerLockModifyBeginError::ResultPending)
        );
        let result = worker.take_result().expect("result is durable");
        assert_eq!(result.code(), 5);
        assert_eq!(result.request(), request());
        assert!(!worker.is_active());
        assert_eq!(backend.published, 1);
    }
}
