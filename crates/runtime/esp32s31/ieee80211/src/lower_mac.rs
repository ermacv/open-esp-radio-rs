//! The ESP32-S31 Wi-Fi backend on the owner shape of the lower-MAC port
//! (`oer_ieee80211_lower_mac::owner`, `oer-ieee80211-port`), with the
//! product event model (#563).
//!
//! [`Esp32s31LowerMac`] is the runtime a composition places in a `static`:
//! the sans-IO [`LowerMacCore`] of `oer-esp32s31-ieee80211` and its register
//! owner, the event queues and the product RX pipeline's two ends
//! ([`PortRxReceiver`], [`PortRxService`]: the staging queue and the RX DMA
//! epoch that fills it). [`Esp32s31LowerMac::install`] installs a disabled
//! core with its register owner, its retune, the RX pipeline, the
//! interrupt runtimes and the install's channel and access-point address,
//! and returns the [`Esp32s31Backend`], which the composition hands to
//! [`run_port`](oer_ieee80211_port::run_port) with the port's body.
//!
//! The interrupts only post: the MAC and power interrupt handlers publish
//! into [`IrqConsumer`] and [`PowerIrqConsumer`], and the
//! backend's driver ([`PortBackend::drive`]), joined with the body, serves
//! them in the vendor's order: RX success first, as budgeted drains of the
//! RX DMA epoch into the staging queue while the RX interrupt stays masked,
//! unmasked once the frontier is drained (a full staging queue waits for
//! the event consumer to return a credit); then the coalesced TX
//! completion edges of the core, the station TBTT, and the publication
//! watchdog. A submission publishes at once, with no interrupt round trip.
//! The driver gives every ended attempt's permits back before the event
//! consumer hands its completion out.
//!
//! The event consumer reads the core's completions and TBTTs in the order
//! they occurred, then the staged units the core's receive rules admit, as
//! received events whose buffer is the staged DMA unit itself.
//!
//! `Enable` is one bounded step of the owner: it enables the core and,
//! after a channel change, retunes the PHY through the [`LowerMacRetune`]
//! it holds, under the shared radio's lease ([`Esp32s31PortRetune`]); a
//! refused retune leaves the port disabled ([`RetuneRefused`]).
//!
//! The receive Block Ack slots are one pool both roles share, as the
//! hardware's banks are (owner decision, Claude Code chat with common,
//! 2026-10-12). Beside the base contract the backend serves A-MPDU
//! aggregates, the station's beacon timing and monitor reception.

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
    future::{Future, poll_fn, ready},
    marker::PhantomData,
    ops::Range,
    task::{Poll, Waker},
};
use oer_ieee80211_mac::ccmp::GroupKeyId;

use embassy_futures::select::{Either3, Either4, select, select3, select4};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_esp32s31_ieee80211::{
    lower_mac::{
        AmpduBacking, AmpduBackingSource, CheckedKeyScope, Esp32s31AmpduBuffer, Esp32s31TxBuffer,
        LifecycleStart, LowerMacCore, LowerMacFault, LowerMacHardware, LowerMacSink, MpduValues,
        NoAmpdu, esp32s31_lower_mac_capabilities,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
    tx::WifiTxWake,
};
use oer_esp32s31_ieee80211_mac::{
    rx::{
        NormalizedRxFrame, RxError, RxIngressConfig, pool::NetworkRxFrame, view_normalized_rx_frame,
    },
    tx::TxPhyRate,
};
use oer_ieee80211_lower_mac::{
    AccessPoint, AccessPointClient, AckHeader, AmpduClient, AmpduRate, AttemptKey, Backoff,
    BeaconTimingClient, BssColor, CancelError, Ccmp128Key, ClockError, EventsLost,
    Ieee80211Capabilities, Ieee80211Instant, Ieee80211Radio, InterfaceRole, KeyHandle,
    LifecycleEvent, LowerMacBackend, LowerMacCapabilities, LowerMacClient, LowerMacControl,
    LowerMacEvent, LowerMacEvents, LowerMacSetting, LowerMacSetup, MacAddress, MonitorControl,
    PhaseAmpdu, PhaseAmpduBuffer, PhaseAttempt, Poisoned, PortResult, ReceiveFilter,
    RxBeaconPriority, RxBlockAckAgreement, RxBuffer, RxEvidence, RxMeta, SizedFrame, Station,
    StationClient, StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, TbttEvent, TbttPlan, TbttSchedule, Tid, TsfSample, TxBody, TxBuffer,
    TxCompletion, TxFrame, TxFrameBody, TxGated, TxId, TxPower, TxSeats, VifConfig, VifId, VifRole,
    VifTsf,
};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_port::{
    EnableResult, Lent, PhaseParts, PortBackend, RadioPortClient, RadioPortEvents, RadioWork,
    Retire, Work,
};
use oer_ieee80211_runtime::lower_mac_owner::{
    BufferPermit, QueuePermit, TxBufferKind, TxPermits, TxQueueKind,
};
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{Mint, Permit, PermitPool, Seat, Slot};
use oer_time::Duration;

use crate::mac_clock::ReceptionClock;
use oer_esp32s31_ieee80211_mac::irq::consumer::{
    IrqConsumer, Moderated, PowerIrqConsumer, RxDelivery,
};
use oer_ieee80211_mac::channel::WifiChannel;

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

/// Attempt completions the port holds at most: one per queue, twice over
/// for completions the consumer has not taken yet.
pub const COMPLETION_CAPACITY: usize = 8;
/// Lifecycle terminals the core owes the owner's transitions at most.
const LIFECYCLE_CAPACITY: usize = 4;
/// TBTT events waiting for the consumer, a loss marker included.
pub const TBTT_CAPACITY: usize = 2;
/// Transmit queues of the ESP32-S31: the four ordinary EDCA queues.
pub const TX_QUEUES: usize = 4;
/// Pairwise key slots of the station: its access point's.
pub const STATION_PAIRWISE_KEYS: usize = 1;
/// Group key slots of the station.
pub const STATION_GROUP_KEYS: usize = 1;
/// Pairwise key slots of the access point: one per association
/// identifier, which a slot's index plus one is.
pub const ACCESS_POINT_PAIRWISE_KEYS: usize = 15;
/// Group key slots of the access point.
pub const ACCESS_POINT_GROUP_KEYS: usize = 1;
/// Receive Block Ack banks, which both roles share.
pub const RX_BLOCK_ACK_SLOTS: usize = 8;
/// A BlockAckReq without its FCS: Frame Control, Duration, RA, TA, BAR
/// Control and Starting Sequence Control.
const BLOCK_ACK_REQUEST_LEN: usize = 20;
/// How long a driver asked to retire may take to return.
const RETIRE_BOUND: Duration = Duration::from_millis(10);

const COMPLETION_SLOTS: usize = COMPLETION_CAPACITY + 1;
const LIFECYCLE_SLOTS: usize = LIFECYCLE_CAPACITY + 1;

/// The interface of a role: the station's is zero, the access point's one.
const fn vif_of(role: VifRole) -> VifId {
    match role {
        VifRole::Station => VifId(0),
        VifRole::AccessPoint => VifId(1),
    }
}

/// The PHY retune of an `Enable` after a channel change, over the port's
/// register owner `H`. The ESP32-S31 retunes through
/// `switch_published_esp32s31_wifi_channel` under the shared-radio lease
/// ([`Esp32s31PortRetune`]).
pub trait LowerMacRetune<H> {
    /// Tune to `channel` through `hardware`; `false` when the PHY refused.
    fn retune(&mut self, hardware: &mut H, channel: WifiChannel) -> impl Future<Output = bool>;
}

/// The radio refused the retune of an `Enable`: the port stays disabled on
/// the channel it was tuned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetuneRefused;

/// One unit of the receive producer: an owned receive buffer whose MPDU and
/// hardware metadata the port reads in place. Dropping it returns the
/// buffer to its producer.
pub trait LowerMacRxUnit {
    /// The whole receive buffer, the hardware's prefix included.
    fn buffer(&self) -> &[u8];
    /// The MPDU and metadata the hardware reported for the unit.
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError>;
}

/// The receiving end of the product RX pipeline: the staging queue the RX
/// DMA epoch ([`PortRxService`]) fills, which the event consumer reads.
pub trait PortRxReceiver {
    /// One received unit.
    type Unit: LowerMacRxUnit;
    /// The pipeline's producing end.
    type Service: PortRxService;

    /// The next staged unit, if any.
    fn try_receive(&self) -> Option<Self::Unit>;

    /// The staged units waiting.
    fn len(&self) -> usize;

    /// Whether no staged unit waits.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The producing end of the product RX pipeline: the RX DMA epoch, which
/// the backend's driver serves after an RX interrupt.
pub trait PortRxService {
    /// Serve one budgeted RX frontier into the staging queue.
    fn service(&mut self) -> impl Future<Output = RxPass>;
}

/// How one budgeted RX pass ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPass {
    /// The completed frontier is drained: the interrupt is unmasked.
    Drained,
    /// The budget ran out with more pending: the interrupt stays masked and
    /// the driver serves again after yielding.
    More,
    /// The staging queue is full: the interrupt stays masked until the
    /// event consumer takes a unit and returns its credit.
    Blocked,
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

/// One owned event of the port. An attempt's completion owns the body of
/// its attempt.
#[derive(Debug)]
pub enum Esp32s31LowerMacEvent<U, O, const SUBFRAMES: usize> {
    /// A received MPDU.
    Received {
        frame: Esp32s31RxBuffer<U>,
        meta: oer_esp32s31_ieee80211::lower_mac::Esp32s31RxMeta,
    },
    TxCompleted(TxCompletion, Option<Esp32s31TxBodies<O, SUBFRAMES>>),
    /// A station TBTT.
    Tbtt(TbttEvent),
}

/// The bodies of one attempt: an MPDU's, or an aggregate's by subframe.
/// The port holds them from the attempt's admission; its completion event
/// carries them back.
#[derive(Debug)]
pub struct Esp32s31TxBodies<O, const SUBFRAMES: usize> {
    mpdu: Option<O>,
    subframes: [Option<O>; SUBFRAMES],
}

impl<O, const SUBFRAMES: usize> Esp32s31TxBodies<O, SUBFRAMES> {
    const NONE: Self = Self {
        mpdu: None,
        subframes: [const { None }; SUBFRAMES],
    };
}

impl<O, const SUBFRAMES: usize> IntoIterator for Esp32s31TxBodies<O, SUBFRAMES> {
    type Item = (usize, O);
    type IntoIter = core::iter::Chain<
        core::option::IntoIter<(usize, O)>,
        core::iter::FilterMap<
            core::iter::Enumerate<core::array::IntoIter<Option<O>, SUBFRAMES>>,
            fn((usize, Option<O>)) -> Option<(usize, O)>,
        >,
    >;

    /// The bodies with their subframe index (0 for an MPDU's).
    fn into_iter(self) -> Self::IntoIter {
        let subframe: fn((usize, Option<O>)) -> Option<(usize, O)> =
            |(index, body)| body.map(|body| (index, body));
        self.mpdu
            .map(|body| (0, body))
            .into_iter()
            .chain(self.subframes.into_iter().enumerate().filter_map(subframe))
    }
}

/// A bounded queue whose overflow is reported in its order: a loss marker
/// takes the place of the first dropped entry. Each entry keeps the
/// occurrence number it was queued with, so the port merges its queues in
/// the order the events occurred.
struct EventQueue<M: RawMutex, T, const N: usize> {
    entries: Channel<M, Result<T, EventsLost>, N>,
    /// The occurrence of each queued entry, in the same order.
    occurrences: Channel<M, u64, N>,
    /// The occurrence of the first dropped entry whose marker is not queued
    /// yet.
    lost: Mutex<M, Cell<Option<u64>>>,
}

impl<M: RawMutex, T, const N: usize> EventQueue<M, T, N> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            occurrences: Channel::new(),
            lost: Mutex::new(Cell::new(None)),
        }
    }

    /// Queue the marker of a pending loss, if there is room for it.
    fn queue_marker(&self, lost: &Cell<Option<u64>>) -> bool {
        let Some(first) = lost.get() else {
            return true;
        };
        if self.entries.try_send(Err(EventsLost)).is_err() {
            return false;
        }
        let _ = self.occurrences.try_send(first);
        lost.set(None);
        true
    }

    /// Queue `entry`, which occurred as `occurrence`, behind a pending loss
    /// marker; drop it and remember the loss when there is no room.
    fn push(&self, occurrence: u64, entry: T) {
        self.lost.lock(|lost| {
            if !self.queue_marker(lost) {
                return;
            }
            if self.entries.try_send(Ok(entry)).is_err() {
                lost.set(Some(occurrence));
            } else {
                let _ = self.occurrences.try_send(occurrence);
            }
        });
    }

    /// The occurrence of the entry [`Self::take`] would return next.
    fn head(&self) -> Option<u64> {
        self.lost
            .lock(|lost| self.occurrences.try_peek().ok().or(lost.get()))
    }

    /// The next entry, or the loss marker after the last entry.
    fn take(&self) -> Option<Result<T, EventsLost>> {
        self.lost.lock(|lost| match self.entries.try_receive() {
            Ok(entry) => {
                let _ = self.occurrences.try_receive();
                Some(entry)
            }
            Err(_) if lost.take().is_some() => Some(Err(EventsLost)),
            Err(_) => None,
        })
    }

    /// Discard every entry and a pending loss: the stream of the port that
    /// left ends here.
    fn discard(&self) {
        self.lost.lock(|lost| {
            while let Ok(_entry) = self.entries.try_receive() {}
            while let Ok(_occurrence) = self.occurrences.try_receive() {}
            lost.set(None);
        });
    }
}

