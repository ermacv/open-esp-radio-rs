//! Active legacy-advertising composition after the first scheduler `RUN`.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    BluetoothControllerIdleCommandTask, BluetoothControllerIdleResponsePending,
    BluetoothControllerIdleResponsePublication, BluetoothLegacyAdvertisingSchedulerRunning,
    BluetoothSchedulerRunInterruptStorage,
};

/// Accepted Enable response paired with the exact already-running graph.
#[must_use = "publish the response while retaining the running advertising owner"]
pub struct BluetoothLegacyAdvertisingResponsePendingSession<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
    ) -> Self {
        Self { pending, running }
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
        self.pending.wait_response_capacity(controller).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothLegacyAdvertisingResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        let Self { pending, running } = self;
        match pending.try_publish(controller) {
            BluetoothControllerIdleResponsePublication::Published(task) => {
                BluetoothLegacyAdvertisingResponsePublication::Published(
                    BluetoothLegacyAdvertisingActiveSession { task, running },
                )
            }
            BluetoothControllerIdleResponsePublication::Pending(pending) => {
                BluetoothLegacyAdvertisingResponsePublication::Pending(Self { pending, running })
            }
            BluetoothControllerIdleResponsePublication::EndpointMismatch(pending) => {
                BluetoothLegacyAdvertisingResponsePublication::EndpointMismatch(Self {
                    pending,
                    running,
                })
            }
            BluetoothControllerIdleResponsePublication::Fault { pending, error } => {
                BluetoothLegacyAdvertisingResponsePublication::Fault {
                    pending: Self { pending, running },
                    error,
                }
            }
        }
    }
}

/// Result of publishing the Success response for an already-running event.
#[must_use = "retain the active session or unchanged response transaction"]
pub enum BluetoothLegacyAdvertisingResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyAdvertisingActiveSession<'runtime, S, SCHEDULER_CAPACITY>),
    Pending(BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Fault {
        pending: BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>,
        error: open_esp_radio_bluetooth_hci::HciChannelError,
    },
}

/// Exact HCI order and first running advertising graph after Success publication.
///
/// The idle command token is deliberately private: generic idle routing cannot
/// run while the advertising graph owns list zero. Active command and radio
/// progression will be added as methods on this aggregate.
#[must_use = "drive the running advertising graph and active HCI order together"]
pub struct BluetoothLegacyAdvertisingActiveSession<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
    running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingActiveSession<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn scheduler_item_address(
        &self,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> crate::BluetoothSchedulerHardwareListIndex {
        self.running.hardware_list_index()
    }

    pub const fn scheduler_wake(&self) -> &crate::BluetoothSchedulerWakeCell {
        self.task.scheduler_wake()
    }
}
