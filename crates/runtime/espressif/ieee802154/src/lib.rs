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
//! lock as owned values through a bounded queue that any executor may await.
//! The runtime is an [`Ieee802154RadioPort`]: commands, events, the clock,
//! the state and the settings the interrupt handler reads (MAC keys, CSL,
//! the enhanced-ACK generator, armed transmit security) go through the
//! port; installation, pause and resume, coexistence, statistics and the
//! frame-pending table stay inherent.
//!
//! CSMA-CA backoffs and the delays before retries run on the runtime's
//! [`oer_time::Timer`] in [`Ieee802154Runtime::run`], the runner the composition polls for as long
//! as the radio is installed, as ESP-IDF's OpenThread `SubMac` runs them on
//! its tasklet timer; [`RadioPort::next_event`] only takes events.
//!
//! The bounded event queue reserves a slot for every terminal event it
//! owes when the work is admitted: the admitted operation's end, the fault
//! that may end an enabled period, and a lifecycle command's terminal. A
//! terminal event is therefore never lost; an operation or a lifecycle
//! command whose terminal could not be held is refused. Only received
//! frames are dropped when the queue is full, reported as [`EventsLost`] in
//! place of the first dropped frame.
//!
//! The runtime never poisons (its fault type is [`Infallible`]): a broken
//! event sequence ends in a recoverable [`RadioEvent::Fault`] that leaves
//! the radio disabled. A radio that is not installed, also while paused,
//! refuses every call as not installed.

#[cfg(test)]
extern crate std;

mod trace;

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
};

use core::future::{Future, ready};
use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_espressif_ieee802154_engine::engine::{Ieee802154Engine, PENDING_TABLE_SIZE};
pub use oer_espressif_ieee802154_engine::engine::{
    Ieee802154RxAbortStatistics, Ieee802154RxStatistics, Ieee802154TxAbortStatistics,
    Ieee802154TxRxStatistics, Ieee802154TxStatistics,
};
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    ll::{Ieee802154LowLevel, Ieee802154RecentRssi},
    types::Ieee802154MultipanIndex,
};
use oer_espressif_ieee802154_radio::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154Radio, Ieee802154RadioSink,
};

pub use oer_ieee802154::NotInstalled;
use oer_ieee802154::{
    AcceptedCommand, AppliedSecurity, AutoPendingMode, CancelError, ClockError, ClockInfo,
    CommandError, EventsLost, Frame, FrameCounterUpdate, Ieee802154Capabilities, Ieee802154Instant,
    Ieee802154RadioPort, Interface, LifecycleCommand, LifecycleError, LifecycleEvent, MacKeys,
    PendingTable, PortResult, RadioCommand, RadioEvent, RadioFault, RadioPort, RadioSetting,
    RadioState, ReceivedFrame, RequestId, RestingState, RxMetadata, SettingError, TxStatus,
};
use oer_ieee802154_trace::{Lease, PauseRefusal};
use oer_time::{Duration, Instant, Timer};

pub use oer_espressif_ieee802154_radio::{
    IEEE802154_ENH_ACK_PROBING_CAPACITY, IEEE802154_ENHANCED_ACK_IE_CAPACITY,
    IEEE802154_RADIO_CAPABILITIES, Ieee802154Random,
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
        /// The security header fields the radio wrote into the frame.
        security: Option<AppliedSecurity>,
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
    /// A scheduled receive window ended; the radio sleeps.
    ScheduledReceiveDone {
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
    /// The terminal event of a port lifecycle command.
    Lifecycle(LifecycleEvent),
}

impl Ieee802154RadioEvent {
    fn copy(event: RadioEvent<'_>) -> Self {
        match event {
            RadioEvent::Received(frame) => Self::Received(Ieee802154OwnedFrame::copy(frame)),
            RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
                security,
            } => Self::TransmitDone {
                id,
                status,
                acknowledgement: acknowledgement.map(Ieee802154OwnedFrame::copy),
                security,
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
            RadioEvent::ScheduledReceiveDone { id } => Self::ScheduledReceiveDone { id },
            RadioEvent::Fault { id, fault } => Self::Fault { id, fault },
            RadioEvent::Lifecycle(event) => Self::Lifecycle(event),
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
                security,
            } => RadioEvent::TransmitDone {
                id: *id,
                status: *status,
                acknowledgement: acknowledgement.as_ref().map(Ieee802154OwnedFrame::received),
                security: *security,
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
            Self::ScheduledReceiveDone { id } => RadioEvent::ScheduledReceiveDone { id: *id },
            Self::Fault { id, fault } => RadioEvent::Fault {
                id: *id,
                fault: *fault,
            },
            Self::Lifecycle(event) => RadioEvent::Lifecycle(*event),
        }
    }
}

/// The engine and the hardware it drives: the HAL `Ieee802154MacOwners`,
/// or a register model in tests.
pub struct Ieee802154RuntimeParts<'storage, H> {
    /// The MAC engine.
    pub engine: Ieee802154Engine<'storage>,
    /// Register access.
    pub hardware: H,
}

