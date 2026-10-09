//! A service's client of a lower-MAC port: one interface's reception, its
//! transmissions and the port's settings and lifecycle, over the port's
//! [`EventRouter`].
//!
//! The router is the port's one event consumer; a client reads the router's
//! queues and transmits through an [`UpperMacTx`] over it, so a service
//! never competes with the router for the port's events. Received frames
//! wait in the router's receive queue (a TBTT in its extension queue) while
//! an exchange waits for its completion. Every operation runs in the task
//! that owns the client; the composition polls [`EventRouter::run`] beside
//! it.
//!
//! A loss the router reports is a [`PortInput::EventsLost`]; an exchange
//! whose completion fell into the gap cancels its attempt, as the router
//! contract states. The terminal poisoned event ends every operation with
//! [`PortClientError::Poisoned`] or [`PortInput::Poisoned`].

use core::{
    future::{Future, poll_fn},
    ops::Range,
    pin::pin,
    task::Poll,
};

use oer_ieee80211_datapath::SoftwareTxFrame;
use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, EventsLost, FailureClass, Ieee80211LowerMacPort, KeySelector,
    LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacAirReservation, LowerMacAmpdu,
    LowerMacBeaconTiming, LowerMacSetting, MacAddress, PhyRate, ReceiveFilter, RxBuffer, RxMeta,
    SettingError, TbttEvent, TxCompletion, TxPower, VifConfig, VifId, VifRole,
};
use oer_ieee80211_mac::{data::EthernetFrameParts, qos::WmmAccessCategory};
use oer_ieee80211_softmac::BackoffEntropy;
use oer_ieee80211_upper_mac::{
    HeTxopRtsBudget, MpduRequest, RateLadder, TxBody, TxPlanner, TxReceiver, TxReport, TxRequest,
};
use oer_time::{Instant, Timer};

use crate::{
    AmpduFrames, AttachError, Attachment, EventRouter, TxMpdu, UpperMacTx, UpperMacTxError,
    aggregate::PortAggregation, frame::NetworkBody,
};

const FCS_LEN: u32 = 4;
const CCMP_MIC_LEN: u32 = 8;

/// The owner of an MPDU's body that a client's port takes: the network's
/// frame, as [`NetworkBody`].
pub type PortBody<X> = NetworkBody<<X as PortClientEnv>::NetworkFrame>;

/// The types a client of the port is built from, which a service's
/// environment names once.
pub trait PortClientEnv {
    /// The network's frame a client sends, whose payload the port takes by
    /// ownership as an MPDU's body.
    type NetworkFrame: SoftwareTxFrame;
    /// The lower-MAC backend, which takes the network's frames as bodies. A
    /// client reads the TBTTs its interface's schedule reports.
    type Port: LowerMacBeaconTiming
        + Ieee80211LowerMacPort<TxBody = NetworkBody<Self::NetworkFrame>>;
    /// The HE TXOP RTS budget of the transmit planner.
    type Budget: HeTxopRtsBudget;
    /// The rate of each retry of an MPDU.
    type Ladder: RateLadder;
    /// The random source of the EDCA backoff draw.
    type Entropy: BackoffEntropy;
    /// How the port sends A-MPDUs:
    /// [`PortAmpduAggregation`](crate::aggregate::PortAmpduAggregation) over
    /// a port with [`LowerMacAmpdu`],
    /// [`NoAggregation`](crate::aggregate::NoAggregation) over one without.
    type Aggregation: PortAggregation<Self>;
}

/// The port error type of an environment.
pub type PortError<X> = <<X as PortClientEnv>::Port as Ieee80211LowerMacPort>::Error;

/// The receive buffer of an environment's port.
pub type PortRxBuffer<X> = <<X as PortClientEnv>::Port as Ieee80211LowerMacPort>::RxBuffer;

/// The interface a client is and how it transmits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortClientConfig {
    pub vif: VifId,
    /// The interface's address.
    pub address: MacAddress,
    pub role: VifRole,
    pub power: TxPower,
    /// Transmissions one MPDU may make, the first included.
    pub retry_limit: u8,
}

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

/// One received MSDU a service hands to its application, as an Ethernet-II
/// frame.
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
                // The service checked the range when it built the MSDU.
                payload: buffer.bytes().get(payload.clone()).unwrap_or_default(),
            },
            Self::Parts(parts) => *parts,
        }
    }
}

