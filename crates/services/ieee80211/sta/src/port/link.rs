//! The station's client of the lower-MAC port's event router, and its
//! transmit driver.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, EventsLost, FailureClass, Ieee80211LowerMacPort, KeySelector,
    LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacBeaconTiming, LowerMacEvent,
    LowerMacSetting, MacAddress, PhyRate, ReceiveFilter, RxMeta, SettingError, TbttEvent,
    TbttSchedule, TxPower, VifConfig, VifId, VifRole, VifTsf,
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
pub use oer_ieee80211_upper_mac_service::EventRouter;
use oer_ieee80211_upper_mac_service::{UpperMacTx, UpperMacTxError};

/// Exchanges of the station that wait for a completion at once.
pub const PORT_EXCHANGES: usize = 2;

/// The event router of a station's port: the one consumer of the port's
/// events, which the composition polls ([`EventRouter::run`]) beside the
/// station and the backend's runner.
pub type PortRouter<'p, X> =
    EventRouter<'p, <X as PortStationEnv>::Port, PORT_EXCHANGES, PORT_BACKLOG>;
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
/// Received frames the router keeps for the station.
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
///
/// A frame travels inline: the station allocates nothing, so the small
/// variants share the frame's size.
#[allow(
    clippy::large_enum_variant,
    reason = "no_std without an allocator: received frames move by value"
)]
#[derive(Clone)]
pub enum PortInput {
    Frame(PortFrame),
    /// A TBTT of the station's interface, reported through
    /// [`LowerMacBeaconTiming`].
    Tbtt(TbttEvent),
    /// The port lost events: received frames or TBTTs in the gap are gone.
    /// The station goes on; an exchange recovers its own completion.
    EventsLost,
    /// The port reported its terminal poisoned event: the station stops.
    Poisoned,
}

/// What the station dropped at the port boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortLinkCounters {
    /// Received MPDUs longer than [`PORT_FRAME_CAPACITY`] or the backend's
    /// receive buffer.
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
    /// The port lost events while a lifecycle command ran, its terminal
    /// event possibly among them.
    LifecycleLost,
    /// The port reported its terminal poisoned event.
    Poisoned,
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

/// The outcome of a port setting call: the port's own failure outside, the
/// setting's refusal inside.
type SettingOutcome<P> = Result<Result<(), SettingError>, <P as Ieee80211LowerMacPort>::Error>;

/// The beacon-timing operations of a port that implements
/// [`LowerMacBeaconTiming`], kept as values so the station stays generic
/// over the base port.
pub struct BeaconTimingOps<P: Ieee80211LowerMacPort> {
    pub(crate) tbtt: fn(&P::Event) -> Option<TbttEvent>,
    pub(crate) set_tbtt: fn(&P, TbttSchedule) -> SettingOutcome<P>,
    pub(crate) stop_tbtt: fn(&P, VifId) -> SettingOutcome<P>,
    pub(crate) set_tsf: fn(&P, VifTsf) -> SettingOutcome<P>,
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
            stop_tbtt: P::stop_tbtt,
            set_tsf: P::set_tsf,
        }
    }
}

/// The station's client of the port.
///
/// The port's one event consumer is its [`PortRouter`]; the link reads the
/// router's queues and transmits through an [`UpperMacTx`] over it, so the
/// station never competes with the router for the port's events. Received
/// frames wait in the router's receive queue (a TBTT in its extension
/// queue) while an exchange waits for its completion. Every phase runs in
/// the task that owns the link; the composition polls [`EventRouter::run`]
/// beside it.
///
/// A loss the router reports is a [`PortInput::EventsLost`]; an exchange
/// whose completion fell into the gap cancels its attempt, as the router
/// contract states. The terminal poisoned event ends every operation with
/// [`PortLinkError::Poisoned`] or [`PortInput::Poisoned`].
pub struct PortLink<'p, X: PortStationEnv> {
    router: &'p PortRouter<'p, X>,
    tx: UpperMacTx<'p, 'p, X::Port, X::Budget, PORT_EXCHANGES, PORT_BACKLOG>,
    ladder: X::Ladder,
    entropy: X::Entropy,
    config: PortStationConfig,
    beacon_timing: Option<BeaconTimingOps<X::Port>>,
    counters: PortLinkCounters,
}