struct Installed<'storage, H, R> {
    radio: Ieee802154Radio<'storage, R>,
    hardware: H,
}

/// Why the runtime could not pause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PauseError {
    /// No radio is installed.
    NotInstalled,
    /// A transmission, energy scan or CCA is still running.
    Busy,
}

/// The radio role and its hardware held outside the runtime while the MAC is
/// stopped, for example across shared PHY maintenance.
///
/// While paused the runtime rejects commands as not installed and its
/// interrupt entry does nothing, so the platform CPU route must stay closed
/// until [`Ieee802154Runtime::resume`].
#[must_use = "a paused radio must be resumed"]
pub struct Ieee802154RuntimePaused<'storage, H, R> {
    installed: Installed<'storage, H, R>,
    receiving: Option<oer_ieee802154::Channel>,
}

impl<H, R> Ieee802154RuntimePaused<'_, H, R> {
    /// Borrow the hardware, for example to prove the MAC quiescent.
    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.installed.hardware
    }

    /// The channel receive mode resumes on, if the radio was receiving.
    pub const fn receiving(&self) -> Option<oer_ieee802154::Channel> {
        self.receiving
    }
}

/// Correlation identifier of the runtime's own stop and resume of receive
/// mode, in the backend-reserved range; neither produces an event.
const PAUSE_REQUEST: RequestId = RequestId::new(0xFFFF_FF00);

/// What the event queue owes the consumer: the slots it reserved for
/// terminal events, a loss not reported yet and the progress of a quiesce.
#[derive(Default)]
struct Owed {
    /// A received frame was dropped: the next event reports the loss first.
    lost: bool,
    /// The admitted operation's terminal event.
    operation: bool,
    /// The fault that may end the enabled period; `Disable` takes its slot.
    fault: bool,
    /// A quiesce waits for the operation's terminal event to report
    /// `Quiesced`.
    quiescing: bool,
    /// Admission of operations is closed until `Enable`.
    quiesced: bool,
    /// An event taken after the loss it reported.
    held: Option<Ieee802154RadioEvent>,
}

impl Owed {
    fn reserved(&self) -> usize {
        usize::from(self.operation) + usize::from(self.fault) + usize::from(self.quiescing)
    }
}

/// One queued event, with whether a loss precedes it.
struct Entry {
    lost_before: bool,
    event: Ieee802154RadioEvent,
}

/// The bounded event queue: terminal events hold the slots reserved for
/// them, and the loss of received frames is reported in its order, in
/// place of the first dropped frame.
struct EventQueue<M: RawMutex, const EVENTS: usize> {
    entries: Channel<M, Entry, EVENTS>,
    owed: Mutex<M, RefCell<Owed>>,
    /// Raised when a loss is pending without a queued entry.
    changed: Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize> EventQueue<M, EVENTS> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            owed: Mutex::new(RefCell::new(Owed {
                lost: false,
                operation: false,
                fault: false,
                quiescing: false,
                quiesced: false,
                held: None,
            })),
            changed: Signal::new(),
        }
    }

    /// Run `entry` on what the queue owes.
    fn owed<O>(&self, entry: impl FnOnce(&mut Owed) -> O) -> O {
        self.owed.lock(|owed| entry(&mut owed.borrow_mut()))
    }

    /// Whether `slots` more terminal events fit beside the queued and
    /// reserved ones.
    fn has_room(&self, owed: &Owed, slots: usize) -> bool {
        self.entries.len() + owed.reserved() + slots <= EVENTS
    }

    /// Queue `event`: a terminal event in the slot reserved for it, a
    /// received frame when a slot is free; `false` when the frame was
    /// dropped.
    fn push(&self, event: Ieee802154RadioEvent) -> bool {
        self.owed(|owed| {
            let ends_operation = match &event {
                Ieee802154RadioEvent::Received(_) => {
                    if !self.has_room(owed, 1) {
                        owed.lost = true;
                        return false;
                    }
                    false
                }
                // The command checked its room.
                Ieee802154RadioEvent::Lifecycle(_) => false,
                // The first fault of an enabled period holds its reserved
                // slot, and a fault of the admitted operation the operation's.
                // A further fault, as a disabled radio may still report, is
                // not promised: it takes a free slot or is lost.
                Ieee802154RadioEvent::Fault { .. } => {
                    if owed.fault {
                        owed.fault = false;
                    } else if !owed.operation && !self.has_room(owed, 1) {
                        owed.lost = true;
                        return false;
                    }
                    true
                }
                _ => true,
            };
            if ends_operation {
                owed.operation = false;
            }
            self.send(owed, event);
            // A quiesce ends after the operation's terminal event.
            if ends_operation && owed.quiescing {
                owed.quiescing = false;
                owed.quiesced = true;
                self.send(
                    owed,
                    Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Quiesced),
                );
            }
            true
        })
    }

    /// Queue `event` in a slot known to be free.
    fn send(&self, owed: &mut Owed, event: Ieee802154RadioEvent) {
        let lost_before = core::mem::take(&mut owed.lost);
        let sent = self.entries.try_send(Entry { lost_before, event });
        assert!(sent.is_ok(), "a terminal event holds a reserved slot");
    }

    fn take(&self) -> Option<Result<Ieee802154RadioEvent, EventsLost>> {
        self.owed(|owed| {
            if let Some(event) = owed.held.take() {
                return Some(Ok(event));
            }
            match self.entries.try_receive() {
                Ok(Entry {
                    lost_before: true,
                    event,
                }) => {
                    owed.held = Some(event);
                    Some(Err(EventsLost))
                }
                Ok(Entry { event, .. }) => Some(Ok(event)),
                Err(_) if core::mem::take(&mut owed.lost) => Some(Err(EventsLost)),
                Err(_) => None,
            }
        })
    }
}

