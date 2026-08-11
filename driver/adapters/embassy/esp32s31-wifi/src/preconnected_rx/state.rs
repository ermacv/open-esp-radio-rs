use core::marker::PhantomData;

use open_esp_radio_esp32s31_wifi_mac::rx::{RxRingError, RxRingHalted, RxRingLive, RxRingStopped};

/// Hardware-valid RX phase retained by the pre-connected owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31PreconnectedRxPhase {
    Halted,
    Prepared,
    Live,
    Vacant,
}

pub(super) enum Esp32s31PreconnectedRxState<'storage, const COUNT: usize> {
    Halted(RxRingHalted<'storage, COUNT>),
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31PreconnectedRxState<'_, COUNT> {
    pub(super) const fn phase(&self) -> Esp32s31PreconnectedRxPhase {
        match self {
            Self::Halted(_) => Esp32s31PreconnectedRxPhase::Halted,
            Self::Prepared(_) => Esp32s31PreconnectedRxPhase::Prepared,
            Self::Live(_) => Esp32s31PreconnectedRxPhase::Live,
            Self::Vacant => Esp32s31PreconnectedRxPhase::Vacant,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31PreconnectedRxError {
    AlreadyStarted,
    OwnerUnavailable,
    Ring(RxRingError),
}

/// Complete owner return when a finite pre-connected frontier cannot be
/// promoted into the connected live ring.
pub struct Esp32s31PreconnectedRxIntoLiveFailure<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
> {
    pub owner: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub error: Esp32s31PreconnectedRxError,
}

/// Decision made while observing one completed pre-connected descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31PreconnectedRxDirective {
    Continue,
    /// End this bounded service pass after recycling every descriptor already
    /// observed by it. The live RX epoch remains owned by this phase.
    Pause,
    /// Retain the terminal descriptor for a typed transfer into another
    /// protocol phase. This is not a scheduling/batching directive.
    Stop,
}

/// Finite progress returned by one descriptor service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31PreconnectedRxProgress {
    pub completed: u32,
    pub stopped: bool,
}

impl From<RxRingError> for Esp32s31PreconnectedRxError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

/// Unique RX ring owner shared by all finite pre-connected protocol phases.
pub struct Esp32s31PreconnectedRx<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize> {
    pub(super) state: Esp32s31PreconnectedRxState<'storage, COUNT>,
    pub(super) _delay: PhantomData<fn() -> D>,
}
