#![no_std]
use oer_esp32s31_ieee80211_form::scope::driver::Driver;
pub fn transfer(driver: Driver<'_, ()>) -> Driver<'_, ()> {
    #[cfg(ui_reject)]
    let _first = driver;
    driver // ui-error: E0382
}
