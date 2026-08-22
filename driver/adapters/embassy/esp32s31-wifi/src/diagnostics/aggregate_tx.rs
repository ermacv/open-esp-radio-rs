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
    /// One negotiated TX BlockAck agreement changed its operational state.
    /// This is a protocol-control edge, not evidence that an aggregate has
    /// already been published.
    BlockAckOperational {
        tid: u8,
        operational: bool,
    },
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
    /// Actual PHY vector bound to a prepared aggregate. This observation is
    /// role-neutral: AP qualification must not depend on the associated-STA
    /// link snapshot to describe its own transmission.
    RateSelected {
        bandwidth_mhz: u16,
        nominal_kbps: u32,
    },
    Prepared {
        subframes: u8,
        stop: AggregateBuildStop,
    },
    PreparationCompleted {
        micros: u64,
    },
    StandbyPrepared,
    StandbyPublished,
    StandbyCancelled,
    Published {
        /// Clock image captured immediately before queue programming began.
        at_micros: u64,
        program_micros: u64,
    },
    /// One detached hardware A-MPDU completion after BlockAck classification.
    BlockAckProcessed {
        tx_status: u8,
        block_ack_received: bool,
        control: u8,
        first_sequence: u16,
        starting_sequence: u16,
        subframes: u8,
        missing: u8,
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
    /// Observer-owned clock. The production path performs no diagnostic clock
    /// read when no observer is attached.
    fn now_micros(&self) -> u64;

    fn observe(&self, observation: AggregateTxObservation);

    /// Observe one AP Ethernet lease exactly when the radio claims it from
    /// the network frontier, before encoding mutates its backing.
    ///
    /// The borrowed bytes are diagnostic input only and must not be retained.
    /// The default keeps ordinary production observers value-only.
    fn observe_access_point_network_claim(&self, _ethernet: &[u8]) {}
}
