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
//! transmit permits in [`ModelPermitPool`]s, first-in, first-out pools.
//!
//! Its tokens have no value outside it: no caller mints a key, and a key of
//! a station's client is not an access point's.
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKey, Station};
//! fn forge<'id>() -> ModelKey<'id, Station> {
//!     ModelKey { index: 0, slot: todo!() }
//! }
//! ```
//!
//! ```compile_fail,E0308
//! # use oer_ieee80211_lower_mac::LowerMacClient;
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{AccessPoint, ModelClient, ModelKey, Station};
//! fn cross<'id>(client: &ModelClient<'_, 'id, AccessPoint>, key: ModelKey<'id, Station>) {
//!     let _ = client.remove_key(key);
//! }
//! ```
//!
//! Its compiling twin gives a key back to its own role's client:
//!
//! ```
//! # use oer_ieee80211_lower_mac::LowerMacClient;
//! # use oer_ieee80211_runtime::lower_mac_owner::model::{AccessPoint, ModelClient, ModelKey};
//! fn own<'id>(client: &ModelClient<'_, 'id, AccessPoint>, key: ModelKey<'id, AccessPoint>) {
//!     let _ = client.remove_key(key);
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
    CancelError, ClockError, EnableResult, EventsLost, Lent, Mint, Permit, PermitPool, PhaseParts,
    Poisoned, PortBackend, PortResult, RadioPortClient, RadioPortEvents, RadioWork, Retire, Slot,
    Work,
};
use oer_time::RadioInstant;

use oer_ieee80211_lower_mac::{
    Ccmp128Key, Channel, Ieee80211Capabilities, Ieee80211Radio, KeyScope, LowerMacBackend,
    LowerMacCapabilities, LowerMacClient, LowerMacControl, LowerMacEvent, LowerMacEvents,
    LowerMacSetup, MacAddress, MpduLength, PhaseAttempt, RxMeta, SupportedBlockAck,
    SupportedChannel, SupportedFilter, TxCompletion, TxGated, TxId, TxStatus, VifRole,
    model::{MODEL_CAPABILITIES, ModelBody, ModelFault, ModelRxBuffer},
};

use super::{BufferPermit, QueuePermit, QueuePool, TxBufferKind, TxPermits, TxQueueKind};
use oer_ieee80211_mac::qos::WmmAccessCategory;

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// Transmit buffers the model holds.
pub const OWNED_MODEL_TX_BUFFERS: usize = 3;
/// Key slots of each role.
pub const OWNED_MODEL_KEY_SLOTS: usize = 2;
/// Receive Block Ack slots of each role.
pub const OWNED_MODEL_BLOCK_ACK_SLOTS: usize = 1;
/// Transmit queues of the model ([`MODEL_CAPABILITIES`]).
pub const OWNED_MODEL_TX_QUEUES: usize = MODEL_CAPABILITIES.tx_queues as usize;

/// An interface role of the model's clients.
pub trait ModelRole {
    /// The role.
    const ROLE: VifRole;
}

/// The station role.
pub enum Station {}
/// The access-point role.
pub enum AccessPoint {}

impl ModelRole for Station {
    const ROLE: VifRole = VifRole::Station;
}

impl ModelRole for AccessPoint {
    const ROLE: VifRole = VifRole::AccessPoint;
}

/// The kind of a role's key slots.
pub struct ModelKeys<R>(PhantomData<fn() -> R>);
/// The kind of a role's receive Block Ack slots.
pub struct ModelBlockAcks<R>(PhantomData<fn() -> R>);

/// One admitted attempt.
struct Admission {
    id: TxId,
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

/// The model's held part of one install: its transmit permits.
pub struct ModelHeld<'id> {
    pub tx: ModelTxPermits<'id>,
}

