//! The station: one sans-IO machine for the whole lifecycle of an
//! infrastructure station, from its scan to its disconnect ([owner
//! decision on #563]).
//!
//! The machine takes the port's events, its commands and time as
//! [`StaInput`] values, and leaves what the port must do as [`StaAction`]
//! values, in the port's own terms (`oer-ieee80211-lower-mac`): a thin
//! service loop over the port feeds it and carries its actions out, and
//! holds no logic of its own:
//!
//! ```text
//! loop {
//!     select(phase.next_event(), timer(station.poll_deadline()), commands)
//!     station.input(now, input)
//!     while let Some(action) = station.poll_action(now) { carry it out }
//! }
//! ```
//!
//! # Tokens
//!
//! The machine is generic over its tokens ([`StaTokens`]): the transmit
//! tickets and buffers and the key slots the composition hands it at its
//! start, minted by the port. A token leaves in an action by value and
//! comes back in an input, so what the machine holds is what it may do,
//! and a host test plays the port with tokens of its own.
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

use oer_ieee80211_lower_mac::{
    Ieee80211Capabilities, ReceiveFilter, RxEvidence, RxMeta, Station as StationRole, StationReceive,
    SupportedChannel, SupportedFilter, TxCompletion, TxStatus,
};
use oer_ieee80211_mac::{
    management::ProbeRequest,
    scan::{ScanRecord, ScanTable},
    security::StaSecurityPolicy,
    sequence::SequenceNumber,
    station::{
        AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest,
        association::PhyMode,
        SelectedRsn, StaManagementFrame, StaManagementSubtype, StaSequenceCounter,
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
    /// A transmit buffer of the small class, for management frames.
    type SmallBuffer;
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
    /// The port now runs on the channel the station asked for.
    ChannelGranted(SupportedChannel<K>),
    /// The channel the station asked for is not granted: the radio's
    /// arbiter keeps another.
    ChannelRefused,
    /// The receive configuration the station set took effect.
    Configured,
}

/// One action of the station, which the service carries out at once.
pub enum StaAction<'a, T: StaTokens, K> {
    /// Ask for the port to run on `channel`: in a station beside an access
    /// point, the radio's channel arbiter decides
    /// ([`StaInput::ChannelGranted`], [`StaInput::ChannelRefused`]).
    RequestChannel(SupportedChannel<K>),
    /// Set what the station's interface receives ([`StaInput::Configured`]
    /// follows).
    Configure(StationReceive<K>),
    /// Transmit one MPDU ([`StaInput::TxDone`] follows).
    Transmit(StaTransmit<'a, T>),
    /// Tell the application what happened.
    Notify(StaNotice),
}

/// One MPDU to transmit: its ticket and buffer, its octets and how it goes.
pub struct StaTransmit<'a, T: StaTokens> {
    /// Its ticket.
    pub ticket: T::Ticket,
    /// The buffer it goes in.
    pub buffer: T::SmallBuffer,
    /// The MPDU as its encoder wrote it, from its header to the end of its
    /// body, without the FCS.
    pub frame: &'a [u8],
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
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
    /// No access point of the network answered the scan.
    NotFound,
    /// The arbiter refused the network's channel.
    ChannelRefused,
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
/// The longest frame the station encodes itself.
const FRAME_CAPACITY: usize = 512;
/// The tokens of one kind the station holds at most.
const POOL_CAPACITY: usize = 8;
/// The reason a leaving station's Deauthentication gives: it leaves.
const DEAUTHENTICATION_LEAVING: u16 = 3;
/// How long the station waits for a probe's terminal before it dwells
/// without it: the port's terminal arrives well within it.
const TX_TERMINAL_TIMEOUT: Duration = Duration::from_millis(250);

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
    sequence: StaSequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
    frame: [u8; FRAME_CAPACITY],
}

/// The station's state; each variant holds its own type.
enum State<K> {
    /// Not connected and not trying.
    Idle,
    /// Visiting the scan list.
    Scan(Scan<K>),
    /// Joining the chosen access point.
    Join(Join<K>),
    /// Connected.
    Connected(Connected),
    /// Leaving the network.
    Leave(Leave),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason),
}

