//! An in-memory lower-MAC backend on the phase shape
//! (`oer_ieee80211_lower_mac::phase`), for host tests of the shape and of
//! the code that drives it.
//!
//! [`ModelBackend::new`] takes the install's channel and interface
//! addresses and returns the backend, which a test hands to
//! [`run_port`](oer_ieee80211_port::run_port), and the [`ModelAir`] it
//! plays on: the air answers each transmission, delivers received frames
//! and records what the hardware saw. Receive Block Ack agreements are
//! per-slot desired state: updates coalesce before the driver applies them,
//! without occupying the command queue. A configuration confirmation occupies
//! its coalescing slot until that event is taken, including by disable's drain.
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
//! Block Ack (every subframe by default). `ModelAir::answer` queues one
//! `AirAnswer` per publication: ACK, BA bitmap, missing response, collision or
//! hardware timeout. The checked frame determines whether it expects no
//! response, an ACK or a Block Ack (including a single BAR). A mismatched
//! response poisons before publication and
//! quarantines the rejected operation until teardown. There is no separate
//! BA queue whose injection could escape to a later aggregate.
//! The builder rejects duplicate sequence numbers and subframes outside the
//! 64-sequence BA window rooted at
//! its first subframe, using sequence arithmetic modulo 4096. Refusal returns
//! both the unchanged builder and the rejected subframe before publication.
//!
//! Each concrete class pool uses the portable `MpduLayout` proof before
//! writing headers or copying an external body. Its allocation is privately
//! minted at the class capacity; `ClassFrame` proves the MPDU fits it.
//! Small and large classes share one pool implementation. Their private
//! `ModelClass` dispatch only erases or recovers the appropriate owner
//! variant, returning every other class unchanged on refusal.
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
//! An aggregate preflights every MPDU and the entire fresh-PN range before
//! changing any subframe. A publication refusal retains the complete operation;
//! submission after poisoning likewise transfers its owners to teardown custody.
//!
//! `ModelContext` selects an opaque context for each install lifetime, so
//! a station context can retain its `LinkEpoch` brand through every queue
//! and terminal. The model grants one epoch-source slot in its resources.
//!
//! Capacity is the tokens': every terminal returns a ticket, so no terminal
//! is ever lost; received frames beyond [`MODEL_RX_EVENTS`] queued are
//! dropped and the stream says [`EventsLost`] once.
//! The host fixture's VecDeque cannot express the token/capacity equation in
//! its type. A broken equation retains the offending owner, poisons the
//! backend and ends the installation without delivering later events. A
//! received owner dropped without recycle likewise poisons this fixture.
//!
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560

//! Publication and completion are separate owners. Tests may hold a result
//! while posting gate/key/disable barriers; only that publication can finish.
//! The model's 1 ms publication bound is synthetic model time, not an S31
//! timing claim. `set_time` feeds the test's virtual clock. Hardware backends
//! must declare their own PPDU plus response/timeout bound.
//!
//! Unit tests and the `test-support` library use the same implementation,
//! including continuous RX and immediate recycling of its bounded slots. Only the unit-test module is under `cfg(test)`.
//! `tests/model_feature.rs` exercises these paths through the public port
//! from a separate test crate, so the model itself is compiled without that cfg.

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    num::NonZeroU16,
    task::{Poll, Waker},
};
use oer_ieee80211_mac::ccmp::GroupKeyId;
use oer_ieee80211_rsn::keys::CcmpKey;

use oer_ieee80211_mac::phy::{HeMcs, HtMcs, PhyRate};
use oer_ieee80211_port::{ClockError, EnableResult, PortBackend, PortPhase, Retire};
use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, Slot};
use oer_time::RadioInstant;

use oer_ieee80211_lower_mac::{
    AccessPoint, AccessPointPhase, BandSet, BssColor, Ccmp128Key, Channel, CheckedAggregate,
    CheckedPoolSource, CheckedSubframe, CheckedTxSource, CoexPriority, CoexPrioritySet,
    ConnectionEpochKind, ExecutionPayload, FreshSource, FreshSubframe, GateClosed, GateIssuer,
    GateOpen, GatePool, HardwareServices, Ieee80211Capabilities, Ieee80211Radio, InFlight,
    IndeterminateCause, InstalledGroup, InterfaceAggregateAttempt, InterfaceAttempt,
    InterfacePhase, KeyTypes, LinkMetric, LowerMacBackend, LowerMacCapabilities, LowerMacEvent,
    LowerMacPhase, LowerMacSetup, MpduKind, MpduLayout, PhaseRate, PhaseTypes, PhyFormatSet,
    PreparationError, PreparationRefused, PreparedTx, RateSupport, ReceiveFilter, Removed,
    RemovedKey, RetransmissionPlan, RoleKeys, RxBuffer, RxEvidence, RxMeta, RxMetadata,
    SignalMetadata, Station, StationPhase, StationReceive, SupportedBlockAck, SupportedChannel,
    SupportedEdca, SupportedFilter, TxAck, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool,
    TxCompleted, TxEndStatus, TxFrame, TxMpdu, TxOutcome, TxProtection, TxSource, VifRole,
    WidthSet, WithdrawCause,
};
use oer_ieee80211_lower_mac::{
    AggregateRefused, AggregateState, AmpduCapabilities, AmpduPhase, AmpduRate, BlockAckReport,
    ClassSubframe, EmptyAggregate, InterfaceProtection, NonEmptyAggregate, PhaseAggregate,
    SubframeSource, Tid,
};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_mac::qos::WmmAccessCategory;
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

/// FIFO commands: each ticket, install/remove for every pairwise slot,
/// one GTK mutation per interface permission, and close/open of the gate.
/// Receive agreements and configurations are coalesced settings.
pub const MODEL_COMMANDS: usize = MODEL_TICKETS + 2 * (2 * MODEL_KEY_SLOTS) + 2 + 2;
/// Reserved terminals: tickets, pairwise removals, the two GTK permissions,
/// gate closing, two configuration confirmations and the bounded RX owners.
pub const MODEL_EVENTS: usize = MODEL_TICKETS + 2 * MODEL_KEY_SLOTS + 2 + 1 + 2 + MODEL_RX_EVENTS;

/// A queue of the contract's capacity: an overflow is a broken promise and
/// poisons the install while retaining every owner.
struct Bounded<T> {
    items: RefCell<VecDeque<T>>,
    capacity: usize,
    name: &'static str,
}

struct QueueCapacityViolation {
    queue: &'static str,
    capacity: usize,
}
impl core::fmt::Debug for QueueCapacityViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("QueueCapacityViolation")
            .field("queue", &self.queue)
            .field("capacity", &self.capacity)
            .finish()
    }
}
impl<T> Bounded<T> {
    fn new(capacity: usize, name: &'static str) -> Self {
        Self {
            items: RefCell::new(VecDeque::with_capacity(capacity)),
            capacity,
            name,
        }
    }

    fn push(&self, item: T) -> Result<(), QueueCapacityViolation> {
        let mut items = self.items.borrow_mut();
        let failure = (items.len() >= self.capacity).then_some(QueueCapacityViolation {
            queue: self.name,
            capacity: self.capacity,
        });
        // On a backend accounting violation retain every owner in this same
        // queue for fail-stop. No terminal or command is dropped or delivered
        // after the caller latches poisoning. Healthy queues stay bounded by
        // their tickets/slots; Rust cannot connect a VecDeque length to them.
        items.push_back(item);
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
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

/// Paths of the model's fixed installation layout. They add no runtime state.
pub type ModelPool<const I: u8> = oer_radio_port::Child<oer_radio_port::Root, I>;

/// The distinct key pools belonging to each modeled interface role.
pub trait ModelRole: oer_ieee80211_lower_mac::InterfaceRole {
    /// Component containing the role's pairwise slots.
    type PairwisePath: oer_radio_port::PoolPath;
    /// Component containing the role's group slots.
    type GroupPath: oer_radio_port::PoolPath;
    /// Component containing the unique GTK mutation permission.
    type GroupPermissionPath: oer_radio_port::PoolPath;
}
impl ModelRole for Station {
    type PairwisePath = ModelPool<5>;
    type GroupPath = ModelPool<6>;
    type GroupPermissionPath = ModelPool<10>;
}
impl ModelRole for AccessPoint {
    type PairwisePath = ModelPool<7>;
    type GroupPath = ModelPool<8>;
    type GroupPermissionPath = ModelPool<11>;
}

/// The kind of the pairwise key slots of the role `R`.
pub struct ModelPairwiseKeys<R>(PhantomData<fn() -> R>);
/// The kind of the group key slots of the role `R`.
pub struct ModelGroupKeys<R>(PhantomData<fn() -> R>);
/// The kind of the one GTK mutation permission per interface.
pub struct ModelGroupPermission<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots.
pub enum ModelBlockAcks {}

/// A transmit buffer of the class `C`: its token and its memory, which
/// moves with it; only the install mints one.
pub struct ModelBuffer<'id, C: ModelClass> {
    slot: Slot<'id, C, C::Path>,
    memory: ModelMemory,
}

enum ModelMemory {
    Small(Box<[u8; ModelSmall::CAPACITY]>),
    Large(Box<[u8; ModelLarge::CAPACITY]>),
}
impl core::ops::Deref for ModelMemory {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Small(bytes) => bytes.as_slice(),
            Self::Large(bytes) => bytes.as_slice(),
        }
    }
}
impl core::ops::DerefMut for ModelMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Small(bytes) => bytes.as_mut_slice(),
            Self::Large(bytes) => bytes.as_mut_slice(),
        }
    }
}
impl AsRef<[u8]> for ModelMemory {
    fn as_ref(&self) -> &[u8] {
        self
    }
}
/// Retry is bit 11 of Frame Control, bit 3 of its second wire octet.
const RETRY_FLAG: u8 = 1 << 3;

