//! The station's thin service on a lower-MAC port: the one loop that waits
//! and carries out what the sans-IO machines decide (owner request on
//! #563).
//!
//! It polls every input source at each waiting point: the port's next event, the machines' next
//! deadline and the application (a command, or an MSDU while the data
//! machine takes one). After each input it takes every action of the
//! arbiter ([`Arbiter`]), the station ([`Station`]) and its data machine
//! ([`StaData`]) until none owes one; an answer of the arbiter goes back to
//! the station in the same step. A retune leaves the phase: the port is
//! disabled, tuned and enabled, and the arbiter hears how it went.
//! Each step consumes at most one input per source and polls all sources even
//! when RX is ready. Batches stop after eight steps and yield, so a continuous
//! RX stream cannot suppress application commands, data TX or driver progress.
//! `event` returns a typed contract error directly; no sticky failure flag
//! changes later event handling. A disable carries its delivery result with
//! the disabled owner, preserving the existing fail-stop boundary.
//!
//! The tokens the machines hold are the port's: tickets and buffers of two
//! classes, the small one for the station's frames and the large one for
//! data. A completion carries `TxContext`, which routes its ticket and
//! resources to the original machine and link. Buffer classes are used
//! only to recover their memory types; `StationTxBuffer` exhaustively
//! represents both classes and both publication states at this boundary.
//! Machines receive `ReturnedMpdu::Fresh` or `Sent` and decide when to renew.
//! Cross-AC completion order has no bearing on ownership. Each machine
//! returns stale-link resources without advancing its current protocol,
//! and the service excludes those terminals from current-link rate control.
//! A data operation's closed context supplies its frame, access category and
//! checked `DataProtection`. Submission refuses a changed link or protection
//! before preparation and returns the original context, ticket and buffer.
//! Data admission is an owned permit per AC, including old-link work until
//! its terminal, and preserves the station's Voice waiting bound.
//! Preparation returns the original typed source on refusal; context and
//! ticket have not left this service. Submission can refuse only poisoning.
//! The application hears `ServiceOutput::TxRefused`; Station gets `Unsent`
//! and Data gets `Refused`. Neither path fabricates a completion or updates
//! rate statistics, and the service continues. A `Sent` return retains its
//! external body until its machine retires the MPDU; service never renews
//! a completion merely to route it.
//! Key-install actions carry Station's pending tokens straight back with
//! installed keys. Configure retains its one pending token until the port
//! reports it applied; Station cannot issue another before that return.
//! Arbiter answers return their affine places/leases in the same step.
//! MSDUs come and go through
//! affine SPSC queues with atomic wakeups; a received
//! MSDU the network's full queue cannot take is dropped, as an Ethernet
//! receiver drops one.
//! `MsduQueue::split` exclusively borrows its storage. Both endpoints retain
//! that borrow, so no second pair can coexist. After the pair returns, queued
//! values remain owned by the queue and reach the next receiver in order.
//! [`EthernetFrame`] retains its fixed header and bounded payload. Construction
//! from checked Ethernet parts returns [`FrameTooLong`] for a payload exceeding
//! [`MAX_ETHERNET_PAYLOAD_LEN`]; received payloads are never clipped. Raw-byte
//! fixture parsing and serialization do not belong to the product API.

use core::marker::PhantomData;
use oer_ieee80211_sta::machine::TxRefusal;

use core::future::{Future, poll_fn};
use core::task::Poll;
use embassy_futures::yield_now;
use embassy_sync::channel::DynamicReceiver;
use oer_ieee80211_arbiter::{
    AccessPointCommand, AccessPointFact, Arbiter, ArbiterAction, ArbiterNotice,
};
use oer_ieee80211_lower_mac::{
    Ccmp128Key, ClassFrame, CoexPriority, Ieee80211Capabilities, InterfacePhase, KeyTypes,
    LowerMacBackend, LowerMacEvent, LowerMacPhase, PhaseTypes, PoolMpdu, PoolSource, Protection,
    Removed, RemovedKey, RetransmissionPlan, ReturnedMpdu, RxBuffer, RxMetadata, StationPhase,
    SupportedChannel, SupportedCoex, SupportedPower, TxBufferClass, TxBufferPool, TxCompleted,
    TxOutcome, TxPower, TxProtection, TxSource, VifRole,
};
use oer_ieee80211_mac::qos::{WmmAccessCategory, WmmUserPriority};
use oer_ieee80211_port::{Disabled, Enabled, Failed, Owner, PoisonedOwner, PortBackend};
use oer_ieee80211_sta::{
    data::{
        self, DataAction, DataInput, DataProtection, DataRefusal, DataReturn, DataTxContext, Msdu,
        StaData, StaDataTokens,
    },
    machine::{
        Peer, StaAction, StaCommand, StaFrame, StaFrameClass, StaInput, StaNotice, StaRatePlan,
        StaTokens, StaTransmit, StaTxContext, Station,
    },
    power_save::NULL_TRANSMISSIONS,
};
use oer_ieee80211_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_radio_port::Slot;
use oer_time::{Instant, Timer};

