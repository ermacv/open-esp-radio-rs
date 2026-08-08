//! Optional observation boundary for connected aggregate TX.
//!
//! The driver publishes typed, value-only events. It does not prescribe
//! counters, histograms, storage or report formatting. Attaching an observer
//! must not affect retry, queue, DMA ownership or scheduling decisions.

/// Why a network frame used the ordinary MPDU path instead of starting an
/// aggregate exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkSingleMpduReason {
    LegacyRate,
    BlockAckUnavailable,
    HtNeedsPair,
    FreshAggregateCapacity,
}

/// The first resource boundary reached while building one aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateBuildStop {
    FrameLimit,
    CapacityLimit,
    QueueEmpty,
}

/// Value-only observations emitted by the production aggregate TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxObservation {
    /// A coalesced hardware TX interrupt has reached the connected TX owner.
    ///
    /// This timestamp is emitted only when an observer is attached. It lets a
    /// HIL observer correlate the hard-ISR publication with bottom-half
    /// service without placing timing policy in the production owner.
    InterruptServiceStarted {
        at_micros: u64,
    },
    NetworkSingleMpdu {
        reason: NetworkSingleMpduReason,
        ethernet_length: usize,
    },
    Prepared {
        subframes: u8,
        stop: AggregateBuildStop,
    },
    PreparationCompleted {
        micros: u64,
    },
    Published {
        /// Clock image captured immediately before queue programming began.
        at_micros: u64,
        program_micros: u64,
    },
    Completed {
        acknowledged: u8,
        individual_retry: bool,
    },
    HardwareTimeout,
    Collision,
    ExchangeCompleted {
        micros: u64,
        /// Number of hardware aggregate publications required to reach the
        /// terminal result, including the initial publication.
        publications: u8,
    },
}

/// Non-owning diagnostics hook for aggregate TX.
///
/// Implementations must be non-blocking and thread-safe. The production
/// owner emits no events and performs no observation-only clock reads when no
/// observer is attached.
pub trait AggregateTxObserver: Sync {
    fn observe(&self, observation: AggregateTxObservation);
}