impl ModelMemory {
    fn sequence(&self) -> SequenceNumber {
        let control = match self {
            Self::Small(bytes) => {
                let [
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    a,
                    b,
                    ..,
                ] = bytes.as_ref();
                [*a, *b]
            }
            Self::Large(bytes) => {
                let [
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    a,
                    b,
                    ..,
                ] = bytes.as_ref();
                [*a, *b]
            }
        };
        SequenceNumber::from_sequence_control(u16::from_le_bytes(control))
    }
    fn retry(&mut self) {
        match self {
            Self::Small(bytes) => {
                let [_, flags, ..] = bytes.as_mut();
                *flags |= RETRY_FLAG;
            }
            Self::Large(bytes) => {
                let [_, flags, ..] = bytes.as_mut();
                *flags |= RETRY_FLAG;
            }
        }
    }
}

impl<C: ModelClass> TxBufferMemory for ModelBuffer<'_, C> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }
}

impl<C: ModelClass> core::fmt::Debug for ModelBuffer<'_, C> {
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
pub struct ModelSent<'id, C: ModelClass> {
    role: VifRole,
    buffer: ModelBuffer<'id, C>,
    layout: MpduLayout,
    response: ResponsePolicy,
    key: Option<KeyIdentity>,
    body: Option<ModelBody>,
    packet_number: Option<u64>,
}

impl<'id, C: ModelClass> ModelSent<'id, C> {
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
    fn sequence(&self) -> SequenceNumber {
        match self {
            Self::Small(buffer) => buffer.memory.sequence(),
            Self::Large(buffer) => buffer.memory.sequence(),
            Self::SmallSent(mpdu) => mpdu.buffer.memory.sequence(),
            Self::LargeSent(mpdu) => mpdu.buffer.memory.sequence(),
        }
    }
    fn retry(&mut self) {
        match self {
            Self::Small(buffer) => buffer.memory.retry(),
            Self::Large(buffer) => buffer.memory.retry(),
            Self::SmallSent(mpdu) => mpdu.buffer.memory.retry(),
            Self::LargeSent(mpdu) => mpdu.buffer.memory.retry(),
        }
    }

    fn memory(&self) -> &[u8] {
        match self {
            Self::Small(buffer) => &buffer.memory,
            Self::Large(buffer) => &buffer.memory,
            Self::SmallSent(mpdu) => &mpdu.buffer.memory,
            Self::LargeSent(mpdu) => &mpdu.buffer.memory,
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
    group: Option<GroupMaterial>,
    rx_active: Cell<bool>,
    preparations: Rc<()>,
    air: Weak<ModelAir>,
}

#[derive(Debug)]
struct GroupMaterial {
    role: VifRole,
    key: CcmpKey,
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
            group: None,
            rx_active: Cell::new(false),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
        }))
    }
    fn group(
        index: u8,
        key_id: GroupKeyId,
        role: VifRole,
        key: CcmpKey,
        air: &Rc<ModelAir>,
    ) -> Self {
        Self(Rc::new(KeyInstall {
            index,
            key_id: key_id.get(),
            last: Cell::new(0),
            active: Cell::new(true),
            group: Some(GroupMaterial { role, key }),
            rx_active: Cell::new(false),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
        }))
    }
    fn check_packet_numbers(&self, count: usize) -> Result<(), ModelFault> {
        let count = u64::try_from(count).map_err(|_| ModelFault)?;
        self.0
            .last
            .get()
            .checked_add(count)
            .filter(|pn| *pn <= oer_ieee80211_mac::ccmp::CCMP_PACKET_NUMBER_MAX)
            .ok_or(ModelFault)?;
        Ok(())
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

/// One closed MPDU, retaining its memory, external body and publication state.
pub struct ModelMpdu<'id> {
    buffer: ModelAnyBuffer<'id>,
    layout: MpduLayout,
    response: ResponsePolicy,
    // Fresh memory keeps its body here, including during publication. Only
    // consuming the terminal moves both into the returned Sent owner.
    body: Option<ModelBody>,
    published: Option<PublishedMpdu>,
}

struct PublishedMpdu {
    role: VifRole,
    key: Option<KeyIdentity>,
    packet_number: Option<u64>,
}

