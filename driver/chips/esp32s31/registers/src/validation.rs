//! Feature-gated compiled-validation access to qualified register transactions.
//!
//! This module is never enabled by the runtime. It exposes the exact safe
//! transaction while keeping lifecycle ownership in the generated PAC.

#![forbid(unsafe_code)]

use crate::{MacInterruptRegisters, svd};

/// Construct the ordinary task-owned register partition for an isolated probe.
#[inline(always)]
pub fn radio_registers() -> crate::RadioRegisters {
    let peripherals = svd::peripheral_ownership::peripherals_for_validation();
    let (radio, _) = svd::peripheral_ownership::split(peripherals);
    crate::RadioRegisters::from_peripherals(radio)
}

#[inline(always)]
pub fn hal_get_sta_tsf(
    registers: &svd::WifiMacStaTsfLoad,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    crate::mac_tsf::snapshot_station_tsf(registers, low, high);
}

/// Construct the real finite production capability in an isolated probe ELF.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    let peripherals = svd::peripheral_ownership::peripherals_for_validation();
    let (_, interrupts) = svd::peripheral_ownership::split(peripherals);
    MacInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_interrupt)
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event(registers: &svd::WifiMacInterrupt) -> u32 {
    svd::interrupt_snapshot::sample_mac_interrupt(registers).bits()
}

#[inline(always)]
pub fn hal_mac_interrupt_clr_event(registers: &svd::WifiMacInterrupt, events: u32) -> u32 {
    svd::interrupt_snapshot::acknowledge_mac_interrupt(
        registers,
        svd::interrupt_snapshot::mac_interrupt_for_validation(events),
    );
    events
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event(registers: &svd::WifiMacPowerInterrupt) -> u32 {
    svd::interrupt_snapshot::sample_mac_power_interrupt(registers).bits()
}

#[inline(always)]
pub fn hal_pwr_interrupt_clr_event(registers: &svd::WifiMacPowerInterrupt, events: u32) -> u32 {
    svd::interrupt_snapshot::acknowledge_mac_power_interrupt(
        registers,
        svd::interrupt_snapshot::mac_power_interrupt_for_validation(events),
    );
    events
}

#[inline(always)]
pub fn hal_mac_rx_disable(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    crate::mac_rx_dma::set_walker_enabled(registers, false);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_enable(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    crate::mac_rx_dma::set_walker_enabled(registers, true);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrlast(registers: &svd::WifiMacRxDma) -> u32 {
    crate::mac_rx_dma::read_last_descriptor(registers)
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrnext(registers: &svd::WifiMacRxDma) -> u32 {
    crate::mac_rx_dma::read_next_descriptor(registers)
}

#[inline(always)]
pub fn hal_mac_rx_set_base(registers: &svd::WifiMacRxDma, address: u32) -> u32 {
    crate::mac_rx_dma::write_descriptor_base(registers, address);
    address
}

#[inline(always)]
pub fn hal_mac_rx_get_last_dscr(registers: &svd::WifiMacRxDma) -> u32 {
    crate::mac_rx_dma::read_last_descriptor_address(registers)
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload(registers: &svd::WifiMacRxDma) -> u32 {
    u32::from(crate::mac_rx_dma::descriptor_reload_pending(registers))
}

#[inline(always)]
pub fn hal_mac_rx_set_dscr_reload(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    crate::mac_rx_dma::request_descriptor_reload(registers);
    passthrough
}

#[inline(always)]
pub fn hal_mac_tx_set_cca(registers: &svd::WifiMacTxCommon, value: u32) -> u32 {
    crate::mac_tx_queue::set_cca_force(registers, value)
}

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state(registers: &svd::WifiMacTxCommon) -> u32 {
    crate::mac_tx_queue::trigger_flow_state(registers)
}

#[inline(always)]
pub fn hal_mac_is_txq_enabled(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    u32::from(crate::mac_tx_queue::queue_enabled(registers, queue))
}

#[inline(always)]
pub fn hal_mac_is_txq_valid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    u32::from(crate::mac_tx_queue::queue_valid(registers, queue))
}

#[inline(always)]
pub fn hal_mac_set_txq_invalid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    crate::mac_tx_queue::invalidate_queue(registers, queue)
}

#[inline(always)]
pub fn hal_mac_txq_disable(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    crate::mac_tx_queue::disable_queue(registers, queue)
}

#[inline(always)]
pub fn hal_mac_txq_enable_register_slice(
    registers: &svd::WifiMacTxQueueControl,
    queue: u32,
) -> u32 {
    crate::mac_tx_queue::publish_queue(registers, queue)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_timeout(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    crate::mac_modem_wakeup::set_beacon_miss_timeout(registers, value as u16)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    crate::mac_modem_wakeup::set_beacon_miss_limit(registers, (value & 0x0f) as u8)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    crate::mac_modem_wakeup::enable_beacon_miss_limit_wakeup(registers);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    crate::mac_modem_wakeup::set_modem_state_sleep_limit(registers, (value & 0x03ff) as u16)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    crate::mac_modem_wakeup::enable_modem_state_sleep_limit_wakeup(registers);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    crate::mac_modem_wakeup::enable_modem_state_wakeup_protect(registers);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
    registers: &svd::WifiMacRegdmaControl,
    value: u32,
) -> u32 {
    crate::mac_modem_wakeup::set_wakeup_protect_early_time(registers, value as u16)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_enable(
    registers: &svd::WifiMacRegdmaControl,
    passthrough: u32,
) -> u32 {
    crate::mac_modem_wakeup::enable_tbtt_auto_period(registers);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_disable(
    registers: &svd::WifiMacRegdmaControl,
    passthrough: u32,
) -> u32 {
    crate::mac_modem_wakeup::disable_tbtt_auto_period(registers);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_interval(
    registers: &svd::WifiMacRegdmaControl,
    value: u32,
) -> u32 {
    crate::mac_modem_wakeup::set_tbtt_auto_period(registers, (value & 0x03ff) as u16)
}
