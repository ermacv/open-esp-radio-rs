//! Candidate scan over the lower-MAC port.

use oer_ieee80211_lower_mac::{
    Channel, Ieee80211LowerMacPort, KeySelector, LowerMacMonitor, ReceiveFilter, RxEvidence,
    SettingError, VifRole,
};
use oer_ieee80211_mac::{
    management::{BROADCAST_ADDRESS, ProbeRequest, ProbeRequestError},
    qos::WmmAccessCategory,
    scan::{ScanRecord, ScanTable, best_matching_ssid_and_security},
    security::StaSecurityPolicy,
    station::{StaSequenceCounter, StationFrameError},
};
use oer_ieee80211_sta::scan::{ActiveProbeOutcome, StaScanChannelContext, StaScanPort};
use oer_ieee80211_upper_mac_service::UpperMacTxError;
use oer_time::{Clock, Duration, Instant};

use super::link::{PortError, PortFrame, PortInput, PortLink, PortLinkError, PortStationEnv};

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
    tick_deadline: Option<Instant>,
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
            tick_deadline: None,
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

    fn observe(&mut self, frame: &PortFrame) {
        let rssi = match frame.meta().rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
            RxEvidence::Unavailable => UNKNOWN_RSSI_DBM,
        };
        let now = self.timer.now().as_micros();
        self.table
            .observe_management(frame.bytes(), self.channel, rssi, now);
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
        } else if self.set_monitor.is_some() {
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
        self.link.retune(context.channel).await?;
        self.channel = context.channel.number();
        Ok(requested_dwell_ticks)
    }

    async fn start_receive(
        &mut self,
        _context: StaScanChannelContext<Channel>,
    ) -> Result<(), Self::Error> {
        self.tick_deadline = None;
        self.link.discard_backlog();
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
            .transmit(
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
            if let PortInput::Frame(frame) = input {
                self.observe(&frame);
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
        Ok(best_matching_ssid_and_security(
            self.table.records(),
            self.target.ssid,
            self.target.policy,
        )
        .copied())
    }
}