/// Execution access cannot replace or take the MPDU owner.
pub struct ModelMpduExecution<'a, 'id> {
    mpdu: &'a mut ModelMpdu<'id>,
}
impl<'id> ExecutionPayload for ModelMpdu<'id> {
    type Execution<'a>
        = ModelMpduExecution<'a, 'id>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelMpduExecution { mpdu: self }
    }
}
impl ModelMpduExecution<'_, '_> {
    fn publish(&mut self, role: VifRole, key: Option<&KeyIdentity>) -> Result<(), ModelFault> {
        let mpdu = &mut *self.mpdu;
        mpdu.preflight(key.is_some())?;
        if mpdu.published.is_some() {
            return Ok(());
        }
        let memory = match &mut mpdu.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.bytes_mut(),
            ModelAnyBuffer::Large(buffer) => buffer.bytes_mut(),
            ModelAnyBuffer::SmallSent(_) | ModelAnyBuffer::LargeSent(_) => return Ok(()),
        };
        let packet_number = match key {
            Some(key) => {
                let tail = memory
                    .split_at_mut_checked(mpdu.layout.header_len())
                    .ok_or(ModelFault)?
                    .1;
                let (header, _) = tail.split_first_chunk_mut::<8>().ok_or(ModelFault)?;
                let pn = key.allocate()?;
                *header = ccmp_header(pn, key.0.key_id);
                Some(pn)
            }
            None => None,
        };
        mpdu.published = Some(PublishedMpdu {
            role,
            key: key.cloned(),
            packet_number,
        });
        Ok(())
    }
    fn retry(&mut self) {
        self.mpdu.buffer.retry();
    }
}
impl<'id> TxMpdu for ModelMpdu<'id> {
    type Buffer = ModelAnyBuffer<'id>;
    type Body = ModelBody;
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>) {
        let Self {
            buffer,
            layout,
            response,
            body,
            published,
        } = self;
        match (buffer, published) {
            (
                ModelAnyBuffer::Small(buffer),
                Some(PublishedMpdu {
                    role,
                    key,
                    packet_number,
                }),
            ) => (
                ModelAnyBuffer::SmallSent(ModelSent {
                    role,
                    buffer,
                    layout,
                    response,
                    key,
                    body,
                    packet_number,
                }),
                None,
            ),
            (
                ModelAnyBuffer::Large(buffer),
                Some(PublishedMpdu {
                    role,
                    key,
                    packet_number,
                }),
            ) => (
                ModelAnyBuffer::LargeSent(ModelSent {
                    role,
                    buffer,
                    layout,
                    response,
                    key,
                    body,
                    packet_number,
                }),
                None,
            ),
            (buffer, _) => (buffer, body),
        }
    }
}
impl<'id> ModelMpdu<'id> {
    /// Validate all fallible geometry before any PN or publication state changes.
    /// Only a fresh protected MPDU consumes a new PN; Sent retries retain theirs.
    fn preflight(&self, protected: bool) -> Result<usize, ModelFault> {
        let memory = self.buffer.memory();
        memory
            .split_at_checked(self.layout.mpdu_len())
            .ok_or(ModelFault)?;
        let fresh = self.published.is_none()
            && matches!(
                self.buffer,
                ModelAnyBuffer::Small(_) | ModelAnyBuffer::Large(_)
            );
        if protected && fresh {
            memory
                .split_at_checked(self.layout.header_len())
                .ok_or(ModelFault)?
                .1
                .first_chunk::<8>()
                .ok_or(ModelFault)?;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn from_sent(
        mut buffer: ModelAnyBuffer<'id>,
        layout: MpduLayout,
        response: ResponsePolicy,
    ) -> Self {
        buffer.retry();
        Self {
            buffer,
            layout,
            response,
            body: None,
            published: None,
        }
    }
    fn on_air(
        &self,
        role: VifRole,
        keys: &[KeyIdentity],
        key: Option<&KeyIdentity>,
        rate: PhyRate,
    ) -> Result<AirFrame, ModelFault> {
        let bytes = self
            .buffer
            .memory()
            .split_at_checked(self.layout.mpdu_len())
            .ok_or(ModelFault)?
            .0;
        Ok(AirFrame {
            role,
            frame: bytes.into(),
            rate,
            key: key.map(|key| (key.0.index, keys.contains(key))),
            packet_number: self
                .published
                .as_ref()
                .and_then(|state| state.packet_number)
                .or_else(|| self.buffer.packet_number()),
        })
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
    type EpochPath = ModelPool<4>;
    type GatePath = ModelPool<0>;
    type TicketPath = ModelPool<1>;
    type BlockAckPath = ModelPool<9>;
    type Fault = ModelFault;
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
    type TxMpdu = ModelMpdu<'id>;
    type TxAggregate = ModelSubframes<'id>;
    type RxBuffer = ModelRxBuffer;
    type RxMeta = ModelRxMeta;
    type StationKeys = ModelKeys<'id, Station>;
    type AccessPointKeys = ModelKeys<'id, AccessPoint>;
    type BlockAckKind = ModelBlockAcks;
    type BlockAck = ModelBlockAck<'id>;
}

/// The keys of the role `R` and their slots.
pub struct ModelKeys<'id, R>(PhantomData<fn(&'id ()) -> R>);

impl<'id, R: ModelRole + 'id> KeyTypes<'id> for ModelKeys<'id, R> {
    type PairwiseSlot = Slot<'id, ModelPairwiseKeys<R>, R::PairwisePath>;
    type GroupSlot = Slot<'id, ModelGroupKeys<R>, R::GroupPath>;
    type PairwiseKey = ModelPairwiseKey<'id, R>;
    type GroupKey = ModelGroupKey<'id, R>;
    type GroupPermission = Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>;
    type Identity = KeyIdentity;
    type GroupSlotIdentity = u8;
    fn pairwise_identity(key: &Self::PairwiseKey) -> KeyIdentity {
        key.identity.clone()
    }
    fn group_identity(key: &Self::GroupKey) -> KeyIdentity {
        key.identity.clone()
    }
    fn group_slot_identity(key: &Self::GroupKey) -> u8 {
        key.slot.index()
    }
    fn vacant_group_slot_identity(slot: &Self::GroupSlot) -> u8 {
        slot.index()
    }
}

impl<'id, X: ModelContext, C: ModelClass> TxBufferPool<'id, C> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, C>;
    type Sent = ModelSent<'id, C>;

    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> oer_ieee80211_lower_mac::PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
    }

    fn push_aggregate<'k, 'p, R, K, S>(
        operation: oer_ieee80211_lower_mac::CheckedPoolAggregate<'k, 'p, 'id, Self, C, R, K, S>,
    ) -> oer_ieee80211_lower_mac::AggregatePushResult<'k, 'p, 'id, Self, C, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        push_model(operation)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id> {
        C::erase(buffer)
    }

    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent(buffer: Self::Sent) -> ModelAnyBuffer<'id> {
        C::erase_sent(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id>) -> Result<Self::Buffer, ModelAnyBuffer<'id>> {
        C::class(buffer)
    }

    fn sent(buffer: ModelAnyBuffer<'id>) -> Result<Self::Sent, ModelAnyBuffer<'id>> {
        C::sent(buffer)
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
/// [`LowerMacPhase::recycle`]: dropping it instead poisons the backend.
#[derive(Debug)]
pub struct ModelRxBuffer {
    bytes: Vec<u8>,
    recycled: bool,
    air: Weak<ModelAir>,
    streamed: bool,
}

impl RxBuffer for ModelRxBuffer {
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for ModelRxBuffer {
    fn drop(&mut self) {
        if !self.recycled
            && let Some(air) = self.air.upgrade()
        {
            // A dropped affine receive owner cannot be recycled or admitted
            // again. Signal the existing backend fail-stop, never a panic.
            air.poison();
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
    pub connection_epoch: Slot<'id, ConnectionEpochKind, ModelPool<4>>,
    /// The transmit tickets.
    pub tickets: Vec<Slot<'id, ModelTickets, ModelPool<1>>>,
    /// The small transmit buffers.
    pub small: Vec<ModelBuffer<'id, ModelSmall>>,
    /// The large transmit buffers.
    pub large: Vec<ModelBuffer<'id, ModelLarge>>,
    /// The station's pairwise key slots.
    pub station_pairwise: [Slot<'id, ModelPairwiseKeys<Station>, ModelPool<5>>; MODEL_KEY_SLOTS],
    /// The station's group key slots.
    pub station_group: [Slot<'id, ModelGroupKeys<Station>, ModelPool<6>>; MODEL_KEY_SLOTS],
    /// The access point's pairwise key slots.
    pub access_point_pairwise:
        [Slot<'id, ModelPairwiseKeys<AccessPoint>, ModelPool<7>>; MODEL_KEY_SLOTS],
    /// The access point's group key slots.
    pub access_point_group: [Slot<'id, ModelGroupKeys<AccessPoint>, ModelPool<8>>; MODEL_KEY_SLOTS],
    /// The station's sole GTK mutation permission.
    pub station_group_permission: Slot<'id, ModelGroupPermission<Station>, ModelPool<10>>,
    /// The access point's sole GTK mutation permission.
    pub access_point_group_permission: Slot<'id, ModelGroupPermission<AccessPoint>, ModelPool<11>>,
    /// The receive Block Ack slots both roles share.
    pub block_acks: [Slot<'id, ModelBlockAcks, ModelPool<9>>; MODEL_BLOCK_ACK_SLOTS],
    /// The open transmit gate.
    pub gate: GateOpen<'id, ModelPool<0>>,
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

/// One FIFO injection for the next PPDU. ACK and Block Ack must match its
/// shape; a mismatch poisons the model before publication, retaining owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirAnswer {
    Ack,
    /// Bit n acknowledges subframe n of this one aggregate.
    BlockAck(u64),
    NoResponse,
    Collision,
    HardwareTimeout,
}

/// One receive agreement applied by the model driver, after capability checking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelReceiveAgreement {
    /// The receiving interface.
    pub role: VifRole,
    /// The originator.
    pub peer: IndividualAddress,
    /// The receive sequence space.
    pub tid: Tid,
    /// The window's start.
    pub start_sequence: SequenceNumber,
    /// The receive window in MPDUs.
    pub window: u16,
}

impl ModelReceiveAgreement {
    fn checked<K: Ieee80211Capabilities>(role: VifRole, agreement: SupportedBlockAck<K>) -> Self {
        let (peer, tid, start_sequence, window) = agreement.get();
        Self {
            role,
            peer,
            tid,
            start_sequence,
            window,
        }
    }
}

/// Hardware uncertainty injected at a concrete mutation boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupFaultStage {
    Before,
    After,
}

/// A receive fact deliberately delayed in the modeled ingress pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupRxStage {
    Dma,
    Interrupt,
    Software,
    Ready,
}

struct CapturedGroupRx {
    frame: Vec<u8>,
    meta: ModelRxMeta,
    installation: KeyIdentity,
    stage: GroupRxStage,
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
#[derive(Default)]
pub struct ModelAir {
    sent: RefCell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: RefCell<VecDeque<AirAnswer>>,
    rx: RefCell<VecDeque<(Vec<u8>, ModelRxMeta)>>,
    rx_stream: RefCell<Option<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<bool>,
    group_failure: Cell<Option<GroupFaultStage>>,
    group_held: Cell<bool>,
    group_installations: RefCell<Vec<Weak<KeyInstall>>>,
    group_rx: RefCell<VecDeque<CapturedGroupRx>>,
    group_writes: Cell<usize>,
    poisoned: Cell<bool>,
    driver: RefCell<Option<Waker>>,
    /// The driver's passes.
    passes: Cell<u32>,
    /// The commands the driver took, of every kind.
    commands: Cell<u32>,
    /// The settings the driver applied.
    settings: Cell<u32>,
    /// The queues the air keeps busy: their attempts wait unpublished.
    held: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    /// Keep a published PPDU's result pending, independently of prepublication CCA.
    held_results: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    now_micros: Cell<u64>,
    receive_agreements: RefCell<BTreeMap<u8, ModelReceiveAgreement>>,
}

impl ModelAir {
    /// Inject uncertainty before or after a GTK hardware mutation. No success
    /// owner or ordinary permission leaves the poisoned installation.
    pub fn fail_next_group_change(&self, stage: GroupFaultStage) {
        self.group_failure.set(Some(stage));
        self.wake_driver();
    }

    /// Delay commands while RX continues. `set_time` supplies the deadline
    /// wake; the driver poisons at GROUP_COMMAND_BOUND. Normal disable finishes
    /// admitted commands instead of waiting for this test hold to be released.
    pub fn hold_group_commands(&self, held: bool) {
        self.group_held.set(held);
        self.wake_driver();
    }

    /// Actual material writes, including replacement and excluding selection.
    pub fn group_writes(&self) -> usize {
        self.group_writes.get()
    }

    /// Capture a group frame against the current modeled hardware key table.
    /// Delayed DMA/ISR/software stages retain the original installation inside
    /// the backend; no installation identity is added to public RX metadata.
    pub fn receive_group(
        &self,
        frame: &[u8],
        meta: ModelRxMeta,
        role: VifRole,
        key_id: GroupKeyId,
        material: &[u8; 16],
        stage: GroupRxStage,
    ) -> bool {
        let installation =
            self.group_installations
                .borrow()
                .iter()
                .filter_map(Weak::upgrade)
                .find(|key| {
                    key.active.get()
                        && key.rx_active.get()
                        && key.key_id == key_id.get()
                        && key.group.as_ref().is_some_and(|group| {
                            group.role == role && group.key.as_bytes() == material
                        })
                });
        let Some(installation) = installation else {
            return false;
        };
        self.group_rx.borrow_mut().push_back(CapturedGroupRx {
            frame: frame.into(),
            meta,
            installation: KeyIdentity(installation),
            stage,
        });
        self.wake_driver();
        true
    }

    /// Every publication so far, including collisions and unknown outcomes.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.borrow().clone()
    }

    /// Supply exactly one result for the next publication. A Block Ack cannot
    /// be left behind for a later aggregate when a Single is ahead of it.
    pub fn answer(&self, answer: AirAnswer) {
        self.answers.borrow_mut().push_back(answer);
    }

    /// Applied receive agreements by hardware slot. Pending writes remain
    /// invisible until the driver takes the latest value of each slot.
    pub fn receive_agreements(&self) -> Vec<(u8, ModelReceiveAgreement)> {
        self.receive_agreements
            .borrow()
            .iter()
            .map(|(&slot, &agreement)| (slot, agreement))
            .collect()
    }

    /// Keep reception ready on every service poll, like an RX interrupt
    /// source on another executor. This exercises scheduling against a
    /// continuously ready source, rather than a finite queued burst.
    pub fn receive_continuously(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx_stream.borrow_mut().replace((frame.into(), meta));
        self.wake_driver();
    }

    /// Stop generating RX events. Already returned buffers still require
    /// recycling through the phase.
    pub fn stop_continuous_receive(&self) {
        self.rx_stream.borrow_mut().take();
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
        *access_category.select_mut(&mut queues) = held;
        self.held.set(queues);
        self.wake_driver();
    }

    /// Delay the current publication's result until released or its bound
    /// expires. A queued successor cannot pass this publication.
    pub fn hold_completion(&self, category: oer_ieee80211_mac::qos::WmmAccessCategory, held: bool) {
        let mut results = self.held_results.get();
        *category.select_mut(&mut results) = held;
        self.held_results.set(results);
        self.wake_driver();
    }
    /// Set model time from the test's monotonic virtual clock. A held result
    /// becomes a hardware timeout at the backend's publication bound.
    pub fn set_time(&self, now: oer_time::Instant) {
        self.now_micros.set(now.as_micros());
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

    fn result(&self, response: ResponsePolicy) -> Result<AirAnswer, AnswerShapeMismatch> {
        let answer = self
            .answers
            .borrow_mut()
            .pop_front()
            .unwrap_or(match response {
                ResponsePolicy::NoResponseExpected => AirAnswer::NoResponse,
                ResponsePolicy::Ack => AirAnswer::Ack,
                ResponsePolicy::BlockAck { .. } => AirAnswer::BlockAck(u64::MAX),
            });
        match (response, answer) {
            (ResponsePolicy::NoResponseExpected, AirAnswer::Ack | AirAnswer::BlockAck(_))
            | (ResponsePolicy::Ack, AirAnswer::BlockAck(_))
            | (ResponsePolicy::BlockAck { .. }, AirAnswer::Ack) => Err(AnswerShapeMismatch),
            _ => Ok(answer),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ResponsePolicy {
    NoResponseExpected,
    Ack,
    BlockAck { start_sequence: SequenceNumber },
}

impl ResponsePolicy {
    fn of_frame<K>(frame: &TxFrame<'_, '_, K>) -> Self {
        match frame {
            TxFrame::Individual { .. } => Self::Ack,
            TxFrame::IndividualNoAck { .. } | TxFrame::Group { .. } => Self::NoResponseExpected,
            TxFrame::BlockAckRequest {
                starting_sequence, ..
            } => Self::BlockAck {
                start_sequence: *starting_sequence,
            },
        }
    }
}
#[derive(Debug)]
struct AnswerShapeMismatch;

/// A command the handle sends the driver, in the order it made it.
/// The host model boxes its larger single-operation owner so small commands
/// do not reserve that space; the box moves opaque context without cloning.
enum Command<'id, X: ModelContext> {
    Submit(Box<Prepared<'id, X>>),
    SubmitAmpdu(PreparedAggregate<'id, X>),
    InstallKey {
        key: KeyIdentity,
    },
    Group {
        admitted_at: u64,
        change: GroupChange<'id, X>,
    },
    RemoveKey {
        key: KeyIdentity,
        removed: Removed<'id, ModelTypes<X>>,
    },
    CloseGate(GateOpen<'id, ModelPool<0>>),
    OpenGate,
}

type StationGroupPermission<'id> = Slot<'id, ModelGroupPermission<Station>, ModelPool<10>>;

/// Whole command custody, including permission, material and physical owner.
enum GroupChange<'id, X: ModelContext> {
    Install(InstalledGroup<'id, ModelTypes<X>>),
    Replace {
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
        key_id: GroupKeyId,
        key: CcmpKey,
    },
    Select {
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
    },
    Remove {
        key: KeyIdentity,
        removed: Removed<'id, ModelTypes<X>>,
    },
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
    /// Checked at admission against the first subframe's BA window.
    acknowledgement_bit: u64,
}

/// Complete aggregate storage; each subframe retains its own MPDU/body.
pub struct ModelSubframes<'id>(Vec<Subframe<'id>>);
impl ModelSubframes<'_> {
    /// Number of complete subframes returned by a terminal or cancellation.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// An empty builder can be cancelled before it accepts any subframe.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
/// Owning iteration is available after terminal consumption or cancellation.
pub struct ModelSubframeIter<'id>(alloc::vec::IntoIter<Subframe<'id>>);
impl ModelSubframeIter<'_> {
    /// Whether cancellation or terminal iteration has any MPDUs left.
    pub fn is_empty(&self) -> bool {
        self.0.as_slice().is_empty()
    }
}
impl<'id> Iterator for ModelSubframeIter<'id> {
    type Item = ModelMpdu<'id>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|subframe| subframe.mpdu)
    }
}
impl<'id> IntoIterator for ModelSubframes<'id> {
    type Item = ModelMpdu<'id>;
    type IntoIter = ModelSubframeIter<'id>;
    fn into_iter(self) -> Self::IntoIter {
        ModelSubframeIter(self.0.into_iter())
    }
}
/// Execution lends subframe storage without exposing replaceable owners.
pub struct ModelSubframesExecution<'a, 'id> {
    subframes: &'a mut [Subframe<'id>],
}
impl<'id> ExecutionPayload for ModelSubframes<'id> {
    type Execution<'a>
        = ModelSubframesExecution<'a, 'id>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelSubframesExecution {
            subframes: &mut self.0,
        }
    }
}
impl ModelSubframesExecution<'_, '_> {
    fn publish(
        &mut self,
        role: VifRole,
        keys: &[KeyIdentity],
        key: Option<&KeyIdentity>,
        rate: PhyRate,
    ) -> Result<Vec<AirFrame>, ModelFault> {
        let fresh = self.subframes.iter().try_fold(0_usize, |count, subframe| {
            count
                .checked_add(subframe.mpdu.preflight(key.is_some())?)
                .ok_or(ModelFault)
        })?;
        if let Some(key) = key {
            key.check_packet_numbers(fresh)?;
        }
        // This driver step is synchronous and exclusively owns PN allocation.
        // No other publication can consume the checked range before this loop.
        let mut frames = Vec::with_capacity(self.subframes.len());
        for subframe in &mut *self.subframes {
            subframe.mpdu.execution().publish(role, key)?;
            frames.push(subframe.mpdu.on_air(role, keys, key, rate)?);
        }
        Ok(frames)
    }
}

type ModelInFlight<'id, X, M> =
    InFlight<<X as ModelContext>::Context<'id>, Slot<'id, ModelTickets, ModelPool<1>>, M>;

/// An A-MPDU attempt prepared for its queue.
struct PreparedAggregate<'id, X: ModelContext> {
    operation: ModelInFlight<'id, X, ModelSubframes<'id>>,
    role: VifRole,
    start_sequence: SequenceNumber,
    queue: oer_ieee80211_mac::qos::WmmAccessCategory,
    rate: PhyRate,
    key: Option<PreparedKey>,
}

/// An attempt prepared for its queue.
struct Prepared<'id, X: ModelContext> {
    operation: ModelInFlight<'id, X, ModelMpdu<'id>>,
    role: VifRole,
    queue: oer_ieee80211_mac::qos::WmmAccessCategory,
    plan: oer_ieee80211_lower_mac::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
}

/// The settings the handle writes and the driver takes before its next
/// command; the latest value of each kind wins.
#[derive(Default)]
struct Settings {
    station: bool,
    access_point: bool,
    other: bool,
    /// Minted slots bound the keys to MODEL_BLOCK_ACK_SLOTS. Repeated writes
    /// replace a slot's pending value instead of occupying command capacity.
    block_acks: BTreeMap<u8, Option<ModelReceiveAgreement>>,
}

/// The install's queues between the handle and the driver. RX capacity stays
/// charged until recycle, including buffers handed to the body by disable.
/// Disable preserves an unobserved EventsLost indication for the next enable;
/// draining events does not acknowledge that separate loss indication.
pub struct ModelHeld<'id, X: ModelContext = ()> {
    air: Rc<ModelAir>,
    driver_state: RefCell<Driver<'id, X>>,
    commands: Bounded<Command<'id, X>>,
    /// The received buffers the body gives back, at most one per buffer.
    recycled: Bounded<ModelRxBuffer>,
    settings: RefCell<Settings>,
    events: Bounded<LowerMacEvent<'id, ModelTypes<X>>>,
    quarantined: Bounded<Work<'id, X>>,
    failed_group: Bounded<GroupChange<'id, X>>,
    failed_group_terminal: Bounded<LowerMacEvent<'id, ModelTypes<X>>>,
    /// The receive buffers out with the body, queued or held.
    rx_out: Cell<usize>,
    /// A confirmation of each interface's settings waits in `events`.
    configured: Cell<[bool; 2]>,
    /// Received frames dropped since the stream last said so.
    lost: Cell<bool>,
    body: RefCell<Option<Waker>>,
    /// The backend alone attests the hardware barrier with the one gate place.
    gate_issuer: RefCell<GateIssuer<'id, ModelPool<0>>>,
    /// Disable asks the driver to return all queued operations before the
    /// owner ends the phase. Unlike poison, this is an ordinary terminal.
    draining: Cell<bool>,
}

impl<'id, X: ModelContext> ModelHeld<'id, X> {
    fn command(&self, command: Command<'id, X>) {
        if self.commands.push(command).is_err() {
            self.poison();
        }
        self.air.wake_driver();
    }

    fn post(&self, event: LowerMacEvent<'id, ModelTypes<X>>) {
        if self.events.push(event).is_err() {
            self.poison();
        }
        self.wake_body();
    }

    /// Confirm `role`'s settings, once while a confirmation waits.
    fn configured(&self, role: VifRole) {
        let mut waiting = self.configured.get();
        let [station, access_point] = &mut waiting;
        let slot = match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        };
        if !*slot {
            *slot = true;
            self.configured.set(waiting);
            self.post(LowerMacEvent::Configured(role));
        }
    }

    /// Release coalescing admission with the individual event, including when
    /// disable drains it. A later configuration can then produce a new event.
    fn take_event(&self) -> Option<LowerMacEvent<'id, ModelTypes<X>>> {
        let event = self.events.pop()?;
        if let LowerMacEvent::Configured(role) = event {
            let mut waiting = self.configured.get();
            let [station, access_point] = &mut waiting;
            *match role {
                VifRole::Station => station,
                VifRole::AccessPoint => access_point,
            } = false;
            self.configured.set(waiting);
        }
        Some(event)
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
    type Pools = (
        GatePool,
        oer_radio_port::Slots<ModelTickets, MODEL_TICKETS>,
        oer_radio_port::Slots<ModelSmall, MODEL_SMALL_BUFFERS>,
        oer_radio_port::Slots<ModelLarge, MODEL_LARGE_BUFFERS>,
        oer_radio_port::Slots<ConnectionEpochKind, 1>,
        oer_radio_port::Slots<ModelPairwiseKeys<Station>, MODEL_KEY_SLOTS>,
        oer_radio_port::Slots<ModelGroupKeys<Station>, MODEL_KEY_SLOTS>,
        oer_radio_port::Slots<ModelPairwiseKeys<AccessPoint>, MODEL_KEY_SLOTS>,
        oer_radio_port::Slots<ModelGroupKeys<AccessPoint>, MODEL_KEY_SLOTS>,
        oer_radio_port::Slots<ModelBlockAcks, MODEL_BLOCK_ACK_SLOTS>,
        oer_radio_port::Slots<ModelGroupPermission<Station>, 1>,
        oer_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
    );
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
            while let Some(event) = held.take_event() {
                sink(event);
            }
            // Draining transfers RX owners to the body, not back to the pool.
            // Only recycle releases capacity. An unobserved `lost` indication
            // likewise survives disable and is delivered after re-enable.
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
        mint: Mint<'id, Self::Pools>,
    ) -> (ModelHeld<'id, X>, ModelLocal<'id>, ModelResources<'id>) {
        let (
            (gate, gate_issuer),
            tickets,
            small,
            large,
            [epoch_slot],
            station_pairwise,
            station_group,
            access_point_pairwise,
            access_point_group,
            block_acks,
            [station_group_permission],
            [access_point_group_permission],
        ) = mint.into_pools();
        let held = ModelHeld {
            air: self.air.clone(),
            driver_state: RefCell::new(Driver {
                queues: core::array::from_fn(|_| TxQueue {
                    waiting: VecDeque::new(),
                    air: None,
                }),
                keys: Vec::new(),
                removing: Vec::new(),
                gate: DriverGate::Open,
            }),
            commands: Bounded::new(MODEL_COMMANDS, "command"),
            recycled: Bounded::new(MODEL_RX_EVENTS, "recycle"),
            settings: RefCell::default(),
            events: Bounded::new(MODEL_EVENTS, "event"),
            quarantined: Bounded::new(1, "answer shape rejection"),
            failed_group: Bounded::new(2, "failed GTK command"),
            failed_group_terminal: Bounded::new(2, "uncertain GTK terminal"),
            rx_out: Cell::new(0),
            configured: Cell::new([false; 2]),
            lost: Cell::new(false),
            body: RefCell::default(),
            gate_issuer: RefCell::new(gate_issuer),
            draining: Cell::new(false),
        };
        let resources = ModelResources {
            connection_epoch: epoch_slot,
            tickets: tickets.into_iter().collect(),
            small: small
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: ModelMemory::Small(Box::new([0; ModelSmall::CAPACITY])),
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: ModelMemory::Large(Box::new([0; ModelLarge::CAPACITY])),
                })
                .collect(),
            station_pairwise,
            station_group,
            access_point_pairwise,
            access_point_group,
            block_acks,
            station_group_permission,
            access_point_group_permission,
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
        poll_fn(move |cx| {
            held.air.passes.set(held.air.passes.get().saturating_add(1));
            // A driver that sees the abandon latch publishes nothing and
            // stops.
            if held.air.poisoned.get() || retire.is_requested() || retire.is_abandoned() {
                return Poll::Ready(());
            }
            held.driver_state.borrow_mut().serve(held);
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
    const PUBLICATION_BOUND: oer_time::Duration = MODEL_PUBLICATION_BOUND;
    const GROUP_COMMAND_BOUND: oer_time::Duration = MODEL_GROUP_COMMAND_BOUND;
}

/// The driver retains the unique open place throughout the closing barrier.
enum DriverGate<'id> {
    Open,
    Closing(GateOpen<'id, ModelPool<0>>),
    Closed,
}

struct Removal<'id, X: ModelContext> {
    key: KeyIdentity,
    removed: Removed<'id, ModelTypes<X>>,
    group_admitted_at: Option<u64>,
}

/// The model's hardware, which only the driver owns.
struct Driver<'id, X: ModelContext> {
    /// The prepared attempts of each queue, the head first.
    queues: [TxQueue<'id, X>; WmmAccessCategory::ALL.len()],
    /// The installed hardware keys.
    keys: Vec<KeyIdentity>,
    removing: Vec<Removal<'id, X>>,
    gate: DriverGate<'id>,
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
            for (slot, wanted) in settings.block_acks {
                let mut applied = held.air.receive_agreements.borrow_mut();
                match wanted {
                    Some(agreement) => {
                        applied.insert(slot, agreement);
                    }
                    None => {
                        applied.remove(&slot);
                    }
                }
            }
            if settings.station {
                held.air
                    .settings
                    .set(held.air.settings.get().saturating_add(1));
                held.configured(VifRole::Station);
            }
            if settings.access_point {
                held.air
                    .settings
                    .set(held.air.settings.get().saturating_add(1));
                held.configured(VifRole::AccessPoint);
            }
            while let Some(mut buffer) = held.recycled.pop() {
                buffer.recycled = true;
                held.rx_out.set(held.rx_out.get().saturating_sub(1));
            }
            if settings.other {
                held.air
                    .settings
                    .set(held.air.settings.get().saturating_add(1));
            }
            let command = held.commands.pop();
            let Some(command) = command else {
                break;
            };
            held.air
                .commands
                .set(held.air.commands.get().saturating_add(1));
            match command {
                Command::Submit(prepared) => {
                    let queue = prepared.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Single(prepared));
                }
                Command::SubmitAmpdu(aggregate) => {
                    let queue = aggregate.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Aggregate(aggregate));
                }
                Command::InstallKey { key } => self.keys.push(key),
                Command::Group {
                    admitted_at,
                    change,
                } => {
                    if admitted_at
                        .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                        .is_none_or(|deadline| held.air.now_micros.get() >= deadline)
                        || held.air.group_failure.get() == Some(GroupFaultStage::Before)
                    {
                        let _capacity_fault = held.failed_group.push(change);
                        held.poison();
                        return;
                    }
                    if held.air.group_held.get() && !held.draining.get() {
                        // We popped exactly this cell; restore it without creating
                        // capacity or allowing a later command to pass it.
                        held.commands.items.borrow_mut().push_front(Command::Group {
                            admitted_at,
                            change,
                        });
                        break;
                    }
                    self.group_change(held, admitted_at, change);
                    if held.air.poisoned.get() {
                        return;
                    }
                }
                Command::RemoveKey { key, removed } => {
                    self.removing.push(Removal {
                        key,
                        removed,
                        group_admitted_at: None,
                    });
                }
                Command::CloseGate(gate) => self.gate = DriverGate::Closing(gate),
                Command::OpenGate => self.gate = DriverGate::Open,
            }
            self.publish(held);
        }
        // A result released by the world makes progress without a command.
        self.publish(held);
        let idle = self
            .queues
            .iter()
            .all(|queue| queue.air.is_none() && queue.waiting.is_empty());
        if idle {
            self.gate = match core::mem::replace(&mut self.gate, DriverGate::Closed) {
                DriverGate::Closing(gate) => {
                    let closed = held.gate_issuer.borrow_mut().close(gate);
                    held.post(LowerMacEvent::GateClosed(closed));
                    DriverGate::Closed
                }
                state => state,
            };
        }
        if held.air.poisoned.get() {
            return;
        }
        self.finish_removals(held);
        if held.air.poisoned.get() {
            return;
        }
        if idle
            && self.removing.is_empty()
            && held.commands.is_empty()
            && held.draining.replace(false)
        {
            held.wake_body();
        }
        self.receive(held);
    }

    fn activate_group(&mut self, held: &ModelHeld<'id, X>, identity: &KeyIdentity, write: bool) {
        let Some(selected) = identity.0.group.as_ref() else {
            return;
        };
        for key in &self.keys {
            if key.0.key_id == identity.0.key_id
                && key
                    .0
                    .group
                    .as_ref()
                    .is_some_and(|group| group.role == selected.role)
            {
                key.0.rx_active.set(false);
            }
        }
        identity.0.rx_active.set(true);
        if write {
            held.air
                .group_writes
                .set(held.air.group_writes.get().saturating_add(1));
            let mut installations = held.air.group_installations.borrow_mut();
            installations.retain(|key| key.upgrade().is_some_and(|key| key.active.get()));
            installations.push(Rc::downgrade(&identity.0));
            self.keys.push(identity.clone());
        }
    }

    fn group_change(
        &mut self,
        held: &ModelHeld<'id, X>,
        admitted_at: u64,
        change: GroupChange<'id, X>,
    ) {
        // A finite capture of all old ingress stages. Continuous RX is a
        // separate source; no drain-to-idle loop can postpone this boundary.
        self.receive(held);
        let captured = core::mem::take(&mut *held.air.group_rx.borrow_mut());
        for received in captured {
            self.deliver(held, received.frame, received.meta);
        }
        let terminal = match change {
            GroupChange::Install(installed) => {
                let identity = match &installed {
                    InstalledGroup::Station { key, .. } => &key.identity,
                    InstalledGroup::AccessPoint { key, .. } => &key.identity,
                };
                self.activate_group(held, identity, true);
                LowerMacEvent::GroupKeyInstalled(installed)
            }
            GroupChange::Replace {
                permission,
                installed,
                key_id,
                key,
            } => {
                installed.identity.0.active.set(false);
                installed.identity.0.rx_active.set(false);
                self.keys.retain(|key| *key != installed.identity);
                let identity = KeyIdentity::group(
                    installed.slot.index(),
                    key_id,
                    VifRole::Station,
                    key,
                    &held.air,
                );
                self.activate_group(held, &identity, true);
                LowerMacEvent::GroupKeyReplaced {
                    key: ModelGroupKey {
                        identity,
                        slot: installed.slot,
                    },
                    permission,
                }
            }
            GroupChange::Select {
                permission,
                installed,
            } => {
                self.activate_group(held, &installed.identity, false);
                LowerMacEvent::GroupKeySelected {
                    key: installed,
                    permission,
                }
            }
            GroupChange::Remove { key, removed } => {
                key.0.active.set(false);
                key.0.rx_active.set(false);
                self.removing.push(Removal {
                    key,
                    removed,
                    group_admitted_at: Some(admitted_at),
                });
                // AP removal can still wait for a published TX to end. Its
                // permission remains in removing, not in a success terminal.
                return;
            }
        };
        if held.air.group_failure.get() == Some(GroupFaultStage::After) {
            let _capacity_fault = held.failed_group_terminal.push(terminal);
            held.poison();
        } else {
            held.post(terminal);
        }
    }

    /// Keep each published head until its result; barriers stop the next
    /// publication, not just admission of new plans.
    fn publish(&mut self, held: &ModelHeld<'id, X>) {
        for ((queue, busy), result_held) in self
            .queues
            .iter_mut()
            .zip(held.air.held.get())
            .zip(held.air.held_results.get())
        {
            loop {
                if let Some(publication) = queue.air.take() {
                    let elapsed = held
                        .air
                        .now_micros
                        .get()
                        .saturating_sub(publication.started);
                    if result_held && elapsed < MODEL_PUBLICATION_BOUND.as_micros() {
                        queue.air = Some(publication);
                        break;
                    }
                    let cause =
                        withdrawal(held, &self.gate, publication.work.key(), &self.removing);
                    let result = if result_held {
                        AirAnswer::HardwareTimeout
                    } else {
                        publication.result
                    };
                    queue.air = publication.work.complete(held, &self.keys, result, cause);
                    if held.air.poisoned.get() {
                        return;
                    }
                    continue;
                }
                let Some(work) = queue.waiting.pop_front() else {
                    break;
                };
                if let Some(cause) = withdrawal(held, &self.gate, work.key(), &self.removing) {
                    withdrawn(held, work, cause);
                } else if busy {
                    queue.waiting.push_front(work);
                    break;
                } else {
                    queue.air = start_work(held, &self.keys, work);
                    if held.air.poisoned.get() {
                        return;
                    }
                }
            }
        }
    }

    fn finish_removals(&mut self, held: &ModelHeld<'id, X>) {
        // The single reference belongs to KeyInstall. Additional owners are
        // actual prepared/queued/published attempts. Returned Sent identities
        // do not keep a preparation alive and do not delay key-slot reuse.
        let mut pending = Vec::new();
        for removal in self.removing.drain(..) {
            let overdue = removal.group_admitted_at.is_some_and(|admitted| {
                admitted
                    .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                    .is_none_or(|deadline| held.air.now_micros.get() >= deadline)
            });
            let uncertain = removal.group_admitted_at.is_some()
                && held.air.group_failure.get() == Some(GroupFaultStage::After);
            if overdue || uncertain || held.air.poisoned.get() {
                pending.push(removal);
                held.poison();
            } else if Rc::strong_count(&removal.key.0.preparations) == 1 {
                self.keys.retain(|installed| *installed != removal.key);
                held.post(LowerMacEvent::KeyRemoved(removal.removed));
            } else {
                pending.push(removal);
            }
        }
        self.removing = pending;
    }

    fn deliver(&mut self, held: &ModelHeld<'id, X>, frame: Vec<u8>, meta: ModelRxMeta) {
        if held.rx_out.get() < MODEL_RX_EVENTS {
            held.rx_out.set(held.rx_out.get().saturating_add(1));
            held.post(LowerMacEvent::Received {
                frame: ModelRxBuffer {
                    bytes: frame,
                    recycled: false,
                    air: Rc::downgrade(&held.air),
                    streamed: false,
                },
                meta,
            });
        } else {
            held.lost.set(true);
            held.wake_body();
        }
    }

    /// Capture only this finite batch; newly arriving traffic belongs to the
    /// next pass. Delayed ingress is forced through before a GTK boundary.
    fn receive(&mut self, held: &ModelHeld<'id, X>) {
        let captured = core::mem::take(&mut *held.air.rx.borrow_mut());
        for (frame, meta) in captured {
            self.deliver(held, frame, meta);
        }
        let captured = core::mem::take(&mut *held.air.group_rx.borrow_mut());
        for received in captured {
            if received.stage == GroupRxStage::Ready {
                // Integrity was established at capture under this owner.
                // Retain it through delivery even if the slot is later reused.
                let _installation = received.installation;
                self.deliver(held, received.frame, received.meta);
            } else {
                held.air.group_rx.borrow_mut().push_back(received);
            }
        }
    }
}

