//! Bounded event drain for one scheduler finished-list observation.
//!
//! One step consumes at most one hardware-list bit. A caller can therefore
//! return to its executor between steps; this module has no polling loop,
//! allocator, waker or RTOS dependency.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_hal::BluetoothSchedulerFinishedListIndex;
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop,
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
        /// Positional hardware list observed in the transferred set.
        index: BluetoothSchedulerFinishedListIndex,
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

    /// Return at most one captured hardware-list index.
    ///
    /// A finished list is not an item-completion proof. This worker therefore does
    /// not accept a software queue, select a descriptor or change ownership.
    /// The future completed-list owner must map `index` to an affine hardware
    /// item and establish device-to-CPU visibility independently.
    pub fn step(&mut self) -> BluetoothSchedulerFinishedListWorkerStep {
        let Some(observation) = self.observation.take() else {
            return BluetoothSchedulerFinishedListWorkerStep::Idle;
        };
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::Complete => {
                BluetoothSchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListPop::List { index, remaining } => {
                let more = !remaining.is_empty();
                if more {
                    self.observation = Some(remaining);
                }
                BluetoothSchedulerFinishedListWorkerStep::List { index, more }
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

    #[test]
    fn captured_mask_yields_only_list_indices_without_item_ownership() {
        let mut backend = Backend {
            observation: Some(
                BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[3, 9])
                    .expect("semantic list set is valid"),
            ),
        };
        let mut worker = BluetoothSchedulerFinishedListWorker::new();

        worker
            .capture_with(&mut backend)
            .expect("idle worker accepts one transfer");
        assert_eq!(
            worker.capture_with(&mut backend),
            Err(BluetoothSchedulerFinishedListCaptureError::DrainAlreadyActive)
        );
        assert!(matches!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::List { index, more: true }
                if index.get() == 3
        ));
        assert!(worker.is_active());
        assert!(matches!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::List { index, more: false }
                if index.get() == 9
        ));
        assert!(!worker.is_active());
        assert_eq!(
            worker.step(),
            BluetoothSchedulerFinishedListWorkerStep::Idle
        );
    }
}
