//! The two stage-specific command stores of `Held<'id>`.
//!
//! Actual lower-MAC permissions, accepted operations and S31 `KeyUse` rights
//! move by value. Configure is a FIFO command with its exact receive value;
//! it is never a coalescing setting. These borrowed endpoints do not register
//! the held context with DriverScope or extend its lifetime.
use atomic_waker::AtomicWaker;
use core::{
    cell::{Cell, UnsafeCell},
    future::poll_fn,
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};
use oer_esp32s31_ieee80211::lower_mac::{CorePools, CoreResources, TxAdmission};
use oer_ieee80211_lower_mac::{
    AccessPoint, Ccmp128Key, Channel, ConfigurePermission, Configured, GateClosed, GateIssuer,
    GateOpen, GatePool, Ieee80211Capabilities, InFlight, InterfaceCommandPool,
    InterfacePermissions, LowerMacTerminal, PhaseTypes, SampledTsfPermission, Station,
    StationReceive, SupportedFilter, TerminalAccess, TransmitPool, TsfPermission, TsfSample,
};
use oer_radio_port::{Child, PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots};

pub type CorePath = Child<Root, 0>;
pub type StationPath = Child<Root, 1>;
pub type AccessPointPath = Child<Root, 2>;
pub type GatePath = Child<Root, 6>;
pub type TicketPath = Child<Root, 11>;
pub enum InstallKeyKind {}
pub enum RemoveKeyKind {}
pub enum GroupMutationKind {}
pub enum RetuneKind {}
pub enum EnableKind {}
pub enum DisableKind {}
pub enum BridgeKind {}
pub enum TicketKind {}
const GROUP_PERMISSIONS: usize = 2;
const GATE_COMMANDS: usize = 2;
const LIFECYCLE_COMMAND: usize = 1;
const RETUNE_COMMAND: usize = 1;

/// The same parameters issue the tickets/slots and determine both FIFO sizes.
pub struct CommandPools<const TICKETS: usize, const PAIRWISE: usize>;
impl<const T: usize, const K: usize> CommandPools<T, K> {
    pub const PHASE_COMMAND_CAPACITY: usize = T
        + 2 * K
        + GROUP_PERMISSIONS
        + InterfaceCommandPool::<Station>::COMMAND_CAPACITY
        + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY
        + GATE_COMMANDS;
    /// The phase-contract formula plus the separate consuming Disable place.
    pub const PHASE_STORAGE: usize = Self::PHASE_COMMAND_CAPACITY + LIFECYCLE_COMMAND;
    /// One configure per interface, one retune and the final Enable.
    pub const SETUP_STORAGE: usize = 2 + RETUNE_COMMAND + LIFECYCLE_COMMAND;
}
type Layout<const T: usize, const K: usize> = (
    CorePools<T>,
    InterfaceCommandPool<Station>,
    InterfaceCommandPool<AccessPoint>,
    Slots<InstallKeyKind, K>,
    Slots<RemoveKeyKind, K>,
    Slots<GroupMutationKind, GROUP_PERMISSIONS>,
    GatePool,
    Slots<RetuneKind, 1>,
    Slots<EnableKind, 1>,
    Slots<DisableKind, 1>,
    Slots<BridgeKind, 1>,
    TransmitPool<TicketKind, T>,
);
impl<const T: usize, const K: usize> PoolSet for CommandPools<T, K> {
    type Pools<'id, P: PoolPath> = Resources<'id, P, T, K>;
    fn build<'id, P: PoolPath>(builder: PoolBuilder<'id, P>) -> Self::Pools<'id, P> {
        let (
            core,
            station,
            access_point,
            installs,
            removes,
            groups,
            (gate, issuer),
            [retune],
            [enable],
            [disable],
            [bridge],
            (tickets, terminals),
        ) = builder.into_pools::<Layout<T, K>>();
        Resources {
            core,
            station,
            access_point,
            installs,
            removes,
            groups,
            gate,
            issuer,
            retune,
            enable,
            disable,
            bridge: CommandAuthority { _slot: bridge },
            tickets,
            terminals,
        }
    }
}
/// The queue's pool sizes remain in this affine construction authority's
/// type. A small store cannot accept the authority minted for larger pools.
#[must_use]
pub struct CommandAuthority<'id, P: PoolPath, const T: usize, const K: usize> {
    _slot: Slot<'id, BridgeKind, Child<P, 10>>,
}