/// Synthetic model latency, in model time; hardware backends declare their
/// own PPDU + response/timeout bound. It does not scale with retry-plan length.
pub const MODEL_PUBLICATION_BOUND: oer_time::Duration = oer_time::Duration::from_millis(1);
/// Synthetic admission-to-GTK-terminal bound, driven by `ModelAir::set_time`.
pub const MODEL_GROUP_COMMAND_BOUND: oer_time::Duration = oer_time::Duration::from_millis(2);

struct TxQueue<'id, X: ModelContext> {
    waiting: VecDeque<Work<'id, X>>,
    air: Option<Publication<'id, X>>,
}
struct Publication<'id, X: ModelContext> {
    started: u64,
    result: AirAnswer,
    work: Published<'id, X>,
}
struct SinglePlan<'id, X: ModelContext> {
    prepared: Box<Prepared<'id, X>>,
    remaining: VecDeque<PhyRate>,
    last: Option<(NonZeroU16, PhyRate)>,
    rate: PhyRate,
}
enum Published<'id, X: ModelContext> {
    Single(SinglePlan<'id, X>),
    Aggregate {
        prepared: PreparedAggregate<'id, X>,
        report: BlockAckReport,
    },
}
/// Derive the terminal from the actual lifecycle barrier and key owner;
/// callers cannot exchange three unrelated boolean arguments. Disable wins
/// over key removal, which wins over a closing or closed gate.
fn withdrawal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    gate: &DriverGate<'id>,
    key: Option<&KeyIdentity>,
    removals: &[Removal<'id, X>],
) -> Option<WithdrawCause> {
    if held.draining.get() {
        Some(WithdrawCause::Disabled)
    } else if key.is_some_and(|key| removals.iter().any(|removal| &removal.key == key)) {
        Some(WithdrawCause::KeyRemoved)
    } else {
        match gate {
            DriverGate::Open => None,
            DriverGate::Closing(_) | DriverGate::Closed => Some(WithdrawCause::GateClosed),
        }
    }
}
impl<'id, X: ModelContext> Published<'id, X> {
    fn key(&self) -> Option<&KeyIdentity> {
        match self {
            Self::Single(plan) => plan.prepared.key.as_ref(),
            Self::Aggregate { prepared, .. } => prepared.key.as_ref(),
        }
        .map(|key| &key.identity)
    }
    fn complete(
        self,
        held: &ModelHeld<'id, X>,
        keys: &[KeyIdentity],
        result: AirAnswer,
        cause: Option<WithdrawCause>,
    ) -> Option<Publication<'id, X>> {
        match self {
            Self::Single(mut plan) => {
                if matches!(
                    result,
                    AirAnswer::Ack | AirAnswer::BlockAck(_) | AirAnswer::NoResponse
                ) {
                    let transmissions = plan
                        .last
                        .map_or(NonZeroU16::MIN, |(count, _)| count.saturating_add(1));
                    plan.last = Some((transmissions, plan.rate));
                }
                let response = plan.prepared.operation.payload().response;
                let report = match (response, result) {
                    (ResponsePolicy::BlockAck { start_sequence }, AirAnswer::BlockAck(bitmap)) => {
                        Some(BlockAckReport {
                            start_sequence,
                            bitmap,
                        })
                    }
                    _ => None,
                };
                let outcome = air_outcome(response, result, plan.last, report, cause);
                if matches!(
                    outcome,
                    TxOutcome::Ended {
                        status: TxEndStatus::AckTimeout | TxEndStatus::ContentionLimit,
                        ..
                    }
                ) && let Some(rate) = plan.remaining.pop_front()
                {
                    plan.rate = rate;
                    return publish_single(held, keys, plan);
                }
                terminal(held, *plan.prepared, outcome);
            }
            Self::Aggregate { prepared, report } => {
                let response = ResponsePolicy::BlockAck {
                    start_sequence: prepared.start_sequence,
                };
                let last = (!matches!(result, AirAnswer::Collision))
                    .then_some((NonZeroU16::MIN, prepared.rate));
                terminal_aggregate(
                    held,
                    prepared,
                    air_outcome(response, result, last, Some(report), cause),
                );
            }
        }
        None
    }
}
/// One interpretation of air completion for both submission shapes.
fn air_outcome(
    response: ResponsePolicy,
    answer: AirAnswer,
    last: Option<(NonZeroU16, PhyRate)>,
    report: Option<BlockAckReport>,
    cause: Option<WithdrawCause>,
) -> TxOutcome {
    match answer {
        AirAnswer::HardwareTimeout => TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
        AirAnswer::Ack | AirAnswer::BlockAck(_) => TxOutcome::Ended {
            status: TxEndStatus::Delivered,
            last,
            ack: Some(TxAck {
                block_ack: report,
                ..TxAck::default()
            }),
        },
        AirAnswer::NoResponse if matches!(response, ResponsePolicy::NoResponseExpected) => {
            TxOutcome::Ended {
                status: TxEndStatus::Delivered,
                last,
                ack: None,
            }
        }
        AirAnswer::NoResponse | AirAnswer::Collision => match cause {
            Some(cause) => TxOutcome::Withdrawn(cause),
            None => TxOutcome::Ended {
                status: if matches!(answer, AirAnswer::Collision) {
                    TxEndStatus::ContentionLimit
                } else {
                    TxEndStatus::AckTimeout
                },
                last,
                ack: None,
            },
        },
    }
}

