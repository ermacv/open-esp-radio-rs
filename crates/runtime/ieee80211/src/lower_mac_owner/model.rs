//! An in-memory lower-MAC backend on the phase shape
//! (`oer_ieee80211_lower_mac::phase`), for host tests of the shape and of
//! the code that drives it.
//!
//! [`ModelBackend::new`] takes the install's channel and interface
//! addresses and returns the backend, which a test hands to
//! [`run_port`](oer_ieee80211_port::run_port), and the [`ModelAir`] it
//! plays on: the air answers each transmission, delivers received frames
//! and records what the hardware saw.
//!
//! The backend has the phase's shape ([owner decisions on #563]): the phase
//! is the body's handle over queues to the model's driver
//! (`PortBackend::drive`), which alone owns the model's hardware (its
//! queues of prepared attempts, its key table, its receive rules). The
//! handle writes commands, in one queue and in order, and setting cells;
//! the driver takes the cells before its next command, carries the
//! commands out, retries the head of each queue by its plan and posts the
//! events. The queues are cells of the one host thread a test runs on: the
//! stand-ins of a backend's lock-free single-producer single-consumer
//! queues.
//!
//! Each interface sends A-MPDUs ([`AmpduPhase`]) of up to
//! [`MODEL_AMPDU_SUBFRAMES`] subframes: under a key the new subframes are
//! numbered at their first transmission and a retransmitted one keeps its
//! number and gets the Retry bit; the air answers each aggregate with a
//! Block Ack ([`ModelAir::block_ack`], every subframe by default).
//!
//! Each concrete class pool constructs a private checked layout against
//! the actual allocation before writing headers or copying an external body. It proves disjoint MAC/CCMP
//! headers and tail body. Numbered returns own that layout, body and the
//! identity of the original key installation. Retry supplies only a new
//! attempt's rates, never an independently reconstructed MPDU or body.
//! Key identity is an allocation identity in this host model; a reused slot
//! cannot match an earlier installation. A refused attempt returns all its
//! resources synchronously and allocates no PN or completion event.
//!
//! `ModelContext` selects an opaque context for each install lifetime, so
//! a station context can retain its `LinkEpoch` brand through every queue
//! and terminal. The model grants one epoch-source slot in its resources.
//!
//! Capacity is the tokens': every terminal returns a ticket, so no terminal
//! is ever lost; received frames beyond [`MODEL_RX_EVENTS`] queued are
//! dropped and the stream says [`EventsLost`] once.
//!
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560

use alloc::{boxed::Box, collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    task::{Poll, Waker},
};
use oer_ieee80211_mac::ccmp::GroupKeyId;

use oer_ieee80211_mac::phy::{HeMcs, HtMcs, PhyRate};
use oer_ieee80211_port::{ClockError, EnableResult, PortBackend, PortPhase, Retire};
use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, Slot};
use oer_time::RadioInstant;

use oer_ieee80211_lower_mac::{
    AccessPoint, AccessPointPhase, AttemptKey, BandSet, BssColor, Ccmp128Key, Channel,
    CoexPriority, CoexPrioritySet, ConnectionEpochKind, FrameHeader, GateClosed, GateClosedKind,
    GateOpen, GateOpenKind, HardwareServices, Ieee80211Capabilities, Ieee80211Radio,
    IndividualAddress, InterfaceAggregateAttempt, InterfaceAttempt, InterfacePhase, KeyTypes,
    LinkMetric, LowerMacBackend, LowerMacCapabilities, LowerMacEvent, LowerMacPhase, LowerMacSetup,
    PhaseRate, PhaseTypes, PhyFormatSet, PoolSource, PreparationError, PreparationRefused,
    PreparedTx, RateSupport, ReceiveFilter, Removed, RemovedKey, RetransmissionPlan, RoleKeys,
    RxBuffer, RxEvidence, RxMeta, RxMetadata, SignalMetadata, Station, StationPhase,
    StationReceive, SubmitError, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool, TxCompletion, TxFrame,
    TxSource, TxStatus, VifRole, WidthSet,
};
use oer_ieee80211_lower_mac::{
    AmpduCapabilities, AmpduPhase, AmpduRate, BlockAckReport, PhaseAggregate, Tid,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

/// The longest MPDU the model's large buffers hold.
pub const MODEL_MAX_MPDU: usize = 2304;

/// What the model serves.
pub const MODEL_CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
    bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
    widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
    rates: RateSupport {
        dsss_cck: true,
        ofdm: true,
        ht_max_mcs: HtMcs::new(7),
        he_max_mcs: HeMcs::new(9),
        he_max_bandwidth_mhz: 20,
        he_dcm: true,
        he_ldpc: true,
        spatial_streams: 1,
    },
    services: HardwareServices::FCS
        .union(HardwareServices::IMMEDIATE_ACK)
        .union(HardwareServices::RETRY_POLICY)
        .union(HardwareServices::CIPHER_TRANSFORM)
        .union(HardwareServices::RX_BLOCK_ACK_MATCHING),
    vifs: 2,
    tx_queues: 4,
    max_mpdu_length: MODEL_MAX_MPDU as u16,
    max_backoff_slots: 1023,
    tx_power_ceiling_min_dbm: Some(0),
    coex_priorities: CoexPrioritySet::only(CoexPriority::Normal)
        .union(CoexPrioritySet::only(CoexPriority::Elevated)),
    individual_no_ack: PhyFormatSet::NON_HT,
    station_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    access_point_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
    key_slots: 4,
    rx_block_ack_agreements: 2,
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
    edca_max_aifsn: 15,
    edca_max_cw_exponent: 10,
};

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// The model's capabilities without its 5 GHz band: a port of the
/// ESP32-S31's bands, for host tests of a station whose rate selection
/// builds only for such a port (#748).
pub struct ModelCaps24;

impl Ieee80211Capabilities for ModelCaps24 {
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        ..MODEL_CAPABILITIES
    };
}

/// The model's transmit tickets.
pub const MODEL_TICKETS: usize = 8;
/// The model's small transmit buffers.
pub const MODEL_SMALL_BUFFERS: usize = 4;
/// The model's large transmit buffers.
pub const MODEL_LARGE_BUFFERS: usize = 4;
/// The received frames the event queue holds beside the terminals.
pub const MODEL_RX_EVENTS: usize = 4;
/// The key slots of each kind of each role.
pub const MODEL_KEY_SLOTS: usize = 2;
/// The receive Block Ack slots both roles share.
pub const MODEL_BLOCK_ACK_SLOTS: usize = 2;
/// The key slots of both roles, of both kinds.
const MODEL_ALL_KEY_SLOTS: usize = 4 * MODEL_KEY_SLOTS;

/// The commands the queue from the handle to the driver holds, first in,
/// first out: what the contract promises, an attempt per ticket and two
/// commands per key slot, Block Ack slot and the gate, whose install,
/// addition or closing and removal or opening may wait at once.
pub const MODEL_COMMANDS: usize =
    MODEL_TICKETS + 2 * (MODEL_ALL_KEY_SLOTS + MODEL_BLOCK_ACK_SLOTS + 1);
/// The events the queue from the driver to the body holds: a terminal per
/// ticket, a removal per key slot, the gate's closing, one confirmation of
/// each interface's settings (they coalesce, as the settings do) and the
/// received frames.
pub const MODEL_EVENTS: usize = MODEL_TICKETS + MODEL_ALL_KEY_SLOTS + 1 + 2 + MODEL_RX_EVENTS;

/// A queue of the contract's capacity: an overflow is a broken promise and
/// fails the test.
struct Bounded<T> {
    items: RefCell<VecDeque<T>>,
    capacity: usize,
    name: &'static str,
}

impl<T> Bounded<T> {
    fn new(capacity: usize, name: &'static str) -> Self {
        Self {
            items: RefCell::new(VecDeque::with_capacity(capacity)),
            capacity,
            name,
        }
    }

    fn push(&self, item: T) {
        let mut items = self.items.borrow_mut();
        assert!(
            items.len() < self.capacity,
            "the {} queue overflowed its contract capacity of {}",
            self.name,
            self.capacity
        );
        items.push_back(item);
    }

    fn pop(&self) -> Option<T> {
        self.items.borrow_mut().pop_front()
    }

    fn is_empty(&self) -> bool {
        self.items.borrow().is_empty()
    }
}

/// The kind of the model's transmit tickets.
pub enum ModelTickets {}

/// The small buffer class: management frames.
#[derive(Debug)]
pub enum ModelSmall {}

impl TxBufferClass for ModelSmall {
    const CAPACITY: usize = 256;
}

/// The large buffer class: any MPDU.
#[derive(Debug)]
pub enum ModelLarge {}

impl TxBufferClass for ModelLarge {
    const CAPACITY: usize = MODEL_MAX_MPDU;
}

