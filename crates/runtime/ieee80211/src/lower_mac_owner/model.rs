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
//! Each concrete class pool uses the portable `MpduLayout` proof before
//! writing headers or copying an external body. Its allocation is privately
//! minted at the class capacity; `ClassFrame` proves the MPDU fits it.
//! `TxProtection` alone determines Protected and the MAC/CCMP prefix. Sent returns own that layout, body and the
//! identity of the original key installation. Retry supplies only a new
//! attempt's rates, never an independently reconstructed MPDU or body.
//! Key identity is an allocation identity in this host model; a reused slot
//! cannot match an earlier installation. A refused attempt returns all its
//! resources synchronously and allocates no PN or completion event.
//! Prepared operations retain a use of their key installation through their
//! terminal or cancellation. `KeyRemoved` waits for these owners; a returned
//! Sent retains only identity and therefore permits removal and slot reuse.
//! A prepared retry submitted after removal is withdrawn in its AC's order,
//! without publication. Cancellation wakes the driver to finish the barrier.
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

use alloc::{
    boxed::Box,
    collections::VecDeque,
    rc::{Rc, Weak},
    vec::Vec,
};
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
    AccessPoint, AccessPointPhase, BandSet, BssColor, Ccmp128Key, Channel, CoexPriority,
    CoexPrioritySet, ConnectionEpochKind, GateClosed, GateClosedKind, GateOpen, GateOpenKind,
    HardwareServices, Ieee80211Capabilities, Ieee80211Radio, IndeterminateCause, IndividualAddress,
    InterfaceAggregateAttempt, InterfaceAttempt, InterfacePhase, KeyTypes, LinkMetric,
    LowerMacBackend, LowerMacCapabilities, LowerMacEvent, LowerMacPhase, LowerMacSetup, MpduKind,
    MpduLayout, PhaseRate, PhaseTypes, PhyFormatSet, PoolSource, PreparationError,
    PreparationRefused, PreparedTx, RateSupport, ReceiveFilter, Removed, RemovedKey,
    RetransmissionPlan, RoleKeys, RxBuffer, RxEvidence, RxMeta, RxMetadata, SignalMetadata,
    Station, StationPhase, StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, TxAck, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool, TxCompleted,
    TxEndStatus, TxFrame, TxOutcome, TxProtection, TxSource, VifRole, WidthSet, WithdrawCause,
};
use oer_ieee80211_lower_mac::{
    AggregateRefused, AggregateState, AmpduCapabilities, AmpduPhase, AmpduRate, BlockAckReport,
    ClassSubframe, EmptyAggregate, InterfaceProtection, NonEmptyAggregate, PhaseAggregate,
    SubframeSource, Tid,
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
    preparation_refusal: Rc<Cell<Option<PreparationError>>>,
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

/// A returned published MPDU (open or protected). Its checked layout, installed-key identity,
/// and external body remain inseparable from the immutable buffer.
#[derive(Debug)]
pub struct ModelSent<'id, C> {
    role: VifRole,
    buffer: ModelBuffer<'id, C>,
    layout: MpduLayout,
    key: Option<KeyIdentity>,
    body: Option<ModelBody>,
    packet_number: Option<u64>,
}

impl<'id, C> ModelSent<'id, C> {
    fn publish(
        role: VifRole,
        mut buffer: ModelBuffer<'id, C>,
        layout: MpduLayout,
        body: Option<ModelBody>,
        key: Option<&KeyIdentity>,
    ) -> Result<Self, ModelFault> {
        let packet_number = match key {
            Some(key) => {
                let pn = key.allocate()?;
                // The range was checked against this allocation before admission.
                buffer.memory[layout.header_len()..layout.prefix_len()]
                    .copy_from_slice(&ccmp_header(pn, key.0.key_id));
                Some(pn)
            }
            None => None,
        };
        Ok(Self {
            role,
            buffer,
            layout,
            body,
            key: key.cloned(),
            packet_number,
        })
    }

    /// The original PN for a protected MPDU, otherwise `None`; a caller supplies no unchecked header offset.
    pub const fn packet_number(&self) -> Option<u64> {
        self.packet_number
    }
    /// The retained external body, available read-only until renewal.
    pub fn body(&self) -> Option<&ModelBody> {
        self.body.as_ref()
    }
}

