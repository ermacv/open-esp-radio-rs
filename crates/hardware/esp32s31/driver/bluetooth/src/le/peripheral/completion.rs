//! Peripheral-connection hook and tail for the shared single-item completion spine.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use crate::{
    le::peripheral::connection::{
        PeripheralConnectionFirstEventCompletionObservation,
        PeripheralConnectionFirstEventCompletionObserved, PeripheralConnectionFirstEventRunning,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved,
        timeline::{SchedulerSequenceReady, SchedulerWindowReservation},
    },
};

use oer_esp32s31_bluetooth_memory::{LeRxError, PeripheralConnectionMemoryGraphRecycleError};

use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalReady,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

pub(crate) struct PeripheralConnectionCompletionRole;

impl crate::scheduler::core::SingleItemSchedulerRole for PeripheralConnectionCompletionRole {
    type RunningItem = PeripheralConnectionFirstEventRunning;
    type CompletionObservedItem = PeripheralConnectionFirstEventCompletionObserved;
    type Retained = SchedulerWindowReservation<SchedulerSequenceReady>;

    fn running_item_address(item: &Self::RunningItem) -> BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::SingleItemRoleCompletionObservation<Self> {
        match item.observe_completion(observed) {
            PeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::core::SingleItemRoleCompletionObservation::ListMismatch {
                running,
                observed,
            },
            PeripheralConnectionFirstEventCompletionObservation::StillInFlight(running) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::StillInFlight(running)
            }
            PeripheralConnectionFirstEventCompletionObservation::CompletionObserved(completed) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::CompletionObserved(
                    completed,
                )
            }
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}

/// Peripheral role tail after the common scheduler removed the sole item.
#[must_use = "the completed graph and timeline reservation must be reclaimed together"]
pub(crate) struct PeripheralConnectionRecycleReady {
    event: PeripheralConnectionFirstEventCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

impl PeripheralConnectionRecycleReady {
    pub(crate) const fn new(
        event: PeripheralConnectionFirstEventCompletionObserved,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
        reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
    ) -> Self {
        Self {
            event,
            removal,
            reservation,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.event, self.removal, self.reservation)
    }
}

/// Exact reason the peripheral-specific recycle tail sealed its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralConnectionRecycleFailureCause {
    SchedulerIdentityMismatch,
    FinishedListDrainStillActive,
    MemoryIdentityMismatch(PeripheralConnectionMemoryGraphRecycleError),
    ReceiveInvalid(LeRxError),
    ReservationIdentityMismatch,
}

/// Lossless role-tail rejection after the common removal-ready boundary.
#[must_use = "the exact completed graph and timeline reservation remain sealed"]
pub(crate) struct PeripheralConnectionRecycleFailure {
    cause: PeripheralConnectionRecycleFailureCause,
    _ready: PeripheralConnectionRecycleReady,
}

impl PeripheralConnectionRecycleFailure {
    pub(crate) const fn new(
        cause: PeripheralConnectionRecycleFailureCause,
        ready: PeripheralConnectionRecycleReady,
    ) -> Self {
        Self {
            cause,
            _ready: ready,
        }
    }

    pub(crate) const fn cause(&self) -> PeripheralConnectionRecycleFailureCause {
        self.cause
    }
}

pub(crate) type PeripheralConnectionRecycleOutcome = ControlFlow<
    PeripheralConnectionRecycleFailure,
    crate::scheduler::core::PeripheralConnectionSchedulerRecycled,
>;