/// An event the port queues; a completion takes the body of its attempt
/// only as the consumer takes it, so the queues hold no body.
#[derive(Debug)]
enum QueuedEvent {
    TxCompleted(TxCompletion),
    Tbtt(TbttEvent),
    /// The units an RX pass staged, which the consumer takes from the
    /// staging queue in this place of the stream.
    Received(usize),
}

/// RX passes the stream holds the places of at once.
const RX_BATCH_SLOTS: usize = 16;

/// The queues of the port, one per kind of event, and the lifecycle
/// terminals the owner's transitions read.
struct Queues<M: RawMutex> {
    completions: EventQueue<M, TxCompletion, COMPLETION_SLOTS>,
    lifecycle: EventQueue<M, LifecycleEvent, LIFECYCLE_SLOTS>,
    tbtt: EventQueue<M, TbttEvent, TBTT_CAPACITY>,
    /// The places of the RX passes' staged units in the stream.
    received: EventQueue<M, usize, RX_BATCH_SLOTS>,
    /// The occurrence number of the next event, one sequence for every
    /// queue: the stream is merged in this order. Its lock also orders the
    /// queues: a producer numbers and queues an event, and the consumer
    /// picks and takes the earliest head, each under it.
    next_occurrence: Mutex<M, Cell<u64>>,
    /// Raised when the consumer must look again without a new entry: a
    /// poisoning.
    changed: Signal<M, ()>,
}

impl<M: RawMutex> Queues<M> {
    const fn new() -> Self {
        Self {
            completions: EventQueue::new(),
            lifecycle: EventQueue::new(),
            tbtt: EventQueue::new(),
            received: EventQueue::new(),
            next_occurrence: Mutex::new(Cell::new(0)),
            changed: Signal::new(),
        }
    }

    /// Queue an event happening now through `queue`, which receives its
    /// occurrence number, under the order lock.
    fn ordered<R>(&self, queue: impl FnOnce(u64) -> R) -> R {
        self.next_occurrence.lock(|next| {
            let occurrence = next.get();
            next.set(occurrence.wrapping_add(1));
            queue(occurrence)
        })
    }

    /// The next queued event of the consumer's stream, in the order the
    /// events occurred.
    fn take(&self) -> Option<Result<QueuedEvent, EventsLost>> {
        self.next_occurrence.lock(|_| {
            let heads = [
                self.completions.head(),
                self.tbtt.head(),
                self.received.head(),
            ];
            let (first, _) = heads
                .iter()
                .enumerate()
                .filter_map(|(queue, head)| Some((queue, (*head)?)))
                .min_by_key(|(_, occurrence)| *occurrence)?;
            match first {
                0 => Some(self.completions.take()?.map(QueuedEvent::TxCompleted)),
                1 => Some(self.tbtt.take()?.map(QueuedEvent::Tbtt)),
                _ => Some(self.received.take()?.map(QueuedEvent::Received)),
            }
        })
    }

    /// Wait until a queue of the stream may have an entry or
    /// [`Self::changed`] rose.
    async fn wait(&self) {
        select4(
            self.completions.entries.ready_to_receive(),
            self.tbtt.entries.ready_to_receive(),
            self.received.entries.ready_to_receive(),
            self.changed.wait(),
        )
        .await;
    }

    /// End the stream of the port that left.
    fn discard(&self) {
        self.completions.discard();
        self.lifecycle.discard();
        self.tbtt.discard();
        self.received.discard();
    }
}

/// The installed core and its register owner.
struct Installed<
    'slot,
    P,
    E,
    T,
    H,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    S: AmpduBacking,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> {
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    /// `None` while lent to the retune of an `Enable`.
    hardware: Option<H>,
    /// The access point's address, which the install gave.
    access_point: MacAddress,
}

/// The attempts whose completion the core reported and whose permits the
/// driver has not given back yet.
type Ended = [Option<TxId>; COMPLETION_CAPACITY];

/// The sink of one locked entry: events go to their queues, and every
/// completion's attempt to the ended set.
struct QueueSink<'a, M: RawMutex> {
    queues: &'a Queues<M>,
    ended: &'a Mutex<M, RefCell<Ended>>,
}

impl<M: RawMutex> LowerMacSink for QueueSink<'_, M> {
    fn tx_completed(&mut self, completion: TxCompletion) {
        let queues = self.queues;
        self.ended.lock(|ended| {
            if let Some(free) = ended.borrow_mut().iter_mut().find(|id| id.is_none()) {
                *free = Some(completion.id);
            }
        });
        queues.ordered(|occurrence| queues.completions.push(occurrence, completion));
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        let queues = self.queues;
        queues.ordered(|occurrence| queues.lifecycle.push(occurrence, event));
    }

    fn tbtt(&mut self, event: TbttEvent) {
        let queues = self.queues;
        queues.ordered(|occurrence| queues.tbtt.push(occurrence, event));
    }
}

/// The wakers of the futures that wait for the port to settle: a closing
/// gate, a key removal, an interface configuration.
const SETTLE_WAITERS: usize = 4;

/// The ESP32-S31 lower MAC's runtime: the installed core, its queues and
/// its interrupt and receive entries. `TX_BUFFERS` counts the transmit
/// buffers the core lends, `EVENTS` bounds the received frames waiting for
/// the consumer (a loss marker included), `U` is the unit of the receive
/// producer, which a received frame holds until the consumer drops it, and
/// `O` is the body an attempt carries.
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
    U: PortRxReceiver,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
    O = Infallible,
> {
    #[expect(
        clippy::type_complexity,
        reason = "the mutex retains the installed core's concrete hardware owner types"
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
                    BUFFER_SIZE,
                    TX_BUFFERS,
                    S,
                    AMPDU_SLOTS,
                    AMPDU_BUFFERS,
                >,
            >,
        >,
    >,
    retune: Mutex<M, RefCell<Option<R>>>,
    fault: Mutex<M, Cell<Option<LowerMacFault>>>,
    queues: Queues<M>,
    /// The staging queue's receiving end, while a backend is installed.
    receiver: Mutex<M, RefCell<Option<U>>>,
    /// The RX DMA epoch while no driver serves it: the driver takes it for
    /// its life and puts it back when it retires.
    rx_service: Mutex<M, RefCell<Option<U::Service>>>,
    /// Raised after every RX service, so the event consumer looks at the
    /// staging queue again.
    rx_ready: Signal<M, ()>,
    /// Raised when the event consumer took a staged unit, returning a
    /// credit to a blocked RX pass.
    rx_capacity: Signal<M, ()>,
    /// Staged units whose place in the stream is queued: the units of the
    /// queued RX passes and of the pass the consumer takes.
    rx_placed: Mutex<M, Cell<usize>>,
    /// Units the consumer still takes from the staging queue before the
    /// stream's next queued event.
    rx_batch: Mutex<M, Cell<usize>>,
    ended: Mutex<M, RefCell<Ended>>,
    /// Raised when a deadline, an install, a completion or a fault may have
    /// started, so the driver rearms.
    wake: Signal<M, ()>,
    settle: Mutex<M, RefCell<embassy_sync::waitqueue::MultiWakerRegistration<SETTLE_WAITERS>>>,
    /// The image's monotonic time the driver's watchdog waits on; the same
    /// time the installed core's transmit owner reads, and the MAC clock.
    timer: T,
    /// The bodies of admitted attempts until their completion events carry
    /// them.
    #[expect(
        clippy::type_complexity,
        reason = "bounded attempt storage pairs each admitted transmit ID with its owned body"
    )]
    bodies:
        Mutex<M, RefCell<[Option<(TxId, Esp32s31TxBodies<O, AMPDU_SLOTS>)>; COMPLETION_CAPACITY]>>,
    next_id: Mutex<M, Cell<u32>>,
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

