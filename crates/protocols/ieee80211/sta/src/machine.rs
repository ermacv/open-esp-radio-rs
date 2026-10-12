//! The station: one sans-IO machine for the whole lifecycle of an
//! infrastructure station, from its scan to its disconnect ([owner
//! decision on #563]).
//!
//! The machine takes the port's events, the channel arbiter's answers and
//! notices, its commands and time as [`StaInput`] values, and leaves what
//! the port and the arbiter must do as [`StaAction`] values, in their own
//! terms (`oer-ieee80211-lower-mac`, `oer-ieee80211-arbiter`): a thin
//! service loop feeds it and carries its actions out, and holds no logic
//! of its own:
//!
//! ```text
//! loop {
//!     select(phase.next_event(), timer(deadline), commands)
//!     station.input(now, input)
//!     while let Some(action) = station.poll_action(now) { carry it out }
//! }
//! ```
//!
//! # Tokens
//!
//! The machine is generic over its tokens ([`StaTokens`]): the transmit
//! tickets and buffers the composition hands it at its start, minted by
//! the port, and the arbiter's [`LeasePlace`]. A token leaves in an action
//! by value and comes back in an input, so what the machine holds is what
//! it may do, and a host test plays the port and the arbiter with tokens
//! of its own. The station encodes each frame straight into the memory of
//! its buffer token ([`TxBufferMemory`]).
//!
//! # Channels
//!
//! The station never changes the port's channel: it asks the arbiter for a
//! lease of each channel it needs ([`StaAction::RequestChannel`]), listens
//! and transmits there only between the lease's start and its end, and
//! gives it back. Beside an access point, its scan goes in absences. A
//! scan the application asked for ends with a partial result after
//! [`StaConfig::skipped_absences`] consecutive cancelled leases.
//!
//! # States
//!
//! Its state is an enum whose variants hold their own types; a transition
//! takes the state by value and returns the next one, so a value of one
//! state is never read in another. Each state names the action it owes,
//! so [`Station::poll_action`] hands it out and moves to the state that
//! awaits its end: there is no queue of actions to overflow.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225

use oer_ieee80211_arbiter::{
    Answer, CancelReason, ChannelRequest, Lease, LeasePlace, LeaveReason, Purpose, StationNotice,
    UpstreamFact,
};
use oer_ieee80211_lower_mac::{
    AckHeader, GroupAddress, HeaderSpec, Ieee80211Capabilities, IndividualAddress, MpduLength,
    ReceiveFilter, RxEvidence, RxMeta, SizedFrame, Station as StationRole, StationReceive,
    SupportedChannel, SupportedFilter, TxBufferMemory, TxCompletion, TxStatus,
};
use oer_ieee80211_mac::{
    management::ProbeRequest,
    scan::{ScanRecord, ScanTable},
    security::StaSecurityPolicy,
    sequence::SequenceNumber,
    station::{
        AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest, SelectedRsn,
        StaManagementFrame, StaManagementSubtype, StaSequenceCounter, association::PhyMode,
        select_association_rsn,
    },
};
use oer_time::{Duration, Instant};

use crate::join::{
    association::{StaAssociationEvent, StaAssociationPoll, StaAssociationRuntime},
    authentication::{StaAuthenticationEvent, StaAuthenticationRuntime},
};

/// The tokens a station holds: the port's, or a host test's.
pub trait StaTokens {
    /// A transmit ticket: a place in flight and a place for its terminal.
    type Ticket;
    /// A transmit buffer of the small class, for management frames: its
    /// memory, which the station encodes into.
    type SmallBuffer: TxBufferMemory;
}

/// What the application asks of the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCommand {
    /// Connect to the configured network.
    Connect,
    /// Leave the network and stay idle.
    Disconnect,
}

/// One input of the station.
pub enum StaInput<'f, T: StaTokens, K> {
    /// A command of the application.
    Command(StaCommand),
    /// A deadline the station named passed.
    Tick,
    /// A frame the station's interface received.
    Received {
        /// The MPDU, from its header to the end of its body.
        frame: &'f [u8],
        /// What the hardware observed of it.
        meta: RxMeta,
    },
    /// The terminal of a transmission: its ticket and buffer come back.
    TxDone {
        /// The transmission's ticket.
        ticket: T::Ticket,
        /// The transmission's buffer.
        buffer: T::SmallBuffer,
        /// How it ended.
        completion: TxCompletion,
    },
    /// The arbiter's answer to the station's request.
    Answer(Answer<K>),
    /// A notice of the arbiter.
    Arbiter(StationNotice<K>),
    /// The receive configuration the station set took effect.
    Configured,
}

