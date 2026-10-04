//! The access point over the lower-MAC port.
//!
//! A [`PortAccessPoint`] owns its interface's
//! [`PortClient`](oer_ieee80211_upper_mac_service::client::PortClient), its
//! beacon and the [`AccessPointService`] of its BSS (peers, security and
//! the management sequence beacons and responses share).
//! [`PortAccessPoint::start`] tunes the port to the BSS's channel,
//! configures the access-point interface to receive its BSS and the Probe
//! Requests it answers, and restarts the interface's TSF.
//! [`PortAccessPoint::run_until`] then:
//!
//! - publishes a beacon at every TBTT of its own schedule (`ApBeacon`: an
//!   absolute cursor that a late publication does not move), its ERP and HT
//!   protection following the associated peers;
//! - answers each Probe Request for its SSID, or for any SSID, with the
//!   current advertisement;
//! - admits stations: Open System authentication and association, a
//!   repeated request answered again without resetting the peer;
//! - removes a peer that disassociates or deauthenticates, and closes one
//!   whose authentication or association went inactive with a
//!   Disassociation (when it was associated) and a Deauthentication;
//! - in a WPA2-Personal BSS, runs the four-way handshake as the
//!   authenticator: Message 1 after a successful association, Message 3 on
//!   a verified Message 2, their retransmissions, and on a verified
//!   Message 4 the peer's pairwise key installed in the port before the
//!   peer is authorized. [`PortAccessPoint::start`] installs the group key.
//!   A peer's pairwise key is removed whenever the peer is.
//!
//! - in a WPA3-Personal BSS, authenticates stations by SAE through the
//!   environment's [`PortApSae`] executor, which the composition places
//!   (another task, another core, or inline with [`InlineSae`]): the access
//!   point hands each SAE frame on and goes on serving its BSS until the
//!   output is ready, as the vendor runs its responder in a task of its own.
//!   The IGTK reaches stations in Message 3; the port holds only the group
//!   and pairwise keys.
//!
//! The composition enables the port and polls the router beside the access
//! point.

use core::{
    future::poll_fn,
    pin::pin,
    task::{Context, Poll},
};

use oer_ieee80211_ap::{
    AP_MAX_CLIENTS, AccessPointService, ApAssociationCapabilities, ApMlmeAction, ApPeerClose,
    ApPeerCloseKind, ApPeerPhase, ApServiceError, ApWpa2Error, ApWpa2Progress, ApWpa2RetryProgress,
    beacon::ApBeacon,
    limits::AP_TIM_VIRTUAL_BITMAP_OCTETS,
    sae::{ApSaeFrame, ApSaeOutput, ApSaeRandom, ApSaeResponder, ApSaeResult},
};
use oer_ieee80211_lower_mac::{
    Channel, Cipher, CoexPriority, Ieee80211LowerMacPort, KeyHandle, KeyInstall, KeyScope,
    KeySelector, LowerMacBeaconTiming, LowerMacSetting, PhyRate, ReceiveFilter, SettingError,
    VifTsf,
};
use oer_ieee80211_mac::{
    ap::{
        ApAssociationResponseError, ApDataFrame, ApDataFrameError, ApManagementRequest,
        ApPeerDisconnectKind, parse_ap_management_request, probe, probe::ResponseError,
        profile::Advertisement, write_ap_peer_disconnect,
        write_ht_association_response_frame_for_security, write_open_authentication_response,
        write_sae_authentication,
    },
    beacon::{AP_BEACON_CAPACITY, ApBeaconBuildError, TimBitmapError, TimVirtualBitmap},
    channel::WifiChannel,
    data::{
        DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN,
        plan_data_decapsulation,
    },
    protection::ApBssProtection,
    qos::WmmAccessCategory,
    security::{ApSecurityPolicy, LinkProtection},
    sequence::SequenceNumber,
    ssid::WifiSsid,
    tsf::TsfInstant,
};
use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk, RsnInterface, frames::RsnTxFrame, runner::RSN_HANDSHAKE_EAPOL_CAPACITY,
};
use oer_ieee80211_upper_mac::TxReport;
use oer_ieee80211_upper_mac_service::{
    EventRouter,
    client::{PortClient, PortClientEnv, PortClientError, PortError, PortInput},
};
use oer_time::{Clock, Duration, Instant, Timer};

