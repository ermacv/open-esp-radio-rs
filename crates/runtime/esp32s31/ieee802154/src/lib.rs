#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Executor-independent IEEE 802.15.4 radio runtime for the ESP32-S31.
//!
//! [`Ieee802154Runtime`] holds the ported ESP-IDF MAC engine together with
//! the MAC owners in one blocking mutex over a caller-chosen raw mutex, as
//! the vendor driver holds its state under `ieee802154_enter_critical`.
//! Every operation — the platform interrupt handler, transmit, receive,
//! energy detection and configuration — runs the engine inside that lock.
//! Engine notifications leave the lock through a bounded event queue that
//! any executor may await with [`Ieee802154Runtime::next_event`].
//!
//! The runtime reproduces the vendor model: an operation start returns at
//! once and its outcome arrives as an event. A received frame stays in its
//! receive-ring slot until [`Ieee802154Runtime::take_frame`] or
//! [`Ieee802154Runtime::release`] returns the slot to the ring.

#[cfg(test)]
extern crate std;

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
};
use oer_esp32s31_hal::ieee802154::{
    ll::{Ieee802154LowLevel, Ieee802154MacOwners, Ieee802154MacPort, Ieee802154RxStatus},
    pib::{Ieee802154Pib, Ieee802154PibDefaults},
};
use oer_esp32s31_ieee802154::engine::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154Environment, Ieee802154FrameImageError,
    Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154SlotError,
    Ieee802154State, Ieee802154TxError, PENDING_TABLE_SIZE,
};
use oer_ieee802154::{FrameAddress, PendingTable};

/// Register access for one engine entry: the hardware MAC owners, or a
/// register model in tests.
pub trait Ieee802154Hardware {
    /// The LL port borrowed for one entry.
    type Port<'a>: Ieee802154LowLevel
    where
        Self: 'a;

    /// Borrow the LL port.
    fn port(&mut self) -> Self::Port<'_>;
}

impl Ieee802154Hardware for Ieee802154MacOwners {
    type Port<'a> = Ieee802154MacPort<'a>;

    fn port(&mut self) -> Self::Port<'_> {
        Ieee802154MacOwners::port(self)
    }
}

/// Builds the enhanced ACK for a received 2015 frame inside the interrupt
/// handler (`esp_ieee802154_enh_ack_generator`); `false` refuses, and the
/// frame is delivered without an ACK.
pub type Ieee802154EnhancedAckGenerator =
    fn(frame: &[u8; FRAME_SIZE], info: &Ieee802154FrameInfo, ack: &mut [u8; FRAME_SIZE]) -> bool;

/// Platform services the engine calls inside the lock.
#[derive(Clone, Copy)]
pub struct Ieee802154Platform {
    /// The monotonic microsecond clock (`esp_timer_get_time`); the engine
    /// truncates it to the vendor's wrapping 32-bit timer domain.
    pub now_micros: fn() -> u64,
    /// The enhanced-ACK generator; `None` refuses every enhanced ACK.
    pub enhanced_ack: Option<Ieee802154EnhancedAckGenerator>,
}

/// One engine notification (`esp_ieee802154_event.c`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154RadioEvent {
    /// A frame arrived in `slot`; take or release it.
    Received {
        /// Receive-ring slot holding the frame.
        slot: Ieee802154RxSlot,
        /// Receive metadata.
        info: Ieee802154FrameInfo,
    },
    /// A frame's SFD was received.
    ReceiveSfd,
    /// The transmission completed; an ACK, if any, is held in its slot.
    Transmitted {
        /// Slot and metadata of the received ACK.
        ack: Option<(Ieee802154RxSlot, Ieee802154FrameInfo)>,
    },
    /// The transmission failed.
    TransmitFailed(Ieee802154TxError),
    /// The transmitted frame's SFD left the radio.
    TransmitSfd,
    /// Energy detection completed with this power in dBm.
    EnergyDetected(i8),
    /// Standalone clear-channel assessment completed.
    ClearChannelAssessed {
        /// The channel was busy.
        busy: bool,
    },
    /// Energy detection or CCA was aborted.
    EnergyDetectionFailed(Ieee802154RxStatus),
    /// A timed receive window closed.
    ReceiveWindowClosed,
}

