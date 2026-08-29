//! Restricted insertion-begin execution command transactions.
//!
//! Command zero binds one selected scheduler item to its hardware list for the
//! execution-lock attempt. Command one requests current-head reconciliation
//! for exactly one list. Both publications are finite; their hardware-owned
//! waits remain split into fresh task and interrupt observations.

#![deny(unsafe_code)]

use super::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerWorkObservation, BluetoothTaskRegisters, device_fence,
};

/// Validated item and list selected for one insertion execution-lock attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerExecutionLockRequest {
    address: BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerExecutionLockRequest {
    /// Bind the merge-selected item to its scheduler hardware list.
    pub const fn new(
        address: BluetoothControllerSramAddress,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            address,
            hardware_list_index,
        }
    }

    /// Selected item address without dereference authority.
    pub const fn address(self) -> BluetoothControllerSramAddress {
        self.address
    }

    /// Hardware list serialized by this attempt.
    pub const fn hardware_list_index(self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Proof that command zero and its trailing device fence were published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the execution-lock command still requires a fresh observation"]
pub struct BluetoothSchedulerExecutionLockPublished {
    _private: (),
}

/// Result of one finite execution-lock observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor; terminal work must advance insertion"]
pub enum BluetoothSchedulerExecutionLockDisposition {
    /// Hardware remains busy and command-zero status has not become ready.
    Pending,
    /// Result zero retains command-zero execution lock for begin outcome four.
    ExecutionLockRetained,
    /// Scheduler idle or a reviewed nonzero result requires current-head
    /// reconciliation through command one.
    ReconcileCurrentHead,
    /// Hardware returned a positional value outside the three nonzero values
    /// accepted by the complete vendor body.
    UnsupportedHardwareResult,
}

/// Proof that command one and its trailing device fence were published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the execution-modify command still requires a fresh observation"]
pub struct BluetoothSchedulerExecutionModifyPublished {
    _private: (),
}

/// Result of one finite execution-modify observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor; terminal work must advance insertion"]
pub enum BluetoothSchedulerExecutionModifyDisposition {
    /// Hardware remains busy and command-one status has not become ready.
    Pending,
    /// Command one reached its accepted reconciliation edge.
    Ready,
    /// The complete vendor body diagnoses positional status 19 on this edge.
    HardwareRejected,
}

trait BluetoothSchedulerInsertionExecutionControl {
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest);
    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex);
    fn order_after_publication(&mut self);
}

trait BluetoothSchedulerInsertionExecutionObservationControl {
    fn observe_execution_lock_ready(&mut self) -> bool;
    fn observe_execution_lock_result(&mut self) -> u8;
    fn observe_execution_modify_ready(&mut self) -> bool;
    fn observe_execution_modify_rejected(&mut self) -> bool;
}

struct HardwareBluetoothSchedulerInsertionExecutionControl<'registers> {
    registers: &'registers super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerInsertionExecutionControl
    for HardwareBluetoothSchedulerInsertionExecutionControl<'_>
{
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest) {
        super::svd::zero_based_field_write::publish_bluetooth_scheduler_execution_lock_request(
            self.registers,
            request.address().compressed_image(),
            request.hardware_list_index().get(),
            true,
        );
    }

    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        let list = match index.get() {
            0 => 0x0001,
            1 => 0x0002,
            2 => 0x0004,
            3 => 0x0008,
            4 => 0x0010,
            5 => 0x0020,
            6 => 0x0040,
            7 => 0x0080,
            8 => 0x0100,
            9 => 0x0200,
            10 => 0x0400,
            11 => 0x0800,
            12 => 0x1000,
            13 => 0x2000,
            14 => 0x4000,
            15 => 0x8000,
            _ => panic!("typed scheduler list index exceeded its PAC domain"),
        };
        super::svd::zero_based_field_write::publish_bluetooth_scheduler_execution_modify_request(
            self.registers,
            list,
            true,
        );
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

impl BluetoothSchedulerInsertionExecutionObservationControl
    for HardwareBluetoothSchedulerInsertionExecutionControl<'_>
{
    fn observe_execution_lock_ready(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_scheduler_execution_lock_ready(self.registers)
    }

    fn observe_execution_lock_result(&mut self) -> u8 {
        super::svd::field_read::observe_bluetooth_scheduler_execution_lock_result(self.registers)
    }

    fn observe_execution_modify_ready(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_scheduler_execution_modify_ready(self.registers)
    }

    fn observe_execution_modify_rejected(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_scheduler_execution_modify_rejected(
            self.registers,
        )
    }
}