impl<
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
    U: PortRxReceiver,
    S: AmpduBacking,
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
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// An empty runtime whose driver waits on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            installed: Mutex::new(RefCell::new(None)),
            retune: Mutex::new(RefCell::new(None)),
            fault: Mutex::new(Cell::new(None)),
            queues: Queues::new(),
            receiver: Mutex::new(RefCell::new(None)),
            rx_service: Mutex::new(RefCell::new(None)),
            rx_ready: Signal::new(),
            rx_capacity: Signal::new(),
            rx_placed: Mutex::new(Cell::new(0)),
            rx_batch: Mutex::new(Cell::new(0)),
            ended: Mutex::new(RefCell::new([None; COMPLETION_CAPACITY])),
            wake: Signal::new(),
            settle: Mutex::new(RefCell::new(
                embassy_sync::waitqueue::MultiWakerRegistration::new(),
            )),
            bodies: Mutex::new(RefCell::new([const { None }; COMPLETION_CAPACITY])),
            next_id: Mutex::new(Cell::new(0)),
        }
    }

    /// Install a disabled core with its register owner and retune, on
    /// `channel`, its access-point interface at `access_point`, and return
    /// the backend, whose stream starts empty.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a backend is already installed.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install<'r>(
        &'r self,
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
        rx: (U, U::Service),
        irq: &'r IrqConsumer<Moderated>,
        power: &'r PowerIrqConsumer<Moderated>,
        channel: SupportedChannel<Esp32s31Caps<BUFFER_SIZE>>,
        access_point: IndividualAddress,
    ) -> Result<
        Esp32s31Backend<
            'r,
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
        >,
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
            mut core,
            hardware,
            retune,
        } = parts;
        let refused = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Some((core, hardware));
            }
            // A supported channel on a disabled core is always taken.
            let _ = core.set_channel(channel.channel());
            *installed = Some(Installed {
                core,
                hardware: Some(hardware),
                access_point: access_point.get(),
            });
            None
        });
        if let Some((core, hardware)) = refused {
            return Err(Esp32s31LowerMacParts {
                core,
                hardware,
                retune,
            });
        }
        let (receiver, rx_service) = rx;
        self.retune.lock(|slot| *slot.borrow_mut() = Some(retune));
        self.receiver
            .lock(|slot| *slot.borrow_mut() = Some(receiver));
        self.rx_service
            .lock(|slot| *slot.borrow_mut() = Some(rx_service));
        self.fault.lock(|fault| fault.set(None));
        self.wake.signal(());
        Ok(Esp32s31Backend {
            runtime: self,
            irq,
            power,
        })
    }

    /// Take the core and its register owner back and discard queued events;
    /// `None`, and nothing changed, unless the core is disabled and idle
    /// with its registers home.
    #[allow(
        clippy::type_complexity,
        reason = "the uninstalled owners are returned as they were installed"
    )]
    fn uninstall(
        &self,
    ) -> Option<(
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
        H,
        U,
        U::Service,
    )> {
        let (core, hardware) = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let stopped = installed.as_ref()?;
            if !stopped.core.idle_disabled() || stopped.hardware.is_none() {
                return None;
            }
            installed
                .take()
                .and_then(|installed| Some((installed.core, installed.hardware?)))
        })?;
        self.queues.discard();
        self.bodies.lock(|bodies| {
            for held in bodies.borrow_mut().iter_mut() {
                *held = None;
            }
        });
        self.ended
            .lock(|ended| *ended.borrow_mut() = [None; COMPLETION_CAPACITY]);
        self.rx_placed.lock(|placed| placed.set(0));
        self.rx_batch.lock(|batch| batch.set(0));
        let receiver = self.receiver.lock(|slot| slot.borrow_mut().take())?;
        let rx_service = self.rx_service.lock(|slot| slot.borrow_mut().take())?;
        Some((core, hardware, receiver, rx_service))
    }

    /// Run one entry over the installed core and its registers under the
    /// lock; `None` without a core or while a retune holds the registers. A
    /// fault poisons the port.
    #[allow(
        clippy::type_complexity,
        reason = "the installed owners keep their types"
    )]
    fn with_installed<V>(
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
            MacAddress,
            &mut QueueSink<'_, M>,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<Option<V>, Poisoned<LowerMacFault>> {
        if let Some(cause) = self.fault.lock(Cell::get) {
            return Err(Poisoned { cause });
        }
        let result = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let Some(installed) = installed.as_mut() else {
                return Ok(None);
            };
            let Some(hardware) = installed.hardware.as_mut() else {
                return Ok(None);
            };
            let mut sink = QueueSink {
                queues: &self.queues,
                ended: &self.ended,
            };
            entry(
                &mut installed.core,
                hardware,
                installed.access_point,
                &mut sink,
            )
            .map(Some)
        });
        let answer = result.map_err(|cause| self.poison(cause));
        self.settled();
        answer
    }

    /// As [`Self::with_installed`], for an entry of a running phase: the
    /// core is installed and its registers home.
    fn with_phase<V>(
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
            MacAddress,
            &mut QueueSink<'_, M>,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<V, Poisoned<LowerMacFault>> {
        self.with_installed(entry)
            .map(|answer| answer.expect("a phase runs over the installed core"))
    }

    /// Record `cause`: the consumer learns of it after the queued events,
    /// and the driver parks.
    fn poison(&self, cause: LowerMacFault) -> Poisoned<LowerMacFault> {
        self.fault.lock(|poisoned| poisoned.set(Some(cause)));
        self.queues.changed.signal(());
        self.wake.signal(());
        Poisoned { cause }
    }

    fn poisoned(&self) -> Option<Poisoned<LowerMacFault>> {
        self.fault.lock(Cell::get).map(|cause| Poisoned { cause })
    }

    /// Wake every future waiting for the port to settle.
    fn settled(&self) {
        self.settle.lock(|settle| settle.borrow_mut().wake());
    }

    /// Wait for the port to settle again with `context`'s waker.
    fn wait_settled(&self, waker: &Waker) {
        self.settle
            .lock(|settle| settle.borrow_mut().register(waker));
    }

    /// The received event of `unit`, when the core's receive rules admit
    /// its MPDU: its metadata, its receive timestamp in the MAC clock's
    /// generation, and where the MPDU lies in the unit's buffer.
    fn admit(&self, unit: U::Unit) -> Option<Esp32s31LowerMacEvent<U::Unit, O, AMPDU_SLOTS>> {
        let (mpdu, meta) = {
            let frame = unit.normalized().ok()?;
            let stamp = frame
                .stamp
                .and_then(|raw| self.timer.snapshot()?.stamp(raw))
                .map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved);
            self.with_installed(|core, hardware, _, _| {
                Ok(core.received(&*hardware, &frame).map(|(bytes, meta)| {
                    (
                        frame.mpdu_offset..frame.mpdu_offset + bytes.len(),
                        meta.with_timestamp(stamp),
                    )
                }))
            })
            .ok()
            .flatten()
            .flatten()?
        };
        Some(Esp32s31LowerMacEvent::Received {
            frame: Esp32s31RxBuffer { unit, mpdu },
            meta,
        })
    }

    /// Hold attempt `id`'s bodies until its completion carries them.
    fn hold_bodies(&self, id: TxId, held: Esp32s31TxBodies<O, AMPDU_SLOTS>) {
        self.bodies.lock(|bodies| {
            let mut bodies = bodies.borrow_mut();
            let free = bodies
                .iter_mut()
                .find(|held| held.is_none())
                .expect("bodies for each queue's attempt in flight");
            *free = Some((id, held));
        });
    }

    /// Hold attempt `id`'s one body until its completion carries it.
    fn hold_body(&self, id: TxId, body: O) {
        self.hold_bodies(
            id,
            Esp32s31TxBodies {
                mpdu: Some(body),
                subframes: [const { None }; AMPDU_SLOTS],
            },
        );
    }

    /// Take back the bodies of attempt `id`, whose completion is taken.
    fn unhold_body(&self, id: TxId) -> Option<Esp32s31TxBodies<O, AMPDU_SLOTS>> {
        self.bodies.lock(|bodies| {
            bodies
                .borrow_mut()
                .iter_mut()
                .find(|held| held.as_ref().is_some_and(|held| held.0 == id))
                .and_then(Option::take)
                .map(|held| held.1)
        })
    }

    /// A fresh attempt identity.
    fn next_id(&self) -> TxId {
        self.next_id.lock(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            TxId(id)
        })
    }

    /// The attempts the core ended since the last call: their permits go
    /// back before their completions are handed out.
    fn take_ended(&self) -> Ended {
        self.ended
            .lock(|ended| core::mem::replace(&mut *ended.borrow_mut(), [None; COMPLETION_CAPACITY]))
    }

    /// The radio clock: the MAC local time, the counter of receive
    /// timestamps, affine to the monotonic clock.
    fn now(&self) -> PortResult<Ieee80211Instant, ClockError, LowerMacFault> {
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        Ok(self
            .timer
            .snapshot()
            .map(|snapshot| snapshot.sample().source)
            .ok_or(ClockError::Unavailable))
    }

    /// The next lifecycle terminal the core reported.
    async fn lifecycle_event(&self) -> Result<LifecycleEvent, Poisoned<LowerMacFault>> {
        loop {
            if let Some(Ok(event)) = self.queues.lifecycle.take() {
                return Ok(event);
            }
            if let Some(poisoned) = self.poisoned() {
                return Err(poisoned);
            }
            select(
                self.queues.lifecycle.entries.ready_to_receive(),
                self.queues.changed.wait(),
            )
            .await;
        }
    }
}

/// Serve the RX frontier until it is drained or blocked: a budgeted pass
/// while the interrupt stays masked, the unmask once drained.
async fn serve_rx<V: PortRxService, R: RxDelivery>(
    rx: &mut Option<V>,
    ready: &impl RxReady,
    irq: &IrqConsumer<R>,
) {
    loop {
        let pass = match rx.as_mut() {
            Some(rx) => rx.service().await,
            None => RxPass::Drained,
        };
        ready.rx_ready();
        match pass {
            RxPass::Drained => {
                irq.unmask_rx_after_drain();
                return;
            }
            RxPass::Blocked => return,
            RxPass::More => embassy_futures::yield_now().await,
        }
    }
}

/// What a served RX pass tells the event consumer.
trait RxReady {
    fn rx_ready(&self);
}

/// The product RX pipeline's receiving end: the staging queue's receiver,
/// with the ingress configuration of the ring its units come from, and the
/// type of the epoch that fills it.
pub struct Esp32s31StagedReceiver<
    'queue,
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    V,
> {
    receiver:
        crate::datapath::rx::staging::StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
    config: RxIngressConfig,
    service: PhantomData<fn() -> V>,
}

impl<'queue, 'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize, V>
    Esp32s31StagedReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS, V>
{
    pub const fn new(
        receiver: crate::datapath::rx::staging::StagedRxReceiver<
            'queue,
            'pool,
            M,
            DEPTH,
            CAPACITY,
            SLOTS,
        >,
        config: RxIngressConfig,
    ) -> Self {
        Self {
            receiver,
            config,
            service: PhantomData,
        }
    }
}

impl<'queue, 'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize, V>
    PortRxReceiver for Esp32s31StagedReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS, V>
where
    V: PortRxService,
{
    type Unit = Esp32s31StagedRx<'pool, SLOTS, CAPACITY>;
    type Service = V;

    fn try_receive(&self) -> Option<Self::Unit> {
        self.receiver
            .try_receive()
            .ok()
            .map(|frame| Esp32s31StagedRx::new(frame, self.config))
    }

    fn len(&self) -> usize {
        self.receiver.len()
    }
}

/// The product RX pipeline's producing end: the RX DMA epoch and the
/// register handle it serves the ring through. The handle reaches the
/// registers one serialized transaction at a time, beside the core's.
pub struct Esp32s31RxDmaService<E, X> {
    pub epoch: E,
    pub hardware: X,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    X: oer_esp32s31_ieee80211_mac::rx::RxDma,
> PortRxService
    for Esp32s31RxDmaService<
        crate::datapath::rx::dma::StagedRxEpoch<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        X,
    >
where
    D: oer_time::Timer,
{
    async fn service(&mut self) -> RxPass {
        use oer_esp32s31_ieee80211::datapath::DatapathRxProgress;
        match self.epoch.service(&mut self.hardware).await {
            Ok(DatapathRxProgress::BudgetExhausted | DatapathRxProgress::ProbePending) => {
                RxPass::More
            }
            Ok(
                DatapathRxProgress::StageCapacityBlocked
                | DatapathRxProgress::NetworkBackpressured
                | DatapathRxProgress::UpperLayerBlockedButDroppable,
            ) => RxPass::Blocked,
            // A drained frontier, and a ring error the next interrupt meets
            // again.
            Ok(_) | Err(_) => RxPass::Drained,
        }
    }
}

/// A value a driver holds for its life: it goes home when the driver
/// retires or is dropped.
struct Retained<V, F: FnMut(V)> {
    value: Option<V>,
    home: F,
}

impl<V, F: FnMut(V)> Drop for Retained<V, F> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            (self.home)(value);
        }
    }
}