/// The bounded event queue overflowed; the events it could not hold were
/// dropped, and any receive slot they carried was returned to the ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventsLost;

/// Why a runtime operation did not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154RuntimeError {
    /// No engine is installed.
    NotInstalled,
    /// The transmit frame is not a `[PHR, PSDU...]` image of one DMA frame.
    FrameImage,
    /// The slot is outside the receive ring.
    Slot,
}

impl From<Ieee802154FrameImageError> for Ieee802154RuntimeError {
    fn from(_: Ieee802154FrameImageError) -> Self {
        Self::FrameImage
    }
}

impl From<Ieee802154SlotError> for Ieee802154RuntimeError {
    fn from(_: Ieee802154SlotError) -> Self {
        Self::Slot
    }
}

/// The engine and the hardware it drives.
pub struct Ieee802154RuntimeParts<'storage, H> {
    /// The MAC engine.
    pub engine: Ieee802154Engine<'storage>,
    /// Register access.
    pub hardware: H,
}

struct Installed<'storage, H> {
    parts: Ieee802154RuntimeParts<'storage, H>,
    platform: Ieee802154Platform,
}

/// Receive slots whose events were dropped, released after the engine entry.
const DROPPED_SLOTS: usize = 4;

/// The engine environment of one locked entry: events go to the queue.
struct QueueEnvironment<'a, M: RawMutex, const EVENTS: usize> {
    events: &'a Channel<M, Ieee802154RadioEvent, EVENTS>,
    lost: &'a AtomicBool,
    platform: Ieee802154Platform,
    dropped: [Option<Ieee802154RxSlot>; DROPPED_SLOTS],
}

impl<M: RawMutex, const EVENTS: usize> QueueEnvironment<'_, M, EVENTS> {
    fn post(&mut self, event: Ieee802154RadioEvent) {
        if self.events.try_send(event).is_ok() {
            return;
        }
        self.lost.store(true, Ordering::Release);
        let slot = match event {
            Ieee802154RadioEvent::Received { slot, .. } => Some(slot),
            Ieee802154RadioEvent::Transmitted {
                ack: Some((slot, _)),
            } => Some(slot),
            _ => None,
        };
        if let Some(slot) = slot
            && let Some(free) = self.dropped.iter_mut().find(|entry| entry.is_none())
        {
            *free = Some(slot);
        }
    }
}

impl<M: RawMutex, const EVENTS: usize> Ieee802154Environment for QueueEnvironment<'_, M, EVENTS> {
    fn now_micros(&mut self) -> u64 {
        (self.platform.now_micros)()
    }

    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        _frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        self.post(Ieee802154RadioEvent::Received { slot, info: *info });
    }

    fn receive_sfd_done(&mut self) {
        self.post(Ieee802154RadioEvent::ReceiveSfd);
    }

    fn transmit_done(&mut self, _frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>) {
        self.post(Ieee802154RadioEvent::Transmitted {
            ack: ack.map(|ack| (ack.slot, *ack.info)),
        });
    }

    fn transmit_failed(&mut self, _frame: &[u8; FRAME_SIZE], error: Ieee802154TxError) {
        self.post(Ieee802154RadioEvent::TransmitFailed(error));
    }

    fn transmit_sfd_done(&mut self, _frame: &[u8; FRAME_SIZE]) {
        self.post(Ieee802154RadioEvent::TransmitSfd);
    }

    fn energy_detect_done(&mut self, power: i8) {
        self.post(Ieee802154RadioEvent::EnergyDetected(power));
    }

    fn cca_done(&mut self, busy: bool) {
        self.post(Ieee802154RadioEvent::ClearChannelAssessed { busy });
    }

    fn ed_failed(&mut self, status: Ieee802154RxStatus) {
        self.post(Ieee802154RadioEvent::EnergyDetectionFailed(status));
    }

    fn receive_at_done(&mut self) {
        self.post(Ieee802154RadioEvent::ReceiveWindowClosed);
    }

    fn generate_enhanced_ack(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> bool {
        self.platform
            .enhanced_ack
            .is_some_and(|generate| generate(frame, info, ack))
    }
}