fn execute_execution_lock_publication(
    control: &mut impl BluetoothSchedulerInsertionExecutionControl,
    request: BluetoothSchedulerExecutionLockRequest,
) -> BluetoothSchedulerExecutionLockPublished {
    control.publish_execution_lock(request);
    control.order_after_publication();
    BluetoothSchedulerExecutionLockPublished { _private: () }
}

fn execute_execution_modify_publication(
    control: &mut impl BluetoothSchedulerInsertionExecutionControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerExecutionModifyPublished {
    control.publish_execution_modify(index);
    control.order_after_publication();
    BluetoothSchedulerExecutionModifyPublished { _private: () }
}

fn execute_execution_lock_observation(
    control: &mut impl BluetoothSchedulerInsertionExecutionObservationControl,
    scheduler: BluetoothSchedulerWorkObservation,
) -> BluetoothSchedulerExecutionLockDisposition {
    if !scheduler.is_busy() {
        return BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead;
    }
    if !control.observe_execution_lock_ready() {
        return BluetoothSchedulerExecutionLockDisposition::Pending;
    }
    match control.observe_execution_lock_result() {
        0 => BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained,
        1 | 3 | 4 => BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
        _ => BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult,
    }
}

fn execute_execution_modify_observation(
    control: &mut impl BluetoothSchedulerInsertionExecutionObservationControl,
    scheduler: BluetoothSchedulerWorkObservation,
) -> BluetoothSchedulerExecutionModifyDisposition {
    if scheduler.is_busy() && !control.observe_execution_modify_ready() {
        return BluetoothSchedulerExecutionModifyDisposition::Pending;
    }
    if control.observe_execution_modify_rejected() {
        BluetoothSchedulerExecutionModifyDisposition::HardwareRejected
    } else {
        BluetoothSchedulerExecutionModifyDisposition::Ready
    }
}

impl BluetoothTaskRegisters {
    /// Publish command zero and return after its trailing device fence.
    ///
    /// # Safety
    ///
    /// The request must name the exact merge-selected initialized item. The
    /// caller must retain that pinned item and exclusive list serialization
    /// until insertion-begin and insertion-end return ownership.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the selected item lifetime and scheduler serialization"
    )]
    pub unsafe fn publish_scheduler_execution_lock(
        &mut self,
        request: BluetoothSchedulerExecutionLockRequest,
    ) -> BluetoothSchedulerExecutionLockPublished {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_lock_publication(&mut control, request)
    }

    /// Perform one finite command-zero observation in the reviewed
    /// short-circuit order.
    ///
    /// An idle scheduler performs no command-register read. While busy, the
    /// positional result is read only after command-zero ready is set.
    pub fn observe_scheduler_execution_lock(
        &mut self,
        scheduler: BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerExecutionLockDisposition {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_lock_observation(&mut control, scheduler)
    }

    /// Publish command one for exactly one hardware list and return after its
    /// trailing device fence.
    ///
    /// # Safety
    ///
    /// The caller must own the insertion reconciliation epoch and exclusive
    /// access to `index` until insertion-end returns ownership.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains insertion reconciliation and list serialization"
    )]
    pub unsafe fn publish_scheduler_execution_modify(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_modify_publication(&mut control, index)
    }

    /// Perform one finite command-one observation in the reviewed
    /// short-circuit order.
    ///
    /// While the scheduler is busy, a clear ready field returns immediately
    /// without reading the terminal rejection field.
    pub fn observe_scheduler_execution_modify(
        &mut self,
        scheduler: BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerExecutionModifyDisposition {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_modify_observation(&mut control, scheduler)
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockRequest,
        BluetoothSchedulerExecutionModifyDisposition, BluetoothSchedulerInsertionExecutionControl,
        BluetoothSchedulerInsertionExecutionObservationControl, execute_execution_lock_observation,
        execute_execution_lock_publication, execute_execution_modify_observation,
        execute_execution_modify_publication,
    };
    use crate::{
        BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerWorkObservation,
    };

    fn scheduler(busy: bool) -> BluetoothSchedulerWorkObservation {
        BluetoothSchedulerWorkObservation::from_fields_for_validation(busy, false, 0)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ObservationOperation {
        LockReady,
        LockResult,
        ModifyReady,
        ModifyRejected,
    }

    struct ObservationRecorder {
        operations: Vec<ObservationOperation>,
        lock_ready: bool,
        lock_result: u8,
        modify_ready: bool,
        modify_rejected: bool,
    }

    impl BluetoothSchedulerInsertionExecutionObservationControl for ObservationRecorder {
        fn observe_execution_lock_ready(&mut self) -> bool {
            self.operations.push(ObservationOperation::LockReady);
            self.lock_ready
        }

        fn observe_execution_lock_result(&mut self) -> u8 {
            self.operations.push(ObservationOperation::LockResult);
            self.lock_result
        }

        fn observe_execution_modify_ready(&mut self) -> bool {
            self.operations.push(ObservationOperation::ModifyReady);
            self.modify_ready
        }

        fn observe_execution_modify_rejected(&mut self) -> bool {
            self.operations.push(ObservationOperation::ModifyRejected);
            self.modify_rejected
        }
    }

    impl ObservationRecorder {
        fn new() -> Self {
            Self {
                operations: Vec::new(),
                lock_ready: false,
                lock_result: 0,
                modify_ready: false,
                modify_rejected: false,
            }
        }
    }

    #[test]
    fn execution_lock_preserves_idle_ready_and_result_short_circuits() {
        let mut recorder = ObservationRecorder::new();
        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(false)),
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead
        );
        assert!(recorder.operations.is_empty());

        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionLockDisposition::Pending
        );
        assert_eq!(recorder.operations, [ObservationOperation::LockReady]);

        recorder.operations.clear();
        recorder.lock_ready = true;
        recorder.lock_result = 0;
        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained
        );
        assert_eq!(
            recorder.operations,
            [
                ObservationOperation::LockReady,
                ObservationOperation::LockResult,
            ]
        );

        recorder.operations.clear();
        recorder.lock_result = 3;
        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead
        );
        recorder.lock_result = 2;
        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult
        );
    }

    #[test]
    fn execution_modify_reads_rejection_only_on_a_terminal_edge() {
        let mut recorder = ObservationRecorder::new();
        assert_eq!(
            execute_execution_modify_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionModifyDisposition::Pending
        );
        assert_eq!(recorder.operations, [ObservationOperation::ModifyReady]);

        recorder.operations.clear();
        assert_eq!(
            execute_execution_modify_observation(&mut recorder, scheduler(false)),
            BluetoothSchedulerExecutionModifyDisposition::Ready
        );
        assert_eq!(recorder.operations, [ObservationOperation::ModifyRejected]);

        recorder.operations.clear();
        recorder.modify_ready = true;
        recorder.modify_rejected = true;
        assert_eq!(
            execute_execution_modify_observation(&mut recorder, scheduler(true)),
            BluetoothSchedulerExecutionModifyDisposition::HardwareRejected
        );
        assert_eq!(
            recorder.operations,
            [
                ObservationOperation::ModifyReady,
                ObservationOperation::ModifyRejected,
            ]
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PublishLock(BluetoothSchedulerExecutionLockRequest),
        PublishModify(BluetoothSchedulerHardwareListIndex),
        DeviceFence,
    }

    struct Recorder {
        operations: Vec<Operation>,
    }

    impl BluetoothSchedulerInsertionExecutionControl for Recorder {
        fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest) {
            self.operations.push(Operation::PublishLock(request));
        }

        fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex) {
            self.operations.push(Operation::PublishModify(index));
        }

        fn order_after_publication(&mut self) {
            self.operations.push(Operation::DeviceFence);
        }
    }

    #[test]
    fn each_execution_command_is_followed_by_its_device_fence() {
        let address = BluetoothControllerSramAddress::new(0x2f00_0040)
            .expect("test address is representable");
        let index =
            BluetoothSchedulerHardwareListIndex::new(6).expect("test list is representable");
        let request = BluetoothSchedulerExecutionLockRequest::new(address, index);
        let mut recorder = Recorder {
            operations: Vec::new(),
        };

        let _lock = execute_execution_lock_publication(&mut recorder, request);
        let _modify = execute_execution_modify_publication(&mut recorder, index);

        assert_eq!(
            recorder.operations,
            [
                Operation::PublishLock(request),
                Operation::DeviceFence,
                Operation::PublishModify(index),
                Operation::DeviceFence,
            ]
        );
    }
}