/// The capabilities marker of an ESP32-S31 backend with transmit slots of
/// `BUFFER_SIZE` bytes.
pub struct Esp32s31Caps<const BUFFER_SIZE: usize>;

impl<const BUFFER_SIZE: usize> Ieee80211Capabilities for Esp32s31Caps<BUFFER_SIZE> {
    const CAPS: LowerMacCapabilities = esp32s31_lower_mac_capabilities(BUFFER_SIZE);
}

/// The transmit permits of one install: a pool per queue, each served
/// first in, first out, and the buffers' pool.
pub type Esp32s31TxPermits<'id, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> = TxPermits<
    'id,
    Esp32s31Caps<BUFFER_SIZE>,
    FifoPermitPool<'id, TxQueueKind, 1, { oer_ieee80211_lower_mac::TX_SEATS }>,
    FifoPermitPool<'id, TxBufferKind, TX_BUFFERS, { oer_ieee80211_lower_mac::TX_SEATS }>,
    TX_QUEUES,
>;

/// The kind of a role's pairwise key slots.
pub struct Esp32s31PairwiseKeys<R>(PhantomData<fn() -> R>);
/// The kind of a role's group key slots.
pub struct Esp32s31GroupKeys<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots, which both roles share.
pub struct Esp32s31BlockAcks;

/// One receive Block Ack agreement and its slot.
struct Agreement<'id> {
    role: VifRole,
    peer: IndividualAddress,
    tid: Tid,
    slot: Slot<'id, Esp32s31BlockAcks>,
}

/// One attempt in flight: its role and the key it uses, which a key
/// removal and an interface configuration wait for.
#[derive(Clone, Copy)]
struct InFlight {
    id: TxId,
    role: VifRole,
    key: Option<KeyHandle>,
}

/// The backend's held part of one install: its transmit permits, the
/// attempts in flight and the receive Block Ack agreements.
pub struct Esp32s31Held<
    'id,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_BUFFERS: usize,
> {
    tx: Esp32s31TxPermits<'id, BUFFER_SIZE, TX_BUFFERS>,
    /// The aggregate buffers' pool and the permits of the aggregates in
    /// flight.
    ampdu: FifoPermitPool<'id, Esp32s31AmpduKind, AMPDU_BUFFERS, AMPDU_SEATS>,
    ampdu_in_flight: RefCell<[Option<(TxId, Permit<'id, Esp32s31AmpduKind>)>; TX_QUEUES]>,
    in_flight: RefCell<[Option<InFlight>; TX_QUEUES]>,
    agreements: RefCell<[Option<Agreement<'id>>; RX_BLOCK_ACK_SLOTS]>,
}

/// The permit kind of the backend's aggregate buffers.
pub enum Esp32s31AmpduKind {}

/// The seats of the aggregate buffers' line: one per interface role.
pub const AMPDU_SEATS: usize = 2;

impl<const BUFFER_SIZE: usize, const TX_BUFFERS: usize, const AMPDU_BUFFERS: usize>
    Esp32s31Held<'_, BUFFER_SIZE, TX_BUFFERS, AMPDU_BUFFERS>
{
    /// Give back the permits of every attempt in `ended`.
    fn complete(&self, ended: Ended) {
        for id in ended.into_iter().flatten() {
            self.tx.complete(id);
            let permit = self
                .ampdu_in_flight
                .borrow_mut()
                .iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|(held, _)| *held == id))
                .and_then(Option::take);
            if let Some((_, permit)) = permit {
                self.ampdu.release(permit);
            }
            for entry in self.in_flight.borrow_mut().iter_mut() {
                if entry.is_some_and(|in_flight| in_flight.id == id) {
                    *entry = None;
                }
            }
        }
    }
}

/// The transmit seats of one interface.
pub type Esp32s31TxSeats<'id> = TxSeats<'id, TxQueueKind, TxBufferKind>;

/// The port-lifetime resources the body of one install receives: each
/// role's transmit seats and key slots, and the receive Block Ack slots
/// both roles share.
pub struct Esp32s31Resources<'id> {
    pub station_tx: Esp32s31TxSeats<'id>,
    pub access_point_tx: Esp32s31TxSeats<'id>,
    pub station_pairwise_keys: [Slot<'id, Esp32s31PairwiseKeys<Station>>; STATION_PAIRWISE_KEYS],
    pub station_group_keys: [Slot<'id, Esp32s31GroupKeys<Station>>; STATION_GROUP_KEYS],
    pub access_point_pairwise_keys:
        [Slot<'id, Esp32s31PairwiseKeys<AccessPoint>>; ACCESS_POINT_PAIRWISE_KEYS],
    pub access_point_group_keys:
        [Slot<'id, Esp32s31GroupKeys<AccessPoint>>; ACCESS_POINT_GROUP_KEYS],
    pub block_acks: [Slot<'id, Esp32s31BlockAcks>; RX_BLOCK_ACK_SLOTS],
    /// Each role's seat on the aggregate buffers' line.
    pub station_ampdu: Seat<'id, Esp32s31AmpduKind>,
    pub access_point_ampdu: Seat<'id, Esp32s31AmpduKind>,
}

/// The ESP32-S31 backend of one install; only
/// [`run_port`](oer_ieee80211_port::run_port) holds it.
pub struct Esp32s31Backend<
    'r,
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
    U: PortRxReceiver,
    S: AmpduBacking,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O,
> {
    runtime: &'r Esp32s31LowerMac<
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
    >,
    /// The MAC interrupt's posts: RX first, then TX events.
    irq: &'r IrqConsumer<Moderated>,
    /// The power interrupt's posts: the station TBTT.
    power: &'r PowerIrqConsumer<Moderated>,
}

/// The backend's disabled-only settings.
pub struct Esp32s31Setup<'a, L> {
    runtime: &'a L,
}

/// The backend's phase control.
pub struct Esp32s31Control<'a, L> {
    runtime: &'a L,
}

/// The backend's closed transmit gate; dropping it opens the gate and
/// publishes the attempts it held.
pub struct Esp32s31Gated<'g, L: GateOwner> {
    runtime: &'g L,
}

/// What a closed gate opens again through.
pub trait GateOwner {
    #[doc(hidden)]
    fn set_gate(&self, open: bool) -> bool;
}

impl<L: GateOwner> TxGated for Esp32s31Gated<'_, L> {}

/// A gate closing: dropped before it yields its guard, it opens the gate
/// again and publishes what it held.
struct GateClosing<'g, L: GateOwner> {
    runtime: &'g L,
}

impl<L: GateOwner> Drop for GateClosing<'_, L> {
    fn drop(&mut self) {
        self.runtime.set_gate(true);
    }
}

impl<L: GateOwner> Drop for Esp32s31Gated<'_, L> {
    fn drop(&mut self) {
        self.runtime.set_gate(true);
    }
}

/// The backend's event consumer: the driver gave each attempt's permits
/// back before the consumer hands its completion out.
pub struct Esp32s31Events<
    'a,
    'id,
    L,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_BUFFERS: usize,
> {
    runtime: &'a L,
    held: &'a Esp32s31Held<'id, BUFFER_SIZE, TX_BUFFERS, AMPDU_BUFFERS>,
}

/// The backend's client of the interface of role `Role`.
pub struct Esp32s31Client<
    'a,
    'id,
    L,
    Role,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_BUFFERS: usize,
> {
    runtime: &'a L,
    held: &'a Esp32s31Held<'id, BUFFER_SIZE, TX_BUFFERS, AMPDU_BUFFERS>,
    role: PhantomData<fn() -> Role>,
}

/// The backend's clients: one per interface role.
pub struct Esp32s31Clients<
    'a,
    'id,
    L,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_BUFFERS: usize,
> {
    pub station: Esp32s31Client<'a, 'id, L, Station, BUFFER_SIZE, TX_BUFFERS, AMPDU_BUFFERS>,
    pub access_point:
        Esp32s31Client<'a, 'id, L, AccessPoint, BUFFER_SIZE, TX_BUFFERS, AMPDU_BUFFERS>,
}

/// The token of one admitted attempt.
pub struct Esp32s31Token(TxId);

impl RadioWork for Esp32s31Token {
    type Id = TxId;

    fn id(&self) -> TxId {
        self.0
    }
}

/// One frame's buffer, bound to its buffer permit: the header the request
/// wrote, then the body the caller writes.
pub struct Esp32s31PhaseBuffer<
    'c,
    'id,
    'slot,
    L: SlotOwner<'slot, BUFFER_SIZE>,
    const BUFFER_SIZE: usize,
> {
    buffer: Option<Esp32s31TxBuffer<'slot, BUFFER_SIZE>>,
    header: usize,
    no_ack: bool,
    rate: TxPhyRate,
    /// `None` once a submission moved it in with the attempt.
    permit: Option<BufferPermit<'c, 'id>>,
    runtime: &'c L,
}

/// What lends and takes back the core's transmit slots.
pub trait SlotOwner<'slot, const BUFFER_SIZE: usize> {
    #[doc(hidden)]
    fn release_slot(&self, buffer: Esp32s31TxBuffer<'slot, BUFFER_SIZE>);
}

impl<'slot, L: SlotOwner<'slot, BUFFER_SIZE>, const BUFFER_SIZE: usize> TxFrameBody
    for Esp32s31PhaseBuffer<'_, '_, 'slot, L, BUFFER_SIZE>
{
    fn body_mut(&mut self) -> &mut [u8] {
        let header = self.header;
        &mut self
            .buffer
            .as_mut()
            .expect("a lent buffer holds its slot")
            .frame_mut()[header..]
    }
}

impl<'slot, L: SlotOwner<'slot, BUFFER_SIZE>, const BUFFER_SIZE: usize> Drop
    for Esp32s31PhaseBuffer<'_, '_, 'slot, L, BUFFER_SIZE>
{
    fn drop(&mut self) {
        // A buffer dropped before its submission gives its slot back; its
        // permit goes back with it.
        if let Some(buffer) = self.buffer.take() {
            self.runtime.release_slot(buffer);
        }
    }
}

/// An installed pairwise key of a client of role `Role`; it holds its slot.
pub struct Esp32s31PairwiseKey<'id, Role> {
    handle: Option<KeyHandle>,
    slot: Slot<'id, Esp32s31PairwiseKeys<Role>>,
}

/// An installed group key of a client of role `Role`; it holds its slot.
pub struct Esp32s31GroupKey<'id, Role> {
    handle: Option<KeyHandle>,
    slot: Slot<'id, Esp32s31GroupKeys<Role>>,
}

