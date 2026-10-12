//! The station's thin service on a lower-MAC port: the one loop that waits
//! and carries out what the sans-IO machines decide (owner request on
//! #563).
//!
//! It waits at one point, on the port's next event, the machines' next
//! deadline and the application (a command, or an MSDU while the data
//! machine takes one). After each input it takes every action of the
//! arbiter ([`Arbiter`]), the station ([`Station`]) and its data machine
//! ([`StaData`]) until none owes one; an answer of the arbiter goes back to
//! the station in the same step. A retune leaves the phase: the port is
//! disabled, tuned and enabled, and the arbiter hears how it went.
//!
//! The tokens the machines hold are the port's: tickets and buffers of two
//! classes, the small one for the station's frames and the large one for
//! data. A completion goes back to the machine whose buffer class it
//! carries, a numbered data buffer renewed. MSDUs come and go through
//! single-producer single-consumer queues (`zerocopy_channel`); a received
//! MSDU the network's full queue cannot take is dropped, as an Ethernet
//! receiver drops one.

use core::marker::PhantomData;
use core::num::NonZeroU8;

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::DynamicReceiver;
use embassy_sync::zerocopy_channel;
use oer_ieee80211_arbiter::{
    AccessPointCommand, AccessPointFact, Arbiter, ArbiterAction, GateTokens,
};
use oer_ieee80211_lower_mac::{
    AttemptKey, Ccmp128Key, ClassFrame, CoexPriority, GateClosed, GateOpen, Ieee80211Capabilities,
    InterfacePhase, KeyTypes, LowerMacBackend, LowerMacEvent, LowerMacPhase,
    PhaseTypes, Protection, Removed, RemovedKey, RetryPlan, RxBuffer, PhaseRate,
    StationPhase, SupportedChannel, SupportedCoex, SupportedPower, TxBufferClass,
    TxBufferPool, TxCompletion, TxFrame, TxPower, VifRole,
};
use oer_ieee80211_mac::{
    phy::PhyRate,
    qos::{WmmAccessCategory, WmmUserPriority},
};
use oer_ieee80211_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_ieee80211_port::{Disabled, Enabled, Failed, Owner, PoisonedOwner, PortBackend};
use oer_radio_port::Slot;
use oer_ieee80211_sta::{
    data::{self, DataAction, DataInput, Msdu, StaData, StaDataTokens},
    machine::{
        LinkProtection, Peer, StaAction, StaCommand, StaFrame, StaFrameClass, StaInput,
        StaNotice, StaRatePlan, StaTokens, StaTransmit, Station,
    },
    power_save::NULL_TRANSMISSIONS,
};
use oer_time::{Instant, Timer};

/// The longest Ethernet frame a queue's slot holds: its 14-octet header and
/// a 1500-octet payload.
pub const ETHERNET_FRAME: usize = 1514;

/// The Ethernet header: destination, source and EtherType.
const ETHERNET_HEADER: usize = 14;

/// A backend the station's service drives: its phase, settings and events
/// as the lower-MAC contract's, and the two buffer classes the service
/// splits between the station and data.
pub trait StationPort<'id>: LowerMacBackend {
    /// The types its phase's events carry.
    type Types: PhaseTypes<'id> + TxBufferPool<'id, Self::Small> + TxBufferPool<'id, Self::Large>;
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

    /// One event of the port, as the contract's.
    fn event(event: Self::Event<'id>) -> LowerMacEvent<'id, Self::Types>;
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
    type Ticket = Slot<'id, Ty::TicketKind>;
    type SmallBuffer = <Ty as TxBufferPool<'id, S>>::Buffer;
    type PairwiseSlot = Slot<'id, <Ty::StationKeys as KeyTypes<'id>>::PairwiseKind>;
    type GroupSlot = Slot<'id, <Ty::StationKeys as KeyTypes<'id>>::GroupKind>;
    type PairwiseKey = <Ty::StationKeys as KeyTypes<'id>>::PairwiseKey;
    type GroupKey = <Ty::StationKeys as KeyTypes<'id>>::GroupKey;
}

/// The data machine's tokens on a port of the phase types `Ty`, its MPDUs
/// in buffers of the class `L`.
pub struct PortDataTokens<'id, Ty, L>(Marker<'id, Ty, L>);