/// A backend the station's service drives: its phase, settings and events
/// as the lower-MAC contract's, and the two buffer classes the service
/// splits between the station and data.
pub trait StationPort<'id>: LowerMacBackend {
    /// The types its phase's events carry.
    type Types: PhaseTypes<'id, Context = TxContext<'id, Self::Caps, Self::Large>>
        + TxBufferPool<'id, Self::Small>
        + TxBufferPool<'id, Self::Large>;
    /// The buffer class of the station's frames.
    type Small: TxBufferClass;
    /// The buffer class of data.
    type Large: TxBufferClass;

    /// The lent phase, as the contract's.
    fn lend<'a, 'r>(
        phase: &'r mut Self::Phase<'a, 'id>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = Self::Caps, Types = Self::Types>
    where
        'id: 'a;

    /// Tune the disabled port to `channel`.
    fn tune(setup: &mut Self::Setup<'_, 'id>, channel: SupportedChannel<Self::Caps>);

    /// The received buffers the port lends at most at once: a disable
    /// returns no more, and the service holds that many for the next phase.
    const RX_BUFFERS: usize;

    /// One event of the port, as the contract's.
    fn event(event: Self::Event<'id>) -> LowerMacEvent<'id, Self::Types>;

    /// Recover the exhaustive resource type of this composition. Every
    /// returned buffer belongs to one of these two pools; no unknown-class
    /// branch or fallible downcast remains in the service.
    fn tx_buffer(
        buffer: <Self::Types as PhaseTypes<'id>>::AnyTxBuffer,
    ) -> StationTxBuffer<'id, Self>;
}

/// The only TX memory a station service's port returns. This describes
/// resources, not operation identity: only `TxContext` selects the operation.
pub enum StationTxBuffer<'id, B: StationPort<'id>> {
    /// The station's fresh allocation or its published MPDU.
    Small(PoolMpdu<'id, B::Types, B::Small>),
    /// Data's fresh allocation or its published MPDU.
    Large(PoolMpdu<'id, B::Types, B::Large>),
}

/// The composition's context for this service. Each variant owns the
/// original machine operation; the backend treats the entire value as opaque.
#[derive(Debug)]
pub enum TxContext<'id, K, C> {
    /// A station protocol operation, including its connection epoch and purpose.
    Station(StaTxContext<'id>),
    /// An MSDU operation, including its original link, TID and queue order.
    Data(DataTxContext<'id, K, C>),
}

/// Each step polls every source and consumes at most one input from each.
/// Eight steps amortize wakeup cost while forcing a cooperative yield before
/// a continuously ready source can monopolize the executor.
const MAX_INPUT_STEPS_PER_BATCH: u8 = 8;

/// What the service dropped or could not send, counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServiceCounters {
    /// Indications of lost port events; EventsLost carries no frame count.
    pub events_lost: oer_ieee80211_lower_mac::Counter,
    /// RX payloads larger than the network's checked Ethernet contract.
    pub rx_msdus_too_long: oer_ieee80211_lower_mac::Counter,
    /// Frames not sent: no rate of their plan is on the port's channel, or
    /// they do not fit their buffer class. Their tokens came back.
    pub unsent_frames: oer_ieee80211_lower_mac::Counter,
    /// MSDUs the network stack sent that do not fit a buffer.
    pub tx_msdus_too_long: oer_ieee80211_lower_mac::Counter,
    /// Received MSDUs the network stack's full queue could not take.
    pub rx_msdus_queue_full: oer_ieee80211_lower_mac::Counter,
}

/// A marker of the install `'id` and two types.
type Marker<'id, A, B> = PhantomData<fn(&'id ()) -> (A, B)>;

/// The station's tokens on a port of the phase types `Ty`, its frames in
/// buffers of the class `S`.
pub struct PortStaTokens<'id, Ty, S>(Marker<'id, Ty, S>);

impl<'id, Ty, S> StaTokens for PortStaTokens<'id, Ty, S>
where
    Ty: PhaseTypes<'id> + TxBufferPool<'id, S>,
    S: TxBufferClass,
{
    type Small = S;
    type Ticket = Slot<'id, Ty::TicketKind>;
    type SmallBuffer = <Ty as TxBufferPool<'id, S>>::Buffer;
    type Sent = <Ty as TxBufferPool<'id, S>>::Sent;

    fn renew(mpdu: Self::Sent) -> Self::SmallBuffer {
        let (memory, body) = <Ty as TxBufferPool<'id, S>>::renew(mpdu);
        drop(body);
        memory
    }
    type PairwiseSlot = <Ty::StationKeys as KeyTypes<'id>>::PairwiseSlot;
    type GroupSlot = <Ty::StationKeys as KeyTypes<'id>>::GroupSlot;
    type PairwiseKey = <Ty::StationKeys as KeyTypes<'id>>::PairwiseKey;
    type GroupKey = <Ty::StationKeys as KeyTypes<'id>>::GroupKey;
    type RxMeta = Ty::RxMeta;
}

/// The data machine's tokens on a port of the phase types `Ty`, its MPDUs
/// in buffers of the class `L`.
pub struct PortDataTokens<'id, Ty, L>(Marker<'id, Ty, L>);

impl<'id, Ty, L> StaDataTokens for PortDataTokens<'id, Ty, L>
where
    Ty: PhaseTypes<'id> + TxBufferPool<'id, L>,
    L: TxBufferClass,
{
    type Class = L;
    type Ticket = Slot<'id, Ty::TicketKind>;
    type Buffer = <Ty as TxBufferPool<'id, L>>::Buffer;
    type Sent = <Ty as TxBufferPool<'id, L>>::Sent;

    fn renew(mpdu: Self::Sent) -> Self::Buffer {
        let (memory, body) = <Ty as TxBufferPool<'id, L>>::renew(mpdu);
        drop(body);
        memory
    }
}

/// The metadata the port `B` gives a received frame.
pub type PortRxMeta<'id, B> = <<B as StationPort<'id>>::Types as PhaseTypes<'id>>::RxMeta;

/// The station of the service on the port `B`.
pub type PortStation<'id, B> = Station<
    'id,
    PortStaTokens<'id, <B as StationPort<'id>>::Types, <B as StationPort<'id>>::Small>,
    <B as LowerMacBackend>::Caps,
>;

/// The data machine of the service on the port `B`.
pub type PortData<'id, B> = StaData<
    'id,
    PortDataTokens<'id, <B as StationPort<'id>>::Types, <B as StationPort<'id>>::Large>,
    <B as LowerMacBackend>::Caps,
>;

/// The arbiter of the service on the port `B`.
pub type PortArbiter<'a, 'id, B> = Arbiter<'a, 'id, <B as LowerMacBackend>::Caps>;