/// A visit of one scan channel.
struct Scan<K> {
    visit: usize,
    step: ScanStep<K>,
}

enum ScanStep<K> {
    NeedChannel(SupportedChannel<K>),
    AwaitChannel,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    AwaitProbe { until: Instant },
    Dwell { until: Instant },
}

/// The join of one access point.
struct Join<K> {
    access_point: ScanRecord,
    rsn: SelectedRsn,
    step: JoinStep<K>,
}

enum JoinStep<K> {
    NeedChannel(SupportedChannel<K>),
    AwaitChannel,
    NeedConfigure,
    AwaitConfigured,
    Authenticate(Authenticate),
    Associate(Associate),
    Report { channel: u8 },
}

struct Authenticate {
    runtime: StaAuthenticationRuntime,
    stage: AuthStage,
}

enum AuthStage {
    NeedSend,
    AwaitSent { response_timeout: Duration },
    AwaitResponse { until: Instant },
}

struct Associate {
    runtime: StaAssociationRuntime,
    pending: Option<SequenceNumber>,
}

/// The connected station.
struct Connected {
    bssid: [u8; 6],
}

/// The station leaving its network, and why.
struct Leave {
    reason: StaDisconnectReason,
    step: LeaveStep,
}

enum LeaveStep {
    NeedDeauthentication { bssid: [u8; 6] },
    AwaitDeauthentication { until: Instant },
    NeedConfigure,
    AwaitConfigured,
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
        debug_assert!(slot.is_some(), "a station holds at most POOL_CAPACITY tokens of a kind");
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

impl<T: StaTokens, K: Ieee80211Capabilities> Station<T, K> {
    /// An idle station of `config`, with the tickets and small buffers the
    /// composition granted it.
    pub fn new(
        config: StaConfig<K>,
        tickets: impl IntoIterator<Item = T::Ticket>,
        buffers: impl IntoIterator<Item = T::SmallBuffer>,
    ) -> Self {
        Self {
            config,
            state: State::Idle,
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
            scan: ScanTable::new(),
            frame: [0; FRAME_CAPACITY],
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
        // A transmission's tokens come back whatever the state.
        let input = match input {
            StaInput::TxDone {
                ticket,
                buffer,
                completion,
            } => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Some(Done::Tx {
                    delivered: matches!(completion.status, TxStatus::Success),
                })
            }
            StaInput::Command(command) => Some(Done::Command(command)),
            StaInput::Tick => Some(Done::Tick),
            StaInput::Received { frame, meta } => {
                self.received(now, frame, meta);
                None
            }
            StaInput::ChannelGranted(_) => Some(Done::ChannelGranted),
            StaInput::ChannelRefused => Some(Done::ChannelRefused),
            StaInput::Configured => Some(Done::Configured),
        };
        if let Some(done) = input {
            let state = core::mem::replace(&mut self.state, State::Idle);
            self.state = self.step(state, now, done);
        }
    }

    /// The state after `done` in `state`.
    fn step(&mut self, state: State<K>, now: Instant, done: Done) -> State<K> {
        match (state, done) {
            (State::Idle | State::Report(_), Done::Command(StaCommand::Connect)) => {
                self.scan.clear();
                self.visit(0)
            }
            (State::Connected(connected), Done::Command(StaCommand::Disconnect)) => {
                State::Leave(Leave::new(
                    StaDisconnectReason::Requested,
                    LeaveStep::NeedDeauthentication {
                        bssid: connected.bssid,
                    },
                ))
            }
            (State::Scan(_) | State::Join(_), Done::Command(StaCommand::Disconnect)) => State::Leave(
                Leave::new(StaDisconnectReason::Requested, LeaveStep::NeedConfigure),
            ),
            (State::Scan(scan), done) => self.scan_step(scan, now, done),
            (State::Join(join), done) => self.join_step(join, now, done),
            (State::Leave(Leave { reason, step }), done) => match (step, done) {
                (LeaveStep::AwaitDeauthentication { .. }, Done::Tx { .. }) => {
                    State::Leave(Leave::new(reason, LeaveStep::NeedConfigure))
                }
                (LeaveStep::AwaitDeauthentication { until }, Done::Tick) if now >= until => {
                    State::Leave(Leave::new(reason, LeaveStep::NeedConfigure))
                }
                (LeaveStep::AwaitConfigured, Done::Configured) => State::Report(reason),
                (step, _) => State::Leave(Leave::new(reason, step)),
            },
            (state, _) => state,
        }
    }

