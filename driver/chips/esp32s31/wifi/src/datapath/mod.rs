//! Executor-neutral DATAPATH contracts shared by ESP32-S31 Wi-Fi roles.
//!
//! DATAPATH coordinates finite LMAC work without owning AP or STA semantics.
//! Role crates consume these value-only contracts while executor adapters own
//! waiting, queues and task placement.

pub mod lifecycle;

/// Result of one bounded RX bottom-half pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathRxProgress {
    /// The durable completion frontier was drained within this pass.
    Drained,
    /// Completed descriptors remain but the role's independent staging is full.
    ///
    /// Descriptor ownership stays with the completed ring until a capacity
    /// wake allows the same head unit to be staged.
    StageCapacityBlocked,
    /// The finite poll budget ended with another completed DMA unit pending.
    BudgetExhausted,
    /// Upper staging was saturated, but bulk units were deliberately dropped
    /// and their descriptors recycled instead of blocking hardware.
    UpperLayerBlockedButDroppable,
    /// A decoded frame is retained while the final network RX queue is full.
    ///
    /// This is not permission to stop the independent DMA frontier: an
    /// executor adapter must also wake for new RX completions so the role can
    /// use any remaining staging credits to republish descriptors.
    NetworkBackpressured,
    /// A republished terminal descriptor needs another hardware observation.
    ProbePending,
}

/// Coherent scheduler facts supplied to one role-control step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatapathControlContext {
    /// A network lease is waiting behind this control boundary.
    pub network_tx_pending: bool,
    /// The finite runner must restore active control state before teardown.
    pub stop_pending: bool,
}

impl DatapathControlContext {
    pub const IDLE: Self = Self {
        network_tx_pending: false,
        stop_pending: false,
    };

    pub const STOPPING: Self = Self {
        network_tx_pending: false,
        stop_pending: true,
    };
}

/// Result of one finite role-control transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathControlProgress<E> {
    Idle,
    More,
    TxPending,
    Exit(E),
}

/// Result of one finite role shutdown transition at an idle TX boundary.
///
/// Roles with no protocol shutdown work use `Stopped` immediately. A role
/// such as an access point may publish a final management transaction and
/// return `TxPending`; DATAPATH drives that transaction to a terminal edge before
/// invoking shutdown again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathStopProgress {
    More,
    TxPending,
    Stopped,
}
