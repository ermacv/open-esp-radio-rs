//! Optional observation boundary for the connected RX pipeline.
//!
//! The driver publishes typed, value-only events. It does not prescribe
//! counters, sampling, storage, report formatting or a HIL clock. Attaching an
//! observer must not affect ownership, backpressure or scheduling decisions.

/// One finite DMA service transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxServiceObservation {
    pub frontier: usize,
    pub pool_credits: usize,
    pub queue_credits: usize,
    pub admitted: usize,
    pub staged_bytes: usize,
    pub overload_discarded: usize,
    pub critical_reserve_admitted: usize,
    pub critical_admission_blocked: bool,
    pub micros: u64,
    /// Hardware counter sampled immediately before this service transaction.
    pub hardware_buffer_full_before: Option<u16>,
    /// Hardware counter sampled after descriptor recycling/reload completes.
    pub hardware_buffer_full_after: Option<u16>,
}

/// Length-class discard observed before a malformed unit is recycled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageDiscard {
    Empty,
    TooLong,
    OverloadBulk,
}

/// Result of one bounded publication into the network adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxNetworkPublicationOutcome {
    Enqueued,
    Dropped,
}

/// Value-only observations emitted by the production RX owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPipelineObservation {
    ServiceStarted {
        at_micros: u64,
    },
    ServiceCompleted(RxServiceObservation),
    /// One complete vendor-ordered append/reload transaction.
    ///
    /// The measurement covers the synchronous doorbell-clear loop and its
    /// immediate NEXT/LAST/conditional-BASE suffix. No observer means no
    /// clock reads on the production path.
    ReloadCompleted {
        micros: u64,
    },
    StageDiscarded(RxStageDiscard),
    NetworkReadyWait {
        micros: u64,
    },
    ProtocolDispatched {
        data: bool,
        amsdu: bool,
        amsdu_subframes: u8,
        unit_bytes: usize,
        micros: u64,
    },
    ReorderStarted {
        tid: u8,
        starting_sequence: u16,
        window: u16,
    },
    ReorderStopped,
    ReorderFirst {
        tid: u8,
        start: u16,
        sequence: u16,
    },
    /// One addressed protected QoS MPDU before agreement/reorder handling.
    ReorderIngress {
        active: bool,
        retry: bool,
    },
    ReorderReleased {
        buffered: bool,
        released: u8,
        missing: u16,
        stale: bool,
    },
    ReorderGapExpired,
    ReorderDiscarded,
    ReorderOccupied {
        occupied: u32,
    },
    NetworkPublication {
        bytes: usize,
        micros: u64,
        outcome: RxNetworkPublicationOutcome,
    },
}

/// Non-owning diagnostics hook for the RX pipeline.
///
/// Implementations must be non-blocking and thread-safe. The timestamp source
/// is observer-owned so a firmware which attaches no observer performs no
/// diagnostic clock reads.
pub trait RxPipelineObserver: Sync {
    fn now_micros(&self) -> u64;

    fn observe(&self, observation: RxPipelineObservation);

    fn begin_service(&self) -> u64 {
        let at_micros = self.now_micros();
        self.observe(RxPipelineObservation::ServiceStarted { at_micros });
        at_micros
    }

    fn elapsed_micros_since(&self, started: u64) -> u64 {
        self.now_micros().wrapping_sub(started)
    }
}
