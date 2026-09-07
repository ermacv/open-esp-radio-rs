//! Connectable legacy-advertising hook for the shared single-item completion spine.

#![forbid(unsafe_code)]

use crate::{
    le::advertising::connectable::{
        LegacyConnectableAdvertisingCompletionObserved, LegacyConnectableAdvertisingPostRunOutcome,
        LegacyConnectableAdvertisingRunning,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved,
        timeline::{SchedulerSequenceReady, SchedulerWindowReservation},
    },
};

use oer_esp32s31_bluetooth_memory::{
    LeRxError, LegacyConnectableAdvertisingMemoryGraphCompletionObservation,
    LegacyConnectableAdvertisingMemoryGraphRecycleError,
};

use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalReady,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

pub(crate) struct LegacyConnectableAdvertisingCompletionRole;

impl crate::scheduler::core::SingleItemSchedulerRole
    for LegacyConnectableAdvertisingCompletionRole
{
    type RunningItem = LegacyConnectableAdvertisingRunning;
    type CompletionObservedItem = LegacyConnectableAdvertisingCompletionObserved;
    type Retained = crate::scheduler::timeline::SchedulerWindowReservation<
        crate::scheduler::timeline::SchedulerSequenceReady,
    >;

    fn running_item_address(
        item: &Self::RunningItem,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::SingleItemRoleCompletionObservation<Self> {
        let (memory, remainder) = item.into_memory_completion();
        match memory.observe_completion(observed) {
            LegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::core::SingleItemRoleCompletionObservation::ListMismatch {
                running: LegacyConnectableAdvertisingRunning::from_memory_completion(
                    running, remainder,
                ),
                observed,
            },
            LegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(
                running,
            ) => crate::scheduler::core::SingleItemRoleCompletionObservation::StillInFlight(
                LegacyConnectableAdvertisingRunning::from_memory_completion(running, remainder),
            ),
            LegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => crate::scheduler::core::SingleItemRoleCompletionObservation::CompletionObserved(
                LegacyConnectableAdvertisingCompletionObserved::new(completed, remainder),
            ),
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}

/// Connectable role tail after the common scheduler removed the sole item.
#[must_use = "the completed graph and timeline reservation must be reclaimed together"]
pub(crate) struct LegacyConnectableAdvertisingRecycleReady {
    item: LegacyConnectableAdvertisingCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

impl LegacyConnectableAdvertisingRecycleReady {
    pub(crate) const fn new(
        item: LegacyConnectableAdvertisingCompletionObserved,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
        reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
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
        LegacyConnectableAdvertisingCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.item, self.removal, self.reservation)
    }
}

/// Lossless role-tail result after the generic RUN-to-removal spine.
#[must_use = "retain the classified result or the exact sealed ownership failure"]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub(crate) enum LegacyConnectableAdvertisingRecycleStep {
    SchedulerIdentityMismatch {
        _ready: LegacyConnectableAdvertisingRecycleReady,
    },
    FinishedListDrainStillActive {
        _ready: LegacyConnectableAdvertisingRecycleReady,
    },
    MemoryIdentityMismatch {
        _ready: LegacyConnectableAdvertisingRecycleReady,
        _error: LegacyConnectableAdvertisingMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        _ready: LegacyConnectableAdvertisingRecycleReady,
        _error: LeRxError,
    },
    ReservationIdentityMismatch {
        _ready: LegacyConnectableAdvertisingRecycleReady,
    },
    Classified(LegacyConnectableAdvertisingPostRunOutcome),
}
