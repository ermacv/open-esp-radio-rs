//! Bounded event drain for one scheduler finished-list observation.
//!
//! One step consumes at most one hardware-list bit. A caller can therefore
//! return to its executor between steps; this module has no polling loop,
//! allocator, waker or RTOS dependency.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop,
};
pub use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
};

trait BluetoothSchedulerFinishedListBackend {
    fn transfer_scheduler_finished_lists(&mut self) -> BluetoothSchedulerFinishedListObservation;
}

impl BluetoothSchedulerFinishedListBackend for BluetoothControllerHal<'_> {
    fn transfer_scheduler_finished_lists(&mut self) -> BluetoothSchedulerFinishedListObservation {
        BluetoothControllerHal::transfer_scheduler_finished_lists(self)
    }
}

/// Why a fresh task-side finished-list transfer was not admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerFinishedListCaptureError {
    /// A previous captured observation still contains unconsumed list work.
    DrainAlreadyActive,
}

/// Result of one bounded finished-list worker step.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the finished-list step must be retained or dispatched"]
pub enum BluetoothSchedulerFinishedListWorkerStep {
    /// No captured transfer is active.
    Idle,
    /// The captured observation contained no work or is fully drained.
    Complete,
    /// One hardware list requires a separately proven item-selection step.
    List {
        /// Affine proof for one list from the fenced transfer.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        /// Whether another captured list requires a later bounded step.
        more: bool,
    },
}

/// Durable task-side owner of one captured finished-list observation.
///
/// Capture performs exactly one finite PAC/HAL transfer. Every subsequent
/// [`Self::step`] consumes at most one list and returns to the executor.
pub struct BluetoothSchedulerFinishedListWorker {
    observation: Option<BluetoothSchedulerFinishedListObservation>,
}

impl BluetoothSchedulerFinishedListWorker {
    /// Construct an idle worker.
    pub const fn new() -> Self {
        Self { observation: None }
    }

    /// Capture one fresh hardware observation through the unique task-side HAL owner.
    pub fn capture(
        &mut self,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> Result<(), BluetoothSchedulerFinishedListCaptureError> {
        self.capture_with(controller)
    }

    fn capture_with(
        &mut self,
        backend: &mut impl BluetoothSchedulerFinishedListBackend,
    ) -> Result<(), BluetoothSchedulerFinishedListCaptureError> {
        if self.observation.is_some() {
            return Err(BluetoothSchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.observation = Some(backend.transfer_scheduler_finished_lists());
        Ok(())
    }

    /// Return at most one affine captured hardware-list observation.
    ///
    /// A finished list is not an item-completion proof. This worker therefore does
    /// not accept a software queue, select a descriptor or change ownership.
    /// The future completed-list owner must match `observed` to an affine
    /// hardware item and inspect the post-fence status before returning it.
    pub fn step(&mut self) -> BluetoothSchedulerFinishedListWorkerStep {
        let Some(observation) = self.observation.take() else {
            return BluetoothSchedulerFinishedListWorkerStep::Idle;
        };
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::Complete => {
                BluetoothSchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => {
                let more = !remaining.is_empty();
                if more {
                    self.observation = Some(remaining);
                }
                BluetoothSchedulerFinishedListWorkerStep::List { observed, more }
            }
        }
    }

    /// Whether a captured list remains for a later event step.
    pub const fn is_active(&self) -> bool {
        self.observation.is_some()
    }
}

impl Default for BluetoothSchedulerFinishedListWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothSchedulerFinishedListObservation;

    use super::{
        BluetoothSchedulerFinishedListBackend, BluetoothSchedulerFinishedListCaptureError,
        BluetoothSchedulerFinishedListWorker, BluetoothSchedulerFinishedListWorkerStep,
    };

    struct Backend {
        observation: Option<BluetoothSchedulerFinishedListObservation>,
    }

    impl BluetoothSchedulerFinishedListBackend for Backend {
        fn transfer_scheduler_finished_lists(
            &mut self,
        ) -> BluetoothSchedulerFinishedListObservation {
            self.observation.take().expect("one scripted transfer")
        }
    }

    fn backend(lists: &[u8]) -> Backend {
        Backend {
            observation: Some(
                BluetoothSchedulerFinishedListObservation::from_lists_for_validation(lists)
                    .expect("semantic list set is valid"),
            ),
        }
    }

    fn assert_list(
        step: BluetoothSchedulerFinishedListWorkerStep,
        expected_index: u8,
        expected_more: bool,
    ) {
        match step {
            BluetoothSchedulerFinishedListWorkerStep::List { observed, more } => {
                assert_eq!(observed.index().get(), expected_index);
                assert_eq!(more, expected_more);
            }
            _ => panic!("the scripted observation must yield one list"),
        }
    }

    #[test]
    fn multiple_lists_are_drained_lowest_first_one_per_step() {
        let mut backend = backend(&[9, 3]);
        let mut worker = BluetoothSchedulerFinishedListWorker::new();

        worker
            .capture_with(&mut backend)
            .expect("idle worker accepts one transfer");
        assert_eq!(
            worker.capture_with(&mut backend),
            Err(BluetoothSchedulerFinishedListCaptureError::DrainAlreadyActive)
        );
        assert_list(worker.step(), 3, true);
        assert!(worker.is_active());
        assert_list(worker.step(), 9, false);
        assert!(!worker.is_active());
        assert_eq!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::Idle
        );
    }

    #[test]
    fn single_list_exhausts_the_capture() {
        let mut backend = backend(&[3]);
        let mut worker = BluetoothSchedulerFinishedListWorker::new();

        worker.capture_with(&mut backend).unwrap();
        assert_list(worker.step(), 3, false);
        assert!(!worker.is_active());
        assert_eq!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::Idle
        );
    }

    #[test]
    fn list_zero_precedes_an_unowned_list_without_losing_the_capture() {
        let mut backend = backend(&[0, 3]);
        let mut worker = BluetoothSchedulerFinishedListWorker::new();

        worker.capture_with(&mut backend).unwrap();
        assert_list(worker.step(), 0, true);
        assert!(worker.is_active());
        assert_list(worker.step(), 3, false);
        assert!(!worker.is_active());
    }

    #[test]
    fn sole_list_zero_exhausts_the_capture() {
        let mut backend = backend(&[0]);
        let mut worker = BluetoothSchedulerFinishedListWorker::new();

        worker.capture_with(&mut backend).unwrap();
        assert_list(worker.step(), 0, false);
        assert!(!worker.is_active());
        assert_eq!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::Idle
        );
    }
}
