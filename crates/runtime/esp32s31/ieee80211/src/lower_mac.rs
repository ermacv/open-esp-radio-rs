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
//! in that queue's order; a received frame holds its receive unit (on the
//! target, the staging pool's DMA buffer) until the consumer drops it, and
//! a frame dropped with an overflow returns it at once. Completions are
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
    ops::Range,
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
use oer_esp32s31_ieee80211_mac::rx::{
    NormalizedRxFrame, RxError, RxIngressConfig, pool::NetworkRxFrame, view_normalized_rx_frame,
};

/// The radio system's coexistence priorities, as its last released guard
/// left them: the source a core asks for the air with
/// ([`LowerMacCore::new`]).
#[cfg(target_arch = "riscv32")]
pub struct RadioCoex<'a>(pub &'a oer_esp32s31_radio_runtime::WifiCoexViewCell);

#[cfg(target_arch = "riscv32")]
impl oer_esp32s31_ieee80211::lower_mac::RadioCoexPriorities for RadioCoex<'_> {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        oer_esp32s31_hal::types::MacPti::new(u32::from(self.0.get().beacon_pti.value()))
            .expect("coexistence priorities are four-bit values")
    }

    fn connection_frame_priorities(
        &self,
    ) -> oer_esp32s31_ieee80211::lower_mac::ConnectionFramePriorities {
        let view = self.0.get();
        let packet = view.connection_pti.value();
        oer_esp32s31_ieee80211::lower_mac::ConnectionFramePriorities {
            packet,
            scheduler: packet.min(view.slice_pti.value()),
        }
    }
}

use oer_ieee80211_lower_mac::{
    AmpduAttempt, AmpduCapabilities, BeaconTimingCapabilities, CancelError, ClockInfo, EventsLost,
    FailureClass, Ieee80211LowerMacPort, Ieee80211Stamp, KeyHandle, KeyInstall, LifecycleCommand,
    LifecycleError, LifecycleEvent, LowerMacAmpdu, LowerMacBeaconTiming, LowerMacCapabilities,
    LowerMacEvent, LowerMacMonitor, LowerMacSetting, MonitorCapabilities, Poisoned, PortError,
    Refused, RxBuffer, RxEvidence, RxMeta, SettingError, SubmitError, SubmitResult, TbttEvent,
    TbttSchedule, TsfSample, TxCompletion, TxId, VifId, VifTsf,
};
use oer_ieee80211_lower_mac::{
    AmpduBuffer, AmpduPayload, MpduAttempt, ReclaimError, TxBody, TxBuffer, TxPayload,
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

/// One unit of the receive producer: an owned receive buffer whose MPDU and
/// hardware metadata the port reads in place. Dropping it returns the
/// buffer to its producer.
pub trait LowerMacRxUnit {
    /// The whole receive buffer, the hardware's prefix included.
    fn buffer(&self) -> &[u8];
    /// The MPDU and metadata the hardware reported for the unit.
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError>;
}

/// A staging-pool DMA buffer, the target's receive unit, with the ingress
/// configuration of the ring it came from.
pub struct Esp32s31StagedRx<'pool, const SLOTS: usize, const CAPACITY: usize> {
    frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    config: RxIngressConfig,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> Esp32s31StagedRx<'pool, SLOTS, CAPACITY> {
    pub const fn new(
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
        config: RxIngressConfig,
    ) -> Self {
        Self { frame, config }
    }

    /// The staged frame back, for its producer to keep.
    pub fn into_frame(self) -> NetworkRxFrame<'pool, SLOTS, CAPACITY> {
        self.frame
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> LowerMacRxUnit
    for Esp32s31StagedRx<'_, SLOTS, CAPACITY>
{
    fn buffer(&self) -> &[u8] {
        self.frame.segment().buffer
    }

    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError> {
        view_normalized_rx_frame(&self.frame.segment(), self.config)
    }
}

/// A received MPDU the port lends: the receive unit itself, with where the
/// MPDU lies in its buffer. Nothing is copied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Esp32s31RxBuffer<U> {
    unit: U,
    mpdu: Range<usize>,
}