/// The levels the service's attempts take, which its port must offer:
/// checked when the service's type is built, so a port without them builds
/// no service.
struct Levels<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Levels<K> {
    /// The station's connection frames (Authentication, Association,
    /// EAPOL), which a coexisting radio lets through first.
    const CONNECTION: SupportedCoex<K> = {
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() -> SupportedCoex<K> {
            match SupportedCoex::new(CoexPriority::Elevated) {
                Some(level) => level,
                None => panic!("a station's port raises its connection frames' coexistence level"),
            }
        }
        checked::<K>()
    };
    /// Every other frame.
    const NORMAL: SupportedCoex<K> = {
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() -> SupportedCoex<K> {
            match SupportedCoex::new(CoexPriority::Normal) {
                Some(level) => level,
                None => panic!("a station's port sends at its normal coexistence level"),
            }
        }
        checked::<K>()
    };
    /// The calibrated power.
    const POWER: SupportedPower<K> = {
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() -> SupportedPower<K> {
            match SupportedPower::new(TxPower::Calibrated) {
                Some(power) => power,
                None => panic!("a station's port sends at its calibrated power"),
            }
        }
        checked::<K>()
    };
}

mod ethernet;
mod msdu_queue;
pub use ethernet::{EthernetFrame, FrameTooLong, MAX_ETHERNET_PAYLOAD_LEN};
pub use msdu_queue::{MsduQueue, MsduReceiver, MsduSender};

/// One checked, owned Ethernet frame with its user priority.
#[derive(Debug)]
pub struct TxMsdu {
    pub priority: WmmUserPriority,
    pub frame: EthernetFrame,
}

/// What the application asks of the service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceCommand<K> {
    /// A command of the station.
    Station(StaCommand),
    /// A fact of the access point beside the station.
    AccessPoint(AccessPointFact<K>),
    /// The access point beside the station is wanted.
    WantAccessPoint,
    /// Stop: the service disables the port and returns its owner.
    Stop,
}

/// What the service tells the application.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a refusal returns the original owned packet without allocation"
)]
pub enum ServiceOutput<K> {
    /// The original network operation refused before data admission.
    MsduRefused { packet: TxMsdu, reason: DataRefusal },
    /// A received MSDU cannot fit the checked Ethernet contract.
    RxMsduRefused(FrameTooLong),
    /// An operation refused before admission. Its resources return and the
    /// service continues; this is not a TX completion or radio statistic.
    TxRefused {
        /// The machine that originated the operation.
        operation: TxOperation,
        /// The preparation failure.
        reason: TxRefusal,
    },
    /// The station's notice.
    Notice(StaNotice),
    /// The arbiter's command to the access point beside the station.
    AccessPoint(AccessPointCommand<K>),
}

/// The origin of a preparation refusal reported to the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOperation {
    /// A station protocol frame.
    Station,
    /// A connected-link MSDU.
    Data,
}

#[derive(Clone, Copy)]
enum ReturnedTx {
    Completed(TxOutcome),
    Refused(TxRefusal),
}

/// A backend returned owners that cannot belong to the submitted operation.
#[derive(Debug)]
struct BackendContractViolation;

/// The service's ends to the application and the network stack.
pub struct ServiceIo<'c, K, const TX_DEPTH: usize, const RX_DEPTH: usize> {
    /// The application's commands.
    pub commands: DynamicReceiver<'c, ServiceCommand<K>>,
    /// The MSDUs the network stack sends.
    pub transmit: MsduReceiver<'c, TxMsdu, TX_DEPTH>,
    /// The MSDUs the station received for the network stack.
    pub receive: MsduSender<'c, EthernetFrame, RX_DEPTH>,
    /// What the service tells the application.
    pub output: &'c mut dyn FnMut(ServiceOutput<K>),
}

/// How the service ended.
pub enum ServiceEnd<'p, 'id, B: PortBackend> {
    /// Stopped on the application's command: the port is disabled.
    Stopped(Owner<'p, 'id, B, Disabled>),
    /// The port failed (it refused to come back to a channel, or a phase
    /// reported it poisoned and its disable still ended): it is disabled,
    /// and the arbiter stopped the station.
    Failed(Owner<'p, 'id, B, Disabled>),
    /// The port is poisoned.
    Poisoned(PoisonedOwner<'p, 'id, B>),
}

/// The station's service: its machines, the rate selection of its frames
/// and the received buffers a disable returned, which go back with the
/// next phase. `RX` bounds those buffers: at least the port's receive
/// buffers.
pub struct StationService<'a, 'id, B: StationPort<'id>, S, const RX: usize> {
    /// The station.
    pub station: PortStation<'id, B>,
    /// Its data machine.
    pub data: PortData<'id, B>,
    /// The arbiter of the port.
    pub arbiter: PortArbiter<'a, 'id, B>,
    /// The rates of the station's frames and data.
    pub rates: S,
    /// What the service dropped or could not send.
    pub counters: ServiceCounters,
    configuring: Option<oer_ieee80211_sta::machine::PendingConfiguration<'id>>,
    held: [Option<<B::Types as PhaseTypes<'id>>::RxBuffer>; RX],
}

/// What a step left the loop to do.
enum Next<K> {
    /// Wait for the next input.
    Wait,
    /// Move the port to the channel.
    Retune(oer_ieee80211_arbiter::PendingRetune<K>),
    /// The port is poisoned.
    Poisoned,
}

