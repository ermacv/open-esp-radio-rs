//! Isolated-image bridges for compiled verification probes.
//!
//! These functions acquire the PAC singleton inside the HAL and expose only
//! named operations or value results. Probe crates cannot recover the owner.

#![forbid(unsafe_code)]

use crate::types::{MacInterface, TxBlockAckPayload};

macro_rules! forward_unary_u32 {
    ($name:ident) => {
        #[inline(always)]
        pub fn $name(value: u32) -> u32 {
            open_esp_radio_esp32s31_pac::validation::$name(value)
        }
    };
}

#[inline(always)]
pub fn hal_get_sta_tsf(low: Option<&mut u32>, high: Option<&mut u32>) {
    open_esp_radio_esp32s31_pac::validation::hal_get_sta_tsf(low, high);
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_interrupt_get_event()
}

forward_unary_u32!(hal_mac_interrupt_clr_event);

#[inline(always)]
pub fn hal_disable_sta_beacon_filter() {
    open_esp_radio_esp32s31_pac::validation::hal_disable_sta_beacon_filter();
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_pwr_interrupt_get_event()
}

forward_unary_u32!(hal_pwr_interrupt_clr_event);
forward_unary_u32!(hal_mac_rx_disable);
forward_unary_u32!(hal_mac_rx_enable);

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrlast() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_read_rxdscrlast()
}

#[inline(always)]
pub fn hal_mac_rx_read_rxdscrnext() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_read_rxdscrnext()
}

forward_unary_u32!(hal_mac_rx_set_base);

#[inline(always)]
pub fn hal_mac_rx_get_last_dscr() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_get_last_dscr()
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_is_dscr_reload()
}

forward_unary_u32!(hal_mac_rx_set_dscr_reload);
forward_unary_u32!(hal_mac_tx_set_cca);

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state() -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_get_txq_in_trig_flow_state()
}

forward_unary_u32!(hal_mac_is_txq_enabled);
forward_unary_u32!(hal_mac_is_txq_valid);
forward_unary_u32!(hal_mac_set_txq_invalid);
forward_unary_u32!(hal_mac_txq_disable);

#[inline(always)]
pub fn hal_mac_tx_config_edca(
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    open_esp_radio_esp32s31_pac::validation::hal_mac_tx_config_edca(
        queue,
        aifsn,
        contention_window,
        interface,
    )
}

#[inline(always)]
pub fn hal_mac_tx_get_blockack(queue: u8) -> Option<TxBlockAckPayload> {
    open_esp_radio_esp32s31_pac::validation::hal_mac_tx_get_blockack(queue)
}

#[inline(always)]
pub fn hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_pac::validation::hal_mac_set_addr(interface, address);
}

#[inline(always)]
pub fn hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_pac::validation::hal_mac_set_bssid(interface, address);
}

#[inline(always)]
pub fn hal_set_rx_beacon_pti(beacon: u32, shared: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_set_rx_beacon_pti(beacon, shared);
}

#[inline(always)]
pub fn hal_clear_rx_beacon_pti() {
    open_esp_radio_esp32s31_pac::validation::hal_clear_rx_beacon_pti();
}

#[inline(always)]
pub fn hal_set_itwt_pti(control: u32, shared: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_set_itwt_pti(control, shared);
}

#[inline(always)]
pub fn hal_clr_itwt_pti(index: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_clr_itwt_pti(index);
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
    open_esp_radio_esp32s31_pac::validation::hal_set_tx_pti(
        queue,
        scheduler_priority,
        pti_2,
        pti_1,
        pti_0,
        pti_3,
        count,
    );
}

forward_unary_u32!(pwr_hal_set_mac_modem_beacon_miss_timeout);
forward_unary_u32!(pwr_hal_set_mac_modem_beacon_miss_limit);
forward_unary_u32!(pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable);
forward_unary_u32!(pwr_hal_set_mac_modem_state_sleep_limit);
forward_unary_u32!(pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable);
forward_unary_u32!(pwr_hal_set_mac_modem_state_wakeup_protect_enable);
forward_unary_u32!(pwr_hal_set_mac_modem_state_wakeup_protect_early_time);
forward_unary_u32!(pwr_hal_set_mac_modem_tbtt_auto_period_enable);
forward_unary_u32!(pwr_hal_set_mac_modem_tbtt_auto_period_disable);
forward_unary_u32!(pwr_hal_set_mac_modem_tbtt_auto_period_interval);

#[inline(always)]
pub fn set_station_tsf_wakeup(enabled: bool) {
    let mut owner = crate::RadioRuntimeOwner::claim_for_validation();
    owner.pac_mut().set_station_tsf_wakeup(enabled);
}
