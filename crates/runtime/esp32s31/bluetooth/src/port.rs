//! The runtime as the radio port of the portable Controller service loop.

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_radio::{RadioInstant, RadioOutcome, RadioRequest, RadioTiming, RequestError};
use oer_bluetooth_runtime::{LeRadioPort, OutcomesLost};

use crate::{BluetoothOutcome, BluetoothRadioHardware, BluetoothRuntime, BluetoothRuntimeError};

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
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

    async fn clock(&self) -> Result<(RadioInstant, RadioTiming), BluetoothRuntimeError> {
        BluetoothRuntime::clock(self).await
    }

    async fn request(
        &self,
        request: RadioRequest<'_>,
    ) -> Result<Result<(), RequestError>, BluetoothRuntimeError> {
        match BluetoothRuntime::request(self, request).await {
            Ok(()) => Ok(Ok(())),
            Err(BluetoothRuntimeError::Rejected(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        }
    }

    async fn next_outcome(&self) -> Result<BluetoothOutcome, OutcomesLost> {
        BluetoothRuntime::next_outcome(self)
            .await
            .map_err(|_| OutcomesLost)
    }

    fn view(outcome: &BluetoothOutcome) -> RadioOutcome<'_> {
        outcome.portable()
    }
}