/// Exchanges of the access point that wait for a completion at once.
pub const PORT_AP_EXCHANGES: usize = 2;
/// Received frames the router keeps for the access point.
pub const PORT_AP_BACKLOG: usize = 4;

/// Probe Responses go out at most once per this interval, whoever asks: a
/// sender that changes its address gains no more air.
const PROBE_RESPONSE_INTERVAL: Duration = Duration::from_millis(10);

/// The reason of a Disassociation for inactivity (IEEE Std 802.11-2020
/// Table 9-90, `DISASSOC_DUE_TO_INACTIVITY`).
const REASON_INACTIVITY: u16 = 4;
/// The reason of every other access-point teardown: the previous
/// authentication is no longer valid.
const REASON_AUTHENTICATION_INVALID: u16 = 2;
/// The EtherType of EAPOL.
const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// An EAPOL-Key frame of the four-way handshake.
type EapolFrame = RsnTxFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>;

/// The event router of an access point's port.
pub type PortApRouter<'p, X> =
    EventRouter<'p, <X as PortClientEnv>::Port, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The access point's client of the port.
pub type PortApClient<'p, X> = PortClient<'p, X, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The types an access point over the port is built from: its port and
/// transmit policy ([`PortClientEnv`]), the image's monotonic time and the
/// source of each handshake's authenticator material.
pub trait PortApEnv: PortClientEnv {
    type Timer: Timer;
    type Authenticator: PortApAuthenticator;
    /// Where a WPA3 BSS's SAE responder runs; [`NoSae`] for another BSS.
    type Sae: PortApSae;
}

/// The executor of an access point's SAE responder: the access point hands
/// it one SAE frame at a time and takes the output when it is ready, so the
/// elliptic-curve work of a Commit runs where the composition places it.
pub trait PortApSae {
    /// Take one received SAE frame; `false` while the output of the last
    /// one was not taken, and the frame is dropped (the station repeats
    /// it).
    fn submit(&mut self, frame: ApSaeFrame<'_>, now: Instant) -> bool;
    /// The station and output of the frame taken, once ready.
    fn poll_output(&mut self, context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)>;
    /// Drop the session of a station the access point removed.
    fn forget(&mut self, peer: [u8; 6]);
}

/// No SAE: the executor of an Open or WPA2 BSS.
pub struct NoSae;

impl PortApSae for NoSae {
    fn submit(&mut self, _frame: ApSaeFrame<'_>, _now: Instant) -> bool {
        false
    }

    fn poll_output(&mut self, _context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)> {
        Poll::Pending
    }

    fn forget(&mut self, _peer: [u8; 6]) {}
}

/// The responder run inline: a submitted frame's work is done before
/// `submit` returns, and its output is ready at the next poll.
pub struct InlineSae<R> {
    responder: ApSaeResponder,
    random: R,
    ready: Option<([u8; 6], ApSaeOutput)>,
}

impl<R: ApSaeRandom> InlineSae<R> {
    pub const fn new(responder: ApSaeResponder, random: R) -> Self {
        Self {
            responder,
            random,
            ready: None,
        }
    }
}

impl<R: ApSaeRandom> PortApSae for InlineSae<R> {
    fn submit(&mut self, frame: ApSaeFrame<'_>, now: Instant) -> bool {
        if self.ready.is_some() {
            return false;
        }
        // Nothing waits in a queue: the frame is served at once.
        let output = self.responder.receive(frame, now, 0, &mut self.random);
        self.ready = Some((frame.peer, output));
        true
    }

    fn poll_output(&mut self, _context: &mut Context<'_>) -> Poll<([u8; 6], ApSaeOutput)> {
        self.ready.take().map_or(Poll::Pending, Poll::Ready)
    }

    fn forget(&mut self, peer: [u8; 6]) {
        self.responder.forget(peer);
    }
}

/// What the access point waits for between its deadlines.
#[expect(
    clippy::large_enum_variant,
    reason = "no_std without an allocator: one SAE output moves by value, once"
)]
enum Wake<B> {
    Input(Option<PortInput<B>>),
    Sae([u8; 6], ApSaeOutput),
}

