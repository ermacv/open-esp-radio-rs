//! Direct command-publication prototype, without a hardware PortBackend.
//!
//! Whole branded owners stay in `Held`. Only execution records and affine
//! place names enter permanent storage. The model executor never dereferences
//! a DMA address or programs MMIO: its completion means the model's finite
//! operation finished. A real DMA terminal requires the driver's completion
//! proof, not merely acceptance by a hardware queue.
//!
//! Cancellation of a reply wait leaves every owner in Held. After the scope
//! has masked and drained actors, finish may withdraw the remaining owners.
//! Unconfirmed shutdown must move Held into `oer_radio_port::Quarantined`.
//! Forgetting Held cannot free the permanent records; names have no pointer
//! back to it. Composition wakers may schedule, never synchronously detach.
//!
//! Key commands and execution records use the same zeroizing `Ccmp128Key`.
//! Projection borrows material to initialize the record owner and erases the
//! Held owner before publication. Apply borrows the permanent owner; apply
//! and withdrawal both erase it in place before publishing their typed reply.

use super::*;
use oer_ieee80211_lower_mac::{ExecutionPayload, ReceiveFilter};
use oer_memory::{CustodyDomain, IndexedCustody, RetainedName, StableDmaRegion};
use zeroize::Zeroize;

/// Fixture descriptor budget: header and two independently retained bodies.
/// This is a prototype budget, not a one-buffer MPDU ownership restriction.
/// The hardware descriptor-chain layout and CCMP support still require HIL.
pub const MODEL_FRAGMENTS: usize = 3;

/// Address metadata, not a CPU reference or permission to start hardware DMA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaSpan {
    address: usize,
    length: usize,
}
impl DmaSpan {
    pub fn from_region(region: StableDmaRegion<'_>) -> Self {
        let bytes = region.into_mut_slice();
        Self {
            address: bytes.as_mut_ptr() as usize,
            length: bytes.len(),
        }
    }
    pub const fn address(self) -> usize {
        self.address
    }
    pub const fn len(self) -> usize {
        self.length
    }
    pub const fn is_empty(self) -> bool {
        self.length == 0
    }
}

/// Data-only projection of a model MPDU; all leases remain in the InFlight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxImage {
    pub fragments: [DmaSpan; MODEL_FRAGMENTS],
    pub sequence: u16,
    pub packet_number: u64,
}
/// Project only the narrow `ExecutionPayload::Execution` view, never an
/// `&mut Self` owner. The associated function avoids a higher-ranked GAT
/// bound that would accidentally require a borrowed payload to be 'static.
/// No borrow, owner or Context may be returned. The view prepares retained
/// stable buffers for a reader before publishing their metadata.
pub trait ProjectExecution: ExecutionPayload {
    fn project(view: Self::Execution<'_>) -> TxImage;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveImage {
    Off,
    OtherBssManagement,
    Bss {
        bssid: [u8; 6],
        filter: ReceiveFilter,
    },
    AccessPoint(ReceiveFilter),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Configure(ReceiveImage),
    Retune(Channel),
    Enable,
    SampleStationTsf,
    SampleAccessPointTsf,
    Transmit(TxImage),
    InstallPairwise(usize),
    RemovePairwise(usize),
    MutateGroup(usize),
    CloseGate,
    OpenGate,
    Disable,
}
/// Model observation (for example a sampled TSF). Carries no owner or address.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Observation(pub u64);
/// A model operation either ran to completion or was withdrawn without being
/// applied. Withdrawal never manufactures a TSF sample or hardware success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Applied(Observation),
    Withdrawn,
}

/// Finite model step. A real S31 backend is deliberately not implemented here.
/// Apply pending settings/BA after command Acquire and before this command.
/// A key is borrowed only while programming the slot; it must not be retained.
pub trait ModelHardware {
    fn apply(&mut self, operation: Operation, key: &[u8; 16]) -> Observation;
}
struct Record {
    operation: Operation,
    key: Ccmp128Key,
    completion: Completion,
}
impl Record {
    fn plain(operation: Operation) -> Self {
        Self {
            operation,
            key: Ccmp128Key::new(&[0; 16]),
            completion: Completion::Withdrawn,
        }
    }
    fn secret(operation: Operation, key: &mut Ccmp128Key) -> Self {
        let record = Self {
            operation,
            key: Ccmp128Key::new(key.as_bytes()),
            completion: Completion::Withdrawn,
        };
        // The original command retains its permission, not a second secret.
        key.zeroize();
        record
    }
    fn execute(&mut self, hardware: &mut impl ModelHardware) {
        let observation = hardware.apply(self.operation, self.key.as_bytes());
        // Erase the actual permanent cell, before its name enters the reply
        // queue. Moving the key to a temporary and erasing that is insufficient.
        self.key.zeroize();
        self.completion = Completion::Applied(observation);
    }
    fn withdraw(&mut self) {
        self.key.zeroize();
        self.completion = Completion::Withdrawn;
    }
}

