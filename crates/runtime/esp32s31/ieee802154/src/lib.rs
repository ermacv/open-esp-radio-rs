#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Executor-independent IEEE 802.15.4 radio runtime for the ESP32-S31.
//!
//! [`Ieee802154Runtime`] holds the radio role — the ported ESP-IDF MAC
//! engine behind the portable radio contract — together with the MAC owners
//! in one blocking mutex over a caller-chosen raw mutex, as the vendor driver
//! holds its state under `ieee802154_enter_critical`. The platform interrupt
//! handler and every command run inside that lock. Portable events leave the
//! lock as owned values through a bounded queue that any executor may await
//! with [`Ieee802154Runtime::next_event`].

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
use oer_esp32s31_hal::ieee802154::{Ieee802154MultipanIndex, ll::Ieee802154LowLevel};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, PENDING_TABLE_SIZE};
use oer_esp32s31_ieee802154::pib::Ieee802154PibDefaults;
use oer_esp32s31_ieee802154_radio::{Ieee802154Radio, Ieee802154RadioSink};
use oer_ieee802154::{
    AcceptedCommand, AutoPendingMode, CommandError, Frame, PendingTable, RadioCommand, RadioEvent,
    RadioFault, RadioState, ReceivedFrame, RequestId, RxMetadata, TxStatus,
};

pub use oer_esp32s31_ieee802154_radio::{
    IEEE802154_RADIO_CAPABILITIES, Ieee802154EnhancedAckGenerator, Ieee802154Platform,
};

/// A received frame copied out of the receive ring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ieee802154OwnedFrame {
    /// MAC bytes without PHR or FCS.
    pub frame: Frame,
    /// Receive metadata.
    pub metadata: RxMetadata,
}

impl Ieee802154OwnedFrame {
    fn copy(received: ReceivedFrame<'_>) -> Self {
        Self {
            frame: Frame::try_from_bytes(received.frame.bytes())
                .expect("a portable frame view fits an owned frame"),
            metadata: received.metadata,
        }
    }

    /// Lend the frame as a portable value.
    pub fn received(&self) -> ReceivedFrame<'_> {
        ReceivedFrame {
            frame: self.frame.view(),
            metadata: self.metadata,
        }
    }
}

/// An owned portable [`RadioEvent`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ieee802154RadioEvent {
    /// A frame was received in receive mode.
    Received(Ieee802154OwnedFrame),
    /// Terminal transmit completion.
    TransmitDone {
        /// The request.
        id: RequestId,
        /// Completion category.
        status: TxStatus,
        /// The received acknowledgement.
        acknowledgement: Option<Ieee802154OwnedFrame>,
    },
    /// Terminal energy scan completion.
    EnergyScanDone {
        /// The request.
        id: RequestId,
        /// Measured channel energy in dBm.
        energy_dbm: i8,
    },
    /// The energy scan was aborted.
    EnergyScanFailed {
        /// The request.
        id: RequestId,
    },
    /// Terminal standalone CCA completion.
    ClearChannelAssessmentDone {
        /// The request.
        id: RequestId,
        /// The channel was idle.
        idle: bool,
    },
    /// The standalone CCA was aborted.
    ClearChannelAssessmentFailed {
        /// The request.
        id: RequestId,
    },
    /// The radio disabled itself after a broken event sequence.
    Fault {
        /// The active request.
        id: Option<RequestId>,
        /// Fault category.
        fault: RadioFault,
    },
}

impl Ieee802154RadioEvent {
    fn copy(event: RadioEvent<'_>) -> Self {
        match event {
            RadioEvent::Received(frame) => Self::Received(Ieee802154OwnedFrame::copy(frame)),
            RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
            } => Self::TransmitDone {
                id,
                status,
                acknowledgement: acknowledgement.map(Ieee802154OwnedFrame::copy),
            },
            RadioEvent::EnergyScanDone { id, energy_dbm } => {
                Self::EnergyScanDone { id, energy_dbm }
            }
            RadioEvent::EnergyScanFailed { id } => Self::EnergyScanFailed { id },
            RadioEvent::ClearChannelAssessmentDone { id, idle } => {
                Self::ClearChannelAssessmentDone { id, idle }
            }
            RadioEvent::ClearChannelAssessmentFailed { id } => {
                Self::ClearChannelAssessmentFailed { id }
            }
            RadioEvent::Fault { id, fault } => Self::Fault { id, fault },
        }
    }

    /// Lend the event as a portable value.
    pub fn portable(&self) -> RadioEvent<'_> {
        match self {
            Self::Received(frame) => RadioEvent::Received(frame.received()),
            Self::TransmitDone {
                id,
                status,
                acknowledgement,
            } => RadioEvent::TransmitDone {
                id: *id,
                status: *status,
                acknowledgement: acknowledgement.as_ref().map(Ieee802154OwnedFrame::received),
            },
            Self::EnergyScanDone { id, energy_dbm } => RadioEvent::EnergyScanDone {
                id: *id,
                energy_dbm: *energy_dbm,
            },
            Self::EnergyScanFailed { id } => RadioEvent::EnergyScanFailed { id: *id },
            Self::ClearChannelAssessmentDone { id, idle } => {
                RadioEvent::ClearChannelAssessmentDone {
                    id: *id,
                    idle: *idle,
                }
            }
            Self::ClearChannelAssessmentFailed { id } => {
                RadioEvent::ClearChannelAssessmentFailed { id: *id }
            }
            Self::Fault { id, fault } => RadioEvent::Fault {
                id: *id,
                fault: *fault,
            },
        }
    }
}

/// The bounded event queue overflowed and dropped the events it could not
/// hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventsLost;

