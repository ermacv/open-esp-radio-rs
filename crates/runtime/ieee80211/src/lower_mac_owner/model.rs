//! An in-memory lower-MAC backend on the owner shape
//! (`oer_ieee80211_lower_mac::owner`), over [`TxPermits`](super::TxPermits),
//! for host tests of the shape and of the code that drives it.
//!
//! [`OwnedModel::new`] takes the install's channel and interface addresses,
//! so no phase runs untuned, and returns the backend, which a test hands to
//! [`run_port`](oer_radio_port::run_port), and a [`ModelHandle`] that plays
//! the air and the world: it ends published attempts, refuses the next
//! enable or poisons the backend, and wakes the futures that wait on it
//! (an events consumer, a closing gate) when it does. An end the world
//! posts is served by the model's driver (`PortBackend::drive`), as an
//! interrupt's status is: it gives the attempt's permits back, then queues
//! the completion, so a body that reads no event still gets its queue back. Its held part of the install keeps the
//! transmit permits in [`ModelPermitPool`]s, first-in, first-out pools,
//! and the body's resources give each interface its transmit seats
//! ([`TxSeats`]).
//!
//! Its tokens have no value outside it: no caller mints a key, a key of a
//! station's client is not an access point's, and a group key is not a
//! pairwise key.
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_lower_mac::Station;
//! # use oer_ieee80211_runtime::lower_mac_owner::model::ModelPairwiseKey;
//! fn forge<'id>() -> ModelPairwiseKey<'id, Station> {
//!     ModelPairwiseKey { index: 0, slot: todo!() }
//! }
//! ```
//!
//! ```compile_fail,E0308
//! # use oer_ieee80211_lower_mac::{AccessPoint, LowerMacClient, Station};
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{ModelClient, ModelPairwiseKey};
//! fn cross<'id>(client: &ModelClient<'_, 'id, AccessPoint>, key: ModelPairwiseKey<'id, Station>) {
//!     let _ = client.remove_pairwise_key(key);
//! }
//! ```
//!
//! ```compile_fail,E0308
//! # use oer_ieee80211_lower_mac::{LowerMacClient, Station};
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{ModelClient, ModelGroupKey};
//! fn scope<'id>(client: &ModelClient<'_, 'id, Station>, key: ModelGroupKey<'id, Station>) {
//!     let _ = client.remove_pairwise_key(key);
//! }
//! ```
//!
//! Its compiling twin gives a key back to its own role's client and kind:
//!
//! ```
//! # use oer_ieee80211_lower_mac::{AccessPoint, LowerMacClient};
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{ModelClient, ModelPairwiseKey};
//! fn own<'id>(client: &ModelClient<'_, 'id, AccessPoint>, key: ModelPairwiseKey<'id, AccessPoint>) {
//!     let _ = client.remove_pairwise_key(key);
//! }
//! ```

use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    task::{Poll, Waker},
};

use oer_radio_port::{
    CancelError, ClockError, EnableResult, EventsLost, Lent, Line, Mint, Permit, PermitPool,
    PhaseParts, Poisoned, PortBackend, PortResult, RadioPortClient, RadioPortEvents, RadioWork,
    Retire, Seat, Slot, Work,
};
use oer_time::RadioInstant;

use oer_ieee80211_lower_mac::{
    AccessPoint, AccessPointClient, AddressSetup, AttemptKey, Ccmp128Key, Channel, GroupKeyId,
    Ieee80211Capabilities, Ieee80211Radio, IndividualAddress, InterfaceRole, KeyScope,
    LowerMacBackend, LowerMacCapabilities, LowerMacClient, LowerMacControl, LowerMacEvent,
    LowerMacEvents, LowerMacSetup, MacAddress, PhaseAttempt, PhyRate, RxMeta, Station,
    StationClient, StationReceive, SupportedBlockAck, SupportedChannel, SupportedFilter, TX_SEATS,
    Tid, TxCompletion, TxFrame, TxFrameBody, TxGated, TxId, TxResponse, TxSeats, TxStatus, VifRole,
    model::{MODEL_CAPABILITIES, ModelBody, ModelFault, ModelRxBuffer},
};

use super::{BufferPermit, QueuePermit, QueuePool, TxBufferKind, TxPermits, TxQueueKind};

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// Transmit buffers the model holds.
pub const OWNED_MODEL_TX_BUFFERS: usize = 3;
/// Pairwise and group key slots of each role.
pub const OWNED_MODEL_KEY_SLOTS: usize = 2;
/// Receive Block Ack slots of each role.
pub const OWNED_MODEL_BLOCK_ACK_SLOTS: usize = 2;
/// Transmit queues of the model ([`MODEL_CAPABILITIES`]).
pub const OWNED_MODEL_TX_QUEUES: usize = MODEL_CAPABILITIES.tx_queues as usize;