/// The kind of the pairwise key slots of the role `R`.
pub struct ModelPairwiseKeys<R>(PhantomData<fn() -> R>);
/// The kind of the group key slots of the role `R`.
pub struct ModelGroupKeys<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots.
pub enum ModelBlockAcks {}

/// A transmit buffer of the class `C`: its token and its memory, which
/// moves with it; only the install mints one.
pub struct ModelBuffer<'id, C> {
    slot: Slot<'id, C>,
    memory: Box<[u8]>,
}

impl<C> TxBufferMemory for ModelBuffer<'_, C> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }
}

impl<C> core::fmt::Debug for ModelBuffer<'_, C> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("ModelBuffer")
            .field(&self.slot.index())
            .finish()
    }
}

/// A returned numbered MPDU. Its checked layout, installed-key identity,
/// and external body remain inseparable from the immutable buffer.
#[derive(Debug)]
pub struct ModelNumbered<'id, C> {
    buffer: ModelBuffer<'id, C>,
    layout: MpduLayout,
    key: KeyIdentity,
    body: Option<ModelBody>,
    packet_number: u64,
}

impl<'id, C> ModelNumbered<'id, C> {
    fn number(
        mut buffer: ModelBuffer<'id, C>,
        layout: MpduLayout,
        body: Option<ModelBody>,
        key: &KeyIdentity,
    ) -> Result<Self, ModelFault> {
        let packet_number = key.allocate()?;
        // Both ends belong to the layout checked against this allocation
        // and body before admission; numbering performs no new arithmetic.
        buffer.memory[layout.header_len..layout.prefix_len]
            .copy_from_slice(&ccmp_header(packet_number, key.0.key_id));
        Ok(Self {
            buffer,
            layout,
            body,
            key: key.clone(),
            packet_number,
        })
    }

    /// The original PN; a caller supplies no unchecked header offset.
    pub const fn packet_number(&self) -> u64 {
        self.packet_number
    }
    /// The retained external body, available read-only until renewal.
    pub fn body(&self) -> Option<&ModelBody> {
        self.body.as_ref()
    }
}

/// A transmit buffer of either class, fresh or numbered.
#[derive(Debug)]
pub enum ModelAnyBuffer<'id> {
    /// Fresh small memory.
    Small(ModelBuffer<'id, ModelSmall>),
    /// Fresh large memory.
    Large(ModelBuffer<'id, ModelLarge>),
    /// The original small numbered MPDU, including its body.
    SmallNumbered(ModelNumbered<'id, ModelSmall>),
    /// The original large numbered MPDU, including its body.
    LargeNumbered(ModelNumbered<'id, ModelLarge>),
}

impl ModelAnyBuffer<'_> {
    fn memory(&self) -> &[u8] {
        match self {
            Self::Small(buffer) => &buffer.memory,
            Self::Large(buffer) => &buffer.memory,
            Self::SmallNumbered(mpdu) => &mpdu.buffer.memory,
            Self::LargeNumbered(mpdu) => &mpdu.buffer.memory,
        }
    }
    fn memory_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Small(buffer) => &mut buffer.memory,
            Self::Large(buffer) => &mut buffer.memory,
            Self::SmallNumbered(mpdu) => &mut mpdu.buffer.memory,
            Self::LargeNumbered(mpdu) => &mut mpdu.buffer.memory,
        }
    }
    fn numbered_layout(&self) -> Option<(&MpduLayout, &KeyIdentity)> {
        match self {
            Self::SmallNumbered(mpdu) => Some((&mpdu.layout, &mpdu.key)),
            Self::LargeNumbered(mpdu) => Some((&mpdu.layout, &mpdu.key)),
            Self::Small(_) | Self::Large(_) => None,
        }
    }
    fn packet_number(&self) -> Option<u64> {
        match self {
            Self::SmallNumbered(mpdu) => Some(mpdu.packet_number),
            Self::LargeNumbered(mpdu) => Some(mpdu.packet_number),
            Self::Small(_) | Self::Large(_) => None,
        }
    }
}

/// Identity of an installation of a key, not its reusable slot index.
/// Rc is the host model's allocation identity; only the driver allocates PN.
#[derive(Clone, Debug)]
struct KeyIdentity(Rc<KeyInstall>);

#[derive(Debug)]
struct KeyInstall {
    index: u8,
    key_id: u8,
    last: Cell<u64>,
}

impl PartialEq for KeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for KeyIdentity {}

impl KeyIdentity {
    fn new(index: u8, key_id: u8) -> Self {
        Self(Rc::new(KeyInstall {
            index,
            key_id,
            last: Cell::new(0),
        }))
    }
    fn allocate(&self) -> Result<u64, ModelFault> {
        let next = self
            .0
            .last
            .get()
            .checked_add(1)
            .filter(|pn| *pn <= oer_ieee80211_mac::ccmp::CCMP_PACKET_NUMBER_MAX)
            .ok_or(ModelFault)?;
        self.0.last.set(next);
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MpduKind {
    Individual,
    IndividualNoAck,
    Group,
    BlockAckRequest,
}

/// This proof is private and stays beside the exact immutable allocation
/// it checked. All later ranges are within that allocation; neither a body
/// length nor a CCMP offset is reconstructed after admission.
#[derive(Clone, Copy, Debug)]
struct MpduLayout {
    len: usize,
    header_len: usize,
    prefix_len: usize,
    body_start: usize,
    protected: bool,
    kind: MpduKind,
}

impl MpduLayout {
    fn new(
        len: usize,
        header_len: usize,
        capacity: usize,
        protected: bool,
        body_len: usize,
        kind: MpduKind,
    ) -> Result<Self, PreparationError> {
        if len > capacity || header_len > len {
            return Err(PreparationError::BufferTooSmall);
        }
        let prefix = if protected {
            header_len
                .checked_add(8)
                .filter(|end| *end <= len)
                .ok_or(PreparationError::CcmpHeaderTooShort)?
        } else {
            header_len
        };
        let body_start = len
            .checked_sub(body_len)
            .filter(|start| *start >= prefix)
            .ok_or(PreparationError::BodyOverlap)?;
        Ok(Self {
            len,
            header_len,
            prefix_len: prefix,
            body_start,
            protected,
            kind,
        })
    }
    fn check_key(&self, key: Option<&KeyIdentity>) -> Result<(), PreparationError> {
        if self.protected == key.is_some() {
            Ok(())
        } else {
            Err(PreparationError::ProtectionMismatch)
        }
    }
}

struct ModelMpdu<'id> {
    buffer: ModelAnyBuffer<'id>,
    layout: MpduLayout,
    // Present for fresh MPDUs; numbered MPDUs own it within their buffer.
    body: Option<ModelBody>,
}

impl<'id> ModelMpdu<'id> {
    fn from_numbered(mut buffer: ModelAnyBuffer<'id>, layout: MpduLayout) -> Self {
        // The closed layout contains at least the checked MAC header.
        buffer.memory_mut()[1] |= 0x08;
        Self {
            buffer,
            layout,
            body: None,
        }
    }
    fn check_key(&self, key: Option<&KeyIdentity>) -> Result<(), PreparationError> {
        self.layout.check_key(key)?;
        if let Some((_, original)) = self.buffer.numbered_layout()
            && Some(original) != key
        {
            return Err(PreparationError::RetransmissionKey);
        }
        Ok(())
    }
    fn number(self, key: Option<&KeyIdentity>) -> Result<Self, ModelFault> {
        let Some(key) = key else {
            return Ok(self);
        };
        let Self {
            buffer,
            layout,
            body,
        } = self;
        let buffer = match buffer {
            ModelAnyBuffer::Small(buffer) => {
                ModelAnyBuffer::SmallNumbered(ModelNumbered::number(buffer, layout, body, key)?)
            }
            ModelAnyBuffer::Large(buffer) => {
                ModelAnyBuffer::LargeNumbered(ModelNumbered::number(buffer, layout, body, key)?)
            }
            ModelAnyBuffer::SmallNumbered(mpdu) => ModelAnyBuffer::SmallNumbered(mpdu),
            ModelAnyBuffer::LargeNumbered(mpdu) => ModelAnyBuffer::LargeNumbered(mpdu),
        };
        Ok(Self {
            buffer,
            layout,
            body: None,
        })
    }
    fn on_air(
        &self,
        role: VifRole,
        keys: &[KeyIdentity],
        key: Option<&KeyIdentity>,
        rate: PhyRate,
    ) -> AirFrame {
        AirFrame {
            role,
            frame: self.buffer.memory()[..self.layout.len].into(),
            rate,
            key: key.map(|key| (key.0.index, keys.contains(key))),
            packet_number: self.buffer.packet_number(),
        }
    }
}

/// The CCMP header of the packet number `number` under the key `key_id`.
fn ccmp_header(number: u64, key_id: u8) -> [u8; 8] {
    let pn = number.to_le_bytes();
    [
        pn[0],
        pn[1],
        0,
        0x20 | (key_id << 6),
        pn[2],
        pn[3],
        pn[4],
        pn[5],
    ]
}

/// Composition-selected contexts, including the install brand when needed.
/// The model neither interprets nor clones the associated value. Its lifetime
/// is the install, not the phase loan. A machine created inside the generic
/// body cannot be borrowed for that lifetime:
///
/// ```compile_fail,E0597
/// use oer_ieee80211_lower_mac::PhaseTypes;
/// use oer_ieee80211_port::{Disabled, Ended, Owner, PortBody};
/// use oer_ieee80211_runtime::lower_mac_owner::model::{
///     ModelBackend, ModelCaps, ModelContext, ModelResources, ModelTypes,
/// };
/// struct Machine;
/// struct Contexts;
/// impl ModelContext for Contexts { type Context<'id> = &'id Machine; }
/// type Backend = ModelBackend<ModelCaps, Contexts>;
/// struct Body;
/// impl PortBody<Backend> for Body {
///     async fn run<'id>(self, owner: Owner<'_, 'id, Backend, Disabled>,
///         _: ModelResources<'id>) -> Ended<'id, Backend> {
///         let machine = Machine;
///         let context: <ModelTypes<Contexts> as PhaseTypes<'id>>::Context = &machine;
///         core::hint::black_box(context);
///         owner.uninstall(())
///     }
/// }
/// ```
///
/// A context can instead own the install-branded connection epoch. `None`
/// covers operations before Connect, when Station has issued no epoch yet.
/// The `Rc` also demonstrates that the portable contract imposes no `Send`:
///
/// ```
/// use std::rc::Rc;
/// use oer_ieee80211_lower_mac::PhaseTypes;
/// use oer_ieee80211_port::{Disabled, Ended, Owner, PortBody};
/// use oer_ieee80211_sta::epoch::LinkEpoch;
/// use oer_ieee80211_runtime::lower_mac_owner::model::{
///     ModelBackend, ModelCaps, ModelContext, ModelResources, ModelTypes,
/// };
/// struct Context<'id> { epoch: Option<LinkEpoch<'id>>, owner: Rc<()> }
/// struct Contexts;
/// impl ModelContext for Contexts { type Context<'id> = Context<'id>; }
/// type Backend = ModelBackend<ModelCaps, Contexts>;
/// struct Body;
/// impl PortBody<Backend> for Body {
///     async fn run<'id>(self, owner: Owner<'_, 'id, Backend, Disabled>,
///         _: ModelResources<'id>) -> Ended<'id, Backend> {
///         let context: <ModelTypes<Contexts> as PhaseTypes<'id>>::Context =
///             Context { epoch: None, owner: Rc::new(()) };
///         core::hint::black_box(context);
///         owner.uninstall(())
///     }
/// }
/// ```
pub trait ModelContext {
    /// One operation's opaque context in the install `'id`.
    type Context<'id>: 'id;
}

impl ModelContext for () {
    type Context<'id> = ();
}

/// The types the model's events carry.
pub struct ModelTypes<X: ModelContext = ()>(PhantomData<fn() -> X>);

impl<'id, X: ModelContext> PhaseTypes<'id> for ModelTypes<X> {
    type Context = X::Context<'id>;
    type Prepared<'p, R, K: Ieee80211Capabilities> = ModelPrepared<'p, 'id, R, K, X>;
    type TxBody = ModelBody;
    type TicketKind = ModelTickets;
    type AnyTxBuffer = ModelAnyBuffer<'id>;
    type TxBuffers = Vec<ModelAnyBuffer<'id>>;
    type TxBodies = Vec<(u8, ModelBody)>;
    type RxBuffer = ModelRxBuffer;
    type RxMeta = ModelRxMeta;
    type StationKeys = ModelKeys<'id, Station>;
    type AccessPointKeys = ModelKeys<'id, AccessPoint>;
    type BlockAckKind = ModelBlockAcks;
    type BlockAck = ModelBlockAck<'id>;
}

/// The keys of the role `R` and their slots.
pub struct ModelKeys<'id, R>(PhantomData<fn(&'id ()) -> R>);

