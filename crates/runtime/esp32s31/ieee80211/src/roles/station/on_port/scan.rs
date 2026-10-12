//! The station's candidate scan on the port: the portable
//! [`StaScanPort`] the `StaCandidateScanService` of
//! `oer-ieee80211-sta-service` drives.
//!
//! A channel visit is a channel move of the port's owner
//! ([`StationOwner::tune`]); the station receives other BSSs' management
//! frames while it visits, from the phase's event consumer, into the scan
//! table; an active probe is a group Probe Request through the station's
//! client, waited for until its completion.

use oer_ieee80211_lower_mac::{
    Backoff, Channel, CoexPriority, FrameControl, GroupAddress, HeaderSpec, LowerMacClient,
    LowerMacEvent, MpduLength, PhaseAttempt, PhyRate, Protection, SizedFrame, StationClient,
    StationReceive, SupportedBackoff, SupportedChannel, SupportedCoex, SupportedPower, TxFrame,
    TxFrameBody, TxPower, TxSeats,
};
use oer_ieee80211_mac::{
    management::{BROADCAST_ADDRESS, ProbeRequest},
    phy::LegacyRate,
    qos::WmmAccessCategory,
    scan::{ScanRecord, ScanTable, best_matching_ssid_and_security},
    security::StaSecurityPolicy,
    station::StaSequenceCounter,
};
use oer_ieee80211_port::RadioPortEvents;
use oer_ieee80211_sta::scan::{ActiveProbeOutcome, StaScanChannelContext, StaScanPort};

use super::{PortLost, StationOwner, StationParts, try_next_event};
use crate::mac_clock::ReceptionClock;

/// One scan dwell tick: a millisecond.
const SCAN_DWELL_TICK: oer_time::Duration = oer_time::Duration::from_millis(1);

/// The rate of a Probe Request: the lowest DSSS rate every 2.4 GHz station
/// receives.
const PROBE_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Dsss1M);

/// The longest Probe Request body the scan builds: an SSID element and the
/// supported-rate elements.
const PROBE_FRAME: usize = 24 + 2 + 32 + 2 + 8 + 2 + 8;

/// What the station scans for.
#[derive(Clone, Copy, Debug)]
pub struct ScanTarget<'a> {
    pub ssid: &'a [u8],
    pub security: StaSecurityPolicy,
    /// Whether the scan selects a candidate; a survey only fills the table.
    pub select_candidate: bool,
    /// The supported rates a Probe Request advertises.
    pub supported_rates: &'a [u8],
}

/// Why a scan step failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortScanError<F, R> {
    /// The station lost its port.
    Port(PortLost<F, R>),
    /// A channel the backend does not carry.
    UnsupportedChannel(u8),
    /// The port is not enabled where a visit needs it.
    NotRunning,
    /// The Probe Request does not encode.
    Probe,
}

/// The candidate scan of a station on the port.
pub struct PortScan<'s, 'p, 'id, B: StationParts, T, const RECORDS: usize> {
    owner: &'s mut StationOwner<'p, 'id, B>,
    seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
    table: &'s mut ScanTable<RECORDS>,
    sequence: &'s mut StaSequenceCounter,
    clock: &'s T,
    target: ScanTarget<'s>,
    /// The channel of the visit in progress, for frames that carry none.
    channel: u8,
}

impl<'s, 'p, 'id, B: StationParts, T, const RECORDS: usize> PortScan<'s, 'p, 'id, B, T, RECORDS> {
    pub fn new(
        owner: &'s mut StationOwner<'p, 'id, B>,
        seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
        table: &'s mut ScanTable<RECORDS>,
        sequence: &'s mut StaSequenceCounter,
        clock: &'s T,
        target: ScanTarget<'s>,
    ) -> Self {
        Self {
            owner,
            seats,
            table,
            sequence,
            clock,
            target,
            channel: 0,
        }
    }
}

impl<'s, 'p, 'id, B, T, const RECORDS: usize> PortScan<'s, 'p, 'id, B, T, RECORDS>
where
    B: StationParts,
    T: ReceptionClock,
{
    /// Record every management frame the phase holds now into the table;
    /// other events end their work and are dropped.
    fn drain(&mut self) -> Result<(), PortScanError<B::Fault, B::Refusal>> {
        let snapshot = self.clock.snapshot();
        let channel = self.channel;
        let Some(mut phase) = self.owner.phase() else {
            return Ok(());
        };
        while let Some(event) = try_next_event(&mut *phase.events) {
            let event = match event {
                Ok(Ok(event)) => event,
                Ok(Err(_lost)) => continue,
                Err(poisoned) => {
                    return Err(PortScanError::Port(PortLost::Poisoned(poisoned.cause)));
                }
            };
            if let LowerMacEvent::Received { frame, meta } = B::view(&event) {
                let rssi = match meta.rssi_dbm {
                    oer_ieee80211_lower_mac::RxEvidence::HardwareObserved(rssi)
                    | oer_ieee80211_lower_mac::RxEvidence::ProtocolValidated(rssi) => rssi,
                    oer_ieee80211_lower_mac::RxEvidence::Unavailable => i8::MIN,
                };
                let received_at = match meta.timestamp {
                    oer_ieee80211_lower_mac::RxEvidence::HardwareObserved(stamp) => {
                        snapshot.and_then(|snapshot| snapshot.monotonic(stamp))
                    }
                    _ => None,
                };
                self.table
                    .observe_management(frame, channel, rssi, received_at);
            }
        }
        Ok(())
    }
}

