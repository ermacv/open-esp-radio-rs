//! The ESP32-S31 Wi-Fi backend as an `Ieee80211LowerMacPort`.
//!
//! [`Esp32s31LowerMac`] holds the sans-IO
//! [`LowerMacCore`] of `oer-esp32s31-ieee80211` and its register owner in one
//! blocking mutex over a caller-chosen raw mutex, as the IEEE 802.15.4
//! runtime does. The MAC interrupt handler ([`Esp32s31LowerMac::on_interrupt`]),
//! the receive producer ([`Esp32s31LowerMac::on_received`]) and every port
//! call run inside that lock; events leave it as owned values through
//! bounded queues that any executor may await.
//!
//! Each kind of event has its own queue. Attempt completions and lifecycle
//! terminals cannot overflow: the port refuses an attempt (`Busy`) or a
//! lifecycle command (`LifecycleError::Busy`) while its queue could not hold
//! the terminal event it would owe. TBTT events and received frames have
//! queues of their own; an overflow of either is reported as [`EventsLost`]
//! in that queue's order, and a received MPDU longer than `FRAME` is
//! reported as [`LowerMacEvent::RxTooLong`], not as a loss. Completions are
//! taken first, then lifecycle terminals, TBTTs and received frames; the
//! loss ordering rule holds within each queue. A fault poisons the port:
//! the queued events are still reported, then [`LowerMacEvent::Poisoned`]
//! at every call.
//!
//! [`Ieee80211LowerMacPort::next_event`] only takes events. The two waits
//! the core cannot run itself are in [`Esp32s31LowerMac::run`], the runner
//! the composition polls for as long as the port exists: the publication
//! watchdog of the attempts in flight, which it turns into a deadline edge
//! of the ordinary TX owner, and the PHY retune an `Enable` needs after a
//! channel change, through [`LowerMacRetune`]. The ordinary TX owner's timer
//! and the port's own timer must therefore read the image's monotonic clock.
//! The port's timer is also the MAC clock ([`ReceptionClock`]): the port's
//! radio clock is the Wi-Fi MAC local time, the counter receive timestamps
//! carry, affine to the monotonic clock ([`MAC_CLOCK_INFO`]).
//!
//! Besides the base port it implements the extensions the ESP32-S31 has:
//! [`LowerMacAmpdu`] (HT aggregates, when the core was built with aggregate
//! owners over an [`AmpduBackingSource`]), [`LowerMacBeaconTiming`] (the
//! station TSF and TBTT schedule, whose events the power interrupt delivers
//! through [`Esp32s31LowerMac::on_power_interrupt`], and the access-point TSF
//! restart) and [`LowerMacMonitor`].
//!
//! Each of the four ordinary EDCA queues holds one attempt; completions leave
//! the queue in whatever order the queues end, correlated by `TxId`. The MAC
//! interrupt entry offers each edge to every published queue, and the
//! watchdog waits for the earliest deadline of any of them.
//!
//! This is an additional entry point: the station and access-point roles do
//! not drive the backend through it yet.

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
};

use embassy_futures::select::{Either, select, select4};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_esp32s31_hal::types::MacPowerInterruptObservation;
use oer_esp32s31_ieee80211::{
    lower_mac::{
        AmpduBacking, AmpduBackingSource, ESP32S31_BEACON_TIMING_CAPABILITIES,
        ESP32S31_MONITOR_CAPABILITIES, Esp32s31AmpduAttempt, Esp32s31AmpduBuffer,
        Esp32s31MpduAttempt, Esp32s31TxBuffer, LifecycleStart, LowerMacCore, LowerMacFault,
        LowerMacHardware, LowerMacSink, NoAmpdu, esp32s31_ampdu_capabilities,
        esp32s31_lower_mac_capabilities,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
    tx::WifiTxWake,
};
use oer_esp32s31_ieee80211_mac::rx::NormalizedRxFrame;

/// The radio system's beacon-window priority, as its last released guard
/// left it: the source a core receives beacons at when the station asks
/// ([`LowerMacCore::new`]).
#[cfg(target_arch = "riscv32")]
pub struct RadioBeaconWindow<'a>(pub &'a oer_esp32s31_radio_runtime::WifiCoexViewCell);