impl<'a, 'id, B: StationPort<'id>, S: RateSelection<PortRxMeta<'id, B>>, const RX: usize>
    StationService<'a, 'id, B, S, RX>
{
    /// The service of the machines the composition built, its frames at
    /// the rates `rates` selects.
    pub fn new(
        station: PortStation<'id, B>,
        data: PortData<'id, B>,
        arbiter: PortArbiter<'a, 'id, B>,
        rates: S,
    ) -> Self {
        // The port's received buffers fit the held ones.
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
            const fn checked<'id, B: StationPort<'id>, const RX: usize>() {
                if RX < B::RX_BUFFERS {
                    panic!("the service holds every received buffer its port lends");
                }
            }
            checked::<'id, B, RX>();
        };
        // The port's levels, checked as the type is built.
        const {
            let _connection = Levels::<B::Caps>::CONNECTION;
            let _normal = Levels::<B::Caps>::NORMAL;
            let _power = Levels::<B::Caps>::POWER;
        };
        Self {
            station,
            data,
            arbiter,
            rates,
            counters: ServiceCounters::default(),
            configuring: None,
            held: [const { None }; RX],
        }
    }

    /// Run the service on the disabled port until the application stops it
    /// or the port fails.
    pub async fn run<'p, const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        owner: Owner<'p, 'id, B, Disabled>,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
        timer: &impl Timer,
    ) -> ServiceEnd<'p, 'id, B> {
        let mut owner = match self.enable(owner, None, io, timer).await {
            Ok(owner) => owner,
            Err(end) => return end,
        };
        loop {
            let next = {
                let mut lent = owner.phase();
                let phase = B::lend(&mut lent);
                self.serve(phase, io, timer).await
            };
            match next {
                Served::Stop => {
                    return match self.disable(owner, io, timer).await {
                        Ok((owner, Ok(()))) => ServiceEnd::Stopped(owner),
                        Ok((owner, Err(_))) => ServiceEnd::Failed(owner),
                        Err(poisoned) => ServiceEnd::Poisoned(poisoned),
                    };
                }
                Served::Poisoned => {
                    return match self.disable(owner, io, timer).await {
                        Ok((owner, _)) => ServiceEnd::Failed(owner),
                        Err(poisoned) => ServiceEnd::Poisoned(poisoned),
                    };
                }
                Served::Retune(pending) => {
                    let disabled = match self.disable(owner, io, timer).await {
                        Ok((owner, Ok(()))) => owner,
                        Ok((owner, Err(_))) => return ServiceEnd::Failed(owner),
                        Err(poisoned) => {
                            self.fail_stop(timer.now(), io);
                            return ServiceEnd::Poisoned(poisoned);
                        }
                    };
                    owner = match self.enable(disabled, Some(pending), io, timer).await {
                        Ok(owner) => owner,
                        Err(end) => return end,
                    };
                }
            }
        }
    }

    /// Enable the port, tuned to `channel` first when it moves; a refused
    /// move goes back to the arbiter, which names the next.
    async fn enable<'p, const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        mut owner: Owner<'p, 'id, B, Disabled>,
        mut pending: Option<oer_ieee80211_arbiter::PendingRetune<B::Caps>>,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
        timer: &impl Timer,
    ) -> Result<Owner<'p, 'id, B, Enabled>, ServiceEnd<'p, 'id, B>> {
        loop {
            if let Some(pending) = pending.as_ref() {
                let mut setup = owner.setup();
                B::tune(&mut setup, pending.channel());
            }
            match owner.enable().await {
                Ok(enabled) => {
                    if let Some(pending) = pending {
                        self.arbiter.retuned(pending);
                    }
                    return Ok(enabled);
                }
                Err(Failed::Refused(disabled, _)) => {
                    owner = disabled;
                    let Some(requested) = pending.take() else {
                        // Initial enable has no retune action to answer.
                        self.fail_stop(timer.now(), io);
                        return Err(ServiceEnd::Failed(owner));
                    };
                    pending = self.arbiter.retune_refused(requested);
                    let now = timer.now();
                    // A disabled port receives only notices. The return retune
                    // came directly with its affine token, without polling
                    // actions that could consume a gate.
                    self.deliver_arbiter_notices(now, io);
                    if pending.is_none() {
                        self.settle(now, io);
                        return Err(ServiceEnd::Failed(owner));
                    }
                }
                Err(Failed::Poisoned(poisoned)) => {
                    self.fail_stop(timer.now(), io);
                    return Err(ServiceEnd::Poisoned(poisoned));
                }
            }
        }
    }

    /// Disable the port: the work it ends comes back as events, its
    /// received buffers held for the next phase.
    async fn disable<'p, const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        owner: Owner<'p, 'id, B, Enabled>,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
        timer: &impl Timer,
    ) -> Result<
        (
            Owner<'p, 'id, B, Disabled>,
            Result<(), BackendContractViolation>,
        ),
        PoisonedOwner<'p, 'id, B>,
    > {
        let mut delivery = Ok(());
        let mut sink = |event: B::Event<'id>| {
            let now = timer.now();
            if delivery.is_err() {
                // Fail-stop has consumed an invalid terminal. Later owners are
                // retired with this disable; they cannot re-enter the machines.
                drop(event);
                return;
            }
            match self.event(now, B::event(event), io) {
                Ok(Some(buffer)) => match self.held.iter_mut().find(|slot| slot.is_none()) {
                    Some(slot) => *slot = Some(buffer),
                    None => {
                        delivery = Err(BackendContractViolation);
                    }
                },
                Ok(None) => {}
                Err(error) => delivery = Err(error),
            }
        };
        let disabled = owner.disable(&mut sink).await?;
        if delivery.is_err() {
            self.fail_stop(timer.now(), io);
        }
        Ok((disabled, delivery))
    }

    /// Serve the running phase until the port must move, stop or failed.
    async fn serve<'p, P, const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        phase: &mut P,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
        timer: &impl Timer,
    ) -> Served<B::Caps>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    {
        for slot in &mut self.held {
            if let Some(buffer) = slot.take() {
                phase.recycle(buffer);
            }
        }
        // Each step consumes at most one input per source. Every source is
        // polled even when RX is ready; eight steps bound a batch before a
        // cooperative yield lets the driver and application progress.
        let mut batch = 0_u8;
        loop {
            match self.step(timer.now(), phase, io) {
                Next::Wait => {}
                Next::Retune(pending) => return Served::Retune(pending),
                Next::Poisoned => return Served::Poisoned,
            }
            let deadline = [self.arbiter.poll_deadline(), self.station.poll_deadline()]
                .into_iter()
                .flatten()
                .min();
            let sending = self.data.accepts() && self.station.link().is_some();
            let (event, command, tick, transmit) = {
                let event = phase.next_event();
                let command = io.commands.receive();
                let alarm = async {
                    match deadline {
                        Some(deadline) => timer.wait_until(deadline).await,
                        None => core::future::pending::<()>().await,
                    }
                };
                let transmit_ready = io.transmit.ready();
                let mut transmit_ready = core::pin::pin!(transmit_ready);
                let mut event = core::pin::pin!(event);
                let mut command = core::pin::pin!(command);
                let mut alarm = core::pin::pin!(alarm);
                poll_fn(|cx| {
                    let event = match event.as_mut().poll(cx) {
                        Poll::Ready(event) => Some(event),
                        Poll::Pending => None,
                    };
                    let command = match command.as_mut().poll(cx) {
                        Poll::Ready(command) => Some(command),
                        Poll::Pending => None,
                    };
                    let tick = alarm.as_mut().poll(cx).is_ready();
                    let transmit = sending && transmit_ready.as_mut().poll(cx).is_ready();
                    if event.is_some() || command.is_some() || tick || transmit {
                        Poll::Ready((event, command, tick, transmit))
                    } else {
                        Poll::Pending
                    }
                })
                .await
            };
            let now = timer.now();
            match event {
                Some(Ok(Ok(event))) => match self.event(now, event, io) {
                    Ok(Some(buffer)) => phase.recycle(buffer),
                    Ok(None) => {}
                    Err(_) => {
                        self.fail_stop(now, io);
                        return Served::Poisoned;
                    }
                },
                Some(Ok(Err(_))) => self.counters.events_lost.increment(),
                None => {}
                Some(Err(_)) => {
                    self.fail_stop(now, io);
                    return Served::Poisoned;
                }
            }
            if tick {
                self.station.input(now, StaInput::Tick);
            }
            if let Some(command) = command {
                match command {
                    ServiceCommand::Station(command) => {
                        self.station.input(now, StaInput::Command(command))
                    }
                    ServiceCommand::AccessPoint(fact) => self.arbiter.access_point(now, fact),
                    ServiceCommand::WantAccessPoint => self.arbiter.want_access_point(),
                    ServiceCommand::Stop => return Served::Stop,
                }
            }
            if transmit && self.data.accepts() && self.station.link().is_some() {
                self.send(now, io);
            }
            batch = batch.saturating_add(1);
            if batch == MAX_INPUT_STEPS_PER_BATCH {
                batch = 0;
                yield_now().await;
            }
        }
    }

    /// Hand the network stack's next MSDU to the data machine on the
    /// station's link.
    fn send<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) {
        let Some(mut link) = self.station.link() else {
            return;
        };
        let Some(packet) = io.transmit.try_receive() else {
            return;
        };
        let parts = packet.frame.parts();
        let msdu = Msdu {
            destination: parts.destination,
            ether_type: parts.ether_type,
            priority: packet.priority,
            payload: parts.payload,
        };
        // Admission was polled immediately above under this same exclusive
        // service borrow. A too-long MSDU is refused without changing bytes.
        match self.data.input(
            now,
            DataInput::Msdu {
                msdu,
                link: &mut link,
            },
        ) {
            Ok(()) => {}
            Err(reason) => {
                if reason == DataRefusal::TooLong {
                    self.counters.tx_msdus_too_long.increment();
                }
                // The available-pool observation is dynamic and the public
                // data input can refuse it. Return the original owned packet,
                // rather than manufacturing a terminal or losing its bytes.
                (io.output)(ServiceOutput::MsduRefused { packet, reason });
            }
        }
    }

    /// Carry out every action the machines owe at `now`.
    fn step<'p, P, const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        phase: &mut P,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) -> Next<B::Caps>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    {
        loop {
            let mut progress = false;
            while let Some(action) = self.arbiter.poll_action(now) {
                progress = true;
                match action {
                    ArbiterAction::Station(notice) => {
                        self.station.input(now, StaInput::Arbiter(notice));
                    }
                    ArbiterAction::AccessPoint(command) => {
                        (io.output)(ServiceOutput::AccessPoint(command));
                    }
                    ArbiterAction::CloseGate { gate, reserve: () } => phase.close_gate(gate),
                    ArbiterAction::Retune(pending) => return Next::Retune(pending),
                    ArbiterAction::OpenGate(closed) => {
                        let open = phase.open_gate(closed);
                        self.arbiter.gate_opened(open);
                    }
                }
            }
            while let Some(action) = self.station.poll_action(now) {
                progress = true;
                let submitted = station_action(
                    now,
                    phase,
                    io,
                    &mut self.arbiter,
                    &self.rates,
                    &mut self.configuring,
                    action,
                )
                .and_then(|follow| self.follow(now, follow, io).map_err(|_| ()));
                if submitted.is_err() {
                    self.fail_stop(now, io);
                    return Next::Poisoned;
                }
            }
            // Data goes only while the link is up: otherwise its MPDUs wait
            // in the machine, none lost. At most the backend's queue depth
            // of data attempts of a category waits at the port, so a
            // station frame, submitted at once, waits for at most that
            // depth and the one on the air ([owner decision on #563]); the
            // limit is the service's, for every data source it runs.
            //
            // [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6141039497
            loop {
                // This scope owns the short LinkView borrow. Only an owned
                // follow-up leaves it; handling it may then borrow all of self.
                let submitted = {
                    let Some(link) = self.station.link() else {
                        break;
                    };
                    let Some(DataAction::Transmit(transmit)) = self.data.poll_action(now) else {
                        break;
                    };
                    progress = true;
                    let data::DataTransmitParts {
                        context,
                        ticket,
                        buffer,
                    } = transmit.into_parts();
                    let access_category = context.access_category();
                    let plan = context.protection(&link).and_then(|protection| {
                        self.rates
                            .plan(FrameRates::Data, |rate| phase.rate(rate))
                            .map(|plan| (protection, plan))
                            .ok_or(TxRefusal::NoRate)
                    });
                    let prepared = match (buffer, plan) {
                        (ReturnedMpdu::Fresh(buffer), Ok((protection, plan))) => {
                            prepare_source::<B, B::Large>(TxSource::Fresh {
                                buffer,
                                frame: ClassFrame::individual(*context.frame(), plan),
                                body: None,
                                protection: match protection {
                                    DataProtection::Open => TxProtection::Open,
                                    DataProtection::Ccmp(key) => TxProtection::Ccmp(key),
                                },
                            })
                        }
                        (ReturnedMpdu::Sent(mpdu), Ok((_protection, plan))) => {
                            prepare_source::<B, B::Large>(TxSource::Retransmission {
                                mpdu,
                                plan: RetransmissionPlan::Individual(plan),
                            })
                        }
                        (buffer, Err(reason)) => {
                            let buffer = match buffer {
                                ReturnedMpdu::Fresh(buffer) => {
                                    <B::Types as TxBufferPool<'id, B::Large>>::erase(buffer)
                                }
                                ReturnedMpdu::Sent(mpdu) => {
                                    <B::Types as TxBufferPool<'id, B::Large>>::erase_sent(mpdu)
                                }
                            };
                            Err((reason, buffer))
                        }
                    };
                    match prepared {
                        Err((reason, buffer)) => Ok(Follow::Refused {
                            context: TxContext::Data(context),
                            ticket,
                            buffer,
                            reason,
                        }),
                        Ok(source) => {
                            let submitted =
                                phase.station().submit(oer_ieee80211_lower_mac::Attempt {
                                    context: TxContext::Data(context),
                                    ticket,
                                    source,
                                    access_category,
                                    protection: Protection::None,
                                    power: Levels::<B::Caps>::POWER,
                                    coex: Levels::<B::Caps>::NORMAL,
                                });
                            match submitted {
                                Ok(()) => Ok(Follow::Nothing),
                                Err(_) => Err(()),
                            }
                        }
                    }
                };
                match submitted {
                    Ok(follow) => {
                        if self.follow(now, follow, io).is_err() {
                            self.fail_stop(now, io);
                            return Next::Poisoned;
                        }
                    }
                    Err(()) => {
                        self.fail_stop(now, io);
                        return Next::Poisoned;
                    }
                }
            }
            if !progress {
                return Next::Wait;
            }
        }
    }

    /// What follows a station's action, once it no longer borrows the
    /// station.
    fn follow<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        follow: Follow<'id, B>,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) -> Result<(), BackendContractViolation> {
        match follow {
            Follow::Nothing => {}
            Follow::Input(input) => self.station.input(now, input),
            // The rates start from what the association made known.
            Follow::Associated(peer) => {
                self.rates
                    .associated(&peer.associated.rate_peer(), &peer.response);
            }
            Follow::Disconnected => {
                self.rates.disconnected();
                self.data.leave();
            }
            Follow::Refused {
                context,
                ticket,
                buffer,
                reason,
            } => {
                self.counters.unsent_frames.increment();
                let operation = match &context {
                    TxContext::Station(_) => TxOperation::Station,
                    TxContext::Data(_) => TxOperation::Data,
                };
                (io.output)(ServiceOutput::TxRefused { operation, reason });
                self.returned(now, context, ticket, buffer, ReturnedTx::Refused(reason))?;
            }
        }
        Ok(())
    }

    /// Report service termination without converting a recoverable enable
    /// refusal into hardware poisoning. The caller retains its typed owner.
    fn fail_stop<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) {
        self.arbiter.port_failed();
        self.settle(now, io);
    }

    fn deliver_arbiter_notices<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) {
        while let Some(notice) = self.arbiter.poll_notice(now) {
            match notice {
                ArbiterNotice::Station(notice) => {
                    self.station.input(now, StaInput::Arbiter(notice))
                }
                ArbiterNotice::AccessPoint(command) => {
                    (io.output)(ServiceOutput::AccessPoint(command))
                }
            }
        }
    }

    /// Carry out the actions that need no phase once the port failed: the
    /// arbiter's notices reach the station, the station's notices the
    /// application.
    fn settle<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) {
        self.deliver_arbiter_notices(now, io);
        while let Some(action) = self.station.poll_action(now) {
            if let StaAction::Notify(notice) = action {
                (io.output)(ServiceOutput::Notice(notice));
                if let StaNotice::Disconnected(_) = notice {
                    self.rates.disconnected();
                    self.data.leave();
                }
            }
        }
    }

    /// Take one event of the port; a received frame's buffer comes back
    /// for its recycle.
    fn event<const TX_DEPTH: usize, const RX_DEPTH: usize>(
        &mut self,
        now: Instant,
        event: LowerMacEvent<'id, B::Types>,
        io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    ) -> Result<Option<<B::Types as PhaseTypes<'id>>::RxBuffer>, BackendContractViolation> {
        Ok(match event {
            LowerMacEvent::Received { frame, meta } => {
                let bytes = frame.bytes();
                self.station
                    .input(now, StaInput::Received { frame: bytes, meta });
                if let Some(mut link) = self.station.link()
                    && let Some(msdus) = data::received(&mut link, bytes, meta.common())
                {
                    for parts in msdus {
                        // A full queue drops the MSDU, as an Ethernet
                        // receiver does, counted.
                        match EthernetFrame::from_parts(parts) {
                            Ok(frame) => {
                                if io.receive.try_send(frame).is_err() {
                                    self.counters.rx_msdus_queue_full.increment();
                                }
                            }
                            Err(reason) => {
                                self.counters.rx_msdus_too_long.increment();
                                (io.output)(ServiceOutput::RxMsduRefused(reason));
                            }
                        }
                    }
                }
                Some(frame)
            }
            LowerMacEvent::TxCompleted(TxCompleted::Single {
                context,
                ticket,
                buffer,
                completion,
                body,
            }) => {
                self.returned(
                    now,
                    context,
                    ticket,
                    buffer,
                    ReturnedTx::Completed(completion),
                )?;
                // This service submits no external bodies. Sent MPDUs own theirs.
                drop(body);
                None
            }
            LowerMacEvent::TxCompleted(TxCompleted::Aggregate { .. }) => {
                // A backend terminal must have the same cardinality as submit.
                return Err(BackendContractViolation);
            }

            LowerMacEvent::GateClosed(closed) => {
                self.arbiter.gate_closed(closed);
                None
            }
            LowerMacEvent::Configured(VifRole::Station) => {
                if let Some(pending) = self.configuring.take() {
                    self.station.input(now, StaInput::Configured(pending));
                }
                None
            }
            LowerMacEvent::KeyRemoved(Removed::Station(RemovedKey::Pairwise(slot))) => {
                self.station.input(now, StaInput::PairwiseKeyRemoved(slot));
                None
            }
            LowerMacEvent::KeyRemoved(Removed::Station(RemovedKey::Group(slot))) => {
                self.station.input(now, StaInput::GroupKeyRemoved(slot));
                None
            }
            // No access point runs in this service; the station follows no
            // TBTT or TSF sample yet.
            LowerMacEvent::Configured(VifRole::AccessPoint)
            | LowerMacEvent::KeyRemoved(Removed::AccessPoint(_))
            | LowerMacEvent::Tbtt(_)
            | LowerMacEvent::TsfSampled(_) => None,
        })
    }

    /// The context identifies the operation. Class is checked exactly once
    /// against that identity; a mismatch violates the backend contract and
    /// takes the same fail-stop path as Poisoned. A shared AC queue erases
    /// classes, so this identity obligation cannot be proved by this API.
    fn returned(
        &mut self,
        now: Instant,
        context: TxContext<'id, B::Caps, B::Large>,
        ticket: Slot<'id, <B::Types as PhaseTypes<'id>>::TicketKind>,
        buffer: <B::Types as PhaseTypes<'id>>::AnyTxBuffer,
        end: ReturnedTx,
    ) -> Result<(), BackendContractViolation> {
        match (context, B::tx_buffer(buffer)) {
            (TxContext::Station(context), StationTxBuffer::Small(buffer)) => {
                if self.station.is_current(&context)
                    && let ReturnedTx::Completed(completion) = end
                    && completion.is_rate_sample()
                {
                    let rates = if context.is_probe() {
                        FrameRates::Probe
                    } else {
                        FrameRates::Control
                    };
                    self.rates.completed(now, rates, &completion);
                }
                let input = match end {
                    ReturnedTx::Completed(completion) => StaInput::TxDone {
                        context,
                        ticket,
                        buffer,
                        completion,
                    },
                    ReturnedTx::Refused(reason) => StaInput::Unsent {
                        context,
                        ticket,
                        buffer,
                        reason,
                    },
                };
                self.station.input(now, input);
                Ok(())
            }
            (TxContext::Data(context), StationTxBuffer::Large(buffer)) => {
                if self.data.is_current(&context)
                    && let ReturnedTx::Completed(completion) = end
                    && completion.is_rate_sample()
                {
                    self.rates.completed(now, FrameRates::Data, &completion);
                }
                let input = match end {
                    ReturnedTx::Completed(completion) => DataReturn::TxDone {
                        context,
                        ticket,
                        buffer,
                        completion,
                    },
                    ReturnedTx::Refused(reason) => DataReturn::Refused {
                        context,
                        ticket,
                        buffer,
                        reason,
                    },
                };
                self.data.returned(now, input);
                Ok(())
            }
            (TxContext::Station(_), StationTxBuffer::Large(_))
            | (TxContext::Data(_), StationTxBuffer::Small(_)) => Err(BackendContractViolation),
        }
    }
}