impl<'id, R> KeyTypes<'id> for ModelKeys<'id, R> {
    type PairwiseKind = ModelPairwiseKeys<R>;
    type GroupKind = ModelGroupKeys<R>;
    type PairwiseKey = ModelPairwiseKey<'id, R>;
    type GroupKey = ModelGroupKey<'id, R>;
}

impl<'id, X: ModelContext> TxBufferPool<'id, ModelSmall> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, ModelSmall>;
    type Numbered = ModelNumbered<'id, ModelSmall>;

    fn prepare<'p, R, K: Ieee80211Capabilities>(
        source: PoolSource<'p, 'id, Self, ModelSmall, K>,
    ) -> Result<
        ModelPrepared<'p, 'id, R, K, X>,
        PreparationRefused<PoolSource<'p, 'id, Self, ModelSmall, K>>,
    > {
        prepare_model(source)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Small(buffer)
    }

    fn erase_numbered(buffer: Self::Numbered) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::SmallNumbered(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id>) -> Result<Self::Buffer, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn numbered(buffer: ModelAnyBuffer<'id>) -> Result<Self::Numbered, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::SmallNumbered(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn renew(mpdu: Self::Numbered) -> (Self::Buffer, Option<ModelBody>) {
        (mpdu.buffer, mpdu.body)
    }
}

impl<'id, X: ModelContext> TxBufferPool<'id, ModelLarge> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, ModelLarge>;
    type Numbered = ModelNumbered<'id, ModelLarge>;

    fn prepare<'p, R, K: Ieee80211Capabilities>(
        source: PoolSource<'p, 'id, Self, ModelLarge, K>,
    ) -> Result<
        ModelPrepared<'p, 'id, R, K, X>,
        PreparationRefused<PoolSource<'p, 'id, Self, ModelLarge, K>>,
    > {
        prepare_model(source)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Large(buffer)
    }

    fn erase_numbered(buffer: Self::Numbered) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::LargeNumbered(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id>) -> Result<Self::Buffer, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Large(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn numbered(buffer: ModelAnyBuffer<'id>) -> Result<Self::Numbered, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::LargeNumbered(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn renew(mpdu: Self::Numbered) -> (Self::Buffer, Option<ModelBody>) {
        (mpdu.buffer, mpdu.body)
    }
}

/// What the model's radio observed of a received frame: the portable
/// fields and, always, its signal and the noise floor, so the model is a
/// port whose metadata measures the signal ([`SignalMetadata`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelRxMeta {
    common: RxMeta,
    rssi_dbm: i8,
    noise_floor_dbm: i8,
}

impl ModelRxMeta {
    /// A frame received on `channel` at `rssi_dbm` over `noise_floor_dbm`.
    pub const fn new(channel: Channel, rssi_dbm: i8, noise_floor_dbm: i8) -> Self {
        let mut common = RxMeta::unavailable(channel);
        common.rssi_dbm = RxEvidence::HardwareObserved(rssi_dbm);
        common.noise_floor_dbm = RxEvidence::HardwareObserved(noise_floor_dbm);
        Self {
            common,
            rssi_dbm,
            noise_floor_dbm,
        }
    }

    /// The same frame, with what the hardware observed of its decryption.
    pub const fn with_crypto(
        mut self,
        crypto: RxEvidence<oer_ieee80211_lower_mac::RxCryptoStatus>,
    ) -> Self {
        self.common.crypto = crypto;
        self
    }
}

impl RxMetadata for ModelRxMeta {
    fn common(&self) -> &RxMeta {
        &self.common
    }
}

impl SignalMetadata for ModelRxMeta {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
    }
}

/// The memory of one received MPDU, which goes back through
/// [`LowerMacPhase::recycle`]: dropped without it, it panics in a test.
#[derive(Debug)]
pub struct ModelRxBuffer {
    bytes: Vec<u8>,
    recycled: bool,
}

