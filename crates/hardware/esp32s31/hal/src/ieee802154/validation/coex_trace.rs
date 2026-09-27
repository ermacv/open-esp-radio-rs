//! Validation-only entries of the IEEE 802.15.4 coexistence and BTBB
//! writes, for comparison with the vendor leaves they port
//! (`hal_set_IEEE802154_TXRX_pti`, `hal_set_IEEE802154_ACK_pti` of
//! libcoexist, `ieee802154_txon_delay_set` of libbtbb).
//!
//! Each takes the IEEE 802.15.4 partition of an isolated validation root
//! and performs exactly the production write: the MAC owner's PTI field
//! lease, or the shared-radio lease's transmit-on delay override.

use core::mem::ManuallyDrop;

use oer_esp32s31_pac::Ieee802154TaskRegisters;

use crate::coex::CoexPti;
use crate::ieee802154::{ll::mac_pti, mac::Ieee802154TaskOwner};
use crate::root::RadioHardware;
use crate::shared_radio::RadioClient;

/// The vendor status of a rejected input: no write.
const REJECTED: u32 = u32::MAX;

/// The IEEE 802.15.4 MAC owner of an isolated validation root.
fn task_owner() -> Ieee802154TaskOwner {
    let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let (task, _interrupts) = Ieee802154TaskRegisters::new(partitions.ieee802154.into_mac());
    Ieee802154TaskOwner::new(task)
}

/// `hal_set_IEEE802154_TXRX_pti`: replace the TX/RX PTI field, as the
/// low-level `set_txrx_pti` does. Zero, or [`REJECTED`] without a write for
/// a priority outside the four-bit domain.
pub fn set_txrx_pti(pti: u32) -> u32 {
    let Some(pti) = u8::try_from(pti).ok().and_then(CoexPti::new) else {
        return REJECTED;
    };
    task_owner().lease().set_txrx_pti(mac_pti(pti));
    0
}

/// `hal_set_IEEE802154_ACK_pti`: replace the ACK PTI field, as the
/// low-level `set_ack_pti` does. Zero, or [`REJECTED`] without a write.
pub fn set_ack_pti(pti: u32) -> u32 {
    let Some(pti) = u8::try_from(pti).ok().and_then(CoexPti::new) else {
        return REJECTED;
    };
    task_owner().lease().set_ack_pti(mac_pti(pti));
    0
}

/// `ieee802154_txon_delay_set`: the shared-radio lease's IEEE 802.15.4
/// transmit-on delay override, with IEEE 802.15.4 holding BTBB. Zero, or
/// [`REJECTED`] when the lease refuses it.
pub fn override_tx_on_delay() -> u32 {
    let (mut radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let (task, _interrupts) = Ieee802154TaskRegisters::new(partitions.ieee802154.into_mac());
    radio.hold_btbb_for_validation(RadioClient::Ieee802154);
    // The validation lease stays unreleased, as the grant-protect probes
    // keep theirs: releasing it would add its own register traffic.
    let mut lease = ManuallyDrop::new(radio.lease_for_validation());
    match lease.override_ieee802154_tx_on_delay(&task) {
        Ok(()) => 0,
        Err(_) => REJECTED,
    }
}