/// One complete mint result; no second bridge can be built from its authority.
pub struct Resources<'id, P: PoolPath, const T: usize, const K: usize> {
    pub core: CoreResources<'id, Child<P, 0>, T>,
    pub station: InterfacePermissions<'id, Station, Child<P, 1>>,
    pub access_point: InterfacePermissions<'id, AccessPoint, Child<P, 2>>,
    pub installs: [Slot<'id, InstallKeyKind, Child<P, 3>>; K],
    pub removes: [Slot<'id, RemoveKeyKind, Child<P, 4>>; K],
    pub groups: [Slot<'id, GroupMutationKind, Child<P, 5>>; GROUP_PERMISSIONS],
    pub gate: GateOpen<'id, Child<P, 6>>,
    pub issuer: GateIssuer<'id, Child<P, 6>>,
    pub retune: Slot<'id, RetuneKind, Child<P, 7>>,
    pub enable: Slot<'id, EnableKind, Child<P, 8>>,
    pub disable: Slot<'id, DisableKind, Child<P, 9>>,
    pub bridge: CommandAuthority<'id, P, T, K>,
    pub tickets: [Slot<'id, TicketKind, Child<P, 11>>; T],
    pub terminals: TerminalAccess<'id, TicketKind, Child<P, 11>>,
}

#[must_use]
pub enum Configuration<'id, Caps: Ieee80211Capabilities, SC, AC> {
    Station {
        permission: ConfigurePermission<'id, Station, StationPath, SC>,
        receive: StationReceive<Caps>,
    },
    AccessPoint {
        permission: ConfigurePermission<'id, AccessPoint, AccessPointPath, AC>,
        receive: SupportedFilter<Caps, AccessPoint>,
    },
}
impl<'id, Caps: Ieee80211Capabilities, SC: 'id, AC: 'id> Configuration<'id, Caps, SC, AC> {
    /// Called after the receive-configuration hardware barrier completes.
    /// Both the affine admission and opaque context are returned unchanged.
    pub fn configured<P>(self) -> LowerMacTerminal<'id, P>
    where
        P: PhaseTypes<
                'id,
                StationCommandPath = StationPath,
                AccessPointCommandPath = AccessPointPath,
                StationConfigureContext = SC,
                AccessPointConfigureContext = AC,
            >,
    {
        LowerMacTerminal::Configured(match self {
            Self::Station { permission, .. } => Configured::Station(permission),
            Self::AccessPoint { permission, .. } => Configured::AccessPoint(permission),
        })
    }
}
#[must_use]
pub enum Sampling<'id> {
    Station(TsfPermission<'id, Station, StationPath>),
    AccessPoint(TsfPermission<'id, AccessPoint, AccessPointPath>),
}
impl<'id> Sampling<'id> {
    pub fn sampled<P>(self, sample: TsfSample) -> LowerMacTerminal<'id, P>
    where
        P: PhaseTypes<
                'id,
                StationCommandPath = StationPath,
                AccessPointCommandPath = AccessPointPath,
            >,
    {
        let permission = match self {
            Self::Station(permission) => SampledTsfPermission::Station(permission),
            Self::AccessPoint(permission) => SampledTsfPermission::AccessPoint(permission),
        };
        LowerMacTerminal::TsfSampled { permission, sample }
    }
}

/// The accepted operation is never unpacked or reconstructed by the FIFO.
/// Protected admission is the existing S31 KeyUse owner inside TxAdmission.
/// M owns the whole MPDU, including a header and any number of retained bodies.
/// This envelope requires neither one contiguous slice nor one DMA descriptor;
/// retirement, retry and quarantine retain or return the complete M.
#[must_use]
pub struct Submission<'id, C, M> {
    pub operation: InFlight<C, Slot<'id, TicketKind, TicketPath>, M>,
    pub admission: TxAdmission<'id, CorePath>,
}
#[must_use]
pub struct Enable<'id> {
    _place: Slot<'id, EnableKind, Child<Root, 8>>,
}
#[must_use]
pub struct Disable<'id> {
    _place: Slot<'id, DisableKind, Child<Root, 9>>,
}
#[must_use]
pub struct Disabled<'id> {
    enable: Enable<'id>,
    disable: Disable<'id>,
}