/// The installed runtime type of one backend.
type Runtime<
    'slot,
    M,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U,
    S,
    const AS: usize,
    const AB: usize,
    O,
> = Esp32s31LowerMac<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>;

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> RxReady for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// Place the units the pass staged in the stream, after every event
    /// queued before it.
    fn rx_ready(&self) {
        let staged = self
            .receiver
            .lock(|receiver| receiver.borrow().as_ref().map_or(0, PortRxReceiver::len));
        self.rx_placed.lock(|placed| {
            let new = staged.saturating_sub(placed.get());
            if new > 0 {
                let queues = &self.queues;
                queues.ordered(|occurrence| queues.received.push(occurrence, new));
                placed.set(placed.get() + new);
            }
        });
        self.rx_ready.signal(());
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
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> SlotOwner<'slot, B> for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    fn release_slot(&self, buffer: Esp32s31TxBuffer<'slot, B>) {
        let _ = self.with_installed(|core, _, _, _| {
            core.release_tx_buffer(buffer);
            Ok(())
        });
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
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> GateOwner for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// Open or close the core's transmit gate; `false` when the core keeps
    /// it open because an attempt is published.
    fn set_gate(&self, open: bool) -> bool {
        let set = self.with_installed(|core, hardware, _, _| {
            Ok(core
                .apply(hardware, LowerMacSetting::TxGate { open })
                .map(|setting| setting.is_ok())
                .unwrap_or(false))
        });
        // Opening the gate publishes a held attempt: its watchdog starts.
        self.wake.signal(());
        matches!(set, Ok(Some(true)))
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
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> LowerMacSetup for Esp32s31Setup<'_, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;

    fn set_channel(&mut self, channel: SupportedChannel<Esp32s31Caps<B>>) {
        // A supported channel on a disabled core is always taken: the next
        // `Enable` retunes.
        let _ = self.runtime.with_installed(|core, _, _, _| {
            let _ = core.set_channel(channel.channel());
            Ok(())
        });
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
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> LowerMacControl
    for Esp32s31Control<'_, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
    type Gated<'g>
        = Esp32s31Gated<'g, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
    where
        Self: 'g;

    /// Admission closes at once: the core holds every attempt admitted from
    /// now on unpublished, and the gate closes once the published ones
    /// ended, so the wait ends under any traffic. A future dropped before
    /// it yields the guard opens the gate again.
    fn close_gate(&mut self) -> impl Future<Output = Self::Gated<'_>> {
        let runtime = self.runtime;
        let _ = runtime.with_installed(|core, _, _, _| {
            core.hold_new_attempts();
            Ok(())
        });
        let mut closing = Some(GateClosing { runtime });
        poll_fn(move |context| {
            let blocked = runtime.with_installed(|core, hardware, _, _| {
                Ok(core
                    .published_idle()
                    .then(|| core.block_held_queues(hardware)))
            });
            if let Ok(Some(Some(()))) = blocked {
                // The guard keeps the gate closed from here.
                core::mem::forget(closing.take());
                return Poll::Ready(Esp32s31Gated { runtime });
            }
            runtime.wait_settled(context.waker());
            Poll::Pending
        })
    }

    fn set_edca(&mut self, parameters: SupportedEdca<Esp32s31Caps<B>>) {
        let _ = self.runtime.with_installed(|core, _, _, _| {
            core.set_checked_edca(parameters.get());
            Ok(())
        });
    }

    fn set_rx_beacon_priority(&mut self, priority: RxBeaconPriority) {
        let _ = self.runtime.with_installed(|core, hardware, _, _| {
            let _ = core.apply(hardware, LowerMacSetting::RxBeaconPriority(priority));
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> RadioPortEvents
    for Esp32s31Events<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Event = Esp32s31LowerMacEvent<U::Unit, O, AS>;
    type Fault = LowerMacFault;

    async fn next_event(
        &mut self,
    ) -> PortResult<Esp32s31LowerMacEvent<U::Unit, O, AS>, EventsLost, LowerMacFault> {
        let runtime = self.runtime;
        loop {
            // The permits of every ended attempt are back before its
            // completion is handed out.
            self.held.complete(runtime.take_ended());
            // The units of the RX pass in progress come before the stream's
            // next queued event: one stream in the order the events
            // occurred (#266 D8). A unit no receive rule admits is dropped,
            // which returns its buffer.
            while runtime.rx_batch.lock(Cell::get) > 0 {
                runtime.rx_batch.lock(|batch| batch.set(batch.get() - 1));
                runtime
                    .rx_placed
                    .lock(|placed| placed.set(placed.get().saturating_sub(1)));
                let unit = runtime.receiver.lock(|receiver| {
                    receiver
                        .borrow()
                        .as_ref()
                        .and_then(PortRxReceiver::try_receive)
                });
                runtime.rx_capacity.signal(());
                if let Some(event) = unit.and_then(|unit| runtime.admit(unit)) {
                    return Ok(Ok(event));
                }
            }
            if let Some(event) = runtime.queues.take() {
                match event {
                    Ok(QueuedEvent::Received(units)) => {
                        runtime.rx_batch.lock(|batch| batch.set(units));
                        continue;
                    }
                    Ok(QueuedEvent::TxCompleted(completion)) => {
                        return Ok(Ok(Esp32s31LowerMacEvent::TxCompleted(
                            completion,
                            runtime.unhold_body(completion.id),
                        )));
                    }
                    Ok(QueuedEvent::Tbtt(event)) => {
                        return Ok(Ok(Esp32s31LowerMacEvent::Tbtt(event)));
                    }
                    Err(EventsLost) => return Ok(Err(EventsLost)),
                }
            }
            if let Some(poisoned) = runtime.poisoned() {
                return Err(poisoned);
            }
            runtime.queues.wait().await;
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> LowerMacEvents
    for Esp32s31Events<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type RxBuffer = Esp32s31RxBuffer<U::Unit>;
    type TxBodies = Esp32s31TxBodies<O, AS>;

    fn view(event: &Esp32s31LowerMacEvent<U::Unit, O, AS>) -> LowerMacEvent<'_> {
        match event {
            Esp32s31LowerMacEvent::Received { frame, meta } => LowerMacEvent::Received {
                frame: frame.bytes(),
                meta: *oer_ieee80211_lower_mac::RxMetadata::common(meta),
            },
            Esp32s31LowerMacEvent::TxCompleted(completion, _) => {
                LowerMacEvent::TxCompleted(*completion)
            }
            Esp32s31LowerMacEvent::Tbtt(event) => LowerMacEvent::Extension {
                kind: oer_ieee80211_lower_mac::ExtensionKind::Tbtt,
                vif: event.tbtt.vif,
            },
        }
    }

    fn into_received(
        event: Esp32s31LowerMacEvent<U::Unit, O, AS>,
    ) -> Result<(Esp32s31RxBuffer<U::Unit>, RxMeta), Esp32s31LowerMacEvent<U::Unit, O, AS>> {
        match event {
            Esp32s31LowerMacEvent::Received { frame, meta } => Ok((frame, meta)),
            event => Err(event),
        }
    }

    fn into_completed(
        event: Esp32s31LowerMacEvent<U::Unit, O, AS>,
    ) -> Result<(TxCompletion, Esp32s31TxBodies<O, AS>), Esp32s31LowerMacEvent<U::Unit, O, AS>>
    {
        match event {
            Esp32s31LowerMacEvent::TxCompleted(completion, bodies) => {
                Ok((completion, bodies.unwrap_or(Esp32s31TxBodies::NONE)))
            }
            event => Err(event),
        }
    }

    fn tbtt(event: &Esp32s31LowerMacEvent<U::Unit, O, AS>) -> Option<TbttEvent> {
        match event {
            Esp32s31LowerMacEvent::Tbtt(event) => Some(*event),
            _ => None,
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> RadioPortClient
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Role,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Token = Esp32s31Token;
    type Domain = Ieee80211Radio;
    type Fault = LowerMacFault;

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee80211Instant, ClockError, LowerMacFault>> + '_ {
        ready(self.runtime.now())
    }

    fn end(
        &self,
        token: Esp32s31Token,
    ) -> impl Future<Output = PortResult<(), CancelError, LowerMacFault>> + '_ {
        ready(
            self.runtime
                .with_phase(|core, _, _, sink| Ok(core.cancel(token.0, sink))),
        )
    }
}

/// The role a client's type names, with what its keys install as.
pub trait Esp32s31Role: InterfaceRole {
    #[doc(hidden)]
    fn pairwise_scope(peer: IndividualAddress, slot: u8) -> CheckedKeyScope;
    #[doc(hidden)]
    fn group_scope(key_id: GroupKeyId) -> Result<CheckedKeyScope, LowerMacFault>;
}

impl Esp32s31Role for Station {
    fn pairwise_scope(peer: IndividualAddress, _: u8) -> CheckedKeyScope {
        CheckedKeyScope::StationPairwise { peer: peer.get() }
    }

    fn group_scope(key_id: GroupKeyId) -> Result<CheckedKeyScope, LowerMacFault> {
        Ok(CheckedKeyScope::StationGroup { key_id })
    }
}

impl Esp32s31Role for AccessPoint {
    /// An access point's pairwise slot is its association identifier
    /// ([owner decision on #581](https://github.com/ermacv/open-esp-radio-rs/issues/581#issuecomment-6123003983)):
    /// the slot's index plus one.
    fn pairwise_scope(peer: IndividualAddress, slot: u8) -> CheckedKeyScope {
        CheckedKeyScope::AccessPointPairwise {
            peer: peer.get(),
            association_id: u16::from(slot) + 1,
        }
    }

    fn group_scope(key_id: GroupKeyId) -> Result<CheckedKeyScope, LowerMacFault> {
        oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId::new(key_id.get())
            .map(|key_id| CheckedKeyScope::AccessPointGroup { key_id })
            .ok_or(LowerMacFault::Key(
                oer_esp32s31_ieee80211_mac::crypto::CryptoKeyError::InvalidGroupKeyId,
            ))
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role: Esp32s31Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
>
    Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Role,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// Ready once no attempt in flight matches `uses`: an attempt the
    /// closed gate holds is cancelled, a published one is waited for.
    async fn settle(&self, uses: impl Fn(&InFlight) -> bool) {
        poll_fn(|context| {
            let runtime = self.runtime;
            self.held.complete(runtime.take_ended());
            let in_flight = *self.held.in_flight.borrow();
            let held: [Option<TxId>; TX_QUEUES] =
                in_flight.map(|entry| entry.filter(&uses).map(|in_flight| in_flight.id));
            if held.iter().all(Option::is_none) {
                return Poll::Ready(());
            }
            for id in held.into_iter().flatten() {
                // An unpublished attempt ends at once; a published one ends
                // on the air.
                let _ = runtime.with_phase(|core, _, _, sink| Ok(core.cancel(id, sink)));
            }
            runtime.wait_settled(context.waker());
            Poll::Pending
        })
        .await;
    }

    /// Remove an installed key once no attempt in flight uses it.
    async fn remove_key(&self, handle: Option<KeyHandle>) {
        let Some(handle) = handle else {
            return;
        };
        self.settle(|in_flight| in_flight.key == Some(handle)).await;
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.remove_checked_key(hardware, handle);
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role: Esp32s31Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> LowerMacClient<'id>
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Role,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
    type QueueKind = TxQueueKind;
    type BufferKind = TxBufferKind;
    type Queue<'c>
        = QueuePermit<'c, 'id>
    where
        Self: 'c,
        'id: 'c;
    type TxBuffer<'c>
        = Esp32s31PhaseBuffer<
        'c,
        'id,
        'slot,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        B,
    >
    where
        Self: 'c,
        'id: 'c;
    type TxBody = O;
    type PairwiseKeyKind = Esp32s31PairwiseKeys<Role>;
    type GroupKeyKind = Esp32s31GroupKeys<Role>;
    type PairwiseKey = Esp32s31PairwiseKey<'id, Role>;
    type GroupKey = Esp32s31GroupKey<'id, Role>;
    type BlockAckKind = Esp32s31BlockAcks;

    fn channel(&self) -> oer_ieee80211_lower_mac::Channel {
        self.runtime
            .with_phase(|core, _, _, _| Ok(core.channel()))
            .unwrap_or_else(|_| panic!("a poisoned phase reads no channel"))
    }

    fn address(&self) -> IndividualAddress {
        let address = self
            .runtime
            .with_phase(|core, _, access_point, _| {
                Ok(match Role::ROLE {
                    VifRole::Station => core.station_address(),
                    VifRole::AccessPoint => access_point,
                })
            })
            .unwrap_or_else(|_| panic!("a poisoned phase reads no address"));
        IndividualAddress::new(address).expect("an interface's address is individual")
    }

    fn queue<'c, 's>(
        &'c self,
        seat: &'s mut Seat<'id, TxQueueKind>,
    ) -> impl Future<Output = QueuePermit<'c, 'id>> + 's
    where
        'id: 'c,
        'c: 's,
    {
        self.held.tx.queue(seat)
    }

    async fn tx_buffer<'c, 's>(
        &'c self,
        frame: TxFrame<'c, Esp32s31Caps<B>>,
        seat: &'s mut Seat<'id, TxBufferKind>,
    ) -> Self::TxBuffer<'c>
    where
        'id: 'c,
        'c: 's,
    {
        let permit = self.held.tx.buffer(seat).await;
        let runtime = self.runtime;
        let len = match &frame {
            TxFrame::Individual { frame, .. } => frame.len().get(),
            TxFrame::IndividualNoAck { frame, .. } => frame.len().get(),
            TxFrame::Group { frame, .. } => frame.len().get(),
            TxFrame::BlockAckRequest { .. } => BLOCK_ACK_REQUEST_LEN,
        };
        // A buffer permit stands for one of the core's slots.
        let mut buffer = runtime
            .with_phase(|core, _, _, _| Ok(core.tx_buffer(len)))
            .ok()
            .flatten()
            .expect("a buffer permit stands for a free slot");
        let bytes = buffer.frame_mut();
        let (header, no_ack, rate) = match frame {
            TxFrame::Individual { frame, rate } => (
                frame.header().header().encode(bytes, false),
                false,
                rate.get(),
            ),
            TxFrame::IndividualNoAck { frame, rate } => (
                frame.header().header().encode(bytes, true),
                true,
                rate.rate().get(),
            ),
            // A group frame solicits no response: its QoS Ack Policy is No
            // Ack, and the core sends it without one.
            TxFrame::Group { frame, rate } => {
                (frame.header().encode(bytes, true), false, rate.get())
            }
            TxFrame::BlockAckRequest {
                receiver,
                transmitter,
                tid,
                starting_sequence,
                rate,
            } => {
                let request = oer_ieee80211_mac::block_ack::encode_block_ack_request(
                    receiver.get(),
                    transmitter,
                    tid.get(),
                    starting_sequence,
                );
                bytes[..request.len()].copy_from_slice(&request);
                (request.len(), false, rate.rate().get())
            }
        };
        Esp32s31PhaseBuffer {
            buffer: Some(buffer),
            header,
            no_ack,
            // A phase rate is one the capabilities carry on the channel.
            rate: TxPhyRate::try_from(rate).unwrap_or_else(|_| panic!("a phase rate converts")),
            permit: Some(permit),
            runtime,
        }
    }

    fn submit<'p>(
        &'p self,
        attempt: PhaseAttempt<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<LowerMacFault>>
    where
        'id: 'p,
    {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let PhaseAttempt {
            queue,
            mut frame,
            body,
            protection,
            key,
            power,
            backoff,
            coex,
        } = attempt;
        let key = key.and_then(|key| match key {
            AttemptKey::Pairwise(key) => key.handle,
            AttemptKey::Group(key) => key.handle,
        });
        let id = runtime.next_id();
        let mut buffer = frame.buffer.take().expect("a lent buffer holds its slot");
        if let Some(body) = &body {
            // The body ends the MPDU: its octets follow what the caller
            // wrote.
            let len = buffer.len();
            let bytes = body.bytes();
            buffer.frame_mut()[len - bytes.len()..].copy_from_slice(bytes);
        }
        if let Some(body) = body {
            runtime.hold_body(id, body);
        }
        let values = MpduValues {
            id,
            vif: vif_of(Role::ROLE),
            access_category: oer_ieee80211_lower_mac::QueueGrant::access_category(&queue),
            frame: buffer,
            no_ack: frame.no_ack,
            rate: frame.rate,
            protection,
            key,
            power_ceiling_dbm: match power.get() {
                TxPower::Calibrated => None,
                TxPower::MaxDbm(dbm) => Some(dbm),
            },
            backoff_slots: match backoff.get() {
                Backoff::Slots(slots) => slots,
                Backoff::HardwareDraw { .. } => {
                    unreachable!("the ESP32-S31 supports caller-drawn backoff only")
                }
            },
            coex: coex.get(),
        };
        runtime.with_phase(|core, hardware, _, _| core.submit_checked(hardware, values))?;
        // A publication starts its watchdog.
        runtime.wake.signal(());
        let in_flight = InFlight {
            id,
            role: Role::ROLE,
            key,
        };
        if let Some(free) = self
            .held
            .in_flight
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.is_none())
        {
            *free = Some(in_flight);
        }
        // The permit goes in with the attempt; the buffer, its slot taken,
        // drops without giving it back.
        let permit = frame.permit.take().expect("a lent buffer holds its permit");
        drop(frame);
        self.held.tx.admit(id, queue, permit);
        Ok(Work::new(self, Esp32s31Token(id)))
    }

    fn install_pairwise_key(
        &self,
        slot: Slot<'id, Esp32s31PairwiseKeys<Role>>,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> Esp32s31PairwiseKey<'id, Role> {
        let scope = Role::pairwise_scope(peer, slot.index());
        let handle = self
            .runtime
            .with_phase(|core, hardware, _, _| {
                core.install_checked_key(hardware, vif_of(Role::ROLE), scope, &key.0)
            })
            .ok();
        Esp32s31PairwiseKey { handle, slot }
    }

    fn install_group_key(
        &self,
        slot: Slot<'id, Esp32s31GroupKeys<Role>>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> Esp32s31GroupKey<'id, Role> {
        let scope = Role::group_scope(key_id);
        let handle = self
            .runtime
            .with_phase(|core, hardware, _, _| {
                core.install_checked_key(hardware, vif_of(Role::ROLE), scope?, &key.0)
            })
            .ok();
        Esp32s31GroupKey { handle, slot }
    }

    async fn remove_pairwise_key(
        &self,
        key: Esp32s31PairwiseKey<'id, Role>,
    ) -> Slot<'id, Esp32s31PairwiseKeys<Role>> {
        self.remove_key(key.handle).await;
        key.slot
    }

    async fn remove_group_key(
        &self,
        key: Esp32s31GroupKey<'id, Role>,
    ) -> Slot<'id, Esp32s31GroupKeys<Role>> {
        self.remove_key(key.handle).await;
        key.slot
    }

    fn add_block_ack(
        &self,
        slot: Slot<'id, Esp32s31BlockAcks>,
        agreement: SupportedBlockAck<Esp32s31Caps<B>>,
    ) -> Option<Slot<'id, Esp32s31BlockAcks>> {
        let (peer, tid, start_sequence, window) = agreement.get();
        // The core replaces an agreement of the same pair in its bank.
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.start_checked_rx_block_ack(
                hardware,
                RxBlockAckAgreement {
                    vif: vif_of(Role::ROLE),
                    peer: peer.get(),
                    tid: tid.get(),
                    start_sequence,
                    window,
                },
            )
        });
        let mut agreements = self.held.agreements.borrow_mut();
        let replaced = agreements
            .iter_mut()
            .find(|entry| {
                entry.as_ref().is_some_and(|agreement| {
                    (agreement.role, agreement.peer, agreement.tid) == (Role::ROLE, peer, tid)
                })
            })
            .and_then(Option::take)
            .map(|agreement| agreement.slot);
        let free = agreements
            .iter_mut()
            .find(|entry| entry.is_none())
            .expect("an agreement slot stands for a free entry");
        *free = Some(Agreement {
            role: Role::ROLE,
            peer,
            tid,
            slot,
        });
        replaced
    }

    fn remove_block_ack(
        &self,
        peer: IndividualAddress,
        tid: Tid,
    ) -> Option<Slot<'id, Esp32s31BlockAcks>> {
        let slot = self
            .held
            .agreements
            .borrow_mut()
            .iter_mut()
            .find(|entry| {
                entry.as_ref().is_some_and(|agreement| {
                    (agreement.role, agreement.peer, agreement.tid) == (Role::ROLE, peer, tid)
                })
            })
            .and_then(Option::take)
            .map(|agreement| agreement.slot)?;
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.end_checked_rx_block_ack(hardware, vif_of(Role::ROLE), peer.get(), tid.get())
        });
        Some(slot)
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> StationClient<'id>
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Station,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    async fn configure(&self, receive: StationReceive<Esp32s31Caps<B>>) {
        self.settle(|in_flight| in_flight.role == VifRole::Station)
            .await;
        let (bssid, receive) = match receive {
            StationReceive::Off => (None, ReceiveFilter::NONE),
            StationReceive::OtherBssManagement => (None, ReceiveFilter::OTHER_BSS_MANAGEMENT),
            StationReceive::Bss { bssid, filter } => (Some(bssid.get()), filter.get()),
        };
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            let address = core.station_address();
            core.configure_checked_vif(
                hardware,
                vif_of(VifRole::Station),
                VifConfig {
                    address,
                    role: VifRole::Station,
                    bssid,
                    receive,
                },
            );
            Ok(())
        });
    }

    fn set_bss_color(&self, color: BssColor) {
        let _ = self.runtime.with_phase(|core, _, _, _| {
            core.set_checked_he_bss_color(color.get());
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> AccessPointClient<'id>
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        AccessPoint,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    async fn configure(&self, filter: SupportedFilter<Esp32s31Caps<B>, AccessPoint>) {
        self.settle(|in_flight| in_flight.role == VifRole::AccessPoint)
            .await;
        let _ = self.runtime.with_phase(|core, hardware, address, _| {
            core.configure_checked_vif(
                hardware,
                vif_of(VifRole::AccessPoint),
                VifConfig {
                    address,
                    role: VifRole::AccessPoint,
                    bssid: Some(address),
                    receive: filter.get(),
                },
            );
            Ok(())
        });
    }
}

/// What lends and takes back the core's aggregate buffers.
pub trait AggregateOwner<'slot, S: AmpduBacking, const AMPDU_SLOTS: usize> {
    #[doc(hidden)]
    fn release_aggregate(&self, buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>);
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> AggregateOwner<'slot, S, AS> for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    fn release_aggregate(&self, buffer: Esp32s31AmpduBuffer<'slot, S, AS>) {
        let _ = self.with_installed(|core, _, _, _| {
            core.release_ampdu_buffer(buffer);
            Ok(())
        });
    }
}

