//! Terminal maintenance escalation selected by the final composition.
//!
//! `oer_esp32s31_ieee80211_runtime::roles::station::maintenance::policy`
//! classifies failures; only this composition maps a terminal reason to
//! system reset. Radio drivers and the shared PHY contract have no dependency
//! on the SoC reset or watchdog mechanism.

use oer_esp32s31_ieee80211_runtime::roles::station::maintenance::policy::escalate;
use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

pub(crate) fn enforce<E: ?Sized>(failure: &E, reason: Option<SharedPhyFailStop>) {
    escalate(failure, reason, |reason, _retained| {
        let _reason = core::hint::black_box(reason);
        oer_esp32s31_soc_esp_hal::reset_system()
    });
}
