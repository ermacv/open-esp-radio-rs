//! ieee802154 register and hardware operations.

pub(crate) mod backend;

pub mod lifecycle;

pub(crate) mod operation;

pub(crate) mod policy;

pub(crate) mod role;

pub(crate) mod tx_power;

pub(crate) mod validation;

#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use role::{Ieee802154EdEventProbeFinished, Ieee802154EventStatusProbeFinished};

pub use lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154ClockCheckpoint, Ieee802154ClockReadback, Ieee802154FoundationCheckpoint,
    Ieee802154ReadbackError, Ieee802154ResetCheckpoint, Ieee802154ResetReadback,
};

pub use operation::{
    Ieee802154OperationEventMaskState, Ieee802154OperationEventObservation,
    Ieee802154OperationPollBudget, Ieee802154OperationRxAbortMaskState, Ieee802154OperationStage,
    Ieee802154PolledOperation, Ieee802154PolledOperationAbortEvidence,
    Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
    Ieee802154PolledOperationResult,
};

pub use policy::{
    IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS, IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS,
    Ieee802154AckTimeout, Ieee802154AckTimeoutError, Ieee802154CcaMode, Ieee802154MacControl,
    Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint, Ieee802154PanIdentity,
};

pub use role::{Ieee802154ClockTransitionFailure, Ieee802154Clocked};

#[cfg(feature = "validation-probes")]
pub use validation::{
    ed_event::{
        Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeEvidence,
        Ieee802154EdEventProbeIsolation, Ieee802154EdEventProbeStop,
    },
    event_status::{
        Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
        Ieee802154EventStatusProbeIsolation, Ieee802154EventStatusProbeStop,
    },
};

pub use role::{
    Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure,
    Ieee802154MacPolicyConfigured, Ieee802154MacPolicyRecovery,
    Ieee802154MacPolicyTransitionFailure, Ieee802154OperationCompleted, Ieee802154OperationFailed,
    Ieee802154Owned, Ieee802154PowerTransitionFailure, Ieee802154Powered, Ieee802154Reset,
    Ieee802154ResetTransitionFailure,
};

pub use tx_power::{
    Ieee802154ResolvedTxPower, Ieee802154TxPowerLevels, Ieee802154TxPowerLevelsError,
};