/// An interface role of the model's clients: its table of receive Block
/// Ack agreements in the held part of the install.
pub trait ModelRole: InterfaceRole + Sized {
    #[doc(hidden)]
    fn block_acks<'a, 'id>(held: &'a ModelHeld<'id>) -> &'a BlockAckTable<'id, Self>;
}

impl ModelRole for Station {
    fn block_acks<'a, 'id>(held: &'a ModelHeld<'id>) -> &'a BlockAckTable<'id, Self> {
        &held.station_block_acks
    }
}

impl ModelRole for AccessPoint {
    fn block_acks<'a, 'id>(held: &'a ModelHeld<'id>) -> &'a BlockAckTable<'id, Self> {
        &held.access_point_block_acks
    }
}

/// The kind of a role's pairwise key slots.
pub struct ModelPairwiseKeys<R>(PhantomData<fn() -> R>);
/// The kind of a role's group key slots.
pub struct ModelGroupKeys<R>(PhantomData<fn() -> R>);
/// The kind of a role's receive Block Ack slots.
pub struct ModelBlockAcks<R>(PhantomData<fn() -> R>);

/// A role's receive Block Ack agreements: one per peer and TID, each with
/// its slot.
pub type BlockAckTable<'id, R> =
    RefCell<Vec<(IndividualAddress, Tid, Slot<'id, ModelBlockAcks<R>>)>>;

/// Which installed key an attempt uses: its role, whether it is pairwise,
/// and its slot.
type KeyUse = (VifRole, bool, u8);

/// One admitted attempt.
struct Admission {
    id: TxId,
    role: VifRole,
    key: Option<KeyUse>,
    published: bool,
    body: Option<ModelBody>,
}

struct State {
    /// The channel the next `Enable` tunes to.
    next_channel: Channel,
    /// The channel the last `Enable` tuned to, the install's before one.
    tuned: Channel,
    addresses: [MacAddress; 2],
    gate_open: bool,
    /// The wakers of the futures waiting on the world or the driver.
    waiting: Vec<Waker>,
    /// The ends the world posted, which the driver has not served yet, as
    /// an interrupt's latched status.
    posted: Vec<(TxId, TxStatus)>,
    admitted: Vec<Admission>,
    events: VecDeque<ModelEvent>,
    next_id: u32,
    installed_keys: Vec<(VifRole, u8, KeyScope)>,
    refuse_enable: bool,
    poisoned: bool,
    lifecycle: Vec<&'static str>,
}

type Shared = Rc<RefCell<State>>;

/// The world's side of an [`OwnedModel`].
#[derive(Clone)]
pub struct ModelHandle(Shared);

impl ModelHandle {
    /// End the published attempt `id` on the air with `status`, as an
    /// interrupt posts the status: the driver serves it, gives the
    /// attempt's permits back and queues its completion. `false` when no
    /// such attempt is published.
    pub fn complete(&self, id: TxId, status: TxStatus) -> bool {
        let mut state = self.0.borrow_mut();
        if !state
            .admitted
            .iter()
            .any(|admitted| admitted.id == id && admitted.published)
        {
            return false;
        }
        state.posted.push((id, status));
        wake(&mut state);
        true
    }

    /// Refuse the next `Enable`, as a radio that rejects the channel.
    pub fn refuse_enable(&self, refuse: bool) {
        self.0.borrow_mut().refuse_enable = refuse;
    }

    /// Poison the backend.
    pub fn poison(&self) {
        let mut state = self.0.borrow_mut();
        state.poisoned = true;
        wake(&mut state);
    }

    /// The lifecycle steps the backend took, in order.
    pub fn lifecycle(&self) -> Vec<&'static str> {
        self.0.borrow().lifecycle.clone()
    }

    /// The channel the radio is tuned to.
    pub fn tuned(&self) -> Channel {
        self.0.borrow().tuned
    }

    /// The address of the interface of `role`.
    pub fn address(&self, role: VifRole) -> MacAddress {
        self.0.borrow().addresses[role_index(role)]
    }

    /// Whether the transmit gate is open.
    pub fn gate_open(&self) -> bool {
        self.0.borrow().gate_open
    }

    /// The installed keys: their role, slot and scope.
    pub fn installed_keys(&self) -> Vec<(VifRole, u8, KeyScope)> {
        self.0.borrow().installed_keys.clone()
    }
}