/// One action of the station, which the service carries out at once.
pub enum StaAction<T: StaTokens, K> {
    /// Ask the arbiter for a channel, with the station's place; the service
    /// hands the answer back ([`StaInput::Answer`]) in the same step.
    RequestChannel(ChannelRequest<K>, LeasePlace),
    /// Give a lease back to the arbiter.
    ReleaseChannel(Lease<K>),
    /// Tell the arbiter a fact about the station's upstream.
    Upstream(UpstreamFact<K>),
    /// Set what the station's interface receives ([`StaInput::Configured`]
    /// follows).
    Configure(StationReceive<K>),
    /// Transmit one MPDU ([`StaInput::TxDone`] follows).
    Transmit(StaTransmit<T, K>),
    /// Tell the application what happened.
    Notify(StaNotice),
}

/// One MPDU to transmit: its ticket, the buffer the station encoded it in,
/// its kind and its rates.
pub struct StaTransmit<T: StaTokens, K> {
    /// Its ticket.
    pub ticket: T::Ticket,
    /// The buffer holding the frame, from its header on.
    pub buffer: T::SmallBuffer,
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<K>,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
}

/// The kind of a station frame, as the port's frame kinds take it.
pub enum StaFrame<K> {
    /// An individual frame that solicits an ACK.
    Individual(SizedFrame<AckHeader, K>),
    /// A group frame.
    Group(SizedFrame<HeaderSpec<GroupAddress>, K>),
}

/// The rates a station frame goes at, which the service turns into the
/// port's retry plan on the phase's channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRatePlan {
    /// An individual management frame: the BSS's lowest basic rate,
    /// retried by the station's management limit.
    Management,
    /// A group frame (a broadcast Probe Request): one transmission at the
    /// band's lowest rate.
    Group,
}

/// What the station tells the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaNotice {
    /// The station is connected to `bssid` on `channel`.
    Connected {
        /// The access point.
        bssid: [u8; 6],
        /// Its primary channel number.
        channel: u8,
    },
    /// The station left its network, or failed to join one.
    Disconnected(StaDisconnectReason),
}

/// Why the station is not connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectReason {
    /// The application asked it to leave.
    Requested,
    /// No access point of the network answered the scan; `partial` when
    /// the scan ended early after cancelled absences.
    NotFound {
        /// The scan ended early.
        partial: bool,
    },
    /// The arbiter did not let the station on its network's channel.
    Arbiter(LeaveReason),
    /// The access point refused or never answered the authentication.
    AuthenticationFailed,
    /// The access point refused or never answered the association.
    AssociationFailed,
    /// The access point deauthenticated or disassociated the station.
    Peer {
        /// The reason code it gave.
        reason_code: u16,
    },
    /// A frame the station had to send did not encode.
    Encode,
}

/// The longest SSID.
const SSID_CAPACITY: usize = 32;
/// The scan list's channels at most.
const CHANNEL_CAPACITY: usize = 16;
/// The BSSs a scan keeps at most.
const SCAN_CAPACITY: usize = 16;
/// The tokens of one kind the station holds at most.
const POOL_CAPACITY: usize = 8;
/// The reason a leaving station's Deauthentication gives: it leaves.
const DEAUTHENTICATION_LEAVING: u16 = 3;

/// What the station connects to and how.
pub struct StaConfig<K> {
    /// The interface's own address.
    pub address: [u8; 6],
    /// The network's SSID.
    pub ssid: [u8; SSID_CAPACITY],
    /// The SSID's length.
    pub ssid_len: u8,
    /// The channels the scan visits, in order.
    pub channels: [Option<SupportedChannel<K>>; CHANNEL_CAPACITY],
    /// How long the scan listens on each channel.
    pub dwell: Duration,
    /// The consecutive cancelled leases after which a scan ends with a
    /// partial result.
    pub skipped_absences: u8,
    /// The rates the Probe Request offers.
    pub supported_rates: [u8; 8],
    /// The capabilities the Association Request offers.
    pub association: &'static AssociationCapabilities,
    /// The PHY the station associates with.
    pub phy: PhyMode,
}

/// The station: its state and its tokens.
pub struct Station<T: StaTokens, K: Ieee80211Capabilities> {
    config: StaConfig<K>,
    state: State<K>,
    tickets: Pool<T::Ticket>,
    buffers: Pool<T::SmallBuffer>,
    /// The station's place for a lease, while it holds none.
    place: Option<LeasePlace>,
    /// The lease the station holds, or gave back and awaits the end of.
    lease: Held<K>,
    sequence: StaSequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
}

/// The station's lease.
enum Held<K> {
    /// None: the station holds its place.
    None,
    /// Held, from the arbiter's answer to its end.
    Lease(Lease<K>),
    /// Given back; its end's notice names it.
    Released(oer_ieee80211_arbiter::LeaseId),
}

impl<K> Held<K> {
    fn id(&self) -> Option<oer_ieee80211_arbiter::LeaseId> {
        match self {
            Self::None => None,
            Self::Lease(lease) => Some(lease.id()),
            Self::Released(id) => Some(*id),
        }
    }
}

