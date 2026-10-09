//! The station's client of the lower-MAC port's event router, and its
//! transmit driver.

use core::future::Future;

use oer_ieee80211_datapath::DestinationTxQueues;
use oer_ieee80211_lower_mac::{
    AmpduCapabilities, Channel, ClockError, CoexPriority, KeySelector, LifecycleCommand,
    LifecycleError, LowerMacSetting, MacAddress, PhyRate, Poisoned, ReceiveFilter, SettingError,
    TxPower, VifId, VifRole,
};
use oer_ieee80211_mac::{
    ccmp::CcmpTxPacketNumberError,
    qos::WmmAccessCategory,
    station::{AssociationRequestError, StationFrameError},
};
use oer_ieee80211_rsn::aes::AsyncRsnKeyUnwrap;
use oer_ieee80211_sta::modem_sleep::{CoexView, PmCoexAction};
use oer_ieee80211_upper_mac::{TxPlanner, TxReport, TxRequest, rate_control::RateControl};
use oer_ieee80211_upper_mac_service::{
    AmpduFrames, AttachError, TxMpdu, UpperMacTx, UpperMacTxError,
    aggregate::PortAggregation,
    client::{
        PortBody, PortClient, PortClientConfig, PortClientCounters, PortClientEnv, PortClientError,
        PortFault, PortInput, PortRxBuffer,
    },
};
pub use oer_ieee80211_upper_mac_service::{EventRouter, PORT_BACKLOG, PORT_EXCHANGES};
use oer_time::{Instant, Timer};

/// The event router of a station's port: the one consumer of the port's
/// events, shared with an access point beside the station, which the
/// composition polls ([`EventRouter::run`]) beside its clients and the
/// backend's runner.
pub type PortRouter<'p, X> =
    oer_ieee80211_upper_mac_service::PortRouter<'p, <X as PortClientEnv>::Port>;

/// The types one station over the port is built from.
///
/// An integrator names its port and transmit policy ([`PortClientEnv`]:
/// the station needs the port's TBTTs and TSF, as its power manager runs
/// for every association), the timer and the key-data unwrap once, in a
/// marker type, instead of on every station item.
pub trait PortStationEnv: PortClientEnv {
    /// The image's monotonic time: deadlines of every phase.
    type Timer: Timer;
    /// The EAPOL-Key data unwrap of the WPA2 handshake.
    type KeyUnwrap: AsyncRsnKeyUnwrap;
    /// The coexistence schedule of the radio system the station shares its
    /// RF with; [`NoCoexistence`] when it shares it with none.
    type Coex: PortCoexistence;
    /// How the station picks its data rates: one controller per
    /// association (`FixedRateControl`, or the Espressif
    /// `EspressifRateControl` of `oer-espressif-ieee80211-policy`).
    type RateControl: RateControl;
    /// Where the frames the station sends wait: the network's own owners,
    /// queued by Ethernet destination, which the station takes when it
    /// sends them.
    type Frames: DestinationTxQueues<Frame = Self::NetworkFrame>;
}

/// A frame the station sends: an owner of its network's source.
pub type PortStationFrame<X> = <X as PortClientEnv>::NetworkFrame;

/// The coexistence schedule of the radio system a station shares its RF
/// with, which its integrator names once in [`PortStationEnv::Coex`].
///
/// The schedule belongs to the radio system every radio protocol shares,
/// not to the Wi-Fi MAC behind the port: the station's power manager reads
/// it and asks it for the air.
pub trait PortCoexistence {
    /// The schedule as the power manager decides with it.
    fn view(&self) -> CoexView;

    /// Perform one coexistence effect of the power manager; a radio system
    /// whose schedule is shared under a lock takes it.
    fn perform(
        &mut self,
        action: PmCoexAction,
    ) -> impl Future<Output = Result<(), PortCoexistenceRefused>>;

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

    async fn perform(&mut self, _action: PmCoexAction) -> Result<(), PortCoexistenceRefused> {
        Ok(())
    }

    async fn connection_frame(&mut self, _frame: PortConnectionFrame) -> bool {
        false
    }
}

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

/// Why a station operation over the port failed; `F` is the port's poison
/// cause.
#[derive(Debug, Eq, PartialEq)]
pub enum PortLinkError<F> {
    /// The port is poisoned.
    Poisoned(Poisoned<F>),
    /// An exchange ended without a report.
    Tx(UpperMacTxError<F>),
    /// The port refused a setting or a key.
    Setting(SettingError),
    /// The port refused a lifecycle command.
    Lifecycle(LifecycleError),
    /// A lifecycle command failed and left the port as it was.
    LifecycleFailed,
    /// The router's lifecycle queue overflowed while a lifecycle command
    /// ran, its terminal event possibly among the dropped ones.
    LifecycleLost,
    /// The port's radio clock could not be read.
    Clock(ClockError),
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

impl<F> From<Poisoned<F>> for PortLinkError<F> {
    fn from(poisoned: Poisoned<F>) -> Self {
        Self::Poisoned(poisoned)
    }
}

impl<F> From<PortClientError<F>> for PortLinkError<F> {
    fn from(error: PortClientError<F>) -> Self {
        match error {
            PortClientError::Poisoned(poisoned) => Self::Poisoned(poisoned),
            PortClientError::Tx(error) => Self::Tx(error),
            PortClientError::Setting(error) => Self::Setting(error),
            PortClientError::Lifecycle(error) => Self::Lifecycle(error),
            PortClientError::LifecycleFailed => Self::LifecycleFailed,
            PortClientError::LifecycleLost => Self::LifecycleLost,
            PortClientError::FrameTooShort => {
                Self::Frame(StationFrameError::OutputTooSmall { required: 10 })
            }
            PortClientError::AggregationUnsupported => Self::MissingState,
        }
    }
}

/// The station's client of the port: the [`PortClient`] of its interface,
/// with the coexistence schedule and the rate control configuration the
/// station adds.
pub struct PortLink<'p, X: PortStationEnv> {
    client: PortClient<'p, X, PORT_EXCHANGES, PORT_BACKLOG>,
    config: PortStationConfig,
    coex: X::Coex,
    rate: <X::RateControl as RateControl>::Config,
}