/// A transmit buffer of either class, fresh or published.
#[derive(Debug)]
pub enum ModelAnyBuffer<'id> {
    /// Fresh small memory.
    Small(ModelBuffer<'id, ModelSmall>),
    /// Fresh large memory.
    Large(ModelBuffer<'id, ModelLarge>),
    /// The original small published MPDU, including its body.
    SmallSent(ModelSent<'id, ModelSmall>),
    /// The original large published MPDU, including its body.
    LargeSent(ModelSent<'id, ModelLarge>),
}

impl ModelAnyBuffer<'_> {
    fn memory(&self) -> &[u8] {
        match self {
            Self::Small(buffer) => &buffer.memory,
            Self::Large(buffer) => &buffer.memory,
            Self::SmallSent(mpdu) => &mpdu.buffer.memory,
            Self::LargeSent(mpdu) => &mpdu.buffer.memory,
        }
    }
    fn memory_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Small(buffer) => &mut buffer.memory,
            Self::Large(buffer) => &mut buffer.memory,
            Self::SmallSent(mpdu) => &mut mpdu.buffer.memory,
            Self::LargeSent(mpdu) => &mut mpdu.buffer.memory,
        }
    }
    fn packet_number(&self) -> Option<u64> {
        match self {
            Self::SmallSent(mpdu) => mpdu.packet_number,
            Self::LargeSent(mpdu) => mpdu.packet_number,
            Self::Small(_) | Self::Large(_) => None,
        }
    }
}

/// Identity of an installation of a key, not its reusable slot index.
/// Rc is the host model's allocation identity; only the driver allocates PN.
#[derive(Clone, Debug)]
pub struct KeyIdentity(Rc<KeyInstall>);

#[derive(Debug)]
struct KeyInstall {
    index: u8,
    key_id: u8,
    last: Cell<u64>,
    active: Cell<bool>,
    preparations: Rc<()>,
    air: Weak<ModelAir>,
}

impl PartialEq for KeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for KeyIdentity {}

impl KeyIdentity {
    fn new(index: u8, key_id: u8, air: &Rc<ModelAir>) -> Self {
        Self(Rc::new(KeyInstall {
            index,
            key_id,
            last: Cell::new(0),
            active: Cell::new(true),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
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

/// A preparation owns a use of this installed key until cancellation or its
/// terminal. Sent MPDUs retain identity without retaining a preparation use.
/// The model uses Rc ownership counts, not a parallel operation lookup table.
struct PreparedKey {
    identity: KeyIdentity,
    _lease: Rc<()>,
}
impl PreparedKey {
    fn new(identity: KeyIdentity) -> Self {
        let lease = identity.0.preparations.clone();
        Self {
            identity,
            _lease: lease,
        }
    }
}
impl Drop for PreparedKey {
    fn drop(&mut self) {
        // A pending removal may now finish, including cancellation of an
        // externally held preparation, which enqueues no command of its own.
        if let Some(air) = self.identity.0.air.upgrade() {
            air.wake_driver();
        }
    }
}

struct ModelMpdu<'id> {
    buffer: ModelAnyBuffer<'id>,
    layout: MpduLayout,
    // Present for fresh MPDUs; `Sent` MPDUs own it within their buffer.
    body: Option<ModelBody>,
}

impl<'id> ModelMpdu<'id> {
    fn from_sent(mut buffer: ModelAnyBuffer<'id>, layout: MpduLayout) -> Self {
        // The closed layout contains at least the checked MAC header.
        buffer.memory_mut()[1] |= 0x08;
        Self {
            buffer,
            layout,
            body: None,
        }
    }
    fn publish(self, role: VifRole, key: Option<&KeyIdentity>) -> Result<Self, ModelFault> {
        let Self {
            buffer,
            layout,
            body,
        } = self;
        let buffer = match buffer {
            ModelAnyBuffer::Small(buffer) => {
                ModelAnyBuffer::SmallSent(ModelSent::publish(role, buffer, layout, body, key)?)
            }
            ModelAnyBuffer::Large(buffer) => {
                ModelAnyBuffer::LargeSent(ModelSent::publish(role, buffer, layout, body, key)?)
            }
            ModelAnyBuffer::SmallSent(mpdu) => ModelAnyBuffer::SmallSent(mpdu),
            ModelAnyBuffer::LargeSent(mpdu) => ModelAnyBuffer::LargeSent(mpdu),
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
            frame: self.buffer.memory()[..self.layout.len()].into(),
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
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = ModelPrepared<'k, 'p, 'id, R, K, X>;
    type KeyIdentity = KeyIdentity;
    type Aggregate<'k, 'p, R, K, S>
        = ModelAggregate<'k, 'p, 'id, R, K, X, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k;
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

impl<'id, R: 'id> KeyTypes<'id> for ModelKeys<'id, R> {
    type PairwiseKind = ModelPairwiseKeys<R>;
    type GroupKind = ModelGroupKeys<R>;
    type PairwiseKey = ModelPairwiseKey<'id, R>;
    type GroupKey = ModelGroupKey<'id, R>;
    type Identity = KeyIdentity;
    fn pairwise_identity(key: &Self::PairwiseKey) -> KeyIdentity {
        key.identity.clone()
    }
    fn group_identity(key: &Self::GroupKey) -> KeyIdentity {
        key.identity.clone()
    }
}

impl<'id, X: ModelContext> TxBufferPool<'id, ModelSmall> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, ModelSmall>;
    type Sent = ModelSent<'id, ModelSmall>;

    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: PoolSource<'k, 'p, 'id, Self, ModelSmall, K, R>,
    ) -> oer_ieee80211_lower_mac::PoolPreparation<'k, 'p, 'id, Self, ModelSmall, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
    }

    fn push_aggregate<'k, 'p, R, K, S>(
        aggregate: ModelAggregate<'k, 'p, 'id, R, K, X, S>,
        source: oer_ieee80211_lower_mac::PoolSubframe<'id, Self, ModelSmall, K>,
    ) -> oer_ieee80211_lower_mac::AggregatePushResult<'k, 'p, 'id, Self, ModelSmall, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        push_model(aggregate, source)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Small(buffer)
    }

    fn erase_sent(buffer: Self::Sent) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::SmallSent(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id>) -> Result<Self::Buffer, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn sent(buffer: ModelAnyBuffer<'id>) -> Result<Self::Sent, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::SmallSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn renew(mpdu: Self::Sent) -> (Self::Buffer, Option<ModelBody>) {
        (mpdu.buffer, mpdu.body)
    }
}

impl<'id, X: ModelContext> TxBufferPool<'id, ModelLarge> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, ModelLarge>;
    type Sent = ModelSent<'id, ModelLarge>;

    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: PoolSource<'k, 'p, 'id, Self, ModelLarge, K, R>,
    ) -> oer_ieee80211_lower_mac::PoolPreparation<'k, 'p, 'id, Self, ModelLarge, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
    }

