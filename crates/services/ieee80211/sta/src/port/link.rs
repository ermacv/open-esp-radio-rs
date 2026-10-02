//! The one owner of the lower-MAC port's events and transmissions.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, EventsLost, FailureClass, Ieee80211LowerMacPort, KeySelector,
    LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacBeaconTiming, LowerMacEvent,
    LowerMacSetting, MacAddress, PhyRate, ReceiveFilter, RxMeta, SettingError, TbttEvent,
    TbttSchedule, Tsf, TxPower, VifConfig, VifId, VifRole,
};
use oer_ieee80211_mac::{
    ccmp::CcmpTxPacketNumberError,
    qos::WmmAccessCategory,
    station::{AssociationRequestError, StationFrameError},
};
use oer_ieee80211_rsn::aes::AsyncRsnKeyUnwrap;
use oer_ieee80211_softmac::BackoffEntropy;
use oer_ieee80211_upper_mac::{
    HeTxopRtsBudget, MpduRequest, RateLadder, TxBody, TxPlanner, TxReceiver, TxReport, TxRequest,
};
use oer_ieee80211_upper_mac_service::{UpperMacTx, UpperMacTxError};
use oer_time::{Instant, Timer};

/// The types one station over the port is built from.
///
/// An integrator names its port, the transmit policy parameters, the
/// entropy of the backoff draw, the timer and the key-data unwrap once, in a
/// marker type, instead of on every station item.
pub trait PortStationEnv {
    /// The lower-MAC backend.
    type Port: Ieee80211LowerMacPort;
    /// The HE TXOP RTS budget of the transmit planner.
    type Budget: HeTxopRtsBudget;
    /// The rate of each retry of an MPDU.
    type Ladder: RateLadder;
    /// The random source of the EDCA backoff draw.
    type Entropy: BackoffEntropy;
    /// The image's monotonic time: deadlines of every phase.
    type Timer: Timer;
    /// The EAPOL-Key data unwrap of the WPA2 handshake.
    type KeyUnwrap: AsyncRsnKeyUnwrap;
}

/// The port error type of an environment.
pub type PortError<X> = <<X as PortStationEnv>::Port as Ieee80211LowerMacPort>::Error;

/// How the station transmits and which interface it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortStationConfig {
    pub vif: VifId,
    /// The station's address.
    pub address: MacAddress,
    /// The first rate of management, EAPOL and Null frames.
    pub management_rate: PhyRate,
    /// The first rate of data frames.
    pub data_rate: PhyRate,
    pub power: TxPower,
    pub coex: CoexPriority,
    /// Transmissions one MPDU may make, the first included.
    pub retry_limit: u8,
}

/// Octets of one received MPDU the station keeps.
pub const PORT_FRAME_CAPACITY: usize = 2_352;
/// Inputs the station keeps while it waits for its own transmission.
pub const PORT_BACKLOG: usize = 4;

const FCS_LEN: u32 = 4;
const CCMP_MIC_LEN: u32 = 8;

/// One received MPDU, header to end of body, and its metadata.
#[derive(Clone)]
pub struct PortFrame {
    bytes: [u8; PORT_FRAME_CAPACITY],
    len: usize,
    meta: RxMeta,
}

