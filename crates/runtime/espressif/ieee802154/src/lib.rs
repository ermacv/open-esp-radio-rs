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
//! [`Ieee802154Runtime::install`] returns the installed radio's two
//! handles. The [`Ieee802154Port`] is the [`Ieee802154RadioPort`](oer_ieee802154::Ieee802154RadioPort) of the
//! protocol's one event consumer: commands, events, the clock, the state and
//! the settings the interrupt handler reads (MAC keys, CSL, the
//! enhanced-ACK generator, armed transmit security). The
//! [`Ieee802154Control`] stays with the composition: shared-PHY maintenance,
//! coexistence, statistics, the frame-pending table and the uninstall, which
//! consumes both. Neither handle exists without an installed radio, so no
//! call is refused as not installed.
//!
//! CSMA-CA backoffs and the delays before retries run on the runtime's
//! [`oer_time::Timer`] in [`Ieee802154Runtime::run`], the runner the composition polls for as long
//! as the radio is installed, as ESP-IDF's OpenThread `SubMac` runs them on
//! its tasklet timer; [`RadioPort::next_event`](oer_ieee802154::RadioPort::next_event) only takes events.
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
//! the radio disabled.
//!
//! Shared-PHY maintenance is a layer over the port's lifecycle:
//! [`Ieee802154Control::pause`] quiesces an enabled port (the consumer sees
//! `Quiesced`), leaves receive mode and lends the hardware out;
//! [`Ieee802154Control::resume`] takes it back, enters receive mode again
//! and enables the port (`Enabled`). A quiesced port holds the radio
//! still: it refuses every command and setting that writes the hardware
//! as quiesced and admits software configuration; while the hardware is lent, lifecycle commands are refused
//! as busy too.

#[cfg(test)]
extern crate std;

mod port;
mod trace;

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
};

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_espressif_ieee802154_engine::engine::Ieee802154Engine;
pub use oer_espressif_ieee802154_engine::engine::{
    Ieee802154RxAbortStatistics, Ieee802154RxStatistics, Ieee802154TxAbortStatistics,
    Ieee802154TxRxStatistics, Ieee802154TxStatistics,
};
use oer_espressif_ieee802154_engine::ll::{Ieee802154LowLevel, Ieee802154RecentRssi};
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_radio::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154Radio, Ieee802154RadioSink,
    writes_registers,
};

use oer_ieee802154::{
    AcceptedCommand, AppliedSecurity, CancelError, CommandError, EventsLost, Frame,
    FrameCounterUpdate, Interface, LifecycleCommand, LifecycleError, LifecycleEvent, MacKeys,
    PortResult, RadioCommand, RadioEvent, RadioFault, RadioSetting, RadioState, ReceivedFrame,
    RequestId, RestingState, RxMetadata, SettingError, TxStatus,
};
use oer_ieee802154_trace::{Lease, PauseRefusal};
use oer_time::{Duration, Instant, Timer};

pub use port::{Ieee802154Control, Ieee802154Port};

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
    hardware: Hardware<H>,
}

/// The hardware of the installed radio, or what shared-PHY maintenance left
/// in its place while it holds it.
enum Hardware<H> {
    Held(H),
    /// [`Ieee802154Control::pause`] lent the hardware out; `recent_rssi` is
    /// the RSSI it read last, which no reception changes while the MAC
    /// stays stopped.
    Lent {
        recent_rssi: i8,
    },
}

/// The invariant of every port and control call: the handles exist only
/// while their radio is installed.
const INSTALLED: &str = "a port or control exists only while its radio is installed";

/// Why the radio could not pause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PauseError {
    /// A transmission, energy scan or CCA is still running.
    Busy,
    /// The event queue has no room for the `Quiesced` and `Enabled` the
    /// pause owes the consumer; taking events frees it.
    EventQueueFull,
}

/// The hardware of a paused radio, lent out across shared PHY maintenance.
///
/// The radio stays installed with its port quiesced, and its interrupt
/// entry does nothing, so the platform CPU route must stay closed until
/// [`Ieee802154Control::resume`].
#[must_use = "a paused radio must be resumed"]
pub struct Ieee802154RuntimePaused<H> {
    hardware: H,
    receiving: Option<oer_ieee802154::Channel>,
    /// The pause quiesced an enabled port; the resume enables it again.
    reopen: bool,
}

