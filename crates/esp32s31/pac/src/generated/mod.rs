//! Checked-in production candidates emitted from closed vendor Effect Contracts.
//!
//! These leaves deliberately remain private. Handwritten capability wrappers
//! own lifecycle, aliasing and domain types; generated code owns only the
//! recovered finite register transaction.

pub(crate) mod hal_get_sta_tsf;
pub(crate) mod hal_mac_get_txq_in_trig_flow_state;
pub(crate) mod hal_mac_interrupt_clr_event;
pub(crate) mod hal_mac_interrupt_get_event;
pub(crate) mod hal_mac_is_txq_enabled;
pub(crate) mod hal_mac_is_txq_valid;
pub(crate) mod hal_mac_rx_disable;
pub(crate) mod hal_mac_rx_enable;
pub(crate) mod hal_mac_rx_get_last_dscr;
pub(crate) mod hal_mac_rx_is_dscr_reload;
pub(crate) mod hal_mac_rx_read_rxdscrlast;
pub(crate) mod hal_mac_rx_read_rxdscrnext;
pub(crate) mod hal_mac_rx_set_base;
pub(crate) mod hal_mac_rx_set_dscr_reload;
pub(crate) mod hal_mac_set_txq_invalid;
pub(crate) mod hal_mac_tx_set_cca;
pub(crate) mod hal_mac_txq_disable;
pub(crate) mod hal_mac_txq_enable_register_slice;
pub(crate) mod hal_pwr_interrupt_clr_event;
pub(crate) mod hal_pwr_interrupt_get_event;
pub(crate) mod pwr_hal_set_mac_modem_beacon_miss_limit;
pub(crate) mod pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable;
pub(crate) mod pwr_hal_set_mac_modem_beacon_miss_timeout;
pub(crate) mod pwr_hal_set_mac_modem_state_sleep_limit;
pub(crate) mod pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable;
pub(crate) mod pwr_hal_set_mac_modem_state_wakeup_protect_early_time;
pub(crate) mod pwr_hal_set_mac_modem_state_wakeup_protect_enable;
pub(crate) mod pwr_hal_set_mac_modem_tbtt_auto_period_disable;
pub(crate) mod pwr_hal_set_mac_modem_tbtt_auto_period_enable;
pub(crate) mod pwr_hal_set_mac_modem_tbtt_auto_period_interval;