impl RxBuffer for ModelRxBuffer {
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for ModelRxBuffer {
    fn drop(&mut self) {
        #[cfg(any(test, feature = "test-support"))]
        if !self.recycled && !std::thread::panicking() {
            panic!("a received buffer was dropped without its recycle");
        }
    }
}

/// The owner of an MPDU's body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelBody(pub Vec<u8>);

impl TxBody for ModelBody {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// What poisons the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelFault;

/// The tokens the install gives the body once.
pub struct ModelResources<'id> {
    /// The install's single epoch-source slot; Station constructs its source.
    pub connection_epoch: Slot<'id, ConnectionEpochKind>,
    /// The transmit tickets.
    pub tickets: Vec<Slot<'id, ModelTickets>>,
    /// The small transmit buffers.
    pub small: Vec<ModelBuffer<'id, ModelSmall>>,
    /// The large transmit buffers.
    pub large: Vec<ModelBuffer<'id, ModelLarge>>,
    /// The station's pairwise key slots.
    pub station_pairwise: [Slot<'id, ModelPairwiseKeys<Station>>; MODEL_KEY_SLOTS],
    /// The station's group key slots.
    pub station_group: [Slot<'id, ModelGroupKeys<Station>>; MODEL_KEY_SLOTS],
    /// The access point's pairwise key slots.
    pub access_point_pairwise: [Slot<'id, ModelPairwiseKeys<AccessPoint>>; MODEL_KEY_SLOTS],
    /// The access point's group key slots.
    pub access_point_group: [Slot<'id, ModelGroupKeys<AccessPoint>>; MODEL_KEY_SLOTS],
    /// The receive Block Ack slots both roles share.
    pub block_acks: [Slot<'id, ModelBlockAcks>; MODEL_BLOCK_ACK_SLOTS],
    /// The open transmit gate.
    pub gate: GateOpen<'id>,
}

/// One transmission the air saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// The interface that sent it.
    pub role: VifRole,
    /// The MPDU, header and body, without the FCS.
    pub frame: Vec<u8>,
    /// Its rate.
    pub rate: PhyRate,
    /// The hardware key it named, and whether that key was installed when
    /// it went.
    pub key: Option<(u8, bool)>,
    /// The packet number of its CCMP header, under a key.
    pub packet_number: Option<u64>,
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
#[derive(Default)]
pub struct ModelAir {
    sent: RefCell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: RefCell<VecDeque<bool>>,
    rx: RefCell<VecDeque<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<bool>,
    preparation_refusal: Cell<Option<PreparationError>>,
    poisoned: Cell<bool>,
    driver: RefCell<Option<Waker>>,
    /// The driver's passes.
    passes: Cell<u32>,
    /// The commands the driver took, of every kind.
    commands: Cell<u32>,
    /// The settings the driver applied.
    settings: Cell<u32>,
    /// The queues the air keeps busy: their attempts wait unpublished.
    held: Cell<[bool; 4]>,
    /// The Block Acks of the next A-MPDUs, in order: bit `n` acknowledges
    /// the aggregate's subframe `n`; once none is queued, every subframe
    /// is acknowledged.
    block_acks: RefCell<VecDeque<u64>>,
}

impl ModelAir {
    /// Inject one refusal before admission for a service's failure-path
    /// scenario. No PN is allocated or TX terminal is posted. Class-pool
    /// preparation may already have written the checked MPDU.
    pub fn refuse_preparation(&self, reason: PreparationError) {
        self.preparation_refusal.set(Some(reason));
    }
    /// Every transmission so far.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.borrow().clone()
    }

    /// Answer the next transmission: acknowledged or not.
    pub fn answer(&self, acknowledged: bool) {
        self.answers.borrow_mut().push_back(acknowledged);
    }

    /// Answer the next A-MPDU with a Block Ack of the subframes whose
    /// bits `acknowledged` sets, bit `n` for its subframe `n`.
    pub fn block_ack(&self, acknowledged: u64) {
        self.block_acks.borrow_mut().push_back(acknowledged);
    }

    /// Deliver `frame` to the radio.
    pub fn receive(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx.borrow_mut().push_back((frame.into(), meta));
        self.wake_driver();
    }

    /// Keep the queue of `access_category` busy, or free it: its attempts
    /// wait unpublished meanwhile, as behind a long transmission.
    pub fn hold(&self, access_category: oer_ieee80211_mac::qos::WmmAccessCategory, held: bool) {
        let mut queues = self.held.get();
        queues[access_category as usize] = held;
        self.held.set(queues);
        self.wake_driver();
    }

    /// Refuse the next enable.
    pub fn refuse_enable(&self) {
        self.refuse_enable.set(true);
    }

    /// Poison the backend.
    pub fn poison(&self) {
        self.poisoned.set(true);
        self.wake_driver();
    }

    /// The driver's passes so far.
    pub fn passes(&self) -> u32 {
        self.passes.get()
    }

    /// The commands the driver took so far.
    pub fn commands(&self) -> u32 {
        self.commands.get()
    }

    /// The settings the driver applied so far.
    pub fn settings(&self) -> u32 {
        self.settings.get()
    }

    /// The frames the air holds for the driver yet.
    pub fn pending_rx(&self) -> usize {
        self.rx.borrow().len()
    }

    fn wake_driver(&self) {
        if let Some(waker) = self.driver.borrow_mut().take() {
            waker.wake();
        }
    }

    fn acknowledges(&self) -> bool {
        self.answers.borrow_mut().pop_front().unwrap_or(true)
    }
}

/// One stage of a retry plan, as the model's hardware takes it.
#[derive(Clone, Copy, Debug)]
struct Stage {
    rate: PhyRate,
    transmissions: u8,
}

/// A command the handle sends the driver, in the order it made it.
enum Command<'id, X: ModelContext> {
    Submit(Prepared<'id, X>),
    SubmitAmpdu(PreparedAggregate<'id, X>),
    InstallKey {
        key: KeyIdentity,
    },
    RemoveKey {
        key: KeyIdentity,
        removed: Removed<'id, ModelTypes<X>>,
    },
    CloseGate,
    OpenGate,
}

/// The work of a queue: one MPDU, or one A-MPDU.
enum Work<'id, X: ModelContext> {
    Single(Prepared<'id, X>),
    Aggregate(PreparedAggregate<'id, X>),
}

/// One subframe of an aggregate, in its buffer.
struct Subframe<'id> {
    mpdu: ModelMpdu<'id>,
}

/// An A-MPDU attempt prepared for its queue.
struct PreparedAggregate<'id, X: ModelContext> {
    context: X::Context<'id>,
    role: VifRole,
    ticket: Slot<'id, ModelTickets>,
    subframes: Vec<Subframe<'id>>,
    queue: usize,
    rate: PhyRate,
    key: Option<KeyIdentity>,
}

/// An attempt prepared for its queue.
struct Prepared<'id, X: ModelContext> {
    context: X::Context<'id>,
    role: VifRole,
    ticket: Slot<'id, ModelTickets>,
    mpdu: ModelMpdu<'id>,
    queue: usize,
    plan: Vec<Stage>,
    key: Option<KeyIdentity>,
}

/// The settings the handle writes and the driver takes before its next
/// command; the latest value of each kind wins.
#[derive(Default)]
struct Settings {
    station: bool,
    access_point: bool,
    other: bool,
}

/// The install's queues between the handle and the driver.
pub struct ModelHeld<'id, X: ModelContext = ()> {
    air: Rc<ModelAir>,
    commands: Bounded<Command<'id, X>>,
    /// The received buffers the body gives back, at most one per buffer.
    recycled: Bounded<ModelRxBuffer>,
    settings: RefCell<Settings>,
    events: Bounded<LowerMacEvent<'id, ModelTypes<X>>>,
    /// The receive buffers out with the body, queued or held.
    rx_out: Cell<usize>,
    /// A confirmation of each interface's settings waits in `events`.
    configured: Cell<[bool; 2]>,
    /// Received frames dropped since the stream last said so.
    lost: Cell<bool>,
    body: RefCell<Option<Waker>>,
    /// The closed gate the driver holds while the gate is open.
    closed: RefCell<Option<GateClosed<'id>>>,
    /// The open gate the driver holds while the gate is closed.
    open: RefCell<Option<GateOpen<'id>>>,
    /// Disable asks the driver to return all queued operations before the
    /// owner ends the phase. Unlike poison, this is an ordinary terminal.
    draining: Cell<bool>,
}

impl<'id, X: ModelContext> ModelHeld<'id, X> {
    fn command(&self, command: Command<'id, X>) {
        self.commands.push(command);
        self.air.wake_driver();
    }

    fn post(&self, event: LowerMacEvent<'id, ModelTypes<X>>) {
        self.events.push(event);
        self.wake_body();
    }

    /// Confirm `role`'s settings, once while a confirmation waits.
    fn configured(&self, role: VifRole) {
        let mut waiting = self.configured.get();
        let slot = &mut waiting[role as usize];
        if !*slot {
            *slot = true;
            self.configured.set(waiting);
            self.post(LowerMacEvent::Configured(role));
        }
    }

    fn wake_body(&self) {
        if let Some(waker) = self.body.borrow_mut().take() {
            waker.wake();
        }
    }

    fn poison(&self) {
        self.air.poison();
        self.wake_body();
    }
}

/// The install's local brand. Key identity belongs to each installed key;
/// there is no independent wrapping key-index source in the handle.
#[derive(Default)]
pub struct ModelLocal<'id> {
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// The model's backend; only [`run_port`](oer_ieee80211_port::run_port)
/// holds it.
pub struct ModelBackend<C = ModelCaps, X: ModelContext = ()> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    caps: PhantomData<fn() -> (C, X)>,
}

impl<C, X: ModelContext> ModelBackend<C, X> {
    /// A backend tuned to `channel` with the station's and the access
    /// point's addresses, and the air it plays on.
    pub fn new(channel: Channel, addresses: [IndividualAddress; 2]) -> (Self, Rc<ModelAir>) {
        let air = Rc::new(ModelAir::default());
        (
            Self {
                air: air.clone(),
                channel,
                addresses,
                caps: PhantomData,
            },
            air,
        )
    }
}

/// The model's disabled-only settings.
pub struct ModelSetup<'a, C = ModelCaps> {
    channel: &'a mut Channel,
    caps: PhantomData<fn() -> C>,
}

impl<C: Ieee80211Capabilities> LowerMacSetup for ModelSetup<'_, C> {
    type Caps = C;