/// End the admitted attempt at `index`: queue its completion with its body.
fn end(state: &mut State, index: usize, status: TxStatus) {
    let admitted = state.admitted.remove(index);
    state.events.push_back(ModelEvent::Completed(
        TxCompletion {
            id: admitted.id,
            status,
            ack_rssi_dbm: None,
            ack_snr_db: None,
            block_ack: None,
        },
        admitted.body,
    ));
    wake(state);
}

/// Serve the ends the world posted: give each attempt's permits back to
/// `tx`, then queue its completion, as the backend's driver does with an
/// interrupt's status.
fn serve(state: &mut State, tx: &ModelTxPermits<'_>) {
    for (id, status) in core::mem::take(&mut state.posted) {
        if let Some(index) = state.admitted.iter().position(|admitted| admitted.id == id) {
            tx.complete(id);
            end(state, index, status);
        }
    }
}

/// Wake every future waiting on the world: the world moved.
fn wake(state: &mut State) {
    for waker in state.waiting.drain(..) {
        waker.wake();
    }
}

/// One owned event of the model.
pub enum ModelEvent {
    /// The completion of one admitted attempt, with its body.
    Completed(TxCompletion, Option<ModelBody>),
}

/// The model's backend; only [`run_port`](oer_radio_port::run_port) holds it.
pub struct OwnedModel(Shared);

impl OwnedModel {
    /// A disabled backend on `channel` whose station and access-point
    /// interfaces have `addresses`, and its world.
    pub fn new(
        channel: SupportedChannel<ModelCaps>,
        addresses: [MacAddress; 2],
    ) -> (Self, ModelHandle) {
        let state = State {
            next_channel: channel.channel(),
            tuned: channel.channel(),
            addresses,
            gate_open: true,
            waiting: Vec::new(),
            posted: Vec::new(),
            admitted: Vec::new(),
            events: VecDeque::new(),
            next_id: 0,
            installed_keys: Vec::new(),
            refuse_enable: false,
            poisoned: false,
            lifecycle: Vec::new(),
        };
        let shared = Rc::new(RefCell::new(state));
        (Self(shared.clone()), ModelHandle(shared))
    }
}

/// The transmit permits of one install of the model.
pub type ModelTxPermits<'id> = TxPermits<
    'id,
    ModelCaps,
    ModelPermitPool<'id, TxQueueKind>,
    ModelPermitPool<'id, TxBufferKind>,
    OWNED_MODEL_TX_QUEUES,
>;

/// The model's held part of one install: its transmit permits and each
/// role's receive Block Ack agreements.
pub struct ModelHeld<'id> {
    pub tx: ModelTxPermits<'id>,
    station_block_acks: BlockAckTable<'id, Station>,
    access_point_block_acks: BlockAckTable<'id, AccessPoint>,
}

/// The transmit seats of one interface of the model.
pub type ModelTxSeats<'id> = TxSeats<'id, TxQueueKind, TxBufferKind>;

