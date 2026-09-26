//! List-zero publications through the task HAL owner and the stable
//! interrupt owner.

use oer_esp32s31_bluetooth_memory::ControllerSramLinkAddress;
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerCancellationIndexed, BluetoothSchedulerCancellationRequested,
    BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerExecutionLockPublished,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyPublished,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerInsertionCommand,
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
    BluetoothSchedulerLockModifyRequest, BluetoothSchedulerSkipPublished,
    BluetoothSchedulerSkipRequest, ControllerHal,
};

use super::{
    SchedulerHardwareBackend, SchedulerHardwareError, SchedulerPublications, SchedulerStartError,
};
use crate::scheduler::{SchedulerObservation, SchedulerRunInterruptStorage, SchedulerWait};

const LIST: BluetoothSchedulerHardwareListIndex = BluetoothSchedulerHardwareListIndex::ZERO;

/// The HAL proofs of the list-zero publications.
pub(crate) enum HalPublications {}

impl SchedulerPublications for HalPublications {
    type Lock = BluetoothSchedulerExecutionLockPublished;
    type Modify = BluetoothSchedulerExecutionModifyPublished;
    type LockModify = BluetoothSchedulerLockModifyPublished;
    type Indexed = BluetoothSchedulerCancellationIndexed;
    type Acknowledged = BluetoothSchedulerCancellationSourceAcknowledged;
    type Cancellation = BluetoothSchedulerCancellationRequested;
    type Skip = BluetoothSchedulerSkipPublished;
    type Run = BluetoothSchedulerHardwareRunCommandPublished;
}

/// The task HAL owner and the interrupt-owner storage for one call.
///
/// Every address it publishes was resolved against the scheduler item space
/// by the powered task runtime: it names a submitted item of a pool whose
/// storage is bound for `'static`.
pub(crate) struct LiveSchedulerBackend<'call, 'registers, S> {
    pub(crate) controller: &'call mut ControllerHal<'registers>,
    pub(crate) storage: &'call S,
}

fn head(at: ControllerSramLinkAddress) -> BluetoothSchedulerHardwareListHead {
    BluetoothSchedulerHardwareListHead::from_address(at.controller_address())
        .expect("a controller link never encodes the empty head")
}

#[allow(
    unsafe_code,
    reason = "the powered task runtime resolved every address to 'static pool storage and serializes list zero"
)]
impl<S: SchedulerRunInterruptStorage> SchedulerHardwareBackend for LiveSchedulerBackend<'_, '_, S> {
    type Publications = HalPublications;
    type StartError = S::Error;

    fn publish_execution_lock(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> BluetoothSchedulerExecutionLockPublished {
        // SAFETY: `at` names a submitted item in 'static pool storage, and
        // the executor keeps it listed until the lock is released.
        unsafe {
            self.controller.publish_scheduler_execution_lock(
                BluetoothSchedulerExecutionLockRequest::new(at.controller_address(), LIST),
            )
        }
    }

    fn release_execution_lock(&mut self, _lock: BluetoothSchedulerExecutionLockPublished) {
        // SAFETY: the consumed proof is the insertion result this clear ends,
        // and the task runtime serializes the command word.
        let _cleared = unsafe {
            self.controller
                .clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::Zero)
        };
    }

    fn publish_execution_modify(
        &mut self,
        list_deletion: bool,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        // SAFETY: the executor owns list zero's reconciliation or deletion
        // until it releases execution modify.
        unsafe {
            if list_deletion {
                self.controller
                    .publish_scheduler_execution_modify_list_deletion(LIST)
            } else {
                self.controller.publish_scheduler_execution_modify(LIST)
            }
        }
    }

