//! The runtime as the radio port of the portable Controller service loop.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_radio::{
    CancelError, ClockError, ClockInfo, EventId, EventsLost, LeInstant, LeRadio,
    LeRadioCapabilities, LeRadioPort, LifecycleCommand, LifecycleError, NotInstalled, PortResult,
    RadioActivity, RadioEpoch, RadioOutcome, RadioPort, RadioRequest, RequestError,
};
use oer_time::Timer;

use crate::{BluetoothFault, BluetoothOutcome, BluetoothRadioHardware, BluetoothRuntime};

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> RadioPort
    for BluetoothRuntime<
        M,
        H,
        T,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    type Event = BluetoothOutcome;
    type Id = EventId;
    type Domain = LeRadio;
    type Fault = BluetoothFault<H::StartError>;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<BluetoothOutcome, EventsLost, Self::Fault>> + '_ {
        self.wait_event()
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, Self::Fault>> + '_ {
        self.read_now()
    }

    fn cancel(
        &self,
        id: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_ {
        self.cancel_event(id)
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Self::Fault>> + '_ {
        self.run_lifecycle(command)
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> LeRadioPort
    for BluetoothRuntime<
        M,
        H,
        T,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    fn capabilities(&self) -> LeRadioCapabilities {
        BluetoothRuntime::capabilities(self)
    }

    /// The radio time is the controller clock, extended from the 32-bit
    /// controller-time latch the radio samples; the runtime keeps no
    /// measured relation between it and the image's monotonic clock.
    fn clock_info(&self) -> ClockInfo {
        ClockInfo {
            resolution: ClockInfo::MONOTONIC_MICROS.resolution,
            epoch: RadioEpoch::Unrelated,
        }
    }

    fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> impl Future<Output = PortResult<(), RequestError, Self::Fault>> {
        self.submit_request(request)
    }

    fn view(outcome: &BluetoothOutcome) -> RadioOutcome<'_> {
        outcome.portable()
    }

    fn activity(&self, activity: RadioActivity) -> PortResult<(), NotInstalled, Self::Fault> {
        self.publish_activity(activity);
        Ok(Ok(()))
    }
}