impl<'p, X: PortStationEnv> PortLink<'p, X> {
    /// A link of `config.vif` over the port of `router`, which allocates
    /// the attempt identities.
    pub fn new(
        router: &'p PortRouter<'p, X>,
        planner: TxPlanner<X::Budget>,
        ladder: X::Ladder,
        entropy: X::Entropy,
        config: PortStationConfig,
    ) -> Self {
        Self {
            router,
            tx: UpperMacTx::new(router, config.vif, planner),
            ladder,
            entropy,
            config,
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

    pub fn port(&self) -> &'p X::Port {
        self.router.port()
    }

    pub const fn router(&self) -> &'p PortRouter<'p, X> {
        self.router
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

    pub fn tx_mut(
        &mut self,
    ) -> &mut UpperMacTx<'p, 'p, X::Port, X::Budget, PORT_EXCHANGES, PORT_BACKLOG> {
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
            ..
        } = self;
        match tx.send_mpdu(frame, key, request, ladder, entropy).await {
            Ok(report) => Ok(report),
            Err(UpperMacTxError::Poisoned) => Err(PortLinkError::Poisoned),
            Err(error) => Err(PortLinkError::Tx(error)),
        }
    }

    /// The next input the router already holds: a TBTT first, then a
    /// received frame. It never waits.
    pub async fn try_input(&mut self) -> Option<PortInput> {
        poll_fn(|context| match self.poll_input(context) {
            Poll::Ready(input) => Poll::Ready(input),
            Poll::Pending => Poll::Ready(None),
        })
        .await
    }

    /// The next input, or `None` once `deadline` passed with none ready.
    pub async fn next_input<T: Timer>(
        &mut self,
        timer: &T,
        deadline: Instant,
    ) -> Option<PortInput> {
        let mut wait = pin!(timer.wait_until(deadline));
        poll_fn(|context| {
            if let Poll::Ready(input) = self.poll_input(context) {
                return Poll::Ready(input);
            }
            if wait.as_mut().poll(context).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await
    }

    /// Poll the router's extension and receive queues once; `Ready(None)`
    /// never occurs, a poisoned port is [`PortInput::Poisoned`].
    fn poll_input(&mut self, context: &mut core::task::Context<'_>) -> Poll<Option<PortInput>> {
        loop {
            let router = self.router;
            let tbtt = self.beacon_timing.map(|ops| ops.tbtt);
            if let Some(view) = tbtt {
                match pin!(router.extension()).poll(context) {
                    Poll::Ready(Some(Ok(event))) => {
                        if let Some(tbtt) = view(&event) {
                            return Poll::Ready(Some(PortInput::Tbtt(tbtt)));
                        }
                        continue;
                    }
                    Poll::Ready(Some(Err(EventsLost))) => {
                        return Poll::Ready(Some(self.lost()));
                    }
                    Poll::Ready(None) => return Poll::Ready(Some(PortInput::Poisoned)),
                    Poll::Pending => {}
                }
            }
            return match pin!(router.received()).poll(context) {
                Poll::Ready(Some(Ok(event))) => match self.frame(&event) {
                    Some(input) => Poll::Ready(Some(input)),
                    None => continue,
                },
                Poll::Ready(Some(Err(EventsLost))) => Poll::Ready(Some(self.lost())),
                Poll::Ready(None) => Poll::Ready(Some(PortInput::Poisoned)),
                Poll::Pending => Poll::Pending,
            };
        }
    }

    fn lost(&mut self) -> PortInput {
        self.counters.events_lost = self.counters.events_lost.saturating_add(1);
        PortInput::EventsLost
    }

    /// The station's copy of a received frame; `None` for one it cannot
    /// hold.
    fn frame(&mut self, event: &<X::Port as Ieee80211LowerMacPort>::Event) -> Option<PortInput> {
        match <X::Port as Ieee80211LowerMacPort>::view(event) {
            LowerMacEvent::Received { frame, meta } => match PortFrame::copy(frame, meta) {
                Some(frame) => Some(PortInput::Frame(frame)),
                None => {
                    self.counters.oversized_frames =
                        self.counters.oversized_frames.saturating_add(1);
                    None
                }
            },
            LowerMacEvent::RxTooLong { .. } => {
                self.counters.oversized_frames = self.counters.oversized_frames.saturating_add(1);
                None
            }
            _ => None,
        }
    }

    /// Drop every input the router already holds, as a new phase that must
    /// not see an earlier phase's frames does.
    pub async fn discard_backlog(&mut self) {
        while let Some(input) = self.try_input().await {
            if let PortInput::Poisoned = input {
                return;
            }
        }
    }

    /// Apply one setting.
    pub fn apply(&self, setting: LowerMacSetting) -> Result<(), PortLinkError<PortError<X>>> {
        self.port()
            .apply(setting)
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Setting)
    }

    /// Contend with the access point's EDCA parameters: the port's queues
    /// take their AIFSN and TXOP limits, the transmit planner each access
    /// category's contention window.
    pub fn install_edca(
        &mut self,
        parameters: oer_ieee80211_mac::extensions::wmm::WmmParameterSet,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.apply(LowerMacSetting::Edca(parameters))?;
        for category in [
            WmmAccessCategory::BestEffort,
            WmmAccessCategory::Background,
            WmmAccessCategory::Video,
            WmmAccessCategory::Voice,
        ] {
            self.tx.planner_mut().set_contention(
                category,
                oer_ieee80211_softmac::EdcaContention::from_wmm(
                    parameters.access_category(category),
                ),
            );
        }
        Ok(())
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

    /// Run one lifecycle command to its terminal event, which the router
    /// queues for the link.
    pub async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.port()
            .lifecycle(command)
            .map_err(PortLinkError::Port)?
            .map_err(PortLinkError::Lifecycle)?;
        match self.router.lifecycle().await {
            Some(Ok(LifecycleEvent::Failed { class, .. })) => {
                Err(PortLinkError::LifecycleFailed(class))
            }
            Some(Ok(_)) => Ok(()),
            // A lifecycle command has no identity to cancel: its terminal
            // may be in the gap.
            Some(Err(EventsLost)) => {
                self.counters.events_lost = self.counters.events_lost.saturating_add(1);
                Err(PortLinkError::LifecycleLost)
            }
            None => Err(PortLinkError::Poisoned),
        }
    }
}
