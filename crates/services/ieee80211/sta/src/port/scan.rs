//! Candidate scan over the lower-MAC port.

use oer_ieee80211_lower_mac::{
    Channel, Ieee80211LowerMacPort, Ieee80211Stamp, KeySelector, LowerMacMonitor, ReceiveFilter,
    RxEvidence, SettingError, VifRole,
};
use oer_ieee80211_mac::{
    management::{BROADCAST_ADDRESS, ProbeRequest, ProbeRequestError},
    qos::WmmAccessCategory,
    scan::{ScanRecord, ScanTable, best_matching_ssid_and_security},
    security::StaSecurityPolicy,
    station::{StaSequenceCounter, StationFrameError, select_association_rsn},
};
use oer_ieee80211_sta::scan::{ActiveProbeOutcome, StaScanChannelContext, StaScanPort};
use oer_ieee80211_upper_mac_service::UpperMacTxError;
use oer_ieee80211_upper_mac_service::client::{PortError, PortFrame, PortInput, PortRxBuffer};
use oer_time::{Clock, Duration, Instant};

use super::link::{PortConnectionFrame, PortLink, PortLinkError, PortStationEnv};

/// The Probe Request of an active scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortProbe<'a> {
    /// The SSID the request names; empty for a wildcard request.
    pub ssid: &'a [u8],
    /// The Supported Rates the station advertises, in 500 kb/s units.
    pub supported_rates: &'a [u8],
}

/// What a scan looks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortScanTarget<'a> {
    pub ssid: &'a [u8],
    /// Only access points that admit this policy are candidates.
    pub policy: StaSecurityPolicy,
    /// Send this Probe Request on each channel; `None` scans passively.
    pub probe: Option<PortProbe<'a>>,
}

type SetMonitor<P> =
    fn(&P, bool) -> Result<Result<(), SettingError>, <P as Ieee80211LowerMacPort>::Error>;

/// Received signal strength of a frame whose backend reports none.
const UNKNOWN_RSSI_DBM: i8 = -100;

/// The primitive scan operations of [`StaScanPort`] over the lower-MAC port.
///
/// The `StaScanBackend` of this crate orders them per channel. Each channel
/// visit tunes through [`LowerMacSetting::Channel`](oer_ieee80211_lower_mac::LowerMacSetting::Channel),
/// receives other BSSs' beacons and Probe Responses through the station
/// interface's `OTHER_BSS_MANAGEMENT` receive filter, sends the optional
/// Probe Request through the transmit planner and records every beacon and
/// Probe Response of the dwell in the scan table. A port whose station
/// filters lack `OTHER_BSS_MANAGEMENT` needs monitor reception, which
/// [`Self::with_monitor`] offers for a port with [`LowerMacMonitor`].
pub struct PortScan<'a, 'p, X: PortStationEnv, const N: usize> {
    link: &'a mut PortLink<'p, X>,
    timer: &'a X::Timer,
    sequence: &'a mut StaSequenceCounter,
    table: &'a mut ScanTable<N>,
    target: PortScanTarget<'a>,
    tick: Duration,
    set_monitor: Option<SetMonitor<X::Port>>,
    monitoring: bool,
    channel: u8,
    /// An owner-selected channel; a visit must not tune the shared port.
    fixed_channel: Option<Channel>,
    tick_deadline: Option<Instant>,
    known_bssid: Option<[u8; 6]>,
}

impl<'a, 'p, X: PortStationEnv, const N: usize> PortScan<'a, 'p, X, N> {
    /// A scan for `target` whose dwell ticks last `tick` each; its Probe
    /// Requests take their sequence numbers from `sequence`.
    pub fn new(
        link: &'a mut PortLink<'p, X>,
        timer: &'a X::Timer,
        sequence: &'a mut StaSequenceCounter,
        table: &'a mut ScanTable<N>,
        target: PortScanTarget<'a>,
        tick: Duration,
    ) -> Self {
        Self {
            link,
            timer,
            sequence,
            table,
            target,
            tick,
            set_monitor: None,
            monitoring: false,
            channel: 0,
            fixed_channel: None,
            tick_deadline: None,
            known_bssid: None,
        }
    }