    fn push_aggregate<'k, 'p, R, K, S>(
        aggregate: ModelAggregate<'k, 'p, 'id, R, K, X, S>,
        source: oer_ieee80211_lower_mac::PoolSubframe<'id, Self, ModelLarge, K>,
    ) -> oer_ieee80211_lower_mac::AggregatePushResult<'k, 'p, 'id, Self, ModelLarge, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        push_model(aggregate, source)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Large(buffer)
    }

    fn erase_sent(buffer: Self::Sent) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::LargeSent(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id>) -> Result<Self::Buffer, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Large(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn sent(buffer: ModelAnyBuffer<'id>) -> Result<Self::Sent, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::LargeSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }

    fn renew(mpdu: Self::Sent) -> (Self::Buffer, Option<ModelBody>) {
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

#[derive(Clone, Copy)]
enum AirResult {
    Acknowledged,
    AckTimeout,
    Collision,
    HardwareTimeout,
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
#[derive(Default)]
pub struct ModelAir {
    sent: RefCell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: RefCell<VecDeque<AirResult>>,
    rx: RefCell<VecDeque<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<bool>,
    preparation_refusal: Rc<Cell<Option<PreparationError>>>,
    poisoned: Cell<bool>,
    #[cfg(test)]
    swap_terminals: Cell<bool>,
    #[cfg(test)]
    pub(crate) corrupted_terminal: Cell<bool>,
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
    #[cfg(test)]
    pub(crate) fn swap_next_two_single_buffers(&self) {
        self.swap_terminals.set(true);
    }
    /// Inject one refusal before admission for a service's failure-path
    /// scenario. No PN is allocated or TX terminal is posted. Class-pool
    /// preparation may already have written the checked MPDU.
    pub fn refuse_preparation(&self, reason: PreparationError) {
        self.preparation_refusal.set(Some(reason));
    }
    /// Every publication so far, including collisions and unknown outcomes.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.borrow().clone()
    }

    /// Answer the next transmission: acknowledged or not.
    pub fn answer(&self, acknowledged: bool) {
        self.answers.borrow_mut().push_back(if acknowledged {
            AirResult::Acknowledged
        } else {
            AirResult::AckTimeout
        });
    }

    /// End the next publication with no completion. A timeout is final:
    /// the MPDU becomes Indeterminate and the next queued attempt may run.
    pub fn timeout(&self) {
        self.answers
            .borrow_mut()
            .push_back(AirResult::HardwareTimeout);
    }

    /// An internal collision before the MPDU reached the air. The driver
    /// consumes the next step of the plan without returning its resources.
    pub fn collision(&self) {
        self.answers.borrow_mut().push_back(AirResult::Collision);
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

    fn result(&self) -> AirResult {
        self.answers
            .borrow_mut()
            .pop_front()
            .unwrap_or(AirResult::Acknowledged)
    }
}

/// One stage of a retry plan, as the model's hardware takes it.
#[derive(Clone, Copy, Debug)]
struct Stage {
    rate: PhyRate,
    transmissions: u8,
}

/// A command the handle sends the driver, in the order it made it.
/// The host model boxes its larger single-operation owner so small commands
/// do not reserve that space; the box moves opaque context without cloning.
enum Command<'id, X: ModelContext> {
    Submit(Box<Prepared<'id, X>>),
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
    Single(Box<Prepared<'id, X>>),
    Aggregate(PreparedAggregate<'id, X>),
}

impl<X: ModelContext> Work<'_, X> {
    fn key(&self) -> Option<&KeyIdentity> {
        match self {
            Self::Single(prepared) => prepared.key.as_ref(),
            Self::Aggregate(prepared) => prepared.key.as_ref(),
        }
        .map(|key| &key.identity)
    }
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
    key: Option<PreparedKey>,
}

/// An attempt prepared for its queue.
struct Prepared<'id, X: ModelContext> {
    context: X::Context<'id>,
    role: VifRole,
    ticket: Slot<'id, ModelTickets>,
    mpdu: ModelMpdu<'id>,
    queue: usize,
    plan: Vec<Stage>,
    key: Option<PreparedKey>,
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
    #[cfg(test)]
    held_terminal: RefCell<Option<TxCompleted<'id, ModelTypes<X>>>>,
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
        #[cfg(test)]
        let event = match event {
            LowerMacEvent::TxCompleted(mut current @ TxCompleted::Single { .. })
                if self.air.swap_terminals.get() =>
            {
                let Some(mut previous) = self.held_terminal.borrow_mut().take() else {
                    *self.held_terminal.borrow_mut() = Some(current);
                    return;
                };
                match (&mut previous, &mut current) {
                    (
                        TxCompleted::Single {
                            buffer: a,
                            body: ab,
                            ..
                        },
                        TxCompleted::Single {
                            buffer: b,
                            body: bb,
                            ..
                        },
                    ) => {
                        core::mem::swap(a, b);
                        core::mem::swap(ab, bb);
                    }
                    _ => panic!("test injector holds only Single terminals"),
                }
                self.air.swap_terminals.set(false);
                self.air.corrupted_terminal.set(true);
                self.events.push(LowerMacEvent::TxCompleted(previous));
                LowerMacEvent::TxCompleted(current)
            }
            event => event,
        };
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
            #[cfg(test)]
            held_terminal: RefCell::new(None),
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
                    preparation_refusal: self.air.preparation_refusal.clone(),
                    memory: alloc::vec![0; ModelSmall::CAPACITY].into_boxed_slice(),
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    preparation_refusal: self.air.preparation_refusal.clone(),
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
            removing: Vec::new(),
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
    removing: Vec<(KeyIdentity, Removed<'id, ModelTypes<X>>)>,
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
                    self.removing.push((key, removed));
                }
                Command::CloseGate => {
                    self.gate_open = false;
                    self.withdraw(held, WithdrawCause::GateClosed);
                    let closed = held.closed.borrow_mut().take();
                    if let Some(closed) = closed {
                        held.post(LowerMacEvent::GateClosed(closed));
                    }
                }
                Command::OpenGate => self.gate_open = true,
            }
            if !held.draining.get() {
                self.publish(held);
            }
        }
        if held.draining.replace(false) {
            self.withdraw(held, WithdrawCause::Disabled);
            held.wake_body();
        }
        // A queue the air released publishes without a command.
        self.publish(held);
        if held.air.poisoned.get() {
            return;
        }
        self.finish_removals(held);
        self.receive(held);
    }

    /// Return unpublished work in each queue's original order.
    fn withdraw(&mut self, held: &ModelHeld<'id, X>, cause: WithdrawCause) {
        for queue in &mut self.queues {
            while let Some(work) = queue.pop_front() {
                withdrawn(held, work, cause);
            }
        }
    }

    /// Publish the head of every queue and carry its retry plan out.
    fn publish(&mut self, held: &ModelHeld<'id, X>) {
        let busy = held.air.held.get();
        for (queue, busy) in self.queues.iter_mut().zip(busy) {
            while let Some(work) = queue.pop_front() {
                // Inspect the owned head. If it cannot run, put that same
                // owner back; no second lookup reconstructs its identity.
                let cause = if work.key().is_some_and(|key| !self.keys.contains(key)) {
                    Some(WithdrawCause::KeyRemoved)
                } else if !self.gate_open {
                    Some(WithdrawCause::GateClosed)
                } else {
                    None
                };
                if let Some(cause) = cause {
                    withdrawn(held, work, cause);
                } else if busy {
                    queue.push_front(work);
                    break;
                } else {
                    match work {
                        Work::Single(prepared) => publish_single(held, &self.keys, *prepared),
                        Work::Aggregate(aggregate) => {
                            publish_aggregate(held, &self.keys, aggregate)
                        }
                    }
                }
                if held.air.poisoned.get() {
                    return;
                }
            }
        }
    }

    fn finish_removals(&mut self, held: &ModelHeld<'id, X>) {
        // The single reference belongs to KeyInstall. Additional owners are
        // actual prepared/queued/published attempts. Returned Sent identities
        // do not keep a preparation alive and do not delay key-slot reuse.
        let mut pending = Vec::new();
        for (key, removed) in self.removing.drain(..) {
            if Rc::strong_count(&key.0.preparations) == 1 {
                held.post(LowerMacEvent::KeyRemoved(removed));
            } else {
                pending.push((key, removed));
            }
        }
        self.removing = pending;
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
    prepared.mpdu = match prepared.mpdu.publish(
        prepared.role,
        prepared.key.as_ref().map(|key| &key.identity),
    ) {
        Ok(mpdu) => mpdu,
        Err(_) => {
            held.poison();
            return;
        }
    };
    // A closed RetryPlan holds at most four u8 stages: u16 retains the
    // exact count even for a plan longer than 255 transmissions.
    let mut transmissions = 0_u16;
    let mut last = None;
    let mut status = TxEndStatus::AckTimeout;
    'plan: for stage in &prepared.plan {
        for _ in 0..stage.transmissions {
            if transmissions != 0 {
                prepared.mpdu.buffer.memory_mut()[1] |= 0x08;
            }
            held.air.sent.borrow_mut().push(prepared.mpdu.on_air(
                prepared.role,
                keys,
                prepared.key.as_ref().map(|key| &key.identity),
                stage.rate,
            ));
            match held.air.result() {
                AirResult::HardwareTimeout => {
                    terminal(
                        held,
                        prepared,
                        TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
                    );
                    return;
                }
                AirResult::Collision => status = TxEndStatus::ContentionLimit,
                AirResult::Acknowledged => {
                    transmissions += 1;
                    last = Some(stage.rate);
                    status = TxEndStatus::Success;
                    break 'plan;
                }
                AirResult::AckTimeout => {
                    transmissions += 1;
                    last = Some(stage.rate);
                    status = TxEndStatus::AckTimeout;
                }
            }
        }
    }
    terminal(
        held,
        prepared,
        TxOutcome::Ended {
            status,
            transmissions,
            rate: last,
            ack: (status == TxEndStatus::Success).then_some(TxAck::default()),
        },
    );
}

/// Preserve the submission shape and every owner on withdrawal.
fn withdrawn<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    work: Work<'id, X>,
    cause: WithdrawCause,
) {
    match work {
        Work::Single(prepared) => {
            terminal(held, *prepared, TxOutcome::Withdrawn(cause));
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
            held.post(LowerMacEvent::TxCompleted(TxCompleted::Aggregate {
                context: aggregate.context,
                ticket: aggregate.ticket,
                buffers,
                bodies,
                completion: TxOutcome::Withdrawn(cause),
            }));
        }
    }
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
        let mpdu = match subframe
            .mpdu
            .publish(role, key.as_ref().map(|key| &key.identity))
        {
            Ok(mpdu) => mpdu,
            Err(_) => {
                held.poison();
                return;
            }
        };
        held.air.sent.borrow_mut().push(mpdu.on_air(
            role,
            keys,
            key.as_ref().map(|key| &key.identity),
            rate,
        ));
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
    held.post(LowerMacEvent::TxCompleted(TxCompleted::Aggregate {
        context,
        ticket,
        buffers,
        completion: TxOutcome::Ended {
            status: TxEndStatus::Success,
            transmissions: 1,
            rate: Some(rate),
            ack: Some(TxAck {
                block_ack: start.and_then(SequenceNumber::new).map(|start_sequence| {
                    BlockAckReport {
                        start_sequence,
                        bitmap,
                    }
                }),
                ..TxAck::default()
            }),
        },
        bodies,
    }));
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    prepared: Prepared<'id, X>,
    outcome: TxOutcome,
) {
    held.post(LowerMacEvent::TxCompleted(TxCompleted::Single {
        context: prepared.context,
        ticket: prepared.ticket,
        buffer: prepared.mpdu.buffer,
        completion: outcome,
        body: prepared.mpdu.body,
    }));
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
    role: PhaseProof<'p, (R, C)>,
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

/// The checked frame description is the source of the MAC header and its
/// QoS Ack Policy. The caller already encoded everything after the header.
fn write_header<C: Ieee80211Capabilities>(
    frame: &TxFrame<'_, C>,
    bytes: &mut [u8],
    protected: bool,
) {
    match frame {
        TxFrame::Individual { frame, .. } => {
            frame
                .header()
                .header()
                .with_protection(protected)
                .encode(bytes, false);
        }
        TxFrame::IndividualNoAck { frame, .. } => {
            frame
                .header()
                .header()
                .with_protection(protected)
                .encode(bytes, true);
        }
        TxFrame::Group { frame, .. } => {
            frame
                .header()
                .with_protection(protected)
                .encode(bytes, true);
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

fn copy_body(buffer: &mut ModelAnyBuffer<'_>, layout: MpduLayout, body: Option<&ModelBody>) {
    if let Some(body) = body {
        // Layout and this body were checked together against this allocation.
        buffer.memory_mut()[layout.body_start()..layout.len()].copy_from_slice(body.bytes());
    }
}

fn retry_stages(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_>,
) -> Result<Vec<Stage>, PreparationError> {
    match (layout.kind(), plan) {
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

type PhaseProof<'p, T> = PhantomData<fn(&'p ()) -> (&'p (), T)>;
type TypeProof<T> = PhantomData<fn() -> T>;
type ModelSource<'k, 'p, 'id, C, K, R, X> = TxSource<
    'p,
    ModelBuffer<'id, C>,
    ModelSent<'id, C>,
    oer_ieee80211_lower_mac::ClassFrame<'p, C, K>,
    ModelBody,
    <R as RoleKeys<'id, ModelTypes<X>>>::TxKey<'k>,
>;
type ModelPreparation<'k, 'p, 'id, R, K, X, C> = Result<
    ModelPrepared<'k, 'p, 'id, R, K, X>,
    PreparationRefused<ModelSource<'k, 'p, 'id, C, K, R, X>>,
>;

/// Closed preparation proof retaining role, phase, install and the key borrow.
/// Dropping or submitting it ends that borrow; a queued operation owns only
/// the backend identity, never a reference to a portable machine.
pub struct ModelPrepared<'k, 'p, 'id, R, K, X: ModelContext = ()> {
    mpdu: ModelMpdu<'id>,
    plan: Vec<Stage>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    proof: PhaseProof<'p, (R, K, X)>,
}
impl<'id, R, K, X: ModelContext> PreparedTx<'id> for ModelPrepared<'_, '_, 'id, R, K, X> {
    type Types = ModelTypes<X>;
    fn into_parts(self) -> (ModelAnyBuffer<'id>, Option<ModelBody>) {
        (self.mpdu.buffer, self.mpdu.body)
    }
}

trait ModelClass: TxBufferClass {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id>
    where
        Self: Sized;
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id>
    where
        Self: Sized;
}

impl ModelClass for ModelSmall {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Small(buffer)
    }
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::SmallSent(mpdu)
    }
}
impl ModelClass for ModelLarge {
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Large(buffer)
    }
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::LargeSent(mpdu)
    }
}