    fn set_channel(&mut self, channel: SupportedChannel<C>) {
        *self.channel = channel.channel();
    }
}

impl<C: Ieee80211Capabilities, X: ModelContext> PortBackend for ModelBackend<C, X> {
    type Event<'id> = LowerMacEvent<'id, ModelTypes<X>>;
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;
    type Refusal = ();
    type Setup<'a, 'id>
        = ModelSetup<'a, C>
    where
        Self: 'a;
    type Phase<'a, 'id>
        = ModelPhase<'a, 'id, C, X>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = ModelResources<'id>;
    type Held<'id> = ModelHeld<'id, X>;
    type Local<'id> = ModelLocal<'id>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = Rc<ModelAir>;

    fn enable(&mut self) -> impl Future<Output = EnableResult<(), ModelFault>> + '_ {
        let air = &self.air;
        core::future::ready(if air.poisoned.get() {
            Err(Poisoned { cause: ModelFault })
        } else if air.refuse_enable.replace(false) {
            Ok(Err(()))
        } else {
            Ok(Ok(()))
        })
    }

    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a ModelHeld<'id, X>,
        _: &'a mut ModelLocal<'id>,
        sink: &'a mut dyn FnMut(LowerMacEvent<'id, ModelTypes<X>>),
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        // The driver ends every command it holds and takes every buffer
        // back; then every event goes to the sink.
        held.draining.set(true);
        held.air.wake_driver();
        poll_fn(move |cx| {
            if held.air.poisoned.get() {
                return Poll::Ready(Err(Poisoned { cause: ModelFault }));
            }
            if held.draining.get() || !held.commands.is_empty() || !held.recycled.is_empty() {
                *held.body.borrow_mut() = Some(cx.waker().clone());
                held.air.wake_driver();
                return Poll::Pending;
            }
            while let Some(event) = held.events.pop() {
                sink(event);
            }
            held.rx_out.set(0);
            Poll::Ready(Ok(()))
        })
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Ieee80211Radio>, ClockError, ModelFault>> + '_
    {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn resources<'id>(
        &mut self,
        mut mint: Mint<'id>,
    ) -> (ModelHeld<'id, X>, ModelLocal<'id>, ModelResources<'id>) {
        let [gate] = mint.slots::<GateOpenKind, 1>();
        let [closed] = mint.slots::<GateClosedKind, 1>();
        let tickets: [Slot<'id, ModelTickets>; MODEL_TICKETS] = mint.slots();
        let small: [Slot<'id, ModelSmall>; MODEL_SMALL_BUFFERS] = mint.slots();
        let large: [Slot<'id, ModelLarge>; MODEL_LARGE_BUFFERS] = mint.slots();
        let held = ModelHeld {
            air: self.air.clone(),
            commands: Bounded::new(MODEL_COMMANDS, "command"),
            recycled: Bounded::new(MODEL_RX_EVENTS, "recycle"),
            settings: RefCell::default(),
            events: Bounded::new(MODEL_EVENTS, "event"),
            rx_out: Cell::new(0),
            configured: Cell::new([false; 2]),
            lost: Cell::new(false),
            body: RefCell::default(),
            closed: RefCell::new(Some(closed)),
            open: RefCell::default(),
            draining: Cell::new(false),
        };
        let [epoch_slot] = mint.slots::<ConnectionEpochKind, 1>();
        let resources = ModelResources {
            connection_epoch: epoch_slot,
            tickets: tickets.into_iter().collect(),
            small: small
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: alloc::vec![0; ModelSmall::CAPACITY].into_boxed_slice(),
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: alloc::vec![0; ModelLarge::CAPACITY].into_boxed_slice(),
                })
                .collect(),
            station_pairwise: mint.slots(),
            station_group: mint.slots(),
            access_point_pairwise: mint.slots(),
            access_point_group: mint.slots(),
            block_acks: mint.slots(),
            gate,
        };
        (held, ModelLocal::default(), resources)
    }

    fn setup<'a, 'id>(&'a mut self, _: &'a mut Self::Local<'id>) -> Self::Setup<'a, 'id> {
        ModelSetup {
            channel: &mut self.channel,
            caps: PhantomData,
        }
    }

    fn phase<'a, 'id>(
        &'a mut self,
        held: &'a ModelHeld<'id, X>,
        _: &'a mut ModelLocal<'id>,
    ) -> ModelPhase<'a, 'id, C, X>
    where
        'id: 'a,
    {
        ModelPhase {
            backend: self,
            held,
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, C, X> {
        let mut driver = Driver {
            queues: core::array::from_fn(|_| VecDeque::new()),
            keys: Vec::new(),
            gate_open: true,
        };
        poll_fn(move |cx| {
            held.air.passes.set(held.air.passes.get() + 1);
            // A driver that sees the abandon latch publishes nothing and
            // stops.
            if held.air.poisoned.get() || retire.is_requested() || retire.is_abandoned() {
                return Poll::Ready(());
            }
            driver.serve(held);
            if held.air.poisoned.get() {
                return Poll::Ready(());
            }
            *held.air.driver.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        core::future::pending()
    }

    fn detach<'id>(&mut self, _: &ModelHeld<'id, X>) {}

    fn uninstall<'id>(
        self,
        _: ModelHeld<'id, X>,
        _: ModelLocal<'id>,
        (): (),
        (): (),
    ) -> Rc<ModelAir> {
        self.air
    }

    fn abandon(&mut self) {}
}

/// The depth of the model driver's software queue of each access
/// category: the composition's initial value.
pub const MODEL_TX_QUEUE_DEPTH: core::num::NonZeroU8 = match core::num::NonZeroU8::new(2) {
    Some(depth) => depth,
    None => panic!("a queue holds an attempt"),
};

impl<C: Ieee80211Capabilities, X: ModelContext> LowerMacBackend for ModelBackend<C, X> {
    type Caps = C;
    const TX_QUEUE_DEPTH: core::num::NonZeroU8 = MODEL_TX_QUEUE_DEPTH;
}

/// The model's hardware, which only the driver owns.
struct Driver<'id, X: ModelContext> {
    /// The prepared attempts of each queue, the head first.
    queues: [VecDeque<Work<'id, X>>; 4],
    /// The installed hardware keys.
    keys: Vec<KeyIdentity>,
    gate_open: bool,
}

impl<'id, X: ModelContext> Driver<'id, X> {
    /// Serve everything posted: the settings, the commands in order, the
    /// queues, then the air.
    fn serve(&mut self, held: &ModelHeld<'id, X>) {
        loop {
            if held.air.poisoned.get() {
                return;
            }
            // The settings come before the next command.
            let settings = core::mem::take(&mut *held.settings.borrow_mut());
            if settings.station {
                held.air.settings.set(held.air.settings.get() + 1);
                held.configured(VifRole::Station);
            }
            if settings.access_point {
                held.air.settings.set(held.air.settings.get() + 1);
                held.configured(VifRole::AccessPoint);
            }
            while let Some(mut buffer) = held.recycled.pop() {
                buffer.recycled = true;
                held.rx_out.set(held.rx_out.get().saturating_sub(1));
            }
            if settings.other {
                held.air.settings.set(held.air.settings.get() + 1);
            }
            let command = held.commands.pop();
            let Some(command) = command else {
                break;
            };
            held.air.commands.set(held.air.commands.get() + 1);
            match command {
                Command::Submit(prepared) => {
                    let queue = prepared.queue;
                    self.queues[queue].push_back(Work::Single(prepared));
                }
                Command::SubmitAmpdu(aggregate) => {
                    let queue = aggregate.queue;
                    self.queues[queue].push_back(Work::Aggregate(aggregate));
                }
                Command::InstallKey { key } => self.keys.push(key),
                Command::RemoveKey { key, removed } => {
                    self.keys.retain(|installed| *installed != key);
                    held.post(LowerMacEvent::KeyRemoved(removed));
                }
                Command::CloseGate => {
                    self.gate_open = false;
                    self.withdraw(held);
                    let closed = held.closed.borrow_mut().take();
                    if let Some(closed) = closed {
                        held.post(LowerMacEvent::GateClosed(closed));
                    }
                }
                Command::OpenGate => self.gate_open = true,
            }
            if self.gate_open && !held.draining.get() {
                self.publish(held);
            }
        }
        if held.draining.replace(false) {
            self.withdraw(held);
            held.wake_body();
        }
        // A queue the air released publishes without a command.
        if self.gate_open {
            self.publish(held);
        }
        if held.air.poisoned.get() {
            return;
        }
        self.receive(held);
    }

    /// Return unpublished work in each queue's original order.
    fn withdraw(&mut self, held: &ModelHeld<'id, X>) {
        for queue in &mut self.queues {
            while let Some(work) = queue.pop_front() {
                match work {
                    Work::Single(prepared) => {
                        terminal(held, prepared, TxStatus::Aborted, 0, None);
                    }
                    Work::Aggregate(aggregate) => {
                        let mut buffers = Vec::with_capacity(aggregate.subframes.len());
                        let mut bodies = Vec::new();
                        for (index, subframe) in aggregate.subframes.into_iter().enumerate() {
                            buffers.push(subframe.mpdu.buffer);
                            if let Some(body) = subframe.mpdu.body {
                                bodies.push((index as u8, body));
                            }
                        }
                        held.post(LowerMacEvent::TxCompleted {
                            context: aggregate.context,
                            ticket: aggregate.ticket,
                            buffers,
                            bodies,
                            completion: TxCompletion {
                                status: TxStatus::Aborted,
                                ack_rssi_dbm: None,
                                ack_snr_db: None,
                                block_ack: None,
                                transmissions: 0,
                                rate: None,
                            },
                        });
                    }
                }
            }
        }
    }

    /// Publish the head of every queue and carry its retry plan out.
    fn publish(&mut self, held: &ModelHeld<'id, X>) {
        let busy = held.air.held.get();
        for (queue, busy) in self.queues.iter_mut().zip(busy) {
            if busy {
                continue;
            }
            while let Some(work) = queue.pop_front() {
                match work {
                    Work::Single(prepared) => publish_single(held, &self.keys, prepared),
                    Work::Aggregate(aggregate) => publish_aggregate(held, &self.keys, aggregate),
                }
                if held.air.poisoned.get() {
                    return;
                }
            }
        }
    }

    /// Deliver the frames the air brought, as many as the receive buffers
    /// the body does not hold.
    fn receive(&mut self, held: &ModelHeld<'id, X>) {
        loop {
            let frame = held.air.rx.borrow_mut().pop_front();
            let Some((frame, meta)) = frame else {
                break;
            };
            if held.rx_out.get() < MODEL_RX_EVENTS {
                held.rx_out.set(held.rx_out.get() + 1);
                held.post(LowerMacEvent::Received {
                    frame: ModelRxBuffer {
                        bytes: frame,
                        recycled: false,
                    },
                    meta,
                });
            } else {
                held.lost.set(true);
                held.wake_body();
            }
        }
    }
}

/// Publish one MPDU and carry its retry plan out.
fn publish_single<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    mut prepared: Prepared<'id, X>,
) {
    prepared.mpdu = match prepared.mpdu.number(prepared.key.as_ref()) {
        Ok(mpdu) => mpdu,
        Err(_) => {
            held.poison();
            return;
        }
    };
    let mut transmissions = 0_u8;
    let mut last = None;
    let mut delivered = false;
    'plan: for stage in &prepared.plan {
        for _ in 0..stage.transmissions {
            transmissions = transmissions.saturating_add(1);
            last = Some(stage.rate);
            held.air.sent.borrow_mut().push(prepared.mpdu.on_air(
                prepared.role,
                keys,
                prepared.key.as_ref(),
                stage.rate,
            ));
            if held.air.acknowledges() {
                delivered = true;
                break 'plan;
            }
        }
    }
    let status = if delivered {
        TxStatus::Success
    } else {
        TxStatus::AckTimeout
    };
    terminal(held, prepared, status, transmissions, last);
}

