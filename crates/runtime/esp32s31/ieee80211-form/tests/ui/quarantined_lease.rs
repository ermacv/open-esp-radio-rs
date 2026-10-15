#![no_std]
use core::convert::Infallible;
use oer_esp32s31_ieee80211_form::scope::{Quarantined, ScopeReturn, WakePlan};
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};
pub fn quarantine(
    value: Quarantined<ScopeReturn<(), ActiveEpoch<Moderated>, (), Infallible, fn(&(), WakePlan)>>,
) {
    #[cfg(ui_reject)]
    let _lease = value.lease; // ui-error: E0609
    #[cfg(not(ui_reject))]
    drop(value);
}
