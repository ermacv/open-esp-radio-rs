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
        .modify(|_, writer| writer.force().set(value as u8));
    0
}

#[inline(always)]
pub(crate) fn completion_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.complete_state().read();
    match queue {
        0 => state.queue_0().bit_is_set(),
        1 => state.queue_1().bit_is_set(),
        2 => state.queue_2().bit_is_set(),
        3 => state.queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn collision_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.collision_queue_0().bit_is_set(),
        1 => state.collision_queue_1().bit_is_set(),
        2 => state.collision_queue_2().bit_is_set(),
        3 => state.collision_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn timeout_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.timeout_queue_0().bit_is_set(),
        1 => state.timeout_queue_1().bit_is_set(),
        2 => state.timeout_queue_2().bit_is_set(),
        3 => state.timeout_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn queue_in_trigger_flow(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.trigger_flow_queue_0().bit_is_set(),
        1 => state.trigger_flow_queue_1().bit_is_set(),
        2 => state.trigger_flow_queue_2().bit_is_set(),
        3 => state.trigger_flow_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

/// Return the complete vendor ABI projection for the isolated comparison
/// probe. Production completion code selects one named queue field directly.
#[cfg(feature = "validation-probes")]
#[inline(always)]
pub(crate) fn validation_trigger_flow_state(registers: &svd::WifiMacTxCommon) -> u32 {
    let state = registers.queue_state().read();
    u32::from(state.trigger_flow_queue_0().bit_is_set())
        + u32::from(state.trigger_flow_queue_1().bit_is_set()) * 2
        + u32::from(state.trigger_flow_queue_2().bit_is_set()) * 4
        + u32::from(state.trigger_flow_queue_3().bit_is_set()) * 8
        + u32::from(state.trigger_flow_high_unknown().bits()) * 16
}

#[inline(always)]
pub(crate) fn acknowledge_completion(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    let state = registers.complete_clear().read();
    svd::zero_based_field_write::acknowledge_mac_tx_completion(
        registers,
        state.queue_0().bit_is_set() || queue == 0,
        state.queue_1().bit_is_set() || queue == 1,
        state.queue_2().bit_is_set() || queue == 2,
        state.queue_3().bit_is_set() || queue == 3,
        state.high_state_unknown().bits(),
    );
}

#[inline(always)]
pub(crate) fn acknowledge_collision(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    svd::zero_based_field_write::clear_mac_tx_collision_state(
        registers,
        queue == 0,
        queue == 1,
        queue == 2,
        queue == 3,
    );
}

#[inline(always)]
pub(crate) fn acknowledge_timeout(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    svd::zero_based_field_write::clear_mac_tx_timeout_state(
        registers,
        queue == 0,
        queue == 1,
        queue == 2,
        queue == 3,
    );
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
