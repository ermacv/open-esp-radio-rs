//! Legacy nonconnectable-advertising hook for the shared single-item completion spine.

#![forbid(unsafe_code)]

use crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved;

pub(crate) struct LegacyAdvertisingCompletionRole<'a>(core::marker::PhantomData<&'a ()>);

impl<'a> crate::scheduler::core::SingleItemSchedulerRole for LegacyAdvertisingCompletionRole<'a> {
    type RunningItem = crate::le::advertising::legacy::LegacyAdvertisingRunningEvent<'a>;
    type CompletionObservedItem =
        crate::le::advertising::legacy::LegacyAdvertisingCompletionObservedEvent<'a>;
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
        match item.observe_completion(observed) {
            crate::le::advertising::legacy::LegacyAdvertisingRunningEventCompletionObservation::ListMismatch {
                item,
                observed,
            } => crate::scheduler::core::SingleItemRoleCompletionObservation::ListMismatch {
                running: item,
                observed,
            },
            crate::le::advertising::legacy::LegacyAdvertisingRunningEventCompletionObservation::StillInFlight(item) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::StillInFlight(item)
            }
            crate::le::advertising::legacy::LegacyAdvertisingRunningEventCompletionObservation::CompletionObserved(item) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::CompletionObserved(item)
            }
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}
