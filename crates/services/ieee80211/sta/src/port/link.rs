//! The station's client of the lower-MAC port's event router, and its
//! transmit driver.

use core::{
    future::{Future, poll_fn},
    ops::Range,
    pin::pin,
    task::Poll,
};

use oer_ieee80211_lower_mac::{
    AmpduCapabilities, Channel, CoexPriority, EventsLost, FailureClass, Ieee80211LowerMacPort,
    KeySelector, LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacAmpdu,
    LowerMacBeaconTiming, LowerMacEvent, LowerMacSetting, MacAddress, PhyRate, ReceiveFilter,
    RxBuffer, RxMeta, SettingError, TbttEvent, TxPower, VifConfig, VifId, VifRole,
};
use oer_ieee80211_mac::{
    ccmp::CcmpTxPacketNumberError,
    data::EthernetFrameParts,
    qos::WmmAccessCategory,
    station::{AssociationRequestError, StationFrameError},
};
use oer_ieee80211_rsn::aes::AsyncRsnKeyUnwrap;
use oer_ieee80211_softmac::BackoffEntropy;
use oer_ieee80211_sta::{
    modem_sleep::{CoexView, PmCoexAction},
    rate_control::StaRateControl,
};
use oer_ieee80211_upper_mac::{
    HeTxopRtsBudget, MpduRequest, RateLadder, TxBody, TxPlanner, TxReceiver, TxReport, TxRequest,
};
pub use oer_ieee80211_upper_mac_service::EventRouter;
use oer_ieee80211_upper_mac_service::{AmpduFrames, UpperMacTx, UpperMacTxError};

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
    /// The lower-MAC backend. A station needs its TBTTs and TSF: its power
    /// manager runs for every association.
    type Port: LowerMacBeaconTiming;
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
    /// How the station sends A-MPDUs: [`PortAmpduAggregation`] over a port
    /// with [`LowerMacAmpdu`], [`NoAggregation`] over one without.
    type Aggregation: PortAggregation<Self>;
    /// The coexistence schedule of the radio system the station shares its
    /// RF with; [`NoCoexistence`] when it shares it with none.
    type Coex: PortCoexistence;
    /// How the station picks its data rates: one controller per
    /// association (`StaFixedRateControl`, or the Espressif
    /// `EspressifRateControl` of `oer-espressif-ieee80211-policy`).
    type RateControl: StaRateControl;
}

/// The coexistence schedule of the radio system a station shares its RF
/// with, which its integrator names once in [`PortStationEnv::Coex`].
///
/// The schedule belongs to the radio system every radio protocol shares,
/// not to the Wi-Fi MAC behind the port: the station's power manager reads
/// it and asks it for the air.
pub trait PortCoexistence {
    /// The schedule as the power manager decides with it.
    fn view(&self) -> CoexView;

    /// Perform one coexistence effect of the power manager.
    fn perform(&mut self, action: PmCoexAction) -> Result<(), PortCoexistenceRefused>;

    /// Ask the air for one connection frame the station is about to send,
    /// as the radio system's reconnect policy does after a lost
    /// association; `true` when the frame then carries
    /// [`CoexPriority::Elevated`]. A Probe Request only asks for the air.
    fn connection_frame(&mut self, frame: PortConnectionFrame) -> impl Future<Output = bool>;
}

/// A frame a station sends to (re)join its access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortConnectionFrame {
    ProbeRequest,
    Authentication,
    Association,
    Eapol,
}

/// The radio system refused a coexistence effect; its integrator keeps the
/// reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortCoexistenceRefused;

/// A station alone on its RF: coexistence is inactive and has no effects.
pub struct NoCoexistence;

impl PortCoexistence for NoCoexistence {
    fn view(&self) -> CoexView {
        CoexView::INACTIVE
    }

    fn perform(&mut self, _action: PmCoexAction) -> Result<(), PortCoexistenceRefused> {
        Ok(())
    }

    async fn connection_frame(&mut self, _frame: PortConnectionFrame) -> bool {
        false
    }
}

/// How a station's port sends A-MPDUs, which its integrator names once in
/// [`PortStationEnv::Aggregation`].
pub trait PortAggregation<X: PortStationEnv + ?Sized> {
    /// The port's A-MPDU capabilities; `None` sends every frame alone.
    fn capabilities(port: &X::Port) -> Option<AmpduCapabilities>;