/// The sink of one locked entry: events go to the queue.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize> {
    events: &'a EventQueue<M, EVENTS>,
}

impl<M: RawMutex, const EVENTS: usize> Ieee802154RadioSink for QueueSink<'_, M, EVENTS> {
    fn event(&mut self, event: RadioEvent<'_>) {
        if !self.events.push(Ieee802154RadioEvent::copy(event))
            && let RadioEvent::Received(frame) = event
        {
            trace::emit(|| oer_ieee802154_trace::RxOutcome {
                // The PHR length counts the two FCS bytes the portable
                // frame leaves out.
                length: frame.frame.bytes().len() as u8 + 2,
                result: oer_ieee802154_trace::RxResult::Dropped(
                    oer_ieee802154_trace::RxDrop::QueueFull,
                ),
            });
        }
    }
}

/// The radio role, its hardware and the event queue.
///
/// `EVENTS` bounds the events waiting for the consumer, the reserved
/// terminal slots included; it is at least three (an operation's end, a
/// fault and a lifecycle terminal). A full queue drops the newest received
/// frame and reports [`EventsLost`] once in its place. `T` is the image's
/// monotonic time the delays wait on.
// CAPABILITY: ieee802154-mac-operation-subset
pub struct Ieee802154Runtime<'storage, M: RawMutex, H, T, R, const EVENTS: usize> {
    timer: T,
    installed: Mutex<M, RefCell<Option<Installed<'storage, H, R>>>>,
    events: EventQueue<M, EVENTS>,
    /// When the running backoff or retry delay ends.
    backoff_until: Mutex<M, Cell<Option<Instant>>>,
    /// Raised when a delay starts, so an awaiting consumer rearms.
    backoff_started: Signal<M, ()>,
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer + Default,
    R: Ieee802154Random,
    const EVENTS: usize,
> Default for Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
    const EVENTS: usize,
> Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    /// An empty runtime whose delays wait on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        assert!(
            EVENTS >= 3,
            "the queue holds an operation's end, a fault and a lifecycle terminal"
        );
        Self {
            timer,
            installed: Mutex::new(RefCell::new(None)),
            events: EventQueue::new(),
            backoff_until: Mutex::new(Cell::new(None)),
            backoff_started: Signal::new(),
        }
    }

    /// The image's monotonic clock the runtime reads: its radio clock
    /// ([`ClockInfo::MONOTONIC_MICROS`]), for a synchronous reader such as
    /// OpenThread's `otPlatRadioGetNow`.
    pub const fn clock(&self) -> &T {
        &self.timer
    }

    /// Start the timer of a transmission's backoff or retry delay, if one is
    /// due.
    fn start_backoff(&self, backoff_micros: Option<u32>) {
        if let Some(micros) = backoff_micros {
            // A delay past the timer's range never ends.
            let until = self
                .timer
                .deadline_after(Duration::from_micros(u64::from(micros)))
                .unwrap_or(Instant::from_micros(u64::MAX));
            self.backoff_until.lock(|backoff| backoff.set(Some(until)));
            self.backoff_started.signal(());
        }
    }

    /// `esp_ieee802154_enable` after clocks, PHY and the interrupt owner are
    /// active: enable the engine, run its MAC initialization and install the
    /// radio role in the `Disabled` state. The platform CPU route may be
    /// enabled afterwards; the port's `Enable` lifecycle command opens
    /// admission.
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
        random: R,
        defaults: Ieee802154PibDefaults,
    ) -> Result<(), Ieee802154RuntimeParts<'storage, H>> {
        #[allow(
            clippy::result_large_err,
            reason = "the no-alloc runtime returns the unconsumed owners by value"
        )]
        let install = |installed: &RefCell<Option<Installed<'storage, H, R>>>| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Err(parts);
            }
            parts.engine.enable();
            parts.engine.mac_init(&mut parts.hardware, defaults);
            *installed = Some(Installed {
                radio: Ieee802154Radio::new(parts.engine, random),
                hardware: parts.hardware,
            });
            Ok(())
        };
        self.installed.lock(install)
    }

    /// `esp_ieee802154_disable` after the platform CPU route is disabled:
    /// end the operation in flight with its terminal event, return the
    /// MAC's coexistence PTIs to the disabled foundation image, and disable
    /// and return the engine and hardware. Queued events stay with the
    /// consumer.
    pub fn uninstall(&self) -> Option<Ieee802154RuntimeParts<'storage, H>> {
        let parts = self.installed.lock(|installed| {
            installed.borrow_mut().take().map(|installed| {
                let Installed {
                    mut radio,
                    mut hardware,
                    ..
                } = installed;
                let mut sink = QueueSink {
                    events: &self.events,
                };
                // A disabled radio has nothing in flight.
                let _ = radio.disable(&mut hardware, &self.timer, &mut sink);
                let mut engine = radio.into_engine();
                engine.mac_deinit(&mut hardware);
                engine.disable();
                Ieee802154RuntimeParts { engine, hardware }
            })
        });
        self.events.owed(|owed| {
            owed.fault = false;
            owed.quiesced = false;
        });
        self.backoff_until.lock(|backoff| backoff.set(None));
        parts
    }

    /// Stop the MAC and take the radio out of the runtime.
    ///
    /// A resting radio pauses: receive mode is left, and resumed by
    /// [`Self::resume`]. The caller then closes the platform CPU route.
    ///
    /// # Errors
    ///
    /// No radio is installed, or an operation with a pending terminal event
    /// is running; nothing changes.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime moves the paused owners by value"
    )]
    pub fn pause(&self) -> Result<Ieee802154RuntimePaused<'storage, H, R>, Ieee802154PauseError> {
        let result = self.pause_locked();
        trace::emit(|| match &result {
            Ok(paused) => Lease::Paused {
                receiving: paused.receiving.map(oer_ieee802154::Channel::get),
            },
            Err(Ieee802154PauseError::NotInstalled) => {
                Lease::PauseRefused(PauseRefusal::NotInstalled)
            }
            Err(Ieee802154PauseError::Busy) => Lease::PauseRefused(PauseRefusal::Busy),
        });
        result
    }

    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime moves the paused owners by value"
    )]
    fn pause_locked(
        &self,
    ) -> Result<Ieee802154RuntimePaused<'storage, H, R>, Ieee802154PauseError> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let receiving = match installed
                .as_ref()
                .ok_or(Ieee802154PauseError::NotInstalled)?
                .radio
                .state()
            {
                RadioState::Resting(RestingState::Receiving { channel }) => Some(channel),
                RadioState::Resting(RestingState::Sleeping) | RadioState::Disabled => None,
                _ => return Err(Ieee802154PauseError::Busy),
            };
            let mut paused = installed.take().expect("checked above");
            if receiving.is_some() {
                let mut sink = QueueSink {
                    events: &self.events,
                };
                let Installed {
                    radio, hardware, ..
                } = &mut paused;
                if radio
                    .submit(
                        hardware,
                        &self.timer,
                        RadioCommand::Sleep { id: PAUSE_REQUEST },
                        &mut sink,
                    )
                    .is_err()
                {
                    *installed = Some(paused);
                    return Err(Ieee802154PauseError::Busy);
                }
            }
            Ok(Ieee802154RuntimePaused {
                installed: paused,
                receiving,
            })
        })
    }

    /// Put a paused radio back, entering receive mode again if it was
    /// receiving. The caller opens the platform CPU route afterwards.
    ///
    /// # Errors
    ///
    /// Another radio is installed; the paused radio is returned.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the paused owners by value"
    )]
    pub fn resume(
        &self,
        paused: Ieee802154RuntimePaused<'storage, H, R>,
    ) -> Result<(), Ieee802154RuntimePaused<'storage, H, R>> {
        let receiving = paused.receiving.map(oer_ieee802154::Channel::get);
        let result = self.resume_locked(paused);
        trace::emit(|| match &result {
            Ok(()) => Lease::Resumed { receiving },
            Err(_) => Lease::ResumeRefused,
        });
        result
    }

    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the paused owners by value"
    )]
    fn resume_locked(
        &self,
        paused: Ieee802154RuntimePaused<'storage, H, R>,
    ) -> Result<(), Ieee802154RuntimePaused<'storage, H, R>> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Err(paused);
            }
            let Ieee802154RuntimePaused {
                installed: mut resumed,
                receiving,
            } = paused;
            if let Some(channel) = receiving {
                let mut sink = QueueSink {
                    events: &self.events,
                };
                let Installed {
                    radio, hardware, ..
                } = &mut resumed;
                // The paused state is the sleeping state it left, so the
                // receive admission cannot be rejected.
                let _ = radio.submit(
                    hardware,
                    &self.timer,
                    RadioCommand::Receive {
                        id: PAUSE_REQUEST,
                        channel,
                    },
                    &mut sink,
                );
            }
            *installed = Some(resumed);
            Ok(())
        })
    }

    fn with_radio<O>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Radio<'storage, R>,
            &mut H,
            &mut QueueSink<'_, M, EVENTS>,
        ) -> O,
    ) -> Result<O, NotInstalled> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().ok_or(NotInstalled)?;
            let mut sink = QueueSink {
                events: &self.events,
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
        if let Ok(backoff) = self.with_radio(|radio, port, sink| {
            radio.isr(port, &self.timer, sink);
            radio.take_delay()
        }) {
            self.start_backoff(backoff);
        }
    }

    /// Collect the vendor's TX/RX statistics
    /// (`CONFIG_IEEE802154_TXRX_STATISTIC`), from zero, or stop collecting
    /// them.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn set_txrx_statistics(&self, collect: bool) -> Result<(), NotInstalled> {
        self.with_radio(|radio, _, _| radio.engine().set_txrx_statistics(collect))
    }

    /// The TX/RX statistics while collected
    /// (`esp_ieee802154_txrx_statistic_print` reads them).
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn txrx_statistics(&self) -> Result<Option<Ieee802154TxRxStatistics>, NotInstalled> {
        self.with_radio(|radio, _, _| radio.engine().txrx_statistics())
    }

    /// `esp_ieee802154_txrx_statistic_clear`.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn clear_txrx_statistics(&self) -> Result<(), NotInstalled> {
        self.with_radio(|radio, _, _| radio.engine().clear_txrx_statistics())
    }

    /// Admit and start one portable command. An operation reserves its
    /// terminal event's slot first, and is refused while admission is
    /// quiesced or no slot is free.
    fn submit_locked(&self, command: RadioCommand<'_>) -> Result<AcceptedCommand, CommandError> {
        let operation = matches!(
            command,
            RadioCommand::Transmit(_)
                | RadioCommand::EnergyScan(_)
                | RadioCommand::ClearChannelAssessment { .. }
                | RadioCommand::ScheduledReceive(_)
        );
        let submitted = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().ok_or(CommandError::NotInstalled)?;
            // A second operation is the state machine's to refuse.
            let reserved = operation
                && self.events.owed(|owed| {
                    if owed.quiescing || owed.quiesced {
                        return Err(CommandError::Quiesced);
                    }
                    if owed.operation {
                        return Ok(false);
                    }
                    if !self.events.has_room(owed, 1) {
                        return Err(CommandError::EventQueueFull);
                    }
                    owed.operation = true;
                    Ok(true)
                })?;
            let mut sink = QueueSink {
                events: &self.events,
            };
            let accepted =
                installed
                    .radio
                    .submit(&mut installed.hardware, &self.timer, command, &mut sink);
            if accepted.is_err() && reserved {
                self.events.owed(|owed| owed.operation = false);
            }
            Ok((accepted, installed.radio.take_delay()))
        });
        let (accepted, backoff) = submitted?;
        self.start_backoff(backoff);
        accepted
    }

    /// End the running operation `target` through its terminal event.
    fn cancel_locked(&self, target: RequestId) -> Result<(), CancelError> {
        let cancelled = self.with_radio(|radio, hardware, sink| {
            let cancelled = radio.cancel(hardware, &self.timer, target, sink);
            (cancelled, radio.take_delay())
        });
        let Ok((cancelled, backoff)) = cancelled else {
            return Err(CancelError::NotInstalled);
        };
        self.start_backoff(backoff);
        cancelled
    }

    /// Start one lifecycle command; its terminal event is queued in a slot
    /// it found free or reserved.
    ///
    /// - `Enable` enables a disabled radio, reserving the slot of the fault
    ///   that may end its enabled period, or reopens a quiesced one.
    /// - `Disable` ends the operation in flight with its terminal event,
    ///   then disables the radio; its terminal takes the fault's slot.
    /// - `Quiesce` closes admission of operations and ends with `Quiesced`
    ///   after the terminal event of the operation in flight.
    fn lifecycle_locked(&self, command: LifecycleCommand) -> Result<(), LifecycleError> {
        let started = self.with_radio(|radio, hardware, sink| {
            let enabled = radio.state() != RadioState::Disabled;
            let (quiescing, quiesced) = self.events.owed(|owed| (owed.quiescing, owed.quiesced));
            if quiescing {
                return Err(LifecycleError::Busy);
            }
            match command {
                LifecycleCommand::Enable if enabled && !quiesced => {
                    Err(LifecycleError::AlreadyInState)
                }
                LifecycleCommand::Enable if enabled => {
                    if !self.events.owed(|owed| self.events.has_room(owed, 1)) {
                        return Err(LifecycleError::Busy);
                    }
                    self.events.owed(|owed| owed.quiesced = false);
                    sink.event(RadioEvent::Lifecycle(LifecycleEvent::Enabled));
                    Ok(())
                }
                LifecycleCommand::Enable => {
                    // The terminal and the fault slot of the enabled period.
                    if !self.events.owed(|owed| self.events.has_room(owed, 2)) {
                        return Err(LifecycleError::Busy);
                    }
                    radio
                        .enable(hardware, &self.timer, sink)
                        .map_err(|_| LifecycleError::InvalidState)?;
                    self.events.owed(|owed| owed.quiesced = false);
                    sink.event(RadioEvent::Lifecycle(LifecycleEvent::Enabled));
                    self.events.owed(|owed| owed.fault = true);
                    Ok(())
                }
                LifecycleCommand::Disable if !enabled => Err(LifecycleError::AlreadyInState),
                LifecycleCommand::Disable => {
                    radio
                        .disable(hardware, &self.timer, sink)
                        .map_err(|_| LifecycleError::InvalidState)?;
                    self.events.owed(|owed| {
                        owed.fault = false;
                        owed.quiesced = false;
                    });
                    sink.event(RadioEvent::Lifecycle(LifecycleEvent::Disabled));
                    Ok(())
                }
                LifecycleCommand::Quiesce if !enabled => Err(LifecycleError::InvalidState),
                LifecycleCommand::Quiesce if quiesced => Err(LifecycleError::AlreadyInState),
                LifecycleCommand::Quiesce => {
                    let running = self.events.owed(|owed| {
                        if !self.events.has_room(owed, 1) {
                            return Err(LifecycleError::Busy);
                        }
                        if owed.operation {
                            owed.quiescing = true;
                        } else {
                            owed.quiesced = true;
                        }
                        Ok(owed.operation)
                    })?;
                    if !running {
                        sink.event(RadioEvent::Lifecycle(LifecycleEvent::Quiesced));
                    }
                    Ok(())
                }
            }
        });
        started.unwrap_or(Err(LifecycleError::NotInstalled))
    }

    /// The radio's runner: a transmission's CSMA-CA backoff or retry delay,
    /// after which the transmission goes on.
    ///
    /// Poll it for as long as the runtime is used, beside the consumer of
    /// [`RadioPort::next_event`]; it never ends. Dropping it keeps
    /// the delay for the next runner.
    pub async fn run(&self) -> Infallible {
        loop {
            let until = self.backoff_until.lock(Cell::get);
            let backoff = async {
                match until {
                    Some(until) => self.timer.wait_until(until).await,
                    None => core::future::pending().await,
                }
            };
            match select(backoff, self.backoff_started.wait()).await {
                Either::First(()) => {
                    self.backoff_until.lock(|backoff| backoff.set(None));
                    if let Ok(backoff) = self.with_radio(|radio, port, sink| {
                        radio.delay_elapsed(port, &self.timer, sink);
                        radio.take_delay()
                    }) {
                        self.start_backoff(backoff);
                    }
                }
                // A backoff started; wait for its end instead.
                Either::Second(()) => {}
            }
        }
    }

    /// Take the next event; nothing else progresses here.
    async fn wait_event(&self) -> PortResult<Ieee802154RadioEvent, EventsLost, Infallible> {
        loop {
            if let Some(event) = self.events.take() {
                return Ok(event);
            }
            select(
                self.events.entries.ready_to_receive(),
                self.events.changed.wait(),
            )
            .await;
        }
    }

    /// Read or change the frame-pending table.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn with_pending_table<O>(
        &self,
        change: impl FnOnce(&mut PendingTable<PENDING_TABLE_SIZE>) -> O,
    ) -> Result<O, NotInstalled> {
        self.with_radio(|radio, _, _| change(radio.engine().pending_table()))
    }

    /// `esp_ieee802154_set_pending_mode`: how the automatic acknowledgement
    /// decides frame pending. The PIB publishes it before the next operation.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn set_pending_mode(&self, mode: AutoPendingMode) -> Result<(), NotInstalled> {
        self.with_radio(|radio, _, _| {
            radio
                .engine()
                .pib()
                .set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, mode);
        })
    }

    /// Replace the engine's software-coexistence priorities
    /// ([`Ieee802154Engine::set_coexistence`]).
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn set_coexistence(&self, coexistence: Ieee802154Coexistence) -> Result<(), NotInstalled> {
        self.with_radio(|radio, _, _| radio.engine().set_coexistence(coexistence))
    }
}

