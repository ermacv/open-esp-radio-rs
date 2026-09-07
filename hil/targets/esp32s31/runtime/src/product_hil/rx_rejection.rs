//! Preserve the first driver failure across cores; convert only at report time.
#[cfg(feature = "driver-observation")]
use core::cell::Cell;
#[cfg(feature = "driver-observation")]
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(feature = "driver-observation")]
use oer_esp32s31_embassy_wifi::AccessPointRxRejection;

#[cfg(feature = "driver-observation")]
static FIRST: Mutex<CriticalSectionRawMutex, Cell<Option<AccessPointRxRejection>>> =
    Mutex::new(Cell::new(None));

#[cfg(feature = "driver-observation")]
pub(super) fn observe(record: Option<AccessPointRxRejection>) {
    if let Some(record) = record {
        FIRST.lock(|first| {
            if first.get().is_none() {
                first.set(Some(record));
            }
        });
    }
}

pub(super) fn snapshot() -> Option<open_esp_radio_hil_protocol::WifiRxRejection> {
    #[cfg(feature = "driver-observation")]
    {
        FIRST
            .lock(Cell::get)
            .map(open_esp_radio_hil_esp32s31_telemetry::rx_rejection::evidence)
    }
    #[cfg(not(feature = "driver-observation"))]
    {
        None
    }
}

#[cfg(feature = "driver-observation")]
pub(super) fn reset() {
    FIRST.lock(|first| first.set(None));
}
