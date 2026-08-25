//! Event-driven controller model for one scheduler lock/modify transaction.
//!
//! Every transition consumes a fresh cross-owner observation. `Waiting` means
//! the caller should return to its executor and resume only after an interrupt
//! or another controller event supplies a new observation; no path spins.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::{
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyRequest,
};

/// Result of evaluating one fresh scheduler event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "waiting state or completed transition must remain owned"]
pub enum BluetoothSchedulerLockModifyProgress<W, R> {
    /// Hardware still has both BUSY and START set.
    Waiting(W),
    /// The current phase can advance without polling.
    Ready(R),
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

/// Permission and exact images for the task owner to publish one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "publication must be performed or explicitly abandoned"]
pub struct BluetoothSchedulerLockModifyPublication {
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyPublication {
    /// First `OPERATIONAL_WORD_036C` image: clear the low nibble.
    pub const fn argument_clear_image(self, first_fresh_read: u32) -> u32 {
        self.request.argument_clear_image(first_fresh_read)
    }

    /// Second `OPERATIONAL_WORD_036C` image: OR the four-bit argument.
    pub const fn argument_image(self, second_fresh_read: u32) -> u32 {
        self.request.argument_image(second_fresh_read)
    }

    /// Complete `SCHEDULER_LOCK_MODIFY_REQUEST` publication image.
    pub const fn request_image(self) -> u32 {
        self.request.publication_image()
    }

    /// Record that the ordered writes completed and begin awaiting hardware.
    ///
    /// A future live task-owner method will perform the MMIO writes and consume
    /// this token internally. Keeping the transition explicit prevents a
    /// caller from interpreting the pre-publication observation as completion.
    pub const fn published(self) -> BluetoothSchedulerLockModifyInFlight {
        BluetoothSchedulerLockModifyInFlight { _private: () }
    }
}

/// One published lock/modify request awaiting its publication-result event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the in-flight scheduler request still owns its publication result"]
pub struct BluetoothSchedulerLockModifyInFlight {
    _private: (),
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
}

impl BluetoothSchedulerLockModifyPublicationResult {
    /// Return zero for an idle scheduler or the reviewed request bits 30:27.
    ///
    /// The value remains positional: its higher-level success/error meanings
    /// are not established by the current archive.
    pub const fn code(self) -> u8 {
        self.code
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerSramAddress, BluetoothSchedulerLockModifyObservation,
        BluetoothSchedulerLockModifyRequest,
    };

    use super::{
        BluetoothSchedulerLockModifyAwaitingPublication, BluetoothSchedulerLockModifyProgress,
    };

    fn request() -> BluetoothSchedulerLockModifyRequest {
        BluetoothSchedulerLockModifyRequest::new(
            BluetoothControllerSramAddress::new(0x2f00_0040)
                .expect("test address is representable"),
            6,
        )
        .expect("test argument fits")
    }

    #[test]
    fn each_busy_edge_returns_control_to_the_executor() {
        let waiting = BluetoothSchedulerLockModifyAwaitingPublication::new(request());
        let waiting = match waiting.observe(
            BluetoothSchedulerLockModifyObservation::from_registers(1 << 31, 1 << 31),
        ) {
            BluetoothSchedulerLockModifyProgress::Waiting(waiting) => waiting,
            BluetoothSchedulerLockModifyProgress::Ready(_) => panic!("busy request advanced"),
        };

        let publication = match waiting.observe(
            BluetoothSchedulerLockModifyObservation::from_registers(1 << 31, 0),
        ) {
            BluetoothSchedulerLockModifyProgress::Ready(publication) => publication,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => panic!("ready request stalled"),
        };
        assert_eq!(publication.argument_clear_image(0xffff_ffff), 0xffff_fff0);
        assert_eq!(publication.argument_image(0xffff_fff0), 0xffff_fff6);
        assert_eq!(publication.request_image(), 0x8000_0010);

        let in_flight = publication.published();
        let in_flight = match in_flight.observe(
            BluetoothSchedulerLockModifyObservation::from_registers(1 << 31, 1 << 31),
        ) {
            BluetoothSchedulerLockModifyProgress::Waiting(in_flight) => in_flight,
            BluetoothSchedulerLockModifyProgress::Ready(_) => panic!("in-flight wait skipped"),
        };

        let result = match in_flight.observe(
            BluetoothSchedulerLockModifyObservation::from_registers(1 << 31, 0x2800_0000),
        ) {
            BluetoothSchedulerLockModifyProgress::Ready(result) => result,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => {
                panic!("publication result stalled")
            }
        };
        assert_eq!(result.code(), 5);
    }

    #[test]
    fn scheduler_idle_forces_zero_publication_result_code() {
        let publication = match BluetoothSchedulerLockModifyAwaitingPublication::new(request())
            .observe(BluetoothSchedulerLockModifyObservation::from_registers(
                0,
                1 << 31,
            )) {
            BluetoothSchedulerLockModifyProgress::Ready(publication) => publication,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => panic!("idle scheduler stalled"),
        };
        let result = match publication.published().observe(
            BluetoothSchedulerLockModifyObservation::from_registers(0, 0x7800_0000),
        ) {
            BluetoothSchedulerLockModifyProgress::Ready(result) => result,
            BluetoothSchedulerLockModifyProgress::Waiting(_) => {
                panic!("idle publication result stalled")
            }
        };
        assert_eq!(result.code(), 0);
    }
}