/// Apply one portable setting to the radio under the runtime's lock.
fn apply_setting<L: Ieee802154LowLevel + ?Sized, R: Ieee802154Random>(
    radio: &mut Ieee802154Radio<'_, R>,
    hardware: &mut L,
    setting: RadioSetting<'_>,
) -> Result<(), SettingError> {
    fn keys<'r, R: Ieee802154Random>(
        radio: &'r mut Ieee802154Radio<'_, R>,
        interface: Interface,
    ) -> Result<&'r mut Option<MacKeys>, SettingError> {
        let interfaces = radio.interfaces();
        radio
            .interface_mac_keys(interface)
            .ok_or(SettingError::UnknownInterface {
                interface,
                interfaces,
            })
    }
    match setting {
        RadioSetting::MacKeys {
            interface,
            key_id,
            previous,
            current,
            next,
        } => keys(radio, interface)?
            .get_or_insert(MacKeys::ZEROED)
            .set_keys(key_id, previous, current, next),
        RadioSetting::FrameCounter { interface, update } => {
            let keys = keys(radio, interface)?.get_or_insert(MacKeys::ZEROED);
            match update {
                FrameCounterUpdate::Set(counter) => keys.set_frame_counter(counter),
                FrameCounterUpdate::SetIfLarger(counter) => {
                    keys.set_frame_counter_if_larger(counter);
                }
            }
        }
        RadioSetting::RemoveMacKeys { interface } => *keys(radio, interface)? = None,
        RadioSetting::Csl(csl) => {
            *radio.csl() = Ieee802154Csl {
                period: csl.period,
                sample_time: csl.sample_time,
            };
        }
        RadioSetting::EnhancedAck(generation) => {
            *radio.enhanced_ack() = generation.map(|generation| {
                let mut generator = Ieee802154EnhancedAckGenerator::new();
                generator
                    .probing()
                    .set_noise_floor(generation.noise_floor_dbm);
                generator
            });
        }
        RadioSetting::EnhancedAckHeaderIes(ies) => radio
            .enhanced_ack()
            .as_mut()
            .ok_or(SettingError::EnhancedAckDisabled)?
            .set_header_ies(ies)
            .map_err(|_| SettingError::HeaderIesTooLong {
                capacity: IEEE802154_ENHANCED_ACK_IE_CAPACITY,
            })?,
        RadioSetting::EnhancedAckProbing(initiators) => radio
            .enhanced_ack()
            .as_mut()
            .ok_or(SettingError::EnhancedAckDisabled)?
            .probing()
            .replace(initiators),
        // `esp_ieee802154_set_transmit_security`; the transmission's later
        // attempts arm it again.
        RadioSetting::TransmitSecurity(arming) => {
            radio.set_transmit_security(hardware, arming.frame, &arming.key, &arming.address);
        }
    }
    Ok(())
}