/// The fresh material of each WPA2 four-way handshake the access point
/// starts.
pub trait PortApAuthenticator {
    /// The authenticator's nonce, unpredictable, and the initial EAPOL-Key
    /// replay counter.
    fn handshake_material(&mut self) -> ([u8; 32], u64);
}

/// The BSS an access point runs; its security is its service's.
#[derive(Clone, Copy, Debug)]
pub struct PortApProfile<'a> {
    pub ssid: &'a WifiSsid,
    pub channel: WifiChannel,
    pub beacon_interval_tu: u16,
    pub dtim_period: u8,
    /// The rates, HT capabilities and WMM parameters the access point
    /// claims.
    pub advertisement: &'a Advertisement,
    /// The rate of beacons and management frames.
    pub management_rate: PhyRate,
    pub coex: CoexPriority,
}

/// What the access point sent, admitted and ignored.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortApCounters {
    pub beacons: u32,
    pub probe_responses: u32,
    /// Probe Requests answered by none: another SSID, or inside the
    /// response interval.
    pub probes_ignored: u32,
    /// Authentication Responses sent, successful or not.
    pub authentications: u32,
    /// Association Responses sent, successful or not.
    pub associations: u32,
    /// Peers that disassociated or deauthenticated.
    pub peers_left: u32,
    /// Peers the access point closed.
    pub peers_closed: u32,
    /// Management requests the access point does not serve: SAE outside a
    /// WPA3 BSS, Block Ack actions.
    pub unserved: u32,
    /// EAPOL-Key frames sent, retransmissions included.
    pub eapol_sent: u32,
    /// SAE frames handed to the executor.
    pub sae_frames: u32,
    /// SAE frames dropped while the executor was busy.
    pub sae_dropped: u32,
    /// Stations SAE authenticated.
    pub sae_accepted: u32,
    /// Peers authorized by a completed four-way handshake.
    pub handshakes: u32,
}

/// Why an access point could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortApBuildError {
    Beacon(ApBeaconBuildError),
    /// The service's address is not the client's interface address.
    AddressMismatch,
}

/// Why an access-point operation failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortApError<E> {
    Client(PortClientError<E>),
    /// The beacon template could not be stamped or protected.
    Beacon,
    Tim(TimBitmapError),
    Probe(ResponseError),
    Response(ApAssociationResponseError),
    Service(ApServiceError),
    Wpa2(ApWpa2Error),
    Data(ApDataFrameError),
    /// The port refused the interface's TSF restart.
    Tsf(SettingError),
    /// The port refused a key or its removal.
    Key(SettingError),
    /// The port holds the keys of no more peers.
    KeysFull,
}

impl<E> From<ApWpa2Error> for PortApError<E> {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

impl<E> From<ApDataFrameError> for PortApError<E> {
    fn from(error: ApDataFrameError) -> Self {
        Self::Data(error)
    }
}

impl<E> From<PortClientError<E>> for PortApError<E> {
    fn from(error: PortClientError<E>) -> Self {
        Self::Client(error)
    }
}

impl<E> From<ApAssociationResponseError> for PortApError<E> {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Response(error)
    }
}

impl<E> From<ApServiceError> for PortApError<E> {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

/// One access point over the port: see the [module](self).
pub struct PortAccessPoint<'p, X: PortApEnv> {
    client: PortApClient<'p, X>,
    timer: X::Timer,
    authenticator: X::Authenticator,
    sae: X::Sae,
    profile: PortApProfile<'p>,
    beacon: ApBeacon<'p>,
    service: AccessPointService<'p>,
    /// The group key the port holds, in a protected BSS.
    group_key: Option<KeyHandle>,
    /// The pairwise key the port holds for each authorized peer.
    pairwise_keys: [Option<([u8; 6], KeyHandle)>; AP_MAX_CLIENTS],
    /// The protection the beacon template carries.
    advertised: ApBssProtection,
    /// Before it no Probe Response goes out.
    next_probe_response: Instant,
    counters: PortApCounters,
}