/// Publish one A-MPDU: its new subframes numbered, its retransmitted ones
/// with their numbers; the air's Block Ack names the subframes it
/// acknowledged by their sequence numbers.
fn publish_aggregate<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    aggregate: PreparedAggregate<'id, X>,
) {
    let PreparedAggregate {
        context,
        role,
        ticket,
        subframes,
        rate,
        key,
        ..
    } = aggregate;
    let acknowledged = held
        .air
        .block_acks
        .borrow_mut()
        .pop_front()
        .unwrap_or(u64::MAX);
    let sequence = |mpdu: &ModelMpdu<'_>| {
        // Aggregate construction accepts only an AckHeader, at least 24 bytes.
        u16::from_le_bytes([mpdu.buffer.memory()[22], mpdu.buffer.memory()[23]]) >> 4
    };
    let mut buffers = Vec::with_capacity(subframes.len());
    let mut bodies = Vec::new();
    let mut start = None;
    let mut bitmap = 0_u64;
    for (index, subframe) in subframes.into_iter().enumerate() {
        let mpdu = match subframe.mpdu.number(key.as_ref()) {
            Ok(mpdu) => mpdu,
            Err(_) => {
                held.poison();
                return;
            }
        };
        held.air
            .sent
            .borrow_mut()
            .push(mpdu.on_air(role, keys, key.as_ref(), rate));
        let sequence = sequence(&mpdu);
        let start = *start.get_or_insert(sequence);
        if acknowledged & (1 << index) != 0 {
            bitmap |= 1 << (sequence.wrapping_sub(start) & 0x0fff).min(63);
        }
        buffers.push(mpdu.buffer);
        if let Some(body) = mpdu.body {
            bodies.push((index as u8, body));
        }
    }
    held.post(LowerMacEvent::TxCompleted {
        context,
        ticket,
        buffers,
        completion: TxCompletion {
            status: TxStatus::Success,
            ack_rssi_dbm: None,
            ack_snr_db: None,
            block_ack: start
                .and_then(SequenceNumber::new)
                .map(|start_sequence| BlockAckReport {
                    start_sequence,
                    bitmap,
                }),
            transmissions: 1,
            rate: Some(rate),
        },
        bodies,
    });
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    prepared: Prepared<'id, X>,
    status: TxStatus,
    transmissions: u8,
    rate: Option<PhyRate>,
) {
    held.post(LowerMacEvent::TxCompleted {
        context: prepared.context,
        ticket: prepared.ticket,
        buffers: alloc::vec![prepared.mpdu.buffer],
        completion: TxCompletion {
            status,
            ack_rssi_dbm: None,
            ack_snr_db: None,
            block_ack: None,
            transmissions,
            rate,
        },
        bodies: prepared
            .mpdu
            .body
            .into_iter()
            .map(|body| (0, body))
            .collect(),
    });
}

/// The model's running phase: the body's handle over the queues.
pub struct ModelPhase<'a, 'id, C = ModelCaps, X: ModelContext = ()> {
    backend: &'a mut ModelBackend<C, X>,
    held: &'a ModelHeld<'id, X>,
}

impl<'id, C, X: ModelContext> PortPhase for ModelPhase<'_, 'id, C, X> {
    type Event = LowerMacEvent<'id, ModelTypes<X>>;
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, ModelFault>> + '_ {
        let held = self.held;
        // Cancel-safe: the event stays queued until a poll takes it.
        poll_fn(move |cx| {
            if held.air.poisoned.get() {
                return Poll::Ready(Err(Poisoned { cause: ModelFault }));
            }
            if let Some(event) = held.events.pop() {
                if let LowerMacEvent::Configured(role) = event {
                    let mut waiting = held.configured.get();
                    waiting[role as usize] = false;
                    held.configured.set(waiting);
                }
                return Poll::Ready(Ok(Ok(event)));
            }
            if held.lost.replace(false) {
                return Poll::Ready(Ok(Err(EventsLost)));
            }
            *held.body.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
    }
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> LowerMacPhase<'p, 'id>
    for ModelPhase<'p, 'id, C, X>
{
    type Caps = C;
    type Types = ModelTypes<X>;
    type Station<'v>
        = ModelInterface<'p, 'v, 'id, Station, C, X>
    where
        Self: 'v;
    type AccessPoint<'v>
        = ModelInterface<'p, 'v, 'id, AccessPoint, C, X>
    where
        Self: 'v;

    fn channel(&self) -> Channel {
        self.backend.channel
    }

    fn recycle(&mut self, buffer: ModelRxBuffer) {
        self.held.recycled.push(buffer);
        self.held.air.wake_driver();
    }

    fn close_gate(&mut self, gate: GateOpen<'id>) {
        // The open gate waits with the backend while the gate is closed.
        self.held.open.borrow_mut().replace(gate);
        self.held.command(Command::CloseGate);
    }

    fn open_gate(&mut self, gate: GateClosed<'id>) -> GateOpen<'id> {
        // The closed gate goes back to the backend, the open one it kept to
        // the caller.
        self.held.closed.borrow_mut().replace(gate);
        self.held.command(Command::OpenGate);
        self.held
            .open
            .borrow_mut()
            .take()
            .expect("a closed gate's open twin waits with the backend")
    }

    fn set_edca(&mut self, _: SupportedEdca<C>) {
        self.held.settings.borrow_mut().other = true;
        self.held.air.wake_driver();
    }

    fn station(&mut self) -> ModelInterface<'p, '_, 'id, Station, C, X> {
        ModelInterface {
            held: self.held,
            address: self.backend.addresses[0],
            role: PhantomData,
        }
    }

    fn access_point(&mut self) -> ModelInterface<'p, '_, 'id, AccessPoint, C, X> {
        ModelInterface {
            held: self.held,
            address: self.backend.addresses[1],
            role: PhantomData,
        }
    }
}

