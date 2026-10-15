#![no_std]
use oer_esp32s31_bluetooth::emergency::{ControllerEmergencyStop, EmergencyInterruptStorage};
pub fn retain<S: EmergencyInterruptStorage>(stop: ControllerEmergencyStop<'_, S>) {
    #[cfg(ui_reject)]
    let _second = stop.clone(); // ui-error: E0599
    core::mem::forget(stop);
}
