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
    AccessPoint, AccessPointPhase, AttemptKey, BandSet, BssColor, Ccmp128Key, Channel, ClassFrame,
    CoexPriority, CoexPrioritySet, FrameHeader, GateClosed, GateClosedKind, GateOpen, GateOpenKind,
    HardwareServices, Ieee80211Capabilities, Ieee80211Radio, IndividualAddress, InterfaceAttempt,
    InterfacePhase, KeyTypes, LinkMetric, LowerMacBackend, LowerMacCapabilities, LowerMacEvent,
    LowerMacPhase, LowerMacSetup, PhaseRate, PhaseTypes, PhyFormatSet, Protection, RateSupport,
    ReceiveFilter, Removed, RemovedKey, RoleKeys, RxBuffer, RxEvidence, RxMeta, RxMetadata,
    SignalMetadata, Station, StationPhase, StationReceive, SupportedBlockAck, SupportedChannel,
    SupportedEdca, SupportedFilter, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool,
    TxCompletion, TxFrame, TxFrameBody, TxStatus, VifRole, WidthSet,
};

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

/// A buffer of the class `C` that holds an MPDU the model numbered.
#[derive(Debug)]
pub struct ModelNumbered<'id, C>(ModelBuffer<'id, C>);

impl<C> ModelNumbered<'_, C> {
    /// The MPDU's packet number, from its CCMP header after the
    /// `header_len` octets of its MAC header.
    pub fn packet_number(&self, header_len: usize) -> u64 {
        packet_number(&self.0.memory[header_len..])
    }
}

/// A transmit buffer of either class, fresh or numbered.
#[derive(Debug)]
pub enum ModelAnyBuffer<'id> {
    /// A small buffer.
    Small(ModelBuffer<'id, ModelSmall>),
    /// A large buffer.
    Large(ModelBuffer<'id, ModelLarge>),
    /// A numbered small buffer.
    SmallNumbered(ModelNumbered<'id, ModelSmall>),
    /// A numbered large buffer.
    LargeNumbered(ModelNumbered<'id, ModelLarge>),
}

impl ModelAnyBuffer<'_> {
    fn memory(&self) -> &[u8] {
        match self {
            Self::Small(buffer) | Self::SmallNumbered(ModelNumbered(buffer)) => &buffer.memory,
            Self::Large(buffer) | Self::LargeNumbered(ModelNumbered(buffer)) => &buffer.memory,
        }
    }

    fn memory_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Small(buffer) | Self::SmallNumbered(ModelNumbered(buffer)) => &mut buffer.memory,
            Self::Large(buffer) | Self::LargeNumbered(ModelNumbered(buffer)) => &mut buffer.memory,
        }
    }

    /// The buffer, numbered.
    fn into_numbered(self) -> Self {
        match self {
            Self::Small(buffer) => Self::SmallNumbered(ModelNumbered(buffer)),
            Self::Large(buffer) => Self::LargeNumbered(ModelNumbered(buffer)),
            numbered => numbered,
        }
    }
}