impl<'p, X: PortApEnv> PortAccessPoint<'p, X> {
    /// An access point of `profile` and `service` over `client`, its beacon
    /// template in `storage`.
    pub fn new(
        client: PortApClient<'p, X>,
        timer: X::Timer,
        authenticator: X::Authenticator,
        sae: X::Sae,
        profile: PortApProfile<'p>,
        service: AccessPointService<'p>,
        storage: &'p mut [u8; AP_BEACON_CAPACITY],
    ) -> Result<Self, PortApBuildError> {
        if service.address() != client.config().address {
            return Err(PortApBuildError::AddressMismatch);
        }
        let beacon = ApBeacon::new(
            storage,
            profile.advertisement,
            client.config().address,
            profile.ssid,
            profile.channel,
            profile.beacon_interval_tu,
            profile.dtim_period,
            SequenceNumber::ZERO,
            service.security_policy(),
        )
        .map_err(PortApBuildError::Beacon)?;
        Ok(Self {
            client,
            timer,
            authenticator,
            sae,
            profile,
            beacon,
            service,
            group_key: None,
            pairwise_keys: [None; AP_MAX_CLIENTS],
            advertised: ApBssProtection::default(),
            next_probe_response: Instant::EPOCH,
            counters: PortApCounters::default(),
        })
    }

    pub const fn counters(&self) -> PortApCounters {
        self.counters
    }