    /// Fall back to monitor reception when the station's receive filters do
    /// not admit other BSSs' management frames.
    pub fn with_monitor(mut self) -> Self
    where
        X::Port: LowerMacMonitor,
    {
        self.set_monitor = Some(<X::Port as LowerMacMonitor>::set_monitor);
        self
    }

    /// Receive on the owner's channel without retuning the radio.
    /// Only the station's interface is configured. Unlike a standalone
    /// scan, this cannot fall back to the port-wide monitor mode.
    pub(crate) fn on_channel(mut self, channel: Channel) -> Self {
        self.fixed_channel = Some(channel);
        self
    }

    pub(crate) fn known_bssid(mut self, bssid: Option<[u8; 6]>) -> Self {
        self.known_bssid = bssid;
        self
    }

    /// Observe one owner-selected channel up to an absolute deadline.
    /// This operation never tunes the port or starts monitor reception and
    /// does not transmit a probe. The receive filter is closed even when
    /// the observation fails.
    pub(crate) async fn listen_until(
        &mut self,
        channel: Channel,
        until: Instant,
    ) -> Result<Option<ScanRecord>, PortLinkError<PortError<X>>> {
        if self.fixed_channel != Some(channel) {
            return Err(PortLinkError::MissingState);
        }
        self.begin_scan().await?;
        self.channel = channel.number();
        self.link.discard_backlog().await;
        self.link
            .configure(None, ReceiveFilter::OTHER_BSS_MANAGEMENT)?;
        let result = async {
            while let Some(input) = self.link.next_input(self.timer, until).await {
                match input {
                    PortInput::Frame(frame) if frame.meta().channel == channel => {
                        self.observe(&frame);
                        if let Some(candidate) = self.select_candidate()? {
                            return Ok(Some(candidate));
                        }
                    }
                    PortInput::Poisoned => return Err(PortLinkError::Poisoned),
                    PortInput::Frame(_) | PortInput::Tbtt(_) | PortInput::EventsLost => {}
                }
            }
            Ok(None)
        }
        .await;
        let stopped = self.link.configure(None, ReceiveFilter::NONE);
        let candidate = result?;
        stopped?;
        Ok(candidate)
    }

    fn observe(&mut self, frame: &PortFrame<PortRxBuffer<X>>) {
        let rssi = match frame.meta().rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
            RxEvidence::Unavailable => UNKNOWN_RSSI_DBM,
        };
        let received_at = self.reception_time(frame.meta().timestamp);
        self.table
            .observe_management(frame.bytes(), self.channel, rssi, received_at);
    }

    /// The monotonic time of a frame's reception: its port stamp converted
    /// with a sample of the port's clock; `None` without a stamp, a sample
    /// or a relation of the stamp's generation.
    fn reception_time(&self, timestamp: RxEvidence<Ieee80211Stamp>) -> Option<Instant> {
        let stamp = match timestamp {
            RxEvidence::HardwareObserved(stamp) | RxEvidence::ProtocolValidated(stamp) => stamp,
            RxEvidence::Unavailable => return None,
        };
        let port = self.link.port();
        let sample = port.clock_sample().ok()?;
        port.clock_info()
            .to_monotonic_with(stamp, &sample)
            .ok()
            .map(|projected| projected.at)
    }

    fn set_monitor(&self, enabled: bool) -> Result<(), PortLinkError<PortError<X>>> {
        match self.set_monitor {
            Some(set) => set(self.link.port(), enabled)
                .map_err(PortLinkError::Port)?
                .map_err(PortLinkError::Setting),
            None => Ok(()),
        }
    }
}

impl<X: PortStationEnv, const N: usize> StaScanPort for PortScan<'_, '_, X, N> {
    type Channel = Channel;
    type Candidate = ScanRecord;
    type Error = PortLinkError<PortError<X>>;