/// The station's state; each variant holds its own type.
enum State<K> {
    /// Not connected and not trying.
    Idle,
    /// Visiting the scan list.
    Scan(Scan),
    /// Joining the chosen access point.
    Join(Join<K>),
    /// Connected.
    Connected(Connected<K>),
    /// Leaving the network.
    Leave(Leave),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason),
}

/// A visit of one scan channel.
struct Scan {
    visit: usize,
    /// Consecutive cancelled leases.
    skipped: u8,
    step: ScanStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanStep {
    /// Ask the arbiter for the visit's channel.
    NeedLease,
    /// The arbiter answers in the same step.
    AwaitAnswer,
    /// The lease is granted; the port moves at its start.
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    /// Listening until the lease's end.
    Listen,
}

/// The join of one access point.
struct Join<K> {
    access_point: ScanRecord,
    rsn: SelectedRsn,
    channel: SupportedChannel<K>,
    step: JoinStep,
}

enum JoinStep {
    NeedLease,
    AwaitAnswer,
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    Authenticate(Authenticate),
    Associate(StaAssociationRuntime),
    Report,
}

struct Authenticate {
    runtime: StaAuthenticationRuntime,
    stage: AuthStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthStage {
    NeedSend,
    AwaitSent { response_timeout: Duration },
    AwaitResponse { until: Instant },
}

/// The connected station.
struct Connected<K> {
    bssid: [u8; 6],
    channel: SupportedChannel<K>,
    /// The upstream fact the arbiter has yet to hear.
    report: Option<UpstreamFact<K>>,
}

/// The station leaving its network, and why.
struct Leave {
    reason: StaDisconnectReason,
    step: LeaveStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaveStep {
    NeedDeauthentication { bssid: [u8; 6] },
    AwaitDeauthentication,
    NeedConfigure,
    AwaitConfigured,
    NeedRelease,
    /// The lease ends with its notice, which gives the place back.
    AwaitEnd,
}

impl Leave {
    const fn new(reason: StaDisconnectReason, step: LeaveStep) -> Self {
        Self { reason, step }
    }
}

/// A bounded pool of tokens of one kind.
struct Pool<X> {
    items: [Option<X>; POOL_CAPACITY],
}

impl<X> Pool<X> {
    fn new(items: impl IntoIterator<Item = X>) -> Self {
        let mut pool = Self {
            items: [const { None }; POOL_CAPACITY],
        };
        for item in items {
            pool.put(item);
        }
        pool
    }

    fn take(&mut self) -> Option<X> {
        self.items.iter_mut().find_map(Option::take)
    }

    /// Keep `item`: a pool holds every token of its kind the station was
    /// granted, which [`POOL_CAPACITY`] bounds.
    fn put(&mut self, item: X) {
        let slot = self.items.iter_mut().find(|slot| slot.is_none());
        debug_assert!(
            slot.is_some(),
            "a station holds at most POOL_CAPACITY tokens of a kind"
        );
        if let Some(slot) = slot {
            *slot = Some(item);
        }
    }
}

/// The frame control of a Deauthentication and of a Disassociation.
const DEAUTHENTICATION: u16 = 0x00c0;
const DISASSOCIATION: u16 = 0x00a0;

/// The disconnect `frame` carries from `bssid`, if it is one.
fn peer_disconnect(frame: &[u8], bssid: [u8; 6]) -> Option<u16> {
    let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?) & 0x00fc;
    if control != DEAUTHENTICATION && control != DISASSOCIATION {
        return None;
    }
    if frame.get(10..16)? != bssid {
        return None;
    }
    Some(u16::from_le_bytes(frame.get(24..26)?.try_into().ok()?))
}

/// What ended in the station's world.
enum Done {
    Command(StaCommand),
    Tick,
    /// A transmission ended: acknowledged, or not.
    Tx {
        delivered: bool,
    },
    Configured,
}

/// An action a state owes.
enum Owed<T: StaTokens, K> {
    Request(ChannelRequest<K>),
    Release,
    Upstream(UpstreamFact<K>),
    Configure(StationReceive<K>),
    Transmit(StaTransmit<T, K>),
    Notify(StaNotice),
}

impl<T: StaTokens, K: Ieee80211Capabilities> Station<T, K> {
    /// An idle station of `config`, with the tickets and small buffers the
    /// composition granted it and the arbiter's place.
    pub fn new(
        config: StaConfig<K>,
        tickets: impl IntoIterator<Item = T::Ticket>,
        buffers: impl IntoIterator<Item = T::SmallBuffer>,
        place: LeasePlace,
    ) -> Self {
        Self {
            config,
            state: State::Idle,
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            place: Some(place),
            lease: Held::None,
            sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
            scan: ScanTable::new(),
        }
    }