impl<'id, Ty, L> StaDataTokens for PortDataTokens<'id, Ty, L>
where
    Ty: PhaseTypes<'id> + TxBufferPool<'id, L>,
    L: TxBufferClass,
{
    type Ticket = Slot<'id, Ty::TicketKind>;
    type Buffer = <Ty as TxBufferPool<'id, L>>::Buffer;
}

/// The arbiter's gate tokens on a port of the install `'id`.
pub struct PortGate<'id>(PhantomData<fn(&'id ()) -> &'id ()>);

impl<'id> GateTokens for PortGate<'id> {
    type Open = GateOpen<'id>;
    type Closed = GateClosed<'id>;
}

/// The station of the service on the port `B`.
pub type PortStation<'id, B> = Station<
    PortStaTokens<'id, <B as StationPort<'id>>::Types, <B as StationPort<'id>>::Small>,
    <B as LowerMacBackend>::Caps,
>;

/// The data machine of the service on the port `B`.
pub type PortData<'id, B> = StaData<
    PortDataTokens<'id, <B as StationPort<'id>>::Types, <B as StationPort<'id>>::Large>,
    <B as LowerMacBackend>::Caps,
>;

/// The arbiter of the service on the port `B`.
pub type PortArbiter<'a, 'id, B> = Arbiter<'a, PortGate<'id>, <B as LowerMacBackend>::Caps>;

/// The levels the service's attempts take, which its port must offer:
/// checked when the service's type is built, so a port without them builds
/// no service.
struct Levels<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Levels<K> {
    /// The station's connection frames (Authentication, Association,
    /// EAPOL), which a coexisting radio lets through first.
    const CONNECTION: SupportedCoex<K> = match SupportedCoex::new(CoexPriority::Elevated) {
        Some(level) => level,
        None => panic!("a station's port raises its connection frames' coexistence level"),
    };
    /// Every other frame.
    const NORMAL: SupportedCoex<K> = match SupportedCoex::new(CoexPriority::Normal) {
        Some(level) => level,
        None => panic!("a station's port sends at its normal coexistence level"),
    };
    /// The calibrated power.
    const POWER: SupportedPower<K> = match SupportedPower::new(TxPower::Calibrated) {
        Some(power) => power,
        None => panic!("a station's port sends at its calibrated power"),
    };
    /// A Null's transmissions.
    const NULL: NonZeroU8 = match NonZeroU8::new(NULL_TRANSMISSIONS) {
        Some(transmissions) => transmissions,
        None => panic!("a Null is sent at least once"),
    };
}

/// One Ethernet frame the network stack sends, with its user priority.
pub struct TxMsdu {
    /// Its user priority.
    pub priority: WmmUserPriority,
    /// The frame's length.
    pub len: usize,
    /// The frame, from its header on.
    pub frame: [u8; ETHERNET_FRAME],
}