/// One interface of the model's phase.
pub struct ModelInterface<'p, 'v, 'id, R, C = ModelCaps, X: ModelContext = ()> {
    held: &'v ModelHeld<'id, X>,
    address: IndividualAddress,
    role: PhantomData<fn(&'p ()) -> (&'p (), R, C)>,
}

/// A key the model installed.
pub struct ModelPairwiseKey<'id, R> {
    identity: KeyIdentity,
    slot: Slot<'id, ModelPairwiseKeys<R>>,
}

/// A group key the model installed.
pub struct ModelGroupKey<'id, R> {
    identity: KeyIdentity,
    slot: Slot<'id, ModelGroupKeys<R>>,
}

/// A receive Block Ack agreement the model keeps.
pub struct ModelBlockAck<'id> {
    slot: Slot<'id, ModelBlockAcks>,
}

/// A retry plan's stages, as the model's hardware takes them.
fn stages<C: Ieee80211Capabilities>(frame: &TxFrame<'_, C>) -> Vec<Stage> {
    match frame {
        TxFrame::Individual { plan, .. } => plan
            .stages()
            .map(|stage| Stage {
                rate: stage.rate.get(),
                transmissions: stage.transmissions.get(),
            })
            .collect(),
        TxFrame::BlockAckRequest { plan, .. } => plan
            .stages()
            .map(|stage| Stage {
                rate: stage.rate.rate().get(),
                transmissions: stage.transmissions.get(),
            })
            .collect(),
        TxFrame::IndividualNoAck { rate, .. } => alloc::vec![Stage {
            rate: rate.rate().get(),
            transmissions: 1,
        }],
        TxFrame::Group { rate, .. } => alloc::vec![Stage {
            rate: rate.get(),
            transmissions: 1,
        }],
    }
}

fn header_len<C: Ieee80211Capabilities>(frame: &TxFrame<'_, C>) -> usize {
    match frame {
        TxFrame::Individual { frame, .. } => frame.header().header_len(),
        TxFrame::IndividualNoAck { frame, .. } => frame.header().header_len(),
        TxFrame::Group { frame, .. } => frame.header().header_len(),
        TxFrame::BlockAckRequest { .. } => 20,
    }
}

/// The checked frame description is the source of the MAC header and its
/// QoS Ack Policy. The caller already encoded everything after the header.
fn write_header<C: Ieee80211Capabilities>(frame: &TxFrame<'_, C>, bytes: &mut [u8]) {
    match frame {
        TxFrame::Individual { frame, .. } => {
            frame.header().header().encode(bytes, false);
        }
        TxFrame::IndividualNoAck { frame, .. } => {
            frame.header().header().encode(bytes, true);
        }
        TxFrame::Group { frame, .. } => {
            frame.header().encode(bytes, true);
        }
        TxFrame::BlockAckRequest {
            receiver,
            transmitter,
            tid,
            starting_sequence,
            ..
        } => {
            let header = oer_ieee80211_mac::block_ack::encode_block_ack_request(
                receiver.get(),
                *transmitter,
                tid.get(),
                *starting_sequence,
            );
            bytes[..header.len()].copy_from_slice(&header);
        }
    }
}

fn frame_layout<K: Ieee80211Capabilities>(
    frame: &TxFrame<'_, K>,
    capacity: usize,
    body: Option<&ModelBody>,
) -> Result<MpduLayout, PreparationError> {
    let (protected, kind) = match frame {
        TxFrame::Individual { frame, .. } => {
            (frame.header().header().protected(), MpduKind::Individual)
        }
        TxFrame::IndividualNoAck { frame, .. } => (
            frame.header().header().protected(),
            MpduKind::IndividualNoAck,
        ),
        TxFrame::Group { frame, .. } => (frame.header().protected(), MpduKind::Group),
        TxFrame::BlockAckRequest { .. } => (false, MpduKind::BlockAckRequest),
    };
    MpduLayout::new(
        frame.mpdu_len(),
        header_len(frame),
        capacity,
        protected,
        body.map_or(0, |body| body.bytes().len()),
        kind,
    )
}

fn copy_body(buffer: &mut ModelAnyBuffer<'_>, layout: MpduLayout, body: Option<&ModelBody>) {
    if let Some(body) = body {
        // Layout and this body were checked together against this allocation.
        buffer.memory_mut()[layout.body_start..layout.len].copy_from_slice(body.bytes());
    }
}

fn retry_stages(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_>,
) -> Result<Vec<Stage>, PreparationError> {
    match (layout.kind, plan) {
        (MpduKind::Individual, RetransmissionPlan::Individual(plan)) => Ok(plan
            .stages()
            .map(|stage| Stage {
                rate: stage.rate.get(),
                transmissions: stage.transmissions.get(),
            })
            .collect()),
        (MpduKind::IndividualNoAck, RetransmissionPlan::IndividualNoAck(rate)) => {
            Ok(alloc::vec![Stage {
                rate: rate.rate().get(),
                transmissions: 1,
            }])
        }
        (MpduKind::Group, RetransmissionPlan::Group(rate)) => Ok(alloc::vec![Stage {
            rate: rate.get(),
            transmissions: 1,
        }]),
        _ => Err(PreparationError::RetransmissionKind),
    }
}

/// The pool's checked MPDU, independent of the input class after preparation.
/// Its private fields retain phase, install, capabilities and role until submit.
pub struct ModelPrepared<'p, 'id, R, K, X: ModelContext = ()> {
    mpdu: ModelMpdu<'id>,
    plan: Vec<Stage>,
    proof: PhantomData<fn(&'p ()) -> (&'p (), R, K, X)>,
}

impl<'id, R, K, X: ModelContext> PreparedTx<'id> for ModelPrepared<'_, 'id, R, K, X> {
    type Types = ModelTypes<X>;

    fn into_parts(self) -> (ModelAnyBuffer<'id>, Option<ModelBody>) {
        (self.mpdu.buffer, self.mpdu.body)
    }
}

trait ModelClass: TxBufferClass {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id>
    where
        Self: Sized;
    fn erase_numbered<'id>(mpdu: ModelNumbered<'id, Self>) -> ModelAnyBuffer<'id>
    where
        Self: Sized;
}

impl ModelClass for ModelSmall {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Small(buffer)
    }
    fn erase_numbered<'id>(mpdu: ModelNumbered<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::SmallNumbered(mpdu)
    }
}
impl ModelClass for ModelLarge {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Large(buffer)
    }
    fn erase_numbered<'id>(mpdu: ModelNumbered<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::LargeNumbered(mpdu)
    }
}

fn prepare_model<'p, 'id, R, K: Ieee80211Capabilities, X: ModelContext, C: ModelClass>(
    source: TxSource<
        'p,
        ModelBuffer<'id, C>,
        ModelNumbered<'id, C>,
        oer_ieee80211_lower_mac::ClassFrame<'p, C, K>,
        ModelBody,
    >,
) -> Result<
    ModelPrepared<'p, 'id, R, K, X>,
    PreparationRefused<
        TxSource<
            'p,
            ModelBuffer<'id, C>,
            ModelNumbered<'id, C>,
            oer_ieee80211_lower_mac::ClassFrame<'p, C, K>,
            ModelBody,
        >,
    >,
