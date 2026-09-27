//! Platform observations and diagnostics, independent of radio protocols.
#[cfg(feature = "system-watchdog")]
pub(super) mod console;
pub(crate) mod postmortem;
#[cfg(feature = "system-watchdog")]
mod watchdog;

pub(super) fn boot_evidence() -> oer_hil_protocol::BootEvidence {
    use esp_hal::rtc_cntl::SocResetReason as Soc;
    use oer_hil_protocol::{BootEvidence, ResetReason};
    let raw = esp_hal::system::reset_reason();
    BootEvidence {
        reset_reason: match raw {
            Some(Soc::CoreSw) => ResetReason::Software,
            Some(Soc::CoreMwdt1) => ResetReason::MainWatchdog1,
            Some(Soc::ChipPowerOn) => ResetReason::PowerOn,
            Some(Soc::SysBrownOut) => ResetReason::Brownout,
            Some(Soc::CoreMwdt0) => ResetReason::MainWatchdog0,
            Some(Soc::CoreRwdt | Soc::CpuRwdt | Soc::SysRwdt) => ResetReason::RtcWatchdog,
            Some(Soc::SysSuperWdt) => ResetReason::SuperWatchdog,
            Some(Soc::CoreUsbJtag | Soc::CoreUsbUart) => ResetReason::UsbSerialJtag,
            Some(Soc::CpuJtag) => ResetReason::Jtag,
            Some(Soc::CpuLockup) => ResetReason::CpuLockup,
            _ => ResetReason::Other,
        },
        raw_reset_reason: raw.map_or(0, |reason| reason as u8),
        post_mortem: postmortem::previous(|previous| previous.map(|previous| previous.summary())),
    }
}

/// Checkpoints `first..` of the previous boot's post-mortem.
pub(super) fn post_mortem_checkpoints(first: u8) -> oer_hil_protocol::PostMortemCheckpoints {
    postmortem::previous(|previous| {
        previous.map_or_else(
            || oer_hil_protocol::PostMortemCheckpoints {
                first,
                checkpoints: Default::default(),
            },
            |previous| previous.page(first),
        )
    })
}