    /// Whether the station is connected.
    pub const fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }

    fn ssid(&self) -> &[u8] {
        &self.config.ssid[..usize::from(self.config.ssid_len)]
    }

    fn channel(&self, visit: usize) -> Option<SupportedChannel<K>> {
        self.config.channels.get(visit).copied().flatten()
    }

    /// Take one input at `now`.
    pub fn input(&mut self, now: Instant, input: StaInput<'_, T, K>) {
        let done = match input {
            StaInput::TxDone {
                ticket,
                buffer,
                completion,
            } => {
                // A transmission's tokens come back whatever the state.
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Done::Tx {
                    delivered: matches!(completion.status, TxStatus::Success),
                }
            }
            StaInput::Command(command) => Done::Command(command),
            StaInput::Tick => Done::Tick,
            StaInput::Configured => Done::Configured,
            StaInput::Received { frame, meta } => {
                self.received(now, frame, meta);
                return;
            }
            StaInput::Answer(answer) => {
                self.answered(answer);
                return;
            }
            StaInput::Arbiter(notice) => {
                self.noticed(notice);
                return;
            }
        };
        let state = core::mem::replace(&mut self.state, State::Idle);
        self.state = self.step(state, now, done);
    }

    /// The arbiter's answer.
    fn answered(&mut self, answer: Answer<K>) {
        let state = core::mem::replace(&mut self.state, State::Idle);
        self.state = match answer {
            Answer::Lease(lease) => {
                self.lease = Held::Lease(lease);
                match state {
                    State::Scan(mut scan) => {
                        scan.step = ScanStep::AwaitStart;
                        State::Scan(scan)
                    }
                    State::Join(mut join) => {
                        join.step = JoinStep::AwaitStart;
                        State::Join(join)
                    }
                    state => state,
                }
            }
            Answer::Leave(reason, place) => {
                self.place = Some(place);
                State::Report(StaDisconnectReason::Arbiter(reason))
            }
        };
    }

    /// A notice of the arbiter.
    fn noticed(&mut self, notice: StationNotice<K>) {
        let state = core::mem::replace(&mut self.state, State::Idle);
        self.state = match notice {
            StationNotice::LeaseStarted(id) => {
                if self.lease.id() != Some(id) {
                    state
                } else {
                    match state {
                        State::Scan(mut scan) if scan.step == ScanStep::AwaitStart => {
                            scan.skipped = 0;
                            scan.step = ScanStep::NeedConfigure;
                            State::Scan(scan)
                        }
                        State::Join(mut join) if matches!(join.step, JoinStep::AwaitStart) => {
                            join.step = JoinStep::NeedConfigure;
                            State::Join(join)
                        }
                        state => state,
                    }
                }
            }
            StationNotice::LeaseEnded(id, place) | StationNotice::LeaseCancelled(id, _, place)
                if self.lease.id() != Some(id) =>
            {
                // A notice of an earlier lease: its place came back long
                // ago, so this one is the arbiter's mistake.
                debug_assert!(false, "a notice names a lease the station does not hold");
                drop(place);
                state
            }
            StationNotice::LeaseEnded(_, place) => {
                self.lease = Held::None;
                self.place = Some(place);
                self.lease_ended(state)
            }
            StationNotice::LeaseCancelled(_, reason, place) => {
                self.lease = Held::None;
                self.place = Some(place);
                self.lease_cancelled(state, reason)
            }
            StationNotice::Moved(channel) => match state {
                State::Connected(mut connected) => {
                    connected.channel = channel;
                    State::Connected(connected)
                }
                state => state,
            },
            StationNotice::Leave(reason) => match state {
                State::Connected(_) | State::Join(_) | State::Scan(_) => State::Leave(Leave::new(
                    StaDisconnectReason::Arbiter(reason),
                    LeaveStep::NeedConfigure,
                )),
                state => state,
            },
        };
    }

    /// The state after the lease ended at its own end.
    fn lease_ended(&mut self, state: State<K>) -> State<K> {
        match state {
            // The visit's listening is over: the next channel.
            State::Scan(scan) => self.visit(scan.visit + 1, 0),
            State::Leave(leave) => State::Report(leave.reason),
            // The connected lease ended without the station's release: the
            // port left; the station leaves.
            State::Connected(_) | State::Join(_) => State::Leave(Leave::new(
                StaDisconnectReason::Arbiter(LeaveReason::PortFailed),
                LeaveStep::NeedConfigure,
            )),
            state => state,
        }
    }