impl PortFrame {
    /// A copy of `frame`; `None` when it exceeds [`PORT_FRAME_CAPACITY`].
    pub fn copy(frame: &[u8], meta: RxMeta) -> Option<Self> {
        let mut bytes = [0; PORT_FRAME_CAPACITY];
        bytes.get_mut(..frame.len())?.copy_from_slice(frame);
        Some(Self {
            bytes,
            len: frame.len(),
            meta,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub const fn meta(&self) -> RxMeta {
        self.meta
    }
}

/// One input of the port the station consumes.
#[derive(Clone)]
pub enum PortInput {
    Frame(PortFrame),
    /// A TBTT of the station's interface, reported through
    /// [`LowerMacBeaconTiming`].
    Tbtt(TbttEvent),
    /// The port lost events.
    EventsLost,
}

/// Inputs that arrived while the station waited for a completion.
struct Backlog {
    entries: [Option<PortInput>; PORT_BACKLOG],
    head: usize,
    len: usize,
}

impl Backlog {
    const fn new() -> Self {
        Self {
            entries: [const { None }; PORT_BACKLOG],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, input: PortInput, counters: &mut PortLinkCounters) {
        if self.len == PORT_BACKLOG {
            counters.backlog_overflows = counters.backlog_overflows.saturating_add(1);
            return;
        }
        self.entries[(self.head + self.len) % PORT_BACKLOG] = Some(input);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<PortInput> {
        if self.len == 0 {
            return None;
        }
        let input = self.entries[self.head].take();
        self.head = (self.head + 1) % PORT_BACKLOG;
        self.len -= 1;
        input
    }

    fn clear(&mut self) {
        while self.pop().is_some() {}
    }
}

/// What the station dropped at the port boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortLinkCounters {
    /// Inputs that arrived during a transmission while the backlog was full.
    pub backlog_overflows: u32,
    /// Received MPDUs longer than [`PORT_FRAME_CAPACITY`].
    pub oversized_frames: u32,
    /// Reports of lost port events.
    pub events_lost: u32,
}

/// Why a station operation over the port failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortLinkError<E> {
    /// The port is poisoned.
    Port(E),
    /// An exchange ended without a report.
    Tx(UpperMacTxError<E>),
    /// The port refused a setting or a key.
    Setting(SettingError),
    /// The port refused a lifecycle command.
    Lifecycle(LifecycleError),
    /// A lifecycle command failed.
    LifecycleFailed(FailureClass),
    /// A frame did not encode.
    Frame(StationFrameError),
    /// An Association Request did not encode.
    Association(AssociationRequestError),
    /// The pairwise key's transmit packet numbers are used up.
    PacketNumber(CcmpTxPacketNumberError),
    /// The port lets the station receive no other BSS's beacons: no receive
    /// filter of it admits them and no monitor reception was offered.
    ReceptionUnsupported,
    /// A phase ran without the state an earlier phase establishes.
    MissingState,
}

/// The beacon-timing operations of a port that implements
/// [`LowerMacBeaconTiming`], kept as values so the station stays generic
/// over the base port.
pub struct BeaconTimingOps<P: Ieee80211LowerMacPort> {
    pub(crate) tbtt: fn(&P::Event) -> Option<TbttEvent>,
    pub(crate) set_tbtt:
        fn(&P, VifId, Option<TbttSchedule>) -> Result<Result<(), SettingError>, P::Error>,
    pub(crate) set_tsf: fn(&P, VifId, Tsf) -> Result<Result<(), SettingError>, P::Error>,
}

impl<P: Ieee80211LowerMacPort> Clone for BeaconTimingOps<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Ieee80211LowerMacPort> Copy for BeaconTimingOps<P> {}

impl<P: LowerMacBeaconTiming> BeaconTimingOps<P> {
    pub fn of_port() -> Self {
        Self {
            tbtt: P::tbtt,
            set_tbtt: P::set_tbtt,
            set_tsf: P::set_tsf,
        }
    }
}

/// Poll `future` once: its output, or `None` while it is pending.
pub(crate) async fn poll_once<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    poll_fn(|context| {
        Poll::Ready(match future.as_mut().poll(context) {
            Poll::Ready(output) => Some(output),
            Poll::Pending => None,
        })
    })
    .await
}

/// The station's single consumer of the port.
///
/// The link is the one owner of [`Ieee80211LowerMacPort::next_event`] and
/// of the [`UpperMacTx`] exchange driver: every phase runs in the task that
/// owns it and takes inputs from it in turn. While an exchange waits for its
/// completion, the received frames and TBTTs it hands aside are kept in a
/// bounded backlog, which the next input read returns first, in arrival
/// order.
pub struct PortLink<'p, X: PortStationEnv> {
    port: &'p X::Port,
    tx: UpperMacTx<'p, X::Port, X::Budget>,
    ladder: X::Ladder,
    entropy: X::Entropy,
    config: PortStationConfig,
    backlog: Backlog,
    beacon_timing: Option<BeaconTimingOps<X::Port>>,
    counters: PortLinkCounters,
}

impl<'p, X: PortStationEnv> PortLink<'p, X> {
    /// A link of `config.vif` over `port` whose attempt identities start at
    /// `first_tx_id`.
    pub fn new(
        port: &'p X::Port,
        planner: TxPlanner<X::Budget>,
        ladder: X::Ladder,
        entropy: X::Entropy,
        config: PortStationConfig,
        first_tx_id: u32,
    ) -> Self {
        Self {
            port,
            tx: UpperMacTx::new(port, config.vif, planner, first_tx_id),
            ladder,
            entropy,
            config,
            backlog: Backlog::new(),
            beacon_timing: None,
            counters: PortLinkCounters::default(),
        }
    }