/// The port-lifetime resources the body of one install of the model
/// receives: each role's slots and transmit seats.
pub struct ModelResources<'id> {
    pub station_tx: ModelTxSeats<'id>,
    pub access_point_tx: ModelTxSeats<'id>,
    pub station_pairwise_keys: [Slot<'id, ModelPairwiseKeys<Station>>; OWNED_MODEL_KEY_SLOTS],
    pub station_group_keys: [Slot<'id, ModelGroupKeys<Station>>; OWNED_MODEL_KEY_SLOTS],
    pub access_point_pairwise_keys:
        [Slot<'id, ModelPairwiseKeys<AccessPoint>>; OWNED_MODEL_KEY_SLOTS],
    pub access_point_group_keys: [Slot<'id, ModelGroupKeys<AccessPoint>>; OWNED_MODEL_KEY_SLOTS],
    pub station_block_acks: [Slot<'id, ModelBlockAcks<Station>>; OWNED_MODEL_BLOCK_ACK_SLOTS],
    pub access_point_block_acks:
        [Slot<'id, ModelBlockAcks<AccessPoint>>; OWNED_MODEL_BLOCK_ACK_SLOTS],
}

/// The model's disabled-only settings.
pub struct ModelSetup(Shared);

impl LowerMacSetup for ModelSetup {
    type Caps = ModelCaps;

    fn set_channel(&mut self, channel: SupportedChannel<ModelCaps>) {
        self.0.borrow_mut().next_channel = channel.channel();
    }
}

impl AddressSetup for ModelSetup {
    fn set_address(&mut self, role: VifRole, address: IndividualAddress) {
        self.0.borrow_mut().addresses[role_index(role)] = address.get();
    }
}

const fn role_index(role: VifRole) -> usize {
    match role {
        VifRole::Station => 0,
        VifRole::AccessPoint => 1,
    }
}

/// The model's phase control.
pub struct ModelControl(Shared);

/// A gate closing: dropped before it yields its guard, it opens the gate
/// again.
struct Closing<'g> {
    control: Option<&'g ModelControl>,
}

impl Drop for Closing<'_> {
    fn drop(&mut self) {
        if let Some(control) = self.control {
            open(&mut control.0.borrow_mut());
        }
    }
}

/// Open the gate: the attempts it held are published.
fn open(state: &mut State) {
    state.gate_open = true;
    for admitted in &mut state.admitted {
        admitted.published = true;
    }
}

/// The model's closed transmit gate; dropping it opens the gate and
/// publishes the attempts it held.
pub struct ModelGated<'g>(&'g ModelControl);

impl TxGated for ModelGated<'_> {}

impl Drop for ModelGated<'_> {
    fn drop(&mut self) {
        open(&mut self.0.0.borrow_mut());
    }
}

impl LowerMacControl for ModelControl {
    type Caps = ModelCaps;
    type Gated<'g> = ModelGated<'g>;

    fn close_gate(&mut self) -> impl Future<Output = ModelGated<'_>> {
        // Admission stops at once, before the future is polled.
        self.0.borrow_mut().gate_open = false;
        let mut closing = Closing {
            control: Some(&*self),
        };
        poll_fn(move |context| {
            // The whole guard moves into the future, so dropping the future
            // drops it; a closure that named only its field would copy that.
            let closing = &mut closing;
            let control = closing
                .control
                .expect("a closed gate yields its guard once");
            let mut state = control.0.borrow_mut();
            if state.admitted.iter().any(|admitted| admitted.published) {
                state.waiting.push(context.waker().clone());
                return Poll::Pending;
            }
            closing.control = None;
            Poll::Ready(ModelGated(control))
        })
    }

    fn set_edca(&mut self, _: oer_ieee80211_lower_mac::SupportedEdca<ModelCaps>) {}

    fn set_rx_beacon_priority(&mut self, _: oer_ieee80211_lower_mac::RxBeaconPriority) {}
}

/// The model's client of the interface of role `R` in a phase of the
/// install `'id`.
pub struct ModelClient<'a, 'id, R> {
    state: Shared,
    held: &'a ModelHeld<'id>,
    role: PhantomData<fn() -> R>,
}

/// The model's clients: one per interface role.
pub struct ModelClients<'a, 'id> {
    pub station: ModelClient<'a, 'id, Station>,
    pub access_point: ModelClient<'a, 'id, AccessPoint>,
}

/// The token of one admitted attempt.
pub struct ModelToken(TxId);

impl RadioWork for ModelToken {
    type Id = TxId;

    fn id(&self) -> TxId {
        self.0
    }
}

/// One frame's memory, bound to its buffer permit: the header the request
/// wrote, then the body the caller writes.
pub struct ModelTxBuffer<'c, 'id> {
    pub bytes: Vec<u8>,
    header: usize,
    /// The response the frame's kind solicits.
    pub response: TxResponse,
    /// The rate the frame's kind took.
    pub rate: PhyRate,
    permit: BufferPermit<'c, 'id>,
}

impl TxFrameBody for ModelTxBuffer<'_, '_> {
    fn body_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.header..]
    }
}

/// An installed pairwise key of a client of role `R`; it holds its slot.
pub struct ModelPairwiseKey<'id, R> {
    index: u8,
    slot: Slot<'id, ModelPairwiseKeys<R>>,
}

/// An installed group key of a client of role `R`; it holds its slot.
pub struct ModelGroupKey<'id, R> {
    index: u8,
    slot: Slot<'id, ModelGroupKeys<R>>,
}

impl<R> RadioPortClient for ModelClient<'_, '_, R> {
    type Token = ModelToken;
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Ieee80211Radio>, ClockError, ModelFault>> + '_
    {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn end(
        &self,
        token: ModelToken,
    ) -> impl Future<Output = PortResult<(), CancelError, ModelFault>> + '_ {
        // A cancel is a request: the driver ends the attempt.
        let mut state = self.state.borrow_mut();
        let ended = if state.admitted.iter().any(|admitted| admitted.id == token.0) {
            state.posted.push((token.0, TxStatus::Aborted));
            wake(&mut state);
            Ok(())
        } else {
            Err(CancelError::NotRunning)
        };
        core::future::ready(Ok(ended))
    }
}