impl<H> Ieee802154RuntimePaused<H> {
    /// Borrow the hardware, for example to prove the MAC quiescent.
    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
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
    /// A pause quiesced the port; its resume's `Enabled` holds a slot.
    maintenance: bool,
    /// An event taken after the loss it reported.
    held: Option<Ieee802154RadioEvent>,
}

impl Owed {
    fn reserved(&self) -> usize {
        usize::from(self.operation)
            + usize::from(self.fault)
            + usize::from(self.quiescing)
            + usize::from(self.maintenance)
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
                maintenance: false,
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
    /// ([`ClockInfo::MONOTONIC_MICROS`](oer_ieee802154::ClockInfo::MONOTONIC_MICROS)), for a synchronous reader such as
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
    /// Returns the radio's port, for the protocol's one event consumer, and
    /// its control, for the composition; [`Ieee802154Control::uninstall`]
    /// consumes both.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a radio is already installed.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install(
        &self,
        mut parts: Ieee802154RuntimeParts<'storage, H>,
        random: R,
        defaults: Ieee802154PibDefaults,
    ) -> Result<
        (Ieee802154Port<'_, Self>, Ieee802154Control<'_, Self>),
        Ieee802154RuntimeParts<'storage, H>,
    > {
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
                hardware: Hardware::Held(parts.hardware),
            });
            Ok(())
        };
        self.installed.lock(install)?;
        Ok((Ieee802154Port::new(self), Ieee802154Control::new(self)))
    }

    /// `esp_ieee802154_disable` after the platform CPU route is disabled:
    /// end the operation in flight with its terminal event, return the
    /// MAC's coexistence PTIs to the disabled foundation image, and disable
    /// and return the engine and hardware. Queued events stay with the
    /// consumer.
    fn uninstall(&self) -> Ieee802154RuntimeParts<'storage, H> {
        let parts = self.installed.lock(|installed| {
            let Installed {
                mut radio,
                hardware,
            } = installed.borrow_mut().take().expect(INSTALLED);
            let Hardware::Held(mut hardware) = hardware else {
                panic!("a paused radio is resumed before its uninstall");
            };
            let mut sink = QueueSink {
                events: &self.events,
            };
            // A disabled radio has nothing in flight.
            let _ = radio.disable(&mut hardware, &self.timer, &mut sink);
            let mut engine = radio.into_engine();
            engine.mac_deinit(&mut hardware);
            engine.disable();
            Ieee802154RuntimeParts { engine, hardware }
        });
        self.events.owed(|owed| {
            owed.fault = false;
            owed.quiesced = false;
            owed.maintenance = false;
        });
        self.backoff_until.lock(|backoff| backoff.set(None));
        parts
    }

    /// Whether the port is quiesced or quiescing: it holds the radio still.
    fn quiesced(&self) -> bool {
        self.events.owed(|owed| owed.quiescing || owed.quiesced)
    }

    /// Run `entry` on the installed radio and its slot for the hardware.
    fn with_installed<O>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Radio<'storage, R>,
            &mut Hardware<H>,
            &mut QueueSink<'_, M, EVENTS>,
        ) -> O,
    ) -> O {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().expect(INSTALLED);
            let mut sink = QueueSink {
                events: &self.events,
            };
            entry(&mut installed.radio, &mut installed.hardware, &mut sink)
        })
    }

    /// Run `entry` on the radio and its hardware while a radio is installed
    /// and holds its hardware: the interrupt entry and the runner, which
    /// outlive the installed radio and do nothing while the hardware is
    /// lent.
    fn with_held<O>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Radio<'storage, R>,
            &mut H,
            &mut QueueSink<'_, M, EVENTS>,
        ) -> O,
    ) -> Option<O> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut()?;
            let Hardware::Held(hardware) = &mut installed.hardware else {
                return None;
            };
            let mut sink = QueueSink {
                events: &self.events,
            };
            Some(entry(&mut installed.radio, hardware, &mut sink))
        })
    }

    /// The platform interrupt handler (`ieee802154_isr`). Does nothing while
    /// no radio is installed or a pause holds its hardware.
    pub fn on_interrupt(&self) {
        if let Some(backoff) = self.with_held(|radio, port, sink| {
            radio.isr(port, &self.timer, sink);
            radio.take_delay()
        }) {
            self.start_backoff(backoff);
        }
    }

    /// Admit and start one portable command. A quiesced port holds the radio
    /// still: it refuses every command that writes the MAC registers as
    /// quiesced, as it does while a pause holds the hardware, and admits a
    /// configuration of the PIB or the pending table. An operation reserves
    /// its terminal event's slot first and is refused while no slot is
    /// free.
    fn submit_locked(&self, command: RadioCommand<'_>) -> Result<AcceptedCommand, CommandError> {
        let operation = matches!(
            command,
            RadioCommand::Transmit(_)
                | RadioCommand::EnergyScan(_)
                | RadioCommand::ClearChannelAssessment { .. }
                | RadioCommand::ScheduledReceive(_)
        );
        let submitted = self.with_installed(|radio, hardware, sink| {
            // A configuration of the PIB or the pending table needs no
            // hardware: a quiesced or paused radio takes it too.
            if !writes_registers(&command) {
                return Ok((radio.submit_software_configuration(command), None));
            }
            // A paused radio is quiesced, or disabled: only `Enable` would
            // serve a disabled one, and it waits for the resume.
            let Hardware::Held(hardware) = hardware else {
                return Err(if self.quiesced() {
                    CommandError::Quiesced
                } else {
                    CommandError::Disabled
                });
            };
            if self.quiesced() {
                return Err(CommandError::Quiesced);
            }
            // A second operation is the state machine's to refuse.
            let reserved = operation
                && self.events.owed(|owed| {
                    if owed.operation {
                        return Ok(false);
                    }
                    if !self.events.has_room(owed, 1) {
                        return Err(CommandError::EventQueueFull);
                    }
                    owed.operation = true;
                    Ok(true)
                })?;
            let accepted = radio.submit(hardware, &self.timer, command, sink);
            if accepted.is_err() && reserved {
                self.events.owed(|owed| owed.operation = false);
            }
            Ok((accepted, radio.take_delay()))
        });
        let (accepted, backoff) = submitted?;
        self.start_backoff(backoff);
        accepted
    }

    /// End the running operation `target` through its terminal event. While
    /// a pause holds the hardware, no operation runs.
    fn cancel_locked(&self, target: RequestId) -> Result<(), CancelError> {
        let (cancelled, backoff) = self.with_installed(|radio, hardware, sink| {
            let Hardware::Held(hardware) = hardware else {
                return (Err(CancelError::NotRunning), None);
            };
            let cancelled = radio.cancel(hardware, &self.timer, target, sink);
            (cancelled, radio.take_delay())
        });
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
    ///
    /// Every command is refused as busy while a pause holds the hardware.
    fn lifecycle_locked(&self, command: LifecycleCommand) -> Result<(), LifecycleError> {
        self.with_installed(|radio, hardware, sink| {
            let Hardware::Held(hardware) = hardware else {
                return Err(LifecycleError::Busy);
            };
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
        })
    }

    /// The radio's runner: a transmission's CSMA-CA backoff or retry delay,
    /// after which the transmission goes on.
    ///
    /// Poll it for as long as the runtime is used, beside the consumer of
    /// the port's events; it never ends. Dropping it keeps the delay for the
    /// next runner.
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
                    if let Some(backoff) = self.with_held(|radio, port, sink| {
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
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
    T: Timer,
    R: Ieee802154Random,
    const EVENTS: usize,
> Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    /// Quiesce an enabled port, leave receive mode and lend the hardware
    /// out ([`Ieee802154Control::pause`]).
    fn pause(&self) -> Result<Ieee802154RuntimePaused<H>, Ieee802154PauseError> {
        let result = self.pause_locked();
        trace::emit(|| match &result {
            Ok(paused) => Lease::Paused {
                receiving: paused.receiving.map(oer_ieee802154::Channel::get),
            },
            Err(Ieee802154PauseError::Busy) => Lease::PauseRefused(PauseRefusal::Busy),
            Err(Ieee802154PauseError::EventQueueFull) => {
                Lease::PauseRefused(PauseRefusal::EventQueueFull)
            }
        });
        result
    }

    fn pause_locked(&self) -> Result<Ieee802154RuntimePaused<H>, Ieee802154PauseError> {
        self.with_installed(|radio, slot, sink| {
            let Hardware::Held(hardware) = slot else {
                panic!("a paused radio is resumed before the next pause");
            };
            let state = radio.state();
            let receiving = match state {
                RadioState::Resting(RestingState::Receiving { channel }) => Some(channel),
                RadioState::Resting(RestingState::Sleeping) | RadioState::Disabled => None,
                _ => return Err(Ieee802154PauseError::Busy),
            };
            // An enabled port the consumer has not quiesced is quiesced now
            // and enabled again by the resume: `Quiesced` and `Enabled`.
            let reopen = self.events.owed(|owed| {
                let reopen = state != RadioState::Disabled && !owed.quiesced;
                if reopen && !self.events.has_room(owed, 2) {
                    return Err(Ieee802154PauseError::EventQueueFull);
                }
                Ok(reopen)
            })?;
            if receiving.is_some()
                && radio
                    .submit(
                        hardware,
                        &self.timer,
                        RadioCommand::Sleep { id: PAUSE_REQUEST },
                        sink,
                    )
                    .is_err()
            {
                return Err(Ieee802154PauseError::Busy);
            }
            let recent_rssi = hardware.recent_rssi();
            if reopen {
                self.events.owed(|owed| {
                    owed.quiesced = true;
                    owed.maintenance = true;
                    self.events.send(
                        owed,
                        Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Quiesced),
                    );
                });
            }
            let Hardware::Held(hardware) = core::mem::replace(slot, Hardware::Lent { recent_rssi })
            else {
                unreachable!("the hardware was held above");
            };
            Ok(Ieee802154RuntimePaused {
                hardware,
                receiving,
                reopen,
            })
        })
    }

    /// Take a paused radio's hardware back, enter receive mode again if it
    /// was receiving and enable the port the pause quiesced
    /// ([`Ieee802154Control::resume`]).
    fn resume(&self, paused: Ieee802154RuntimePaused<H>) {
        let Ieee802154RuntimePaused {
            mut hardware,
            receiving,
            reopen,
        } = paused;
        self.with_installed(|radio, slot, sink| {
            assert!(
                matches!(slot, Hardware::Lent { .. }),
                "only a paused radio resumes"
            );
            if let Some(channel) = receiving {
                // The paused state is the sleeping state it left, so the
                // receive admission cannot be rejected.
                let _ = radio.submit(
                    &mut hardware,
                    &self.timer,
                    RadioCommand::Receive {
                        id: PAUSE_REQUEST,
                        channel,
                    },
                    sink,
                );
            }
            *slot = Hardware::Held(hardware);
            if reopen {
                self.events.owed(|owed| {
                    owed.maintenance = false;
                    owed.quiesced = false;
                    self.events.send(
                        owed,
                        Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled),
                    );
                });
            }
        });
        trace::emit(|| Lease::Resumed {
            receiving: receiving.map(oer_ieee802154::Channel::get),
        });
    }
}