impl<U: LowerMacRxUnit> RxBuffer for Esp32s31RxBuffer<U> {
    fn bytes(&self) -> &[u8] {
        // The port located the MPDU when it received the unit.
        self.unit
            .buffer()
            .get(self.mpdu.clone())
            .unwrap_or_default()
    }
}

/// One owned event of the port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Esp32s31LowerMacEvent<U> {
    /// A received MPDU.
    Received {
        frame: Esp32s31RxBuffer<U>,
        meta: RxMeta,
    },
    TxCompleted(TxCompletion),
    Lifecycle(LifecycleEvent),
    /// A station TBTT, viewed through [`LowerMacBeaconTiming::tbtt`].
    Tbtt(TbttEvent),
    /// The terminal event of a poisoned port.
    Poisoned,
}

impl<U: LowerMacRxUnit> Esp32s31LowerMacEvent<U> {
    /// Lend the event as a portable value.
    pub fn portable(&self) -> LowerMacEvent<'_> {
        match self {
            Self::Received { frame, meta } => LowerMacEvent::Received {
                frame: frame.bytes(),
                meta: *meta,
            },
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

    /// Queue `entry` behind a pending loss marker, or hand it back when
    /// there is no room for both: nothing is lost.
    fn try_push(&self, entry: T) -> Result<(), T> {
        self.lost.lock(|lost| {
            let marker = usize::from(lost.get());
            if self.entries.free_capacity() < marker + 1 {
                return Err(entry);
            }
            if lost.replace(false) {
                let _ = self.entries.try_send(Err(EventsLost));
            }
            self.entries
                .try_send(Ok(entry))
                .map_err(|error| match error {
                    embassy_sync::channel::TrySendError::Full(Ok(entry)) => entry,
                    embassy_sync::channel::TrySendError::Full(Err(_)) => {
                        unreachable!("an entry is sent as `Ok`")
                    }
                })
        })
    }

    /// Entries that fit now.
    fn room(&self) -> usize {
        self.lost.lock(|lost| {
            self.entries
                .free_capacity()
                .saturating_sub(usize::from(lost.get()))
        })
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

/// The bodies of one admitted attempt, which the port holds until the
/// caller reclaims them: an MPDU's, or an aggregate's by subframe.
struct HeldBodies<O, const SUBFRAMES: usize> {
    id: TxId,
    mpdu: Option<O>,
    subframes: [Option<O>; SUBFRAMES],
}

/// An aggregate buffer of the port: the core's, whose MPDUs it fills, and
/// the bodies whose octets it copied after their headers, which the port
/// holds from the attempt's admission.
pub struct Esp32s31PortAmpduBuffer<'slot, S: AmpduBacking, const SLOTS: usize, O> {
    inner: Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    bodies: [Option<O>; SLOTS],
}

impl<S: AmpduBacking, const SLOTS: usize, O> core::fmt::Debug
    for Esp32s31PortAmpduBuffer<'_, S, SLOTS, O>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Esp32s31PortAmpduBuffer")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// The port copies each body after its header as it is pushed: the core
/// sends from its own DMA memory.
impl<S: AmpduBackingSource, const SLOTS: usize, O: TxBody> AmpduBuffer
    for Esp32s31PortAmpduBuffer<'_, S, SLOTS, O>
{
    type Body = O;

    fn push_mpdu(&mut self, len: usize, body: Option<O>) -> Result<&mut [u8], Option<O>> {
        let body_len = body.as_ref().map_or(0, |body| body.bytes().len());
        let Some(header) = len.checked_sub(body_len) else {
            return Err(body);
        };
        let index = self.inner.subframes();
        let Ok(mpdu) = self.inner.push_mpdu(len, None) else {
            return Err(body);
        };
        if let Some(body) = &body {
            mpdu[header..].copy_from_slice(body.bytes());
        }
        if let Some(slot) = self.bodies.get_mut(index) {
            *slot = body;
        }
        Ok(&mut mpdu[..header])
    }

    fn subframes(&self) -> usize {
        self.inner.subframes()
    }
}

/// The queues of the port, one per kind of event.
struct Queues<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> {
    completions: EventQueue<M, TxCompletion, COMPLETION_SLOTS>,
    lifecycle: EventQueue<M, LifecycleEvent, LIFECYCLE_SLOTS>,
    tbtt: EventQueue<M, TbttEvent, TBTT_CAPACITY>,
    received: EventQueue<M, Esp32s31LowerMacEvent<U>, EVENTS>,
    /// Completions owed: admitted attempts whose completion was not taken.
    owed_completions: Mutex<M, Cell<usize>>,
    /// Lifecycle terminals owed.
    owed_lifecycle: Mutex<M, Cell<usize>>,
    /// Raised when the consumer must look again without a new entry: a
    /// poisoning, or a loss an uninstall recorded.
    changed: Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> Queues<M, EVENTS, U> {
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
    fn take(&self) -> Option<Result<Esp32s31LowerMacEvent<U>, EventsLost>> {
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
struct QueueSink<'a, M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> {
    queues: &'a Queues<M, EVENTS, U>,
}

impl<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> LowerMacSink
    for QueueSink<'_, M, EVENTS, U>
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
/// and `U` is the unit of the receive producer, which a received frame
/// holds until the consumer drops it.
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
    U: LowerMacRxUnit,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
    O = Infallible,
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
    queues: Queues<M, EVENTS, U>,
    /// Raised when a deadline, a retune, an install or a fault may have
    /// started, so the runner rearms.
    wake: Signal<M, ()>,
    /// The image's monotonic time the runner's watchdog waits on; the same
    /// time the installed core's transmit owner reads.
    timer: T,
    /// The bodies of admitted attempts until the caller reclaims them, one
    /// entry per attempt; the owed completions bound them.
    #[allow(clippy::type_complexity, reason = "one entry per owed attempt")]
    bodies: Mutex<M, RefCell<[Option<HeldBodies<O, AMPDU_SLOTS>>; COMPLETION_CAPACITY]>>,
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
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
            bodies: Mutex::new(RefCell::new([const { None }; COMPLETION_CAPACITY])),
        }
    }

    /// Hold attempt `id`'s bodies before the attempt is published, so a
    /// completion and reclaim that follow the publication at once find them;
    /// the bodies come back when a slot is free for no attempt, or one of
    /// the same identity already holds bodies.
    #[allow(
        clippy::type_complexity,
        reason = "the refused bodies come back in the shape they were given"
    )]
    fn hold_bodies(
        &self,
        id: TxId,
        mpdu: Option<O>,
        subframes: [Option<O>; AMPDU_SLOTS],
    ) -> Result<(), (Option<O>, [Option<O>; AMPDU_SLOTS])> {
        self.bodies.lock(|bodies| {
            let mut bodies = bodies.borrow_mut();
            if bodies.iter().flatten().any(|held| held.id == id) {
                return Err((mpdu, subframes));
            }
            let Some(free) = bodies.iter_mut().find(|held| held.is_none()) else {
                return Err((mpdu, subframes));
            };
            *free = Some(HeldBodies {
                id,
                mpdu,
                subframes,
            });
            Ok(())
        })
    }

    /// Take back the bodies of attempt `id`, which the backend did not admit.
    fn unhold_bodies(&self, id: TxId) -> Option<HeldBodies<O, AMPDU_SLOTS>> {
        self.bodies.lock(|bodies| {
            bodies
                .borrow_mut()
                .iter_mut()
                .find(|held| held.as_ref().is_some_and(|held| held.id == id))
                .and_then(Option::take)
        })
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
        // The core's attempts went with it: their bodies go back.
        self.bodies.lock(|bodies| {
            for held in bodies.borrow_mut().iter_mut() {
                *held = None;
            }
        });
        self.pending_retune.lock(|pending| pending.set(None));
        installed.map(|installed| (installed.core, installed.hardware))
    }

    /// Run one entry under the lock; a fault poisons the port.
    fn with_core<V>(
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
            &mut QueueSink<'_, M, EVENTS, U>,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<V, Esp32s31LowerMacError> {
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

    /// Run one admission under the lock with `input`, which comes back when
    /// no backend is installed; a fault poisons the port.
    fn admit_with_core<I, V>(
        &self,
        input: I,
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
            I,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<Result<V, I>, Poisoned> {
        if self.poisoned() {
            return Err(Poisoned);
        }
        let result = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let Some(installed) = installed.as_mut() else {
                return Ok(Err(input));
            };
            entry(&mut installed.core, &mut installed.hardware, input).map(Ok)
        });
        result.map_err(|fault| {
            self.fault.lock(|poisoned| poisoned.set(Some(fault)));
            // The consumer learns of it after the queued events; the runner
            // parks.
            self.queues.changed.signal(());
            self.wake.signal(());
            Poisoned
        })
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

    /// One unit of the receive producer, queued as it is while the port
    /// receives and a receive rule or monitor reception admits its MPDU;
    /// a unit the port does not queue is dropped, which returns its buffer.
    /// A unit whose hardware report does not decode is the error.
    pub fn on_received(&self, unit: U) -> Result<(), RxError>
    where
        T: ReceptionClock,
    {
        self.receive(unit, false).map(|_| ())
    }

    /// Received frames that fit in the port's queue now.
    pub fn received_room(&self) -> usize {
        self.queues.received.room()
    }

    /// As [`Self::on_received`], but a unit the port would queue and has
    /// no room for comes back: nothing is lost, and its producer keeps it
    /// until there is room.
    pub fn try_on_received(&self, unit: U) -> Result<Result<(), U>, RxError>
    where
        T: ReceptionClock,
    {
        self.receive(unit, true)
    }

    /// Queue `unit` when a receive rule or monitor reception admits it;
    /// without room, hand it back when `keep`, else drop it as a loss.
    fn receive(&self, unit: U, keep: bool) -> Result<Result<(), U>, RxError>
    where
        T: ReceptionClock,
    {
        let frame = unit.normalized()?;
        // The receive timestamp, in the generation it was taken in; a
        // frame received before the snapshot always has its place unless a
        // break left it without one.
        let stamp = frame
            .stamp
            .and_then(|raw| self.timer.snapshot()?.stamp(raw))
            .map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved);
        let received = self.with_core(|core, hardware, _| {
            Ok(core.received(&*hardware, &frame).map(|(bytes, mut meta)| {
                meta.timestamp = stamp;
                (frame.mpdu_offset..frame.mpdu_offset + bytes.len(), meta)
            }))
        });
        let Ok(Some((mpdu, meta))) = received else {
            return Ok(Ok(()));
        };
        let event = Esp32s31LowerMacEvent::Received {
            frame: Esp32s31RxBuffer { unit, mpdu },
            meta,
        };
        // Queued under the core's lock, as every event is: a port poisoned
        // meanwhile drops the unit instead.
        let mut refused = None;
        let _ = self.with_core(|_, _, sink| {
            if keep {
                refused = sink.queues.received.try_push(event).err();
            } else {
                sink.queues.received.push(event);
            }
            Ok(())
        });
        Ok(match refused {
            Some(Esp32s31LowerMacEvent::Received { frame, .. }) => Err(frame.unit),
            _ => Ok(()),
        })
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
    async fn wait_event(&self) -> Result<Esp32s31LowerMacEvent<U>, EventsLost> {
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
fn owe_completion<M: RawMutex, A, const EVENTS: usize, U: LowerMacRxUnit>(
    queues: &Queues<M, EVENTS, U>,
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    type Event = Esp32s31LowerMacEvent<U>;
    type Error = Esp32s31LowerMacError;
    type TxBuffer = Esp32s31TxBuffer<'slot, BUFFER_SIZE>;
    type TxBody = O;
    type RxBuffer = Esp32s31RxBuffer<U>;

    fn view(event: &Esp32s31LowerMacEvent<U>) -> LowerMacEvent<'_> {
        event.portable()
    }

    fn into_received(
        event: Esp32s31LowerMacEvent<U>,
    ) -> Result<(Esp32s31RxBuffer<U>, RxMeta), Esp32s31LowerMacEvent<U>> {
        match event {
            Esp32s31LowerMacEvent::Received { frame, meta } => Ok((frame, meta)),
            event => Err(event),
        }
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

    /// The body's octets are copied after the header into the lent slot,
    /// which the core sends from; the body waits in the port until it is
    /// reclaimed. The port holds it before the core publishes the attempt,
    /// so a completion and reclaim right after the publication find it.
    fn submit(
        &self,
        mut attempt: MpduAttempt<Esp32s31TxBuffer<'slot, BUFFER_SIZE>, O>,
    ) -> SubmitResult<MpduAttempt<Esp32s31TxBuffer<'slot, BUFFER_SIZE>, O>> {
        let Some(header) = attempt.payload.header_len() else {
            return Ok(Err(Refused {
                error: SubmitError::InvalidLength,
                attempt,
            }));
        };
        let id = attempt.id;
        let body = attempt.payload.body.take();
        if let Some(body) = &body {
            attempt.payload.frame.frame_mut()[header..].copy_from_slice(body.bytes());
        }
        let holds = body.is_some();
        if holds && let Err((body, _)) = self.hold_bodies(id, body, [const { None }; AMPDU_SLOTS]) {
            attempt.payload.body = body;
            return Ok(Err(Refused {
                error: SubmitError::Busy,
                attempt,
            }));
        }
        let attempt: Esp32s31MpduAttempt<'slot, BUFFER_SIZE> =
            attempt.map_payload(|payload| TxPayload {
                frame: payload.frame,
                body: None,
                response: payload.response,
            });
        let queues = &self.queues;
        // A poisoned backend keeps the held body until its reset.
        let admitted = self
            .admit_with_core(attempt, |core, hardware, attempt| {
                owe_completion(queues, attempt, |attempt| core.submit(hardware, attempt))
            })?
            .unwrap_or_else(|attempt| {
                Err(Refused {
                    error: SubmitError::NotInstalled,
                    attempt,
                })
            })
            .map_err(|Refused { error, attempt }| {
                let body = holds
                    .then(|| self.unhold_bodies(id))
                    .flatten()
                    .and_then(|held| held.mpdu);
                Refused {
                    error,
                    attempt: attempt.map_payload(|payload| TxPayload {
                        frame: payload.frame,
                        body,
                        response: payload.response,
                    }),
                }
            });
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
    }

    fn reclaim_tx_bodies(
        &self,
        id: TxId,
        mut each: impl FnMut(usize, O),
    ) -> Result<Result<(), ReclaimError>, Esp32s31LowerMacError> {
        if self.with_core(|core, _, _| Ok(core.running(id)))? {
            return Ok(Err(ReclaimError::Running));
        }
        let held = self.bodies.lock(|bodies| {
            bodies
                .borrow_mut()
                .iter_mut()
                .find(|held| held.as_ref().is_some_and(|held| held.id == id))
                .and_then(Option::take)
        });
        let Some(held) = held else {
            return Ok(Err(ReclaimError::Unknown));
        };
        if let Some(body) = held.mpdu {
            each(0, body);
        }
        for (index, body) in held.subframes.into_iter().enumerate() {
            if let Some(body) = body {
                each(index, body);
            }
        }
        Ok(Ok(()))
    }

    async fn next_event(&self) -> Result<Esp32s31LowerMacEvent<U>, EventsLost> {
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
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

    fn tbtt(event: &Esp32s31LowerMacEvent<U>) -> Option<TbttEvent> {
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBackingSource,
{
    type AmpduBuffer = Esp32s31PortAmpduBuffer<'slot, S, AMPDU_SLOTS, O>;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(AMPDU_SLOTS)
    }

    fn ampdu_buffer(
        &self,
    ) -> Result<Option<Esp32s31PortAmpduBuffer<'slot, S, AMPDU_SLOTS, O>>, Esp32s31LowerMacError>
    {
        let inner = self.with_core(|core, _, _| Ok(core.ampdu_buffer()))?;
        Ok(inner.map(|inner| Esp32s31PortAmpduBuffer {
            inner,
            bodies: [const { None }; AMPDU_SLOTS],
        }))
    }

    /// An aggregate released while no backend is installed or the port is
    /// poisoned returns its subframes, and its aggregate owner is lost until
    /// the radio is reset.
    fn release_ampdu_buffer(&self, buffer: Esp32s31PortAmpduBuffer<'slot, S, AMPDU_SLOTS, O>) {
        let _ = self.with_core(|core, _, _| {
            core.release_ampdu_buffer(buffer.inner);
            Ok(())
        });
    }

    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<Esp32s31PortAmpduBuffer<'slot, S, AMPDU_SLOTS, O>>,
    ) -> SubmitResult<AmpduAttempt<Esp32s31PortAmpduBuffer<'slot, S, AMPDU_SLOTS, O>>> {
        let id = attempt.id;
        let carries = attempt.payload.subframes.bodies.iter().any(Option::is_some);
        let mut attempt = attempt;
        if carries {
            let bodies = core::mem::replace(
                &mut attempt.payload.subframes.bodies,
                [const { None }; AMPDU_SLOTS],
            );
            if let Err((_, bodies)) = self.hold_bodies(id, None, bodies) {
                attempt.payload.subframes.bodies = bodies;
                return Ok(Err(Refused {
                    error: SubmitError::Busy,
                    attempt,
                }));
            }
        }
        let attempt: Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS> =
            attempt.map_payload(|payload| AmpduPayload {
                subframes: payload.subframes.inner,
                tid: payload.tid,
                min_mpdu_start_spacing: payload.min_mpdu_start_spacing,
            });
        let queues = &self.queues;
        // A poisoned backend keeps the held bodies until its reset.
        let admitted = self
            .admit_with_core(attempt, |core, hardware, attempt| {
                owe_completion(queues, attempt, |attempt| {
                    core.submit_ampdu(hardware, attempt)
                })
            })?
            .unwrap_or_else(|attempt| {
                Err(Refused {
                    error: SubmitError::NotInstalled,
                    attempt,
                })
            })
            .map_err(|Refused { error, attempt }| {
                let bodies = carries
                    .then(|| self.unhold_bodies(id))
                    .flatten()
                    .map_or([const { None }; AMPDU_SLOTS], |held| held.subframes);
                Refused {
                    error,
                    attempt: attempt.map_payload(|payload| AmpduPayload {
                        subframes: Esp32s31PortAmpduBuffer {
                            inner: payload.subframes,
                            bodies,
                        },
                        tid: payload.tid,
                        min_mpdu_start_spacing: payload.min_mpdu_start_spacing,
                    }),
                }
            });
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
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
    U: LowerMacRxUnit,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O: TxBody,
> PortRxSink<U>
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
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn received_room(&self) -> usize {
        Esp32s31LowerMac::received_room(self)
    }

    fn try_on_received(&self, unit: U) -> Result<Result<(), U>, RxError> {
        Esp32s31LowerMac::try_on_received(self, unit)
    }
}

mod rx_publisher;

pub use rx_publisher::{Esp32s31PortRxPublisher, PortRxSink};

#[cfg(all(test, not(target_pointer_width = "32")))]
mod tests;