#[expect(
    clippy::result_large_err,
    reason = "a refusal returns the whole operation by value"
)]
fn prepare_model<
    'k,
    'p,
    'id: 'k,
    R: RoleKeys<'id, ModelTypes<X>>,
    K: Ieee80211Capabilities,
    X: ModelContext,
    C: ModelClass,
>(
    source: ModelSource<'k, 'p, 'id, C, K, R, X>,
) -> ModelPreparation<'k, 'p, 'id, R, K, X, C> {
    let checked = match &source {
        TxSource::Fresh {
            buffer,
            frame,
            body,
            protection,
        } => {
            let key = match protection {
                TxProtection::Open => None,
                TxProtection::Ccmp(key) => Some(R::tx_identity(key)),
            };
            let layout = MpduLayout::new(
                frame.frame(),
                body.as_ref().map_or(0, |body| body.bytes().len()),
                key.is_some(),
            );
            match buffer.preparation_refusal.take() {
                Some(reason) => Err(reason),
                None => layout.map(|layout| (layout, stages(frame.frame()), key)),
            }
        }
        TxSource::Retransmission { mpdu, plan } => {
            if mpdu.role != R::ROLE {
                return Err(PreparationRefused {
                    reason: PreparationError::RetransmissionKind,
                    operation: source,
                });
            }
            if mpdu.key.as_ref().is_some_and(|key| !key.0.active.get()) {
                Err(PreparationError::RetransmissionKey)
            } else {
                retry_stages(&mpdu.layout, plan).map(|plan| (mpdu.layout, plan, mpdu.key.clone()))
            }
        }
    };
    let (layout, plan, key) = match checked {
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
            ..
        } => {
            let mut buffer = C::erase(buffer);
            write_header(frame.frame(), buffer.memory_mut(), key.is_some());
            copy_body(&mut buffer, layout, body.as_ref());
            ModelMpdu {
                buffer,
                layout,
                body,
            }
        }
        TxSource::Retransmission { mpdu, .. } => ModelMpdu::from_sent(C::erase_sent(mpdu), layout),
    };
    Ok(ModelPrepared {
        mpdu,
        plan,
        key: key.map(PreparedKey::new),
        key_borrow: PhantomData,
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
    ) -> Result<(), Poisoned<ModelFault>> {
        if self.held.air.poisoned.get() {
            return Err(Poisoned { cause: ModelFault });
        }
        let ModelPrepared {
            mpdu, plan, key, ..
        } = attempt.source;
        self.held.command(Command::Submit(Box::new(Prepared {
            context: attempt.context,
            role: R::ROLE,
            ticket: attempt.ticket,
            mpdu,
            queue: attempt.access_category as usize,
            plan,
            key,
        })));
        Ok(())
    }

    fn install_pairwise_key(
        &mut self,
        slot: Slot<'id, ModelPairwiseKeys<R>>,
        _: IndividualAddress,
        _: Ccmp128Key,
    ) -> ModelPairwiseKey<'id, R> {
        let identity = KeyIdentity::new(slot.index(), 0, &self.held.air);
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
        let identity = KeyIdentity::new(slot.index(), key_id.get(), &self.held.air);
        self.held.command(Command::InstallKey {
            key: identity.clone(),
        });
        ModelGroupKey { identity, slot }
    }

    fn remove_pairwise_key(&mut self, key: ModelPairwiseKey<'id, R>) {
        key.identity.0.active.set(false);
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: R::removed(RemovedKey::Pairwise(key.slot)),
        });
    }

    fn remove_group_key(&mut self, key: ModelGroupKey<'id, R>) {
        key.identity.0.active.set(false);
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

/// Builder retaining receiver, TID, role and one installed protection.
/// Only `push_aggregate` can create the nonempty state accepted by submission.
///
/// Role, phase and install stay in the builder through submission:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// use oer_ieee80211_lower_mac::{Station, AccessPoint};
/// fn role<'k, 'p, 'id>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'p, 'id, AccessPoint> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// use oer_ieee80211_lower_mac::Station;
/// fn phase<'k, 'p, 'q, 'id>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'q, 'id, Station> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// use oer_ieee80211_lower_mac::Station;
/// fn install<'k, 'p, 'id, 'other>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'p, 'other, Station> { a }
/// ```
///
/// ```
/// use oer_ieee80211_runtime::lower_mac_owner::model::ModelAggregate;
/// fn same<'k, 'p, 'id, R>(a: ModelAggregate<'k, 'p, 'id, R>) -> ModelAggregate<'k, 'p, 'id, R> { a }
/// ```
pub struct ModelAggregate<'k, 'p, 'id, R, K = ModelCaps, X: ModelContext = (), S = EmptyAggregate> {
    subframes: Vec<Subframe<'id>>,
    receiver: IndividualAddress,
    tid: Tid,
    rate: AmpduRate<'p>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    caps: TypeProof<(R, K, X, S)>,
}
impl<'id, R, K, X: ModelContext, S> PhaseAggregate<'id>
    for ModelAggregate<'_, '_, 'id, R, K, X, S>
{
    type Types = ModelTypes<X>;
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
type ModelSubframe<'id, C, K> =
    SubframeSource<ModelBuffer<'id, C>, ModelSent<'id, C>, ClassSubframe<C, K>, ModelBody>;
type ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> = Result<
    ModelAggregate<'k, 'p, 'id, R, K, X, NonEmptyAggregate>,
    AggregateRefused<ModelAggregate<'k, 'p, 'id, R, K, X, S>, ModelSubframe<'id, C, K>>,
>;

#[expect(
    clippy::result_large_err,
    reason = "a refusal returns the whole operation by value"
)]
fn push_model<
    'k,
    'p,
    'id,
    R: oer_ieee80211_lower_mac::InterfaceRole,
    K: Ieee80211Capabilities,
    X: ModelContext,
    S,
    C: ModelClass,