> {
    let checked = match &source {
        TxSource::Fresh {
            buffer,
            frame,
            body,
        } => frame_layout(frame.frame(), buffer.memory.len(), body.as_ref())
            .map(|layout| (layout, stages(frame.frame()))),
        TxSource::Retransmission { mpdu, plan } => {
            retry_stages(&mpdu.layout, plan).map(|plan| (mpdu.layout, plan))
        }
    };
    let (layout, plan) = match checked {
        Ok(checked) => checked,
        Err(reason) => {
            return Err(PreparationRefused {
                reason,
                operation: source,
            });
        }
    };
    let mpdu = match source {
        TxSource::Fresh {
            buffer,
            frame,
            body,
        } => {
            let mut buffer = C::erase(buffer);
            write_header(frame.frame(), buffer.memory_mut());
            copy_body(&mut buffer, layout, body.as_ref());
            ModelMpdu {
                buffer,
                layout,
                body,
            }
        }
        TxSource::Retransmission { mpdu, .. } => {
            ModelMpdu::from_numbered(C::erase_numbered(mpdu), layout)
        }
    };
    Ok(ModelPrepared {
        mpdu,
        plan,
        proof: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> InterfacePhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, X>
where
    R: RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    type Caps = K;
    type Types = ModelTypes<X>;
    type Role = R;
    type Fault = ModelFault;
    type TxBody = ModelBody;

    fn address(&self) -> IndividualAddress {
        self.address
    }

    fn submit<'k>(
        &mut self,
        attempt: InterfaceAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), SubmitError<InterfaceAttempt<'k, 'p, 'id, Self>, ModelFault>> {
        if self.held.air.poisoned.get() {
            return Err(SubmitError::Poisoned(Poisoned { cause: ModelFault }));
        }
        let key = attempt.key.as_ref().map(|key| match key {
            AttemptKey::Pairwise(key) => key.identity.clone(),
            AttemptKey::Group(key) => key.identity.clone(),
        });
        let checked = match self.held.air.preparation_refusal.take() {
            Some(reason) => Err(reason),
            None => attempt.source.mpdu.check_key(key.as_ref()),
        };
        if let Err(reason) = checked {
            return Err(SubmitError::Refused(PreparationRefused {
                reason,
                operation: attempt,
            }));
        }
        let ModelPrepared { mpdu, plan, .. } = attempt.source;
        self.held.command(Command::Submit(Prepared {
            context: attempt.context,
            role: R::ROLE,
            ticket: attempt.ticket,
            mpdu,
            queue: attempt.access_category as usize,
            plan,
            key,
        }));
        Ok(())
    }

    fn install_pairwise_key(
        &mut self,
        slot: Slot<'id, ModelPairwiseKeys<R>>,
        _: IndividualAddress,
        _: Ccmp128Key,
    ) -> ModelPairwiseKey<'id, R> {
        let identity = KeyIdentity::new(slot.index(), 0);
        self.held.command(Command::InstallKey {
            key: identity.clone(),
        });
        ModelPairwiseKey { identity, slot }
    }

    fn install_group_key(
        &mut self,
        slot: Slot<'id, ModelGroupKeys<R>>,
        key_id: GroupKeyId,
        _: Ccmp128Key,
    ) -> ModelGroupKey<'id, R> {
        let identity = KeyIdentity::new(slot.index(), key_id.get());
        self.held.command(Command::InstallKey {
            key: identity.clone(),
        });
        ModelGroupKey { identity, slot }
    }

    fn remove_pairwise_key(&mut self, key: ModelPairwiseKey<'id, R>) {
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: R::removed(RemovedKey::Pairwise(key.slot)),
        });
    }

    fn remove_group_key(&mut self, key: ModelGroupKey<'id, R>) {
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: R::removed(RemovedKey::Group(key.slot)),
        });
    }

    fn add_block_ack(
        &mut self,
        slot: Slot<'id, ModelBlockAcks>,
        _: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        ModelBlockAck { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ModelBlockAck<'id>,
        _: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        running
    }

    fn remove_block_ack(&mut self, agreement: ModelBlockAck<'id>) -> Slot<'id, ModelBlockAcks> {
        agreement.slot
    }
}

/// The subframes one model A-MPDU holds.
pub const MODEL_AMPDU_SUBFRAMES: usize = 8;

/// An A-MPDU the model builds: its subframes, each in a buffer of its own.
/// The rate retains the phase proof and the buffers retain the install;
/// the interface role also remains part of this intermediate type.
///
/// ```compile_fail
/// # use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// # use oer_ieee80211_lower_mac::{Station, AccessPoint};
/// fn other_role<'p, 'id>(a: ModelAggregate<'p, 'id, Station>) -> ModelAggregate<'p, 'id, AccessPoint> {
///     a
/// }
/// ```
///
/// ```compile_fail
/// # use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// # use oer_ieee80211_lower_mac::Station;
/// fn other_phase<'p, 'q, 'id>(a: ModelAggregate<'p, 'id, Station>) -> ModelAggregate<'q, 'id, Station> {
///     a
/// }
/// ```
///
/// ```compile_fail
/// # use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// # use oer_ieee80211_lower_mac::Station;
/// fn other_install<'p, 'id, 'other>(a: ModelAggregate<'p, 'id, Station>) -> ModelAggregate<'p, 'other, Station> {
///     a
/// }
/// ```
///
/// Moving it within the same phase, install and role is permitted:
///
/// ```
/// # use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// fn same<'p, 'id, R>(a: ModelAggregate<'p, 'id, R>) -> ModelAggregate<'p, 'id, R> { a }
/// ```
pub struct ModelAggregate<'p, 'id, R, C = ModelCaps, X: ModelContext = ()> {
    subframes: Vec<Subframe<'id>>,
    tid: Tid,
    rate: AmpduRate<'p>,
    caps: PhantomData<fn() -> (R, C, X)>,
}

impl<'p, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> PhaseAggregate<'p, 'id>
    for ModelAggregate<'p, 'id, R, K, X>
where
    R: RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    type Body = ModelBody;
    type Role = R;
    type Caps = K;
    type Types = ModelTypes<X>;

    fn push(
        &mut self,
        prepared: ModelPrepared<'p, 'id, R, K, X>,
    ) -> Result<(), PreparationRefused<ModelPrepared<'p, 'id, R, K, X>>> {
        let reason = if self.subframes.len() >= MODEL_AMPDU_SUBFRAMES {
            Some(PreparationError::AggregateFull)
        } else if prepared.mpdu.layout.kind != MpduKind::Individual {
            Some(PreparationError::RetransmissionKind)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PreparationRefused {
                reason,
                operation: prepared,
            });
        }
        self.subframes.push(Subframe {
            mpdu: prepared.mpdu,
        });
        Ok(())
    }

    fn subframes(&self) -> usize {
        self.subframes.len()
    }

    fn into_parts(self) -> (Vec<ModelAnyBuffer<'id>>, Vec<(u8, ModelBody)>) {
        let mut buffers = Vec::with_capacity(self.subframes.len());
        let mut bodies = Vec::new();
        for (index, subframe) in self.subframes.into_iter().enumerate() {
            buffers.push(subframe.mpdu.buffer);
            if let Some(body) = subframe.mpdu.body {
                bodies.push((index as u8, body));
            }
        }
        (buffers, bodies)
    }
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> AmpduPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, X>
where
    R: RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    type Aggregate = ModelAggregate<'p, 'id, R, K, X>;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        AmpduCapabilities {
            max_subframes: MODEL_AMPDU_SUBFRAMES as u16,
            formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
            max_length: 65_535,
        }
    }

    fn aggregate(&mut self, tid: Tid, rate: AmpduRate<'p>) -> ModelAggregate<'p, 'id, R, K, X> {
        ModelAggregate {
            subframes: Vec::new(),
            tid,
            rate,
            caps: PhantomData,
        }
    }

    fn submit_ampdu<'k>(
        &mut self,
        attempt: InterfaceAggregateAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), SubmitError<InterfaceAggregateAttempt<'k, 'p, 'id, Self>, ModelFault>> {
        if self.held.air.poisoned.get() {
            return Err(SubmitError::Poisoned(Poisoned { cause: ModelFault }));
        }
        let key = attempt.key.map(|key| key.identity.clone());
        let checked = if attempt.aggregate.subframes.is_empty() {
            Err(PreparationError::EmptyAggregate)
        } else {
            attempt
                .aggregate
                .subframes
                .iter()
                .try_for_each(|subframe| subframe.mpdu.check_key(key.as_ref()))
        };
        if let Err(reason) = checked {
            return Err(SubmitError::Refused(PreparationRefused {
                reason,
                operation: attempt,
            }));
        }
        let ModelAggregate {
            subframes,
            tid,
            rate,
            ..
        } = attempt.aggregate;
        let queue = match oer_ieee80211_mac::qos::WmmUserPriority::new(tid.get()) {
            Some(priority) => priority.access_category() as usize,
            None => oer_ieee80211_mac::qos::WmmAccessCategory::BestEffort as usize,
        };
        self.held.command(Command::SubmitAmpdu(PreparedAggregate {
            context: attempt.context,
            role: R::ROLE,
            ticket: attempt.ticket,
            subframes,
            queue,
            rate: rate.rate().get(),
            key,
        }));
        Ok(())
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> StationPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, Station, C, X>
{
    fn configure(&mut self, _: StationReceive<C>) {
        self.held.settings.borrow_mut().station = true;
        self.held.air.wake_driver();
    }

    fn set_bss_color(&mut self, _: BssColor) {
        self.held.settings.borrow_mut().other = true;
        self.held.air.wake_driver();
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> AccessPointPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, AccessPoint, C, X>
{
    fn configure(&mut self, _: SupportedFilter<C, AccessPoint>) {
        self.held.settings.borrow_mut().access_point = true;
        self.held.air.wake_driver();
    }
}

#[allow(dead_code, reason = "the rate check the service uses")]
fn rate<'p>(phase: &ModelPhase<'p, '_>, rate: PhyRate) -> Option<PhaseRate<'p>> {
    phase.rate(rate)
}

#[cfg(test)]
mod tests;
