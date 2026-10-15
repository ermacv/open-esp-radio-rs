#![no_std]
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_espressif_ieee802154_engine::ll::Ieee802154LowLevel;
use oer_espressif_ieee802154_runtime::{Ieee802154Random, Ieee802154Runtime, RuntimeQueue};
use oer_time::Timer;

pub fn twice<M, H, T, R>(
    storage: &'static mut Ieee802154Runtime<'static, M, H, T, R, 4>,
    fresh: &'static mut Ieee802154Runtime<'static, M, H, T, R, 4>,
) -> (RuntimeQueue, RuntimeQueue)
where
    M: RawMutex + 'static,
    H: Ieee802154LowLevel + 'static,
    T: Timer + 'static,
    R: Ieee802154Random + 'static,
{
    let (_, first) = Ieee802154Runtime::pin_static(storage);
    #[cfg(not(ui_reject))]
    let storage = fresh;
    #[cfg(ui_reject)]
    let _ = fresh;
    let (_, second) = Ieee802154Runtime::pin_static(storage); // ui-error: E0499
    (first, second)
}