fn start_work<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    work: Work<'id, X>,
) -> Option<Publication<'id, X>> {
    match work {
        Work::Single(prepared) => {
            let rate = prepared.plan.first().rate;
            let remaining = prepared
                .plan
                .stages()
                .flat_map(|stage| {
                    core::iter::repeat_n(stage.rate, usize::from(stage.transmissions.get()))
                })
                .skip(1)
                .collect();
            publish_single(
                held,
                keys,
                SinglePlan {
                    prepared,
                    remaining,
                    last: None,
                    rate,
                },
            )
        }
        Work::Aggregate(prepared) => publish_aggregate(held, keys, prepared),
    }
}
fn publish_single<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    mut plan: SinglePlan<'id, X>,
) -> Option<Publication<'id, X>> {
    let result = match held.air.result(plan.prepared.operation.payload().response) {
        Ok(result) => result,
        Err(AnswerShapeMismatch) => {
            let _fault = held.quarantined.push(Work::Single(plan.prepared));
            held.poison();
            return None;
        }
    };
    if plan
        .prepared
        .operation
        .execute()
        .publish(
            plan.prepared.role,
            plan.prepared.key.as_ref().map(|key| &key.identity),
        )
        .is_err()
    {
        let _fault = held.quarantined.push(Work::Single(plan.prepared));
        held.poison();
        return None;
    }
    if plan.last.is_some() {
        plan.prepared.operation.execute().retry();
    }
    let frame = match plan.prepared.operation.payload().on_air(
        plan.prepared.role,
        keys,
        plan.prepared.key.as_ref().map(|key| &key.identity),
        plan.rate,
    ) {
        Ok(frame) => frame,
        Err(_) => {
            let _fault = held.quarantined.push(Work::Single(plan.prepared));
            held.poison();
            return None;
        }
    };
    held.air.sent.borrow_mut().push(frame);
    Some(Publication {
        started: held.air.now_micros.get(),
        result,
        work: Published::Single(plan),
    })
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
            terminal_aggregate(held, aggregate, TxOutcome::Withdrawn(cause))
        }
    }
}

