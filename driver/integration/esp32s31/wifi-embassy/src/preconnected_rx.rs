//! RX descriptor ownership across Authentication, Association and WPA2.
//!
//! These finite phases intentionally share one ring epoch. A halted ring may
//! be prepared and started, Association may leave it live for WPA2, and every
//! retry must retain the last hardware-valid frontier. This owner contains no
//! board storage addresses or protocol policy.

use core::{future::Future, marker::PhantomData};

use embassy_time::Timer;
use open_esp_radio_esp32s31_wifi_lmac::rx::{
    RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
};

use crate::rx_backend::{ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage};

/// Executor edge between walker publication and its first live observation.
pub trait Esp32s31PreconnectedRxDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()>;
}

/// Production Embassy-time delay adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31PreconnectedRxDelay;

impl Esp32s31PreconnectedRxDelay for EmbassyEsp32s31PreconnectedRxDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()> {
        Timer::after_micros(u64::from(micros))
    }
}

/// Hardware-valid RX phase retained by the pre-connected owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31PreconnectedRxPhase {
    Halted,
    Prepared,
    Live,
    Vacant,
}

enum Esp32s31PreconnectedRxState<'storage, const COUNT: usize> {
    Halted(RxRingHalted<'storage, COUNT>),
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31PreconnectedRxState<'_, COUNT> {
    const fn phase(&self) -> Esp32s31PreconnectedRxPhase {
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
    state: Esp32s31PreconnectedRxState<'storage, COUNT>,
    _delay: PhantomData<fn() -> D>,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize>
    Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>
where
    D: Esp32s31PreconnectedRxDelay,
{
    pub const fn from_halted(ring: RxRingHalted<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31PreconnectedRxState::Halted(ring),
            _delay: PhantomData,
        }
    }

    pub const fn from_prepared(ring: RxRingStopped<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31PreconnectedRxState::Prepared(ring),
            _delay: PhantomData,
        }
    }

    pub const fn phase(&self) -> Esp32s31PreconnectedRxPhase {
        self.state.phase()
    }

    /// Move the current frontier into another finite phase while leaving an
    /// explicit vacant placeholder for its eventual returned owner.
    pub fn take(&mut self) -> Result<Self, Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        if matches!(state, Esp32s31PreconnectedRxState::Vacant) {
            return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
        }
        Ok(Self {
            state,
            _delay: PhantomData,
        })
    }

    /// Prepare a halted frontier if needed, wait the qualified walker settle
    /// edge, and publish one live RX epoch.
    pub async fn start<M, F>(
        &mut self,
        hardware: &mut M,
        prepare_buffer: F,
    ) -> Result<(), Esp32s31PreconnectedRxError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        let prepared = match state {
            Esp32s31PreconnectedRxState::Halted(halted) => {
                match halted.prepare(hardware, DMA_BUFFER_SIZE as u32, prepare_buffer) {
                    Ok(prepared) => prepared,
                    Err((halted, error)) => {
                        self.state = Esp32s31PreconnectedRxState::Halted(halted);
                        return Err(error.into());
                    }
                }
            }
            Esp32s31PreconnectedRxState::Prepared(prepared) => prepared,
            live @ Esp32s31PreconnectedRxState::Live(_) => {
                self.state = live;
                return Err(Esp32s31PreconnectedRxError::AlreadyStarted);
            }
            Esp32s31PreconnectedRxState::Vacant => {
                return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
            }
        };
        D::after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31PreconnectedRxState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31PreconnectedRxState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    /// Start RX using the production DMA storage bound to this ring.
    pub fn start_with_storage<'operation, M, const DMA_STORAGE_SIZE: usize>(
        &'operation mut self,
        hardware: &'operation mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> impl Future<Output = Result<(), Esp32s31PreconnectedRxError>> + 'operation
    where
        M: RxDma,
        'storage: 'operation,
    {
        self.start(hardware, move |index| {
            // SAFETY: the halted ring invokes this only after DMA released
            // the matching buffer and immediately before descriptor rearm.
            unsafe { storage.buffers()[index].prepare_for_recycle() }
        })
    }

    pub fn stop<M: RxDma>(&mut self, hardware: &mut M) -> Result<(), Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        let Esp32s31PreconnectedRxState::Live(live) = state else {
            self.state = state;
            return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
        };
        match live.try_stop(hardware) {
            Ok(halted) => {
                self.state = Esp32s31PreconnectedRxState::Halted(halted);
                Ok(())
            }
            Err((live, error)) => {
                self.state = Esp32s31PreconnectedRxState::Live(live);
                Err(error.into())
            }
        }
    }

    pub fn live_mut(
        &mut self,
    ) -> Result<&mut RxRingLive<'storage, COUNT>, Esp32s31PreconnectedRxError> {
        match &mut self.state {
            Esp32s31PreconnectedRxState::Live(ring) => Ok(ring),
            _ => Err(Esp32s31PreconnectedRxError::OwnerUnavailable),
        }
    }

    /// Observe every currently completed descriptor, then recycle the
    /// completed half unless the observer reports a terminal frame.
    ///
    /// The higher-ranked observer lifetime prevents a DMA-buffer reference
    /// from escaping across the recycle edge. A terminal descriptor remains
    /// owned and observed by the live ring for transfer into the next finite
    /// protocol phase.
    pub fn service_completed<M, F, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mut observe: F,
    ) -> Result<Esp32s31PreconnectedRxProgress, Esp32s31PreconnectedRxError>
    where
        M: RxDma,
        F: for<'frame> FnMut(RxSegment<'frame>) -> Esp32s31PreconnectedRxDirective,
    {
        let ring = self.live_mut()?;
        let mut progress = Esp32s31PreconnectedRxProgress::default();
        for index in 0..COUNT {
            let Some(completed) = ring.take_completed(index) else {
                continue;
            };
            progress.completed = progress.completed.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer: unsafe {
                    // SAFETY: taking the completed descriptor transferred the
                    // matching buffer from DMA to this unique live owner.
                    storage.buffers()[index].completed()
                },
                next_descriptor_address: completed.next_descriptor_address(),
            };
            if observe(segment) == Esp32s31PreconnectedRxDirective::Stop {
                progress.stopped = true;
                return Ok(progress);
            }
        }

        ring.recycle_completed_half(hardware, |index| {
            // SAFETY: the live ring invokes this only for a detached completed
            // half immediately before republishing it to DMA.
            unsafe { storage.buffers()[index].prepare_for_recycle() }
        })?;
        if ring.all_observed() {
            return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
        }
        Ok(progress)
    }

    pub fn take_live(
        &mut self,
    ) -> Result<RxRingLive<'storage, COUNT>, Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        match state {
            Esp32s31PreconnectedRxState::Live(ring) => Ok(ring),
            state => {
                self.state = state;
                Err(Esp32s31PreconnectedRxError::OwnerUnavailable)
            }
        }
    }

    /// Consume a finite protocol owner, start its DMA frontier if necessary,
    /// and return the exact live ring required by connected RX.
    ///
    /// The complete pre-connected owner is returned on every failure. The
    /// application/HIL therefore cannot strand a halted or prepared ring
    /// between the WPA2 and connected phases.
    #[allow(clippy::result_large_err)]
    pub async fn try_into_live_with_storage<M, const DMA_STORAGE_SIZE: usize>(
        mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<
        RxRingLive<'storage, COUNT>,
        Esp32s31PreconnectedRxIntoLiveFailure<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    >
    where
        M: RxDma,
    {
        if self.phase() != Esp32s31PreconnectedRxPhase::Live
            && let Err(error) = self.start_with_storage(hardware, storage).await
        {
            return Err(Esp32s31PreconnectedRxIntoLiveFailure { owner: self, error });
        }
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(error) => Err(Esp32s31PreconnectedRxIntoLiveFailure { owner: self, error }),
        }
    }
}

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};

    use open_esp_radio_esp32s31_wifi_lmac::{
        descriptor::{BIT_30, DESCRIPTOR_BYTES},
        rx::{RxDma, RxRingStopped},
    };

    use super::*;
    use crate::rx_backend::Esp32s31RxDmaStorage;

    const COUNT: usize = 2;
    const BUFFER_SIZE: usize = 64;
    const STORAGE_SIZE: usize = 128;
    const BASE: u32 = 0x2f00_1000;
    const BUFFERS: [u32; COUNT] = [0x2f00_2000, 0x2f00_2100];

    struct ReadyDelay;

    impl Esp32s31PreconnectedRxDelay for ReadyDelay {
        fn after_micros(_micros: u32) -> impl Future<Output = ()> {
            ready(())
        }
    }

    #[derive(Default)]
    struct Hardware {
        walker: bool,
        descriptor_base: u32,
    }

    impl RxDma for Hardware {
        fn last_descriptor_low(&mut self) -> u32 {
            0
        }
        fn next_descriptor_low(&mut self) -> u32 {
            BASE + DESCRIPTOR_BYTES
        }
        fn walker_enabled(&mut self) -> bool {
            self.walker
        }
        fn reload_pending(&mut self) -> bool {
            false
        }
        fn set_descriptor_high_window(&mut self, _address_high: u16) {}
        fn write_descriptor_base(&mut self, address: u32) {
            self.descriptor_base = address;
        }
        fn publish_walker_enable(&mut self) {
            self.walker = true;
        }
        fn request_reload(&mut self) {}
        fn try_enable_walker(&mut self) -> bool {
            if self.walker {
                false
            } else {
                self.walker = true;
                true
            }
        }
        fn try_disable_walker(&mut self) -> bool {
            if self.walker {
                self.walker = false;
                true
            } else {
                false
            }
        }
        fn fence(&mut self) {}
    }

    fn halted_ring<'a>(
        hardware: &mut Hardware,
        storage: &'a Esp32s31RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>,
    ) -> RxRingHalted<'a, COUNT> {
        RxRingStopped::prepare(
            hardware,
            storage.descriptors(),
            BASE,
            &BUFFERS,
            BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap()
        .into_halted()
    }

    #[test]
    fn owner_services_a_terminal_descriptor_and_round_trips_between_phases() {
        let storage = Esp32s31RxDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
        let mut hardware = Hardware::default();
        let mut rx = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(
            halted_ring(&mut hardware, &storage),
        );
        embassy_futures::block_on(rx.start_with_storage(&mut hardware, &storage)).unwrap();
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Live);

        storage.descriptors()[0].write_word0(storage.descriptors()[0].word0() | BIT_30);
        let progress = rx
            .service_completed(&mut hardware, &storage, |segment| {
                assert_eq!(segment.descriptor_address, BASE);
                assert_eq!(segment.buffer.len(), BUFFER_SIZE);
                Esp32s31PreconnectedRxDirective::Stop
            })
            .unwrap();
        assert_eq!(
            progress,
            Esp32s31PreconnectedRxProgress {
                completed: 1,
                stopped: true,
            }
        );

        let mut moved = rx.take().unwrap();
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Vacant);
        assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Live);
        moved.stop(&mut hardware).unwrap();
        assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Halted);
        rx = moved;
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Halted);
    }

    #[test]
    fn consuming_connected_promotion_returns_live_ring_or_exact_owner() {
        let storage = Esp32s31RxDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
        let mut hardware = Hardware::default();
        let rx = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(
            halted_ring(&mut hardware, &storage),
        );
        let live =
            embassy_futures::block_on(rx.try_into_live_with_storage(&mut hardware, &storage))
                .unwrap_or_else(|_| panic!("fresh halted owner must become live"));
        assert_eq!(live.descriptor_base(), BASE);

        let halted = match live.try_stop(&mut hardware) {
            Ok(halted) => halted,
            Err(_) => panic!("mock walker must stop before the failure case"),
        };
        let mut already_live =
            Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(halted);
        embassy_futures::block_on(already_live.start_with_storage(&mut hardware, &storage))
            .expect("finite protocol phase starts the ring");
        let live = embassy_futures::block_on(
            already_live.try_into_live_with_storage(&mut hardware, &storage),
        )
        .unwrap_or_else(|_| panic!("an existing live frontier must not start twice"));
        let halted = match live.try_stop(&mut hardware) {
            Ok(halted) => halted,
            Err(_) => panic!("mock walker must stop after the live handoff"),
        };
        let mut vacant =
            Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(halted);
        let retained = vacant.take().expect("test retains the exact halted owner");
        let failure =
            embassy_futures::block_on(vacant.try_into_live_with_storage(&mut hardware, &storage))
                .err()
                .expect("vacant frontier must return its placeholder owner");
        assert_eq!(failure.error, Esp32s31PreconnectedRxError::OwnerUnavailable);
        assert_eq!(failure.owner.phase(), Esp32s31PreconnectedRxPhase::Vacant);
        assert_eq!(retained.phase(), Esp32s31PreconnectedRxPhase::Halted);
    }
}
