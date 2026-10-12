//! The old lower-MAC attempt shape, which the ESP32-S31 direct path still
//! submits until its datapath moves onto the phase.

use crate::tx::{TxAttempt, TxPayload};

/// A single attempt: one MPDU, its header in a backend
/// buffer `B` and its body, if any, the owner `O`.
pub type MpduAttempt<B, O> = TxAttempt<TxPayload<B, O>>;
