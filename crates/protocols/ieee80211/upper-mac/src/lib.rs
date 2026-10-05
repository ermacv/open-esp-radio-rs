#![no_std]
#![forbid(unsafe_code)]

//! Portable IEEE 802.11 transmit policy above the lower-MAC port.
//!
//! One lower-MAC submission is one hardware transmission attempt
//! (`oer-ieee80211-lower-mac`). Everything a backend does not do on its own
//! lies above that port and is decided here, once for every backend:
//!
//! - [`retry`]: the retry counters, limits and Retry bit of one MPDU, and the
//!   [`RateLadder`] its attempts walk;
//! - [`aggregate`]: how many queued frames one A-MPDU carries;
//! - [`ampdu`]: which subframes of an A-MPDU the next attempt resends after
//!   a BlockAck, with their Retry bit, and when the aggregate ends;
//! - [`protection`]: RTS/CTS or CTS-to-self before each PPDU;
//! - [`tx`]: the [`TxPlanner`], which combines them with the
//!   EDCA backoff draw of `oer-ieee80211-softmac` into one attempt at a time
//!   and turns each completion into the next attempt or the exchange's
//!   statistics.
//!
//! The package is sans-IO: completions and the radio time enter as values,
//! attempt plans and reports leave as values, and nothing waits. The driver
//! that submits the plans to a port and awaits its events is
//! `oer-ieee80211-upper-mac-service`. Vendor retry schedules, limits and
//! duration estimates are parameters a profile supplies, such as
//! `oer-espressif-ieee80211-policy`.

pub mod aggregate;
pub mod ampdu;
pub mod protection;
pub mod retry;
pub mod tx;

pub use ampdu::{
    AmpduAttemptResult, AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
};
pub use protection::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold, HeTxopRtsBudget,
    ProtectEveryHeTxop, ProtectedPpdu, ProtectionPolicy, RtsLengthThreshold, TxProtection,
    TxProtectionDecision, TxProtectionReasons, TxPsdu, TxReceiver,
};
pub use retry::{
    AckFailureAccounting, ContentionUpdate, FixedRate, FrameClass, MpduRetryState, RateLadder,
    RetryCounters, RetryDecision, RetryLimits, RetryOutcome, RetryStep,
};
pub use tx::{
    AmpduRequest, AttemptContent, MpduRequest, TxAttemptPlan, TxBody, TxExchange, TxPlanError,
    TxPlanner, TxReport, TxRequest, TxStep,
};