impl<'s, 'p, 'id, B, T, const RECORDS: usize> StaScanPort for PortScan<'s, 'p, 'id, B, T, RECORDS>
where
    B: StationParts,
    T: ReceptionClock + oer_time::Timer,
{
    type Channel = u8;
    type Candidate = ScanRecord;
    type Error = PortScanError<B::Fault, B::Refusal>;

    async fn begin_scan(&mut self) -> Result<(), Self::Error> {
        self.table.clear();
        Ok(())
    }

    async fn switch_channel(
        &mut self,
        context: StaScanChannelContext<u8>,
        requested_dwell_ticks: u16,
    ) -> Result<u16, Self::Error> {
        let channel = Channel::ghz2_4(
            context.channel,
            oer_ieee80211_lower_mac::ChannelWidth::Mhz20,
        )
        .ok()
        .and_then(SupportedChannel::<B::Caps>::new)
        .ok_or(PortScanError::UnsupportedChannel(context.channel))?;
        // A leftover terminal of the previous visit ends its work here.
        self.owner
            .tune(channel, &mut |_| {})
            .await
            .map_err(PortScanError::Port)?;
        self.channel = context.channel;
        Ok(requested_dwell_ticks)
    }

    async fn start_receive(&mut self, _: StaScanChannelContext<u8>) -> Result<(), Self::Error> {
        let phase = self.owner.phase().ok_or(PortScanError::NotRunning)?;
        B::station(&phase.clients)
            .configure(StationReceive::OtherBssManagement)
            .await;
        drop(phase.events);
        Ok(())
    }

    async fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<u8>,
    ) -> Result<ActiveProbeOutcome, Self::Error> {
        let sequence = self.sequence.take();
        let mut phase = self.owner.phase().ok_or(PortScanError::NotRunning)?;
        let station = B::station(&phase.clients);
        let Some(rate) = station.rate(PROBE_RATE) else {
            return Ok(ActiveProbeOutcome::PassiveFallback);
        };
        let address = station.address().get();
        let mut encoded = [0; PROBE_FRAME];
        let len = ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: address,
            bssid: BROADCAST_ADDRESS,
            sequence_number: sequence,
            ssid: b"",
            supported_rates: self.target.supported_rates,
        }
        .encode(&mut encoded)
        .map_err(|_| PortScanError::Probe)?;
        let header = HeaderSpec::new(
            FrameControl::new(0x0040).ok_or(PortScanError::Probe)?,
            0,
            GroupAddress::new(BROADCAST_ADDRESS).ok_or(PortScanError::Probe)?,
            address,
            BROADCAST_ADDRESS,
            sequence.sequence_control(),
            None,
            None,
            None,
        )
        .ok_or(PortScanError::Probe)?;
        let frame = MpduLength::new(len)
            .and_then(|len| SizedFrame::new(header, len))
            .ok_or(PortScanError::Probe)?;
        let category = WmmAccessCategory::Voice;
        let mut buffer = station
            .tx_buffer(TxFrame::Group { frame, rate }, self.seats.buffer(category))
            .await;
        buffer.body_mut().copy_from_slice(&encoded[24..len]);
        let queue = station.queue(self.seats.queue(category)).await;
        let attempt = PhaseAttempt {
            queue,
            frame: buffer,
            body: None,
            protection: Protection::None,
            key: None,
            power: SupportedPower::new(TxPower::Calibrated).ok_or(PortScanError::Probe)?,
            backoff: SupportedBackoff::new(Backoff::Slots(0)).ok_or(PortScanError::Probe)?,
            coex: SupportedCoex::new(CoexPriority::Elevated)
                .or_else(|| SupportedCoex::new(CoexPriority::Normal))
                .ok_or(PortScanError::Probe)?,
        };
        let work = station
            .submit(attempt)
            .map_err(|poisoned| PortScanError::Port(PortLost::Poisoned(poisoned.cause)))?;
        let id = work.id();
        let _ = context;
        // The probe's completion; frames received meanwhile go to the
        // table.
        loop {
            match phase.events.next_event().await {
                Ok(Ok(event)) => {
                    if let LowerMacEvent::TxCompleted(completion) = B::view(&event)
                        && completion.id == id
                    {
                        return Ok(ActiveProbeOutcome::Transmitted);
                    }
                    if let LowerMacEvent::Received { frame, meta } = B::view(&event) {
                        let rssi = match meta.rssi_dbm {
                            oer_ieee80211_lower_mac::RxEvidence::HardwareObserved(rssi)
                            | oer_ieee80211_lower_mac::RxEvidence::ProtocolValidated(rssi) => rssi,
                            oer_ieee80211_lower_mac::RxEvidence::Unavailable => i8::MIN,
                        };
                        self.table
                            .observe_management(frame, self.channel, rssi, None);
                    }
                }
                Ok(Err(_lost)) => {}
                Err(poisoned) => {
                    return Err(PortScanError::Port(PortLost::Poisoned(poisoned.cause)));
                }
            }
        }
    }

    fn observe_receive(&mut self, _: StaScanChannelContext<u8>) -> Result<(), Self::Error> {
        self.drain()
    }

    async fn wait_dwell_tick(&mut self) -> Result<(), Self::Error> {
        crate::time::wait_for(self.clock, SCAN_DWELL_TICK).await;
        Ok(())
    }

    async fn stop_receive(&mut self, _: StaScanChannelContext<u8>) -> Result<(), Self::Error> {
        {
            let phase = self.owner.phase().ok_or(PortScanError::NotRunning)?;
            B::station(&phase.clients)
                .configure(StationReceive::Off)
                .await;
        }
        self.drain()
    }

    fn prepare_next_ring(&mut self, _: StaScanChannelContext<u8>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn select_candidate(&mut self) -> Result<Option<ScanRecord>, Self::Error> {
        if !self.target.select_candidate {
            return Ok(None);
        }
        Ok(best_matching_ssid_and_security(
            self.table.records(),
            self.target.ssid,
            self.target.security,
        )
        .copied())
    }
}