    /// Send one A-MPDU through the link's transmit driver.
    fn send(
        link: &mut PortLink<'_, X>,
        frames: AmpduFrames<'_>,
        request: TxRequest,
    ) -> impl Future<Output = Result<TxReport, PortLinkError<PortError<X>>>>
    where
        X: Sized;
}

/// A port without [`LowerMacAmpdu`]: the station sends every frame alone,
/// its Block Ack agreements notwithstanding.
pub struct NoAggregation;

impl<X: PortStationEnv + ?Sized> PortAggregation<X> for NoAggregation {
    fn capabilities(_port: &X::Port) -> Option<AmpduCapabilities> {
        None
    }

    async fn send(
        _link: &mut PortLink<'_, X>,
        _frames: AmpduFrames<'_>,
        _request: TxRequest,
    ) -> Result<TxReport, PortLinkError<PortError<X>>>
    where
        X: Sized,
    {
        Err(PortLinkError::MissingState)
    }
}

/// A port with [`LowerMacAmpdu`]: aggregates go through its A-MPDU
/// attempts.
pub struct PortAmpduAggregation;

impl<X: PortStationEnv + ?Sized> PortAggregation<X> for PortAmpduAggregation
where
    X::Port: LowerMacAmpdu,
{
    fn capabilities(port: &X::Port) -> Option<AmpduCapabilities> {
        Some(port.ampdu_capabilities())
    }

    async fn send(
        link: &mut PortLink<'_, X>,
        frames: AmpduFrames<'_>,
        request: TxRequest,
    ) -> Result<TxReport, PortLinkError<PortError<X>>>
    where
        X: Sized,
    {
        link.transmit_ampdu(frames, request).await
    }
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

/// One received MPDU, header to end of body, in the port's buffer, and its
/// metadata. Dropping it returns the buffer to the port.
pub struct PortFrame<B> {
    buffer: B,
    meta: RxMeta,
}

impl<B: RxBuffer> PortFrame<B> {
    pub const fn new(buffer: B, meta: RxMeta) -> Self {
        Self { buffer, meta }
    }

    pub fn bytes(&self) -> &[u8] {
        self.buffer.bytes()
    }

    pub const fn meta(&self) -> RxMeta {
        self.meta
    }

    /// The port's buffer, to hand on.
    pub fn into_buffer(self) -> B {
        self.buffer
    }
}

/// One received MSDU the station hands to its application, as an
/// Ethernet-II frame.
pub enum PortMsdu<'a, B> {
    /// The only MSDU of an MPDU delivered in order, still in the port's
    /// buffer: its payload is `buffer.bytes()[payload]`. An application
    /// whose network stack adopts the port's buffers writes the Ethernet
    /// header over the dead 802.11 prefix and hands the buffer on, without
    /// a copy; any other copies [`Self::parts`].
    Buffer {
        buffer: B,
        destination: MacAddress,
        source: MacAddress,
        ether_type: u16,
        payload: Range<usize>,
    },
    /// An MSDU of an A-MSDU, or of an MPDU a reorder window kept: its
    /// parts, which the application copies.
    Parts(EthernetFrameParts<'a>),
}

impl<B: RxBuffer> PortMsdu<'_, B> {
    /// The frame's Ethernet header fields and payload.
    pub fn parts(&self) -> EthernetFrameParts<'_> {
        match self {
            Self::Buffer {
                buffer,
                destination,
                source,
                ether_type,
                payload,
            } => EthernetFrameParts {
                destination: *destination,
                source: *source,
                ether_type: *ether_type,
                // The station checked the range when it built the MSDU.
                payload: buffer.bytes().get(payload.clone()).unwrap_or_default(),
            },
            Self::Parts(parts) => *parts,
        }
    }
}

/// The receive buffer of a station's port.
pub type PortRxBuffer<X> = <<X as PortStationEnv>::Port as Ieee80211LowerMacPort>::RxBuffer;

