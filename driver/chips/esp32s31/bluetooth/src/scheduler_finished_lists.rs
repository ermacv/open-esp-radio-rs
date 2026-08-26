//! Bounded event drain for one scheduler finished-list observation.
//!
//! One step consumes at most one hardware-list bit. A caller can therefore
//! return to its executor between steps; this module has no polling loop,
//! allocator, waker or RTOS dependency.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothSchedulerFinishedListObservation,
};

/// One of the sixteen scheduler hardware-list indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerFinishedListIndex(u8);

impl BluetoothSchedulerFinishedListIndex {
    /// Return the zero-based index in `0..16`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Remaining finite work from one sampled finished-list mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "every finished-list bit must be drained or explicitly abandoned"]
pub struct BluetoothSchedulerFinishedListDrain {
    remaining: u16,
}

impl BluetoothSchedulerFinishedListDrain {
    /// Begin draining one already transferred hardware observation.
    pub const fn new(observation: BluetoothSchedulerFinishedListObservation) -> Self {
        Self {
            remaining: observation.bits(),
        }
    }

    /// Whether every captured list bit has been consumed.
    const fn is_empty(self) -> bool {
        self.remaining == 0
    }

    /// Consume at most one lowest-numbered finished-list bit.
    pub const fn step(self) -> BluetoothSchedulerFinishedListDrainStep {
        if self.remaining == 0 {
            BluetoothSchedulerFinishedListDrainStep::Complete
        } else {
            let index = self.remaining.trailing_zeros() as u8;
            let remaining = self.remaining & !(1u16 << index);
            BluetoothSchedulerFinishedListDrainStep::List {
                index: BluetoothSchedulerFinishedListIndex(index),
                remaining: Self { remaining },
            }
        }
    }
}

/// Result of one bounded finished-list drain step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the completed or remaining drain state must be handled"]
pub enum BluetoothSchedulerFinishedListDrainStep {
    /// Every bit from the sampled mask has been consumed.
    Complete,
    /// One list index is ready for item selection; more mask state is retained.
    List {
        /// Lowest numbered list selected in this step.
        index: BluetoothSchedulerFinishedListIndex,
        /// Remaining finite work after consuming `index`.
        remaining: BluetoothSchedulerFinishedListDrain,
    },
}

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
    /// A previous captured mask still contains unconsumed list work.
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
        /// Positional hardware list observed in the transferred mask.
        index: BluetoothSchedulerFinishedListIndex,
        /// Whether another captured list bit requires a later bounded step.
        more: bool,
    },
}

/// Durable task-side owner of one captured finished-list mask.
///
/// Capture performs exactly one finite PAC/HAL transfer. Every subsequent
/// [`Self::step`] consumes at most one list bit and returns to the executor.
pub struct BluetoothSchedulerFinishedListWorker {
    drain: Option<BluetoothSchedulerFinishedListDrain>,
}

impl BluetoothSchedulerFinishedListWorker {
    /// Construct an idle worker.
    pub const fn new() -> Self {
        Self { drain: None }
    }

    /// Capture one fresh hardware mask through the unique task-side HAL owner.
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
        if self.drain.is_some() {
            return Err(BluetoothSchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.drain = Some(BluetoothSchedulerFinishedListDrain::new(
            backend.transfer_scheduler_finished_lists(),
        ));
        Ok(())
    }

    /// Return at most one captured hardware-list index.
    ///
    /// A list bit is not an item-completion proof. This worker therefore does
    /// not accept a software queue, select a descriptor or change ownership.
    /// The future completed-list owner must map `index` to an affine hardware
    /// item and establish device-to-CPU visibility independently.
    pub fn step(&mut self) -> BluetoothSchedulerFinishedListWorkerStep {
        let Some(drain) = self.drain.take() else {
            return BluetoothSchedulerFinishedListWorkerStep::Idle;
        };
        match drain.step() {
            BluetoothSchedulerFinishedListDrainStep::Complete => {
                BluetoothSchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListDrainStep::List { index, remaining } => {
                let more = !remaining.is_empty();
                if more {
                    self.drain = Some(remaining);
                }
                BluetoothSchedulerFinishedListWorkerStep::List { index, more }
            }
        }
    }

    /// Whether a captured list bit remains for a later event step.
    pub const fn is_active(&self) -> bool {
        self.drain.is_some()
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
        BluetoothSchedulerFinishedListDrain, BluetoothSchedulerFinishedListDrainStep,
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
    fn one_step_consumes_only_the_lowest_finished_list() {
        let drain = BluetoothSchedulerFinishedListDrain::new(
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[2, 7, 15])
                .expect("semantic list set is valid"),
        );

        let (index, drain) = match drain.step() {
            BluetoothSchedulerFinishedListDrainStep::List { index, remaining } => {
                (index, remaining)
            }
            BluetoothSchedulerFinishedListDrainStep::Complete => panic!("nonempty mask vanished"),
        };
        assert_eq!(index.get(), 2);

        let (index, drain) = match drain.step() {
            BluetoothSchedulerFinishedListDrainStep::List { index, remaining } => {
                (index, remaining)
            }
            BluetoothSchedulerFinishedListDrainStep::Complete => panic!("second bit vanished"),
        };
        assert_eq!(index.get(), 7);

        let (index, drain) = match drain.step() {
            BluetoothSchedulerFinishedListDrainStep::List { index, remaining } => {
                (index, remaining)
            }
            BluetoothSchedulerFinishedListDrainStep::Complete => panic!("last bit vanished"),
        };
        assert_eq!(index.get(), 15);
        assert_eq!(
            drain.step(),
            BluetoothSchedulerFinishedListDrainStep::Complete
        );
    }

    #[test]
    fn empty_observation_completes_without_synthetic_work() {
        let drain = BluetoothSchedulerFinishedListDrain::new(
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[])
                .expect("empty semantic set is valid"),
        );

        assert_eq!(
            drain.step(),
            BluetoothSchedulerFinishedListDrainStep::Complete
        );
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