#[cfg(target_arch = "riscv32")]
impl oer_esp32s31_ieee80211::lower_mac::BeaconWindowPriority for RadioBeaconWindow<'_> {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        oer_esp32s31_hal::types::MacPti::new(u32::from(self.0.get().beacon_pti.value()))
            .expect("coexistence priorities are four-bit values")
    }
}
use oer_ieee80211_lower_mac::{
    AmpduCapabilities, BeaconTimingCapabilities, CancelError, ClockInfo, EventsLost, FailureClass,
    Ieee80211LowerMacPort, Ieee80211Stamp, KeyHandle, KeyInstall, LifecycleCommand, LifecycleError,
    LifecycleEvent, LowerMacAmpdu, LowerMacBeaconTiming, LowerMacCapabilities, LowerMacEvent,
    LowerMacMonitor, LowerMacSetting, MonitorCapabilities, Poisoned, PortError, Refused,
    RxEvidence, RxMeta, SettingError, SubmitError, SubmitResult, TbttEvent, TbttSchedule,
    TsfSample, TxCompletion, TxId, VifId, VifTsf,
};
use oer_ieee80211_lower_mac::{Ieee80211ClockSample, Ieee80211Instant};

use crate::mac_clock::{MAC_CLOCK_INFO, ReceptionClock};
use oer_ieee80211_mac::channel::WifiChannel;

/// Attempt completions the port owes at most: admitted attempts whose
/// completion the consumer has not taken yet.
pub const COMPLETION_CAPACITY: usize = 8;
/// Lifecycle terminals the port owes at most.
pub const LIFECYCLE_CAPACITY: usize = 4;
/// TBTT events waiting for the consumer, a loss marker included.
pub const TBTT_CAPACITY: usize = 2;

const COMPLETION_SLOTS: usize = COMPLETION_CAPACITY + 1;
const LIFECYCLE_SLOTS: usize = LIFECYCLE_CAPACITY + 1;

/// The PHY retune of an `Enable` after a channel change.
///
/// The ESP32-S31 retunes through `switch_esp32s31_wifi_channel` under the
/// shared-radio lease while the MAC's DMA and interrupt service are stopped,
/// which a disabled port guarantees.
pub trait LowerMacRetune {
    /// Tune to `channel`; `false` when the PHY refused.
    fn retune(&mut self, channel: WifiChannel) -> impl Future<Output = bool>;
}

/// One owned event of the port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Esp32s31LowerMacEvent<const FRAME: usize> {
    /// A received MPDU, its first `length` bytes in `frame`.
    Received {
        frame: [u8; FRAME],
        length: usize,
        meta: RxMeta,
    },
    /// A received MPDU longer than `FRAME` was dropped.
    RxTooLong {
        length: usize,
    },
    TxCompleted(TxCompletion),
    Lifecycle(LifecycleEvent),
    /// A station TBTT, viewed through [`LowerMacBeaconTiming::tbtt`].
    Tbtt(TbttEvent),
    /// The terminal event of a poisoned port.
    Poisoned,
}

impl<const FRAME: usize> Esp32s31LowerMacEvent<FRAME> {
    /// Lend the event as a portable value.
    pub fn portable(&self) -> LowerMacEvent<'_> {
        match self {
            Self::Received {
                frame,
                length,
                meta,
            } => LowerMacEvent::Received {
                frame: &frame[..*length],
                meta: *meta,
            },
            Self::RxTooLong { length } => LowerMacEvent::RxTooLong { length: *length },
            Self::TxCompleted(completion) => LowerMacEvent::TxCompleted(*completion),
            Self::Lifecycle(event) => LowerMacEvent::Lifecycle(*event),
            Self::Tbtt(_) => LowerMacEvent::Extension,
            Self::Poisoned => LowerMacEvent::Poisoned(Poisoned),
        }
    }
}

/// Why the port cannot serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31LowerMacError {
    /// No backend is installed: a [`FailureClass::Rejected`] state that an
    /// install ends.
    NotInstalled,
    /// The backend's state is unknown; only a radio reset restores it.
    Poisoned(LowerMacFault),
    /// The port's MAC clock belongs to a radio start a later one replaced:
    /// a [`FailureClass::Rejected`] state of a port that outlived its start.
    StaleClock,
}

