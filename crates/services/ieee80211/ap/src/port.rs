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
//!   Disassociation (when it was associated) and a Deauthentication.
//!
//! An RSN BSS (its WPA2 handshake and pairwise keys) and SAE are not served
//! yet: [`PortAccessPoint::new`] refuses an RSN service. The composition
//! enables the port and polls the router beside the access point.

use oer_ieee80211_ap::{
    AccessPointService, ApAssociationCapabilities, ApMlmeAction, ApPeerClose, ApPeerCloseKind,
    ApPeerPhase, ApServiceError, beacon::ApBeacon, limits::AP_TIM_VIRTUAL_BITMAP_OCTETS,
};
use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, KeySelector, LowerMacBeaconTiming, PhyRate, ReceiveFilter, SettingError,
    VifTsf,
};
use oer_ieee80211_mac::{
    ap::{
        ApAssociationResponseError, ApManagementRequest, ApPeerDisconnectKind,
        parse_ap_management_request, probe, probe::ResponseError, profile::Advertisement,
        write_ap_peer_disconnect, write_ht_association_response_frame_for_security,
        write_open_authentication_response,
    },
    beacon::{AP_BEACON_CAPACITY, ApBeaconBuildError, TimBitmapError, TimVirtualBitmap},
    channel::WifiChannel,
    protection::ApBssProtection,
    qos::WmmAccessCategory,
    security::LinkProtection,
    sequence::SequenceNumber,
    ssid::WifiSsid,
    tsf::TsfInstant,
};
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

/// The event router of an access point's port.
pub type PortApRouter<'p, X> =
    EventRouter<'p, <X as PortClientEnv>::Port, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The access point's client of the port.
pub type PortApClient<'p, X> = PortClient<'p, X, PORT_AP_EXCHANGES, PORT_AP_BACKLOG>;

/// The types an access point over the port is built from: its port and
/// transmit policy ([`PortClientEnv`]) and the image's monotonic time.
pub trait PortApEnv: PortClientEnv {
    type Timer: Timer;
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
    /// Management requests the access point does not serve yet (SAE,
    /// Block Ack actions).
    pub unserved: u32,
}

/// Why an access point could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortApBuildError {
    Beacon(ApBeaconBuildError),
    /// The service's address is not the client's interface address.
    AddressMismatch,
    /// The service's BSS is protected; the access point serves an Open BSS
    /// only.
    SecurityUnsupported,
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
    /// The port refused the interface's TSF restart.
    Tsf(SettingError),
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
    profile: PortApProfile<'p>,
    beacon: ApBeacon<'p>,
    service: AccessPointService<'p>,
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
        profile: PortApProfile<'p>,
        service: AccessPointService<'p>,
        storage: &'p mut [u8; AP_BEACON_CAPACITY],
    ) -> Result<Self, PortApBuildError> {
        if service.address() != client.config().address {
            return Err(PortApBuildError::AddressMismatch);
        }
        if service.link_protection() != LinkProtection::Open {
            return Err(PortApBuildError::SecurityUnsupported);
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
            profile,
            beacon,
            service,
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
    /// the access point answers, and restart the interface's TSF.
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
            .map_err(PortApError::Tsf)
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
            if now >= deadline {
                return Ok(());
            }
            let wake = [
                self.beacon.next_publication(),
                self.service.next_peer_deadline(),
            ]
            .into_iter()
            .flatten()
            .fold(deadline, Instant::min);
            match self.client.next_input(&self.timer, wake).await {
                Some(PortInput::Frame(frame)) => self.receive(frame.bytes()).await?,
                Some(PortInput::Poisoned) => {
                    return Err(PortApError::Client(PortClientError::Poisoned));
                }
                Some(PortInput::Tbtt(_) | PortInput::EventsLost) | None => {}
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
                    self.service.remove_peer(peer)?;
                    self.counters.peers_left = self.counters.peers_left.saturating_add(1);
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
    /// of an associated peer is answered again with its association.
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
        let (result, association_id, ht) = if status.phase == ApPeerPhase::Authorized
            && self.service.matches_association_security(security)
        {
            (0, status.association_id, status.ht)
        } else if status.phase == ApPeerPhase::Authenticated {
            let ApMlmeAction::AssociationResponse {
                status,
                association_id,
                ..
            } = self
                .service
                .associate_open(peer, security, capabilities, now)?
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
        self.service.remove_peer(close.peer)?;
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