/// One Ethernet frame the station received for the network stack.
pub struct RxMsdu {
    /// The frame's length.
    pub len: usize,
    /// The frame, from its header on.
    pub frame: [u8; ETHERNET_FRAME],
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceOutput<K> {
    /// The station's notice.
    Notice(StaNotice),
    /// The arbiter's command to the access point beside the station.
    AccessPoint(AccessPointCommand<K>),
}

/// The service's ends to the application and the network stack.
pub struct ServiceIo<'c, M: RawMutex, K> {
    /// The application's commands.
    pub commands: DynamicReceiver<'c, ServiceCommand<K>>,
    /// The MSDUs the network stack sends.
    pub transmit: zerocopy_channel::Receiver<'c, M, TxMsdu>,
    /// The MSDUs the station received for the network stack.
    pub receive: zerocopy_channel::Sender<'c, M, RxMsdu>,
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
    held: [Option<<B::Types as PhaseTypes<'id>>::RxBuffer>; RX],
}

/// What a step left the loop to do.
enum Next<K> {
    /// Wait for the next input.
    Wait,
    /// Move the port to the channel.
    Retune(SupportedChannel<K>),
    /// The port is poisoned.
    Poisoned,
}

impl<'a, 'id, B: StationPort<'id>, S: RateSelection, const RX: usize>
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
        // The port's levels, checked as the type is built.
        let _ = (
            Levels::<B::Caps>::CONNECTION,
            Levels::<B::Caps>::NORMAL,
            Levels::<B::Caps>::POWER,
        );
        Self {
            station,
            data,
            arbiter,
            rates,
            held: [const { None }; RX],
        }
    }

    /// Run the service on the disabled port until the application stops it
    /// or the port fails.
    pub async fn run<'p, M: RawMutex>(
        &mut self,
        owner: Owner<'p, 'id, B, Disabled>,
        io: &mut ServiceIo<'_, M, B::Caps>,
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
                        Ok(owner) => ServiceEnd::Stopped(owner),
                        Err(poisoned) => ServiceEnd::Poisoned(poisoned),
                    };
                }
                Served::Poisoned => {
                    return match self.disable(owner, io, timer).await {
                        Ok(owner) => ServiceEnd::Failed(owner),
                        Err(poisoned) => ServiceEnd::Poisoned(poisoned),
                    };
                }
                Served::Retune(channel) => {
                    let disabled = match self.disable(owner, io, timer).await {
                        Ok(owner) => owner,
                        Err(poisoned) => {
                            self.arbiter.poisoned();
                            self.settle(timer.now(), io);
                            return ServiceEnd::Poisoned(poisoned);
                        }
                    };
                    owner = match self.enable(disabled, Some(channel), io, timer).await {
                        Ok(owner) => owner,
                        Err(end) => return end,
                    };
                }
            }
        }
    }

    /// Enable the port, tuned to `channel` first when it moves; a refused
    /// move goes back to the arbiter, which names the next.
    async fn enable<'p, M: RawMutex>(
        &mut self,
        mut owner: Owner<'p, 'id, B, Disabled>,
        mut channel: Option<SupportedChannel<B::Caps>>,
        io: &mut ServiceIo<'_, M, B::Caps>,
        timer: &impl Timer,
    ) -> Result<Owner<'p, 'id, B, Enabled>, ServiceEnd<'p, 'id, B>> {
        loop {
            if let Some(channel) = channel {
                let mut setup = owner.setup();
                B::tune(&mut setup, channel);
            }
            match owner.enable().await {
                Ok(enabled) => {
                    if let Some(channel) = channel {
                        self.arbiter.retuned(channel);
                    }
                    return Ok(enabled);
                }
                Err(Failed::Refused(disabled, _)) => {
                    owner = disabled;
                    self.arbiter.retune_refused();
                    // The arbiter's notices reach the station; its next
                    // action is the move back, or none once it stopped.
                    channel = None;
                    let now = timer.now();
                    while let Some(action) = self.arbiter.poll_action(now) {
                        match action {
                            ArbiterAction::Retune(to) => {
                                channel = Some(to);
                                break;
                            }
                            ArbiterAction::Station(notice) => {
                                self.station.input(now, StaInput::Arbiter(notice));
                            }
                            ArbiterAction::AccessPoint(command) => {
                                (io.output)(ServiceOutput::AccessPoint(command));
                            }
                            // The gate stays closed while the port is
                            // disabled.
                            ArbiterAction::CloseGate { .. } | ArbiterAction::OpenGate(_) => {}
                        }
                    }
                    if channel.is_none() {
                        self.settle(now, io);
                        return Err(ServiceEnd::Failed(owner));
                    }
                }
                Err(Failed::Poisoned(poisoned)) => {
                    self.arbiter.poisoned();
                    return Err(ServiceEnd::Poisoned(poisoned));
                }
            }
        }
    }

    /// Disable the port: the work it ends comes back as events, its
    /// received buffers held for the next phase.
    async fn disable<'p, M: RawMutex>(
        &mut self,
        owner: Owner<'p, 'id, B, Enabled>,
        io: &mut ServiceIo<'_, M, B::Caps>,
        timer: &impl Timer,
    ) -> Result<Owner<'p, 'id, B, Disabled>, PoisonedOwner<'p, 'id, B>> {
        let mut sink = |event: B::Event<'id>| {
            let now = timer.now();
            if let Some(buffer) = self.event(now, B::event(event), io) {
                let slot = self
                    .held
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .expect("the service holds every receive buffer of its port");
                *slot = Some(buffer);
            }
        };
        owner.disable(&mut sink).await
    }

    /// Serve the running phase until the port must move, stop or failed.
    async fn serve<'p, M: RawMutex, P>(
        &mut self,
        phase: &mut P,
        io: &mut ServiceIo<'_, M, B::Caps>,
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
        loop {
            match self.step(timer.now(), phase, io) {
                Next::Wait => {}
                Next::Retune(channel) => return Served::Retune(channel),
                Next::Poisoned => return Served::Poisoned,
            }
            let deadline = [
                self.arbiter.poll_deadline(),
                self.station.poll_deadline(),
                self.data.poll_deadline(),
            ]
            .into_iter()
            .flatten()
            .min();
            let sending = self.data.accepts() && self.station.link().is_some();
            let input = {
                let event = phase.next_event();
                let timer = async {
                    match deadline {
                        Some(deadline) => timer.wait_until(deadline).await,
                        None => core::future::pending().await,
                    }
                };
                let transmit = &mut io.transmit;
                let application = select(io.commands.receive(), async move {
                    if sending {
                        transmit.receive().await;
                    } else {
                        core::future::pending::<()>().await;
                    }
                });
                select3(event, timer, application).await
            };
            let now = timer.now();
            match input {
                Either3::First(Ok(Ok(event))) => {
                    if let Some(buffer) = self.event(now, event, io) {
                        phase.recycle(buffer);
                    }
                }
                // Received frames were dropped; the machines' timers cover
                // what they lose.
                Either3::First(Ok(Err(_))) => {}
                Either3::First(Err(_)) => {
                    self.arbiter.poisoned();
                    self.settle(now, io);
                    return Served::Poisoned;
                }
                Either3::Second(()) => {}
                Either3::Third(Either::First(command)) => match command {
                    ServiceCommand::Station(command) => {
                        self.station.input(now, StaInput::Command(command));
                    }
                    ServiceCommand::AccessPoint(fact) => self.arbiter.access_point(now, fact),
                    ServiceCommand::WantAccessPoint => self.arbiter.want_access_point(),
                    ServiceCommand::Stop => return Served::Stop,
                },
                Either3::Third(Either::Second(())) => self.send(now, io),
            }
        }
    }

    /// Hand the network stack's next MSDU to the data machine on the
    /// station's link.
    fn send<M: RawMutex>(&mut self, now: Instant, io: &mut ServiceIo<'_, M, B::Caps>) {
        let Some(msdu) = io.transmit.try_receive() else {
            return;
        };
        let Some(mut link) = self.station.link() else {
            return;
        };
        let frame = &msdu.frame[..msdu.len.min(ETHERNET_FRAME)];
        if frame.len() >= ETHERNET_HEADER {
            let mut destination = [0; 6];
            destination.copy_from_slice(&frame[..6]);
            let msdu = Msdu {
                destination,
                ether_type: u16::from_be_bytes([frame[12], frame[13]]),
                priority: msdu.priority,
                payload: &frame[ETHERNET_HEADER..],
            };
            // The machine took it, or it can never go (too long): either
            // way it leaves the queue.
            let _ = self.data.input(now, DataInput::Msdu { msdu, link: &mut link });
        }
        io.transmit.receive_done();
    }

    /// Carry out every action the machines owe at `now`.
    fn step<'p, M: RawMutex, P>(
        &mut self,
        now: Instant,
        phase: &mut P,
        io: &mut ServiceIo<'_, M, B::Caps>,
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
                    ArbiterAction::Retune(channel) => return Next::Retune(channel),
                    ArbiterAction::OpenGate(closed) => {
                        let open = phase.open_gate(closed);
                        self.arbiter.gate_opened(open);
                    }
                }
            }
            while let Some(action) = self.station.poll_action(now) {
                progress = true;
                let submitted =
                    station_action(now, phase, io, &mut self.arbiter, &self.rates, action)
                        .map(|follow| self.follow(now, follow));
                if submitted.is_err() {
                    self.arbiter.poisoned();
                    self.settle(now, io);
                    return Next::Poisoned;
                }
            }
            while let Some(DataAction::Transmit(transmit)) = self.data.poll_action(now) {
                progress = true;
                let Some(link) = self.station.link() else {
                    // The link closed in this step: the MPDU's tokens come
                    // back unsent.
                    let _ = self.data.input::<PortStaTokens<'id, B::Types, B::Small>>(
                        now,
                        DataInput::TxDone {
                            ticket: transmit.ticket,
                            buffer: transmit.buffer,
                        },
                    );
                    continue;
                };
                let key = match (&link.protection, transmit.protected) {
                    (LinkProtection::Ccmp { key, .. }, true) => Some(AttemptKey::Pairwise(*key)),
                    _ => None,
                };
                let plan = phase_plan(phase, self.rates.plan(FrameRates::Data));
                let frame = TxFrame::Individual {
                    frame: transmit.frame,
                    plan,
                };
                let mut station = phase.station();
                let Ok(frame) = ClassFrame::<B::Large, B::Caps>::new(frame) else {
                    unreachable!("an MPDU encoded into its buffer fits its class");
                };
                let frame = station.tx_buffer::<B::Large>(transmit.buffer, frame);
                let submitted = station.submit(oer_ieee80211_lower_mac::Attempt {
                    ticket: transmit.ticket,
                    frame,
                    body: None,
                    access_category: transmit.access_category,
                    protection: Protection::None,
                    key,
                    power: Levels::<B::Caps>::POWER,
                    coex: Levels::<B::Caps>::NORMAL,
                });
                if submitted.is_err() {
                    self.arbiter.poisoned();
                    self.settle(now, io);
                    return Next::Poisoned;
                }
            }
            if !progress {
                return Next::Wait;
            }
        }
    }

    /// What follows a station's action, once it no longer borrows the
    /// station.
    fn follow(&mut self, now: Instant, follow: Follow<'id, B>) {
        match follow {
            Follow::Nothing => {}
            Follow::Input(input) => self.station.input(now, input),
            // The rates start from what the association made known.
            Follow::Associated(peer) => {
                self.rates
                    .associated(&peer.associated.rate_peer(), peer.link_metric);
            }
            Follow::Disconnected => self.rates.disconnected(),
        }
    }

    /// Carry out the actions that need no phase once the port failed: the
    /// arbiter's notices reach the station, the station's notices the
    /// application.
    fn settle<M: RawMutex>(&mut self, now: Instant, io: &mut ServiceIo<'_, M, B::Caps>) {
        while let Some(action) = self.arbiter.poll_action(now) {
            match action {
                ArbiterAction::Station(notice) => {
                    self.station.input(now, StaInput::Arbiter(notice));
                }
                ArbiterAction::AccessPoint(command) => {
                    (io.output)(ServiceOutput::AccessPoint(command));
                }
                ArbiterAction::CloseGate { .. }
                | ArbiterAction::Retune(_)
                | ArbiterAction::OpenGate(_) => {}
            }
        }
        while let Some(action) = self.station.poll_action(now) {
            if let StaAction::Notify(notice) = action {
                (io.output)(ServiceOutput::Notice(notice));
                if let StaNotice::Disconnected(_) = notice {
                    self.rates.disconnected();
                }
            }
        }
    }

    /// Take one event of the port; a received frame's buffer comes back
    /// for its recycle.
    fn event<M: RawMutex>(
        &mut self,
        now: Instant,
        event: LowerMacEvent<'id, B::Types>,
        io: &mut ServiceIo<'_, M, B::Caps>,
    ) -> Option<<B::Types as PhaseTypes<'id>>::RxBuffer> {
        match event {
            LowerMacEvent::Received { frame, meta } => {
                let bytes = frame.bytes();
                self.station.input(
                    now,
                    StaInput::Received {
                        frame: bytes,
                        meta,
                    },
                );
                if let Some(mut link) = self.station.link()
                    && let Some(msdus) = data::received(&mut link, bytes, &meta)
                {
                    for parts in msdus {
                        // A full queue drops the MSDU, as an Ethernet
                        // receiver does.
                        let Some(slot) = io.receive.try_send() else {
                            continue;
                        };
                        if let Ok(len) = parts.copy_to(&mut slot.frame) {
                            slot.len = len;
                            io.receive.send_done();
                        }
                    }
                }
                Some(frame)
            }
            LowerMacEvent::TxCompleted {
                ticket,
                buffers,
                completion,
                ..
            } => {
                self.completed(now, ticket, buffers, completion);
                None
            }
            LowerMacEvent::GateClosed(closed) => {
                self.arbiter.gate_closed(closed);
                None
            }
            LowerMacEvent::Configured(VifRole::Station) => {
                self.station.input(now, StaInput::Configured);
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
        }
    }

    /// An attempt's terminal: its ticket and buffer go back to the machine
    /// whose buffer class it carries.
    fn completed(
        &mut self,
        now: Instant,
        ticket: Slot<'id, <B::Types as PhaseTypes<'id>>::TicketKind>,
        buffers: <B::Types as PhaseTypes<'id>>::TxBuffers,
        completion: TxCompletion,
    ) {
        let mut ticket = Some(ticket);
        for buffer in buffers {
            let Some(owner) = ticket.take() else {
                unreachable!("the station and data send one MPDU an attempt");
            };
            match Buffer::<'id, B>::of(buffer) {
                Buffer::Small(buffer) => {
                    self.rates.completed(now, FrameRates::Control, &completion);
                    self.station.input(
                        now,
                        StaInput::TxDone {
                            ticket: owner,
                            buffer,
                            completion,
                        },
                    );
                }
                Buffer::Large(buffer) => {
                    self.rates.completed(now, FrameRates::Data, &completion);
                    let _ = self.data.input::<PortStaTokens<'id, B::Types, B::Small>>(
                        now,
                        DataInput::TxDone {
                            ticket: owner,
                            buffer,
                        },
                    );
                }
            }
        }
    }
}