// These names cannot be constructed, obtained, or routed by a bridge caller.
// The sole binding consumes the Mint's CommandAuthority and &'static mut
// ExecutionStorage; split lends only that binding's endpoints. Even two
// installations with identical types cannot interchange a reply or a store.
struct Owners;
struct Records;
type OwnerName<const N: usize> = RetainedName<Entry<(), N>, N>;
type RecordName<const N: usize> = RetainedName<OwnerName<N>, N>;
#[allow(
    unsafe_code,
    reason = "sealed endpoints bind Mint names to one Held store"
)]
// SAFETY: Held::bind is the only constructor and consumes one CommandAuthority.
// DirectSender receives only entries made by the existing typed admissions;
// all range and uniqueness proofs are shared with Entry. Sender and Replies
// borrow the same private store. The exact name returns along its private
// reply endpoint before taking the owner. Neither names nor stores escape,
// including when there are multiple instances of this zero-sized domain type.
// Names contain only an index, with no lifetime, pointer or owner destructor.
unsafe impl<const N: usize> CustodyDomain<N> for Owners {
    type Name = Entry<(), N>;
}
#[allow(
    unsafe_code,
    reason = "permanent records are paired with private reply endpoints"
)]
// SAFETY: an exclusive static lane is split once by Held::bind. Only that
// lane's sender inserts a retained owner name, only its executor receives
// the resulting record name, and only its paired Replies takes the record.
// Both names preserve Entry's unique index and response-ordered reuse. No
// public API can mix lanes or installations, recreate names, or recover the
// static storage. Names cannot access the owner store, even after forget.
unsafe impl<const N: usize> CustodyDomain<N> for Records {
    type Name = OwnerName<N>;
}

