//! Bounded physical MAC stop before RX ownership is withdrawn.
use oer_esp32s31_phy::state::client::PhyTrackingTimer;
use oer_esp32s31_wifi_mac::init::MacRuntimeStopHardware;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopError {
    DeadlineOverflow,
    ClockReversed,
    TimedOut { active_state: u8 },
}

/// Wait on timer events between explicit activity readbacks. Cancelling this
/// borrow leaves the hardware owner with the caller and never proves it idle.
/// RX must remain published until this function succeeds.
pub async fn stop_mac<H: MacRuntimeStopHardware, T: PhyTrackingTimer>(
    hardware: &mut H,
    timer: &mut T,
    timeout_micros: u64,
) -> Result<(), StopError> {
    let mut previous = timer.now_micros();
    let deadline = previous
        .checked_add(timeout_micros)
        .ok_or(StopError::DeadlineOverflow)?;
    hardware.request_mac_runtime_stop();
    loop {
        let now = timer.now_micros();
        if now < previous {
            return Err(StopError::ClockReversed);
        }
        previous = now;
        let active_state = hardware.mac_runtime_active_state();
        if active_state == 0 {
            return Ok(());
        }
        if now >= deadline {
            return Err(StopError::TimedOut { active_state });
        }
        // MAC idle has no dedicated completion IRQ. This is a bounded,
        // timer-driven observation interval, never an assumed completion delay.
        let next = now.saturating_add(20).min(deadline);
        timer.wait_until_micros(next).await;
    }
}
#[cfg(test)]
mod tests;