impl<M: RawMutex, H, T: Timer, R: Ieee802154Random, const EVENTS: usize> RadioPort
    for Ieee802154Runtime<'_, M, H, T, R, EVENTS>
where
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
{
    type Event = Ieee802154RadioEvent;
    type Id = RequestId;
    type Domain = oer_ieee802154::Ieee802154Radio;
    /// The runtime never poisons.
    type Fault = Infallible;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154RadioEvent, EventsLost, Infallible>> + '_ {
        self.wait_event()
    }

    /// The radio clock (`otPlatRadioGetNow`): the runtime's own clock, the
    /// one its engine reads at every event. It runs whether or not a radio
    /// is installed.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, Infallible>> + '_ {
        ready(Ok(Ok(Ieee802154Instant::from_micros(
            self.timer.now().as_micros(),
        ))))
    }

    fn cancel(
        &self,
        id: RequestId,
    ) -> impl Future<Output = PortResult<(), CancelError, Infallible>> + '_ {
        ready(Ok(self.cancel_locked(id)))
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Infallible>> + '_ {
        ready(Ok(self.lifecycle_locked(command)))
    }
}

impl<M: RawMutex, H, T: Timer, R: Ieee802154Random, const EVENTS: usize> Ieee802154RadioPort
    for Ieee802154Runtime<'_, M, H, T, R, EVENTS>
