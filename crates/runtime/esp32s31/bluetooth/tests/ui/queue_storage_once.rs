#![no_std]
use oer_esp32s31_bluetooth_runtime::RuntimeQueue;
use oer_esp32s31_bluetooth_system::BluetoothSystemRuntime;

pub fn twice(
    storage: &'static mut BluetoothSystemRuntime,
    fresh: &'static mut BluetoothSystemRuntime,
) -> (RuntimeQueue, RuntimeQueue) {
    let (_, first) = BluetoothSystemRuntime::pin_static(storage);
    #[cfg(not(ui_reject))]
    let storage = fresh;
    #[cfg(ui_reject)]
    let _ = fresh;
    let (_, second) = BluetoothSystemRuntime::pin_static(storage); // ui-error: E0499
    (first, second)
}
