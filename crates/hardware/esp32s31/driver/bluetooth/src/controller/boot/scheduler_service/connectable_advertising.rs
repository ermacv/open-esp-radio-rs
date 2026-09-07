//! Atomic publication of one response-capable legacy advertising event.
//!
//! Every fallible precondition is resolved while the graph remains CPU-owned.
//! After the first MMIO write, an unexpected proof disagreement seals the
//! complete controller and hardware-owned graph rather than offering rollback.

use crate::{
    le::advertising::connectable::{
        LegacyConnectableAdvertisingPublicationRemainder,
        completion::LegacyConnectableAdvertisingCompletionRole,
    },
    scheduler::{
        SchedulerHeadPublicationError,
        core::{
            LegacyConnectableAdvertisingEmptySchedulerMergePrepared, SingleItemSchedulerRunning,
        },
    },
};

use oer_esp32s31_bluetooth_memory::{
    LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
    LegacyConnectableAdvertisingMemoryGraphPublicationError,
    LegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
    LegacyConnectableAdvertisingMemoryGraphRunMismatch,
    LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
};

use super::super::{
    BluetoothSchedulerRunInterruptsPrepared, ControllerPublishedTaskService,
    SchedulerRunInterruptStorage,
};

/// A start rejection that happened before any MMIO publication.
#[must_use = "the exact pre-publication owner remains retryable or cancellable"]
pub(crate) struct LegacyConnectableAdvertisingSchedulerStartRetry<
    'runtime,
    S,
    E,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    error: LegacyConnectableAdvertisingSchedulerStartRetryError<E>,
}

/// Finite reason an atomic start did not enter its MMIO suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingSchedulerStartRetryError<E> {
    Head(SchedulerHeadPublicationError),
    Interrupts(E),
}

impl<'runtime, S, E, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingSchedulerStartRetry<'runtime, S, E, SCHEDULER_CAPACITY>
{
    pub(crate) const fn error(&self) -> &LegacyConnectableAdvertisingSchedulerStartRetryError<E> {
        &self.error
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        LegacyConnectableAdvertisingSchedulerStartRetryError<E>,
    ) {
        (self.controller, self.merged, self.error)
    }
}

/// Observable reason the non-rollback publication suffix stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingSchedulerFailStopCause {
    ReceivePublication(LegacyConnectableAdvertisingMemoryGraphPublicationError),
    SchedulerHead(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
    SchedulerRun(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
}

/// Complete sealed owner after an impossible post-MMIO proof disagreement.
///
/// Deliberately no API can recover the task service or graph from this state:
/// selector/head/RUN visibility has no reviewed rollback transaction.
#[must_use = "the post-MMIO controller and graph remain permanently fail-stop owned"]
pub(crate) struct LegacyConnectableAdvertisingSchedulerFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    _controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ownership: LegacyConnectableAdvertisingSchedulerFailStopOwnership,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn cause(&self) -> LegacyConnectableAdvertisingSchedulerFailStopCause {
        match &self.ownership {
            LegacyConnectableAdvertisingSchedulerFailStopOwnership::ReceivePublication {
                mismatch,
                ..
            } => LegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(
                mismatch.error(),
            ),
            LegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerHead {
                mismatch,
                ..
            } => {
                LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(mismatch.error())
            }
            LegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerRun {
                mismatch,
                ..
            } => LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(mismatch.error()),
        }
    }
}

enum LegacyConnectableAdvertisingSchedulerFailStopOwnership {
    ReceivePublication {
        mismatch: LegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
        _remainder: LegacyConnectableAdvertisingPublicationRemainder,
        _head: BluetoothSchedulerHardwareListHead,
        _interrupts: BluetoothSchedulerRunInterruptsPrepared,
    },
    SchedulerHead {
        mismatch: LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
        _publication: BluetoothSchedulerHardwareListHeadPublished,
        _remainder: LegacyConnectableAdvertisingPublicationRemainder,
        _interrupts: BluetoothSchedulerRunInterruptsPrepared,
    },
    SchedulerRun {
        mismatch: LegacyConnectableAdvertisingMemoryGraphRunMismatch,
        _remainder: LegacyConnectableAdvertisingPublicationRemainder,
        _run: BluetoothSchedulerHardwareRunCommandPublished,
    },
}

/// Result of consuming a complete connectable start owner.
#[must_use = "the returned controller owner must remain in exactly one live state"]
pub(crate) enum LegacyConnectableAdvertisingSchedulerStartStep<
    'runtime,
    S,
    E,
    const SCHEDULER_CAPACITY: usize,
> {
    Running {
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: SingleItemSchedulerRunning<LegacyConnectableAdvertisingCompletionRole>,
    },
    Retryable {
        failure:
            LegacyConnectableAdvertisingSchedulerStartRetry<'runtime, S, E, SCHEDULER_CAPACITY>,
    },
    FailStop(LegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Validate every recoverable condition, then publish RX, HEAD, event and RUN.
    #[allow(
        unsafe_code,
        reason = "this consuming boundary retains the pinned graph and sole task owner across the non-rollback MMIO suffix"
    )]
    pub(crate) fn start_legacy_connectable_advertising_scheduler(
        mut self,
        merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> LegacyConnectableAdvertisingSchedulerStartStep<'runtime, S, S::Error, SCHEDULER_CAPACITY>
    {
        let address = merged.scheduler_item_address();
        let head = match self.runtime.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return LegacyConnectableAdvertisingSchedulerStartStep::Retryable {
                    failure: LegacyConnectableAdvertisingSchedulerStartRetry {
                        controller: self,
                        merged,
                        error: LegacyConnectableAdvertisingSchedulerStartRetryError::Head(error),
                    },
                };
            }
        };
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return LegacyConnectableAdvertisingSchedulerStartStep::Retryable {
                    failure: LegacyConnectableAdvertisingSchedulerStartRetry {
                        controller: self,
                        merged,
                        error: LegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(
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
                return LegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    LegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            LegacyConnectableAdvertisingSchedulerFailStopOwnership::ReceivePublication {
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
                return LegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    LegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            LegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerHead {
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
                return LegacyConnectableAdvertisingSchedulerStartStep::FailStop(
                    LegacyConnectableAdvertisingSchedulerFailStop {
                        _controller: self,
                        ownership:
                            LegacyConnectableAdvertisingSchedulerFailStopOwnership::SchedulerRun {
                                mismatch,
                                _remainder: remainder,
                                _run: run,
                            },
                    },
                );
            }
        };
        let running =
            SingleItemSchedulerRunning::new(remainder.into_running(memory), run, reservation);
        LegacyConnectableAdvertisingSchedulerStartStep::Running {
            controller: self,
            running,
        }
    }
}