    fn release_execution_modify(&mut self, _modify: BluetoothSchedulerExecutionModifyPublished) {
        // SAFETY: the consumed proof is the command this clear ends, and the
        // task runtime serializes the command word.
        let _cleared = unsafe {
            self.controller
                .clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::One)
        };
    }

    fn publish_lock_modify(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> BluetoothSchedulerLockModifyPublished {
        // SAFETY: `at` names the newly linked item in 'static pool storage;
        // the executor holds list zero until the request leaves START.
        unsafe {
            self.controller
                .publish_scheduler_lock_modify(BluetoothSchedulerLockModifyRequest::new(
                    at.controller_address(),
                    LIST,
                ))
        }
    }

    fn publish_head(&mut self, at: Option<ControllerSramLinkAddress>) {
        let head = at.map_or(BluetoothSchedulerHardwareListHead::empty(), head);
        // SAFETY: the executor finished every descriptor write of the list it
        // publishes; each item lives in 'static pool storage.
        let _published = unsafe {
            self.controller
                .publish_scheduler_hardware_list_head(LIST, head)
        };
    }

    fn index_cancellation(&mut self) -> BluetoothSchedulerCancellationIndexed {
        // SAFETY: the executor observed the lock-modify request idle and holds
        // list zero until it releases the cancellation.
        unsafe { self.controller.index_scheduler_cancellation(LIST) }
    }

    fn acknowledge_cancellation_source(
        &mut self,
        indexed: &BluetoothSchedulerCancellationIndexed,
    ) -> Result<BluetoothSchedulerCancellationSourceAcknowledged, SchedulerHardwareError> {
        self.storage
            .with_interrupt_registers(indexed, |interrupts, indexed| {
                interrupts.acknowledge_scheduler_cancellation_source(indexed)
            })
            .map_err(|_| SchedulerHardwareError::InterruptOwnerUnavailable)
    }

    fn request_cancellation(
        &mut self,
        indexed: BluetoothSchedulerCancellationIndexed,
        acknowledged: BluetoothSchedulerCancellationSourceAcknowledged,
    ) -> BluetoothSchedulerCancellationRequested {
        self.controller
            .request_scheduler_cancellation(indexed, acknowledged)
    }

    fn release_cancellation(&mut self, cancellation: BluetoothSchedulerCancellationRequested) {
        let _released = self.controller.release_scheduler_cancellation(cancellation);
    }

    fn publish_skip(&mut self, at: ControllerSramLinkAddress) -> BluetoothSchedulerSkipPublished {
        // SAFETY: `at` names a detached item that its pool keeps submitted in
        // 'static storage until the cancellation releases it.
        unsafe {
            self.controller
                .publish_scheduler_skip(BluetoothSchedulerSkipRequest::new(
                    at.controller_address(),
                    LIST,
                ))
        }
    }

    fn clear_skip(&mut self, skip: BluetoothSchedulerSkipPublished) {
        let _cleared = self.controller.clear_scheduler_skip(skip);
    }

    fn observe(
        &mut self,
        wait: SchedulerWait,
        cancellation: Option<&mut BluetoothSchedulerCancellationRequested>,
        skip: Option<&BluetoothSchedulerSkipPublished>,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        if wait == SchedulerWait::LockModify {
            let interrupt = self
                .storage
                .with_interrupt_registers((), |interrupts, ()| {
                    interrupts.capture_scheduler_lock_modify_interrupt()
                })
                .map_err(|()| SchedulerHardwareError::InterruptOwnerUnavailable)?;
            let task = self.controller.capture_scheduler_lock_modify_task();
            return Ok(SchedulerObservation::LockModify(
                BluetoothSchedulerLockModifyObservation::from_split(interrupt, task),
            ));
        }
        let work = self
            .storage
            .with_interrupt_registers((), |interrupts, ()| interrupts.capture_scheduler_work())
            .map_err(|()| SchedulerHardwareError::InterruptOwnerUnavailable)?;
        Ok(match (wait, cancellation, skip) {
            (SchedulerWait::ExecutionLock, _, _) => SchedulerObservation::ExecutionLock(
                self.controller.observe_scheduler_execution_lock(work),
            ),
            (SchedulerWait::ExecutionModify, _, _) => SchedulerObservation::ExecutionModify(
                self.controller.observe_scheduler_execution_modify(work),
            ),
            (SchedulerWait::Cancellation, Some(requested), _) => {
                SchedulerObservation::Cancellation(
                    self.controller
                        .observe_scheduler_cancellation(requested, work),
                )
            }
            (SchedulerWait::Skip, _, Some(published)) => {
                SchedulerObservation::Skip(self.controller.observe_scheduler_skip(published, work))
            }
            _ => return Err(SchedulerHardwareError::NotAwaiting(wait)),
        })
    }

    fn start(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> Result<BluetoothSchedulerHardwareRunCommandPublished, SchedulerStartError<S::Error>> {
        // SAFETY: the executor linked every listed item before returning the
        // head; each lives in 'static pool storage, and the scheduler is idle.
        let published = unsafe {
            self.controller
                .publish_scheduler_hardware_list_head(LIST, head(at))
        };
        let interrupts = self
            .storage
            .prepare_scheduler_run_interrupts()
            .map_err(SchedulerStartError::Interrupts)?;
        let event = self
            .controller
            .publish_scheduler_run_event(published, interrupts);
        Ok(self
            .controller
            .publish_scheduler_hardware_run_command(event))
    }
}