    /// The state after the lease was cancelled.
    fn lease_cancelled(&mut self, state: State<K>, reason: CancelReason) -> State<K> {
        match state {
            // A skipped absence: the same channel again, until too many in
            // a row end the scan with what it found.
            State::Scan(scan) => {
                let skipped = scan.skipped.saturating_add(1);
                if skipped >= self.config.skipped_absences {
                    self.select(true)
                } else {
                    State::Scan(Scan {
                        visit: scan.visit,
                        skipped,
                        step: ScanStep::NeedLease,
                    })
                }
            }
            State::Leave(leave) => State::Report(leave.reason),
            State::Join(_) | State::Connected(_) => {
                let _ = reason;
                State::Leave(Leave::new(
                    StaDisconnectReason::Arbiter(LeaveReason::PortFailed),
                    LeaveStep::NeedConfigure,
                ))
            }
            state => state,
        }
    }

    /// The state after `done` in `state`.
    fn step(&mut self, state: State<K>, now: Instant, done: Done) -> State<K> {
        match (state, done) {
            (State::Idle | State::Report(_), Done::Command(StaCommand::Connect)) => {
                self.scan.clear();
                self.visit(0, 0)
            }
            (State::Connected(connected), Done::Command(StaCommand::Disconnect)) => {
                State::Leave(Leave::new(
                    StaDisconnectReason::Requested,
                    LeaveStep::NeedDeauthentication {
                        bssid: connected.bssid,
                    },
                ))
            }
            (State::Scan(_) | State::Join(_), Done::Command(StaCommand::Disconnect)) => {
                State::Leave(Leave::new(
                    StaDisconnectReason::Requested,
                    LeaveStep::NeedConfigure,
                ))
            }
            (State::Scan(mut scan), Done::Configured) if scan.step == ScanStep::AwaitConfigured => {
                scan.step = ScanStep::NeedProbe;
                State::Scan(scan)
            }
            (State::Join(join), done) => self.join_step(join, now, done),
            (State::Leave(Leave { reason, step }), done) => match (step, done) {
                (LeaveStep::AwaitDeauthentication, Done::Tx { .. }) => {
                    State::Leave(Leave::new(reason, LeaveStep::NeedConfigure))
                }
                (LeaveStep::AwaitConfigured, Done::Configured) => {
                    State::Leave(Leave::new(reason, LeaveStep::NeedRelease))
                }
                (step, _) => State::Leave(Leave::new(reason, step)),
            },
            (state, _) => state,
        }
    }

    /// The scan's visit of the channel at `visit`, or its end.
    fn visit(&mut self, visit: usize, skipped: u8) -> State<K> {
        match self.channel(visit) {
            Some(_) => State::Scan(Scan {
                visit,
                skipped,
                step: ScanStep::NeedLease,
            }),
            None => self.select(false),
        }
    }

    /// The join of the best access point of the network the scan found.
    fn select(&mut self, partial: bool) -> State<K> {
        let not_found = State::Report(StaDisconnectReason::NotFound { partial });
        let ssid = self.ssid();
        let Some(access_point) = self
            .scan
            .records()
            .iter()
            .filter(|record| record.ssid_bytes() == ssid)
            .filter(|record| select_association_rsn(record, StaSecurityPolicy::Open).is_ok())
            .max_by_key(|record| record.rssi)
            .copied()
        else {
            return not_found;
        };
        let Ok(rsn) = select_association_rsn(&access_point, StaSecurityPolicy::Open) else {
            return not_found;
        };
        let Some(channel) = self
            .config
            .channels
            .iter()
            .flatten()
            .copied()
            .find(|channel| channel.channel().number() == access_point.channel)
        else {
            return not_found;
        };
        State::Join(Join {
            access_point,
            rsn,
            channel,
            step: JoinStep::NeedLease,
        })
    }

    fn join_step(&mut self, join: Join<K>, now: Instant, done: Done) -> State<K> {
        let Join {
            access_point,
            rsn,
            channel,
            step,
        } = join;
        let step = match (step, done) {
            (JoinStep::AwaitConfigured, Done::Configured) => JoinStep::Authenticate(Authenticate {
                runtime: StaAuthenticationRuntime::new(self.config.address, access_point.bssid),
                stage: AuthStage::NeedSend,
            }),
            (JoinStep::Authenticate(mut auth), done) => {
                auth.stage = match (auth.stage, done) {
                    (AuthStage::AwaitSent { response_timeout }, Done::Tx { delivered: true }) => {
                        AuthStage::AwaitResponse {
                            until: now.checked_add(response_timeout).unwrap_or(now),
                        }
                    }
                    // Unacknowledged or unanswered: as a timeout.
                    (AuthStage::AwaitSent { .. }, Done::Tx { delivered: false }) => {
                        match self.auth_timed_out(&mut auth) {
                            Ok(stage) => stage,
                            Err(state) => return state,
                        }
                    }
                    (AuthStage::AwaitResponse { until }, Done::Tick) if now >= until => {
                        match self.auth_timed_out(&mut auth) {
                            Ok(stage) => stage,
                            Err(state) => return state,
                        }
                    }
                    (stage, _) => stage,
                };
                JoinStep::Authenticate(auth)
            }
            (step, _) => step,
        };
        State::Join(Join {
            access_point,
            rsn,
            channel,
            step,
        })
    }

