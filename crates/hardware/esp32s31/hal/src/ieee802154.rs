//! ieee802154 register and hardware operations.

pub(crate) mod backend;

pub mod coex;

pub mod lifecycle;

pub mod ll;

pub mod mac;

pub(crate) mod operation;

pub(crate) mod policy;

pub(crate) mod role;

pub mod tx_power;

pub(crate) mod validation;

pub use oer_esp32s31_pac::{Ieee802154MultipanIndex, Ieee802154PowerSequence};

/// The memory the ESP32-S31 IEEE 802.15.4 MAC DMA reaches: the internal SRAM
/// of `SOC_DMA_LOW`..`SOC_DMA_HIGH` in ESP-IDF's
/// `soc/esp32s31/include/soc/soc.h` at
/// `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. PSRAM is outside it: a frame
/// there makes the MAC report a DMA error, and nothing reaches the air or
/// memory. The engine checks its frame buffers against it
/// ([`oer_espressif_ieee802154_engine::engine::Ieee802154Engine::buffers_dma_visible`]).
pub const IEEE802154_DMA_WINDOW: core::ops::Range<usize> = 0x2f00_0000..0x2f08_0000;

#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use role::{Ieee802154EdEventProbeFinished, Ieee802154EventStatusProbeFinished};

/// Validation entries of the IEEE 802.15.4 coexistence and BTBB writes.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use validation::coex_trace;

pub use lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154FoundationCheckpoint, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
    Ieee802154ResetReadback,
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

pub use role::{
    Ieee802154ClockTransitionFailure, Ieee802154Clocked, Ieee802154Cold, Ieee802154Operational,
};

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
    route_retrigger::{
        Ieee802154PolledSameBit, Ieee802154RouteProbeAction, Ieee802154RouteProbeConfig,
        Ieee802154RouteProbeEntry, Ieee802154RouteProbeRegisters, Ieee802154RouteProbeStop,
        Ieee802154SameBitOutcome, Ieee802154TimerEvents, finish_route_probe, route_probe_entry,
        run_polled_same_bit, start_route_probe_phase,
    },
};

pub use role::{
    Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure,
    Ieee802154MacPolicyConfigured, Ieee802154MacPolicyRecovery,
    Ieee802154MacPolicyTransitionFailure, Ieee802154OperationCompleted, Ieee802154OperationFailed,
    Ieee802154PowerTransitionFailure, Ieee802154Powered, Ieee802154Reset,
    Ieee802154ResetTransitionFailure,
};

pub use tx_power::ESP32S31_TX_POWER_LEVELS;
