#![no_std]
use oer_esp32s31_ieee80211_form::scope::ScopeLease;
pub fn transfer(lease: ScopeLease<()>) -> ScopeLease<()> {
    #[cfg(ui_reject)]
    let _first = lease;
    lease // ui-error: E0382
}