fn terminal_aggregate<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    aggregate: PreparedAggregate<'id, X>,
    completion: TxOutcome,
) {
    held.post(LowerMacEvent::TxCompleted(TxCompleted::Aggregate(
        aggregate.operation.complete(completion),
    )));
}

fn publish_aggregate<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    mut aggregate: PreparedAggregate<'id, X>,
) -> Option<Publication<'id, X>> {
    let result = match held.air.result(ResponsePolicy::BlockAck {
        start_sequence: aggregate.start_sequence,
    }) {
        Ok(result) => result,
        Err(AnswerShapeMismatch) => {
            let _fault = held.quarantined.push(Work::Aggregate(aggregate));
            held.poison();
            return None;
        }
    };
    let acknowledged = match result {
        AirAnswer::BlockAck(bitmap) => bitmap,
        _ => 0,
    };
    let frames = match aggregate.operation.execute().publish(
        aggregate.role,
        keys,
        aggregate.key.as_ref().map(|key| &key.identity),
        aggregate.rate,
    ) {
        Ok(frames) => frames,
        Err(_) => {
            let _fault = held.quarantined.push(Work::Aggregate(aggregate));
            held.poison();
            return None;
        }
    };
    let mut bitmap = 0_u64;
    for (index, subframe) in aggregate.operation.payload().0.iter().enumerate() {
        if acknowledged & (1 << index) != 0 {
            bitmap |= subframe.acknowledgement_bit;
        }
    }
    held.air.sent.borrow_mut().extend(frames);
    // NonEmptyAggregate carries the first sequence into PreparedAggregate below.
    let report = BlockAckReport {
        start_sequence: aggregate.start_sequence,
        bitmap,
    };
    Some(Publication {
        started: held.air.now_micros.get(),
        result,
        work: Published::Aggregate {
            prepared: aggregate,
            report,
        },
    })
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    prepared: Prepared<'id, X>,
    outcome: TxOutcome,
) {
    held.post(LowerMacEvent::TxCompleted(TxCompleted::Single(
        prepared.operation.complete(outcome),
    )));
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
            if let Some(event) = held.take_event() {
                return Poll::Ready(Ok(Ok(event)));
            }
            if let Some((bytes, meta)) = held.air.rx_stream.borrow().as_ref()
                && held.rx_out.get() < MODEL_RX_EVENTS
            {
                held.rx_out.set(held.rx_out.get().saturating_add(1));
                return Poll::Ready(Ok(Ok(LowerMacEvent::Received {
                    frame: ModelRxBuffer {
                        bytes: bytes.clone(),
                        recycled: false,
                        air: Rc::downgrade(&held.air),
                        streamed: true,
                    },
                    meta: *meta,
                })));
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
        if buffer.streamed {
            let mut buffer = buffer;
            // The independent source observes this return immediately. Its
            // next poll can reuse the same bounded RX capacity without a
            // body/driver task switch, exactly the starvation case.
            buffer.recycled = true;
            self.held
                .rx_out
                .set(self.held.rx_out.get().saturating_sub(1));
            return;
        }
        if self.held.recycled.push(buffer).is_err() {
            self.held.poison();
        }
        self.held.air.wake_driver();
    }

    fn close_gate(&mut self, gate: GateOpen<'id, ModelPool<0>>) {
        self.held.command(Command::CloseGate(gate));
    }

    fn open_gate(&mut self, gate: GateClosed<'id, ModelPool<0>>) -> GateOpen<'id, ModelPool<0>> {
        let open = self.held.gate_issuer.borrow_mut().open(gate);
        self.held.command(Command::OpenGate);
        open
    }

    fn set_edca(&mut self, _: SupportedEdca<C>) {
        self.held.settings.borrow_mut().other = true;
        self.held.air.wake_driver();
    }

    fn station(&mut self) -> ModelInterface<'p, '_, 'id, Station, C, X> {
        let [station, _] = self.backend.addresses;
        ModelInterface {
            held: self.held,
            address: station,
            role: PhantomData,
        }
    }

    fn access_point(&mut self) -> ModelInterface<'p, '_, 'id, AccessPoint, C, X> {
        let [_, access_point] = self.backend.addresses;
        ModelInterface {
            held: self.held,
            address: access_point,
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
pub struct ModelPairwiseKey<'id, R: ModelRole> {
    identity: KeyIdentity,
    slot: Slot<'id, ModelPairwiseKeys<R>, R::PairwisePath>,
}

/// A group key the model installed.
pub struct ModelGroupKey<'id, R: ModelRole> {
    identity: KeyIdentity,
    slot: Slot<'id, ModelGroupKeys<R>, R::GroupPath>,
}