impl<'a, 'id, R: ModelRole> LowerMacClient<'id> for ModelClient<'a, 'id, R> {
    type Caps = ModelCaps;
    type QueueKind = TxQueueKind;
    type BufferKind = TxBufferKind;
    type Queue<'c>
        = QueuePermit<'c, 'id>
    where
        Self: 'c,
        'id: 'c;
    type TxBuffer<'c>
        = ModelTxBuffer<'c, 'id>
    where
        Self: 'c,
        'id: 'c;
    type TxBody = ModelBody;
    type PairwiseKeyKind = ModelPairwiseKeys<R>;
    type GroupKeyKind = ModelGroupKeys<R>;
    type PairwiseKey = ModelPairwiseKey<'id, R>;
    type GroupKey = ModelGroupKey<'id, R>;
    type BlockAckKind = ModelBlockAcks<R>;

    fn channel(&self) -> Channel {
        self.state.borrow().tuned
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
        frame: TxFrame<'c, ModelCaps>,
        seat: &'s mut Seat<'id, TxBufferKind>,
    ) -> ModelTxBuffer<'c, 'id>
    where
        'id: 'c,
        'c: 's,
    {
        let permit = self.held.tx.buffer(seat).await;
        let (bytes, header, response, rate) = match frame {
            TxFrame::Individual { frame, rate } => {
                let header = frame.header().header();
                let mut bytes = alloc::vec![0; frame.len().get()];
                let written = header.encode(&mut bytes, false);
                (bytes, written, TxResponse::Ack, rate.get())
            }
            TxFrame::IndividualNoAck { frame, rate } => {
                let header = frame.header().header();
                let mut bytes = alloc::vec![0; frame.len().get()];
                let written = header.encode(&mut bytes, true);
                (bytes, written, TxResponse::None, rate.rate().get())
            }
            TxFrame::Group { frame, rate } => {
                let mut bytes = alloc::vec![0; frame.len().get()];
                // A group frame solicits no response: its QoS Ack Policy is
                // No Ack.
                let written = frame.header().encode(&mut bytes, true);
                (bytes, written, TxResponse::None, rate.get())
            }
            TxFrame::BlockAckRequest {
                receiver,
                transmitter,
                tid,
                starting_sequence,
                rate,
            } => {
                let bytes = oer_ieee80211_mac::block_ack::encode_block_ack_request(
                    receiver.get(),
                    transmitter,
                    tid.get(),
                    starting_sequence,
                )
                .to_vec();
                let header = bytes.len();
                (bytes, header, TxResponse::BlockAck, rate.rate().get())
            }
        };
        ModelTxBuffer {
            bytes,
            header,
            response,
            rate,
            permit,
        }
    }

    fn submit<'p>(
        &'p self,
        attempt: PhaseAttempt<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<ModelFault>>
    where
        'id: 'p,
    {
        let mut state = self.state.borrow_mut();
        if state.poisoned {
            return Err(Poisoned { cause: ModelFault });
        }
        let PhaseAttempt {
            queue,
            frame,
            body,
            key,
            ..
        } = attempt;
        let key = key.map(|key| match key {
            AttemptKey::Pairwise(key) => (R::ROLE, true, key.index),
            AttemptKey::Group(key) => (R::ROLE, false, key.index),
        });
        let id = TxId(state.next_id);
        state.next_id += 1;
        let published = state.gate_open;
        state.admitted.push(Admission {
            id,
            role: R::ROLE,
            key,
            published,
            body,
        });
        drop(state);
        self.held.tx.admit(id, queue, frame.permit);
        Ok(Work::new(self, ModelToken(id)))
    }

    fn install_pairwise_key(
        &self,
        slot: Slot<'id, ModelPairwiseKeys<R>>,
        peer: IndividualAddress,
        _: Ccmp128Key,
    ) -> ModelPairwiseKey<'id, R> {
        let index = slot.index();
        self.state.borrow_mut().installed_keys.push((
            R::ROLE,
            index,
            KeyScope::Pairwise { peer: peer.get() },
        ));
        ModelPairwiseKey { index, slot }
    }

    fn install_group_key(
        &self,
        slot: Slot<'id, ModelGroupKeys<R>>,
        key_id: GroupKeyId,
        _: Ccmp128Key,
    ) -> ModelGroupKey<'id, R> {
        let index = slot.index();
        self.state.borrow_mut().installed_keys.push((
            R::ROLE,
            index,
            KeyScope::Group {
                key_id: key_id.get(),
            },
        ));
        ModelGroupKey { index, slot }
    }

    fn remove_pairwise_key(
        &self,
        key: ModelPairwiseKey<'id, R>,
    ) -> impl Future<Output = Slot<'id, ModelPairwiseKeys<R>>> + '_ {
        let state = &self.state;
        let mut key = Some(key);
        poll_fn(move |context| {
            let index = key.as_ref().expect("a removal ends once").index;
            if !unused(state, (R::ROLE, true, index), context) {
                return Poll::Pending;
            }
            uninstall(state, R::ROLE, index, true);
            Poll::Ready(key.take().expect("a removal ends once").slot)
        })
    }

    fn remove_group_key(
        &self,
        key: ModelGroupKey<'id, R>,
    ) -> impl Future<Output = Slot<'id, ModelGroupKeys<R>>> + '_ {
        let state = &self.state;
        let mut key = Some(key);
        poll_fn(move |context| {
            let index = key.as_ref().expect("a removal ends once").index;
            if !unused(state, (R::ROLE, false, index), context) {
                return Poll::Pending;
            }
            uninstall(state, R::ROLE, index, false);
            Poll::Ready(key.take().expect("a removal ends once").slot)
        })
    }

    fn add_block_ack(
        &self,
        slot: Slot<'id, ModelBlockAcks<R>>,
        agreement: SupportedBlockAck<ModelCaps>,
    ) -> Option<Slot<'id, ModelBlockAcks<R>>> {
        let (peer, tid, ..) = agreement.get();
        let mut table = R::block_acks(self.held).borrow_mut();
        let replaced = table
            .iter()
            .position(|(other, other_tid, _)| (*other, *other_tid) == (peer, tid))
            .map(|index| table.swap_remove(index).2);
        table.push((peer, tid, slot));
        replaced
    }

    fn remove_block_ack(
        &self,
        peer: IndividualAddress,
        tid: Tid,
    ) -> Option<Slot<'id, ModelBlockAcks<R>>> {
        let mut table = R::block_acks(self.held).borrow_mut();
        let index = table
            .iter()
            .position(|(other, other_tid, _)| (*other, *other_tid) == (peer, tid))?;
        Some(table.swap_remove(index).2)
    }
}

