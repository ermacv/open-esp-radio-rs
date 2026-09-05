//! Atomic publication of one response-capable legacy advertising event.
//!
//! Every fallible precondition is resolved while the graph remains CPU-owned.
//! After the first MMIO write, an unexpected proof disagreement seals the
//! complete controller and hardware-owned graph rather than offering rollback.

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
    BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch,
    BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
};

use super::super::{
    BluetoothControllerPublishedTaskService, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerRunInterruptsPrepared,
};
use crate::{
    BluetoothSchedulerHeadPublicationError,
    connectable_advertising::BluetoothLegacyConnectableAdvertisingPublicationRemainder,
    legacy_connectable_advertising_completion::BluetoothLegacyConnectableAdvertisingCompletionRole,
    scheduler::core::{
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothSingleItemSchedulerRunning,
    },
};

/// A start rejection that happened before any MMIO publication.
#[must_use = "the exact pre-publication owner remains retryable or cancellable"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingSchedulerStartRetry<
    'runtime,
    S,
    E,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    error: BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError<E>,
}

/// Finite reason an atomic start did not enter its MMIO suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError<E> {
    Head(BluetoothSchedulerHeadPublicationError),
    Interrupts(E),
}

impl<'runtime, S, E, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingSchedulerStartRetry<'runtime, S, E, SCHEDULER_CAPACITY>
{
    pub(crate) const fn error(
        &self,
    ) -> &BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError<E> {
        &self.error
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError<E>,
    ) {
        (self.controller, self.merged, self.error)
    }
}

/// Observable reason the non-rollback publication suffix stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause {
    ReceivePublication(BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError),
    SchedulerHead(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
    SchedulerRun(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
}

/// Complete sealed owner after an impossible post-MMIO proof disagreement.
///
/// Deliberately no API can recover the task service or graph from this state:
/// selector/head/RUN visibility has no reviewed rollback transaction.
#[must_use = "the post-MMIO controller and graph remain permanently fail-stop owned"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingSchedulerFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    _controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ownership: BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn cause(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause {
        match &self.ownership {
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::ReceivePublication {
                mismatch,
                ..
            } => BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(
                mismatch.error(),
            ),
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerHead {
                mismatch,
                ..
            } => BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(
                mismatch.error(),
            ),
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerRun {
                mismatch,
                ..
            } => BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(
                mismatch.error(),
            ),
        }
    }
}

enum BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership {
    ReceivePublication {
        mismatch: BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
        _remainder: BluetoothLegacyConnectableAdvertisingPublicationRemainder,
        _head: BluetoothSchedulerHardwareListHead,
        _interrupts: BluetoothSchedulerRunInterruptsPrepared,
    },
    SchedulerHead {
        mismatch: BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
        _publication: BluetoothSchedulerHardwareListHeadPublished,
        _remainder: BluetoothLegacyConnectableAdvertisingPublicationRemainder,
        _interrupts: BluetoothSchedulerRunInterruptsPrepared,
    },
    SchedulerRun {
        mismatch: BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch,
        _remainder: BluetoothLegacyConnectableAdvertisingPublicationRemainder,
        _run: BluetoothSchedulerHardwareRunCommandPublished,
    },
}

/// Result of consuming a complete connectable start owner.
#[must_use = "the returned controller owner must remain in exactly one live state"]
pub(crate) enum BluetoothLegacyConnectableAdvertisingSchedulerStartStep<
    'runtime,
    S,
    E,
    const SCHEDULER_CAPACITY: usize,
> {
    Running {
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothSingleItemSchedulerRunning<
            BluetoothLegacyConnectableAdvertisingCompletionRole,
        >,
    },
    Retryable {
        failure: BluetoothLegacyConnectableAdvertisingSchedulerStartRetry<
            'runtime,
            S,
            E,
            SCHEDULER_CAPACITY,
        >,
    },
    FailStop(
        BluetoothLegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Validate every recoverable condition, then publish RX, HEAD, event and RUN.
    #[allow(
        unsafe_code,
        reason = "this consuming boundary retains the pinned graph and sole task owner across the non-rollback MMIO suffix"
    )]
    pub(crate) fn start_legacy_connectable_advertising_scheduler(
        mut self,
        merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerStartStep<
        'runtime,
        S,
        S::Error,
        SCHEDULER_CAPACITY,
    > {
        let address = merged.scheduler_item_address();
        let head = match self.runtime.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Retryable {
                    failure: BluetoothLegacyConnectableAdvertisingSchedulerStartRetry {
                        controller: self,
                        merged,
                        error: BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Head(
                            error,
                        ),
                    },
                };
            }
        };
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Retryable {
                    failure: BluetoothLegacyConnectableAdvertisingSchedulerStartRetry {
                        controller: self,
                        merged,
                        error:
                            BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(
                                error,
                            ),
                    },
                };
            }
        };

        // No operation below this point may be retried or rolled back.
        let prepared = merged.prepare_publication();
        let random_address = prepared.random_address();
        let (item, reservation) = prepared.into_parts();
        let (memory, remainder) = item.into_parts();

        if let Some(random_address) = random_address {
            self.runtime
                .task
                .program_random_device_address_while_idle(random_address);
        }
        let memory = match unsafe {
            self.runtime
                .task
                .publish_legacy_connectable_advertising_rx_memory(memory)
        } {
            Ok(memory) => memory,
            Err(mismatch) => {
                return BluetoothLegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::ReceivePublication {
                                mismatch,
                                _remainder: remainder,
                                _head: head,
                                _interrupts: interrupts,
                            },
                    },
                );
            }
        };
        let publication = self.runtime.publish_validated_first_scheduler_item_head(
            address,
            BluetoothSchedulerHardwareListIndex::ZERO,
            head,
        );
        let memory = match memory.into_head_published(&publication) {
            Ok(memory) => memory,
            Err(mismatch) => {
                return BluetoothLegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerHead {
                                mismatch,
                                _publication: publication,
                                _remainder: remainder,
                                _interrupts: interrupts,
                            },
                    },
                );
            }
        };
        let event = self
            .runtime
            .publish_scheduler_run_event(publication, interrupts);
        let run = self.runtime.publish_scheduler_hardware_run_command(event);
        self.runtime.retain_running_first_item(address);
        let memory = match memory.into_running(&run) {
            Ok(memory) => memory,
            Err(mismatch) => {
                return BluetoothLegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            BluetoothLegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerRun {
                                mismatch,
                                _remainder: remainder,
                                _run: run,
                            },
                    },
                );
            }
        };
        let running = BluetoothSingleItemSchedulerRunning::new(
            remainder.into_running(memory),
            run,
            reservation,
        );
        BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Running {
            controller: self,
            running,
        }
    }
}
