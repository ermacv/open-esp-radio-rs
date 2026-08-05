//! Checked-in production candidates emitted from closed vendor Effect Contracts.
//!
//! These leaves deliberately remain private. Handwritten capability wrappers
//! own lifecycle, aliasing and domain types; generated code owns only the
//! recovered finite register transaction.

pub(crate) mod hal_get_sta_tsf;
pub(crate) mod hal_mac_get_txq_in_trig_flow_state;
pub(crate) mod hal_mac_is_txq_enabled;
pub(crate) mod hal_mac_is_txq_valid;
pub(crate) mod hal_mac_set_txq_invalid;
pub(crate) mod hal_mac_tx_set_cca;
pub(crate) mod hal_mac_txq_disable;
pub(crate) mod hal_mac_txq_enable_register_slice;
