//! The session's radio operations: its commands to the running system and
//! the frames received between collections.

use embassy_time::{Duration, with_timeout};
use oer_esp32s31_ieee802154_runtime::{
    Ieee802154EnhancedAckGenerator, Ieee802154OwnedFrame, Ieee802154RadioEvent,
};
use oer_esp32s31_ieee802154_system::Ieee802154SystemRuntime;
use oer_hil_protocol::{
    ieee802154::IEEE802154_SESSION_RECORDED_FRAMES, ieee802154::Ieee802154AirCcaOutcome,
    ieee802154::Ieee802154AirEnergyOutcome, ieee802154::Ieee802154AirTxOutcome,
    ieee802154::Ieee802154SessionAck, ieee802154::Ieee802154SessionAssessRequest,
    ieee802154::Ieee802154SessionAssessment, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionFrame, ieee802154::Ieee802154SessionPendingMode,
    ieee802154::Ieee802154SessionPendingRequest, ieee802154::Ieee802154SessionReceiveEvidence,
    ieee802154::Ieee802154SessionReceivedFrame, ieee802154::Ieee802154SessionRecentRssi,
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154SessionTransmitEvidence,
    ieee802154::Ieee802154SessionTransmitRequest, ieee802154::Ieee802154SessionTxMode,
    ieee802154::ieee802154_frame_crc32c,
};
use oer_ieee802154::{
    AutoPendingMode, Channel, Configuration, EnergyScanRequest, FrameAddress, FrameView, Interface,
    RadioCommand, RequestId, TxMode, TxRequest, TxSecurity,
};

use super::super::client::tx_outcome;

/// Bound on one transmission's terminal event, over its retries.
pub(super) const TRANSMIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Bound on the terminal event of one energy scan beyond its duration, and
/// of one clear-channel assessment.
pub(super) const ASSESS_TIMEOUT: Duration = Duration::from_millis(100);

/// Frames received since the previous collection.
#[derive(Default)]
pub(super) struct Received {
    total: u16,
    frames: heapless::Vec<Ieee802154SessionReceivedFrame, IEEE802154_SESSION_RECORDED_FRAMES>,
}

impl Received {
    pub(super) fn record(&mut self, frame: &Ieee802154OwnedFrame) {
        self.total = self.total.saturating_add(1);
        let bytes = frame.frame.as_bytes();
        let _ = self.frames.push(Ieee802154SessionReceivedFrame {
            length: bytes.len() as u8,
            crc32c: ieee802154_frame_crc32c(bytes),
            rssi_dbm: frame.metadata.rssi_dbm,
            lqi: frame.metadata.link_quality,
        });
    }

    pub(super) fn take(&mut self) -> Ieee802154SessionReceiveEvidence {
        let Self { total, frames } = core::mem::take(self);
        Ieee802154SessionReceiveEvidence {
            result: Ieee802154SessionResult::Done,
            total,
            frames,
        }
    }
}

pub(super) struct Session {
    pub(super) runtime: &'static Ieee802154SystemRuntime,
    pub(super) channel: Channel,
    pub(super) next_id: u32,
    pub(super) received: Received,
    pub(super) lost: bool,
}

impl Session {
    pub(super) fn id(&mut self) -> RequestId {
        self.next_id = self.next_id.wrapping_add(1);
        RequestId::new(self.next_id)
    }

    pub(super) fn submit(
        &mut self,
        command: RadioCommand<'_>,
    ) -> Result<(), Ieee802154SessionResult> {
        self.runtime
            .submit(command)
            .map(|_| ())
            .map_err(|_| Ieee802154SessionResult::CommandRejected)
    }

    /// Apply the host's identity and filter to an enabled radio, and
    /// install the enhanced-ACK generator the host asked for.
    pub(super) fn configure(
        &mut self,
        config: Ieee802154SessionConfig,
    ) -> Result<(), Ieee802154SessionResult> {
        let generator = config
            .enhanced_ack
            .then(Ieee802154EnhancedAckGenerator::new);
        self.runtime
            .with_enhanced_ack(|installed| *installed = generator)
            .map_err(|_| Ieee802154SessionResult::StartFailed)?;
        let id = self.id();
        self.submit(RadioCommand::Enable { id })?;
        for configuration in [
            Configuration::PanId(config.pan_id),
            Configuration::ShortAddress(config.short_address),
            Configuration::ExtendedAddress(config.extended_address),
            Configuration::Promiscuous(config.promiscuous),
        ] {
            let id = self.id();
            self.submit(RadioCommand::Configure { id, configuration })?;
        }
        Ok(())
    }