    /// The scan's visit of the channel at `visit`, or its end.
    fn visit(&mut self, visit: usize) -> State<K> {
        match self.channel(visit) {
            Some(channel) => State::Scan(Scan {
                visit,
                step: ScanStep::NeedChannel(channel),
            }),
            None => self.select(),
        }
    }

    /// The join of the best access point of the network the scan found.
    fn select(&mut self) -> State<K> {
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
            return State::Report(StaDisconnectReason::NotFound);
        };
        let Ok(rsn) = select_association_rsn(&access_point, StaSecurityPolicy::Open) else {
            return State::Report(StaDisconnectReason::NotFound);
        };
        let channel = self.config.channels.iter().flatten().copied().find(|channel| {
            channel.channel().number() == access_point.channel
        });
        let Some(channel) = channel else {
            return State::Report(StaDisconnectReason::NotFound);
        };
        State::Join(Join {
            access_point,
            rsn,
            step: JoinStep::NeedChannel(channel),
        })
    }

    fn scan_step(&mut self, scan: Scan<K>, now: Instant, done: Done) -> State<K> {
        let Scan { visit, step } = scan;
        let step = match (step, done) {
            (ScanStep::AwaitChannel, Done::ChannelGranted) => ScanStep::NeedConfigure,
            // The arbiter keeps another channel: skip this one.
            (ScanStep::AwaitChannel, Done::ChannelRefused) => return self.visit(visit + 1),
            (ScanStep::AwaitConfigured, Done::Configured) => ScanStep::NeedProbe,
            (ScanStep::AwaitProbe { .. }, Done::Tx { .. }) => ScanStep::Dwell {
                until: now.checked_add(self.config.dwell).unwrap_or(now),
            },
            (ScanStep::AwaitProbe { until }, Done::Tick) if now >= until => ScanStep::Dwell {
                until: now.checked_add(self.config.dwell).unwrap_or(now),
            },
            (ScanStep::Dwell { until }, Done::Tick) if now >= until => {
                return self.visit(visit + 1);
            }
            (step, _) => step,
        };
        State::Scan(Scan { visit, step })
    }

    fn join_step(&mut self, join: Join<K>, now: Instant, done: Done) -> State<K> {
        let Join {
            access_point,
            rsn,
            step,
        } = join;
        let step = match (step, done) {
            (JoinStep::AwaitChannel, Done::ChannelGranted) => JoinStep::NeedConfigure,
            (JoinStep::AwaitChannel, Done::ChannelRefused) => {
                return State::Report(StaDisconnectReason::ChannelRefused);
            }
            (JoinStep::AwaitConfigured, Done::Configured) => {
                JoinStep::Authenticate(Authenticate {
                    runtime: StaAuthenticationRuntime::new(
                        self.config.address,
                        access_point.bssid,
                    ),
                    stage: AuthStage::NeedSend,
                })
            }
            (JoinStep::Authenticate(mut auth), done) => {
                auth.stage = match (auth.stage, done) {
                    (AuthStage::AwaitSent { response_timeout }, Done::Tx { delivered: true }) => {
                        AuthStage::AwaitResponse {
                            until: now.checked_add(response_timeout).unwrap_or(now),
                        }
                    }
                    // The access point never acknowledged the request: as
                    // unanswered.
                    (AuthStage::AwaitSent { .. }, Done::Tx { delivered: false }) => {
                        match auth.runtime.response_timed_out() {
                            Ok(StaAuthenticationEvent::Retry { .. }) => AuthStage::NeedSend,
                            _ => return State::Report(StaDisconnectReason::AuthenticationFailed),
                        }
                    }
                    (AuthStage::AwaitResponse { until }, Done::Tick) if now >= until => {
                        match auth.runtime.response_timed_out() {
                            Ok(StaAuthenticationEvent::Retry { .. }) => AuthStage::NeedSend,
                            _ => return State::Report(StaDisconnectReason::AuthenticationFailed),
                        }
                    }
                    (stage, _) => stage,
                };
                JoinStep::Authenticate(auth)
            }
            (JoinStep::Associate(associate), Done::Tx { .. }) => JoinStep::Associate(associate),
            (step, _) => step,
        };
        State::Join(Join {
            access_point,
            rsn,
            step,
        })
    }