impl<'a, 'id> StationClient<'id> for ModelClient<'a, 'id, Station> {
    fn configure(&self, _: StationReceive<ModelCaps>) -> impl Future<Output = ()> + '_ {
        let state = &self.state;
        poll_fn(move |context| quiet(state, VifRole::Station, context))
    }

    fn set_bss_color(&self, _: oer_ieee80211_lower_mac::BssColor) {}
}

impl<'a, 'id> AccessPointClient<'id> for ModelClient<'a, 'id, AccessPoint> {
    fn configure(
        &self,
        _: SupportedFilter<ModelCaps, AccessPoint>,
    ) -> impl Future<Output = ()> + '_ {
        let state = &self.state;
        poll_fn(move |context| quiet(state, VifRole::AccessPoint, context))
    }
}

/// Whether no attempt in flight uses `key`: one the closed gate holds is
/// posted to end as aborted, and a published one is waited for.
fn unused(state: &Shared, key: KeyUse, context: &mut core::task::Context<'_>) -> bool {
    settle(state, |admitted| admitted.key == Some(key), context)
}

/// Whether no admitted attempt matches `uses`: the held ones are posted to
/// end as aborted, which the driver serves, and the waker waits for the
/// rest.
fn settle(
    state: &Shared,
    uses: impl Fn(&Admission) -> bool,
    context: &mut core::task::Context<'_>,
) -> bool {
    let mut state = state.borrow_mut();
    let held: Vec<TxId> = state
        .admitted
        .iter()
        .filter(|admitted| uses(admitted) && !admitted.published)
        .map(|admitted| admitted.id)
        .filter(|id| !state.posted.iter().any(|(posted, _)| posted == id))
        .collect();
    if !held.is_empty() {
        for id in held {
            state.posted.push((id, TxStatus::Aborted));
        }
        // The driver serves what was posted: wake it, before this waiter
        // joins the line.
        wake(&mut state);
    }
    if state.admitted.iter().any(&uses) {
        state.waiting.push(context.waker().clone());
        false
    } else {
        true
    }
}

/// Forget an installed key of `role` in slot `index`.
fn uninstall(state: &Shared, role: VifRole, index: u8, pairwise: bool) {
    state
        .borrow_mut()
        .installed_keys
        .retain(|(other, slot, scope)| {
            (*other, *slot, matches!(scope, KeyScope::Pairwise { .. })) != (role, index, pairwise)
        });
}