    /// The authentication's next stage after a timeout, or the report of
    /// its failure.
    fn auth_timed_out(&mut self, auth: &mut Authenticate) -> Result<AuthStage, State<K>> {
        match auth.runtime.response_timed_out() {
            Ok(StaAuthenticationEvent::Retry { .. }) => Ok(AuthStage::NeedSend),
            _ => Err(self.leave_joined(StaDisconnectReason::AuthenticationFailed)),
        }
    }

    /// Leave the access point the join reached: receive off, the lease
    /// back.
    fn leave_joined(&mut self, reason: StaDisconnectReason) -> State<K> {
        State::Leave(Leave::new(reason, LeaveStep::NeedConfigure))
    }

    /// Take a received frame.
    fn received(&mut self, now: Instant, frame: &[u8], meta: RxMeta) {
        let rssi = match meta.rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
            RxEvidence::Unavailable => i8::MIN,
        };
        let state = core::mem::replace(&mut self.state, State::Idle);
        self.state = match state {
            State::Scan(scan) if scan.step == ScanStep::Listen || scan.step == ScanStep::NeedProbe => {
                let channel = self
                    .channel(scan.visit)
                    .map_or(0, |channel| channel.channel().number());
                self.scan.observe_management(frame, channel, rssi, Some(now));
                State::Scan(scan)
            }
            State::Join(mut join) => match &mut join.step {
                JoinStep::Authenticate(auth) => {
                    let _ = auth.runtime.observe_received_frame();
                    match auth.runtime.observe_management_frame(frame) {
                        Ok(StaAuthenticationEvent::Authenticated { .. }) => {
                            join.step = JoinStep::Associate(StaAssociationRuntime::new(
                                self.config.address,
                                join.access_point.bssid,
                                oer_ieee80211_mac::security::LinkProtection::Open,
                            ));
                            State::Join(join)
                        }
                        Ok(StaAuthenticationEvent::Retry { .. }) => {
                            auth.stage = AuthStage::NeedSend;
                            State::Join(join)
                        }
                        Ok(StaAuthenticationEvent::Failed { .. }) | Err(_) => {
                            self.leave_joined(StaDisconnectReason::AuthenticationFailed)
                        }
                        Ok(StaAuthenticationEvent::Irrelevant) => State::Join(join),
                    }
                }
                JoinStep::Associate(runtime) => {
                    let _ = runtime.observe_received_frame();
                    match runtime.observe_management_frame(frame, now) {
                        Ok(StaAssociationEvent::Associated { .. }) => {
                            join.step = JoinStep::Report;
                            State::Join(join)
                        }
                        Ok(StaAssociationEvent::Failed { .. }) | Err(_) => {
                            self.leave_joined(StaDisconnectReason::AssociationFailed)
                        }
                        Ok(StaAssociationEvent::Irrelevant) => State::Join(join),
                    }
                }
                _ => State::Join(join),
            },
            State::Connected(connected) => match peer_disconnect(frame, connected.bssid) {
                Some(reason_code) => State::Leave(Leave::new(
                    StaDisconnectReason::Peer { reason_code },
                    LeaveStep::NeedConfigure,
                )),
                None => State::Connected(connected),
            },
            state => state,
        };
    }

    /// The next action to carry out at `now`, if any. The service takes
    /// every action before it waits again.
    pub fn poll_action(&mut self, now: Instant) -> Option<StaAction<T, K>> {
        let state = core::mem::replace(&mut self.state, State::Idle);
        let (state, owed) = self.owe(state, now);
        self.state = state;
        Some(match owed? {
            Owed::Request(request) => {
                let place = self.place.take().expect("a request holds the place");
                StaAction::RequestChannel(request, place)
            }
            Owed::Release => match core::mem::replace(&mut self.lease, Held::None) {
                Held::Lease(lease) => {
                    self.lease = Held::Released(lease.id());
                    StaAction::ReleaseChannel(lease)
                }
                held => {
                    self.lease = held;
                    unreachable!("a release holds the lease")
                }
            },
            Owed::Upstream(fact) => StaAction::Upstream(fact),
            Owed::Configure(receive) => StaAction::Configure(receive),
            Owed::Transmit(transmit) => StaAction::Transmit(transmit),
            Owed::Notify(notice) => StaAction::Notify(notice),
        })
    }