where
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
{
    fn view(event: &Ieee802154RadioEvent) -> RadioEvent<'_> {
        event.portable()
    }

    /// The installed radio's capabilities and interfaces, or the role's
    /// with the primary interface alone without one.
    fn capabilities(&self) -> Ieee802154Capabilities {
        self.installed.lock(|installed| {
            installed.borrow().as_ref().map_or(
                Ieee802154Capabilities {
                    operations: IEEE802154_RADIO_CAPABILITIES,
                    interfaces: 1,
                },
                |installed| Ieee802154Capabilities {
                    operations: installed.radio.capabilities(),
                    interfaces: installed.radio.interfaces(),
                },
            )
        })
    }

    fn submit(
        &self,
        command: RadioCommand<'_>,
    ) -> PortResult<AcceptedCommand, CommandError, Infallible> {
        Ok(self.submit_locked(command))
    }

    /// The radio clock is the runtime's clock (`esp_timer_get_time` in
    /// ESP-IDF), which the ESP32-S31 composition binds to the image's
    /// monotonic clock.
    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    fn state(&self) -> PortResult<RadioState, NotInstalled, Infallible> {
        Ok(self.with_radio(|radio, _, _| radio.state()))
    }

    fn apply(&self, setting: RadioSetting<'_>) -> PortResult<(), SettingError, Infallible> {
        Ok(self
            .with_radio(|radio, hardware, _| apply_setting(radio, hardware, setting))
            .unwrap_or(Err(SettingError::NotInstalled)))
    }

    fn frame_counter(
        &self,
        interface: Interface,
    ) -> PortResult<Option<u32>, NotInstalled, Infallible> {
        Ok(self.with_radio(|radio, _, _| {
            radio
                .interface_mac_keys(interface)
                .and_then(|keys| keys.as_ref().map(MacKeys::frame_counter))
        }))
    }

    /// The live RSSI (`esp_ieee802154_get_recent_rssi`), read from the
    /// hardware whatever the radio's state, as the vendor reads it.
    // CAPABILITY: ieee802154-phy-and-rf-rssi
    fn recent_rssi(&self) -> PortResult<i8, NotInstalled, Infallible> {
        Ok(self.with_radio(|_, hardware, _| hardware.recent_rssi()))
    }
}