/// A receive Block Ack agreement the model keeps.
pub struct ModelBlockAck<'id> {
    slot: Slot<'id, ModelBlockAcks, ModelPool<9>>,
}

/// A retry plan's stages, as the model's hardware takes them.
fn stages<C: Ieee80211Capabilities>(
    frame: &TxFrame<'_, '_, C>,
) -> oer_ieee80211_lower_mac::RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one_stage(rate.rate().get()),
        TxFrame::Group { rate, .. } => one_stage(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}
fn one_stage(rate: PhyRate) -> oer_ieee80211_lower_mac::RetryPlan<PhyRate> {
    oer_ieee80211_lower_mac::RetryPlan::new(oer_ieee80211_lower_mac::RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::MIN,
        protection: oer_ieee80211_lower_mac::Protection::None,
    })
}

fn retry_stages<K>(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_, '_, K>,
) -> Result<oer_ieee80211_lower_mac::RetryPlan<PhyRate>, PreparationError> {
    match (layout.kind(), plan) {
        (MpduKind::Individual, RetransmissionPlan::Individual(plan)) => {
            Ok(plan.map(|rate| rate.get()))
        }
        (MpduKind::IndividualNoAck, RetransmissionPlan::IndividualNoAck(rate)) => {
            Ok(one_stage(rate.rate().get()))
        }
        (MpduKind::Group, RetransmissionPlan::Group(rate)) => Ok(one_stage(rate.get())),
        _ => Err(PreparationError::RetransmissionKind),
    }
}

type PhaseProof<'p, T> = PhantomData<fn(&'p ()) -> (&'p (), T)>;
type TypeProof<T> = PhantomData<fn() -> T>;
type ModelSource<'k, 'p, 'id, C, K, R, X> = TxSource<
    ModelBuffer<'id, C>,
    ModelSent<'id, C>,
    oer_ieee80211_lower_mac::ClassFrame<'p, 'id, C, K>,
    ModelBody,
    <R as RoleKeys<'id, ModelTypes<X>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;
type ModelCheckedSource<'k, 'p, 'id, C, K, R, X> = CheckedTxSource<
    ModelBuffer<'id, C>,
    ModelSent<'id, C>,
    oer_ieee80211_lower_mac::ClassFrame<'p, 'id, C, K>,
    ModelBody,
    <R as RoleKeys<'id, ModelTypes<X>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
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
    plan: oer_ieee80211_lower_mac::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    proof: PhaseProof<'p, (R, K, X)>,
}
impl<'id, R, K, X: ModelContext> PreparedTx<'id> for ModelPrepared<'_, '_, 'id, R, K, X> {
    type Types = ModelTypes<X>;
    fn into_parts(self) -> (ModelAnyBuffer<'id>, Option<ModelBody>) {
        self.mpdu.into_parts()
    }
}