    /// The action `state` owes, and the state that awaits its end.
    fn owe(&mut self, state: State<K>, now: Instant) -> (State<K>, Option<Owed<T, K>>) {
        match state {
            State::Report(reason) => (
                State::Idle,
                Some(Owed::Notify(StaNotice::Disconnected(reason))),
            ),
            State::Scan(Scan {
                visit,
                skipped,
                step,
            }) => {
                let (step, owed) = match step {
                    ScanStep::NeedLease if self.place.is_some() => {
                        let channel = self.channel(visit).expect("a visit names a channel");
                        (
                            ScanStep::AwaitAnswer,
                            Some(Owed::Request(ChannelRequest {
                                channel,
                                purpose: Purpose::Scan {
                                    dwell: self.config.dwell,
                                },
                                announces_absence: false,
                            })),
                        )
                    }
                    ScanStep::NeedConfigure => (
                        ScanStep::AwaitConfigured,
                        Some(Owed::Configure(StationReceive::OtherBssManagement)),
                    ),
                    // No token or no encoding: listen passively.
                    ScanStep::NeedProbe => (
                        ScanStep::Listen,
                        self.encode_probe().map(Owed::Transmit),
                    ),
                    step => (step, None),
                };
                (
                    State::Scan(Scan {
                        visit,
                        skipped,
                        step,
                    }),
                    owed,
                )
            }
            State::Join(join) => self.owe_join(join, now),
            State::Connected(mut connected) => {
                let owed = connected.report.take().map(Owed::Upstream);
                (State::Connected(connected), owed)
            }
            State::Leave(Leave { reason, step }) => {
                let (step, owed) = match step {
                    LeaveStep::NeedDeauthentication { bssid } => match self.encode_management(
                        StaManagementSubtype::Deauthentication,
                        bssid,
                        &DEAUTHENTICATION_LEAVING.to_le_bytes(),
                    ) {
                        Some(transmit) => (
                            LeaveStep::AwaitDeauthentication,
                            Some(Owed::Transmit(transmit)),
                        ),
                        None => (LeaveStep::NeedConfigure, None),
                    },
                    LeaveStep::NeedConfigure => (
                        LeaveStep::AwaitConfigured,
                        Some(Owed::Configure(StationReceive::Off)),
                    ),
                    // No lease to give back: done.
                    LeaveStep::NeedRelease if !matches!(self.lease, Held::Lease(_)) => {
                        return (State::Report(reason), None);
                    }
                    LeaveStep::NeedRelease => (LeaveStep::AwaitEnd, Some(Owed::Release)),
                    step => (step, None),
                };
                (State::Leave(Leave::new(reason, step)), owed)
            }
            state => (state, None),
        }
    }

    fn owe_join(&mut self, join: Join<K>, now: Instant) -> (State<K>, Option<Owed<T, K>>) {
        let Join {
            access_point,
            rsn,
            channel,
            step,
        } = join;
        let (step, owed) = match step {
            JoinStep::NeedLease if self.place.is_some() => (
                JoinStep::AwaitAnswer,
                Some(Owed::Request(ChannelRequest {
                    channel,
                    purpose: Purpose::Connect,
                    announces_absence: false,
                })),
            ),
            JoinStep::NeedConfigure => {
                let receive = SupportedFilter::<K, StationRole>::new(ReceiveFilter::BSS_MEMBER)
                    .and_then(|filter| {
                        Some(StationReceive::Bss {
                            bssid: IndividualAddress::new(access_point.bssid)?,
                            filter,
                        })
                    });
                match receive {
                    Some(receive) => (JoinStep::AwaitConfigured, Some(Owed::Configure(receive))),
                    None => return (self.leave_joined(StaDisconnectReason::Encode), None),
                }
            }
            JoinStep::Authenticate(mut auth) if auth.stage == AuthStage::NeedSend => {
                let Ok(attempt) = auth.runtime.begin_attempt(&mut self.sequence) else {
                    return (
                        self.leave_joined(StaDisconnectReason::AuthenticationFailed),
                        None,
                    );
                };
                let encoded = self.encode_individual(|frame| {
                    OpenAuthenticationRequest {
                        source: access_point.bssid,
                        bssid: access_point.bssid,
                        sequence_number: attempt.sequence_number,
                    }
                    .encode(frame)
                    .ok()
                }, self.config.address);
                match encoded {
                    Ok(transmit) => {
                        auth.stage = AuthStage::AwaitSent {
                            response_timeout: attempt.response_timeout,
                        };
                        (JoinStep::Authenticate(auth), Some(Owed::Transmit(transmit)))
                    }
                    // No token now: the next poll tries again.
                    Err(Unsent::NoTokens) => (JoinStep::Authenticate(auth), None),
                    Err(Unsent::Encode) => {
                        return (self.leave_joined(StaDisconnectReason::Encode), None);
                    }
                }
            }
            JoinStep::Associate(mut runtime) => match runtime.poll(now, &mut self.sequence) {
                Ok(StaAssociationPoll::Transmit(attempt)) => {
                    let capabilities = self.config.association;
                    let phy = self.config.phy;
                    let address = self.config.address;
                    let encoded = self.encode_individual(|frame| {
                        AssociationRequest {
                            source: address,
                            access_point: &access_point,
                            sequence_number: attempt.sequence_number,
                            listen_interval: 10,
                            phy,
                            security: &rsn,
                            power_capability: None,
                            he_ul_mu_power: None,
                        }
                        .encode(frame, capabilities)
                        .ok()
                    }, address);
                    match encoded {
                        Ok(transmit) => (JoinStep::Associate(runtime), Some(Owed::Transmit(transmit))),
                        Err(Unsent::NoTokens) => (JoinStep::Associate(runtime), None),
                        Err(Unsent::Encode) => {
                            return (self.leave_joined(StaDisconnectReason::Encode), None);
                        }
                    }
                }
                Ok(StaAssociationPoll::Idle) => (JoinStep::Associate(runtime), None),
                Ok(StaAssociationPoll::Failed { .. }) | Err(_) => {
                    return (self.leave_joined(StaDisconnectReason::AssociationFailed), None);
                }
            },
            JoinStep::Report => {
                let bssid = access_point.bssid;
                return (
                    State::Connected(Connected {
                        bssid,
                        channel,
                        report: Some(UpstreamFact::Connected(channel)),
                    }),
                    Some(Owed::Notify(StaNotice::Connected {
                        bssid,
                        channel: channel.channel().number(),
                    })),
                );
            }
            step => (step, None),
        };
        (
            State::Join(Join {
                access_point,
                rsn,
                channel,
                step,
            }),
            owed,
        )
    }

