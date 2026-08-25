//! Safe generated-PAC transactions for ordinary EDCA TX queue ownership.

#![forbid(unsafe_code)]

use super::{MacInterface, svd};

const ORDINARY_QUEUE_COUNT: u32 = 4;
const LAST_CONTROL_ADDRESS: u32 = 0x2010_4d70;
const CONTROL_STRIDE: u32 = 0x10;

#[inline(always)]
const fn physical_bank(queue: u32) -> usize {
    assert!(
        queue < ORDINARY_QUEUE_COUNT,
        "ordinary TX queue is out of range"
    );
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

#[inline(always)]
const fn control_address(queue: u32) -> u32 {
    assert!(
        queue < ORDINARY_QUEUE_COUNT,
        "ordinary TX queue is out of range"
    );
    LAST_CONTROL_ADDRESS - queue * CONTROL_STRIDE
}

#[inline(always)]
pub(crate) fn set_cca_force(registers: &svd::WifiMacTxCommon, value: u32) -> u32 {
    registers
        .cca_control()
        .modify(|_, writer| writer.force().set((value & 3) as u8));
    0
}

#[inline(always)]
pub(crate) fn trigger_flow_state(registers: &svd::WifiMacTxCommon) -> u32 {
    u32::from(registers.queue_state().read().trigger_flow().bits())
}

#[inline(always)]
pub(crate) fn queue_enabled(registers: &svd::WifiMacTxQueueControl, queue: u32) -> bool {
    registers
        .control(physical_bank(queue))
        .read()
        .enable()
        .bit()
}

#[inline(always)]
pub(crate) fn queue_valid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> bool {
    registers.control(physical_bank(queue)).read().valid().bit()
}

#[inline(always)]
pub(crate) fn invalidate_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().clear_bit());
    control_address(queue)
}

#[inline(always)]
pub(crate) fn disable_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().clear_bit().enable().clear_bit());
    control_address(queue)
}

#[inline(always)]
pub(crate) fn publish_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().set_bit().enable().set_bit());
    0
}

/// Publish the three independent EDCA fields for one ordinary logical queue.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::hal_mac_tx_config_edca`.
/// Each field is a separate fresh-read RMW edge in this exact order.
#[inline(always)]
pub(crate) fn configure_edca(
    registers: &svd::WifiMacTxQueueControl,
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    let config = registers.config(physical_bank(queue));
    config.modify(|_, writer| writer.aifsn().set(aifsn));
    config.modify(|_, writer| writer.contention_window().set(contention_window));
    config.modify(|_, writer| writer.interface().set(interface.bits() as u8));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_queues_reverse_the_four_physical_banks() {
        assert_eq!(physical_bank(0), 3);
        assert_eq!(physical_bank(1), 2);
        assert_eq!(physical_bank(2), 1);
        assert_eq!(physical_bank(3), 0);
    }
}