impl PortError for Esp32s31LowerMacError {
    fn class(&self) -> FailureClass {
        match self {
            Self::NotInstalled | Self::StaleClock => FailureClass::Rejected,
            Self::Poisoned(_) => FailureClass::Poisoned,
        }
    }
}

/// A bounded queue whose overflow is reported in its order: a loss marker
/// takes the place of the first dropped entry.
struct EventQueue<M: RawMutex, T, const N: usize> {
    entries: Channel<M, Result<T, EventsLost>, N>,
    /// An entry was dropped and its marker is not queued yet.
    lost: Mutex<M, Cell<bool>>,
}

impl<M: RawMutex, T, const N: usize> EventQueue<M, T, N> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            lost: Mutex::new(Cell::new(false)),
        }
    }

    /// Queue `entry` behind a pending loss marker; drop it and remember the
    /// loss when there is no room.
    fn push(&self, entry: T) {
        self.lost.lock(|lost| {
            if lost.get() {
                if self.entries.try_send(Err(EventsLost)).is_err() {
                    return;
                }
                lost.set(false);
            }
            if self.entries.try_send(Ok(entry)).is_err() {
                lost.set(true);
            }
        });
    }

    /// The next entry, or the loss marker after the last entry.
    fn take(&self) -> Option<Result<T, EventsLost>> {
        self.lost.lock(|lost| match self.entries.try_receive() {
            Ok(entry) => Some(entry),
            Err(_) if lost.replace(false) => Some(Err(EventsLost)),
            Err(_) => None,
        })
    }

    /// Discard every entry. A discarded event, a pending marker or `owed`
    /// entries that will never arrive leave one loss for the consumer.
    fn discard(&self, owed: bool) {
        self.lost.lock(|lost| {
            let mut discarded = owed || lost.get();
            while let Ok(_entry) = self.entries.try_receive() {
                discarded = true;
            }
            lost.set(discarded);
        });
    }
}

/// The queues of the port, one per kind of event.
struct Queues<M: RawMutex, const EVENTS: usize, const FRAME: usize> {
    completions: EventQueue<M, TxCompletion, COMPLETION_SLOTS>,
    lifecycle: EventQueue<M, LifecycleEvent, LIFECYCLE_SLOTS>,
    tbtt: EventQueue<M, TbttEvent, TBTT_CAPACITY>,
    received: EventQueue<M, Esp32s31LowerMacEvent<FRAME>, EVENTS>,
    /// Completions owed: admitted attempts whose completion was not taken.
    owed_completions: Mutex<M, Cell<usize>>,
    /// Lifecycle terminals owed.
    owed_lifecycle: Mutex<M, Cell<usize>>,
    /// Raised when the consumer must look again without a new entry: a
    /// poisoning, or a loss an uninstall recorded.
    changed: Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize, const FRAME: usize> Queues<M, EVENTS, FRAME> {
    const fn new() -> Self {
        Self {
            completions: EventQueue::new(),
            lifecycle: EventQueue::new(),
            tbtt: EventQueue::new(),
            received: EventQueue::new(),
            owed_completions: Mutex::new(Cell::new(0)),
            owed_lifecycle: Mutex::new(Cell::new(0)),
            changed: Signal::new(),
        }
    }

    /// The next queued event: completions, lifecycle terminals, TBTTs,
    /// then received frames.
    fn take(&self) -> Option<Result<Esp32s31LowerMacEvent<FRAME>, EventsLost>> {
        if let Some(entry) = self.completions.take() {
            if entry.is_ok() {
                self.owed_completions
                    .lock(|owed| owed.set(owed.get().saturating_sub(1)));
            }
            return Some(entry.map(Esp32s31LowerMacEvent::TxCompleted));
        }
        if let Some(entry) = self.lifecycle.take() {
            if entry.is_ok() {
                self.owed_lifecycle
                    .lock(|owed| owed.set(owed.get().saturating_sub(1)));
            }
            return Some(entry.map(Esp32s31LowerMacEvent::Lifecycle));
        }
        if let Some(entry) = self.tbtt.take() {
            return Some(entry.map(Esp32s31LowerMacEvent::Tbtt));
        }
        self.received.take()
    }

    /// Wait until a queue may have an entry or [`Self::changed`] rose.
    async fn wait(&self) {
        select4(
            self.completions.entries.ready_to_receive(),
            self.lifecycle.entries.ready_to_receive(),
            select(
                self.tbtt.entries.ready_to_receive(),
                self.received.entries.ready_to_receive(),
            ),
            self.changed.wait(),
        )
        .await;
    }

    /// Discard every queued event on uninstall; terminal events the
    /// uninstalled core still owed are lost as well, and every loss stays
    /// pending for the consumer.
    fn discard(&self) {
        let owed = |owed: &Mutex<M, Cell<usize>>| owed.lock(|owed| owed.replace(0)) > 0;
        self.completions.discard(owed(&self.owed_completions));
        self.lifecycle.discard(owed(&self.owed_lifecycle));
        self.tbtt.discard(false);
        self.received.discard(false);
        self.changed.signal(());
    }
}

/// The core, its register owner and the retune of `Enable`.
pub struct Esp32s31LowerMacParts<
    'slot,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
> {
    pub core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    pub hardware: H,
    pub retune: R,
}

struct Installed<
    'slot,
    P,
    E,
    T,
    H,
    S: AmpduBacking,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> {
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    hardware: H,
}

/// The sink of one locked entry: events go to their queues.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize, const FRAME: usize> {
    queues: &'a Queues<M, EVENTS, FRAME>,
}