type OwnersOf<T, const N: usize> = IndexedCustody<T, Owners, N>;
type RecordsOf<const N: usize> = IndexedCustody<Record, Records, N>;
struct Lane<const N: usize> {
    records: RecordsOf<N>,
    requests: AffineIndexedSpscQueue<RecordName<N>, N>,
    replies: AffineIndexedSpscQueue<RecordName<N>, N>,
    request_wake: AtomicWaker,
    reply_wake: AtomicWaker,
}
impl<const N: usize> Lane<N> {
    const fn new() -> Self {
        Self {
            records: IndexedCustody::new(Records),
            requests: AffineIndexedSpscQueue::new(),
            replies: AffineIndexedSpscQueue::new(),
            request_wake: AtomicWaker::new(),
            reply_wake: AtomicWaker::new(),
        }
    }
    fn split(&'static mut self) -> (CallerLane<N>, ExecutorLane<N>) {
        let (send, receive) = self.requests.split();
        let (reply, replied) = self.replies.split();
        (
            CallerLane {
                records: &self.records,
                send,
                replied,
                request_wake: &self.request_wake,
                reply_wake: &self.reply_wake,
            },
            ExecutorLane {
                records: &self.records,
                receive,
                reply,
                reply_wake: &self.reply_wake,
                request_wake: &self.request_wake,
            },
        )
    }
}
/// Composition-owned permanent SRAM, disjoint from the driver's exclusive
/// region. `bind` consumes its static lease exactly once; this prototype has
/// no reinstall or backing-recovery operation. Dropping a future or a boxed
/// Held never frees these records. No allocator is needed by the binding.
pub struct ExecutionStorage<const S: usize, const P: usize> {
    setup: Lane<S>,
    phase: Lane<P>,
}
impl<const S: usize, const P: usize> ExecutionStorage<S, P> {
    pub const fn new() -> Self {
        Self {
            setup: Lane::new(),
            phase: Lane::new(),
        }
    }
}
impl<const S: usize, const P: usize> Default for ExecutionStorage<S, P> {
    fn default() -> Self {
        Self::new()
    }
}
struct CallerLane<const N: usize> {
    records: &'static RecordsOf<N>,
    send: AffineIndexedSpscSender<'static, RecordName<N>, N>,
    replied: AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    request_wake: &'static AtomicWaker,
    reply_wake: &'static AtomicWaker,
}
struct ExecutorLane<const N: usize> {
    records: &'static RecordsOf<N>,
    receive: AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    reply: AffineIndexedSpscSender<'static, RecordName<N>, N>,
    reply_wake: &'static AtomicWaker,
    request_wake: &'static AtomicWaker,
}
impl<const N: usize> ExecutorLane<N> {
    fn withdraw_pending(&mut self) {
        // At most N names; shutdown must first end producer access. Even a
        // misused model caller cannot make this finite operation spin forever.
        for _ in 0..N {
            let Ok(mut name) = self.receive.try_receive() else {
                break;
            };
            self.records.with_mut(&mut name, Record::withdraw);
            self.reply.send(name);
        }
        self.reply_wake.wake();
    }
    fn step(&mut self, hardware: &mut impl ModelHardware) -> Option<Operation> {
        let mut name = self.receive.try_receive().ok()?;
        let operation = self.records.with_mut(&mut name, |record| {
            record.execute(hardware);
            record.operation
        });
        self.reply.send(name);
        self.reply_wake.wake();
        Some(operation)
    }
}
#[derive(Clone, Copy)]
enum Stage {
    Setup,
    Phase,
}
/// The separate executor owns only permanent cells and names, never Held.
/// Each step completes at most one model operation. It has no awaited local
/// owner; dropping a reply wait leaves finish/abandon all accepted commands.
pub struct Executor<const S: usize, const P: usize> {
    setup: ExecutorLane<S>,
    phase: ExecutorLane<P>,
    stage: Stage,
}
impl<const S: usize, const P: usize> Executor<S, P> {
    /// Return queued model commands without applying them. End producer
    /// access first, and join any worker that owns this executor. A completed
    /// reply remains Applied; a queued command becomes Withdrawn. This is
    /// model accounting after drain, not an assertion that hardware stopped.
    pub fn withdraw_pending(mut self) {
        self.setup.withdraw_pending();
        self.phase.withdraw_pending();
    }
    /// Register before checking work, so direct publication cannot lose a
    /// wake between the empty observation and parking the separate executor.
    pub fn poll_step(
        &mut self,
        cx: &mut Context<'_>,
        hardware: &mut impl ModelHardware,
    ) -> Poll<Operation> {
        self.setup.request_wake.register(cx.waker());
        self.phase.request_wake.register(cx.waker());
        match self.step(hardware) {
            Some(operation) => Poll::Ready(operation),
            None => Poll::Pending,
        }
    }
    pub fn step(&mut self, hardware: &mut impl ModelHardware) -> Option<Operation> {
        let operation = match self.stage {
            Stage::Setup => self.setup.step(hardware),
            Stage::Phase => self.phase.step(hardware),
        }?;
        match operation {
            Operation::Enable => self.stage = Stage::Phase,
            Operation::Disable => self.stage = Stage::Setup,
            _ => {}
        }
        Some(operation)
    }
}