    /// Take a received frame.
    fn received(&mut self, now: Instant, frame: &[u8], meta: RxMeta) {
        let rssi = match meta.rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
            RxEvidence::Unavailable => i8::MIN,
        };
        let state = core::mem::replace(&mut self.state, State::Idle);
        self.state = match state {
            State::Scan(scan) => {
                let channel = self
                    .channel(scan.visit)
                    .map_or(0, |channel| channel.channel().number());
                self.scan
                    .observe_management(frame, channel, rssi, Some(now));
                State::Scan(scan)
            }
            State::Join(mut join) => {
                let next = match &mut join.step {
                    JoinStep::Authenticate(auth) => {
                        let _ = auth.runtime.observe_received_frame();
                        match auth.runtime.observe_management_frame(frame) {
                            Ok(StaAuthenticationEvent::Authenticated { .. }) => {
                                Some(JoinStep::Associate(Associate {
                                    runtime: StaAssociationRuntime::new(
                                        self.config.address,
                                        join.access_point.bssid,
                                        oer_ieee80211_mac::security::LinkProtection::Open,
                                    ),
                                    pending: None,
                                }))
                            }
                            Ok(StaAuthenticationEvent::Retry { .. }) => {
                                auth.stage = AuthStage::NeedSend;
                                None
                            }
                            Ok(StaAuthenticationEvent::Failed { .. }) | Err(_) => {
                                self.state = State::Report(StaDisconnectReason::AuthenticationFailed);
                                return;
                            }
                            Ok(StaAuthenticationEvent::Irrelevant) => None,
                        }
                    }
                    JoinStep::Associate(associate) => {
                        let _ = associate.runtime.observe_received_frame();
                        match associate.runtime.observe_management_frame(frame, now) {
                            Ok(StaAssociationEvent::Associated { .. }) => Some(JoinStep::Report {
                                channel: join.access_point.channel,
                            }),
                            Ok(StaAssociationEvent::Failed { .. }) | Err(_) => {
                                self.state = State::Report(StaDisconnectReason::AssociationFailed);
                                return;
                            }
                            Ok(StaAssociationEvent::Irrelevant) => None,
                        }
                    }
                    _ => None,
                };
                if let Some(next) = next {
                    join.step = next;
                }
                State::Join(join)
            }
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
    pub fn poll_action(&mut self, now: Instant) -> Option<StaAction<'_, T, K>> {
        let state = core::mem::replace(&mut self.state, State::Idle);
        let (state, owed) = self.owe(state, now);
        self.state = state;
        Some(match owed? {
            Owed::RequestChannel(channel) => StaAction::RequestChannel(channel),
            Owed::Configure(receive) => StaAction::Configure(receive),
            Owed::Notify(notice) => StaAction::Notify(notice),
            Owed::Transmit {
                ticket,
                buffer,
                len,
                plan,
            } => StaAction::Transmit(StaTransmit {
                ticket,
                buffer,
                frame: &self.frame[..len],
                plan,
            }),
        })
    }

