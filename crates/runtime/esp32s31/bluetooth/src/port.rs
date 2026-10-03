//! The runtime as the radio port of the portable Controller service loop.

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_radio::{
    ClockInfo, EventsLost, LeInstant, LeRadioCapabilities, LeRadioPort, RadioActivity, RadioEpoch,
    RadioOutcome, RadioRequest, RadioTiming, RequestError,
};
use oer_time::Timer;

use crate::{BluetoothOutcome, BluetoothRadioHardware, BluetoothRuntime, BluetoothRuntimeError};

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
    type Outcome = BluetoothOutcome;
    /// Never [`BluetoothRuntimeError::Rejected`]: a refusal is an answer.
    type Error = BluetoothRuntimeError;

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

    async fn clock(&self) -> Result<(LeInstant, RadioTiming), BluetoothRuntimeError> {
        BluetoothRuntime::clock(self).await
    }

    async fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> Result<Result<(), RequestError>, BluetoothRuntimeError> {
        match BluetoothRuntime::request(self, request).await {
            Ok(()) => Ok(Ok(())),
            Err(BluetoothRuntimeError::Rejected(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        }
    }

    async fn next_outcome(&self) -> Result<BluetoothOutcome, EventsLost> {
        BluetoothRuntime::next_outcome(self).await
    }

    fn view(outcome: &BluetoothOutcome) -> RadioOutcome<'_> {
        outcome.portable()
    }

    fn activity(&self, activity: RadioActivity) -> Result<(), BluetoothRuntimeError> {
        self.publish_activity(activity);
        Ok(())
    }
}
