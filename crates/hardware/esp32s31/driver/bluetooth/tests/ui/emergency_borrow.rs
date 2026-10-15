#![no_std]
use oer_esp32s31_bluetooth::{
    emergency::PortControllerBorrow, runtime_resources::ControllerPoweredTaskRuntime,
};
#[cfg(ui_reject)]
pub fn escape<'p, 'a>(
    mut task: PortControllerBorrow<'p, 'a>,
) -> &'p mut ControllerPoweredTaskRuntime<'a> {
    &mut task // ui-error: E0515
}
#[cfg(not(ui_reject))]
pub fn finite<'p, 'a>(
    task: &'p mut PortControllerBorrow<'_, 'a>,
) -> &'p mut ControllerPoweredTaskRuntime<'a> {
    task
}