/// What follows a station's action.
enum Follow<'id, B: StationPort<'id>> {
    Nothing,
    /// An arbiter's answer or a key, back to the station.
    Input(StaInput<'static, PortStaTokens<'id, B::Types, B::Small>, B::Caps>),
    /// The station associated: the rates start from its peer.
    Associated(Peer),
    /// The station left: the rates leave its peer.
    Disconnected,
}

/// Submit a frame of the station at the rates `rates` selects, under `key`
/// when it names one.
fn submit_station<'p, 'id: 'p, B, P, S>(
    phase: &mut P,
    rates: &S,
    key: Option<&<<B::Types as PhaseTypes<'id>>::StationKeys as KeyTypes<'id>>::PairwiseKey>,
    transmit: StaTransmit<PortStaTokens<'id, B::Types, B::Small>, B::Caps>,
) -> Result<(), ()>
where
    B: StationPort<'id>,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    S: RateSelection,
{
    let frame = match transmit.frame {
        StaFrame::Individual(frame) => {
            let plan = match transmit.plan {
                // A Null's retries are its plan's
                // ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130033259)).
                StaRatePlan::Null => rates
                    .plan(FrameRates::Control)
                    .limit(Levels::<B::Caps>::NULL),
                StaRatePlan::Management | StaRatePlan::Group => rates.plan(FrameRates::Control),
            };
            TxFrame::Individual {
                frame,
                plan: phase_plan(phase, plan),
            }
        }
        StaFrame::Group(frame) => {
            let plan = rates.plan(FrameRates::Probe);
            let Some(first) = plan.stages().next() else {
                unreachable!("a plan has a first stage");
            };
            let Some(rate) = phase.rate(first.rate) else {
                unreachable!("a rate selection is built for its port's capabilities");
            };
            TxFrame::Group { frame, rate }
        }
    };
    let coex = match transmit.class {
        StaFrameClass::Connection => Levels::<B::Caps>::CONNECTION,
        StaFrameClass::Normal => Levels::<B::Caps>::NORMAL,
    };
    let mut station = phase.station();
    let Ok(frame) = ClassFrame::<B::Small, B::Caps>::new(frame) else {
        unreachable!("a frame encoded into its buffer fits its class");
    };
    let frame = station.tx_buffer::<B::Small>(transmit.buffer, frame);
    station
        .submit(oer_ieee80211_lower_mac::Attempt {
            ticket: transmit.ticket,
            frame,
            body: None,
            access_category: WmmAccessCategory::Voice,
            protection: Protection::None,
            key: key.map(AttemptKey::Pairwise),
            power: Levels::<B::Caps>::POWER,
            coex,
        })
        .map_err(|_| ())
}

