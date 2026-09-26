//! Driver of the source-127 modem low-power timer task.
//!
//! The interrupt service moves the timer owner into software-pending state
//! when register work needs the task. This driver acquires it, advances the
//! software queue and returns the rearmed owner to interrupt storage. The
//! radio role schedules no timer entries, so an expiration has no consumer
//! and is discarded.

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use embassy_time::Timer;
use oer_esp32s31_bluetooth::modem_timer::{
    ControllerModemTimerBegin, ControllerModemTimerReadinessClass, ControllerModemTimerRearm,
    ControllerModemTimerStep, ControllerModemTimerTask, ModemLpTimerSoftwareOwnerStorage,
};

use crate::HARDWARE_RECHECK;

/// Why the timer task stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemTimerFault<TakeError, RestoreError> {
    /// Interrupt storage refused the software-pending owner.
    Take(TakeError),
    /// Interrupt storage refused the rearmed owner, which the task retains.
    Restore(RestoreError),
}

/// Drive the source-127 task until interrupt storage refuses an exchange.
///
/// `wake` is signalled by the platform's source-127 service whenever it opens
/// a new worker wake epoch.
pub async fn run_modem_timer<
    M: RawMutex,
    S: ModemLpTimerSoftwareOwnerStorage,
    const CAPACITY: usize,
>(
    task: &mut ControllerModemTimerTask<'_, S, CAPACITY>,
    wake: &Signal<M, ()>,
) -> ModemTimerFault<S::TakeError, S::RestoreError> {
    loop {
        match task.readiness().class() {
            ControllerModemTimerReadinessClass::Interrupt => {
                if !task.readiness().is_ready() {
                    wake.wait().await;
                    continue;
                }
                if let ControllerModemTimerBegin::StorageRejected(error) = task.begin() {
                    return ModemTimerFault::Take(error);
                }
            }
            ControllerModemTimerReadinessClass::Step => {
                if let ControllerModemTimerStep::Recheck = task.step() {
                    Timer::after(HARDWARE_RECHECK).await;
                }
            }
            ControllerModemTimerReadinessClass::EventCapacity => {
                let _ = task.take_expiration();
            }
            ControllerModemTimerReadinessClass::Rearm => {
                if let ControllerModemTimerRearm::StorageRejected(error) = task.rearm() {
                    return ModemTimerFault::Restore(error);
                }
            }
        }
    }
}
