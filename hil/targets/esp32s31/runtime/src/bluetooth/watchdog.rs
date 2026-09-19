//! Explicit diagnostic client of the same SoC service used by production.
//! HCI Reset/Test End cannot renew the first DTM-triggered ten-second deadline.
use oer_esp32s31_soc::watchdog::{DeadlineBudget, DeadlineLease, DeadlineWatchdog};

pub(crate) struct DtmWatchdog {
    service: &'static DeadlineWatchdog,
    protection: Option<DeadlineLease<'static>>,
}

impl DtmWatchdog {
    #[cfg(feature = "bluetooth-watchdog-reset")]
    pub(super) fn new(service: &'static DeadlineWatchdog) -> Self {
        Self {
            service,
            protection: None,
        }
    }

    pub(super) fn arm_once(&mut self) {
        if self.protection.is_some() {
            return;
        }
        let budget = DeadlineBudget::from_micros(core::num::NonZeroU32::new(10_000_000).unwrap());
        self.protection = Some(
            self.service
                .arm(budget)
                .expect("exclusive diagnostic deadline"),
        );
    }
}