/// Why a runtime operation did not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154RuntimeError {
    /// No radio is installed.
    NotInstalled,
    /// The portable state machine rejected the command.
    Rejected(CommandError),
}

/// The engine and the hardware it drives: the HAL `Ieee802154MacOwners`,
/// or a register model in tests.
pub struct Ieee802154RuntimeParts<'storage, H> {
    /// The MAC engine.
    pub engine: Ieee802154Engine<'storage>,
    /// Register access.
    pub hardware: H,
}

struct Installed<'storage, H> {
    radio: Ieee802154Radio<'storage>,
    hardware: H,
}

/// The sink of one locked entry: events go to the queue.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize> {
    events: &'a Channel<M, Ieee802154RadioEvent, EVENTS>,
    lost: &'a AtomicBool,
}

impl<M: RawMutex, const EVENTS: usize> Ieee802154RadioSink for QueueSink<'_, M, EVENTS> {
    fn event(&mut self, event: RadioEvent<'_>) {
        if self
            .events
            .try_send(Ieee802154RadioEvent::copy(event))
            .is_err()
        {
            self.lost.store(true, Ordering::Release);
        }
    }
}

/// The radio role, its hardware and the event queue.
///
/// `EVENTS` bounds the events waiting for the consumer. Overflow drops the
/// newest event and reports [`Ieee802154EventsLost`] once.
pub struct Ieee802154Runtime<'storage, M: RawMutex, H, const EVENTS: usize> {
    installed: Mutex<M, RefCell<Option<Installed<'storage, H>>>>,
    events: Channel<M, Ieee802154RadioEvent, EVENTS>,
    lost: AtomicBool,
}

impl<'storage, M: RawMutex, H: Ieee802154LowLevel, const EVENTS: usize> Default
    for Ieee802154Runtime<'storage, M, H, EVENTS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'storage, M: RawMutex, H: Ieee802154LowLevel, const EVENTS: usize>
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
    /// active: enable the engine, run its MAC initialization and install the
    /// radio role in the `Disabled` state. The platform CPU route may be
    /// enabled afterwards; the portable `Enable` command opens admission.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a radio is already installed.
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
            parts.engine.mac_init(&mut parts.hardware, defaults);
            *installed = Some(Installed {
                radio: Ieee802154Radio::new(parts.engine, platform),
                hardware: parts.hardware,
            });
            Ok(())
        };
        self.installed.lock(install)
    }

    /// `esp_ieee802154_disable` after the platform CPU route is disabled:
    /// disable and return the engine and hardware, and discard queued
    /// events.
    pub fn uninstall(&self) -> Option<Ieee802154RuntimeParts<'storage, H>> {
        let parts = self.installed.lock(|installed| {
            installed.borrow_mut().take().map(|installed| {
                let mut engine = installed.radio.into_engine();
                engine.disable();
                Ieee802154RuntimeParts {
                    engine,
                    hardware: installed.hardware,
                }
            })
        });
        while self.events.try_receive().is_ok() {}
        self.lost.store(false, Ordering::Release);
        parts
    }

    fn with_radio<T>(
        &self,
        entry: impl FnOnce(&mut Ieee802154Radio<'storage>, &mut H, &mut QueueSink<'_, M, EVENTS>) -> T,
    ) -> Result<T, Ieee802154RuntimeError> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed
                .as_mut()
                .ok_or(Ieee802154RuntimeError::NotInstalled)?;
            let mut sink = QueueSink {
                events: &self.events,
                lost: &self.lost,
            };
            Ok(entry(
                &mut installed.radio,
                &mut installed.hardware,
                &mut sink,
            ))
        })
    }

    /// The platform interrupt handler (`ieee802154_isr`). Does nothing while
    /// no radio is installed.
    pub fn on_interrupt(&self) {
        let _ = self.with_radio(|radio, port, sink| radio.isr(port, sink));
    }

    /// Admit and start one portable command.
    ///
    /// # Errors
    ///
    /// No radio is installed, or the portable state machine rejected the
    /// command.
    pub fn submit(
        &self,
        command: RadioCommand<'_>,
    ) -> Result<AcceptedCommand, Ieee802154RuntimeError> {
        self.with_radio(|radio, port, sink| radio.submit(port, command, sink))?
            .map_err(Ieee802154RuntimeError::Rejected)
    }

    /// The portable state.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn state(&self) -> Result<RadioState, Ieee802154RuntimeError> {
        self.with_radio(|radio, _, _| radio.state())
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

    /// Read or change the frame-pending table.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn with_pending_table<T>(
        &self,
        change: impl FnOnce(&mut PendingTable<PENDING_TABLE_SIZE>) -> T,
    ) -> Result<T, Ieee802154RuntimeError> {
        self.with_radio(|radio, _, _| change(radio.engine().pending_table()))
    }

    /// `esp_ieee802154_set_pending_mode`: how the automatic acknowledgement
    /// decides frame pending. The PIB publishes it before the next operation.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn set_pending_mode(&self, mode: AutoPendingMode) -> Result<(), Ieee802154RuntimeError> {
        self.with_radio(|radio, _, _| {
            radio
                .engine()
                .pib()
                .set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, mode);
        })
    }

    /// `esp_ieee802154_set_transmit_security` for the secured `[PHR, PSDU...]`
    /// image the next transmission sends.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn set_transmit_security(
        &self,
        frame: &[u8],
        key: &[u8; 16],
        address: &[u8; 8],
    ) -> Result<(), Ieee802154RuntimeError> {
        self.with_radio(|radio, port, _| {
            radio
                .engine()
                .set_transmit_security(port, frame, key, address);
        })
    }
}

#[cfg(test)]
mod tests;