/// Caller-owned branded waiting places. No Context: Send or Context: 'static
/// bound is imposed. The permanent executor is returned independently.
pub struct Held<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> {
    setup: OwnersOf<SetupCommand<'id, Caps, SC, AC>, S>,
    phase: OwnersOf<PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    setup_lane: CallerLane<S>,
    phase_lane: CallerLane<P>,
    _authority: CommandAuthority<'id, Root, T, K>,
}
/// Borrowed producer and reply consumer for one direct-publication binding.
pub type Endpoints<
    'a,
    'id,
    Caps,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> = (
    SetupSender<'a, 'id, Caps, SC, AC, C, M, T, K, S, P>,
    Replies<'a, 'id, Caps, SC, AC, C, M, S, P>,
);
impl<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> Held<'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn bind(
        authority: CommandAuthority<'id, Root, T, K>,
        storage: &'static mut ExecutionStorage<S, P>,
    ) -> (Self, Executor<S, P>) {
        const {
            assert!(S == CommandPools::<T, K>::SETUP_STORAGE && S == SetupPlaces::END);
            assert!(P == CommandPools::<T, K>::PHASE_STORAGE && P == PhasePlaces::<T, K>::END);
        }
        let (setup_lane, setup) = storage.setup.split();
        let (phase_lane, phase) = storage.phase.split();
        (
            Self {
                setup: IndexedCustody::new(Owners),
                phase: IndexedCustody::new(Owners),
                setup_lane,
                phase_lane,
                _authority: authority,
            },
            Executor {
                setup,
                phase,
                stage: Stage::Setup,
            },
        )
    }
    pub fn split(
        &mut self,
        issuer: GateIssuer<'id, GatePath>,
        enable: Slot<'id, EnableKind, Child<Root, 8>>,
        disable: Slot<'id, DisableKind, Child<Root, 9>>,
    ) -> Endpoints<'_, 'id, Caps, SC, AC, C, M, T, K, S, P>
    where
        M: ProjectExecution,
    {
        let (setup, setup_rx) = direct(&self.setup, &mut self.setup_lane, project_setup);
        let (phase, phase_rx) = direct(&self.phase, &mut self.phase_lane, project_phase);
        (
            SetupSender {
                channels: SendChannels {
                    setup: Sender::Direct(setup),
                    phase: Sender::Direct(phase),
                    issuer,
                },
                enable: Enable { _place: enable },
                disable: Disable { _place: disable },
            },
            Replies {
                setup: setup_rx,
                phase: phase_rx,
            },
        )
    }
}
fn direct<'a, T, const N: usize>(
    owners: &'a OwnersOf<T, N>,
    lane: &'a mut CallerLane<N>,
    project: fn(&mut T) -> Record,
) -> (DirectSender<'a, T, N>, DirectReceiver<'a, T, N>) {
    (
        DirectSender {
            owners,
            records: lane.records,
            send: &mut lane.send,
            wake: lane.request_wake,
            project,
        },
        DirectReceiver {
            owners,
            records: lane.records,
            receive: &mut lane.replied,
            wake: lane.reply_wake,
        },
    )
}
pub(super) struct DirectSender<'a, T, const N: usize> {
    owners: &'a OwnersOf<T, N>,
    records: &'static RecordsOf<N>,
    send: &'a mut AffineIndexedSpscSender<'static, RecordName<N>, N>,
    wake: &'static AtomicWaker,
    project: fn(&mut T) -> Record,
}
impl<T, const N: usize> DirectSender<'_, T, N> {
    pub(super) fn send(&mut self, entry: Entry<T, N>) {
        let mut name = self.owners.insert(
            Entry {
                index: entry.index,
                command: (),
            },
            entry.command,
        );
        let record = self.owners.with_mut(&mut name, self.project);
        let name = self.records.insert(name, record);
        self.send.send(name);
        self.wake.wake();
    }
}
struct DirectReceiver<'a, T, const N: usize> {
    owners: &'a OwnersOf<T, N>,
    records: &'static RecordsOf<N>,
    receive: &'a mut AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    wake: &'static AtomicWaker,
}
/// The exact original owner, distinguished from an applied command by type.
pub enum Reply<T> {
    Applied(Applied<T>),
    Withdrawn(T),
}
/// Receipt created only by the model step's completed reply path. The fields
/// and constructor are private: recovering an owner consumes this receipt.
pub struct Applied<T> {
    command: T,
    observation: Observation,
}
impl<T> Applied<T> {
    pub fn command(&self) -> &T {
        &self.command
    }
    pub const fn observation(&self) -> Observation {
        self.observation
    }
    pub fn into_parts(self) -> (T, Observation) {
        (self.command, self.observation)
    }
}
impl<T> Reply<T> {
    pub fn completion(&self) -> Completion {
        match self {
            Self::Applied(reply) => Completion::Applied(reply.observation),
            Self::Withdrawn(_) => Completion::Withdrawn,
        }
    }
    pub fn command(&self) -> &T {
        match self {
            Self::Applied(reply) => &reply.command,
            Self::Withdrawn(command) => command,
        }
    }
}
type PhaseApplied<'id, Caps, SC, AC, C, M> = Applied<PhaseCommand<'id, Caps, SC, AC, C, M>>;
pub type DisableReply<'id, Caps, SC, AC, C, M> =
    Result<Disabled<'id>, (Enable<'id>, PhaseApplied<'id, Caps, SC, AC, C, M>)>;