/// One input of the port the station consumes; a frame is the port's own
/// buffer, never a copy.
pub enum PortInput<B> {
    Frame(PortFrame<B>),
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
    /// Received MPDUs longer than the backend's receive buffer.
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
    /// The radio system refused a coexistence effect.
    Coexistence,
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
    coex: X::Coex,
    rate: <X::RateControl as StaRateControl>::Config,
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
        coex: X::Coex,
        rate: <X::RateControl as StaRateControl>::Config,
        config: PortStationConfig,
    ) -> Self {
        Self {
            router,
            tx: UpperMacTx::new(router, config.vif, planner),
            ladder,
            entropy,
            config,
            coex,
            rate,
            counters: PortLinkCounters::default(),
        }
    }

    /// What every association's rate controller is configured with.
    pub const fn rate_config(&self) -> <X::RateControl as StaRateControl>::Config {
        self.rate
    }

    /// The coexistence schedule of the station's radio system.
    pub const fn coex(&self) -> &X::Coex {
        &self.coex
    }

    pub(crate) const fn coex_mut(&mut self) -> &mut X::Coex {
        &mut self.coex
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
        let coex = self.config.coex;
        self.transmit_at(frame, key, access_category, rate, coex)
            .await
    }

    /// Send a connection frame: the radio system is asked for the air
    /// first, and under its reconnect policy the frame carries
    /// [`CoexPriority::Elevated`].
    pub async fn transmit_connection_frame(
        &mut self,
        connection: PortConnectionFrame,
        frame: &[u8],
        key: KeySelector,
        access_category: WmmAccessCategory,
        rate: PhyRate,
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
        let coex = if self.coex.connection_frame(connection).await {
            CoexPriority::Elevated
        } else {
            self.config.coex
        };
        self.transmit_at(frame, key, access_category, rate, coex)
            .await
    }

    async fn transmit_at(
        &mut self,
        frame: &[u8],
        key: KeySelector,
        access_category: WmmAccessCategory,
        rate: PhyRate,
        coex: CoexPriority,
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
            coex,
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

    /// Send one A-MPDU until the planner reports the exchange's end.
    pub(crate) async fn transmit_ampdu(
        &mut self,
        frames: AmpduFrames<'_>,
        request: TxRequest,
    ) -> Result<TxReport, PortLinkError<PortError<X>>>
    where
        X::Port: LowerMacAmpdu,
    {
        let Self {
            tx,
            ladder,
            entropy,
            ..
        } = self;
        match tx.send_ampdu(frames, request, ladder, entropy).await {
            Ok(report) => Ok(report),
            Err(UpperMacTxError::Poisoned) => Err(PortLinkError::Poisoned),
            Err(error) => Err(PortLinkError::Tx(error)),
        }
    }

    /// The next input the router already holds: a TBTT first, then a
    /// received frame. It never waits.
    pub async fn try_input(&mut self) -> Option<PortInput<PortRxBuffer<X>>> {
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
    ) -> Option<PortInput<PortRxBuffer<X>>> {
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
    fn poll_input(
        &mut self,
        context: &mut core::task::Context<'_>,
    ) -> Poll<Option<PortInput<PortRxBuffer<X>>>> {
        loop {
            let router = self.router;
            match pin!(router.extension()).poll(context) {
                Poll::Ready(Some(Ok(event))) => {
                    if let Some(tbtt) = <X::Port as LowerMacBeaconTiming>::tbtt(&event) {
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
            return match pin!(router.received()).poll(context) {
                Poll::Ready(Some(Ok(event))) => match self.frame(event) {
                    Some(input) => Poll::Ready(Some(input)),
                    None => continue,
                },
                Poll::Ready(Some(Err(EventsLost))) => Poll::Ready(Some(self.lost())),
                Poll::Ready(None) => Poll::Ready(Some(PortInput::Poisoned)),
                Poll::Pending => Poll::Pending,
            };
        }
    }

    fn lost(&mut self) -> PortInput<PortRxBuffer<X>> {
        self.counters.events_lost = self.counters.events_lost.saturating_add(1);
        PortInput::EventsLost
    }

    /// A received frame in the port's buffer; `None` for any other event,
    /// counting a frame the backend could not hold.
    fn frame(
        &mut self,
        event: <X::Port as Ieee80211LowerMacPort>::Event,
    ) -> Option<PortInput<PortRxBuffer<X>>> {
        match <X::Port as Ieee80211LowerMacPort>::into_received(event) {
            Ok((buffer, meta)) => Some(PortInput::Frame(PortFrame::new(buffer, meta))),
            Err(event) => {
                if let LowerMacEvent::RxTooLong { .. } =
                    <X::Port as Ieee80211LowerMacPort>::view(&event)
                {
                    self.counters.oversized_frames =
                        self.counters.oversized_frames.saturating_add(1);
                }
                None
            }
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