/// The packet number of the CCMP header `header` begins with.
fn packet_number(header: &[u8]) -> u64 {
    u64::from(header[0])
        | u64::from(header[1]) << 8
        | u64::from(header[4]) << 16
        | u64::from(header[5]) << 24
        | u64::from(header[6]) << 32
        | u64::from(header[7]) << 40
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

/// The types the model's events carry.
pub struct ModelTypes;

impl<'id> PhaseTypes<'id> for ModelTypes {
    type TicketKind = ModelTickets;
    type AnyTxBuffer = ModelAnyBuffer<'id>;
    type TxBuffers = Vec<ModelAnyBuffer<'id>>;
    type TxBodies = core::iter::Empty<(u8, ModelBody)>;
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

impl<'id> TxBufferPool<'id, ModelSmall> for ModelTypes {
    type Buffer = ModelBuffer<'id, ModelSmall>;
    type Numbered = ModelNumbered<'id, ModelSmall>;

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

    fn renew(buffer: Self::Numbered) -> Self::Buffer {
        buffer.0
    }
}

impl<'id> TxBufferPool<'id, ModelLarge> for ModelTypes {
    type Buffer = ModelBuffer<'id, ModelLarge>;
    type Numbered = ModelNumbered<'id, ModelLarge>;

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

    fn renew(buffer: Self::Numbered) -> Self::Buffer {
        buffer.0
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
}

impl ModelAir {
    /// Every transmission so far.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.borrow().clone()
    }

    /// Answer the next transmission: acknowledged or not.
    pub fn answer(&self, acknowledged: bool) {
        self.answers.borrow_mut().push_back(acknowledged);
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
enum Command<'id> {
    Submit(Prepared<'id>),
    InstallKey {
        index: u8,
    },
    RemoveKey {
        index: u8,
        removed: Removed<'id, ModelTypes>,
    },
    CloseGate,
    OpenGate,
}

/// An attempt prepared for its queue.
struct Prepared<'id> {
    role: VifRole,
    ticket: Slot<'id, ModelTickets>,
    buffer: ModelAnyBuffer<'id>,
    len: usize,
    header_len: usize,
    queue: usize,
    plan: Vec<Stage>,
    key: Option<u8>,
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
pub struct ModelHeld<'id> {
    air: Rc<ModelAir>,
    commands: Bounded<Command<'id>>,
    /// The received buffers the body gives back, at most one per buffer.
    recycled: Bounded<ModelRxBuffer>,
    settings: RefCell<Settings>,
    events: Bounded<LowerMacEvent<'id, ModelTypes>>,
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
}

impl<'id> ModelHeld<'id> {
    fn command(&self, command: Command<'id>) {
        self.commands.push(command);
        self.air.wake_driver();
    }

    fn post(&self, event: LowerMacEvent<'id, ModelTypes>) {
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
}

/// The handle's own state of the install: the hardware key indices it
/// hands out.
#[derive(Default)]
pub struct ModelLocal<'id> {
    next_key: u8,
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// The model's backend; only [`run_port`](oer_ieee80211_port::run_port)
/// holds it.
pub struct ModelBackend<C = ModelCaps> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    caps: PhantomData<fn() -> C>,
}

impl<C> ModelBackend<C> {
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

impl<C: Ieee80211Capabilities> PortBackend for ModelBackend<C> {
    type Event<'id> = LowerMacEvent<'id, ModelTypes>;
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;
    type Refusal = ();
    type Setup<'a, 'id>
        = ModelSetup<'a, C>
    where
        Self: 'a;
    type Phase<'a, 'id>
        = ModelPhase<'a, 'id, C>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = ModelResources<'id>;
    type Held<'id> = ModelHeld<'id>;
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
        held: &'a ModelHeld<'id>,
        _: &'a mut ModelLocal<'id>,
        sink: &'a mut dyn FnMut(LowerMacEvent<'id, ModelTypes>),
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        // The driver ends every command it holds and takes every buffer
        // back; then every event goes to the sink.
        poll_fn(move |cx| {
            if held.air.poisoned.get() {
                return Poll::Ready(Err(Poisoned { cause: ModelFault }));
            }
            if !held.commands.is_empty() || !held.recycled.is_empty() {
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
    ) -> (ModelHeld<'id>, ModelLocal<'id>, ModelResources<'id>) {
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
        };
        let resources = ModelResources {
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
        held: &'a ModelHeld<'id>,
        local: &'a mut ModelLocal<'id>,
    ) -> ModelPhase<'a, 'id, C>
    where
        'id: 'a,
    {
        ModelPhase {
            backend: self,
            held,
            local,
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, C> {
        let mut driver = Driver {
            queues: core::array::from_fn(|_| VecDeque::new()),
            keys: Vec::new(),
            packet_numbers: Vec::new(),
            gate_open: true,
        };
        poll_fn(move |cx| {
            held.air.passes.set(held.air.passes.get() + 1);
            if held.air.poisoned.get() || retire.is_requested() {
                return Poll::Ready(());
            }
            driver.serve(held);
            *held.air.driver.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        core::future::pending()
    }

    fn detach<'id>(&mut self, _: &ModelHeld<'id>) {}

    fn uninstall<'id>(self, _: ModelHeld<'id>, _: ModelLocal<'id>, (): (), (): ()) -> Rc<ModelAir> {
        self.air
    }

    fn abandon(&mut self) {}
}

impl<C: Ieee80211Capabilities> LowerMacBackend for ModelBackend<C> {
    type Caps = C;
}

/// The model's hardware, which only the driver owns.
struct Driver<'id> {
    /// The prepared attempts of each queue, the head first.
    queues: [VecDeque<Prepared<'id>>; 4],
    /// The installed hardware keys.
    keys: Vec<u8>,
    /// The last packet number of each key that numbered an MPDU.
    packet_numbers: Vec<(u8, u64)>,
    gate_open: bool,
}

impl<'id> Driver<'id> {
    /// Serve everything posted: the settings, the commands in order, the
    /// queues, then the air.
    fn serve(&mut self, held: &ModelHeld<'id>) {
        loop {
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
                    self.queues[queue].push_back(prepared);
                }
                Command::InstallKey { index } => self.keys.push(index),
                Command::RemoveKey { index, removed } => {
                    self.keys.retain(|key| *key != index);
                    held.post(LowerMacEvent::KeyRemoved(removed));
                }
                Command::CloseGate => {
                    self.gate_open = false;
                    // Every prepared, unpublished attempt ends as aborted.
                    for queue in &mut self.queues {
                        while let Some(prepared) = queue.pop_front() {
                            terminal(held, prepared, TxStatus::Aborted, 0, None);
                        }
                    }
                    let closed = held.closed.borrow_mut().take();
                    if let Some(closed) = closed {
                        held.post(LowerMacEvent::GateClosed(closed));
                    }
                }
                Command::OpenGate => self.gate_open = true,
            }
            if self.gate_open {
                self.publish(held);
            }
        }
        self.receive(held);
    }

    /// Publish the head of every queue and carry its retry plan out.
    fn publish(&mut self, held: &ModelHeld<'id>) {
        let busy = held.air.held.get();
        for (queue, busy) in self.queues.iter_mut().zip(busy) {
            if busy {
                continue;
            }
            while let Some(mut prepared) = queue.pop_front() {
                // A fresh MPDU under a key is numbered at its first
                // transmission; a numbered one keeps its number.
                if let Some(key) = prepared.key
                    && matches!(
                        prepared.buffer,
                        ModelAnyBuffer::Small(_) | ModelAnyBuffer::Large(_)
                    )
                {
                    let number = match self
                        .packet_numbers
                        .iter_mut()
                        .find(|(index, _)| *index == key)
                    {
                        Some((_, last)) => {
                            *last += 1;
                            *last
                        }
                        None => {
                            self.packet_numbers.push((key, 1));
                            1
                        }
                    };
                    let at = prepared.header_len;
                    prepared.buffer.memory_mut()[at..at + 8]
                        .copy_from_slice(&ccmp_header(number, 0));
                    prepared.buffer = prepared.buffer.into_numbered();
                }
                let mut transmissions = 0_u8;
                let mut last = None;
                let mut delivered = false;
                'plan: for stage in &prepared.plan {
                    for _ in 0..stage.transmissions {
                        transmissions = transmissions.saturating_add(1);
                        last = Some(stage.rate);
                        held.air.sent.borrow_mut().push(AirFrame {
                            role: prepared.role,
                            frame: prepared.buffer.memory()[..prepared.len].into(),
                            rate: stage.rate,
                            key: prepared.key.map(|key| (key, self.keys.contains(&key))),
                            packet_number: prepared.key.map(|_| {
                                packet_number(&prepared.buffer.memory()[prepared.header_len..])
                            }),
                        });
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
        }
    }

    /// Deliver the frames the air brought, as many as the receive buffers
    /// the body does not hold.
    fn receive(&mut self, held: &ModelHeld<'id>) {
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

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id>(
    held: &ModelHeld<'id>,
    prepared: Prepared<'id>,
    status: TxStatus,
    transmissions: u8,
    rate: Option<PhyRate>,
) {
    held.post(LowerMacEvent::TxCompleted {
        ticket: prepared.ticket,
        buffers: alloc::vec![prepared.buffer],
        completion: TxCompletion {
            status,
            ack_rssi_dbm: None,
            ack_snr_db: None,
            block_ack: None,
            transmissions,
            rate,
        },
        bodies: core::iter::empty(),
    });
}

/// The model's running phase: the body's handle over the queues.
pub struct ModelPhase<'a, 'id, C = ModelCaps> {
    backend: &'a mut ModelBackend<C>,
    held: &'a ModelHeld<'id>,
    local: &'a mut ModelLocal<'id>,
}

impl<'id, C> PortPhase for ModelPhase<'_, 'id, C> {
    type Event = LowerMacEvent<'id, ModelTypes>;
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

impl<'p, 'id: 'p, C: Ieee80211Capabilities> LowerMacPhase<'p, 'id> for ModelPhase<'p, 'id, C> {
    type Caps = C;
    type Types = ModelTypes;
    type Station<'v>
        = ModelInterface<'v, 'id, Station, C>
    where
        Self: 'v;
    type AccessPoint<'v>
        = ModelInterface<'v, 'id, AccessPoint, C>
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

    fn station(&mut self) -> ModelInterface<'_, 'id, Station, C> {
        ModelInterface {
            held: self.held,
            local: self.local,
            address: self.backend.addresses[0],
            role: PhantomData,
        }
    }

    fn access_point(&mut self) -> ModelInterface<'_, 'id, AccessPoint, C> {
        ModelInterface {
            held: self.held,
            local: self.local,
            address: self.backend.addresses[1],
            role: PhantomData,
        }
    }
}

/// One interface of the model's phase.
pub struct ModelInterface<'v, 'id, R, C = ModelCaps> {
    held: &'v ModelHeld<'id>,
    local: &'v mut ModelLocal<'id>,
    address: IndividualAddress,
    role: PhantomData<fn() -> (R, C)>,
}

/// A frame's memory, bound to its frame, ready to submit.
pub struct ModelTxBuffer<'id> {
    buffer: <ModelTypes as PhaseTypes<'id>>::AnyTxBuffer,
    len: usize,
    header_len: usize,
    plan: Vec<Stage>,
}

impl TxFrameBody for ModelTxBuffer<'_> {
    fn body_mut(&mut self) -> &mut [u8] {
        let (header, len) = (self.header_len, self.len);
        match &mut self.buffer {
            ModelAnyBuffer::Small(buffer) => &mut buffer.memory[header..len],
            ModelAnyBuffer::Large(buffer) => &mut buffer.memory[header..len],
            // A numbered MPDU's bytes are not the caller's to write.
            ModelAnyBuffer::SmallNumbered(_) | ModelAnyBuffer::LargeNumbered(_) => &mut [],
        }
    }
}

/// A key the model installed.
pub struct ModelPairwiseKey<'id, R> {
    index: u8,
    slot: Slot<'id, ModelPairwiseKeys<R>>,
}

/// A group key the model installed.
pub struct ModelGroupKey<'id, R> {
    index: u8,
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

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities> InterfacePhase<'p, 'id>
    for ModelInterface<'v, 'id, R, K>
where
    R: RoleKeys<'id, ModelTypes, Keys = ModelKeys<'id, R>>,
{
    type Caps = K;
    type Types = ModelTypes;
    type Role = R;
    type Fault = ModelFault;
    type TxBuffer = ModelTxBuffer<'id>;
    type TxBody = ModelBody;

    fn address(&self) -> IndividualAddress {
        self.address
    }

    fn tx_buffer<C>(
        &mut self,
        buffer: <ModelTypes as TxBufferPool<'id, C>>::Buffer,
        frame: ClassFrame<'p, C, K>,
    ) -> ModelTxBuffer<'id>
    where
        C: TxBufferClass,
        ModelTypes: TxBufferPool<'id, C>,
    {
        let frame = frame.into_frame();
        ModelTxBuffer {
            buffer: <ModelTypes as TxBufferPool<'id, C>>::erase(buffer),
            len: frame.mpdu_len(),
            header_len: header_len(&frame),
            plan: stages(&frame),
        }
    }

    fn retransmit<C>(
        &mut self,
        buffer: <ModelTypes as TxBufferPool<'id, C>>::Numbered,
        frame: ClassFrame<'p, C, K>,
    ) -> ModelTxBuffer<'id>
    where
        C: TxBufferClass,
        ModelTypes: TxBufferPool<'id, C>,
    {
        let frame = frame.into_frame();
        ModelTxBuffer {
            buffer: <ModelTypes as TxBufferPool<'id, C>>::erase_numbered(buffer),
            len: frame.mpdu_len(),
            header_len: header_len(&frame),
            plan: stages(&frame),
        }
    }

    fn submit(
        &mut self,
        attempt: InterfaceAttempt<'_, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>> {
        if self.held.air.poisoned.get() {
            return Err(Poisoned { cause: ModelFault });
        }
        let key = attempt.key.map(|key| match key {
            AttemptKey::Pairwise(key) => key.index,
            AttemptKey::Group(key) => key.index,
        });
        self.held.command(Command::Submit(Prepared {
            role: R::ROLE,
            ticket: attempt.ticket,
            buffer: attempt.frame.buffer,
            len: attempt.frame.len,
            header_len: attempt.frame.header_len,
            queue: attempt.access_category as usize,
            plan: attempt.frame.plan,
            key,
        }));
        let _ = Protection::None;
        Ok(())
    }

    fn install_pairwise_key(
        &mut self,
        slot: Slot<'id, ModelPairwiseKeys<R>>,
        _: IndividualAddress,
        _: Ccmp128Key,
    ) -> ModelPairwiseKey<'id, R> {
        let index = self.local.next_key;
        self.local.next_key = index.wrapping_add(1);
        self.held.command(Command::InstallKey { index });
        ModelPairwiseKey { index, slot }
    }

    fn install_group_key(
        &mut self,
        slot: Slot<'id, ModelGroupKeys<R>>,
        _: GroupKeyId,
        _: Ccmp128Key,
    ) -> ModelGroupKey<'id, R> {
        let index = self.local.next_key;
        self.local.next_key = index.wrapping_add(1);
        self.held.command(Command::InstallKey { index });
        ModelGroupKey { index, slot }
    }

    fn remove_pairwise_key(&mut self, key: ModelPairwiseKey<'id, R>) {
        self.held.command(Command::RemoveKey {
            index: key.index,
            removed: R::removed(RemovedKey::Pairwise(key.slot)),
        });
    }

    fn remove_group_key(&mut self, key: ModelGroupKey<'id, R>) {
        self.held.command(Command::RemoveKey {
            index: key.index,
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

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities> StationPhase<'p, 'id>
    for ModelInterface<'v, 'id, Station, C>
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

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities> AccessPointPhase<'p, 'id>
    for ModelInterface<'v, 'id, AccessPoint, C>
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
