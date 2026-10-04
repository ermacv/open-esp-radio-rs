//! The access point over the lower-MAC port.
//!
//! A [`PortAccessPoint`] owns its interface's
//! [`PortClient`](oer_ieee80211_upper_mac_service::client::PortClient) and
//! its beacon. [`PortAccessPoint::start`] tunes the port to the BSS's
//! channel, configures the access-point interface to receive its BSS and the
//! Probe Requests it answers, and restarts the interface's TSF.
//! [`PortAccessPoint::run_until`] then publishes a beacon at every TBTT of
//! its own schedule (`ApBeacon`: an absolute cursor that a late publication
//! does not move) and answers each Probe Request for its SSID, or for any
//! SSID, with the current advertisement. The composition enables the port
//! and polls the router beside the access point.

use oer_ieee80211_ap::{beacon::ApBeacon, limits::AP_TIM_VIRTUAL_BITMAP_OCTETS};
use oer_ieee80211_lower_mac::{
    Channel, CoexPriority, KeySelector, LowerMacBeaconTiming, PhyRate, ReceiveFilter, SettingError,
    VifTsf,
};
use oer_ieee80211_mac::{
    ap::{
        ApManagementRequest, parse_ap_management_request, probe, probe::ResponseError,
        profile::Advertisement,
    },
    beacon::{AP_BEACON_CAPACITY, ApBeaconBuildError, TimBitmapError, TimVirtualBitmap},
    channel::WifiChannel,
    qos::WmmAccessCategory,
    security::ApSecurityPolicy,
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

/// The BSS an access point runs.
#[derive(Clone, Copy, Debug)]
pub struct PortApProfile<'a> {
    pub ssid: &'a WifiSsid,
    pub channel: WifiChannel,
    pub beacon_interval_tu: u16,
    pub dtim_period: u8,
    pub security: ApSecurityPolicy,
    /// The rates, HT capabilities and WMM parameters the access point
    /// claims.
    pub advertisement: &'a Advertisement,
    /// The rate of beacons and management frames.
    pub management_rate: PhyRate,
    pub coex: CoexPriority,
}

/// What the access point sent and ignored.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortApCounters {
    pub beacons: u32,
    pub probe_responses: u32,
    /// Probe Requests answered by none: another SSID, or inside the
    /// response interval.
    pub probes_ignored: u32,
}

/// Why an access-point operation failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortApError<E> {
    Client(PortClientError<E>),
    /// The beacon template did not encode.
    Beacon(ApBeaconBuildError),
    /// The beacon template could not be stamped for publication.
    BeaconStamp,
    Tim(TimBitmapError),
    Probe(ResponseError),
    /// The port refused the interface's TSF restart.
    Tsf(SettingError),
}

impl<E> From<PortClientError<E>> for PortApError<E> {
    fn from(error: PortClientError<E>) -> Self {
        Self::Client(error)
    }
}

/// One access point over the port: see the [module](self).
pub struct PortAccessPoint<'p, X: PortApEnv> {
    client: PortApClient<'p, X>,
    timer: X::Timer,
    profile: PortApProfile<'p>,
    beacon: ApBeacon<'p>,
    /// The sequence number of the next management frame; beacons and
    /// responses share it.
    sequence: SequenceNumber,
    /// Before it no Probe Response goes out.
    next_probe_response: Instant,
    counters: PortApCounters,
}

impl<'p, X: PortApEnv> PortAccessPoint<'p, X> {
    /// An access point of `profile` over `client`, its beacon template in
    /// `storage`.
    pub fn new(
        client: PortApClient<'p, X>,
        timer: X::Timer,
        profile: PortApProfile<'p>,
        storage: &'p mut [u8; AP_BEACON_CAPACITY],
    ) -> Result<Self, ApBeaconBuildError> {
        let beacon = ApBeacon::new(
            storage,
            profile.advertisement,
            client.config().address,
            profile.ssid,
            profile.channel,
            profile.beacon_interval_tu,
            profile.dtim_period,
            SequenceNumber::ZERO,
            profile.security,
        )?;
        Ok(Self {
            client,
            timer,
            profile,
            beacon,
            sequence: SequenceNumber::ZERO,
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
    /// to every Probe Request it answers.
    pub async fn run_until(&mut self, deadline: Instant) -> Result<(), PortApError<PortError<X>>> {
        loop {
            let now = self.timer.now();
            if self.beacon.publication_due(now) {
                self.publish_beacon(now).await?;
                continue;
            }
            if now >= deadline {
                return Ok(());
            }
            let wake = self
                .beacon
                .next_publication()
                .map_or(deadline, |next| next.min(deadline));
            match self.client.next_input(&self.timer, wake).await {
                Some(PortInput::Frame(frame)) => self.receive(frame.bytes()).await?,
                Some(PortInput::Poisoned) => {
                    return Err(PortApError::Client(PortClientError::Poisoned));
                }
                Some(PortInput::Tbtt(_) | PortInput::EventsLost) | None => {}
            }
        }
    }

    fn next_sequence(&mut self) -> SequenceNumber {
        let sequence = self.sequence;
        self.sequence = sequence.wrapping_add(1);
        sequence
    }

    /// Stamp the beacon and send it once, unacknowledged, on the voice
    /// queue.
    async fn publish_beacon(&mut self, now: Instant) -> Result<(), PortApError<PortError<X>>> {
        let sequence = self.next_sequence();
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
            .ok_or(PortApError::BeaconStamp)?;
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
        let address = self.client.config().address;
        match parse_ap_management_request(self.profile.advertisement, frame, address) {
            Some(ApManagementRequest::Probe { peer, ssid }) => self.probe(peer, ssid).await,
            _ => Ok(()),
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
        let sequence = self.next_sequence();
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
        self.client
            .transmit(
                &response[..length],
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                self.profile.management_rate,
                self.profile.coex,
            )
            .await?;
        self.counters.probe_responses = self.counters.probe_responses.saturating_add(1);
        Ok(())
    }
}