/// One input of the port a client consumes; a frame is the port's own
/// buffer, never a copy.
pub enum PortInput<B> {
    Frame(PortFrame<B>),
    /// A TBTT of an interface's schedule, reported through
    /// [`LowerMacBeaconTiming`].
    Tbtt(TbttEvent),
    /// The port lost events: received frames or TBTTs in the gap are gone.
    /// The service goes on; an exchange recovers its own completion.
    EventsLost,
    /// The port reported its terminal poisoned event: the service stops.
    Poisoned,
}

/// What a client dropped at the port boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortClientCounters {
    /// Reports of lost port events.
    pub events_lost: u32,
}

/// Why a client operation over the port failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortClientError<E> {
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
    /// A frame to send is shorter than its first address.
    FrameTooShort,
    /// An A-MPDU went to a port that sends none
    /// ([`NoAggregation`](crate::aggregate::NoAggregation)).
    AggregationUnsupported,
}

/// One interface's client of the port: see the [module](self).
pub struct PortClient<'p, X: PortClientEnv, const EXCHANGES: usize, const RX: usize> {
    router: &'p EventRouter<'p, X::Port, EXCHANGES, RX>,
    /// The client's interface on the router: its frames are queued for it.
    attachment: Attachment<'p, 'p, X::Port, EXCHANGES, RX>,
    tx: UpperMacTx<'p, 'p, X::Port, X::Budget, EXCHANGES, RX>,
    ladder: X::Ladder,
    entropy: X::Entropy,
    config: PortClientConfig,
    counters: PortClientCounters,
}