/// One aggregate a client lends: the core's aggregate buffer, the bodies
/// its subframes end with, its TID and rate, and its permit.
pub struct Esp32s31PhaseAmpdu<
    'c,
    'id,
    'slot,
    L: AggregateOwner<'slot, S, AMPDU_SLOTS>,
    S: AmpduBacking,
    const AMPDU_SLOTS: usize,
    const BUFFER_SIZE: usize,
    O,
    Pool: oer_radio_port::PermitPool<'id, Esp32s31AmpduKind>,
> {
    /// `None` once a submission moved it in.
    inner: Option<Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>>,
    bodies: [Option<O>; AMPDU_SLOTS],
    tid: Tid,
    rate: oer_ieee80211_lower_mac::PhyRate,
    /// `None` once a submission moved it in.
    permit: Option<Permit<'id, Esp32s31AmpduKind>>,
    pool: &'c Pool,
    runtime: &'c L,
    caps: PhantomData<fn() -> Esp32s31Caps<BUFFER_SIZE>>,
}

impl<'id, 'slot, L, S, const AMPDU_SLOTS: usize, const BUFFER_SIZE: usize, O, Pool> PhaseAmpduBuffer
    for Esp32s31PhaseAmpdu<'_, 'id, 'slot, L, S, AMPDU_SLOTS, BUFFER_SIZE, O, Pool>
