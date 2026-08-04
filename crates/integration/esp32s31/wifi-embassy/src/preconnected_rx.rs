//! RX descriptor ownership across Authentication, Association and WPA2.
//!
//! These finite phases intentionally share one ring epoch. A halted ring may
//! be prepared and started, Association may leave it live for WPA2, and every
//! retry must retain the last hardware-valid frontier. This owner contains no
//! board storage addresses or protocol policy.

use core::{future::Future, marker::PhantomData};

use embassy_time::Timer;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped,
};

use crate::rx_backend::ESP32S31_RX_WALKER_ENABLE_SETTLE_US;

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
}

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};

    use open_esp_radio_esp32s31_wifi_mac::{
        descriptor::DESCRIPTOR_BYTES,
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
    fn owner_round_trips_halted_live_halted_and_can_move_between_phases() {
        let storage = Esp32s31RxDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
        let mut hardware = Hardware::default();
        let mut rx = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(
            halted_ring(&mut hardware, &storage),
        );
        let mut prepared = 0;
        embassy_futures::block_on(rx.start(&mut hardware, |_| {
            prepared += 1;
            Ok(())
        }))
        .unwrap();
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Live);
        assert_eq!(prepared, COUNT);

        let mut moved = rx.take().unwrap();
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Vacant);
        assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Live);
        moved.stop(&mut hardware).unwrap();
        assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Halted);
        rx = moved;
        assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Halted);
    }
}
