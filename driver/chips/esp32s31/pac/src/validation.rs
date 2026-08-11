//! Feature-gated compiled-validation access to qualified register transactions.
//!
//! Probe code can execute reviewed production transactions, but it cannot
//! obtain the raw PAC, peripheral pointers, or physical register addresses.

#![forbid(unsafe_code)]

use crate::{MacInterruptRegisters, svd};

#[inline(always)]
fn partitions() -> (
    svd::peripheral_ownership::RadioPeripherals,
    svd::peripheral_ownership::InterruptPeripherals,
) {
    svd::peripheral_ownership::split(svd::peripheral_ownership::peripherals_for_validation())
}

/// Construct the ordinary task-owned register partition for an isolated probe.
#[inline(always)]
pub fn radio_registers() -> crate::RadioRegisters {
    let (radio, _) = partitions();
    crate::RadioRegisters::from_peripherals(radio)
}

#[inline(always)]
pub fn hal_get_sta_tsf(low: Option<&mut u32>, high: Option<&mut u32>) {
    let (radio, _) = partitions();
    crate::mac_tsf::snapshot_station_tsf(&radio.wifi_mac_sta_tsf_load, low, high);
}

/// Construct the real finite production capability in an isolated probe ELF.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    let (_, interrupts) = partitions();
    MacInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_interrupt)
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event() -> u32 {
    let (_, interrupts) = partitions();
    svd::interrupt_snapshot::sample_mac_interrupt(&interrupts.wifi_mac_interrupt).bits()
}

#[inline(always)]
pub fn hal_mac_interrupt_clr_event(events: u32) -> u32 {
    let (_, interrupts) = partitions();
    svd::interrupt_snapshot::acknowledge_mac_interrupt(
        &interrupts.wifi_mac_interrupt,
        svd::interrupt_snapshot::mac_interrupt_for_validation(events),
    );
    events
}

/// Execute the production two-register connected-STA beacon-filter disable.
#[inline(always)]
pub fn hal_disable_sta_beacon_filter() {
    let (radio, interrupts) = partitions();
    crate::mac_interrupt::disable_sta_beacon_filter_for_validation(
        &radio.wifi_mac_sta_beacon_filter,
        &interrupts.wifi_mac_interrupt,
    );
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event() -> u32 {
    let (_, interrupts) = partitions();
    svd::interrupt_snapshot::sample_mac_power_interrupt(&interrupts.wifi_mac_power_interrupt).bits()
}

#[inline(always)]
pub fn hal_pwr_interrupt_clr_event(events: u32) -> u32 {
    let (_, interrupts) = partitions();
    svd::interrupt_snapshot::acknowledge_mac_power_interrupt(
        &interrupts.wifi_mac_power_interrupt,
        svd::interrupt_snapshot::mac_power_interrupt_for_validation(events),
    );
    events
}

#[inline(always)]
pub fn hal_mac_rx_disable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::set_walker_enabled(&radio.wifi_mac_rx_dma, false);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_enable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::set_walker_enabled(&radio.wifi_mac_rx_dma, true);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrlast() -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::read_last_descriptor(&radio.wifi_mac_rx_dma)
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrnext() -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::read_next_descriptor(&radio.wifi_mac_rx_dma)
}

#[inline(always)]
pub fn hal_mac_rx_set_base(address: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::write_descriptor_base(&radio.wifi_mac_rx_dma, address);
    address
}

#[inline(always)]
pub fn hal_mac_rx_get_last_dscr() -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::read_last_descriptor_address(&radio.wifi_mac_rx_dma)
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload() -> u32 {
    let (radio, _) = partitions();
    u32::from(crate::mac_rx_dma::descriptor_reload_pending(
        &radio.wifi_mac_rx_dma,
    ))
}

#[inline(always)]
pub fn hal_mac_rx_set_dscr_reload(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_rx_dma::request_descriptor_reload(&radio.wifi_mac_rx_dma);
    passthrough
}

#[inline(always)]
pub fn hal_mac_tx_set_cca(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::set_cca_force(&radio.wifi_mac_tx_common, value)
}

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state() -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::trigger_flow_state(&radio.wifi_mac_tx_common)
}

#[inline(always)]
pub fn hal_mac_is_txq_enabled(queue: u32) -> u32 {
    let (radio, _) = partitions();
    u32::from(crate::mac_tx_queue::queue_enabled(
        &radio.wifi_mac_tx_queue_control,
        queue,
    ))
}

#[inline(always)]
pub fn hal_mac_is_txq_valid(queue: u32) -> u32 {
    let (radio, _) = partitions();
    u32::from(crate::mac_tx_queue::queue_valid(
        &radio.wifi_mac_tx_queue_control,
        queue,
    ))
}

#[inline(always)]
pub fn hal_mac_set_txq_invalid(queue: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::invalidate_queue(&radio.wifi_mac_tx_queue_control, queue)
}

#[inline(always)]
pub fn hal_mac_txq_disable(queue: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::disable_queue(&radio.wifi_mac_tx_queue_control, queue)
}

#[inline(always)]
pub fn hal_mac_txq_enable_register_slice(queue: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::publish_queue(&radio.wifi_mac_tx_queue_control, queue)
}

#[inline(always)]
pub fn hal_mac_tx_config_edca(queue: u32, aifsn: u8, contention_window: u16, interface: u8) -> u32 {
    let (radio, _) = partitions();
    crate::mac_tx_queue::configure_edca(
        &radio.wifi_mac_tx_queue_control,
        queue,
        aifsn,
        contention_window,
        interface,
    )
}

#[inline(always)]
pub fn hal_mac_tx_get_blockack(queue: u8) -> Option<crate::TxBlockAckPayload> {
    radio_registers().read_tx_block_ack_payload(queue)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_timeout(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::set_beacon_miss_timeout(&radio.wifi_mac_rtc_timer_update, value as u16)
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::set_beacon_miss_limit(
        &radio.wifi_mac_rtc_timer_update,
        (value & 0x0f) as u8,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::enable_beacon_miss_limit_wakeup(&radio.wifi_mac_rtc_timer_update);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::set_modem_state_sleep_limit(
        &radio.wifi_mac_rtc_timer_update,
        (value & 0x03ff) as u16,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::enable_modem_state_sleep_limit_wakeup(
        &radio.wifi_mac_rtc_timer_update,
    );
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_enable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::enable_modem_state_wakeup_protect(&radio.wifi_mac_rtc_timer_update);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_state_wakeup_protect_early_time(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::set_wakeup_protect_early_time(
        &radio.wifi_mac_regdma_control,
        value as u16,
    )
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_enable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::enable_tbtt_auto_period(&radio.wifi_mac_regdma_control);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_disable(passthrough: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::disable_tbtt_auto_period(&radio.wifi_mac_regdma_control);
    passthrough
}

#[inline(always)]
pub fn pwr_hal_set_mac_modem_tbtt_auto_period_interval(value: u32) -> u32 {
    let (radio, _) = partitions();
    crate::mac_modem_wakeup::set_tbtt_auto_period(
        &radio.wifi_mac_regdma_control,
        (value & 0x03ff) as u16,
    )
}
