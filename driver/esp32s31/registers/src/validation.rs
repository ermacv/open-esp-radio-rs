//! Feature-gated compiled-validation access to generated vendor leaves.
//!
//! This module is never enabled by the runtime. It exposes the exact generated
//! transaction while keeping lifecycle ownership and `steal` in the HIL probe.

use crate::{MacInterruptRegisters, generated, svd};

#[inline(always)]
pub fn hal_get_sta_tsf(
    registers: &svd::WifiMacStaTsfLoad,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    generated::hal_get_sta_tsf::generated_hal_get_sta_tsf(registers, low, high);
}

/// Construct the real finite production capability in an isolated probe ELF.
///
/// # Safety
///
/// The caller must ensure that the probe is the only owner of this peripheral.
#[inline(always)]
pub unsafe fn mac_interrupt_registers() -> MacInterruptRegisters {
    // SAFETY: the caller accepts the validation-only uniqueness contract.
    unsafe { MacInterruptRegisters::steal_for_validation() }
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event(registers: &svd::WifiMacInterrupt) -> u32 {
    generated::hal_mac_interrupt_get_event::generated_hal_mac_interrupt_get_event(registers)
}

#[inline(always)]
pub fn hal_mac_interrupt_clr_event(registers: &svd::WifiMacInterrupt, events: u32) -> u32 {
    generated::hal_mac_interrupt_clr_event::generated_hal_mac_interrupt_clr_event(registers, events)
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event(registers: &svd::WifiMacPowerInterrupt) -> u32 {
    generated::hal_pwr_interrupt_get_event::generated_hal_pwr_interrupt_get_event(registers)
}

#[inline(always)]
pub fn hal_pwr_interrupt_clr_event(registers: &svd::WifiMacPowerInterrupt, events: u32) -> u32 {
    generated::hal_pwr_interrupt_clr_event::generated_hal_pwr_interrupt_clr_event(registers, events)
}

#[inline(always)]
pub fn hal_mac_rx_disable(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    generated::hal_mac_rx_disable::generated_hal_mac_rx_disable(registers, passthrough)
}

#[inline(always)]
pub fn hal_mac_rx_enable(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    generated::hal_mac_rx_enable::generated_hal_mac_rx_enable(registers, passthrough)
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrlast(registers: &svd::WifiMacRxDma) -> u32 {
    generated::hal_mac_rx_read_rxdscrlast::generated_hal_mac_rx_read_rxdscrlast(registers)
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrnext(registers: &svd::WifiMacRxDma) -> u32 {
    generated::hal_mac_rx_read_rxdscrnext::generated_hal_mac_rx_read_rxdscrnext(registers)
}

#[inline(always)]
pub fn hal_mac_rx_set_base(registers: &svd::WifiMacRxDma, address: u32) -> u32 {
    generated::hal_mac_rx_set_base::generated_hal_mac_rx_set_base(registers, address)
}

#[inline(always)]
pub fn hal_mac_rx_get_last_dscr(registers: &svd::WifiMacRxDma) -> u32 {
    generated::hal_mac_rx_get_last_dscr::generated_hal_mac_rx_get_last_dscr(registers)
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload(registers: &svd::WifiMacRxDma) -> u32 {
    generated::hal_mac_rx_is_dscr_reload::generated_hal_mac_rx_is_dscr_reload(registers)
}

#[inline(always)]
pub fn hal_mac_rx_set_dscr_reload(registers: &svd::WifiMacRxDma, passthrough: u32) -> u32 {
    generated::hal_mac_rx_set_dscr_reload::generated_hal_mac_rx_set_dscr_reload(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn hal_mac_tx_set_cca(registers: &svd::WifiMacTxCommon, value: u32) -> u32 {
    generated::hal_mac_tx_set_cca::generated_hal_mac_tx_set_cca(registers, value)
}

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state(registers: &svd::WifiMacTxCommon) -> u32 {
    generated::hal_mac_get_txq_in_trig_flow_state::generated_hal_mac_get_txq_in_trig_flow_state(
        registers,
    )
}

#[inline(always)]
pub fn hal_mac_is_txq_enabled(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    generated::hal_mac_is_txq_enabled::generated_hal_mac_is_txq_enabled(registers, queue)
}

#[inline(always)]
pub fn hal_mac_is_txq_valid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    generated::hal_mac_is_txq_valid::generated_hal_mac_is_txq_valid(registers, queue)
}

#[inline(always)]
pub fn hal_mac_set_txq_invalid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    generated::hal_mac_set_txq_invalid::generated_hal_mac_set_txq_invalid(registers, queue)
}

#[inline(always)]
pub fn hal_mac_txq_disable(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    generated::hal_mac_txq_disable::generated_hal_mac_txq_disable(registers, queue)
}

#[inline(always)]
pub fn hal_mac_txq_enable_register_slice(
    registers: &svd::WifiMacTxQueueControl,
    queue: u32,
) -> u32 {
    generated::hal_mac_txq_enable_register_slice::generated_hal_mac_txq_enable_register_slice(
        registers, queue,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_timeout(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_beacon_miss_timeout::generated_pwr_hal_set_mac_modem_beacon_miss_timeout(
        registers, value,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_beacon_miss_limit::generated_pwr_hal_set_mac_modem_beacon_miss_limit(
        registers, value,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable::generated_pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_state_sleep_limit::generated_pwr_hal_set_mac_modem_state_sleep_limit(
        registers, value,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable::generated_pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_enable(
    registers: &svd::WifiMacRtcTimerUpdate,
    passthrough: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_state_wakeup_protect_enable::generated_pwr_hal_set_mac_modem_state_wakeup_protect_enable(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
    registers: &svd::WifiMacRegdmaControl,
    value: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_state_wakeup_protect_early_time::generated_pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
        registers, value,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_enable(
    registers: &svd::WifiMacRegdmaControl,
    passthrough: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_tbtt_auto_period_enable::generated_pwr_hal_set_mac_modem_tbtt_auto_period_enable(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_disable(
    registers: &svd::WifiMacRegdmaControl,
    passthrough: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_tbtt_auto_period_disable::generated_pwr_hal_set_mac_modem_tbtt_auto_period_disable(
        registers,
        passthrough,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_interval(
    registers: &svd::WifiMacRegdmaControl,
    value: u32,
) -> u32 {
    generated::pwr_hal_set_mac_modem_tbtt_auto_period_interval::generated_pwr_hal_set_mac_modem_tbtt_auto_period_interval(
        registers, value,
    )
}