    /// The action `state` owes, and the state that awaits its end.
    fn owe(&mut self, state: State<K>, now: Instant) -> (State<K>, Option<Owed<T, K>>) {
        match state {
            State::Report(reason) => (
                State::Idle,
                Some(Owed::Notify(StaNotice::Disconnected(reason))),
            ),
            State::Scan(Scan { visit, step }) => {
                let (step, owed) = match step {
                    ScanStep::NeedChannel(channel) => {
                        (ScanStep::AwaitChannel, Some(Owed::RequestChannel(channel)))
                    }
                    ScanStep::NeedConfigure => (
                        ScanStep::AwaitConfigured,
                        Some(Owed::Configure(StationReceive::OtherBssManagement)),
                    ),
                    ScanStep::NeedProbe => match self.encode_probe() {
                        Some((ticket, buffer, len)) => (
                            ScanStep::AwaitProbe {
                                until: now.checked_add(TX_TERMINAL_TIMEOUT).unwrap_or(now),
                            },
                            Some(Owed::Transmit {
                                ticket,
                                buffer,
                                len,
                                plan: StaRatePlan::Group,
                            }),
                        ),
                        // No token or no encoding: listen passively.
                        None => (
                            ScanStep::Dwell {
                                until: now.checked_add(self.config.dwell).unwrap_or(now),
                            },
                            None,
                        ),
                    },
                    step => (step, None),
                };
                (State::Scan(Scan { visit, step }), owed)
            }
            State::Join(join) => self.owe_join(join, now),
            State::Leave(Leave { reason, step }) => {
                let (step, owed) = match step {
                    LeaveStep::NeedDeauthentication { bssid } => match self.encode_management(
                        StaManagementSubtype::Deauthentication,
                        bssid,
                        &DEAUTHENTICATION_LEAVING.to_le_bytes(),
                    ) {
                        Some((ticket, buffer, len)) => (
                            LeaveStep::AwaitDeauthentication {
                                until: now.checked_add(TX_TERMINAL_TIMEOUT).unwrap_or(now),
                            },
                            Some(Owed::Transmit {
                                ticket,
                                buffer,
                                len,
                                plan: StaRatePlan::Management,
                            }),
                        ),
                        None => (LeaveStep::NeedConfigure, None),
                    },
                    LeaveStep::NeedConfigure => (
                        LeaveStep::AwaitConfigured,
                        Some(Owed::Configure(StationReceive::Off)),
                    ),
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
            step,
        } = join;
        let (step, owed) = match step {
            JoinStep::NeedChannel(channel) => {
                (JoinStep::AwaitChannel, Some(Owed::RequestChannel(channel)))
            }
            JoinStep::NeedConfigure => {
                let receive = SupportedFilter::<K, StationRole>::new(ReceiveFilter::BSS_MEMBER)
                    .and_then(|filter| {
                        Some(StationReceive::Bss {
                            bssid: oer_ieee80211_lower_mac::IndividualAddress::new(
                                access_point.bssid,
                            )?,
                            filter,
                        })
                    });
                match receive {
                    Some(receive) => (JoinStep::AwaitConfigured, Some(Owed::Configure(receive))),
                    None => {
                        return (State::Report(StaDisconnectReason::Encode), None);
                    }
                }
            }
            JoinStep::Authenticate(mut auth) => match auth.stage {
                AuthStage::NeedSend => {
                    let Ok(attempt) = auth.runtime.begin_attempt(&mut self.sequence) else {
                        return (State::Report(StaDisconnectReason::AuthenticationFailed), None);
                    };
                    let Some((ticket, buffer)) = self.tokens() else {
                        return (State::Join(Join { access_point, rsn, step: JoinStep::Authenticate(auth) }), None);
                    };
                    let encoded = OpenAuthenticationRequest {
                        source: self.config.address,
                        bssid: access_point.bssid,
                        sequence_number: attempt.sequence_number,
                    }
                    .encode(&mut self.frame);
                    match encoded {
                        Ok(len) => {
                            auth.stage = AuthStage::AwaitSent {
                                response_timeout: attempt.response_timeout,
                            };
                            (
                                JoinStep::Authenticate(auth),
                                Some(Owed::Transmit {
                                    ticket,
                                    buffer,
                                    len,
                                    plan: StaRatePlan::Management,
                                }),
                            )
                        }
                        Err(_) => {
                            self.give_back(ticket, buffer);
                            return (State::Report(StaDisconnectReason::Encode), None);
                        }
                    }
                }
                _ => (JoinStep::Authenticate(auth), None),
            },
            JoinStep::Associate(mut associate) => {
                match associate.runtime.poll(now, &mut self.sequence) {
                    Ok(StaAssociationPoll::Transmit(attempt)) => {
                        associate.pending = Some(attempt.sequence_number);
                        let Some((ticket, buffer)) = self.tokens() else {
                            return (State::Join(Join { access_point, rsn, step: JoinStep::Associate(associate) }), None);
                        };
                        let encoded = AssociationRequest {
                            source: self.config.address,
                            access_point: &access_point,
                            sequence_number: attempt.sequence_number,
                            listen_interval: 10,
                            phy: self.config.phy,
                            security: &rsn,
                            power_capability: None,
                            he_ul_mu_power: None,
                        }
                        .encode(&mut self.frame, self.config.association);
                        match encoded {
                            Ok(len) => (
                                JoinStep::Associate(associate),
                                Some(Owed::Transmit {
                                    ticket,
                                    buffer,
                                    len,
                                    plan: StaRatePlan::Management,
                                }),
                            ),
                            Err(_) => {
                                self.give_back(ticket, buffer);
                                return (State::Report(StaDisconnectReason::Encode), None);
                            }
                        }
                    }
                    Ok(StaAssociationPoll::Idle) => (JoinStep::Associate(associate), None),
                    Ok(StaAssociationPoll::Failed { .. }) | Err(_) => {
                        return (State::Report(StaDisconnectReason::AssociationFailed), None);
                    }
                }
            }
            JoinStep::Report { channel } => {
                let bssid = access_point.bssid;
                return (
                    State::Connected(Connected { bssid }),
                    Some(Owed::Notify(StaNotice::Connected { bssid, channel })),
                );
            }
            step => (step, None),
        };
        (
            State::Join(Join {
                access_point,
                rsn,
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

    fn give_back(&mut self, ticket: T::Ticket, buffer: T::SmallBuffer) {
        self.tickets.put(ticket);
        self.buffers.put(buffer);
    }

    /// A broadcast Probe Request in the frame, with its tokens.
    fn encode_probe(&mut self) -> Option<(T::Ticket, T::SmallBuffer, usize)> {
        let (ticket, buffer) = self.tokens()?;
        let sequence_number = self.sequence.take();
        let rates = self.config.supported_rates;
        let encoded = ProbeRequest {
            destination: [0xff; 6],
            source: self.config.address,
            bssid: [0xff; 6],
            sequence_number,
            ssid: &self.config.ssid[..usize::from(self.config.ssid_len)],
            supported_rates: &rates,
        }
        .encode(&mut self.frame);
        match encoded {
            Ok(len) => Some((ticket, buffer, len)),
            Err(_) => {
                self.give_back(ticket, buffer);
                None
            }
        }
    }

    /// A management frame of `subtype` to `bssid` in the frame, with its
    /// tokens.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: [u8; 6],
        body: &[u8],
    ) -> Option<(T::Ticket, T::SmallBuffer, usize)> {
        let (ticket, buffer) = self.tokens()?;
        let encoded = StaManagementFrame {
            subtype,
            source: self.config.address,
            bssid,
            sequence_number: self.sequence.take(),
            body,
        }
        .encode(&mut self.frame);
        match encoded {
            Ok(len) => Some((ticket, buffer, len)),
            Err(_) => {
                self.give_back(ticket, buffer);
                None
            }
        }
    }

    /// When the station next needs a [`StaInput::Tick`], even without an
    /// event.
    pub fn poll_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Scan(Scan {
                step: ScanStep::AwaitProbe { until } | ScanStep::Dwell { until },
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Authenticate(Authenticate {
                    stage: AuthStage::AwaitResponse { until },
                    ..
                }),
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Associate(associate),
                ..
            }) => associate.runtime.next_deadline(),
            State::Leave(Leave {
                step: LeaveStep::AwaitDeauthentication { until },
                ..
            }) => Some(*until),
            _ => None,
        }
    }
}

/// What ended in the station's world.
enum Done {
    Command(StaCommand),
    Tick,
    /// A transmission ended: acknowledged, or not.
    Tx { delivered: bool },
    ChannelGranted,
    ChannelRefused,
    Configured,
}

/// An action a state owes, before it borrows the frame.
enum Owed<T: StaTokens, K> {
    RequestChannel(SupportedChannel<K>),
    Configure(StationReceive<K>),
    Transmit {
        ticket: T::Ticket,
        buffer: T::SmallBuffer,
        len: usize,
        plan: StaRatePlan,
    },
    Notify(StaNotice),
}

#[cfg(test)]
mod tests;