/// Apply one portable setting to the radio under the runtime's lock.
///
/// `hardware` is `None` while a pause holds it: a setting that writes it is
/// refused as quiesced.
fn apply_setting<L: Ieee802154LowLevel + ?Sized, R: Ieee802154Random>(
    radio: &mut Ieee802154Radio<'_, R>,
    hardware: Option<&mut L>,
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
            let hardware = hardware.ok_or(SettingError::Quiesced)?;
            radio.set_transmit_security(hardware, arming.frame, &arming.key, &arming.address);
        }
    }
    Ok(())
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
            let Hardware::Held(hardware) = &mut installed.hardware else {
                panic!("the radio holds its hardware");
            };
            if let Some(frame) = frame {
                let address = hardware.rx_address.expect("a receive buffer is published");
                assert!(installed.radio.engine().model_dma_write(address, frame));
            }
            hardware.raise(events);
        });
        self.on_interrupt();
    }

    /// Inspect or set the modelled hardware.
    ///
    /// # Panics
    ///
    /// No radio is installed, or a pause holds its hardware.
    pub fn with_model<O>(
        &self,
        entry: impl FnOnce(&mut oer_espressif_ieee802154_engine::ll::model::Ieee802154LlModel) -> O,
    ) -> O {
        self.with_held(|_, hardware, _| entry(hardware))
            .expect("an installed radio holds its hardware")
    }
}

#[cfg(test)]
mod tests;