/// The engine, its hardware and the event queue.
///
/// `EVENTS` bounds the notifications waiting for the consumer. Overflow
/// drops the newest event, reports [`Ieee802154EventsLost`] once and returns
/// any receive slot the dropped event carried to the ring.
pub struct Ieee802154Runtime<'storage, M: RawMutex, H, const EVENTS: usize> {
    installed: Mutex<M, RefCell<Option<Installed<'storage, H>>>>,
    events: Channel<M, Ieee802154RadioEvent, EVENTS>,
    lost: AtomicBool,
}

impl<'storage, M: RawMutex, H: Ieee802154Hardware, const EVENTS: usize> Default
    for Ieee802154Runtime<'storage, M, H, EVENTS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'storage, M: RawMutex, H: Ieee802154Hardware, const EVENTS: usize>
    Ieee802154Runtime<'storage, M, H, EVENTS>
{
    /// An empty runtime, suitable for a `static`.
    pub const fn new() -> Self {
        Self {
            installed: Mutex::new(RefCell::new(None)),
            events: Channel::new(),
            lost: AtomicBool::new(false),
        }
    }

    /// `esp_ieee802154_enable` after clocks, PHY and the interrupt owner are
    /// active: enable the engine, run its MAC initialization and install it.
    /// The platform CPU route may be enabled afterwards.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if an engine is already installed.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install(
        &self,
        mut parts: Ieee802154RuntimeParts<'storage, H>,
        platform: Ieee802154Platform,
        defaults: Ieee802154PibDefaults,
    ) -> Result<(), Ieee802154RuntimeParts<'storage, H>> {
        #[allow(
            clippy::result_large_err,
            reason = "the no-alloc runtime returns the unconsumed owners by value"
        )]
        let install = |installed: &RefCell<Option<Installed<'storage, H>>>| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Err(parts);
            }
            parts.engine.enable();
            let mut port = parts.hardware.port();
            parts.engine.mac_init(&mut port, defaults);
            drop(port);
            *installed = Some(Installed { parts, platform });
            Ok(())
        };
        self.installed.lock(install)
    }

    /// `esp_ieee802154_disable` after the platform CPU route is disabled:
    /// disable and return the engine and hardware, and discard queued
    /// events.
    pub fn uninstall(&self) -> Option<Ieee802154RuntimeParts<'storage, H>> {
        let parts = self.installed.lock(|installed| {
            installed.borrow_mut().take().map(|mut installed| {
                installed.parts.engine.disable();
                installed.parts
            })
        });
        while self.events.try_receive().is_ok() {}
        self.lost.store(false, Ordering::Release);
        parts
    }

    /// Run `entry` on the installed engine with the LL port and a queue
    /// environment, then release the slots of dropped events.
    fn with_engine<T>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Engine<'storage>,
            &mut H::Port<'_>,
            &mut QueueEnvironment<'_, M, EVENTS>,
        ) -> T,
    ) -> Result<T, Ieee802154RuntimeError> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed
                .as_mut()
                .ok_or(Ieee802154RuntimeError::NotInstalled)?;
            let mut environment = QueueEnvironment {
                events: &self.events,
                lost: &self.lost,
                platform: installed.platform,
                dropped: [None; DROPPED_SLOTS],
            };
            let engine = &mut installed.parts.engine;
            let mut port = installed.parts.hardware.port();
            let result = entry(engine, &mut port, &mut environment);
            drop(port);
            for slot in environment.dropped.into_iter().flatten() {
                let _ = engine.receive_handle_done(slot);
            }
            Ok(result)
        })
    }

    /// The platform interrupt handler (`ieee802154_isr`). Does nothing while
    /// no engine is installed.
    pub fn on_interrupt(&self) {
        let _ = self.with_engine(|engine, port, environment| engine.isr(port, environment));
    }

    /// Wait for the next event.
    ///
    /// # Errors
    ///
    /// Reports once that the queue overflowed before the events after it.
    pub async fn next_event(&self) -> Result<Ieee802154RadioEvent, Ieee802154EventsLost> {
        if self.lost.swap(false, Ordering::AcqRel) {
            return Err(Ieee802154EventsLost);
        }
        Ok(self.events.receive().await)
    }

    /// `esp_ieee802154_transmit` of a `[PHR, PSDU...]` image.
    ///
    /// # Errors
    ///
    /// No engine is installed, or the image does not fit one DMA frame.
    pub fn transmit(&self, frame: &[u8], cca: bool) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| {
            engine.transmit(port, environment, frame, cca)
        })?
        .map_err(Into::into)
    }

    /// `esp_ieee802154_transmit_at`: transmit when the timer reaches `time`.
    ///
    /// # Errors
    ///
    /// As for [`Self::transmit`].
    pub fn transmit_at(
        &self,
        frame: &[u8],
        cca: bool,
        time: u32,
    ) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| {
            engine.transmit_at(port, environment, frame, cca, time)
        })?
        .map_err(Into::into)
    }

    /// `esp_ieee802154_receive`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn receive(&self) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| engine.receive(port, environment))
    }

    /// `esp_ieee802154_receive_at`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn receive_at(&self, time: u32, duration: u32) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| {
            engine.receive_at(port, environment, time, duration);
        })
    }

    /// `esp_ieee802154_sleep`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn sleep(&self) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| engine.sleep(port, environment))
    }

    /// `esp_ieee802154_energy_detect` for `duration` 16-microsecond symbols.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn energy_detect(&self, duration: u16) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| {
            engine.energy_detect(port, environment, duration);
        })
    }

    /// `esp_ieee802154_cca`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn cca(&self) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, environment| engine.cca(port, environment))
    }

    /// `esp_ieee802154_get_state`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn state(&self) -> Result<Ieee802154State, Ieee802154RuntimeError> {
        self.with_engine(|engine, _, _| engine.state())
    }

    /// Read or change the PAN information base; changes apply before the
    /// next operation.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn with_pib<T>(
        &self,
        change: impl FnOnce(&mut Ieee802154Pib) -> T,
    ) -> Result<T, Ieee802154RuntimeError> {
        self.with_engine(|engine, _, _| change(engine.pib()))
    }

    /// Read or change the frame-pending table.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn with_pending_table<T>(
        &self,
        change: impl FnOnce(&mut PendingTable<PENDING_TABLE_SIZE>) -> T,
    ) -> Result<T, Ieee802154RuntimeError> {
        self.with_engine(|engine, _, _| change(engine.pending_table()))
    }

    /// `esp_ieee802154_add_pending_addr`; `Ok(false)` when the table is full.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn add_pending_address(
        &self,
        address: FrameAddress,
    ) -> Result<bool, Ieee802154RuntimeError> {
        self.with_pending_table(|table| table.add(address).is_ok())
    }

    /// `esp_ieee802154_set_panid`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn set_panid(&self, panid: u16) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, _| engine.set_panid(port, panid))
    }

    /// `esp_ieee802154_set_short_address`.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn set_short_address(&self, address: u16) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, _| engine.set_short_address(port, address))
    }

    /// `esp_ieee802154_set_extended_address` in frame byte order.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn set_extended_address(&self, address: [u8; 8]) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, _| engine.set_extended_address(port, address))
    }

    /// `esp_ieee802154_set_ack_timeout` in microseconds.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn set_ack_timeout(&self, microseconds: u32) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, _| engine.set_ack_timeout(port, microseconds))
    }

    /// `esp_ieee802154_set_transmit_security` for the secured frame the next
    /// transmission sends.
    ///
    /// # Errors
    ///
    /// No engine is installed.
    pub fn set_transmit_security(
        &self,
        frame: &[u8],
        key: &[u8; 16],
        address: &[u8; 8],
    ) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, port, _| {
            engine.set_transmit_security(port, frame, key, address);
        })
    }

    /// Copy the frame out of `slot` and return the slot to the ring.
    ///
    /// # Errors
    ///
    /// No engine is installed, or the slot is outside the ring.
    pub fn take_frame(
        &self,
        slot: Ieee802154RxSlot,
    ) -> Result<([u8; FRAME_SIZE], Ieee802154FrameInfo), Ieee802154RuntimeError> {
        self.with_engine(|engine, _, _| {
            let frame = engine.rx_frame(slot);
            engine.receive_handle_done(slot).map(|()| frame)
        })?
        .map_err(Into::into)
    }

    /// `esp_ieee802154_receive_handle_done`: return `slot` to the ring.
    ///
    /// # Errors
    ///
    /// No engine is installed, or the slot is outside the ring.
    pub fn release(&self, slot: Ieee802154RxSlot) -> Result<(), Ieee802154RuntimeError> {
        self.with_engine(|engine, _, _| engine.receive_handle_done(slot))?
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