/// Carry out one action of the station; an arbiter's answer goes back to
/// it at once.
fn station_action<'p, 'id: 'p, B, P, M, S>(
    now: Instant,
    phase: &mut P,
    io: &mut ServiceIo<'_, M, B::Caps>,
    arbiter: &mut PortArbiter<'_, 'id, B>,
    rates: &S,
    action: StaAction<'_, PortStaTokens<'id, B::Types, B::Small>, B::Caps>,
) -> Result<Follow<'id, B>, ()>
where
    B: StationPort<'id>,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types>,
    M: RawMutex,
    S: RateSelection,
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
        StaAction::Configure(receive) => {
            phase.station().configure(receive);
            return Ok(Follow::Nothing);
        }
        StaAction::InstallPairwiseKey { slot, peer, key } => StaInput::PairwiseKeyInstalled(
            phase
                .station()
                .install_pairwise_key(slot, peer, Ccmp128Key(key)),
        ),
        StaAction::InstallGroupKey { slot, key_id, key } => StaInput::GroupKeyInstalled(
            phase
                .station()
                .install_group_key(slot, key_id, Ccmp128Key(key)),
        ),
        StaAction::RemovePairwiseKey(key) => {
            phase.station().remove_pairwise_key(key);
            return Ok(Follow::Nothing);
        }
        StaAction::RemoveGroupKey(key) => {
            phase.station().remove_group_key(key);
            return Ok(Follow::Nothing);
        }
        StaAction::Transmit(transmit) => {
            return submit_station::<B, P, S>(phase, rates, None, transmit).map(|()| Follow::Nothing);
        }
        StaAction::TransmitProtected { transmit, key } => {
            return submit_station::<B, P, S>(phase, rates, Some(key), transmit)
                .map(|()| Follow::Nothing);
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

/// A selected plan in the phase's rates.
fn phase_plan<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(
    phase: &P,
    plan: RetryPlan<PhyRate>,
) -> RetryPlan<PhaseRate<'p>> {
    match plan.try_map(|rate| phase.rate(rate).ok_or(rate)) {
        Ok(plan) => plan,
        Err(_) => unreachable!("a rate selection is built for its port's capabilities"),
    }
}