/// What follows a station's action.
enum Follow<'id, B: StationPort<'id>> {
    Nothing,
    /// An arbiter's answer or a key, back to the station.
    Input(StaInput<'static, 'id, PortStaTokens<'id, B::Types, B::Small>, B::Caps>),
    /// The station associated: the rates start from its peer.
    Associated(Peer<PortRxMeta<'id, B>>),
    /// A frame of the station did not go.
    Refused {
        context: TxContext<'id, B::Caps, B::Large>,
        ticket: <PortStaTokens<'id, B::Types, B::Small> as StaTokens>::Ticket,
        buffer: <B::Types as PhaseTypes<'id>>::AnyTxBuffer,
        reason: TxRefusal,
    },
    /// The station left: the rates leave its peer.
    Disconnected,
}

type StationPreparation<'k, 'p, 'id, B> = Result<
    <<B as StationPort<'id>>::Types as PhaseTypes<'id>>::Prepared<
        'k,
        'p,
        oer_ieee80211_lower_mac::Station,
        <B as LowerMacBackend>::Caps,
    >,
    (
        TxRefusal,
        <<B as StationPort<'id>>::Types as PhaseTypes<'id>>::AnyTxBuffer,
    ),
>;

/// Prepare within the class pool; a refusal retains the original owners.
fn prepare_source<'k, 'p, 'id: 'p + 'k, B: StationPort<'id>, C: TxBufferClass>(
    source: PoolSource<'k, 'p, 'id, B::Types, C, B::Caps, oer_ieee80211_lower_mac::Station>,
) -> StationPreparation<'k, 'p, 'id, B>
where
    B::Types: TxBufferPool<'id, C>,
{
    let prepared = match source.check() {
        Ok(source) => <B::Types as TxBufferPool<'id, C>>::prepare(source)
            .map_err(|refused| (TxRefusal::Preparation(refused.reason), refused.operation)),
        Err(refused) => Err((TxRefusal::Layout(refused.reason), refused.operation)),
    };
    prepared.map_err(|(reason, operation)| {
        let buffer = match operation {
            TxSource::Fresh { buffer, body, .. } => {
                drop(body);
                <B::Types as TxBufferPool<'id, C>>::erase(buffer)
            }
            TxSource::Retransmission { mpdu, .. } => {
                <B::Types as TxBufferPool<'id, C>>::erase_sent(mpdu)
            }
        };
        (reason, buffer)
    })
}

/// Submit a station frame with its phase rate and optional pairwise key.
/// Preparation refusal returns the operation to the station without admission.
fn submit_station<'p, 'id: 'p, B, P, S>(
    phase: &mut P,
    rates: &S,
    key: Option<&<<B::Types as PhaseTypes<'id>>::StationKeys as KeyTypes<'id>>::PairwiseKey>,
    transmit: StaTransmit<'id, PortStaTokens<'id, B::Types, B::Small>, B::Caps>,
) -> Result<Follow<'id, B>, ()>
where
    B: StationPort<'id>,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    S: RateSelection<PortRxMeta<'id, B>>,
{
    let StaTransmit {
        context,
        ticket,
        buffer,
        frame,
        class,
        plan,
    } = transmit;
    let frame = {
        let carried = |rate| phase.rate(rate);
        match frame {
            StaFrame::Individual(frame) => {
                let plan = match plan {
                    // A Null's retries are its plan's
                    // ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130033259)).
                    StaRatePlan::Null => rates
                        .plan(FrameRates::Control, carried)
                        .map(|plan| plan.limit(NULL_TRANSMISSIONS)),
                    StaRatePlan::Management | StaRatePlan::Group => {
                        rates.plan(FrameRates::Control, carried)
                    }
                };
                plan.map(|plan| ClassFrame::individual(frame, plan))
            }
            StaFrame::Group(frame) => rates
                .plan(FrameRates::Probe, carried)
                .map(|plan| ClassFrame::group(frame, plan.into_first().rate)),
        }
    };
    let frame = match frame.ok_or(TxRefusal::NoRate) {
        Ok(frame) => frame,
        Err(reason) => {
            return Ok(Follow::Refused {
                context: TxContext::Station(context),
                ticket,
                buffer: <B::Types as TxBufferPool<'id, B::Small>>::erase(buffer),
                reason,
            });
        }
    };
    let source = match prepare_source::<B, B::Small>(TxSource::Fresh {
        buffer,
        frame,
        body: None,
        protection: key.map_or(TxProtection::Open, TxProtection::Ccmp),
    }) {
        Ok(source) => source,
        Err((reason, buffer)) => {
            return Ok(Follow::Refused {
                context: TxContext::Station(context),
                ticket,
                buffer,
                reason,
            });
        }
    };
    let coex = match class {
        StaFrameClass::Connection => Levels::<B::Caps>::CONNECTION,
        StaFrameClass::Normal => Levels::<B::Caps>::NORMAL,
    };
    let mut station = phase.station();
    match station.submit(oer_ieee80211_lower_mac::Attempt {
        context: TxContext::Station(context),
        ticket,
        source,
        access_category: WmmAccessCategory::Voice,
        protection: Protection::None,
        power: Levels::<B::Caps>::POWER,
        coex,
    }) {
        Ok(()) => Ok(Follow::Nothing),
        Err(_) => Err(()),
    }
}

/// Carry out one action of the station; an arbiter's answer goes back to
/// it at once.
fn station_action<'p, 'id: 'p, B, P, S, const TX_DEPTH: usize, const RX_DEPTH: usize>(
    now: Instant,
    phase: &mut P,
    io: &mut ServiceIo<'_, B::Caps, TX_DEPTH, RX_DEPTH>,
    arbiter: &mut PortArbiter<'_, 'id, B>,
    rates: &S,
    configuring: &mut Option<oer_ieee80211_sta::machine::PendingConfiguration<'id>>,
    action: StaAction<'_, 'id, PortStaTokens<'id, B::Types, B::Small>, B::Caps>,
) -> Result<Follow<'id, B>, ()>
where
    B: StationPort<'id>,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    S: RateSelection<PortRxMeta<'id, B>>,
{
    let input = match action {
        StaAction::RequestChannel(request, place) => {
            StaInput::Answer(arbiter.request(now, request, place))
        }
        StaAction::RequestAbsence {
            request,
            home,
            place,
        } => StaInput::AbsenceAnswer(arbiter.request_absence(now, request, home, place)),
        StaAction::Ready(lease) => {
            arbiter.ready(lease);
            return Ok(Follow::Nothing);
        }
        StaAction::ExchangeLease(old) => StaInput::Exchanged(arbiter.exchange(old)),
        StaAction::ReleaseChannel(lease) => {
            arbiter.release(lease);
            return Ok(Follow::Nothing);
        }
        StaAction::Upstream(fact) => {
            arbiter.upstream(now, fact);
            return Ok(Follow::Nothing);
        }
        StaAction::Configure(receive, pending) => {
            *configuring = Some(pending);
            phase.station().configure(receive);
            return Ok(Follow::Nothing);
        }
        StaAction::InstallPairwiseKey {
            pending,
            slot,
            peer,
            key,
        } => StaInput::PairwiseKeyInstalled {
            pending,
            key: phase
                .station()
                .install_pairwise_key(slot, peer, Ccmp128Key(key)),
        },
        StaAction::InstallGroupKey {
            pending,
            slot,
            key_id,
            key,
        } => StaInput::GroupKeyInstalled {
            key: phase
                .station()
                .install_group_key(slot, key_id, Ccmp128Key(key)),
            pending,
        },
        StaAction::RemovePairwiseKey(key) => {
            phase.station().remove_pairwise_key(key);
            return Ok(Follow::Nothing);
        }
        StaAction::RemoveGroupKey(key) => {
            phase.station().remove_group_key(key);
            return Ok(Follow::Nothing);
        }
        StaAction::Transmit(transmit) => {
            return submit_station::<B, P, S>(phase, rates, None, transmit);
        }
        StaAction::TransmitProtected { transmit, key } => {
            return submit_station::<B, P, S>(phase, rates, Some(key), transmit);
        }
        StaAction::Notify(notice) => {
            (io.output)(ServiceOutput::Notice(notice));
            return Ok(match notice {
                StaNotice::Disconnected(_) => Follow::Disconnected,
                StaNotice::Connected { .. } | StaNotice::BackgroundScanDone { .. } => {
                    Follow::Nothing
                }
            });
        }
        StaAction::Associated(peer) => {
            return Ok(Follow::Associated(peer));
        }
    };
    Ok(Follow::Input(input))
}

/// What serving a phase ended with.
enum Served<K> {
    Stop,
    Retune(oer_ieee80211_arbiter::PendingRetune<K>),
    Poisoned,
}

#[cfg(test)]
mod tests;
