//! Host-test construction of register sets from validation partitions.

use super::{
    Ieee802154InterruptSetup, Ieee802154TaskParts, Ieee802154TaskRegisters, RadioPartitions,
};

pub(crate) fn ieee802154_task() -> (Ieee802154TaskRegisters, Ieee802154InterruptSetup) {
    let RadioPartitions {
        ieee802154,
        radio_phy,
        coexistence,
        bluetooth,
        shared_radio,
        ..
    } = RadioPartitions::for_validation();
    Ieee802154TaskRegisters::new(Ieee802154TaskParts {
        ieee802154,
        radio_phy,
        coexistence,
        bluetooth,
        shared_radio,
    })
}
