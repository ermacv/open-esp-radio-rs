//! Isolated-image bridges for compiled production probes.
//!
//! This is the only layer that adapts vendor-shaped scalar ABIs to typed HAL
//! operations.  The PAC validation module only constructs finite owners; it
//! does not own these names, sequences, or semantic conversions.

#![forbid(unsafe_code)]

use crate::types::{
    MacInterface, MacInterruptSnapshot, MacPowerInterruptSnapshot, TxBlockAckPayload,
};
use crate::{
    RadioRuntimeOwner, validation_mac_interrupt_registers, validation_mac_power_interrupt_registers,
};

#[inline(always)]
fn owner() -> RadioRuntimeOwner {
    RadioRuntimeOwner::claim_for_validation()
}

#[inline(always)]
pub fn hal_get_sta_tsf(low: Option<&mut u32>, high: Option<&mut u32>) {
    owner().pac().validation_station_tsf(low, high);
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event() -> u32 {
    validation_mac_interrupt_registers()
        .mac_interrupt_status()
        .bits()
}

#[inline(always)]
pub fn hal_mac_interrupt_clr_event(events: u32) -> u32 {
    let mut interrupts = validation_mac_interrupt_registers();
    interrupts.acknowledge_mac_interrupts(MacInterruptSnapshot::for_validation(events));
    events
}

#[inline(always)]
pub fn hal_disable_sta_beacon_filter() {
    let mut owner = owner();
    let mut interrupts = validation_mac_interrupt_registers();
    owner
        .pac_mut()
        .validation_disable_sta_beacon_filter(&mut interrupts.inner);
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event() -> u32 {
    validation_mac_power_interrupt_registers()
        .power_interrupt_status()
        .bits()
}

#[inline(always)]
pub fn hal_pwr_interrupt_clr_event(events: u32) -> u32 {
    let mut interrupts = validation_mac_power_interrupt_registers();
    interrupts.acknowledge_power_interrupts(MacPowerInterruptSnapshot::for_validation(events));
    events
}

#[inline(always)]
pub fn hal_mac_rx_disable(passthrough: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_set_mac_rx_walker_enabled(false);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_enable(passthrough: u32) -> u32 {
    owner().pac_mut().validation_set_mac_rx_walker_enabled(true);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrlast() -> u32 {
    owner().wifi_mac_hal().rx_last_descriptor_word()
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrnext() -> u32 {
    owner().wifi_mac_hal().rx_next_descriptor_word()
}

#[inline(always)]
pub fn hal_mac_rx_set_base(address: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_write_mac_rx_descriptor_base(address);
    address
}

#[inline(always)]
pub fn hal_mac_rx_get_last_dscr() -> u32 {
    owner().pac().mac_rx_last_descriptor_address()
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload() -> u32 {
    u32::from(owner().pac().mac_rx_reload_pending())
}

#[inline(always)]
pub fn hal_mac_rx_set_dscr_reload(passthrough: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_request_mac_rx_descriptor_reload();
    passthrough
}

#[inline(always)]
pub fn hal_mac_tx_set_cca(value: u32) -> u32 {
    owner().pac_mut().validation_set_mac_tx_cca(value)
}

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state() -> u32 {
    owner().pac().validation_mac_tx_trigger_flow_state()
}

#[inline(always)]
pub fn hal_mac_is_txq_enabled(queue: u32) -> u32 {
    u32::from(owner().pac().validation_mac_tx_queue_enabled(queue))
}

#[inline(always)]
pub fn hal_mac_is_txq_valid(queue: u32) -> u32 {
    u32::from(owner().pac().validation_mac_tx_queue_valid(queue))
}

#[inline(always)]
pub fn hal_mac_set_txq_invalid(queue: u32) -> u32 {
    owner().pac_mut().validation_invalidate_mac_tx_queue(queue)
}

#[inline(always)]
pub fn hal_mac_txq_disable(queue: u32) -> u32 {
    owner().pac_mut().validation_disable_mac_tx_queue(queue)
}

#[inline(always)]
pub fn hal_mac_tx_config_edca(
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    owner()
        .pac_mut()
        .validation_configure_mac_tx_edca(queue, aifsn, contention_window, interface)
}

#[inline(always)]
pub fn hal_mac_tx_get_blockack(queue: u8) -> Option<TxBlockAckPayload> {
    owner().pac().read_tx_block_ack_payload(queue)
}

#[inline(always)]
fn mac_interface(interface: u32) -> MacInterface {
    match interface {
        0 => MacInterface::Station,
        1 => MacInterface::AccessPoint,
        2 => MacInterface::Context2,
        3 => MacInterface::Context3,
        _ => panic!("verification MAC interface is out of range"),
    }
}

#[inline(always)]
pub fn hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    owner()
        .wifi_mac_hal()
        .program_interface_address(mac_interface(interface), *address);
}

#[inline(always)]
pub fn hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    owner()
        .wifi_mac_hal()
        .program_interface_bssid(mac_interface(interface), *address);
}