where
    Pool: oer_radio_port::PermitPool<'id, Esp32s31AmpduKind>,
    L: AggregateOwner<'slot, S, AMPDU_SLOTS>,
    S: AmpduBackingSource,
    O: TxBody,
{
    type Body = O;
    type Caps = Esp32s31Caps<BUFFER_SIZE>;

    fn push(
        &mut self,
        frame: SizedFrame<AckHeader, Esp32s31Caps<BUFFER_SIZE>>,
        body: Option<O>,
    ) -> Result<&mut [u8], (SizedFrame<AckHeader, Esp32s31Caps<BUFFER_SIZE>>, Option<O>)> {
        let inner = self
            .inner
            .as_mut()
            .expect("a lent aggregate holds its buffer");
        let index = oer_ieee80211_lower_mac::AmpduBuffer::subframes(inner);
        let len = frame.len().get();
        let body_len = body.as_ref().map_or(0, |body| body.bytes().len());
        let Some(header_room) = len.checked_sub(body_len) else {
            return Err((frame, body));
        };
        let Ok(mpdu) = oer_ieee80211_lower_mac::AmpduBuffer::push_mpdu(inner, len, None) else {
            return Err((frame, body));
        };
        let header = frame.header().header().encode(mpdu, false);
        if let Some(body) = &body {
            mpdu[header_room..].copy_from_slice(body.bytes());
        }
        if let Some(slot) = self.bodies.get_mut(index) {
            *slot = body;
        }
        Ok(&mut mpdu[header..header_room])
    }

    fn subframes(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, oer_ieee80211_lower_mac::AmpduBuffer::subframes)
    }
}

impl<'id, 'slot, L, S, const AMPDU_SLOTS: usize, const BUFFER_SIZE: usize, O, Pool> Drop
    for Esp32s31PhaseAmpdu<'_, 'id, 'slot, L, S, AMPDU_SLOTS, BUFFER_SIZE, O, Pool>