    async fn begin_scan(&mut self) -> Result<(), Self::Error> {
        self.table.clear();
        let filters = self
            .link
            .port()
            .capabilities()
            .receive_filters(VifRole::Station);
        self.monitoring = if filters.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT) {
            false
        } else if self.fixed_channel.is_none() && self.set_monitor.is_some() {
            true
        } else {
            return Err(PortLinkError::ReceptionUnsupported);
        };
        self.link.configure(None, ReceiveFilter::NONE)
    }

    async fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Channel>,
        requested_dwell_ticks: u16,
    ) -> Result<u16, Self::Error> {
        if let Some(channel) = self.fixed_channel {
            if context.channel != channel {
                return Err(PortLinkError::MissingState);
            }
        } else {
            self.link.retune(context.channel).await?;
        }
        self.channel = context.channel.number();
        Ok(requested_dwell_ticks)
    }

    async fn start_receive(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<(), Self::Error> {
        self.tick_deadline = None;
        self.link.discard_backlog().await;
        if self.monitoring {
            self.set_monitor(true)
        } else {
            self.link
                .configure(None, ReceiveFilter::OTHER_BSS_MANAGEMENT)
        }
    }

    async fn transmit_active_probe(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<ActiveProbeOutcome, Self::Error> {
        let Some(probe) = self.target.probe else {
            return Ok(ActiveProbeOutcome::PassiveFallback);
        };
        let config = *self.link.config();
        let mut frame = [0_u8; 128];
        let length = ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: config.address,
            bssid: BROADCAST_ADDRESS,
            sequence_number: self.sequence.take(),
            ssid: probe.ssid,
            supported_rates: probe.supported_rates,
        }
        .encode(&mut frame)
        .map_err(|error| {
            PortLinkError::Frame(match error {
                ProbeRequestError::SsidTooLong => StationFrameError::SsidTooLong,
                ProbeRequestError::NoSupportedRates => StationFrameError::NoSupportedRates,
                ProbeRequestError::TooManySupportedRates => {
                    StationFrameError::TooManySupportedRates
                }
                ProbeRequestError::OutputTooSmall { required } => {
                    StationFrameError::OutputTooSmall { required }
                }
            })
        })?;
        match self
            .link
            .transmit_connection_frame(
                PortConnectionFrame::ProbeRequest,
                &frame[..length],
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                config.management_rate,
            )
            .await
        {
            Ok(_) => Ok(ActiveProbeOutcome::Transmitted),
            // A probe the port could not take leaves a passive dwell.
            Err(PortLinkError::Tx(UpperMacTxError::Refused(_) | UpperMacTxError::NoBuffer)) => {
                Ok(ActiveProbeOutcome::PassiveFallback)
            }
            Err(error) => Err(error),
        }
    }

    fn observe_receive(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<(), Self::Error> {
        // The frames an exchange kept aside; the dwell reads the rest.
        Ok(())
    }

    async fn wait_dwell_tick(&mut self) -> Result<(), Self::Error> {
        let start = self.tick_deadline.unwrap_or_else(|| self.timer.now());
        let deadline = start.checked_add(self.tick).unwrap_or(start);
        self.tick_deadline = Some(deadline);
        while let Some(input) = self.link.next_input(self.timer, deadline).await {
            match input {
                PortInput::Frame(frame) => self.observe(&frame),
                PortInput::Poisoned => return Err(PortLinkError::Poisoned),
                PortInput::Tbtt(_) | PortInput::EventsLost => {}
            }
        }
        Ok(())
    }

    async fn stop_receive(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<(), Self::Error> {
        if self.monitoring {
            self.set_monitor(false)?;
        }
        self.link.configure(None, ReceiveFilter::NONE)
    }

    fn prepare_next_ring(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn select_candidate(&mut self) -> Result<Option<ScanRecord>, Self::Error> {
        let candidate = best_matching_ssid_and_security(
            self.table.records(),
            self.target.ssid,
            self.target.policy,
        )
        .copied();
        // A hidden beacon can identify a BSS we have already joined. Do
        // not infer the SSID of an unknown BSSID or accept changed security.
        Ok(candidate.or_else(|| {
            self.table
                .records()
                .iter()
                .find(|record| {
                    Some(record.bssid) == self.known_bssid
                        && record.ssid_bytes().is_empty()
                        && select_association_rsn(record, self.target.policy).is_ok()
                })
                .copied()
        }))
    }
}
