//! Bounded event drain for one scheduler finished-list observation.
//!
//! One step consumes at most one hardware-list bit. A caller can therefore
//! return to its executor between steps; this module has no polling loop,
//! allocator, waker or RTOS dependency.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::BluetoothSchedulerFinishedListObservation;

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

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothSchedulerFinishedListObservation;

    use super::{BluetoothSchedulerFinishedListDrain, BluetoothSchedulerFinishedListDrainStep};

    #[test]
    fn one_step_consumes_only_the_lowest_finished_list() {
        let drain = BluetoothSchedulerFinishedListDrain::new(
            BluetoothSchedulerFinishedListObservation::from_bits(0x8084),
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
            BluetoothSchedulerFinishedListObservation::from_bits(0),
        );

        assert_eq!(
            drain.step(),
            BluetoothSchedulerFinishedListDrainStep::Complete
        );
    }
}
