//! The session's radio operations: its commands to the running system and
//! the frames received between collections.

use embassy_time::{Duration, with_timeout};
use oer_esp32s31_ieee802154_system::Ieee802154SystemPort;
use oer_espressif_ieee802154_runtime::{Ieee802154OwnedFrame, Ieee802154RadioEvent};
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
    AutoPendingMode, Channel, CommandError, Configuration, EnergyScanRequest,
    EnhancedAckGeneration, EventsLost, FrameAddress, FrameView, Ieee802154RadioPort, Interface,
    LifecycleCommand, LifecycleEvent, RadioCommand, RadioPort, RadioSetting, RequestId, TxMode,
    TxRequest, TxSecurity,
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

/// Bound on a shared PHY maintenance window a refused command waits out.
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) struct Session {
    /// The radio's port; the session gives it back only across a restart.
    pub(super) port: Option<Ieee802154SystemPort>,
    /// Shared PHY maintenance quiesced the port and has not enabled it yet.
    pub(super) maintained: bool,
    /// Maintenance windows that ended, so a waiting command sees its own.
    pub(super) maintenance_ends: u32,
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

    /// The radio's port.
    pub(super) fn port(&self) -> &Ieee802154SystemPort {
        self.port
            .as_ref()
            .expect("the session holds its port outside a restart")
    }

    /// Admit `command`. Shared PHY maintenance quiesces the port and refuses
    /// a command that writes the hardware until its `Enabled`: the command
    /// waits out the window and is admitted then.
    pub(super) async fn submit(
        &mut self,
        command: RadioCommand<'_>,
    ) -> Result<(), Ieee802154SessionResult> {
        loop {
            match self.port().submit(command) {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(CommandError::Quiesced)) => self.wait_maintenance().await?,
                _ => return Err(Ieee802154SessionResult::CommandRejected),
            }
        }
    }

    /// Take events until a maintenance window ends; received frames are
    /// recorded and other events dropped, as between commands.
    async fn wait_maintenance(&mut self) -> Result<(), Ieee802154SessionResult> {
        let ends = self.maintenance_ends;
        while self.maintenance_ends == ends {
            let Ok(event) = with_timeout(MAINTENANCE_TIMEOUT, self.taken()).await else {
                return Err(Ieee802154SessionResult::EventTimeout);
            };
            let _ = self.observe(event);
        }
        Ok(())
    }

    /// Apply the host's identity and filter to an enabled radio, and
    /// install the enhanced-ACK generator the host asked for.
    pub(super) async fn configure(
        &mut self,
        config: Ieee802154SessionConfig,
    ) -> Result<(), Ieee802154SessionResult> {
        // A generator measuring link margins from a zero noise floor, as
        // ESP-IDF's port leaves it.
        let generation = config
            .enhanced_ack
            .then_some(EnhancedAckGeneration { noise_floor_dbm: 0 });
        if !matches!(
            self.port().apply(RadioSetting::EnhancedAck(generation)),
            Ok(Ok(()))
        ) {
            return Err(Ieee802154SessionResult::StartFailed);
        }
        self.lifecycle(LifecycleCommand::Enable, LifecycleEvent::Enabled)
            .await?;
        for configuration in [
            Configuration::PanId(config.pan_id),
            Configuration::ShortAddress(config.short_address),
            Configuration::ExtendedAddress(config.extended_address),
            Configuration::Promiscuous(config.promiscuous),
        ] {
            let id = self.id();
            self.submit(RadioCommand::Configure { id, configuration })
                .await?;
        }
        Ok(())
    }

    /// Run one lifecycle command and take its terminal event; frames
    /// received before it are recorded.
    pub(super) async fn lifecycle(
        &mut self,
        command: LifecycleCommand,
        terminal: LifecycleEvent,
    ) -> Result<(), Ieee802154SessionResult> {
        let Ok(Ok(())) = self.port().lifecycle(command).await else {
            return Err(Ieee802154SessionResult::CommandRejected);
        };
        match self.terminal_event(ASSESS_TIMEOUT).await? {
            Ieee802154RadioEvent::Lifecycle(event) if event == terminal => Ok(()),
            _ => Err(Ieee802154SessionResult::UnexpectedEvent),
        }
    }

    /// Leave receive mode and disable the radio before its client stops.
    pub(super) async fn rest_disabled(&mut self) -> Result<(), Ieee802154SessionResult> {
        let id = self.id();
        self.submit(RadioCommand::Sleep { id }).await?;
        self.lifecycle(LifecycleCommand::Disable, LifecycleEvent::Disabled)
            .await
    }

    /// The port's next event; the runtime never poisons.
    pub(super) async fn taken(&self) -> Result<Ieee802154RadioEvent, EventsLost> {
        let Ok(event) = self.port().next_event().await;
        event
    }

    /// Take one runtime event; received frames are recorded.
    pub(super) fn observe(
        &mut self,
        event: Result<Ieee802154RadioEvent, EventsLost>,
    ) -> Option<Ieee802154RadioEvent> {
        match event {
            Ok(Ieee802154RadioEvent::Received(frame)) => {
                self.received.record(&frame);
                None
            }
            // The session never quiesces the port: `Quiesced` opens a shared
            // PHY maintenance window and the next `Enabled` closes it.
            Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Quiesced)) => {
                self.maintained = true;
                None
            }
            Ok(Ieee802154RadioEvent::Lifecycle(LifecycleEvent::Enabled)) if self.maintained => {
                self.maintained = false;
                self.maintenance_ends = self.maintenance_ends.wrapping_add(1);
                None
            }
            Ok(event) => Some(event),
            Err(_) => {
                self.lost = true;
                None
            }
        }
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
        let id = self.id();
        let mode = match request.mode {
            Ieee802154SessionTxMode::Direct => TxMode::Direct,
            Ieee802154SessionTxMode::ClearChannelAssessment => TxMode::ClearChannelAssessment,
            Ieee802154SessionTxMode::CsmaCa { max_backoffs } => TxMode::CsmaCa { max_backoffs },
        };
        if let Err(result) = self
            .submit(RadioCommand::Transmit(TxRequest {
                id,
                frame,
                channel: self.channel,
                mode,
                transmit_power_dbm: None,
                max_frame_retries: request.max_frame_retries,
                security: TxSecurity::Radio,
                interface: Interface::PRIMARY,
                time_sync: None,
            }))
            .await
        {
            evidence.result = result;
            return evidence;
        }
        loop {
            let Ok(event) = with_timeout(TRANSMIT_TIMEOUT, self.taken()).await else {
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
            let Ok(event) = with_timeout(timeout, self.taken()).await else {
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
        if let Err(result) = self
            .submit(RadioCommand::EnergyScan(EnergyScanRequest {
                id,
                channel,
                duration_us: request.energy_scan_micros,
            }))
            .await
        {
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
        if let Err(result) = self
            .submit(RadioCommand::ClearChannelAssessment { id, channel })
            .await
        {
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
        let Ok(rssi_dbm) = self.port().recent_rssi();
        Ieee802154SessionRecentRssi {
            result: Ieee802154SessionResult::Done,
            rssi_dbm,
        }
    }

    /// Set the pending mode and add the host's source through the port; the
    /// pending table is software the next operation publishes, so a
    /// maintenance window does not refuse either.
    pub(super) async fn pending(&mut self, request: Ieee802154SessionPendingRequest) -> bool {
        let mode = match request.mode {
            Ieee802154SessionPendingMode::Disabled => AutoPendingMode::Disable,
            Ieee802154SessionPendingMode::Enabled => AutoPendingMode::Enable,
            Ieee802154SessionPendingMode::Enhanced => AutoPendingMode::Enhanced,
            Ieee802154SessionPendingMode::Zigbee => AutoPendingMode::Zigbee,
        };
        let id = self.id();
        if self
            .submit(RadioCommand::Configure {
                id,
                configuration: Configuration::PendingMode(mode),
            })
            .await
            .is_err()
        {
            return false;
        }
        match request.short_address {
            None => true,
            Some(short) => {
                let id = self.id();
                self.submit(RadioCommand::Configure {
                    id,
                    configuration: Configuration::AddPendingAddress(FrameAddress::Short(
                        short.to_le_bytes(),
                    )),
                })
                .await
                .is_ok()
            }
        }
    }
}
