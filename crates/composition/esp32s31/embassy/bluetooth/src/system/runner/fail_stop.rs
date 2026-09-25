//! Compose a terminal shared-PHY decision with the system-reset mechanism.

use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

/// Retain the exact failed frontier until system reset, without running Drop.
///
/// Local shutdown cannot prove active-DTM/all-RF quiescence. This composition
/// therefore escalates a terminal shared-PHY decision to a full system reset.
/// HCI closure belongs to the caller; neither Host acknowledgement nor IRQ
/// cleanup may gate the reset. No timer peripheral belongs to the radio owner.
#[cold]
pub(crate) fn fail_stop_shared_phy<T>(reason: SharedPhyFailStop, owners: T) -> ! {
    let _owners = core::mem::ManuallyDrop::new(owners);
    let _reason = core::hint::black_box(reason);
    oer_esp32s31_soc_esp_hal::reset_system()
}