/// Ready once no attempt of `role` is in flight: held ones end as aborted,
/// published ones are waited for.
fn quiet(state: &Shared, role: VifRole, context: &mut core::task::Context<'_>) -> Poll<()> {
    if settle(state, |admitted| admitted.role == role, context) {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
}

/// The model's event consumer: the driver gave each attempt's permits
/// back before it queued the attempt's completion.
pub struct ModelEvents(Shared);

impl RadioPortEvents for ModelEvents {
    type Event = ModelEvent;
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<ModelEvent, EventsLost, ModelFault>> + '_ {
        poll_fn(|context| {
            let mut state = self.0.borrow_mut();
            match state.events.pop_front() {
                Some(event) => Poll::Ready(Ok(Ok(event))),
                None if state.poisoned => Poll::Ready(Err(Poisoned { cause: ModelFault })),
                None => {
                    state.waiting.push(context.waker().clone());
                    Poll::Pending
                }
            }
        })
    }
}

impl LowerMacEvents for ModelEvents {
    type RxBuffer = ModelRxBuffer;
    type TxBodies = Option<ModelBody>;

    fn view(event: &ModelEvent) -> LowerMacEvent<'_> {
        match event {
            ModelEvent::Completed(completion, _) => LowerMacEvent::TxCompleted(*completion),
        }
    }

    fn into_received(event: ModelEvent) -> Result<(ModelRxBuffer, RxMeta), ModelEvent> {
        Err(event)
    }

    fn into_completed(event: ModelEvent) -> Result<(TxCompletion, Option<ModelBody>), ModelEvent> {
        match event {
            ModelEvent::Completed(completion, body) => Ok((completion, body)),
        }
    }
}

impl PortBackend for OwnedModel {
    type Event = ModelEvent;
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;
    type Refusal = &'static str;
    type Setup<'a, 'id> = ModelSetup;
    type Events<'a, 'id>
        = ModelEvents
    where
        'id: 'a;
    type Control<'a, 'id>
        = ModelControl
    where
        'id: 'a;
    type Clients<'a, 'id>
        = ModelClients<'a, 'id>
    where
        'id: 'a;
    type Resources<'id> = ModelResources<'id>;
    type Held<'id> = ModelHeld<'id>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = ModelHandle;