    pub const fn client(&self) -> &PortApClient<'p, X> {
        &self.client
    }

    /// The BSS's peers and security.
    pub const fn service(&self) -> &AccessPointService<'p> {
        &self.service
    }

    /// Tune to the BSS's channel, receive the BSS and the Probe Requests
    /// the access point answers, restart the interface's TSF and, in a
    /// protected BSS, install the group key.
    pub async fn start(&mut self) -> Result<(), PortApError<PortError<X>>> {
        self.client
            .retune(Channel::from_wifi_channel(self.profile.channel))
            .await?;
        self.client.configure(
            None,
            ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
        )?;
        let vif = self.client.config().vif;
        self.client
            .port()
            .set_tsf(VifTsf {
                vif,
                at: TsfInstant::from_micros(0),
            })
            .map_err(PortClientError::Port)?
            .map_err(PortApError::Tsf)?;
        if self.service.link_protection() == LinkProtection::Ccmp && self.group_key.is_none() {
            let gtk = self.service.gtk()?;
            let (key_id, key) = (gtk.key_id(), *gtk.key());
            self.group_key = Some(self.install_key(KeyScope::Group { key_id }, &key)?);
        }
        Ok(())
    }

    fn install_key(
        &self,
        scope: KeyScope,
        key: &[u8],
    ) -> Result<KeyHandle, PortApError<PortError<X>>> {
        self.client
            .port()
            .install_key(KeyInstall {
                vif: self.client.config().vif,
                cipher: Cipher::Ccmp128,
                scope,
                key,
            })
            .map_err(PortClientError::Port)?
            .map_err(PortApError::Key)
    }

    /// Remove `peer`'s pairwise key from the port, if it holds one.
    fn remove_pairwise_key(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        let Some(slot) = self
            .pairwise_keys
            .iter_mut()
            .find(|slot| slot.is_some_and(|(address, _)| address == peer))
        else {
            return Ok(());
        };
        let Some((_, handle)) = slot.take() else {
            return Ok(());
        };
        self.client
            .apply(LowerMacSetting::RemoveKey(handle))
            .map_err(|error| match error {
                PortClientError::Setting(error) => PortApError::Key(error),
                error => PortApError::Client(error),
            })
    }

    /// Forget a peer: its pairwise key, its SAE session, then its state.
    fn remove_peer(&mut self, peer: [u8; 6]) -> Result<(), PortApError<PortError<X>>> {
        self.remove_pairwise_key(peer)?;
        self.sae.forget(peer);
        self.service.remove_peer(peer)?;
        Ok(())
    }

    /// Serve the BSS until `deadline`: a beacon at every TBTT, a response
    /// to every management request it answers, a close of every peer that
    /// went inactive.
    pub async fn run_until(&mut self, deadline: Instant) -> Result<(), PortApError<PortError<X>>> {
        loop {
            let now = self.timer.now();
            if self.beacon.publication_due(now) {
                self.publish_beacon(now).await?;
                continue;
            }
            if let Some(close) = self.service.begin_due_peer_close(now) {
                self.close_peer(close).await?;
                continue;
            }
            match self
                .service
                .take_due_wpa2_retry::<RSN_HANDSHAKE_EAPOL_CAPACITY>(now)?
            {
                ApWpa2RetryProgress::Transmit { peer, frame } => {
                    self.send_eapol(peer, &frame, true).await?;
                    continue;
                }
                ApWpa2RetryProgress::Close(close) => {
                    self.close_peer(close).await?;
                    continue;
                }
                ApWpa2RetryProgress::None => {}
            }
            if now >= deadline {
                return Ok(());
            }
            let wake = [
                self.beacon.next_publication(),
                self.service.next_peer_deadline(),
                self.service.next_wpa2_retry_deadline(),
            ]
            .into_iter()
            .flatten()
            .fold(deadline, Instant::min);
            let woken = {
                let Self {
                    client, timer, sae, ..
                } = self;
                let mut input = pin!(client.next_input(&*timer, wake));
                poll_fn(|context| {
                    if let Poll::Ready((peer, output)) = sae.poll_output(context) {
                        return Poll::Ready(Wake::Sae(peer, output));
                    }
                    input.as_mut().poll(context).map(Wake::Input)
                })
                .await
            };
            match woken {
                Wake::Sae(peer, output) => self.sae_output(peer, output, now).await?,
                Wake::Input(Some(PortInput::Frame(frame))) => self.receive(frame.bytes()).await?,
                Wake::Input(Some(PortInput::Poisoned)) => {
                    return Err(PortApError::Client(PortClientError::Poisoned));
                }
                Wake::Input(Some(PortInput::Tbtt(_) | PortInput::EventsLost) | None) => {}
            }
        }
    }

    /// The protection the associated peers require of the BSS.
    fn required_protection(&self) -> ApBssProtection {
        self.service
            .bss_protection(self.profile.channel.bandwidth_mhz() >= 40)
    }

    /// Carry the protection the peers require in the beacon template, which
    /// probe responses repeat.
    fn advertise_current_protection(&mut self) -> Result<(), PortApError<PortError<X>>> {
        let required = self.required_protection();
        if required != self.advertised {
            self.beacon
                .set_bss_protection(required)
                .map_err(|_| PortApError::Beacon)?;
            self.advertised = required;
        }
        Ok(())
    }

    /// Send one management frame to a peer, acknowledged, on the voice
    /// queue at the management rate.
    async fn send_management(&mut self, frame: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        self.client
            .transmit(
                frame,
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        Ok(())
    }

    /// Stamp the beacon and send it once, unacknowledged, on the voice
    /// queue.
    async fn publish_beacon(&mut self, now: Instant) -> Result<(), PortApError<PortError<X>>> {
        self.advertise_current_protection()?;
        let sequence = self.service.next_management_sequence();
        let bitmap = TimVirtualBitmap::<AP_TIM_VIRTUAL_BITMAP_OCTETS>::try_new()
            .map_err(PortApError::Tim)?;
        let Self {
            client,
            beacon,
            profile,
            counters,
            ..
        } = self;
        let frame = beacon
            .prepare(now, sequence, false, bitmap.partial())
            .ok_or(PortApError::Beacon)?;
        client
            .transmit(
                frame,
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                profile.management_rate,
                profile.coex,
            )
            .await?;
        counters.beacons = counters.beacons.saturating_add(1);
        Ok(())
    }

    async fn receive(&mut self, frame: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        let now = self.timer.now();
        // Any frame of an associated peer keeps it; an authentication that
        // never associates ends at its own deadline.
        if let Some(sender) = frame
            .get(10..16)
            .and_then(|address| address.try_into().ok())
            && self.service.peer_status(sender).is_some_and(|status| {
                matches!(
                    status.phase,
                    ApPeerPhase::Securing | ApPeerPhase::Authorized
                )
            })
        {
            self.service.observe_activity(sender, now)?;
        }
        // Data: only EAPOL of a peer in its handshake is served yet.
        if frame
            .first()
            .is_some_and(|control| (control >> 2) & 0b11 == 2)
        {
            return self.receive_eapol(frame, now).await;
        }
        let address = self.client.config().address;
        let retry = frame.get(1).is_some_and(|flags| flags & 0x08 != 0);
        match parse_ap_management_request(self.profile.advertisement, frame, address) {
            Some(ApManagementRequest::Probe { peer, ssid }) => self.probe(peer, ssid).await,
            Some(ApManagementRequest::OpenAuthentication { peer }) => {
                self.authenticate(peer, retry, now).await
            }
            Some(ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
                short_preamble,
            }) => {
                let capabilities = ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps,
                    short_preamble,
                    ht: ht_capabilities,
                    qos_supported,
                };
                self.associate(peer, security, capabilities, now).await
            }
            Some(
                ApManagementRequest::Disassociation { peer, .. }
                | ApManagementRequest::Deauthentication { peer, .. },
            ) => {
                // A peer the access point is closing ends by its own
                // teardown.
                if self
                    .service
                    .peer_status(peer)
                    .is_some_and(|status| status.phase != ApPeerPhase::Closing)
                {
                    self.remove_peer(peer)?;
                    self.counters.peers_left = self.counters.peers_left.saturating_add(1);
                }
                Ok(())
            }
            Some(ApManagementRequest::SaeAuthentication {
                peer,
                transaction,
                status,
                body,
            }) if self.service.security_policy() == ApSecurityPolicy::Wpa3Personal => {
                let frame = ApSaeFrame {
                    peer,
                    transaction,
                    status,
                    body,
                };
                if self.sae.submit(frame, now) {
                    self.counters.sae_frames = self.counters.sae_frames.saturating_add(1);
                } else {
                    self.counters.sae_dropped = self.counters.sae_dropped.saturating_add(1);
                }
                Ok(())
            }
            Some(
                ApManagementRequest::SaeAuthentication { .. }
                | ApManagementRequest::BlockAck { .. },
            ) => {
                self.counters.unserved = self.counters.unserved.saturating_add(1);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Answer a Probe Request for the BSS's SSID, or any SSID, with the
    /// current advertisement, at most once per [`PROBE_RESPONSE_INTERVAL`].
    async fn probe(&mut self, peer: [u8; 6], ssid: &[u8]) -> Result<(), PortApError<PortError<X>>> {
        let now = self.timer.now();
        if !probe::matches_ssid(self.beacon.advertisement(), ssid) || now < self.next_probe_response
        {
            self.counters.probes_ignored = self.counters.probes_ignored.saturating_add(1);
            return Ok(());
        }
        self.advertise_current_protection()?;
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = probe::write_response(
            self.beacon.advertisement(),
            peer,
            sequence,
            now.as_micros(),
            &mut response,
        )
        .map_err(PortApError::Probe)?;
        self.next_probe_response = now.saturating_add(PROBE_RESPONSE_INTERVAL);
        self.send_management(&response[..length]).await?;
        self.counters.probe_responses = self.counters.probe_responses.saturating_add(1);
        Ok(())
    }

    /// Apply one SAE output: an accepted exchange authenticates the station
    /// with its PMK (ending an earlier pairwise-key epoch), a failed one
    /// forgets a station that never associated; then the replies go out in
    /// order.
    async fn sae_output(
        &mut self,
        peer: [u8; 6],
        output: ApSaeOutput,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        match output.result {
            ApSaeResult::Accepted { pmk, pmkid } => {
                if self.service.peer_status(peer).is_some() {
                    self.remove_pairwise_key(peer)?;
                }
                self.service
                    .authenticate_sae(peer, Pmk::from_bytes(pmk), pmkid, now)?;
                self.counters.sae_accepted = self.counters.sae_accepted.saturating_add(1);
            }
            ApSaeResult::Failed { .. } => {
                if self
                    .service
                    .peer_status(peer)
                    .is_some_and(|status| status.phase == ApPeerPhase::Authenticated)
                {
                    self.remove_peer(peer)?;
                }
            }
            ApSaeResult::Continue => {}
        }
        for reply in output.replies() {
            let sequence = self.service.next_management_sequence();
            let mut frame = [0; AP_BEACON_CAPACITY];
            let length = write_sae_authentication(
                &mut frame,
                self.service.address(),
                reply.peer,
                reply.transaction,
                reply.status,
                reply.body(),
                sequence,
            )?;
            self.send_management(&frame[..length]).await?;
        }
        Ok(())
    }

    /// Answer an Open System authentication. A retransmission from a peer
    /// already past authentication is answered again without resetting it
    /// (its response's acknowledgement was lost); a new authentication
    /// starts the peer over.
    async fn authenticate(
        &mut self,
        peer: [u8; 6],
        retry: bool,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let repeated = retry
            && self
                .service
                .peer_status(peer)
                .is_some_and(|status| status.phase != ApPeerPhase::Authenticated);
        let status = if repeated {
            0
        } else {
            // A new authentication ends the peer's earlier pairwise-key
            // epoch before the service starts it over.
            self.remove_pairwise_key(peer)?;
            let ApMlmeAction::AuthenticationResponse { status, .. } =
                self.service.authenticate_open(peer, now)
            else {
                return Err(PortApError::Service(ApServiceError::WrongPeerPhase));
            };
            status
        };
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = write_open_authentication_response(
            &mut response,
            self.service.address(),
            peer,
            status,
            sequence,
        )?;
        self.send_management(&response[..length]).await?;
        self.counters.authentications = self.counters.authentications.saturating_add(1);
        Ok(())
    }

    /// Answer an association of an authenticated peer; a repeated request
    /// of an associated peer (in its handshake, or authorized in an Open
    /// BSS) is answered again with its association and starts no second
    /// handshake. A successful association in a protected BSS begins the
    /// four-way handshake.
    async fn associate(
        &mut self,
        peer: [u8; 6],
        security: oer_ieee80211_mac::ap::ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let Some(status) = self.service.peer_status(peer) else {
            return Ok(());
        };
        let protected = self.service.link_protection() == LinkProtection::Ccmp;
        let repeated = self.service.matches_association_security(security)
            && (status.phase == ApPeerPhase::Securing
                || (!protected && status.phase == ApPeerPhase::Authorized));
        let (result, association_id, ht) = if repeated {
            (0, status.association_id, status.ht)
        } else if status.phase == ApPeerPhase::Authenticated {
            let action = if protected {
                let (nonce, replay_counter) = self.authenticator.handshake_material();
                self.service.associate_rsn(
                    peer,
                    security,
                    capabilities,
                    nonce,
                    replay_counter,
                    now,
                )?
            } else {
                self.service
                    .associate_open(peer, security, capabilities, now)?
            };
            let ApMlmeAction::AssociationResponse {
                status,
                association_id,
                ..
            } = action
            else {
                return Err(PortApError::Service(ApServiceError::WrongPeerPhase));
            };
            (status, association_id.unwrap_or(0), capabilities.ht)
        } else {
            return Ok(());
        };
        let sequence = self.service.next_management_sequence();
        let mut response = [0; AP_BEACON_CAPACITY];
        let length = write_ht_association_response_frame_for_security(
            self.profile.advertisement,
            &mut response,
            self.service.address(),
            peer,
            result,
            association_id,
            sequence,
            self.profile.channel,
            ht,
            self.service.security_policy(),
            self.required_protection(),
        )?;
        self.send_management(&response[..length]).await?;
        self.counters.associations = self.counters.associations.saturating_add(1);
        if protected && !repeated && association_id != 0 {
            let message1: EapolFrame = self.service.begin_wpa2_frame(peer)?;
            self.send_eapol(peer, &message1, false).await?;
        }
        Ok(())
    }

    /// Send one EAPOL-Key frame to a peer in its handshake, unprotected and
    /// acknowledged, and arm its retransmission.
    async fn send_eapol(
        &mut self,
        peer: [u8; 6],
        frame: &EapolFrame,
        retransmission: bool,
    ) -> Result<(), PortApError<PortError<X>>> {
        let mut mpdu = [0; RSN_HANDSHAKE_EAPOL_CAPACITY + 64];
        let sequence_number = self.service.current_data_sequence();
        let length = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: EAPOL_ETHER_TYPE,
            payload: frame.as_bytes(),
        }
        .encode(&mut mpdu)?;
        // The sequence number is spent only once the frame encoded.
        self.service.next_data_sequence();
        let report = self
            .client
            .transmit(
                &mpdu[..length],
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        let acknowledged =
            matches!(report, TxReport::Mpdu(status) if status.acknowledged == Some(true));
        self.counters.eapol_sent = self.counters.eapol_sent.saturating_add(1);
        self.service
            .observe_wpa2_transmit(peer, retransmission, acknowledged, self.timer.now())?;
        Ok(())
    }

    /// Serve one EAPOL-Key frame of a peer in its handshake: answer
    /// Message 2 with Message 3; on Message 4 install the peer's pairwise
    /// key and authorize it; close a peer the handshake rejected. Every
    /// other data frame, and EAPOL of a peer outside its handshake, is
    /// ignored.
    async fn receive_eapol(
        &mut self,
        mpdu: &[u8],
        now: Instant,
    ) -> Result<(), PortApError<PortError<X>>> {
        let header = if mpdu[0] & 0x80 != 0 {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let Some(length) = mpdu.len().checked_sub(header) else {
            return Ok(());
        };
        let Ok(plan) =
            plan_data_decapsulation(DataInterfaceRole::AccessPoint, mpdu, header, length)
        else {
            return Ok(());
        };
        let peer = plan.source;
        if plan.ether_type != EAPOL_ETHER_TYPE
            || plan.destination != self.service.address()
            || !self
                .service
                .peer_status(peer)
                .is_some_and(|status| status.phase == ApPeerPhase::Securing)
        {
            return Ok(());
        }
        let Some(payload) =
            mpdu.get(plan.payload_offset..plan.payload_offset + plan.payload_length)
        else {
            return Ok(());
        };
        let Ok(frame) = OwnedEapolFrame::<RSN_HANDSHAKE_EAPOL_CAPACITY>::try_copy(
            RsnInterface::AccessPoint,
            peer,
            payload,
        ) else {
            return Ok(());
        };
        match self.service.on_eapol(peer, frame)? {
            ApWpa2Progress::None => Ok(()),
            ApWpa2Progress::Transmit(frame) => self.send_eapol(peer, &frame, false).await,
            ApWpa2Progress::AuthorizePeer => self.authorize(peer, now),
            ApWpa2Progress::DeauthenticatePeer => {
                let close = self.service.begin_wpa2_failure_close(peer)?;
                self.close_peer(close).await
            }
        }
    }

    /// Install the verified handshake's pairwise key in the port, then open
    /// the peer's controlled port.
    fn authorize(&mut self, peer: [u8; 6], now: Instant) -> Result<(), PortApError<PortError<X>>> {
        let Some(slot) = self.pairwise_keys.iter().position(Option::is_none) else {
            return Err(PortApError::KeysFull);
        };
        let key = *self.service.pending_ptk(peer)?.temporal_key();
        let handle = self.install_key(KeyScope::Pairwise { peer }, &key)?;
        self.pairwise_keys[slot] = Some((peer, handle));
        self.service.authorize(peer, now)?;
        self.counters.handshakes = self.counters.handshakes.saturating_add(1);
        Ok(())
    }

    /// Close a peer the service began closing: a Disassociation when it was
    /// associated, then a Deauthentication, then its removal.
    async fn close_peer(&mut self, close: ApPeerClose) -> Result<(), PortApError<PortError<X>>> {
        if close.was_associated {
            let reason = if close.kind == ApPeerCloseKind::InactivityTimeout {
                REASON_INACTIVITY
            } else {
                REASON_AUTHENTICATION_INVALID
            };
            self.send_disconnect(close.peer, ApPeerDisconnectKind::Disassociation, reason)
                .await?;
        }
        self.send_disconnect(
            close.peer,
            ApPeerDisconnectKind::Deauthentication,
            REASON_AUTHENTICATION_INVALID,
        )
        .await?;
        self.remove_peer(close.peer)?;
        self.counters.peers_closed = self.counters.peers_closed.saturating_add(1);
        Ok(())
    }

    async fn send_disconnect(
        &mut self,
        peer: [u8; 6],
        kind: ApPeerDisconnectKind,
        reason: u16,
    ) -> Result<(), PortApError<PortError<X>>> {
        let sequence = self.service.next_management_sequence();
        let mut frame = [0; AP_BEACON_CAPACITY];
        let length = write_ap_peer_disconnect(
            &mut frame,
            self.service.address(),
            peer,
            kind,
            reason,
            sequence,
        )?;
        self.send_management(&frame[..length]).await
    }
}