impl<'p, X: PortClientEnv, const EXCHANGES: usize, const RX: usize>
    PortClient<'p, X, EXCHANGES, RX>
{
    /// A client of `config.vif` over the port of `router`, which allocates
    /// the attempt identities and queues the interface's frames for it until
    /// the client drops; another client of the interface is refused.
    pub fn new(
        router: &'p EventRouter<'p, X::Port, EXCHANGES, RX>,
        planner: TxPlanner<X::Budget>,
        ladder: X::Ladder,
        entropy: X::Entropy,
        config: PortClientConfig,
    ) -> Result<Self, AttachError> {
        let attachment = router.attach(config.vif, config.role, config.address)?;
        Ok(Self {
            router,
            attachment,
            tx: UpperMacTx::new(router, config.vif, planner),
            ladder,
            entropy,
            config,
            counters: PortClientCounters::default(),
        })
    }

    pub fn port(&self) -> &'p X::Port {
        self.router.port()
    }

    pub const fn router(&self) -> &'p EventRouter<'p, X::Port, EXCHANGES, RX> {
        self.router
    }

    pub const fn config(&self) -> &PortClientConfig {
        &self.config
    }

    pub const fn counters(&self) -> PortClientCounters {
        self.counters
    }

    pub fn tx_mut(&mut self) -> &mut UpperMacTx<'p, 'p, X::Port, X::Budget, EXCHANGES, RX> {
        &mut self.tx
    }

    /// Send one MPDU, header to end of body, at `coex` until the planner
    /// reports the exchange's end. A group-addressed frame solicits no
    /// response.
    pub async fn transmit(
        &mut self,
        frame: TxMpdu<'_, PortBody<X>>,
        key: KeySelector,
        access_category: WmmAccessCategory,
        rate: PhyRate,
        coex: CoexPriority,
    ) -> Result<TxReport, PortClientError<PortError<X>>> {
        let address1: [u8; 6] = frame
            .header
            .get(4..10)
            .and_then(|address| address.try_into().ok())
            .ok_or(PortClientError::FrameTooShort)?;
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
                length: (frame.header.len()
                    + frame
                        .body
                        .as_ref()
                        .map_or(0, |body| oer_ieee80211_lower_mac::TxBody::bytes(body).len()))
                    as u32
                    + FCS_LEN
                    + mic,
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
            Err(UpperMacTxError::Poisoned) => Err(PortClientError::Poisoned),
            Err(error) => Err(PortClientError::Tx(error)),
        }
    }

    /// Send one A-MPDU until the planner reports the exchange's end.
    pub async fn transmit_ampdu(
        &mut self,
        frames: AmpduFrames<'_, PortBody<X>>,
        request: TxRequest,
    ) -> Result<TxReport, PortClientError<PortError<X>>>
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
            Err(UpperMacTxError::Poisoned) => Err(PortClientError::Poisoned),
            Err(error) => Err(PortClientError::Tx(error)),
        }
    }

    /// Reserve the interface's air for `duration` with a CTS-to-self on the
    /// voice queue at `rate`, and wait until it went out.
    pub async fn reserve_air(
        &mut self,
        duration: oer_time::Duration,
        rate: PhyRate,
        coex: CoexPriority,
    ) -> Result<TxCompletion, PortClientError<PortError<X>>>
    where
        X::Port: LowerMacAirReservation,
    {
        match self
            .tx
            .reserve_air(
                duration,
                WmmAccessCategory::Voice,
                rate,
                self.config.power,
                coex,
            )
            .await
        {
            Ok(completion) => Ok(completion),
            Err(UpperMacTxError::Poisoned) => Err(PortClientError::Poisoned),
            Err(error) => Err(PortClientError::Tx(error)),
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
            let vif = self.config.vif;
            match pin!(router.extension(vif)).poll(context) {
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
            return match pin!(router.received(vif)).poll(context) {
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

    /// A received frame in the port's buffer; `None` for any other event.
    fn frame(
        &mut self,
        event: <X::Port as Ieee80211LowerMacPort>::Event,
    ) -> Option<PortInput<PortRxBuffer<X>>> {
        match <X::Port as Ieee80211LowerMacPort>::into_received(event) {
            Ok((buffer, meta)) => Some(PortInput::Frame(PortFrame::new(buffer, meta))),
            Err(_) => None,
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
    pub fn apply(&self, setting: LowerMacSetting) -> Result<(), PortClientError<PortError<X>>> {
        self.port()
            .apply(setting)
            .map_err(PortClientError::Port)?
            .map_err(PortClientError::Setting)
    }

    /// Contend with a BSS's EDCA parameters: the port's queues take their
    /// AIFSN and TXOP limits, the transmit planner each access category's
    /// contention window.
    pub fn install_edca(
        &mut self,
        parameters: oer_ieee80211_mac::extensions::wmm::WmmParameterSet,
    ) -> Result<(), PortClientError<PortError<X>>> {
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

    /// Configure the client's interface with `bssid` and `receive`.
    pub fn configure(
        &self,
        bssid: Option<MacAddress>,
        receive: ReceiveFilter,
    ) -> Result<(), PortClientError<PortError<X>>> {
        self.apply(LowerMacSetting::Vif {
            vif: self.config.vif,
            config: Some(VifConfig {
                address: self.config.address,
                role: self.config.role,
                bssid,
                receive,
            }),
        })?;
        // The station's frames of its BSS are routed to it from now on.
        self.attachment.set_bssid(bssid);
        Ok(())
    }

    /// Tune to `channel`. A backend that retunes only while disabled
    /// refuses the setting as `Busy`; the client then disables the port,
    /// tunes and enables it again.
    pub async fn retune(&mut self, channel: Channel) -> Result<(), PortClientError<PortError<X>>> {
        match self.apply(LowerMacSetting::Channel(channel)) {
            Err(PortClientError::Setting(SettingError::Busy)) => {
                self.lifecycle(LifecycleCommand::Disable).await?;
                self.apply(LowerMacSetting::Channel(channel))?;
                self.lifecycle(LifecycleCommand::Enable).await
            }
            result => result,
        }
    }

    /// Run one lifecycle command to its terminal event, which the router
    /// queues for the client.
    pub async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> Result<(), PortClientError<PortError<X>>> {
        self.port()
            .lifecycle(command)
            .map_err(PortClientError::Port)?
            .map_err(PortClientError::Lifecycle)?;
        match self.router.lifecycle().await {
            Some(Ok(LifecycleEvent::Failed { class, .. })) => {
                Err(PortClientError::LifecycleFailed(class))
            }
            Some(Ok(_)) => Ok(()),
            // A lifecycle command has no identity to cancel: its terminal
            // may be in the gap.
            Some(Err(EventsLost)) => {
                self.counters.events_lost = self.counters.events_lost.saturating_add(1);
                Err(PortClientError::LifecycleLost)
            }
            None => Err(PortClientError::Poisoned),
        }
    }
}