/// The port-lifetime resources the body of one install of the model
/// receives: each role's slots.
pub struct ModelResources<'id> {
    pub station_keys: [Slot<'id, ModelKeys<Station>>; OWNED_MODEL_KEY_SLOTS],
    pub access_point_keys: [Slot<'id, ModelKeys<AccessPoint>>; OWNED_MODEL_KEY_SLOTS],
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

    fn configure(&mut self, role: VifRole, address: MacAddress) {
        self.0.borrow_mut().addresses[role_index(role)] = address;
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

/// One MPDU's memory, bound to its buffer permit.
pub struct ModelTxBuffer<'c, 'id> {
    pub bytes: Vec<u8>,
    permit: BufferPermit<'c, 'id>,
}

/// An installed key of a client of role `R`; it holds its slot.
pub struct ModelKey<'id, R> {
    index: u8,
    slot: Slot<'id, ModelKeys<R>>,
}

/// A receive Block Ack agreement of a client of role `R`; it holds its
/// slot.
pub struct ModelBlockAck<'id, R> {
    slot: Slot<'id, ModelBlockAcks<R>>,
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
    type KeyKind = ModelKeys<R>;
    type Key = ModelKey<'id, R>;
    type BlockAckKind = ModelBlockAcks<R>;
    type BlockAck = ModelBlockAck<'id, R>;

    fn channel(&self) -> Channel {
        self.state.borrow().tuned
    }

    fn queue<'c>(
        &'c self,
        access_category: WmmAccessCategory,
    ) -> impl Future<Output = QueuePermit<'c, 'id>> + 'c
    where
        'id: 'c,
    {
        self.held.tx.queue(access_category)
    }

    async fn tx_buffer<'c>(&'c self, len: MpduLength<ModelCaps>) -> ModelTxBuffer<'c, 'id>
    where
        'id: 'c,
    {
        ModelTxBuffer {
            bytes: alloc::vec![0; len.get()],
            permit: self.held.tx.buffer().await,
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
            queue, frame, body, ..
        } = attempt;
        let id = TxId(state.next_id);
        state.next_id += 1;
        let published = state.gate_open;
        state.admitted.push(Admission {
            id,
            published,
            body,
        });
        drop(state);
        self.held.tx.admit(id, queue, frame.permit);
        Ok(Work::new(self, ModelToken(id)))
    }

    fn configure(&self, _: Option<MacAddress>, _: SupportedFilter<ModelCaps>) {}

    fn install_key(
        &self,
        slot: Slot<'id, ModelKeys<R>>,
        scope: KeyScope,
        _: Ccmp128Key,
    ) -> ModelKey<'id, R> {
        let index = slot.index();
        self.state
            .borrow_mut()
            .installed_keys
            .push((R::ROLE, index, scope));
        ModelKey { index, slot }
    }

    fn remove_key(&self, key: ModelKey<'id, R>) -> Slot<'id, ModelKeys<R>> {
        self.state
            .borrow_mut()
            .installed_keys
            .retain(|(role, slot, _)| (*role, *slot) != (R::ROLE, key.index));
        key.slot
    }

    fn add_block_ack(
        &self,
        slot: Slot<'id, ModelBlockAcks<R>>,
        _: SupportedBlockAck<ModelCaps>,
    ) -> ModelBlockAck<'id, R> {
        ModelBlockAck { slot }
    }

    fn remove_block_ack(&self, agreement: ModelBlockAck<'id, R>) -> Slot<'id, ModelBlockAcks<R>> {
        agreement.slot
    }

    fn set_bss_color(&self, _: oer_ieee80211_lower_mac::BssColor) {}
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
        let held = ModelHeld {
            tx: TxPermits::new(&mut mint, ModelPermitPool::new(buffers)),
        };
        let resources = ModelResources {
            station_keys: mint.slots(),
            access_point_keys: mint.slots(),
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
    fn of(permit: Permit<'id, TxQueueKind>) -> Self {
        Self::new([permit])
    }
}

/// A first-in, first-out pool of permits.
pub struct ModelPermitPool<'id, Kind> {
    free: RefCell<Vec<Permit<'id, Kind>>>,
    /// The waiting takers, in arrival order: each one's ticket and waker.
    line: Line,
    next: Cell<u32>,
}

type Line = RefCell<VecDeque<(u32, Waker)>>;

/// Wake the taker at the head of `line`, which a free permit now serves.
fn wake_head(line: &Line) {
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
    fn take(&self) -> impl Future<Output = Permit<'id, Kind>> + '_ {
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
    line: &'p Line,
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
