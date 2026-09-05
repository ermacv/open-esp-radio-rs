//! Peripheral-connection hook and tail for the shared single-item completion spine.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeRxError, BluetoothPeripheralConnectionMemoryGraphRecycleError,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    BluetoothSchedulerFinishedHardwareListObserved,
    le::peripheral::connection::{
        BluetoothPeripheralConnectionFirstEventCompletionObservation,
        BluetoothPeripheralConnectionFirstEventCompletionObserved,
        BluetoothPeripheralConnectionFirstEventRunning,
    },
    scheduler::timeline::{BluetoothSchedulerSequenceReady, BluetoothSchedulerWindowReservation},
};

pub(crate) struct BluetoothPeripheralConnectionCompletionRole;

impl crate::scheduler::core::BluetoothSingleItemSchedulerRole
    for BluetoothPeripheralConnectionCompletionRole
{
    type RunningItem = BluetoothPeripheralConnectionFirstEventRunning;
    type CompletionObservedItem = BluetoothPeripheralConnectionFirstEventCompletionObserved;
    type Retained = BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>;

    fn running_item_address(item: &Self::RunningItem) -> BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation<Self> {
        match item.observe_completion(observed) {
            BluetoothPeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::ListMismatch {
                running,
                observed,
            },
            BluetoothPeripheralConnectionFirstEventCompletionObservation::StillInFlight(
                running,
            ) => crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::StillInFlight(
                running,
            ),
            BluetoothPeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                completed,
            ) => {
                crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::CompletionObserved(
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
pub(crate) struct BluetoothPeripheralConnectionRecycleReady {
    event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

impl BluetoothPeripheralConnectionRecycleReady {
    pub(crate) const fn new(
        event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
        reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
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
        BluetoothPeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.event, self.removal, self.reservation)
    }
}

/// Exact reason the peripheral-specific recycle tail sealed its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothPeripheralConnectionRecycleFailureCause {
    SchedulerIdentityMismatch,
    FinishedListDrainStillActive,
    MemoryIdentityMismatch(BluetoothPeripheralConnectionMemoryGraphRecycleError),
    ReceiveInvalid(BluetoothLeRxError),
    ReservationIdentityMismatch,
}

/// Lossless role-tail rejection after the common removal-ready boundary.
#[must_use = "the exact completed graph and timeline reservation remain sealed"]
pub(crate) struct BluetoothPeripheralConnectionRecycleFailure {
    cause: BluetoothPeripheralConnectionRecycleFailureCause,
    _ready: BluetoothPeripheralConnectionRecycleReady,
}

impl BluetoothPeripheralConnectionRecycleFailure {
    pub(crate) const fn new(
        cause: BluetoothPeripheralConnectionRecycleFailureCause,
        ready: BluetoothPeripheralConnectionRecycleReady,
    ) -> Self {
        Self {
            cause,
            _ready: ready,
        }
    }

    pub(crate) const fn cause(&self) -> BluetoothPeripheralConnectionRecycleFailureCause {
        self.cause
    }
}

pub(crate) type BluetoothPeripheralConnectionRecycleOutcome = ControlFlow<
    BluetoothPeripheralConnectionRecycleFailure,
    crate::scheduler::core::BluetoothPeripheralConnectionSchedulerRecycled,
>;