/// What serving a phase ended with.
enum Served<K> {
    Stop,
    Retune(SupportedChannel<K>),
    Poisoned,
}

/// A completed buffer, by the machine whose class it is.
enum Buffer<'id, B: StationPort<'id>> {
    Small(<B::Types as TxBufferPool<'id, B::Small>>::Buffer),
    Large(<B::Types as TxBufferPool<'id, B::Large>>::Buffer),
}

impl<'id, B: StationPort<'id>> Buffer<'id, B> {
    /// `buffer`'s class, a numbered one renewed.
    fn of(buffer: <B::Types as PhaseTypes<'id>>::AnyTxBuffer) -> Self {
        let buffer = match <B::Types as TxBufferPool<'id, B::Small>>::class(buffer) {
            Ok(buffer) => return Self::Small(buffer),
            Err(buffer) => buffer,
        };
        let buffer = match <B::Types as TxBufferPool<'id, B::Large>>::class(buffer) {
            Ok(buffer) => return Self::Large(buffer),
            Err(buffer) => buffer,
        };
        let buffer = match <B::Types as TxBufferPool<'id, B::Large>>::numbered(buffer) {
            Ok(buffer) => {
                return Self::Large(<B::Types as TxBufferPool<'id, B::Large>>::renew(buffer));
            }
            Err(buffer) => buffer,
        };
        match <B::Types as TxBufferPool<'id, B::Small>>::numbered(buffer) {
            Ok(buffer) => Self::Small(<B::Types as TxBufferPool<'id, B::Small>>::renew(buffer)),
            Err(_) => unreachable!("the service's buffers are of its two classes"),
        }
    }
}

#[cfg(test)]
mod tests;