    /// Take one runtime event; received frames are recorded.
    pub(super) fn observe(
        &mut self,
        event: Result<Ieee802154RadioEvent, oer_esp32s31_ieee802154_runtime::Ieee802154EventsLost>,
    ) -> Option<Ieee802154RadioEvent> {
        match event {
            Ok(Ieee802154RadioEvent::Received(frame)) => {
                self.received.record(&frame);
                None
            }
            Ok(event) => Some(event),
            Err(_) => {
                self.lost = true;
                None
            }
        }
    }

    /// Print the MAC power-sequencing words once per boot, before the first
    /// transmission, pausing the resting radio for the read.
    fn report_power_sequence_once(&self) {
        static REPORTED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if REPORTED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Ok(mut paused) = self.runtime.pause() else {
            return;
        };
        let sequence = paused.hardware_mut().power_sequence();
        // A paused radio always resumes into the runtime it came from.
        let _ = self.runtime.resume(paused);
        crate::console::ieee802154_power_sequence_report(sequence);
    }

    pub(super) async fn transmit(
        &mut self,
        request: &Ieee802154SessionTransmitRequest,
    ) -> Ieee802154SessionTransmitEvidence {
        let mut evidence = Ieee802154SessionTransmitEvidence::default();
        let Ok(frame) = FrameView::new(&request.frame) else {
            evidence.result = Ieee802154SessionResult::CommandRejected;
            return evidence;
        };
        self.report_power_sequence_once();
        let id = self.id();
        let mode = match request.mode {
            Ieee802154SessionTxMode::Direct => TxMode::Direct,
            Ieee802154SessionTxMode::ClearChannelAssessment => TxMode::ClearChannelAssessment,
            Ieee802154SessionTxMode::CsmaCa { max_backoffs } => TxMode::CsmaCa { max_backoffs },
        };
        if let Err(result) = self.submit(RadioCommand::Transmit(TxRequest {
            id,
            frame,
            channel: self.channel,
            mode,
            transmit_power_dbm: None,
            max_frame_retries: request.max_frame_retries,
            security: TxSecurity::Radio,
            interface: Interface::PRIMARY,
            time_sync: None,
        })) {
            evidence.result = result;
            return evidence;
        }
        loop {
            let Ok(event) = with_timeout(TRANSMIT_TIMEOUT, self.runtime.next_event()).await else {
                evidence.result = Ieee802154SessionResult::EventTimeout;
                return evidence;
            };
            match self.observe(event) {
                None => continue,
                Some(Ieee802154RadioEvent::TransmitDone {
                    id: done,
                    status,
                    acknowledgement,
                    ..
                }) if done == id => {
                    evidence.result = Ieee802154SessionResult::Done;
                    evidence.outcome = tx_outcome(status);
                    evidence.acknowledgement = acknowledgement.map(|ack| Ieee802154SessionAck {
                        frame: Ieee802154SessionFrame::from_slice(ack.frame.as_bytes())
                            .unwrap_or_default(),
                        rssi_dbm: ack.metadata.rssi_dbm,
                        lqi: ack.metadata.link_quality,
                    });
                    return evidence;
                }
                Some(_) => {
                    evidence.result = Ieee802154SessionResult::UnexpectedEvent;
                    evidence.outcome = Ieee802154AirTxOutcome::NotRun;
                    return evidence;
                }
            }
        }
    }

    /// The next terminal event within `timeout`; received frames are
    /// recorded on the way.
    pub(super) async fn terminal_event(
        &mut self,
        timeout: Duration,
    ) -> Result<Ieee802154RadioEvent, Ieee802154SessionResult> {
        loop {
            let Ok(event) = with_timeout(timeout, self.runtime.next_event()).await else {
                return Err(Ieee802154SessionResult::EventTimeout);
            };
            if let Some(event) = self.observe(event) {
                return Ok(event);
            }
        }
    }

