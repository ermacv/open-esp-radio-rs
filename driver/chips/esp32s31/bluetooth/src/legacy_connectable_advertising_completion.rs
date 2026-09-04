//! Connectable legacy-advertising hook for the shared single-item completion spine.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeRxError, BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation,
    BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    BluetoothSchedulerFinishedHardwareListObserved,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingCompletionObserved,
        BluetoothLegacyConnectableAdvertisingPostRunOutcome,
        BluetoothLegacyConnectableAdvertisingRunning,
    },
    scheduler_timeline::{BluetoothSchedulerSequenceReady, BluetoothSchedulerWindowReservation},
};

pub(crate) struct BluetoothLegacyConnectableAdvertisingCompletionRole;

impl crate::scheduler::BluetoothSingleItemSchedulerRole
    for BluetoothLegacyConnectableAdvertisingCompletionRole
{
    type RunningItem = BluetoothLegacyConnectableAdvertisingRunning;
    type CompletionObservedItem = BluetoothLegacyConnectableAdvertisingCompletionObserved;
    type Retained = crate::scheduler_timeline::BluetoothSchedulerWindowReservation<
        crate::scheduler_timeline::BluetoothSchedulerSequenceReady,
    >;

    fn running_item_address(
        item: &Self::RunningItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::BluetoothSingleItemRoleCompletionObservation<Self> {
        let (memory, remainder) = item.into_memory_completion();
        match memory.observe_completion(observed) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::BluetoothSingleItemRoleCompletionObservation::ListMismatch {
                running: BluetoothLegacyConnectableAdvertisingRunning::from_memory_completion(
                    running, remainder,
                ),
                observed,
            },
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(
                running,
            ) => crate::scheduler::BluetoothSingleItemRoleCompletionObservation::StillInFlight(
                BluetoothLegacyConnectableAdvertisingRunning::from_memory_completion(
                    running, remainder,
                ),
            ),
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => crate::scheduler::BluetoothSingleItemRoleCompletionObservation::CompletionObserved(
                BluetoothLegacyConnectableAdvertisingCompletionObserved::new(
                    completed, remainder,
                ),
            ),
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}

/// Connectable role tail after the common scheduler removed the sole item.
#[must_use = "the completed graph and timeline reservation must be reclaimed together"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingRecycleReady {
    item: BluetoothLegacyConnectableAdvertisingCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

impl BluetoothLegacyConnectableAdvertisingRecycleReady {
    pub(crate) const fn new(
        item: BluetoothLegacyConnectableAdvertisingCompletionObserved,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
        reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) -> Self {
        Self {
            item,
            removal,
            reservation,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.item, self.removal, self.reservation)
    }
}

/// Lossless role-tail result after the generic RUN-to-removal spine.
#[must_use = "retain the classified result or the exact sealed ownership failure"]
pub(crate) enum BluetoothLegacyConnectableAdvertisingRecycleStep {
    SchedulerIdentityMismatch {
        _ready: BluetoothLegacyConnectableAdvertisingRecycleReady,
    },
    FinishedListDrainStillActive {
        _ready: BluetoothLegacyConnectableAdvertisingRecycleReady,
    },
    MemoryIdentityMismatch {
        _ready: BluetoothLegacyConnectableAdvertisingRecycleReady,
        _error: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        _ready: BluetoothLegacyConnectableAdvertisingRecycleReady,
        _error: BluetoothLeRxError,
    },
    ReservationIdentityMismatch {
        _ready: BluetoothLegacyConnectableAdvertisingRecycleReady,
    },
    Classified(BluetoothLegacyConnectableAdvertisingPostRunOutcome),
}
