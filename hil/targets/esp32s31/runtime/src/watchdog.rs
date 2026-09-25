//! HIL-owned engineering budgets, not qualified production limits.
use core::num::NonZeroU32;
use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use static_cell::StaticCell;

static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
const STARTUP: DeadlineBudget = DeadlineBudget::from_micros(NonZeroU32::new(5_000_000).unwrap());
#[cfg(not(feature = "phy-fault-injection"))]
const MAINTENANCE: DeadlineBudget =
    DeadlineBudget::from_micros(NonZeroU32::new(1_000_000).unwrap());
// Diagnostic checkpoint/host acknowledgement fits inside this original lease;
// release never feeds or rearms it.
#[cfg(feature = "phy-fault-injection")]
const MAINTENANCE: DeadlineBudget =
    DeadlineBudget::from_micros(NonZeroU32::new(5_000_000).unwrap());
// Includes quiescence and, for retained Wi-Fi cycles, close plus wake.
const SHUTDOWN: DeadlineBudget = DeadlineBudget::from_micros(NonZeroU32::new(1_000_000).unwrap());

pub(super) fn init(timg: esp_hal::peripherals::TIMG1<'static>) -> &'static DeadlineWatchdog {
    SERVICE.init(DeadlineWatchdog::new(timg))
}

#[cfg(feature = "bluetooth-radio")]
pub(super) fn bluetooth(
    service: &'static DeadlineWatchdog,
) -> &'static oer_esp32s31_bluetooth_system::WatchdogConfig {
    static CONFIG: StaticCell<oer_esp32s31_bluetooth_system::WatchdogConfig> = StaticCell::new();
    CONFIG.init(oer_esp32s31_bluetooth_system::WatchdogConfig::new(
        service,
        STARTUP,
        MAINTENANCE,
        SHUTDOWN,
    ))
}

#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(super) fn wifi(
    service: &'static DeadlineWatchdog,
) -> &'static oer_esp32s31_ieee80211_system::WatchdogConfig {
    static CONFIG: StaticCell<oer_esp32s31_ieee80211_system::WatchdogConfig> = StaticCell::new();
    CONFIG.init(oer_esp32s31_ieee80211_system::WatchdogConfig::new(
        service,
        STARTUP,
        MAINTENANCE,
        SHUTDOWN,
    ))
}