impl<M: RawMutex, const EVENTS: usize, const FRAME: usize> LowerMacSink
    for QueueSink<'_, M, EVENTS, FRAME>
{
    fn tx_completed(&mut self, completion: TxCompletion) {
        self.queues.completions.push(completion);
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        self.queues.lifecycle.push(event);
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.queues.tbtt.push(event);
    }
}

/// The ESP32-S31 lower MAC behind the portable port.
///
/// `TX_BUFFERS` counts the transmit buffers the core lends, `EVENTS` bounds
/// the received frames waiting for the consumer (a loss marker included)
/// and `FRAME` the bytes of one received MPDU; a longer MPDU is reported as
/// [`LowerMacEvent::RxTooLong`].
/// `S`, `AMPDU_SLOTS` and `AMPDU_BUFFERS` are the core's aggregate memory,
/// subframes per aggregate and aggregate owners; with the default
/// [`NoAmpdu`] the port has no aggregates and no [`LowerMacAmpdu`].
pub struct Esp32s31LowerMac<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
> {
    #[allow(
        clippy::type_complexity,
        reason = "the installed owners keep their types"
    )]
    installed: Mutex<
        M,
        RefCell<
            Option<
                Installed<
                    'slot,
                    P,
                    E,
                    T,
                    H,
                    S,
                    BUFFER_SIZE,
                    TX_BUFFERS,
                    AMPDU_SLOTS,
                    AMPDU_BUFFERS,
                >,
            >,
        >,
    >,
    retune: Mutex<M, RefCell<Option<R>>>,
    /// The channel an admitted `Enable` waits to be tuned to.
    pending_retune: Mutex<M, Cell<Option<WifiChannel>>>,
    fault: Mutex<M, Cell<Option<LowerMacFault>>>,
    queues: Queues<M, EVENTS, FRAME>,
    /// Raised when a deadline, a retune, an install or a fault may have
    /// started, so the runner rearms.
    wake: Signal<M, ()>,
    /// The image's monotonic time the runner's watchdog waits on; the same
    /// time the installed core's transmit owner reads.
    timer: T,
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> Default
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
>
    Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    /// An empty port whose runner waits on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            installed: Mutex::new(RefCell::new(None)),
            retune: Mutex::new(RefCell::new(None)),
            pending_retune: Mutex::new(Cell::new(None)),
            fault: Mutex::new(Cell::new(None)),
            queues: Queues::new(),
            wake: Signal::new(),
        }
    }

    /// Install a disabled core with its register owner and retune. A loss
    /// an earlier uninstall recorded stays pending.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a backend is already installed.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install(
        &self,
        parts: Esp32s31LowerMacParts<
            'slot,
            P,
            E,
            T,
            H,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
        >,
    ) -> Result<
        (),
        Esp32s31LowerMacParts<
            'slot,
            P,
            E,
            T,
            H,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
        >,
    > {
        let Esp32s31LowerMacParts {
            core,
            hardware,
            retune,
        } = parts;
        let refused = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Some((core, hardware));
            }
            *installed = Some(Installed { core, hardware });
            None
        });
        if let Some((core, hardware)) = refused {
            return Err(Esp32s31LowerMacParts {
                core,
                hardware,
                retune,
            });
        }
        self.retune.lock(|slot| *slot.borrow_mut() = Some(retune));
        self.fault.lock(|fault| fault.set(None));
        self.wake.signal(());
        Ok(())
    }

    /// Take the core and its register owner back and discard queued events.
    /// The discarded events, and the terminal events the core still owed,
    /// are reported as [`EventsLost`] to the consumer, also across a later
    /// install. The retune stays while an `Enable` awaits it.
    #[allow(
        clippy::type_complexity,
        reason = "the uninstalled owners are returned as they were installed"
    )]
    pub fn uninstall(
        &self,
    ) -> Option<(
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
        H,
    )> {
        let installed = self
            .installed
            .lock(|installed| installed.borrow_mut().take());
        self.queues.discard();
        self.pending_retune.lock(|pending| pending.set(None));
        installed.map(|installed| (installed.core, installed.hardware))
    }

    /// Run one entry under the lock; a fault poisons the port.
    fn with_core<U>(
        &self,
        entry: impl FnOnce(
            &mut LowerMacCore<
                'slot,
                P,
                E,
                T,
                BUFFER_SIZE,
                TX_BUFFERS,
                S,
                AMPDU_SLOTS,
                AMPDU_BUFFERS,
            >,
            &mut H,
            &mut QueueSink<'_, M, EVENTS, FRAME>,
        ) -> Result<U, LowerMacFault>,
    ) -> Result<U, Esp32s31LowerMacError> {
        if let Some(fault) = self.fault.lock(Cell::get) {
            return Err(Esp32s31LowerMacError::Poisoned(fault));
        }
        let result = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed
                .as_mut()
                .ok_or(Esp32s31LowerMacError::NotInstalled)?;
            let mut sink = QueueSink {
                queues: &self.queues,
            };
            entry(&mut installed.core, &mut installed.hardware, &mut sink)
                .map_err(Esp32s31LowerMacError::Poisoned)
        });
        if let Err(Esp32s31LowerMacError::Poisoned(fault)) = result {
            self.fault.lock(|poisoned| poisoned.set(Some(fault)));
            // The consumer learns of it after the queued events; the runner
            // parks.
            self.queues.changed.signal(());
            self.wake.signal(());
        }
        result
    }

    fn poisoned(&self) -> bool {
        self.fault.lock(Cell::get).is_some()
    }

    /// The MAC interrupt handler's TX edges: completion, hardware timeout
    /// and collision events of `oer_esp32s31_ieee80211_mac::irq`.
    pub fn on_interrupt(&self, events: u32) {
        let _ = self.with_core(|core, hardware, sink| {
            core.service(hardware, WifiTxWake::Interrupt { events }, sink)
        });
        // A timeout starts the abort-settle deadline.
        self.wake.signal(());
    }

    /// The power interrupt's semantic causes: a station TBTT is reported as
    /// an event while its schedule runs.
    pub fn on_power_interrupt(&self, observation: MacPowerInterruptObservation) {
        if observation.sta_tbtt() {
            let _ = self.with_core(|core, hardware, sink| {
                core.station_tbtt(hardware, sink);
                Ok(())
            });
        }
    }

    /// One MPDU of the receive producer, copied into the receive queue while
    /// the port receives and a receive rule or monitor reception admits it.
    /// An MPDU longer than `FRAME` is reported as
    /// [`LowerMacEvent::RxTooLong`].
    pub fn on_received(&self, frame: &NormalizedRxFrame<'_>)
    where
        T: ReceptionClock,
    {
        // The receive timestamp, in the generation it was taken in; a
        // frame received before the snapshot always has its place unless a
        // break left it without one.
        let stamp = frame
            .stamp
            .and_then(|raw| self.timer.snapshot()?.stamp(raw))
            .map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved);
        let _ = self.with_core(|core, _, sink| {
            if let Some((bytes, mut meta)) = core.received(frame) {
                meta.timestamp = stamp;
                let mut owned = [0; FRAME];
                let event = match owned.get_mut(..bytes.len()) {
                    Some(prefix) => {
                        prefix.copy_from_slice(bytes);
                        Esp32s31LowerMacEvent::Received {
                            frame: owned,
                            length: bytes.len(),
                            meta,
                        }
                    }
                    None => Esp32s31LowerMacEvent::RxTooLong {
                        length: bytes.len(),
                    },
                };
                sink.queues.received.push(event);
            }
            Ok(())
        });
    }

    /// The port's runner: the publication watchdog of the attempts in
    /// flight and the PHY retune of an admitted `Enable`.
    ///
    /// Poll it for as long as the port exists, beside the consumer of
    /// [`Ieee80211LowerMacPort::next_event`]; it never ends. Dropping it
    /// keeps both for the next runner. While the port is poisoned it only
    /// waits for an install, so an expired deadline of the poisoned core is
    /// never serviced again; a deadline the core kept after servicing it
    /// waits for the next wake instead of being serviced again at once.
    pub async fn run(&self) -> Infallible {
        // The deadline serviced last, which the core may keep until an
        // interrupt edge moves it.
        let mut serviced = None;
        loop {
            if self.poisoned() {
                self.wake.wait().await;
                continue;
            }
            if let Some(channel) = self.pending_retune.lock(Cell::take) {
                let mut guard = RetuneGuard {
                    slot: &self.retune,
                    pending: &self.pending_retune,
                    retune: self.retune.lock(|slot| slot.borrow_mut().take()),
                    channel,
                    done: false,
                };
                let retuned = match guard.retune.as_mut() {
                    Some(retune) => retune.retune(channel).await,
                    None => false,
                };
                guard.done = true;
                drop(guard);
                let _ = self.with_core(|core, _, sink| {
                    core.finish_retune(retuned, sink);
                    Ok(())
                });
                continue;
            }
            let deadline = self
                .installed
                .lock(|installed| {
                    installed
                        .borrow()
                        .as_ref()
                        .map(|installed| installed.core.next_deadline())
                })
                .flatten()
                .filter(|deadline| Some(*deadline) != serviced);
            let watchdog = async {
                match deadline {
                    Some(deadline) => self.timer.wait_until(deadline).await,
                    None => core::future::pending().await,
                }
            };
            match select(watchdog, self.wake.wait()).await {
                Either::First(()) => {
                    serviced = deadline;
                    let _ = self.with_core(|core, hardware, sink| {
                        core.service(hardware, WifiTxWake::Deadline, sink)
                    });
                }
                // A deadline, a retune, an install or a fault; rearm.
                Either::Second(()) => serviced = None,
            }
        }
    }

    /// Take the next event: the queued ones in their order, then the
    /// terminal [`Esp32s31LowerMacEvent::Poisoned`] of a poisoned port.
    async fn wait_event(&self) -> Result<Esp32s31LowerMacEvent<FRAME>, EventsLost> {
        loop {
            if let Some(event) = self.queues.take() {
                return event;
            }
            if self.poisoned() {
                return Ok(Esp32s31LowerMacEvent::Poisoned);
            }
            self.queues.wait().await;
        }
    }
}

