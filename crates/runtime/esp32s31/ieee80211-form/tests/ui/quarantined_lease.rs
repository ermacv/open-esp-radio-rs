#![no_std]
use oer_esp32s31_ieee80211_form::scope::driver::Quarantine;
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};
use core::convert::Infallible;
pub fn quarantine(value: Quarantine<(), ActiveEpoch<Moderated>, (), Infallible, fn(&(), oer_esp32s31_ieee80211_form::scope::WakePlan)>) {
    #[cfg(ui_reject)]
    let _lease = value.lease; // ui-error: E0609
    #[cfg(not(ui_reject))]
    let _observation = value.hardware_stopping_confirmed();
}
