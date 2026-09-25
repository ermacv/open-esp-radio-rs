//! Platform observations and diagnostics, independent of radio protocols.
#[cfg(feature = "system-watchdog")]
pub(super) mod console;
#[cfg(feature = "system-watchdog")]
mod watchdog;

pub(super) fn boot_evidence() -> oer_hil_protocol::BootEvidence {
    use oer_hil_protocol::{BootEvidence, ResetReason};
    BootEvidence {
        reset_reason: match esp_hal::system::reset_reason() {
            Some(esp_hal::rtc_cntl::SocResetReason::CoreSw) => ResetReason::Software,
            Some(esp_hal::rtc_cntl::SocResetReason::CoreMwdt1) => ResetReason::MainWatchdog1,
            _ => ResetReason::Other,
        },
    }
}