/// Returns the retune to its slot, and the channel to the pending retune
/// when the wait was dropped before the retune finished.
struct RetuneGuard<'a, M: RawMutex, R> {
    slot: &'a Mutex<M, RefCell<Option<R>>>,
    pending: &'a Mutex<M, Cell<Option<WifiChannel>>>,
    retune: Option<R>,
    channel: WifiChannel,
    done: bool,
}

impl<M: RawMutex, R> Drop for RetuneGuard<'_, M, R> {
    fn drop(&mut self) {
        let retune = self.retune.take();
        self.slot.lock(|slot| *slot.borrow_mut() = retune);
        if !self.done {
            self.pending.lock(|pending| pending.set(Some(self.channel)));
        }
    }
}

/// Admit `attempt` through `admit` only while the completion queue has room
/// for its completion, and count the completion it then owes.
fn owe_completion<M: RawMutex, A, const EVENTS: usize, const FRAME: usize>(
    queues: &Queues<M, EVENTS, FRAME>,
    attempt: A,
    admit: impl FnOnce(A) -> Result<Result<(), Refused<A>>, LowerMacFault>,
) -> Result<Result<(), Refused<A>>, LowerMacFault> {
    queues.owed_completions.lock(|owed| {
        if owed.get() == COMPLETION_CAPACITY {
            return Ok(Err(Refused {
                error: SubmitError::Busy,
                attempt,
            }));
        }
        let admitted = admit(attempt)?;
        if admitted.is_ok() {
            owed.set(owed.get() + 1);
        }
        Ok(admitted)
    })
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> Ieee80211LowerMacPort
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    type Event = Esp32s31LowerMacEvent<FRAME>;
    type Error = Esp32s31LowerMacError;
    type TxBuffer = Esp32s31TxBuffer<'slot, BUFFER_SIZE>;

    fn view(event: &Esp32s31LowerMacEvent<FRAME>) -> LowerMacEvent<'_> {
        event.portable()
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        esp32s31_lower_mac_capabilities(BUFFER_SIZE)
    }

    /// The radio clock is the Wi-Fi MAC local time, the counter of receive
    /// timestamps: affine to the monotonic clock ([`MAC_CLOCK_INFO`]).
    fn clock_info(&self) -> ClockInfo {
        MAC_CLOCK_INFO
    }

    fn tx_buffer(
        &self,
        len: usize,
    ) -> Result<Option<Esp32s31TxBuffer<'slot, BUFFER_SIZE>>, Esp32s31LowerMacError> {
        self.with_core(|core, _, _| Ok(core.tx_buffer(len)))
    }

    /// A buffer released while no backend is installed or the port is
    /// poisoned is lost until the radio is reset.
    fn release_tx_buffer(&self, buffer: Esp32s31TxBuffer<'slot, BUFFER_SIZE>) {
        let _ = self.with_core(|core, _, _| {
            core.release_tx_buffer(buffer);
            Ok(())
        });
    }

    fn submit(
        &self,
        attempt: Esp32s31MpduAttempt<'slot, BUFFER_SIZE>,
    ) -> SubmitResult<Esp32s31MpduAttempt<'slot, BUFFER_SIZE>, Esp32s31LowerMacError> {
        let queues = &self.queues;
        let admitted = self.with_core(|core, hardware, _| {
            owe_completion(queues, attempt, |attempt| core.submit(hardware, attempt))
        })?;
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
    }

    async fn next_event(&self) -> Result<Esp32s31LowerMacEvent<FRAME>, EventsLost> {
        self.wait_event().await
    }

    fn apply(
        &self,
        setting: LowerMacSetting,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        let applied = self.with_core(|core, hardware, _| core.apply(hardware, setting))?;
        // Opening the transmit gate publishes a held attempt.
        self.wake.signal(());
        Ok(applied)
    }

    fn install_key(
        &self,
        key: KeyInstall<'_>,
    ) -> Result<Result<KeyHandle, SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.install_key(hardware, key)))
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> Result<Result<(), LifecycleError>, Esp32s31LowerMacError> {
        let queues = &self.queues;
        let started = self.with_core(|core, _, sink| {
            Ok(queues.owed_lifecycle.lock(|owed| {
                // Admit only a command whose terminal event has room.
                if owed.get() == LIFECYCLE_CAPACITY {
                    return Err(LifecycleError::Busy);
                }
                let started = core.lifecycle(command, sink);
                if started.is_ok() {
                    owed.set(owed.get() + 1);
                }
                started
            }))
        })?;
        match started {
            Ok(LifecycleStart::Admitted) => Ok(Ok(())),
            Ok(LifecycleStart::Retune(channel)) => {
                self.pending_retune
                    .lock(|pending| pending.set(Some(channel)));
                self.wake.signal(());
                Ok(Ok(()))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    fn cancel(&self, id: TxId) -> Result<Result<(), CancelError>, Esp32s31LowerMacError> {
        self.with_core(|core, _, sink| Ok(core.cancel(id, sink)))
    }

    fn now(&self) -> Result<Ieee80211Instant, Esp32s31LowerMacError> {
        self.with_core(|_, _, _| Ok(self.timer.snapshot()))?
            .map(|snapshot| snapshot.sample().radio)
            .ok_or(Esp32s31LowerMacError::StaleClock)
    }

    /// The MAC local time and the monotonic time read back to back, in the
    /// current generation of the MAC clock.
    fn clock_sample(&self) -> Result<Ieee80211ClockSample, Esp32s31LowerMacError> {
        self.with_core(|_, _, _| Ok(self.timer.snapshot()))?
            .map(|snapshot| snapshot.sample())
            .ok_or(Esp32s31LowerMacError::StaleClock)
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacBeaconTiming
    for Esp32s31LowerMac<
        '_,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities {
        ESP32S31_BEACON_TIMING_CAPABILITIES
    }

    fn tsf(&self, vif: VifId) -> Result<Result<VifTsf, SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.tsf(hardware, vif)))
    }

    /// The TSF read between two MAC-clock readings of one generation: the
    /// sample's radio stamp is the first, its uncertainty the distance to
    /// the second plus the counter's microsecond.
    fn tsf_sample(
        &self,
        vif: VifId,
    ) -> Result<Result<TsfSample, SettingError>, Esp32s31LowerMacError> {
        let (before, reading, after) = self.with_core(|core, hardware, _| {
            let before = self.timer.snapshot();
            let reading = core.tsf_reading(hardware, vif);
            Ok((before, reading, self.timer.snapshot()))
        })?;
        let (before, after) = before
            .zip(after)
            .map(|(before, after)| (before.sample(), after.sample()))
            .filter(|(before, after)| before.generation == after.generation)
            .ok_or(Esp32s31LowerMacError::StaleClock)?;
        Ok(reading.map(|(tsf, generation)| TsfSample {
            tsf,
            local: Ieee80211Stamp {
                at: before.radio,
                generation: before.generation,
            },
            uncertainty: oer_time::Duration::from_micros(
                1 + after
                    .radio
                    .as_micros()
                    .saturating_sub(before.radio.as_micros()),
            ),
            generation,
        }))
    }

    fn set_tsf(&self, tsf: VifTsf) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_tsf(hardware, tsf)))
    }

    fn set_tbtt(
        &self,
        schedule: TbttSchedule,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_tbtt(hardware, schedule)))
    }

    fn stop_tbtt(&self, vif: VifId) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.stop_tbtt(hardware, vif)))
    }

    fn tbtt(event: &Esp32s31LowerMacEvent<FRAME>) -> Option<TbttEvent> {
        match event {
            Esp32s31LowerMacEvent::Tbtt(event) => Some(*event),
            _ => None,
        }
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacMonitor
    for Esp32s31LowerMac<
        '_,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn monitor_capabilities(&self) -> MonitorCapabilities {
        ESP32S31_MONITOR_CAPABILITIES
    }

    fn set_monitor(
        &self,
        enabled: bool,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_monitor(hardware, enabled)))
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacAmpdu
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBackingSource,
{
    type AmpduBuffer = Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(AMPDU_SLOTS)
    }

    fn ampdu_buffer(
        &self,
    ) -> Result<Option<Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>>, Esp32s31LowerMacError> {
        self.with_core(|core, _, _| Ok(core.ampdu_buffer()))
    }

    /// An aggregate released while no backend is installed or the port is
    /// poisoned returns its subframes, and its aggregate owner is lost until
    /// the radio is reset.
    fn release_ampdu_buffer(&self, buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>) {
        let _ = self.with_core(|core, _, _| {
            core.release_ampdu_buffer(buffer);
            Ok(())
        });
    }

    fn submit_ampdu(
        &self,
        attempt: Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>,
    ) -> SubmitResult<Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>, Esp32s31LowerMacError> {
        let queues = &self.queues;
        let admitted = self.with_core(|core, hardware, _| {
            owe_completion(queues, attempt, |attempt| {
                core.submit_ampdu(hardware, attempt)
            })
        })?;
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
    }
}

#[cfg(all(test, not(target_pointer_width = "32")))]
mod tests;