#[must_use]
pub enum SetupCommand<'id, Caps: Ieee80211Capabilities, SC, AC> {
    Configure(Configuration<'id, Caps, SC, AC>),
    Retune {
        permission: Slot<'id, RetuneKind, Child<Root, 7>>,
        channel: Channel,
    },
    Enable(Enable<'id>),
}
#[must_use]
pub enum PhaseCommand<'id, Caps: Ieee80211Capabilities, SC, AC, C, M> {
    Configure(Configuration<'id, Caps, SC, AC>),
    SampleTsf(Sampling<'id>),
    Submit(Submission<'id, C, M>),
    InstallPairwise {
        permission: Slot<'id, InstallKeyKind, Child<Root, 3>>,
        key: Ccmp128Key,
    },
    RemovePairwise(Slot<'id, RemoveKeyKind, Child<Root, 4>>),
    MutateGroup {
        permission: Slot<'id, GroupMutationKind, Child<Root, 5>>,
        key: Ccmp128Key,
    },
    CloseGate(ClosingGate<'id>),
    /// Private publication authority is the consuming open_gate call. The
    /// command is already in FIFO before that call returns GateOpen.
    OpenGate,
    Disable(Disable<'id>),
}

/// Storage owned by Held, with two exclusive SPSC endpoint pairs. There is
/// no general-purpose send operation: every producer method consumes one of
/// the bounded admissions from this installation's sole mint.
pub struct CommandStorage<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const SETUP: usize,
    const PHASE: usize,
> {
    setup: Fifo<SetupCommand<'id, Caps, SC, AC>, SETUP>,
    phase: Fifo<PhaseCommand<'id, Caps, SC, AC, C, M>, PHASE>,
    _authority: CommandAuthority<'id, Root, T, K>,
}
/// The unique pair of endpoints lent by one command store.
pub type SetupEndpoints<'q, 'id, Caps, SC, AC, C, M, const S: usize, const P: usize> = (
    SetupSender<'q, 'id, Caps, SC, AC, C, M, S, P>,
    SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>,
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
> CommandStorage<'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn new(authority: CommandAuthority<'id, Root, T, K>) -> Self {
        const {
            assert!(
                S == CommandPools::<T, K>::SETUP_STORAGE,
                "setup storage must match its minted permissions"
            );
            assert!(
                P == CommandPools::<T, K>::PHASE_STORAGE,
                "phase storage must include every permission and Disable"
            );
        }
        Self {
            setup: Fifo::new(),
            phase: Fifo::new(),
            _authority: authority,
        }
    }
    pub fn split(
        &mut self,
        issuer: GateIssuer<'id, GatePath>,
        enable: Slot<'id, EnableKind, Child<Root, 8>>,
        disable: Slot<'id, DisableKind, Child<Root, 9>>,
    ) -> SetupEndpoints<'_, 'id, Caps, SC, AC, C, M, S, P> {
        (
            SetupSender {
                channels: SendChannels {
                    setup: &self.setup,
                    phase: &self.phase,
                    issuer,
                    _single: PhantomData,
                },
                enable: Enable { _place: enable },
                disable: Disable { _place: disable },
            },
            SetupReceiver {
                channels: ReceiveChannels {
                    setup: &self.setup,
                    phase: &self.phase,
                    _single: PhantomData,
                },
            },
        )
    }
}
struct SendChannels<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    setup: &'q Fifo<SetupCommand<'id, Caps, SC, AC>, S>,
    phase: &'q Fifo<PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    issuer: GateIssuer<'id, GatePath>,
    _single: PhantomData<Cell<()>>,
}
struct ReceiveChannels<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    setup: &'q Fifo<SetupCommand<'id, Caps, SC, AC>, S>,
    phase: &'q Fifo<PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    _single: PhantomData<Cell<()>>,
}
#[must_use]
pub struct SetupSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
    enable: Enable<'id>,
    disable: Disable<'id>,
}
#[must_use]
pub struct PhaseSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
    disable: Disable<'id>,
}
/// The sole producer waiting for its owned Disable reply; no command method.
#[must_use]
pub struct DisablingSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
}
#[must_use]
pub struct SetupReceiver<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: ReceiveChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
}
#[must_use]
pub struct PhaseReceiver<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: ReceiveChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
    enable: Enable<'id>,
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    SetupSender<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub fn configure(&mut self, command: Configuration<'id, Caps, SC, AC>) {
        self.channels.setup.send(SetupCommand::Configure(command));
    }
    pub fn retune(&mut self, permission: Slot<'id, RetuneKind, Child<Root, 7>>, channel: Channel) {
        self.channels.setup.send(SetupCommand::Retune {
            permission,
            channel,
        });
    }
    /// Publishes the final setup command before transferring the producer.
    pub fn enable(self) -> PhaseSender<'q, 'id, Caps, SC, AC, C, M, S, P> {
        self.channels.setup.send(SetupCommand::Enable(self.enable));
        PhaseSender {
            channels: self.channels,
            disable: self.disable,
        }
    }
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    PhaseSender<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub fn configure(&mut self, command: Configuration<'id, Caps, SC, AC>) {
        self.channels.phase.send(PhaseCommand::Configure(command));
    }
    pub fn sample_tsf(&mut self, permission: Sampling<'id>) {
        self.channels
            .phase
            .send(PhaseCommand::SampleTsf(permission));
    }
    pub fn submit(
        &mut self,
        operation: InFlight<C, Slot<'id, TicketKind, TicketPath>, M>,
        admission: TxAdmission<'id, CorePath>,
    ) {
        self.channels.phase.send(PhaseCommand::Submit(Submission {
            operation,
            admission,
        }));
    }
    pub fn install_pairwise(
        &mut self,
        permission: Slot<'id, InstallKeyKind, Child<Root, 3>>,
        key: Ccmp128Key,
    ) {
        self.channels
            .phase
            .send(PhaseCommand::InstallPairwise { permission, key });
    }
    pub fn remove_pairwise(&mut self, permission: Slot<'id, RemoveKeyKind, Child<Root, 4>>) {
        self.channels
            .phase
            .send(PhaseCommand::RemovePairwise(permission));
    }
    pub fn mutate_group(
        &mut self,
        permission: Slot<'id, GroupMutationKind, Child<Root, 5>>,
        key: Ccmp128Key,
    ) {
        self.channels
            .phase
            .send(PhaseCommand::MutateGroup { permission, key });
    }
    pub fn close_gate(&mut self, gate: GateOpen<'id, GatePath>) {
        self.channels
            .phase
            .send(PhaseCommand::CloseGate(ClosingGate { gate }));
    }
    /// Complete the driver's barrier reply at the original gate owner.
    pub fn closed(&mut self, barrier: GateBarrier<'id>) -> GateClosed<'id, GatePath> {
        self.channels.issuer.close(barrier.gate)
    }
    /// No public command constructor or later publication step exists.
    pub fn open_gate(&mut self, gate: GateClosed<'id, GatePath>) -> GateOpen<'id, GatePath> {
        self.channels.phase.send(PhaseCommand::OpenGate);
        self.channels.issuer.open(gate)
    }
    pub fn disable(self) -> DisablingSender<'q, 'id, Caps, SC, AC, C, M, S, P> {
        self.channels
            .phase
            .send(PhaseCommand::Disable(self.disable));
        DisablingSender {
            channels: self.channels,
        }
    }
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    DisablingSender<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub fn finished(self, reply: Disabled<'id>) -> SetupSender<'q, 'id, Caps, SC, AC, C, M, S, P> {
        SetupSender {
            channels: self.channels,
            enable: reply.enable,
            disable: reply.disable,
        }
    }
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn next(&mut self) -> SetupCommand<'id, Caps, SC, AC> {
        poll_fn(|cx| self.channels.setup.poll(cx)).await
    }
    /// S31 enable is synchronous; no intermediate hardware Enabling state.
    pub fn enabled(self, enable: Enable<'id>) -> PhaseReceiver<'q, 'id, Caps, SC, AC, C, M, S, P> {
        PhaseReceiver {
            channels: self.channels,
            enable,
        }
    }
}
#[must_use]
pub struct ClosingGate<'id> {
    gate: GateOpen<'id, GatePath>,
}
#[must_use]
pub struct GateBarrier<'id> {
    gate: GateOpen<'id, GatePath>,
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    PhaseReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn next(&mut self) -> PhaseCommand<'id, Caps, SC, AC, C, M> {
        poll_fn(|cx| self.channels.phase.poll(cx)).await
    }
    /// Called by the driver only after its close barrier finishes.
    pub fn gate_barrier(&mut self, closing: ClosingGate<'id>) -> GateBarrier<'id> {
        GateBarrier { gate: closing.gate }
    }
    /// Called after withdrawing terminal owners and recycling RX locally.
    pub fn disabled(
        self,
        disable: Disable<'id>,
    ) -> (
        SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>,
        Disabled<'id>,
    ) {
        (
            SetupReceiver {
                channels: self.channels,
            },
            Disabled {
                enable: self.enable,
                disable,
            },
        )
    }
}

