//! ESP32-S31 escalation for a terminal shared-PHY epoch.

use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

/// Establish the shared-RF terminal postcondition by resetting the whole SoC.
///
/// Current local Controller shutdown requires already-idle role owners. It
/// cannot prove that active DTM TX/RX and every shared-RF consumer have stopped.
/// Consequently this backend must escalate; disabling CPU routes or closing
/// Bluetooth HCI is insufficient. This is a system reset, not a CPU-only reset.
/// The pinned ESP-HAL path applies S31 pre-reset handling and invokes the ROM
/// system-reset primitive. No Host acknowledgement or protocol command is awaited.
/// The generic policy remains shared-RF fail-stop, allowing a future verified
/// local RF shutdown to preserve non-RF work without changing that policy.
#[cold]
pub fn fail_stop_shared_phy(reason: SharedPhyFailStop) -> ! {
    let _reason = core::hint::black_box(reason);
    esp_hal::system::software_reset()
}