#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::result_large_err,
        reason = "class refusal returns the original affine MPDU by value"
    )
)]
// The return crosses Clippy's size threshold on 64-bit targets only.
// Pointer width changes the lint expectation, never the model or its API.
/// A model buffer class and its unique pool within the installation layout.
pub trait ModelClass: TxBufferClass + Sized {
    /// Nominal component of the backing memory slots.
    type Path: oer_radio_port::PoolPath;
    /// Return fresh memory through the terminal's exhaustive buffer enum.
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id>;
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id>;
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id>(
        buffer: ModelAnyBuffer<'id>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id>>;
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id>(buffer: ModelAnyBuffer<'id>) -> Result<ModelSent<'id, Self>, ModelAnyBuffer<'id>>;
}

impl ModelClass for ModelSmall {
    type Path = ModelPool<2>;
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Small(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::SmallSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id>(
        buffer: ModelAnyBuffer<'id>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id>(buffer: ModelAnyBuffer<'id>) -> Result<ModelSent<'id, Self>, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::SmallSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
}
impl ModelClass for ModelLarge {
    type Path = ModelPool<3>;
    fn erase<'id>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::Large(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id>(mpdu: ModelSent<'id, Self>) -> ModelAnyBuffer<'id> {
        ModelAnyBuffer::LargeSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id>(
        buffer: ModelAnyBuffer<'id>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::Large(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id>(buffer: ModelAnyBuffer<'id>) -> Result<ModelSent<'id, Self>, ModelAnyBuffer<'id>> {
        match buffer {
            ModelAnyBuffer::LargeSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
}

#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::result_large_err,
        reason = "a refusal returns the whole operation by value"
    )
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
    source: ModelCheckedSource<'k, 'p, 'id, C, K, R, X>,
) -> ModelPreparation<'k, 'p, 'id, R, K, X, C> {
    let (mpdu, plan, key) = match source {
        CheckedTxSource::Fresh(fresh) => {
            let (
                FreshSource {
                    buffer,
                    frame,
                    body,
                    protection,
                },
                layout,
            ) = fresh.into_parts();
            let key = match protection {
                TxProtection::Open => None,
                TxProtection::Ccmp(key) => Some(R::tx_identity(&key)),
            };
            let plan = stages(frame.frame());
            let response = ResponsePolicy::of_frame(frame.frame());
            (
                ModelMpdu {
                    buffer: C::erase(buffer),
                    layout,
                    response,
                    body,
                    published: None,
                },
                plan,
                key,
            )
        }
        CheckedTxSource::Retransmission { mpdu, plan } => {
            let checked = if mpdu.role != R::ROLE {
                Err(PreparationError::RetransmissionKind)
            } else if mpdu.key.as_ref().is_some_and(|key| !key.0.active.get()) {
                Err(PreparationError::RetransmissionKey)
            } else {
                retry_stages(&mpdu.layout, &plan)
            };
            let stages = match checked {
                Ok(stages) => stages,
                Err(reason) => {
                    return Err(PreparationRefused {
                        reason,
                        operation: TxSource::Retransmission { mpdu, plan },
                    });
                }
            };
            let key = mpdu.key.clone();
            let layout = mpdu.layout;
            let response = mpdu.response;
            (
                ModelMpdu::from_sent(C::erase_sent(mpdu), layout, response),
                stages,
                key,
            )
        }
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
    R: ModelRole + RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    type Caps = K;
    type Types = ModelTypes<X>;
    type Role = R;

    fn address(&self) -> IndividualAddress {
        self.address
    }

    fn submit<'k>(
        &mut self,
        attempt: InterfaceAttempt<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>> {
        let ModelPrepared {
            mpdu, plan, key, ..
        } = attempt.source;
        self.held.command(Command::Submit(Box::new(Prepared {
            operation: InFlight::admit(attempt.context, attempt.ticket, mpdu),
            role: R::ROLE,
            queue: attempt.access_category,
            plan,
            key,
        })));
        if self.held.air.poisoned.get() {
            Err(Poisoned { cause: ModelFault })
        } else {
            Ok(())
        }
    }

    fn install_pairwise_key(
        &mut self,
        slot: Slot<'id, ModelPairwiseKeys<R>, R::PairwisePath>,
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
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>,
        slot: Slot<'id, ModelGroupKeys<R>, R::GroupPath>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        let identity = KeyIdentity::group(
            slot.index(),
            key_id,
            R::ROLE,
            CcmpKey::new(key.0),
            &self.held.air,
        );
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Install(R::installed_group(
                ModelGroupKey { identity, slot },
                permission,
            )),
        });
    }

    fn remove_pairwise_key(&mut self, key: ModelPairwiseKey<'id, R>) {
        key.identity.0.active.set(false);
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: R::removed(RemovedKey::Pairwise(key.slot)),
        });
    }

    fn remove_group_key(
        &mut self,
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>,
        key: ModelGroupKey<'id, R>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Remove {
                key: key.identity,
                removed: R::removed(RemovedKey::Group {
                    slot: key.slot,
                    permission,
                }),
            },
        });
    }

    fn add_block_ack(
        &mut self,
        slot: Slot<'id, ModelBlockAcks, ModelPool<9>>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        self.held.settings.borrow_mut().block_acks.insert(
            slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        ModelBlockAck { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ModelBlockAck<'id>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        self.held.settings.borrow_mut().block_acks.insert(
            running.slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        running
    }

    fn remove_block_ack(
        &mut self,
        agreement: ModelBlockAck<'id>,
    ) -> Slot<'id, ModelBlockAcks, ModelPool<9>> {
        self.held
            .settings
            .borrow_mut()
            .block_acks
            .insert(agreement.slot.index(), None);
        self.held.air.wake_driver();
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
pub struct ModelAggregate<
    'k,
    'p,
    'id,
    R,
    K = ModelCaps,
    X: ModelContext = (),
    S: AggregateState = EmptyAggregate,
> {
    subframes: Vec<Subframe<'id>>,
    first: S::Head<SequenceNumber>,
    receiver: IndividualAddress,
    tid: Tid,
    rate: AmpduRate<'p, 'id, K>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    caps: TypeProof<(R, K, X, S)>,
}
impl<R, K, X: ModelContext, S: AggregateState> ModelAggregate<'_, '_, '_, R, K, X, S> {
    fn acknowledgement_bit(&self, sequence: SequenceNumber) -> Option<u64> {
        let first = S::retain_head(&self.first, sequence);
        let distance = first.forward_distance(sequence);
        let bit = 1_u64.checked_shl(u32::from(distance))?;
        (!self
            .subframes
            .iter()
            .any(|subframe| subframe.acknowledgement_bit == bit))
        .then_some(bit)
    }
}
impl<'id, R, K, X: ModelContext, S: AggregateState> PhaseAggregate<'id>
    for ModelAggregate<'_, '_, 'id, R, K, X, S>
{
    type Types = ModelTypes<X>;
    type Cancelled = ModelSubframeIter<'id>;
    fn receiver(&self) -> IndividualAddress {
        self.receiver
    }
    fn tid(&self) -> Tid {
        self.tid
    }
    fn protection(&self) -> oer_ieee80211_lower_mac::MpduProtection {
        match &self.key {
            None => oer_ieee80211_lower_mac::MpduProtection::Open,
            Some(_) => oer_ieee80211_lower_mac::MpduProtection::Ccmp,
        }
    }
    fn subframes(&self) -> usize {
        self.subframes.len()
    }
    fn into_parts(self) -> Self::Cancelled {
        ModelSubframeIter(self.subframes.into_iter())
    }
}
type ModelSubframe<'id, C, K> =
    SubframeSource<ModelBuffer<'id, C>, ModelSent<'id, C>, ClassSubframe<C, K>, ModelBody>;
type ModelCheckedAggregate<'k, 'p, 'id, R, K, X, S, C> = CheckedAggregate<
    ModelAggregate<'k, 'p, 'id, R, K, X, S>,
    ModelBuffer<'id, C>,
    ModelSent<'id, C>,
    ClassSubframe<C, K>,
    ModelBody,
>;
type ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> = Result<
    ModelAggregate<'k, 'p, 'id, R, K, X, NonEmptyAggregate>,
    AggregateRefused<ModelAggregate<'k, 'p, 'id, R, K, X, S>, ModelSubframe<'id, C, K>>,
>;

#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::result_large_err,
        reason = "a refusal returns the whole operation by value"
    )
)]
fn push_model<
    'k,
    'p,
    'id,
    R: oer_ieee80211_lower_mac::InterfaceRole,
    K: Ieee80211Capabilities,
    X: ModelContext,
    S: AggregateState,
    C: ModelClass,
>(
    operation: ModelCheckedAggregate<'k, 'p, 'id, R, K, X, S, C>,
) -> ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> {
    let (mut aggregate, source) = operation.into_parts();
    let refused = if aggregate.subframes.len() >= MODEL_AMPDU_SUBFRAMES {
        Some(PreparationError::AggregateFull)
    } else if let CheckedSubframe::Sent(mpdu) = &source {
        if mpdu.key.as_ref() != aggregate.key.as_ref().map(|key| &key.identity)
            || mpdu.key.as_ref().is_some_and(|key| !key.0.active.get())
        {
            Some(PreparationError::RetransmissionKey)
        } else if mpdu.role != R::ROLE
            || mpdu.layout.recipient() != Some((aggregate.receiver, aggregate.tid))
        {
            Some(PreparationError::RetransmissionKind)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(reason) = refused {
        return Err(AggregateRefused {
            reason,
            aggregate,
            subframe: source.into_source(),
        });
    }
    let (mpdu, acknowledgement_bit) = match source {
        CheckedSubframe::Fresh(fresh) => {
            let (
                FreshSubframe {
                    buffer,
                    frame,
                    body,
                },
                layout,
            ) = fresh.into_parts();
            let Some(bit) = aggregate.acknowledgement_bit(buffer.memory.sequence()) else {
                return Err(AggregateRefused {
                    reason: PreparationError::BlockAckWindow,
                    aggregate,
                    subframe: SubframeSource::Fresh {
                        buffer,
                        frame,
                        body,
                    },
                });
            };
            (
                ModelMpdu {
                    buffer: C::erase(buffer),
                    layout,
                    response: ResponsePolicy::Ack,
                    body,
                    published: None,
                },
                bit,
            )
        }
        CheckedSubframe::Sent(mpdu) => {
            let Some(bit) = aggregate.acknowledgement_bit(mpdu.buffer.memory.sequence()) else {
                return Err(AggregateRefused {
                    reason: PreparationError::BlockAckWindow,
                    aggregate,
                    subframe: SubframeSource::Sent(mpdu),
                });
            };
            let layout = mpdu.layout;
            let response = mpdu.response;
            (
                ModelMpdu::from_sent(C::erase_sent(mpdu), layout, response),
                bit,
            )
        }
    };
    let first = S::retain_head(&aggregate.first, mpdu.buffer.sequence());
    aggregate.subframes.push(Subframe {
        mpdu,
        acknowledgement_bit,
    });
    let ModelAggregate {
        subframes,
        receiver,
        tid,
        rate,
        key,
        ..
    } = aggregate;
    Ok(ModelAggregate {
        first,
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
    R: ModelRole + RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
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
        rate: AmpduRate<'p, 'id, K>,
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
            first: (),
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
        let ModelAggregate {
            subframes,
            first,
            tid,
            rate,
            key,
            ..
        } = attempt.aggregate;
        let queue = match oer_ieee80211_mac::qos::WmmUserPriority::new(tid.get()) {
            Some(priority) => priority.access_category(),
            None => oer_ieee80211_mac::qos::WmmAccessCategory::BestEffort,
        };
        self.held.command(Command::SubmitAmpdu(PreparedAggregate {
            operation: InFlight::admit(attempt.context, attempt.ticket, ModelSubframes(subframes)),
            role: R::ROLE,
            start_sequence: first,
            queue,
            rate: rate.rate().get(),
            key,
        }));
        if self.held.air.poisoned.get() {
            Err(Poisoned { cause: ModelFault })
        } else {
            Ok(())
        }
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> StationPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, Station, C, X>
{
    fn replace_group_key(
        &mut self,
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Replace {
                permission,
                installed,
                key_id,
                key: CcmpKey::new(key.0),
            },
        });
    }

    fn select_group_key(
        &mut self,
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Select {
                permission,
                installed,
            },
        });
    }

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
fn rate<'p, 'id>(
    phase: &ModelPhase<'p, 'id>,
    rate: PhyRate,
) -> Option<PhaseRate<'p, 'id, ModelCaps>> {
    phase.rate(rate)
}

#[cfg(test)]
mod tests;