    /// Report TBTTs of the station's interface as [`PortInput::Tbtt`]; the
    /// power-save driver needs them.
    pub fn with_beacon_timing(mut self) -> Self
    where
        X::Port: LowerMacBeaconTiming,
    {
        self.beacon_timing = Some(BeaconTimingOps::of_port());
        self
    }

    pub const fn port(&self) -> &'p X::Port {
        self.port
    }

    pub const fn config(&self) -> &PortStationConfig {
        &self.config
    }

    pub const fn counters(&self) -> PortLinkCounters {
        self.counters
    }

    pub(crate) const fn beacon_timing(&self) -> Option<BeaconTimingOps<X::Port>> {
        self.beacon_timing
    }

    pub fn tx_mut(&mut self) -> &mut UpperMacTx<'p, X::Port, X::Budget> {
        &mut self.tx
    }

    /// Send one MPDU, header to end of body, until the planner reports the
    /// exchange's end. A group-addressed frame solicits no response.
    pub async fn transmit(
        &mut self,
        frame: &[u8],
        key: KeySelector,
        access_category: WmmAccessCategory,
        rate: PhyRate,
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
        let address1: [u8; 6] = frame
            .get(4..10)
            .and_then(|address| address.try_into().ok())
            .ok_or(PortLinkError::Frame(StationFrameError::OutputTooSmall {
                required: 10,
            }))?;
        let receiver = TxReceiver::from_address1(&address1);
        let mic = match key {
            KeySelector::Key(_) => CCMP_MIC_LEN,
            KeySelector::Plaintext => 0,
        };
        let request = TxRequest {
            access_category,
            initial_rate: rate,
            receiver,
            power: self.config.power,
            coex: self.config.coex,
            mpdu_retry_limit: self.config.retry_limit,
            body: TxBody::Mpdu(MpduRequest {
                length: frame.len() as u32 + FCS_LEN + mic,
                response: match receiver {
                    TxReceiver::Individual => oer_ieee80211_lower_mac::TxResponse::Ack,
                    TxReceiver::Group => oer_ieee80211_lower_mac::TxResponse::None,
                },
            }),
        };
        let Self {
            tx,
            ladder,
            entropy,
            backlog,
            beacon_timing,
            counters,
            ..
        } = self;
        let tbtt = beacon_timing.map(|ops| ops.tbtt);
        tx.send_mpdu(frame, key, request, ladder, entropy, |event| {
            if let Some(input) = classify::<X::Port>(event, tbtt, counters) {
                backlog.push(input, counters);
            }
        })
        .await
        .map_err(PortLinkError::Tx)
    }

    /// The next input that is already there: the backlog first, then the
    /// port's ready events. It never waits.
    pub async fn try_input(&mut self) -> Option<PortInput> {
        if let Some(input) = self.backlog.pop() {
            return Some(input);
        }
        loop {
            let event = poll_once(self.port.next_event()).await?;
            if let Some(input) = self.input(event) {
                return Some(input);
            }
        }
    }

    /// The next input, or `None` once `deadline` passed with none ready.
    pub async fn next_input<T: Timer>(
        &mut self,
        timer: &T,
        deadline: Instant,
    ) -> Option<PortInput> {
        if let Some(input) = self.backlog.pop() {
            return Some(input);
        }
        let port = self.port;
        let mut wait = pin!(timer.wait_until(deadline));
        loop {
            let event = {
                let mut next = pin!(port.next_event());
                poll_fn(|context| {
                    if let Poll::Ready(event) = next.as_mut().poll(context) {
                        return Poll::Ready(Some(event));
                    }
                    if wait.as_mut().poll(context).is_ready() {
                        return Poll::Ready(None);
                    }
                    Poll::Pending
                })
                .await
            }?;
            if let Some(input) = self.input(event) {
                return Some(input);
            }
        }
    }

    /// Drop every kept input, as a new phase that must not see an earlier
    /// phase's frames does.
    pub fn discard_backlog(&mut self) {
        self.backlog.clear();
    }

    fn input(
        &mut self,
        event: Result<<X::Port as Ieee80211LowerMacPort>::Event, EventsLost>,
    ) -> Option<PortInput> {
        match event {
            Err(EventsLost) => {
                self.counters.events_lost = self.counters.events_lost.saturating_add(1);
                Some(PortInput::EventsLost)
            }
            Ok(event) => classify::<X::Port>(
                &event,
                self.beacon_timing.map(|ops| ops.tbtt),
                &mut self.counters,
            ),
        }
    }

    /// Apply one setting.
    pub fn apply(&self, setting: LowerMacSetting) -> Result<(), PortLinkError<PortError<X>>> {
        self.port
            .apply(setting)
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Setting)
    }

    /// Configure the station interface with `bssid` and `receive`.
    pub fn configure(
        &self,
        bssid: Option<MacAddress>,
        receive: ReceiveFilter,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.apply(LowerMacSetting::Vif {
            vif: self.config.vif,
            config: Some(VifConfig {
                address: self.config.address,
                role: VifRole::Station,
                bssid,
                receive,
            }),
        })
    }

    /// Tune to `channel`. A backend that retunes only while disabled
    /// refuses the setting as `Busy`; the link then disables the port,
    /// tunes and enables it again.
    pub async fn retune(&mut self, channel: Channel) -> Result<(), PortLinkError<PortError<X>>> {
        match self.apply(LowerMacSetting::Channel(channel)) {
            Err(PortLinkError::Setting(SettingError::Busy)) => {
                self.lifecycle(LifecycleCommand::Disable).await?;
                self.apply(LowerMacSetting::Channel(channel))?;
                self.lifecycle(LifecycleCommand::Enable).await
            }
            result => result,
        }
    }

    /// Run one lifecycle command to its terminal event.
    pub async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.port
            .lifecycle(command)
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Lifecycle)?;
        loop {
            let Ok(event) = self.port.next_event().await else {
                self.counters.events_lost = self.counters.events_lost.saturating_add(1);
                continue;
            };
            match <X::Port as Ieee80211LowerMacPort>::view(&event) {
                LowerMacEvent::Lifecycle(LifecycleEvent::Failed { class, .. }) => {
                    return Err(PortLinkError::LifecycleFailed(class));
                }
                LowerMacEvent::Lifecycle(_) => return Ok(()),
                _ => {
                    let tbtt = self.beacon_timing.map(|ops| ops.tbtt);
                    if let Some(input) = classify::<X::Port>(&event, tbtt, &mut self.counters) {
                        self.backlog.push(input, &mut self.counters);
                    }
                }
            }
        }
    }
}

/// The station's view of one port event; `None` for an event it does not
/// consume here (a completion of no running exchange, a lifecycle event).
fn classify<P: Ieee80211LowerMacPort>(
    event: &P::Event,
    tbtt: Option<fn(&P::Event) -> Option<TbttEvent>>,
    counters: &mut PortLinkCounters,
) -> Option<PortInput> {
    match P::view(event) {
        LowerMacEvent::Received { frame, meta } => match PortFrame::copy(frame, meta) {
            Some(frame) => Some(PortInput::Frame(frame)),
            None => {
                counters.oversized_frames = counters.oversized_frames.saturating_add(1);
                None
            }
        },
        LowerMacEvent::Extension => tbtt.and_then(|view| view(event)).map(PortInput::Tbtt),
        LowerMacEvent::TxCompleted(_) | LowerMacEvent::Lifecycle(_) => None,
    }
}