// This is the command-storage part of the single scoped bridge primitive.
// Its only send callers above consume the mint's credits. No endpoint or
// generic send escapes this module. A removed command frees its cell before
// its credit can return. Enable/Disable retain their places through the
// opposite stage, so repeated transitions cannot create extra FIFO capacity.
struct Fifo<T, const N: usize> {
    cells: [UnsafeCell<MaybeUninit<T>>; N],
    write: AtomicUsize,
    read: AtomicUsize,
    wake: AtomicWaker,
}
#[allow(
    unsafe_code,
    reason = "affine scoped endpoints transfer each initialized command once"
)]
// SAFETY: a unique producer writes only its next vacant cell, then Release
// publishes it. One consumer Acquire reads it before Release freeing that
// cell. The private typed sends bound occupancy by N. T: Send permits the
// transfer across cores; no endpoint is Sync or publicly reconstructible.
unsafe impl<T: Send, const N: usize> Sync for Fifo<T, N> {}
impl<T, const N: usize> Fifo<T, N> {
    const fn new() -> Self {
        const {
            assert!(N > 0 && N <= usize::MAX / 2);
        }
        Self {
            cells: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            wake: AtomicWaker::new(),
        }
    }
    const fn advance(cursor: usize) -> usize {
        if cursor + 1 == N * 2 { 0 } else { cursor + 1 }
    }
    fn send(&self, value: T) {
        let write = self.write.load(Ordering::Relaxed);
        // Acquire pairs with the consumer's release before a credit returns.
        let _released = self.read.load(Ordering::Acquire);
        #[allow(
            unsafe_code,
            reason = "the command's affine credit proves this producer cell vacant"
        )]
        // SAFETY: the cursor stays in 0..2N. The closed producer API and the
        // const mint formula prove occupancy < N for this consumed admission.
        unsafe {
            (*self.cells.get_unchecked(write % N).get()).write(value);
        }
        self.write.store(Self::advance(write), Ordering::Release);
        self.wake.wake();
    }
    fn poll(&self, cx: &mut Context<'_>) -> Poll<T> {
        self.wake.register(cx.waker());
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return Poll::Pending;
        }
        #[allow(
            unsafe_code,
            reason = "the sole consumer owns this Release-published command"
        )]
        // SAFETY: Acquire observed an initialized cell at this bounded cursor;
        // the producer cannot reuse it before the following Release.
        let value = unsafe { (*self.cells.get_unchecked(read % N).get()).assume_init_read() };
        self.read.store(Self::advance(read), Ordering::Release);
        Poll::Ready(value)
    }
}
impl<T, const N: usize> Drop for Fifo<T, N> {
    fn drop(&mut self) {
        let mut read = *self.read.get_mut();
        let write = *self.write.get_mut();
        // Exclusive storage ownership excludes both endpoints; at most N
        // commands remain. Dropping a queue cannot run an unbounded drain.
        for _ in 0..N {
            if read == write {
                break;
            }
            #[allow(unsafe_code, reason = "only the owned published range is initialized")]
            // SAFETY: no endpoint remains, and each initialized cell is
            // destroyed once before moving the cursor within 0..2N.
            unsafe {
                self.cells
                    .get_unchecked_mut(read % N)
                    .get_mut()
                    .assume_init_drop();
            }
            read = Self::advance(read);
        }
    }
}

#[cfg(test)]
mod tests;