impl<T, const N: usize> DirectReceiver<'_, T, N> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Reply<T>> {
        self.wake.register(cx.waker());
        let Ok(name) = self.receive.try_receive() else {
            return Poll::Pending;
        };
        let (name, record) = self.records.take(name);
        let (_, command) = self.owners.take(name);
        Poll::Ready(match record.completion {
            Completion::Applied(observation) => Reply::Applied(Applied {
                command,
                observation,
            }),
            Completion::Withdrawn => Reply::Withdrawn(command),
        })
    }
}
pub struct Replies<
    'a,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    setup: DirectReceiver<'a, SetupCommand<'id, Caps, SC, AC>, S>,
    phase: DirectReceiver<'a, PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
}
impl<'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    Replies<'_, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn setup(&mut self) -> Reply<SetupCommand<'id, Caps, SC, AC>> {
        poll_fn(|cx| self.setup.poll(cx)).await
    }
    pub async fn phase(&mut self) -> Reply<PhaseCommand<'id, Caps, SC, AC, C, M>> {
        poll_fn(|cx| self.phase.poll(cx)).await
    }
    /// Only an applied CloseGate can issue the barrier. A different command
    /// is returned unchanged; a withdrawn command cannot enter this method.
    pub fn gate_barrier(
        &self,
        reply: PhaseApplied<'id, Caps, SC, AC, C, M>,
    ) -> Result<GateBarrier<'id>, PhaseApplied<'id, Caps, SC, AC, C, M>> {
        match reply.command {
            PhaseCommand::CloseGate(closing) => Ok(GateBarrier { gate: closing.gate }),
            _ => Err(reply),
        }
    }
    /// A withdrawn Disable cannot recover a SetupSender. A mismatched applied
    /// command returns both owners, including the retained Enable place.
    pub fn disabled(
        &self,
        enable: Enable<'id>,
        reply: PhaseApplied<'id, Caps, SC, AC, C, M>,
    ) -> DisableReply<'id, Caps, SC, AC, C, M> {
        match reply.command {
            PhaseCommand::Disable(disable) => Ok(Disabled { enable, disable }),
            _ => Err((enable, reply)),
        }
    }
}
fn configuration<Caps: Ieee80211Capabilities, SC, AC>(
    config: &Configuration<'_, Caps, SC, AC>,
) -> Operation {
    Operation::Configure(match config {
        Configuration::Station { receive, .. } => match receive {
            StationReceive::Off => ReceiveImage::Off,
            StationReceive::OtherBssManagement => ReceiveImage::OtherBssManagement,
            StationReceive::Bss { bssid, filter } => ReceiveImage::Bss {
                bssid: bssid.get(),
                filter: filter.get(),
            },
        },
        Configuration::AccessPoint { receive, .. } => ReceiveImage::AccessPoint(receive.get()),
    })
}
fn project_setup<Caps: Ieee80211Capabilities, SC, AC>(
    command: &mut SetupCommand<'_, Caps, SC, AC>,
) -> Record {
    Record::plain(match command {
        SetupCommand::Configure(config) => configuration(config),
        SetupCommand::Retune { channel, .. } => Operation::Retune(*channel),
        SetupCommand::Enable(_) => Operation::Enable,
    })
}
fn project_phase<Caps: Ieee80211Capabilities, SC, AC, C, M: ProjectExecution>(
    command: &mut PhaseCommand<'_, Caps, SC, AC, C, M>,
) -> Record {
    Record::plain(match command {
        PhaseCommand::Configure(config) => configuration(config),
        PhaseCommand::SampleTsf(Sampling::Station(_)) => Operation::SampleStationTsf,
        PhaseCommand::SampleTsf(Sampling::AccessPoint(_)) => Operation::SampleAccessPointTsf,
        PhaseCommand::Submit(submission) => {
            Operation::Transmit(M::project(submission.operation.execute()))
        }
        PhaseCommand::InstallPairwise { permission, key } => {
            return Record::secret(
                Operation::InstallPairwise(usize::from(permission.index())),
                key,
            );
        }
        PhaseCommand::RemovePairwise(permission) => {
            Operation::RemovePairwise(usize::from(permission.index()))
        }
        PhaseCommand::MutateGroup { permission, key } => {
            return Record::secret(Operation::MutateGroup(usize::from(permission.index())), key);
        }
        PhaseCommand::CloseGate(_) => Operation::CloseGate,
        PhaseCommand::OpenGate => Operation::OpenGate,
        PhaseCommand::Disable(_) => Operation::Disable,
    })
}

#[cfg(test)]
mod tests;
