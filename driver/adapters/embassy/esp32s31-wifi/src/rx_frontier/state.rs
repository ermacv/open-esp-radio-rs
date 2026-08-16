use core::marker::PhantomData;

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxRingTopologySnapshot,
};

/// Hardware-valid phase retained by the finite RX frontier owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxFrontierPhase {
    Halted,
    Prepared,
    Live,
    Vacant,
}

pub(super) enum Esp32s31RxFrontierState<'storage, const COUNT: usize> {
    Halted(RxRingHalted<'storage, COUNT>),
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31RxFrontierState<'_, COUNT> {
    pub(super) const fn phase(&self) -> Esp32s31RxFrontierPhase {
        match self {
            Self::Halted(_) => Esp32s31RxFrontierPhase::Halted,
            Self::Prepared(_) => Esp32s31RxFrontierPhase::Prepared,
            Self::Live(_) => Esp32s31RxFrontierPhase::Live,
            Self::Vacant => Esp32s31RxFrontierPhase::Vacant,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxFrontierError {
    AlreadyStarted,
    OwnerUnavailable,
    Ring(RxRingError),
}

/// Complete owner return when a finite frontier cannot be promoted into the
/// connected live ring.
pub struct Esp32s31RxFrontierIntoLiveFailure<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    pub owner: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub error: Esp32s31RxFrontierError,
}

/// Decision made while observing one completed descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxFrontierDirective {
    Continue,
    /// End this bounded service pass after recycling every descriptor already
    /// observed by it. The live RX epoch remains owned by this phase.
    Pause,
    /// Retain the terminal descriptor for a typed transfer into another
    /// protocol phase. This is not a scheduling/batching directive.
    Stop,
}

/// Scheduler edge required after one finite RX ownership observation.
///
/// The vendor `wdevProcessRxSucDataAll` consumes one PP event and remains in
/// its descriptor loop while refreshing the hardware LAST frontier. Rust may
/// cooperatively yield while waiting for a safe ownership proof, but it must
/// not turn that yield into a requirement for another RX interrupt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Esp32s31RxFrontierContinuation {
    #[default]
    AwaitInterrupt,
    ProbePending,
}

/// Read-only scheduler state retained before a live RX owner is halted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxFrontierSchedulerSnapshot {
    pub recycle_start: usize,
    pub accepted_tail: usize,
    pub observed_mask: u64,
    pub topology: RxRingTopologySnapshot,
}

/// Finite progress returned by one descriptor service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31RxFrontierProgress {
    pub completed: u32,
    pub stopped: bool,
}

/// Progress of one complete descriptor-frontier service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31RxFrontierServiceProgress {
    pub completed_descriptors: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

impl From<RxRingError> for Esp32s31RxFrontierError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

/// Unique RX-ring owner shared by all finite role phases.
pub struct Esp32s31RxFrontier<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize> {
    pub(super) state: Esp32s31RxFrontierState<'storage, COUNT>,
    pub(super) _delay: PhantomData<fn() -> D>,
}
