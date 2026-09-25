//! Explicit board budgets and binding to the non-radio deadline service.
use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineLease, DeadlineWatchdog};

/// Board-selected engineering limits; no qualified defaults are supplied.
/// The shared service must be reserved for this composition's serialized PHY
/// operations. It is not a general concurrent peripheral watchdog pool.
#[derive(Clone, Copy)]
pub struct WatchdogConfig {
    watchdog: &'static DeadlineWatchdog,
    startup: DeadlineBudget,
    maintenance: DeadlineBudget,
    shutdown: DeadlineBudget,
}
impl WatchdogConfig {
    /// Bind caller-owned stable policy to one exclusive SoC deadline service.
    /// Startup covers cold registration/tracking or powered restart;
    /// maintenance includes quiescence and full restoration; shutdown covers
    /// physical release (and a complete retained close/wake cycle in Wi-Fi).
    /// Place this configuration in static board/application storage.
    pub const fn new(
        watchdog: &'static DeadlineWatchdog,
        startup: DeadlineBudget,
        maintenance: DeadlineBudget,
        shutdown: DeadlineBudget,
    ) -> Self {
        Self {
            watchdog,
            startup,
            maintenance,
            shutdown,
        }
    }
    #[inline(never)]
    fn begin(self, budget: DeadlineBudget) -> DeadlineLease<'static> {
        // AlreadyArmed means the previous physical obligation was abandoned.
        // Never feed it or grant another radio operation in that epoch.
        match self.watchdog.arm(budget) {
            Ok(lease) => lease,
            Err(_) => oer_esp32s31_soc_esp_hal::reset_system(),
        }
    }
    pub(crate) fn startup(self) -> DeadlineLease<'static> {
        self.begin(self.startup)
    }
    pub(crate) fn shutdown(self) -> DeadlineLease<'static> {
        self.begin(self.shutdown)
    }
    pub(crate) fn maintenance(self, remaining_micros: Option<u64>) -> DeadlineLease<'static> {
        let micros = remaining_micros
            .unwrap_or(u64::from(self.maintenance.as_micros()))
            .min(u64::from(self.maintenance.as_micros())) as u32;
        let Some(micros) = core::num::NonZeroU32::new(micros) else {
            oer_esp32s31_soc_esp_hal::reset_system()
        };
        self.begin(DeadlineBudget::from_micros(micros))
    }
    #[inline(never)]
    pub(crate) fn complete(lease: DeadlineLease<'_>) {
        if lease.complete().is_err() {
            oer_esp32s31_soc_esp_hal::reset_system();
        }
    }
}