/// Host models of the hardware for tests of this runtime and the crates
/// that drive it; host targets only, where no MAC writes received frames.
#[cfg(all(any(test, feature = "model"), not(target_arch = "riscv32")))]
impl<M: RawMutex, T: Timer, R: Ieee802154Random, const EVENTS: usize>
    Ieee802154Runtime<
        '_,
        M,
        oer_espressif_ieee802154_engine::ll::model::Ieee802154LlModel,
        T,
        R,
        EVENTS,
    >
{
    /// Model the MAC DMA writing `frame` into the published receive buffer
    /// and raising `events`, then run the interrupt handler.
    ///
    /// # Panics
    ///
    /// No radio is installed, or a frame arrives with no receive buffer
    /// published.
    pub fn model_interrupt(
        &self,
        frame: Option<&[u8]>,
        events: &[oer_espressif_ieee802154_engine::types::Ieee802154Event],
    ) {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().expect("a radio is installed");
            if let Some(frame) = frame {
                let address = installed
                    .hardware
                    .rx_address
                    .expect("a receive buffer is published");
                assert!(installed.radio.engine().model_dma_write(address, frame));
            }
            installed.hardware.raise(events);
        });
        self.on_interrupt();
    }

    /// Inspect or set the modelled hardware.
    ///
    /// # Errors
    ///
    /// No radio is installed.
    pub fn with_model<O>(
        &self,
        entry: impl FnOnce(&mut oer_espressif_ieee802154_engine::ll::model::Ieee802154LlModel) -> O,
    ) -> Result<O, NotInstalled> {
        self.with_radio(|_, hardware, _| entry(hardware))
    }
}

#[cfg(test)]
mod tests;
