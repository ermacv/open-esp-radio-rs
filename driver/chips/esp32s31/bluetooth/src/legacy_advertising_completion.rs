//! Legacy nonconnectable-advertising hook for the shared single-item completion spine.

#![forbid(unsafe_code)]

use crate::BluetoothSchedulerFinishedHardwareListObserved;

pub(crate) struct BluetoothLegacyAdvertisingCompletionRole<'a>(core::marker::PhantomData<&'a ()>);

impl<'a> crate::scheduler::core::BluetoothSingleItemSchedulerRole
    for BluetoothLegacyAdvertisingCompletionRole<'a>
{
    type RunningItem = crate::legacy_advertising::BluetoothLegacyAdvertisingRunningEvent<'a>;
    type CompletionObservedItem =
        crate::legacy_advertising::BluetoothLegacyAdvertisingCompletionObservedEvent<'a>;
    type Retained = crate::scheduler::timeline::BluetoothSchedulerWindowReservation<
        crate::scheduler::timeline::BluetoothSchedulerSequenceReady,
    >;

    fn running_item_address(
        item: &Self::RunningItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation<Self> {
        match item.observe_completion(observed) {
            crate::legacy_advertising::BluetoothLegacyAdvertisingRunningEventCompletionObservation::ListMismatch {
                item,
                observed,
            } => crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::ListMismatch {
                running: item,
                observed,
            },
            crate::legacy_advertising::BluetoothLegacyAdvertisingRunningEventCompletionObservation::StillInFlight(item) => {
                crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::StillInFlight(item)
            }
            crate::legacy_advertising::BluetoothLegacyAdvertisingRunningEventCompletionObservation::CompletionObserved(item) => {
                crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::CompletionObserved(item)
            }
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}