>(
    mut aggregate: ModelAggregate<'k, 'p, 'id, R, K, X, S>,
    source: ModelSubframe<'id, C, K>,
) -> ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> {
    let checked = if aggregate.subframes.len() >= MODEL_AMPDU_SUBFRAMES {
        Err(PreparationError::AggregateFull)
    } else {
        match &source {
            SubframeSource::Fresh {
                buffer,
                frame,
                body,
            } => match buffer.preparation_refusal.take() {
                Some(reason) => Err(reason),
                None => MpduLayout::subframe(
                    frame,
                    body.as_ref().map_or(0, |body| body.bytes().len()),
                    aggregate.key.is_some(),
                    aggregate.receiver,
                    aggregate.tid,
                ),
            },
            SubframeSource::Sent(mpdu) => {
                if mpdu.key.as_ref() != aggregate.key.as_ref().map(|key| &key.identity)
                    || mpdu.key.as_ref().is_some_and(|key| !key.0.active.get())
                {
                    Err(PreparationError::RetransmissionKey)
                } else if mpdu.role != R::ROLE
                    || mpdu.layout.recipient() != Some((aggregate.receiver, aggregate.tid))
                {
                    Err(PreparationError::RetransmissionKind)
                } else {
                    Ok(mpdu.layout)
                }
            }
        }
    };
    let layout = match checked {
        Ok(layout) => layout,
        Err(reason) => {
            return Err(AggregateRefused {
                reason,
                aggregate,
                subframe: source,
            });
        }
    };
    let mpdu = match source {
        SubframeSource::Fresh {
            buffer,
            frame,
            body,
        } => {
            let mut buffer = C::erase(buffer);
            frame
                .header(aggregate.receiver, aggregate.tid, aggregate.key.is_some())
                .encode(buffer.memory_mut(), false);
            copy_body(&mut buffer, layout, body.as_ref());
            ModelMpdu {
                buffer,
                layout,
                body,
            }
        }
        SubframeSource::Sent(mpdu) => ModelMpdu::from_sent(C::erase_sent(mpdu), layout),
    };
    aggregate.subframes.push(Subframe { mpdu });
    let ModelAggregate {
        subframes,
        receiver,
        tid,
        rate,
        key,
        ..
    } = aggregate;
    Ok(ModelAggregate {
        subframes,
        receiver,
        tid,
        rate,
        key,
        key_borrow: PhantomData,
        caps: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> AmpduPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, X>
where
    R: RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        AmpduCapabilities {
            max_subframes: MODEL_AMPDU_SUBFRAMES as u16,
            formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
            max_length: 65_535,
        }
    }
    fn aggregate<'k>(
        &mut self,
        receiver: IndividualAddress,
        tid: Tid,
        rate: AmpduRate<'p>,
        protection: InterfaceProtection<'k, 'p, 'id, Self>,
    ) -> ModelAggregate<'k, 'p, 'id, R, K, X>
    where
        'id: 'k,
    {
        let key = match &protection {
            TxProtection::Open => None,
            TxProtection::Ccmp(key) => Some(R::tx_identity(key)),
        };
        ModelAggregate {
            subframes: Vec::new(),
            receiver,
            tid,
            rate,
            key: key.map(PreparedKey::new),
            key_borrow: PhantomData,
            caps: PhantomData,
        }
    }
    fn submit_ampdu<'k>(
        &mut self,
        attempt: InterfaceAggregateAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>>
    where
        'id: 'k,
    {
        if self.held.air.poisoned.get() {
            return Err(Poisoned { cause: ModelFault });
        }
        let ModelAggregate {
            subframes,
            tid,
            rate,
            key,
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
