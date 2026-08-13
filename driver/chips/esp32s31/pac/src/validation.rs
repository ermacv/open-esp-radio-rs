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
pub fn hal_mac_tx_config_edca(
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: crate::MacInterface,
) -> u32 {
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
fn validation_mac_interface(interface: u32) -> crate::MacInterface {
    match interface {
        0 => crate::MacInterface::Station,
        1 => crate::MacInterface::AccessPoint,
        2 => crate::MacInterface::Context2,
        3 => crate::MacInterface::Context3,
        _ => panic!("verification MAC interface is out of range"),
    }
}

/// Execute the production closed-PAC replacement for complete
/// `libpp::hal_mac_set_addr` while retaining the vendor pointer ABI.
#[inline(always)]
pub fn hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    radio_registers()
        .program_receive_interface_address(validation_mac_interface(interface), *address);
}

/// Execute the production closed-PAC replacement for complete
/// `libpp::hal_mac_set_bssid` while retaining the vendor pointer ABI.
#[inline(always)]
pub fn hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    radio_registers().program_interface_bssid(validation_mac_interface(interface), *address);
}

#[inline(always)]
pub fn hal_set_rx_beacon_pti(beacon: u32, shared: u32) {
    // The complete vendor leaf consumes only each argument's low nibble. Mask
    // first so the validation bridge has the same total input domain while
    // production callers still require the closed `MacPti` type.
    let beacon = crate::MacPti::new(beacon & 0x0f).expect("masked PTI fits four bits");
    let shared = crate::MacPti::new(shared & 0x0f).expect("masked PTI fits four bits");
    radio_registers().set_rx_beacon_pti(beacon, shared);
}

#[inline(always)]
pub fn hal_clear_rx_beacon_pti() {
    radio_registers().clear_rx_beacon_pti();
}

#[inline(always)]
pub fn hal_set_itwt_pti(control: u32, shared: u32) {
    let shared = crate::MacPti::new(shared & 0x0f).expect("masked PTI fits four bits");
    radio_registers().set_itwt_pti(control == 0, shared);
}

#[inline(always)]
pub fn hal_clr_itwt_pti(index: u32) {
    let index = crate::MacItwtClearIndex::new(index & 31).expect("masked shift index fits");
    radio_registers().clear_itwt_pti(index);
}

#[inline(always)]
pub fn hal_set_tx_pti(
    queue: u32,
    scheduler_priority: u32,
    pti_2: u32,
    pti_1: u32,
    pti_0: u32,
    pti_3: u32,
    count: u32,
) {
    let pti = |value| crate::MacPti::new(value & 0x0f).expect("masked PTI fits four bits");
    let queue = crate::MacTxQueueIndex::new(queue).expect("verification queue is in range");
    let count = crate::MacTxPtiCount::new(count & 0x0fff).expect("masked count fits twelve bits");
    radio_registers().set_tx_pti(
        queue,
        crate::MacTxPtiProgram {
            scheduler_priority: pti(scheduler_priority),
            pti_2: pti(pti_2),
            pti_1: pti(pti_1),
            pti_0: pti(pti_0),
            pti_3: pti(pti_3),
            count,
        },
    );
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