    /// Scan the energy on the requested channel, then assess it once, as
    /// two radio commands on that channel.
    pub(super) async fn assess(
        &mut self,
        request: Ieee802154SessionAssessRequest,
    ) -> Ieee802154SessionAssessment {
        let mut assessment = Ieee802154SessionAssessment::default();
        let Ok(channel) = Channel::new(request.channel) else {
            assessment.result = Ieee802154SessionResult::CommandRejected;
            return assessment;
        };
        let id = self.id();
        if let Err(result) = self.submit(RadioCommand::EnergyScan(EnergyScanRequest {
            id,
            channel,
            duration_us: request.energy_scan_micros,
        })) {
            assessment.result = result;
            return assessment;
        }
        let scan_timeout =
            Duration::from_micros(u64::from(request.energy_scan_micros)) + ASSESS_TIMEOUT;
        assessment.energy = match self.terminal_event(scan_timeout).await {
            Ok(Ieee802154RadioEvent::EnergyScanDone {
                id: done,
                energy_dbm,
                ..
            }) if done == id => Ieee802154AirEnergyOutcome::Energy(energy_dbm),
            Ok(Ieee802154RadioEvent::EnergyScanFailed { id: done, .. }) if done == id => {
                Ieee802154AirEnergyOutcome::Failed
            }
            Ok(_) => {
                assessment.result = Ieee802154SessionResult::UnexpectedEvent;
                return assessment;
            }
            Err(result) => {
                assessment.result = result;
                return assessment;
            }
        };
        let id = self.id();
        if let Err(result) = self.submit(RadioCommand::ClearChannelAssessment { id, channel }) {
            assessment.result = result;
            return assessment;
        }
        assessment.cca = match self.terminal_event(ASSESS_TIMEOUT).await {
            Ok(Ieee802154RadioEvent::ClearChannelAssessmentDone { id: done, idle, .. })
                if done == id =>
            {
                if idle {
                    Ieee802154AirCcaOutcome::Clear
                } else {
                    Ieee802154AirCcaOutcome::Busy
                }
            }
            Ok(Ieee802154RadioEvent::ClearChannelAssessmentFailed { id: done, .. })
                if done == id =>
            {
                Ieee802154AirCcaOutcome::Failed
            }
            Ok(_) => {
                assessment.result = Ieee802154SessionResult::UnexpectedEvent;
                return assessment;
            }
            Err(result) => {
                assessment.result = result;
                return assessment;
            }
        };
        assessment.result = Ieee802154SessionResult::Done;
        assessment
    }

    /// The live RSSI of the most recent baseband reception.
    pub(super) fn recent_rssi(&self) -> Ieee802154SessionRecentRssi {
        match self.runtime.recent_rssi() {
            Ok(rssi_dbm) => Ieee802154SessionRecentRssi {
                result: Ieee802154SessionResult::Done,
                rssi_dbm,
            },
            Err(_) => Ieee802154SessionRecentRssi {
                result: Ieee802154SessionResult::CommandRejected,
                rssi_dbm: 0,
            },
        }
    }

    pub(super) fn pending(&mut self, request: Ieee802154SessionPendingRequest) -> bool {
        let mode = match request.mode {
            Ieee802154SessionPendingMode::Disabled => AutoPendingMode::Disable,
            Ieee802154SessionPendingMode::Enabled => AutoPendingMode::Enable,
            Ieee802154SessionPendingMode::Enhanced => AutoPendingMode::Enhanced,
            Ieee802154SessionPendingMode::Zigbee => AutoPendingMode::Zigbee,
        };
        let runtime = self.runtime;
        if runtime.set_pending_mode(mode).is_err() {
            return false;
        }
        match request.short_address {
            None => true,
            Some(short) => runtime
                .with_pending_table(|table| {
                    table.add(FrameAddress::Short(short.to_le_bytes())).is_ok()
                })
                .unwrap_or(false),
        }
    }
}
