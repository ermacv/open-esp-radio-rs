//! Finite drive for the ESP32-S31 active legacy-advertising radio axis.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyAdvertisingActiveFault, BluetoothLegacyAdvertisingActiveSession,
    BluetoothLegacyAdvertisingActiveStep, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
};

/// First externally meaningful result after driving every immediately ready edge.
#[must_use = "retain the parked, completed, unrelated-list, or fail-stop owner"]
pub enum EmbassyBluetoothLegacyAdvertisingActiveDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Waiting(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    CpuOwned(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Fault(BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
}

/// Run only finite ready transitions; this function never polls or waits.
pub fn drive_legacy_advertising_active_ready<'runtime, S, const CAPACITY: usize>(
    mut session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyAdvertisingActiveDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match session.step_radio() {
            BluetoothLegacyAdvertisingActiveStep::Continue(next) => session = next,
            BluetoothLegacyAdvertisingActiveStep::Waiting(session) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::Waiting(session);
            }
            BluetoothLegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::UnrelatedList {
                    session,
                    observed,
                };
            }
            BluetoothLegacyAdvertisingActiveStep::CpuOwned(owner) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::CpuOwned(owner);
            }
            BluetoothLegacyAdvertisingActiveStep::Fault(fault) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::Fault(fault);
            }
        }
    }
}
