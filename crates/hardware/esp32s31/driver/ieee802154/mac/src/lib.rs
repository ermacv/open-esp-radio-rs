//! Pure transaction planning for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! This leaf joins the DMA ownership tokens to the reviewed, pure IRQ event
//! vocabulary without adding any MMIO, command execution, status
//! acknowledgement, interrupt routing, RF, BTBB, coexistence, or PHY behavior.
//! Its plans are consumed by the audited PAC-backed runtime owner, but this
//! pure leaf alone is not proof that the peripheral is operational.
//!
//! In particular, the plan's external-quiescence requirement is deliberately
//! not a universal `STOP` transition. The pinned vendor driver has
//! state-specific stop side effects and event reconciliation, so this leaf
//! never uses `STOP` as a quiescence predicate. Hardware testing is required
//! only before qualifying a stronger reuse claim. Likewise, this crate consumes
//! already sampled event values and never reads or acknowledges `EVENT_STATUS`.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

mod actor;
mod batch;

pub use actor::{
    MacActive, MacActivePhase, MacBatchOutcome, MacBatchRejectReason, MacBatchRejected,
    MacCommandIntent, MacCompletion, MacDeferred, MacDeferredNext, MacDmaPublication,
    MacEnergyDetectionDuration, MacEnergyDetectionDurationError, MacIntentStep, MacNoDmaResources,
    MacReady, MacResolved, MacResolvedAcknowledgement, MacResolvedAcknowledgementOutcome,
    MacResolvedRx, MacResolvedRxOutcome, MacResolvedTxWithAck, MacRxResolutionFailure,
    MacStartPlan, MacTransmitAccess, MacTransmitAcknowledgement, MacTxWithAckResolutionFailure,
    MacTxWithAckResources,
};
pub use batch::{
    MacBatchConstructionError, MacCcaSample, MacEnergySample, MacEventBatch, MacMeasurementSample,
};