    fn enable(&mut self) -> impl Future<Output = EnableResult<&'static str, ModelFault>> + '_ {
        let mut state = self.0.borrow_mut();
        let result = if state.poisoned {
            Err(Poisoned { cause: ModelFault })
        } else if state.refuse_enable {
            Ok(Err("the radio refused the channel"))
        } else {
            // The channel the install or the setup set is tuned now.
            state.tuned = state.next_channel;
            state.lifecycle.push("enable");
            Ok(Ok(()))
        };
        core::future::ready(result)
    }

    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        sink: &'a mut dyn FnMut(ModelEvent),
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        let mut state = self.0.borrow_mut();
        let result = if state.poisoned {
            Err(Poisoned { cause: ModelFault })
        } else {
            serve(&mut state, &held.tx);
            while let Some(admitted) = state.admitted.first() {
                let id = admitted.id;
                end(&mut state, 0, TxStatus::Aborted);
                held.tx.complete(id);
            }
            while let Some(event) = state.events.pop_front() {
                sink(event);
            }
            debug_assert!(held.tx.idle(), "every permit is back");
            state.lifecycle.push("disable");
            Ok(())
        };
        core::future::ready(result)
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Ieee80211Radio>, ClockError, ModelFault>> + '_
    {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn resources<'id>(&mut self, mut mint: Mint<'id>) -> (Self::Held<'id>, Self::Resources<'id>) {
        let buffers: [Permit<'id, TxBufferKind>; OWNED_MODEL_TX_BUFFERS] = mint.permits();
        // The model's buffer line grows on the heap, so it holds a place for
        // every seat without the line's size.
        let (queue_line, _, [station_tx, access_point_tx]) = TxSeats::mint(&mut mint);
        let held = ModelHeld {
            tx: TxPermits::new(&mut mint, queue_line, ModelPermitPool::new(buffers)),
            station_block_acks: RefCell::new(Vec::new()),
            access_point_block_acks: RefCell::new(Vec::new()),
        };
        let resources = ModelResources {
            station_tx,
            access_point_tx,
            station_pairwise_keys: mint.slots(),
            station_group_keys: mint.slots(),
            access_point_pairwise_keys: mint.slots(),
            access_point_group_keys: mint.slots(),
            station_block_acks: mint.slots(),
            access_point_block_acks: mint.slots(),
        };
        (held, resources)
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(ModelSetup(self.0.clone()))
    }

    fn split<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        let state = &self.0;
        PhaseParts {
            events: Lent::new(ModelEvents(state.clone())),
            control: Lent::new(ModelControl(state.clone())),
            clients: Lent::new(ModelClients {
                station: ModelClient {
                    state: state.clone(),
                    held,
                    role: PhantomData,
                },
                access_point: ModelClient {
                    state: state.clone(),
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
    ) -> impl Future<Output = ()> + use<'p, 'id> {
        let state = self.0.clone();
        // It serves what the world posted at every poll; an idle poll reads
        // one vector's length.
        poll_fn(move |context| {
            let mut state = state.borrow_mut();
            serve(&mut state, &held.tx);
            if retire.is_requested() {
                state.lifecycle.push("retire");
                Poll::Ready(())
            } else {
                state.waiting.push(context.waker().clone());
                Poll::Pending
            }
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        // The model's driver retires at its next poll: it never misses.
        core::future::pending()
    }

    fn detach<'id>(&mut self, _: &Self::Held<'id>) {
        self.0.borrow_mut().lifecycle.push("detach");
    }

    fn uninstall<'id>(self, _: Self::Held<'id>, (): (), (): ()) -> ModelHandle {
        self.0.borrow_mut().lifecycle.push("uninstall");
        ModelHandle(self.0)
    }

    fn abandon(&mut self) {
        self.0.borrow_mut().lifecycle.push("abandon");
    }
}

impl LowerMacBackend for OwnedModel {
    type Caps = ModelCaps;
}

impl<'id> QueuePool<'id> for ModelPermitPool<'id, TxQueueKind> {
    fn of(permit: Permit<'id, TxQueueKind>, _: Line<'id, TxQueueKind, TX_SEATS>) -> Self {
        Self::new([permit])
    }
}

/// A first-in, first-out pool of permits, whose line grows on the heap: a
/// place for every seat that waits.
pub struct ModelPermitPool<'id, Kind> {
    free: RefCell<Vec<Permit<'id, Kind>>>,
    /// The waiting takers, in arrival order: each one's ticket and waker.
    line: Waiting,
    next: Cell<u32>,
}

type Waiting = RefCell<VecDeque<(u32, Waker)>>;

/// Wake the taker at the head of `line`, which a free permit now serves.
fn wake_head(line: &Waiting) {
    if let Some((_, waker)) = line.borrow().front() {
        waker.wake_by_ref();
    }
}

impl<'id, Kind> ModelPermitPool<'id, Kind> {
    /// A pool of `permits`.
    pub fn new(permits: impl IntoIterator<Item = Permit<'id, Kind>>) -> Self {
        Self {
            free: RefCell::new(permits.into_iter().collect()),
            line: RefCell::default(),
            next: Cell::new(0),
        }
    }
}

impl<'id, Kind> PermitPool<'id, Kind> for ModelPermitPool<'id, Kind> {
    fn take<'a>(
        &'a self,
        _: &'a mut Seat<'id, Kind>,
    ) -> impl Future<Output = Permit<'id, Kind>> + 'a {
        let mut place = Place {
            line: &self.line,
            ticket: None,
        };
        poll_fn(move |context| {
            // The whole place moves into the future, so dropping the future
            // drops it; a closure that named only its field would copy that.
            let place = &mut place;
            let mine = match place.ticket {
                Some(mine) => {
                    // Keep the latest waker of this taker.
                    if let Some(entry) = self
                        .line
                        .borrow_mut()
                        .iter_mut()
                        .find(|(ticket, _)| *ticket == mine)
                    {
                        entry.1.clone_from(context.waker());
                    }
                    mine
                }
                None => {
                    let issued = self.next.get();
                    self.next.set(issued + 1);
                    self.line
                        .borrow_mut()
                        .push_back((issued, context.waker().clone()));
                    place.ticket = Some(issued);
                    issued
                }
            };
            if self.line.borrow().front().map(|(ticket, _)| *ticket) != Some(mine) {
                return Poll::Pending;
            }
            let served = self.free.borrow_mut().pop();
            match served {
                Some(permit) => {
                    self.line.borrow_mut().pop_front();
                    place.ticket = None;
                    // A permit still free serves the next taker: wake it.
                    if !self.free.borrow().is_empty() {
                        wake_head(&self.line);
                    }
                    Poll::Ready(permit)
                }
                None => Poll::Pending,
            }
        })
    }

    fn release(&self, permit: Permit<'id, Kind>) {
        self.free.borrow_mut().push(permit);
        wake_head(&self.line);
    }
}

/// A taker's place in a [`ModelPermitPool`]'s line, given up when its
/// future drops before it is served; the taker behind it then leads.
struct Place<'p> {
    line: &'p Waiting,
    ticket: Option<u32>,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            self.line
                .borrow_mut()
                .retain(|(waiting, _)| *waiting != ticket);
            wake_head(self.line);
        }
    }
}

#[cfg(test)]
mod tests;