where
    Pool: oer_radio_port::PermitPool<'id, Esp32s31AmpduKind>,
    L: AggregateOwner<'slot, S, AMPDU_SLOTS>,
    S: AmpduBacking,
{
    fn drop(&mut self) {
        // An aggregate dropped before its submission gives its buffer and
        // permit back; its bodies drop with it.
        if let Some(inner) = self.inner.take() {
            self.runtime.release_aggregate(inner);
        }
        if let Some(permit) = self.permit.take() {
            self.pool.release(permit);
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
    Role: Esp32s31Role,
> AmpduClient<'id>
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Role,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
    S: AmpduBackingSource,
{
    type AmpduKind = Esp32s31AmpduKind;
    type AmpduBuffer<'c>
        = Esp32s31PhaseAmpdu<
        'c,
        'id,
        'slot,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        S,
        AS,
        B,
        O,
        FifoPermitPool<'id, Esp32s31AmpduKind, AB, AMPDU_SEATS>,
    >
    where
        Self: 'c,
        'id: 'c;

    fn ampdu_capabilities(&self) -> oer_ieee80211_lower_mac::AmpduCapabilities {
        oer_esp32s31_ieee80211::lower_mac::esp32s31_ampdu_capabilities(AS)
    }

    async fn ampdu_buffer<'c, 's>(
        &'c self,
        tid: Tid,
        rate: AmpduRate<'c>,
        seat: &'s mut Seat<'id, Esp32s31AmpduKind>,
    ) -> Self::AmpduBuffer<'c>
    where
        'id: 'c,
        'c: 's,
    {
        let held = self.held;
        let permit = held.ampdu.take(seat).await;
        // An aggregate permit stands for one of the core's idle aggregate
        // owners.
        let inner = self
            .runtime
            .with_phase(|core, _, _, _| Ok(core.ampdu_buffer()))
            .ok()
            .flatten()
            .expect("an aggregate permit stands for an idle aggregate owner");
        Esp32s31PhaseAmpdu {
            inner: Some(inner),
            bodies: [const { None }; AS],
            tid,
            rate: rate.rate().get(),
            permit: Some(permit),
            pool: &held.ampdu,
            runtime: self.runtime,
            caps: PhantomData,
        }
    }

    fn submit_ampdu<'p>(
        &'p self,
        attempt: PhaseAmpdu<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<LowerMacFault>>
    where
        'id: 'p,
    {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let PhaseAmpdu {
            queue,
            mut aggregate,
            protection,
            key,
            power,
            backoff,
            coex,
            min_mpdu_start_spacing,
        } = attempt;
        let key = key.and_then(|key| key.handle);
        let id = runtime.next_id();
        let buffer = aggregate
            .inner
            .take()
            .expect("a lent aggregate holds its buffer");
        let bodies = core::mem::replace(&mut aggregate.bodies, [const { None }; AS]);
        runtime.hold_bodies(
            id,
            Esp32s31TxBodies {
                mpdu: None,
                subframes: bodies,
            },
        );
        let attempt = oer_ieee80211_lower_mac::TxAttempt {
            id,
            vif: vif_of(Role::ROLE),
            access_category: oer_ieee80211_lower_mac::QueueGrant::access_category(&queue),
            payload: oer_ieee80211_lower_mac::AmpduPayload {
                subframes: buffer,
                tid: aggregate.tid.get(),
                min_mpdu_start_spacing: min_mpdu_start_spacing.get(),
            },
            rate: aggregate.rate,
            protection,
            key: key.map_or(
                oer_ieee80211_lower_mac::KeySelector::Plaintext,
                oer_ieee80211_lower_mac::KeySelector::Key,
            ),
            power: power.get(),
            backoff: backoff.get(),
            coex: coex.get(),
        };
        // A checked aggregate is never refused: its rate, TID, size and key
        // are its types'.
        runtime.with_phase(|core, hardware, _, _| {
            core.submit_ampdu(hardware, attempt).map(|admitted| {
                debug_assert!(admitted.is_ok(), "a checked aggregate is admitted");
            })
        })?;
        runtime.wake.signal(());
        let in_flight = InFlight {
            id,
            role: Role::ROLE,
            key,
        };
        if let Some(free) = self
            .held
            .in_flight
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.is_none())
        {
            *free = Some(in_flight);
        }
        let permit = aggregate
            .permit
            .take()
            .expect("a lent aggregate holds its permit");
        if let Some(free) = self
            .held
            .ampdu_in_flight
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.is_none())
        {
            *free = Some((id, permit));
        }
        drop(aggregate);
        self.held.tx.admit_queue(id, queue);
        Ok(Work::new(self, Esp32s31Token(id)))
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> BeaconTimingClient<'id>
    for Esp32s31Client<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        Station,
        B,
        N,
        AB,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    fn tsf(&self) -> PortResult<oer_ieee80211_mac::tsf::TsfInstant, ClockError, LowerMacFault> {
        Ok(self
            .runtime
            .with_phase(|core, hardware, _, _| Ok(core.tsf(hardware, vif_of(VifRole::Station))))?
            .map(|tsf| tsf.at)
            .map_err(|_| ClockError::Unavailable))
    }

    /// The TSF read between two MAC-clock readings of one generation: the
    /// sample's radio stamp is the first, its uncertainty the distance to
    /// the second plus the counter's microsecond.
    fn tsf_sample(&self) -> PortResult<TsfSample, ClockError, LowerMacFault> {
        let runtime = self.runtime;
        let (before, reading, after) = runtime.with_phase(|core, hardware, _, _| {
            let before = runtime.timer.snapshot();
            let reading = core.tsf_reading(hardware, vif_of(VifRole::Station));
            Ok((before, reading, runtime.timer.snapshot()))
        })?;
        let Some((before, uncertainty)) = before
            .zip(after)
            .map(|(before, after)| (before.sample(), after.sample()))
            .filter(|(before, after)| before.generation == after.generation)
            .and_then(|(before, after)| {
                after
                    .source
                    .checked_duration_since(before.source)
                    .and_then(|elapsed| {
                        elapsed.checked_add(oer_time::RadioDuration::from_micros(1))
                    })
                    .map(|uncertainty| (before, uncertainty))
            })
        else {
            return Ok(Err(ClockError::Unavailable));
        };
        Ok(reading
            .map(|(tsf, generation)| TsfSample {
                tsf,
                local: oer_ieee80211_lower_mac::Ieee80211Stamp {
                    at: before.source,
                    generation: before.generation,
                },
                uncertainty,
                generation,
            })
            .map_err(|_| ClockError::Unavailable))
    }

    fn set_tsf(
        &self,
        at: oer_ieee80211_mac::tsf::TsfInstant,
    ) -> Result<(), Poisoned<LowerMacFault>> {
        self.runtime.with_phase(|core, hardware, _, _| {
            let set = core.set_tsf(hardware, VifTsf::new(vif_of(VifRole::Station), at));
            debug_assert!(set.is_ok(), "a configured station sets its TSF");
            Ok(())
        })
    }

    fn set_tbtt(&self, plan: TbttPlan) -> Result<(), Poisoned<LowerMacFault>> {
        self.runtime.with_phase(|core, hardware, _, _| {
            let set = core.set_tbtt(
                hardware,
                TbttSchedule {
                    next: VifTsf::new(vif_of(VifRole::Station), plan.next),
                    beacon_interval: plan.beacon_interval,
                    lead: plan.lead,
                },
            );
            debug_assert!(set.is_ok(), "a configured station schedules its TBTTs");
            Ok(())
        })
    }

    fn stop_tbtt(&self) -> Result<(), Poisoned<LowerMacFault>> {
        self.runtime.with_phase(|core, hardware, _, _| {
            let _ = core.stop_tbtt(hardware, vif_of(VifRole::Station));
            Ok(())
        })
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
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> MonitorControl
    for Esp32s31Control<'_, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Fault = LowerMacFault;

    /// Monitor reception runs only while no interface receives, which the
    /// caller arranges first.
    fn set_monitor(&mut self, enabled: bool) -> Result<(), Poisoned<LowerMacFault>> {
        self.runtime.with_phase(|core, hardware, _, _| {
            let set = core.set_monitor(hardware, enabled);
            debug_assert!(
                set.is_ok(),
                "monitor reception runs with no interface receiving"
            );
            Ok(())
        })
    }
}

impl<
    'r,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> PortBackend for Esp32s31Backend<'r, 'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Event = Esp32s31LowerMacEvent<U::Unit, O, AS>;
    type Domain = Ieee80211Radio;
    type Fault = LowerMacFault;
    type Refusal = RetuneRefused;
    type Setup<'a, 'id>
        = Esp32s31Setup<'r, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
    where
        Self: 'a;
    type Events<'a, 'id>
        = Esp32s31Events<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        B,
        N,
        AB,
    >
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = Esp32s31Control<'r, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>>
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = Esp32s31Clients<
        'a,
        'id,
        Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>,
        B,
        N,
        AB,
    >
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = Esp32s31Resources<'id>;
    type Held<'id> = Esp32s31Held<'id, B, N, AB>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = (
        LowerMacCore<'slot, P, E, T, B, N, S, AS, AB>,
        H,
        U,
        U::Service,
    );

    async fn enable(&mut self) -> EnableResult<RetuneRefused, LowerMacFault> {
        let runtime = self.runtime;
        let start = runtime.with_installed(|core, _, _, sink| Ok(core.enable_checked(sink)))?;
        if let Some(LifecycleStart::Retune(channel)) = start {
            // The retune holds the registers for the switch; the driver
            // defers the interrupt causes meanwhile.
            let mut retune = runtime.retune.lock(|slot| slot.borrow_mut().take());
            let mut hardware = runtime.installed.lock(|installed| {
                installed
                    .borrow_mut()
                    .as_mut()
                    .and_then(|installed| installed.hardware.take())
            });
            let retuned = match (retune.as_mut(), hardware.as_mut()) {
                (Some(retune), Some(hardware)) => retune.retune(hardware, channel).await,
                _ => false,
            };
            runtime.retune.lock(|slot| *slot.borrow_mut() = retune);
            if let Some(hardware) = hardware {
                runtime.installed.lock(|installed| {
                    if let Some(installed) = installed.borrow_mut().as_mut() {
                        installed.hardware = Some(hardware);
                    }
                });
            }
            runtime.wake.signal(());
            runtime.with_installed(|core, _, _, sink| {
                core.finish_retune(retuned, sink);
                Ok(())
            })?;
        }
        Ok(match runtime.lifecycle_event().await? {
            LifecycleEvent::Enabled => Ok(()),
            _ => Err(RetuneRefused),
        })
    }

    async fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Esp32s31Held<'id, B, N, AB>,
        sink: &'a mut dyn FnMut(Esp32s31LowerMacEvent<U::Unit, O, AS>),
    ) -> Result<(), Poisoned<LowerMacFault>>
    where
        'id: 'a,
    {
        let runtime = self.runtime;
        runtime.with_installed(|core, _, _, sink| {
            core.disable_checked(sink);
            Ok(())
        })?;
        // Every admitted attempt ends before `Disabled`.
        while runtime.lifecycle_event().await? != LifecycleEvent::Disabled {}
        loop {
            held.complete(runtime.take_ended());
            match runtime.queues.take() {
                Some(Ok(QueuedEvent::TxCompleted(completion))) => {
                    sink(Esp32s31LowerMacEvent::TxCompleted(
                        completion,
                        runtime.unhold_body(completion.id),
                    ))
                }
                Some(_) => {}
                None if held.tx.idle() => return Ok(()),
                None => {
                    if let Some(poisoned) = runtime.poisoned() {
                        return Err(poisoned);
                    }
                    runtime.queues.wait().await;
                }
            }
        }
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee80211Instant, ClockError, LowerMacFault>> + '_ {
        ready(self.runtime.now())
    }

    fn resources<'id>(
        &mut self,
        mut mint: Mint<'id>,
    ) -> (Esp32s31Held<'id, B, N, AB>, Esp32s31Resources<'id>) {
        let (queue_line, buffer_line, [station_tx, access_point_tx]) = TxSeats::mint(&mut mint);
        let buffers = FifoPermitPool::new(mint.permits(), buffer_line);
        let (ampdu_line, [station_ampdu, access_point_ampdu]) =
            mint.seats::<Esp32s31AmpduKind, AMPDU_SEATS>();
        let held = Esp32s31Held {
            tx: TxPermits::new(&mut mint, queue_line, buffers),
            ampdu: FifoPermitPool::new(mint.permits(), ampdu_line),
            ampdu_in_flight: RefCell::new([const { None }; TX_QUEUES]),
            in_flight: RefCell::new([None; TX_QUEUES]),
            agreements: RefCell::new([const { None }; RX_BLOCK_ACK_SLOTS]),
        };
        let resources = Esp32s31Resources {
            station_tx,
            access_point_tx,
            station_pairwise_keys: mint.slots(),
            station_group_keys: mint.slots(),
            access_point_pairwise_keys: mint.slots(),
            access_point_group_keys: mint.slots(),
            block_acks: mint.slots(),
            station_ampdu,
            access_point_ampdu,
        };
        (held, resources)
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(Esp32s31Setup {
            runtime: self.runtime,
        })
    }

    fn split<'a, 'id>(
        &'a mut self,
        held: &'a Esp32s31Held<'id, B, N, AB>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        let runtime = self.runtime;
        PhaseParts {
            events: Lent::new(Esp32s31Events { runtime, held }),
            control: Lent::new(Esp32s31Control { runtime }),
            clients: Lent::new(Esp32s31Clients {
                station: Esp32s31Client {
                    runtime,
                    held,
                    role: PhantomData,
                },
                access_point: Esp32s31Client {
                    runtime,
                    held,
                    role: PhantomData,
                },
            }),
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'r, 'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
    {
        let runtime = self.runtime;
        let irq = self.irq;
        let power = self.power;
        // The driver owns the RX DMA epoch for its life and puts it back
        // when it retires.
        let mut rx = runtime.rx_service.lock(|slot| slot.borrow_mut().take());
        async move {
            let mut rx = Retained {
                value: rx.take(),
                home: |value| {
                    runtime
                        .rx_service
                        .lock(|slot| *slot.borrow_mut() = Some(value))
                },
            };
            // The deadline serviced last, which the core may keep until an
            // interrupt edge moves it.
            let mut serviced = None;
            loop {
                // A driver that sees the abandon latch publishes nothing and
                // stops.
                if retire.is_requested() || retire.is_abandoned() {
                    return;
                }
                held.complete(runtime.take_ended());
                if runtime.poisoned().is_some() {
                    match select(runtime.wake.wait(), retire.requested()).await {
                        embassy_futures::select::Either::First(()) => continue,
                        embassy_futures::select::Either::Second(()) => return,
                    }
                }
                if let Some(events) = irq.try_take_tx() {
                    // Completions coalesce: one service ends every attempt
                    // the edge reports.
                    let _ = runtime.with_installed(|core, hardware, _, sink| {
                        core.service(hardware, WifiTxWake::Interrupt { events }, sink)
                    });
                    continue;
                }
                if let Some(observation) = power.try_take()
                    && observation.sta_tbtt()
                {
                    let _ = runtime.with_installed(|core, hardware, _, sink| {
                        core.station_tbtt(hardware, sink);
                        Ok(())
                    });
                    continue;
                }
                let deadline = runtime
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
                        Some(deadline) => runtime.timer.wait_until(deadline).await,
                        None => core::future::pending().await,
                    }
                };
                // RX success is polled first, as the vendor's interrupt
                // order has it; a returned staging credit resumes a blocked
                // pass.
                let rx_wake = select(irq.wait_rx(), runtime.rx_capacity.wait());
                let interrupt = select3(rx_wake, irq.wait_tx(), power.wait());
                match select4(interrupt, watchdog, runtime.wake.wait(), retire.requested()).await {
                    Either4::First(Either3::First(_)) => {
                        // A budgeted drain while the interrupt stays masked;
                        // the unmask once the frontier is drained.
                        serve_rx(&mut rx.value, runtime, irq).await;
                    }
                    Either4::First(Either3::Second(events)) => {
                        // Completions coalesce: one service ends every
                        // attempt the edge reports.
                        let _ = runtime.with_installed(|core, hardware, _, sink| {
                            core.service(hardware, WifiTxWake::Interrupt { events }, sink)
                        });
                    }
                    Either4::First(Either3::Third(observation)) => {
                        if observation.sta_tbtt() {
                            let _ = runtime.with_installed(|core, hardware, _, sink| {
                                core.station_tbtt(hardware, sink);
                                Ok(())
                            });
                        }
                    }
                    Either4::Second(()) => {
                        serviced = deadline;
                        let _ = runtime.with_installed(|core, hardware, _, sink| {
                            core.service(hardware, WifiTxWake::Deadline, sink)
                        });
                    }
                    // A publication, an install or a fault; rearm.
                    Either4::Third(()) => serviced = None,
                    Either4::Fourth(()) => return,
                }
            }
        }
    }

    async fn retire_bound(&self) {
        let _ = self.runtime.timer.wait_for(RETIRE_BOUND).await;
    }

    fn detach<'id>(&mut self, _: &Esp32s31Held<'id, B, N, AB>) {
        // The interrupt entries reach the runtime, which keeps no core once
        // it is uninstalled: they post nothing to this install again.
    }

    fn uninstall<'id>(
        self,
        _: Esp32s31Held<'id, B, N, AB>,
        (): (),
        (): (),
    ) -> (
        LowerMacCore<'slot, P, E, T, B, N, S, AS, AB>,
        H,
        U,
        U::Service,
    ) {
        self.runtime
            .uninstall()
            .expect("a disabled port's core is idle with its registers home")
    }

    fn abandon(&mut self) {
        // Close transmission at once; the poisoned runtime serves nothing
        // more until the radio is reset.
        self.runtime.set_gate(false);
        self.runtime.fault.lock(|fault| {
            if fault.get().is_none() {
                fault.set(Some(LowerMacFault::Abandoned));
            }
        });
        self.runtime.queues.changed.signal(());
        self.runtime.wake.signal(());
    }
}

impl<
    'r,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: PortRxReceiver,
    S: AmpduBacking,
    const AS: usize,
    const AB: usize,
    O: TxBody,
> LowerMacBackend for Esp32s31Backend<'r, 'slot, M, P, E, T, H, R, B, N, EV, U, S, AS, AB, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
}

#[cfg(target_arch = "riscv32")]
mod install;

#[cfg(target_arch = "riscv32")]
pub use install::{
    Esp32s31PortRetune, Esp32s31WifiPortHeld, Esp32s31WifiPortInstallError,
    Esp32s31WifiPortInstallFailure, Esp32s31WifiPortInstalled, Esp32s31WifiPortUninstallFailure,
    Esp32s31WifiPortUninstalled, install_esp32s31_wifi_port, uninstall_esp32s31_wifi_port,
};