impl<'p, X: PortStationEnv> PortLink<'p, X> {
    /// A link of `config.vif` over the port of `router`, which allocates
    /// the attempt identities and queues the interface's frames for the
    /// link; another client of the interface is refused.
    pub fn new(
        router: &'p PortRouter<'p, X>,
        planner: TxPlanner<X::Budget>,
        ladder: X::Ladder,
        entropy: X::Entropy,
        coex: X::Coex,
        rate: <X::RateControl as RateControl>::Config,
        config: PortStationConfig,
    ) -> Result<Self, AttachError> {
        Ok(Self {
            client: PortClient::new(
                router,
                planner,
                ladder,
                entropy,
                PortClientConfig {
                    vif: config.vif,
                    address: config.address,
                    role: VifRole::Station,
                    power: config.power,
                    retry_limit: config.retry_limit,
                },
            )?,
            config,
            coex,
            rate,
        })
    }

    /// What every association's rate controller is configured with.
    pub const fn rate_config(&self) -> <X::RateControl as RateControl>::Config {
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
        self.client.port()
    }

    pub const fn router(&self) -> &'p PortRouter<'p, X> {
        self.client.router()
    }

    pub const fn config(&self) -> &PortStationConfig {
        &self.config
    }

    pub const fn counters(&self) -> PortClientCounters {
        self.client.counters()
    }

    pub fn tx_mut(
        &mut self,
    ) -> &mut UpperMacTx<'p, 'p, X::Port, X::Budget, PORT_EXCHANGES, PORT_BACKLOG> {
        self.client.tx_mut()
    }

    /// Send one MPDU, header to end of body, until the planner reports the
    /// exchange's end. A group-addressed frame solicits no response.
    pub async fn transmit(
        &mut self,
        frame: TxMpdu<'_, PortBody<X>>,
        key: KeySelector,
        access_category: WmmAccessCategory,
        rate: PhyRate,
    ) -> Result<TxReport, PortLinkError<PortFault<X>>> {
        let coex = self.config.coex;
        Ok(self
            .client
            .transmit(frame, key, access_category, rate, coex)
            .await?)
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
    ) -> Result<TxReport, PortLinkError<PortFault<X>>> {
        let coex = if self.coex.connection_frame(connection).await {
            CoexPriority::Elevated
        } else {
            self.config.coex
        };
        Ok(self
            .client
            .transmit(TxMpdu::whole(frame), key, access_category, rate, coex)
            .await?)
    }

    /// The port's A-MPDU capabilities; `None` when it sends every frame
    /// alone.
    pub(crate) fn ampdu_capabilities(&self) -> Option<AmpduCapabilities> {
        <X::Aggregation as PortAggregation<X>>::capabilities(self.client.port())
    }

    /// Send one A-MPDU until the planner reports the exchange's end.
    pub(crate) async fn transmit_ampdu(
        &mut self,
        frames: AmpduFrames<'_, PortBody<X>>,
        request: TxRequest,
    ) -> Result<TxReport, PortLinkError<PortFault<X>>> {
        Ok(<X::Aggregation as PortAggregation<X>>::send(&mut self.client, frames, request).await?)
    }

    /// The next input the router already holds: a TBTT first, then a
    /// received frame. It never waits.
    pub async fn try_input(&mut self) -> Option<PortInput<PortRxBuffer<X>, PortFault<X>>> {
        self.client.try_input().await
    }

    /// The next input, or `None` once `deadline` passed with none ready.
    pub async fn next_input<T: Timer>(
        &mut self,
        timer: &T,
        deadline: Instant,
    ) -> Option<PortInput<PortRxBuffer<X>, PortFault<X>>> {
        self.client.next_input(timer, deadline).await
    }

    /// Drop every input the router already holds, as a new phase that must
    /// not see an earlier phase's frames does.
    pub async fn discard_backlog(&mut self) {
        self.client.discard_backlog().await;
    }

    /// Apply one setting.
    pub fn apply(&self, setting: LowerMacSetting) -> Result<(), PortLinkError<PortFault<X>>> {
        Ok(self.client.apply(setting)?)
    }

    /// Contend with the access point's EDCA parameters: the port's queues
    /// take their AIFSN and TXOP limits, the transmit planner each access
    /// category's contention window.
    pub fn install_edca(
        &mut self,
        parameters: oer_ieee80211_mac::extensions::wmm::WmmParameterSet,
    ) -> Result<(), PortLinkError<PortFault<X>>> {
        Ok(self.client.install_edca(parameters)?)
    }

    /// Configure the station interface with `bssid` and `receive`.
    pub fn configure(
        &self,
        bssid: Option<MacAddress>,
        receive: ReceiveFilter,
    ) -> Result<(), PortLinkError<PortFault<X>>> {
        Ok(self.client.configure(bssid, receive)?)
    }

    /// Tune to `channel`, disabling the port around it where the backend
    /// retunes only while disabled.
    pub async fn retune(&mut self, channel: Channel) -> Result<(), PortLinkError<PortFault<X>>> {
        Ok(self.client.retune(channel).await?)
    }

    /// Run one lifecycle command to its terminal event.
    pub async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> Result<(), PortLinkError<PortFault<X>>> {
        Ok(self.client.lifecycle(command).await?)
    }
}