    /// A ticket and a small buffer, if the station holds both.
    fn tokens(&mut self) -> Option<(T::Ticket, T::SmallBuffer)> {
        let ticket = self.tickets.take()?;
        match self.buffers.take() {
            Some(buffer) => Some((ticket, buffer)),
            None => {
                self.tickets.put(ticket);
                None
            }
        }
    }

    /// The frame `encode` writes into a buffer, as an individual frame
    /// from `source`.
    fn encode_individual(
        &mut self,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
        source: [u8; 6],
    ) -> Result<StaTransmit<T, K>, Unsent> {
        let _ = source;
        let (ticket, mut buffer) = self.tokens().ok_or(Unsent::NoTokens)?;
        let frame = encode(buffer.bytes_mut()).and_then(|len| {
            let header =
                HeaderSpec::decode(&buffer.bytes_mut()[..len], IndividualAddress::new)?;
            SizedFrame::new(AckHeader::new(header)?, MpduLength::new(len)?)
        });
        match frame {
            Some(frame) => Ok(StaTransmit {
                ticket,
                buffer,
                frame: StaFrame::Individual(frame),
                plan: StaRatePlan::Management,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(Unsent::Encode)
            }
        }
    }

    /// A broadcast Probe Request in a buffer, if the station holds tokens
    /// and it encodes.
    fn encode_probe(&mut self) -> Option<StaTransmit<T, K>> {
        let (ticket, mut buffer) = self.tokens()?;
        let sequence_number = self.sequence.take();
        let rates = self.config.supported_rates;
        let len = ProbeRequest {
            destination: [0xff; 6],
            source: self.config.address,
            bssid: [0xff; 6],
            sequence_number,
            ssid: &self.config.ssid[..usize::from(self.config.ssid_len)],
            supported_rates: &rates,
        }
        .encode(buffer.bytes_mut())
        .ok();
        let frame = len.and_then(|len| {
            let header = HeaderSpec::decode(&buffer.bytes_mut()[..len], GroupAddress::new)?;
            SizedFrame::new(header, MpduLength::new(len)?)
        });
        match frame {
            Some(frame) => Some(StaTransmit {
                ticket,
                buffer,
                frame: StaFrame::Group(frame),
                plan: StaRatePlan::Group,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                None
            }
        }
    }

    /// A management frame of `subtype` to `bssid` in a buffer.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: [u8; 6],
        body: &[u8],
    ) -> Option<StaTransmit<T, K>> {
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        self.encode_individual(
            |frame| {
                StaManagementFrame {
                    subtype,
                    source: address,
                    bssid,
                    sequence_number,
                    body,
                }
                .encode(frame)
                .ok()
            },
            address,
        )
        .ok()
    }

    /// When the station next needs a [`StaInput::Tick`], even without an
    /// event.
    pub fn poll_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Join(Join {
                step:
                    JoinStep::Authenticate(Authenticate {
                        stage: AuthStage::AwaitResponse { until },
                        ..
                    }),
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Associate(runtime),
                ..
            }) => runtime.next_deadline(),
            _ => None,
        }
    }
}

/// Why a frame was not handed out.
enum Unsent {
    /// The station holds no ticket or buffer now.
    NoTokens,
    /// The frame did not encode.
    Encode,
}

#[cfg(test)]
mod tests;
