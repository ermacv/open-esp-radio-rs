//! `openthread::Radio` over the IEEE 802.15.4 runtime.

use embassy_sync::blocking_mutex::raw::RawMutex;
use heapless::Deque;
use oer_esp32s31_hal::ieee802154::ll::Ieee802154LowLevel;
use oer_esp32s31_ieee802154_runtime::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154OwnedFrame, Ieee802154RadioEvent,
    Ieee802154Runtime, Ieee802154RuntimeError,
};
use oer_ieee802154::{
    AppliedSecurity, Channel, CommandError, Configuration, EnergyScanRequest, FrameView, Interface,
    LinkMetrics, PendingTableHalf, RadioCommand, RadioState, RadioTimestamp, RequestId,
    ScheduledReceiveRequest, TxMode, TxRequest, TxSecurity,
};
use openthread::{
    AckSecurity, Capabilities, Config, CslConfig, EnhAckProbingConfig, FrameCounterUpdate,
    MacCapabilities, MacKeys, PsduMeta, Radio, RadioCaps, RadioClock, RadioErrorKind, SentAck,
    SrcMatchConfig, TxFrame,
};

use crate::frames::{
    CSL_ACCURACY_PPM, CSL_UNCERTAINTY, PORT_INITIAL_KEYS, TransmitFailure, csl_period,
    extended_address, pending_changes, pending_mode, psdu_mac, radio_time, replace_enh_ack_probing,
    scan_micros, sent_ack_security, set_frame_counter, set_mac_keys, transmit_failure, tx_security,
    write_applied_security, write_psdu,
};

/// The PHY capabilities the radio reports, as ESP-IDF's OpenThread port
/// reports them (`otPlatRadioGetCaps`). Declare them to OpenThread before
/// its instance is built (`OtResources::set_radio_caps`).
pub const OPEN_THREAD_RADIO_CAPABILITIES: Capabilities = Capabilities::ACK_TIMEOUT
    .union(Capabilities::ENERGY_SCAN)
    .union(Capabilities::SLEEP_TO_TX)
    .union(Capabilities::TRANSMIT_SEC)
    .union(Capabilities::RECEIVE_TIMING)
    .union(Capabilities::TRANSMIT_TIMING);

/// Figures the radio reports to OpenThread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenThreadRadioDefaults {
    /// Transmit power in dBm.
    pub tx_power_dbm: i8,
    /// CCA energy threshold in dBm.
    pub cca_threshold_dbm: i8,
    /// Receive sensitivity in dBm.
    pub receive_sensitivity_dbm: i8,
}

/// The ESP32-S31 IEEE 802.15.4 runtime as an OpenThread radio.
///
/// Frames that arrive while a transmission or energy scan runs wait in a
/// queue of `QUEUE` frames for [`Radio::receive`]; a full queue drops the
/// newest, as the trait allows. A transmission whose future OpenThread drops
/// finishes in the runtime; the next operation first waits for its end.
pub struct OpenThreadRadio<'r, 's, M: RawMutex, H, const EVENTS: usize, const QUEUE: usize> {
    runtime: &'r Ieee802154Runtime<'s, M, H, EVENTS>,
    defaults: OpenThreadRadioDefaults,
    received: Deque<Ieee802154OwnedFrame, QUEUE>,
    /// An operation whose terminal event is still owed.
    pending: Option<RequestId>,
    next_id: u32,
    /// The runtime's radio clock, once the radio is initialized.
    clock: Option<RadioClock>,
    /// The CCA threshold the radio holds, in dBm.
    cca_threshold: Option<i8>,
    /// The source-match table the radio holds.
    src_match: SrcMatchConfig,
}

/// Why a source-match change was refused.
enum PendingRefusal {
    TableFull,
    Other,
}

/// One frame to transmit and how.
struct Outgoing<'a> {
    psdu: &'a [u8],
    channel: u8,
    power: i8,
    /// Its presence asks for a CCA at the radio's own threshold.
    cca_threshold: Option<i8>,
    /// The start time in the low 32 bits of the radio clock.
    tx_at: Option<u32>,
    security: TxSecurity,
}

/// The terminal event of one operation.
enum Terminal {
    Transmitted(Ieee802154RadioEvent),
    Scanned(Option<i8>),
    /// The event was lost with an overflow, or the radio faulted.
    Lost,
}

impl<'r, 's, M, H, const EVENTS: usize, const QUEUE: usize>
    OpenThreadRadio<'r, 's, M, H, EVENTS, QUEUE>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
{
    /// Drive `runtime`, which the composition started.
    pub const fn new(
        runtime: &'r Ieee802154Runtime<'s, M, H, EVENTS>,
        defaults: OpenThreadRadioDefaults,
    ) -> Self {
        Self {
            runtime,
            defaults,
            received: Deque::new(),
            pending: None,
            next_id: 0,
            clock: None,
            cca_threshold: None,
            src_match: SrcMatchConfig::new(),
        }
    }

    /// Admit one configuration, telling a full pending table apart.
    fn submit_configuration(&mut self, configuration: Configuration) -> Result<(), PendingRefusal> {
        let id = self.id();
        match self
            .runtime
            .submit(RadioCommand::Configure { id, configuration })
        {
            Ok(_) => Ok(()),
            Err(Ieee802154RuntimeError::Rejected(CommandError::PendingTableFull)) => {
                Err(PendingRefusal::TableFull)
            }
            Err(_) => Err(PendingRefusal::Other),
        }
    }

    /// Give the radio OpenThread's CCA threshold, as the port's
    /// `otPlatRadioSetCcaEnergyDetectThreshold` does, when it changed.
    fn apply_cca_threshold(&mut self, threshold: i8) -> Result<(), RadioErrorKind> {
        if self.cca_threshold != Some(threshold) {
            self.configure(Configuration::CcaThresholdDbm(threshold))?;
            self.cca_threshold = Some(threshold);
        }
        Ok(())
    }

    /// The full radio-clock instant of a 32-bit OpenThread radio time.
    fn radio_timestamp(&self, low: u32) -> RadioTimestamp {
        let now = self.clock.map_or(0, |clock| clock());
        RadioTimestamp::from_micros(radio_time(now, low))
    }

    /// A request identifier; the runtime reserves `u32::MAX`.
    fn id(&mut self) -> RequestId {
        self.next_id = if self.next_id >= u32::MAX - 1 {
            1
        } else {
            self.next_id + 1
        };
        RequestId::new(self.next_id)
    }

    fn submit(&mut self, command: RadioCommand<'_>) -> Result<(), RadioErrorKind> {
        match self.runtime.submit(command) {
            Ok(_) => Ok(()),
            Err(_) => Err(RadioErrorKind::Other),
        }
    }

    fn queue(&mut self, frame: Ieee802154OwnedFrame) {
        let _ = self.received.push_back(frame);
    }

    /// Wait for the terminal event of `id`, queueing frames meanwhile.
    async fn terminal(&mut self, id: RequestId) -> Terminal {
        loop {
            let event = match self.runtime.next_event().await {
                Ok(event) => event,
                // The terminal event may have been lost with the overflow.
                Err(_) => match self.runtime.state() {
                    Ok(RadioState::Resting(_)) | Ok(RadioState::Disabled) | Err(_) => {
                        return Terminal::Lost;
                    }
                    Ok(_) => continue,
                },
            };
            match event {
                Ieee802154RadioEvent::Received(frame) => self.queue(frame),
                Ieee802154RadioEvent::TransmitDone { id: done, .. } if done == id => {
                    return Terminal::Transmitted(event);
                }
                Ieee802154RadioEvent::EnergyScanDone {
                    id: done,
                    energy_dbm,
                } if done == id => return Terminal::Scanned(Some(energy_dbm)),
                Ieee802154RadioEvent::EnergyScanFailed { id: done } if done == id => {
                    return Terminal::Scanned(None);
                }
                Ieee802154RadioEvent::Fault { .. } => return Terminal::Lost,
                // The end of an operation OpenThread abandoned earlier.
                _ => {}
            }
        }
    }

    /// Wait for the end of an operation whose future was dropped.
    async fn settle(&mut self) {
        if let Some(id) = self.pending {
            let _ = self.terminal(id).await;
            self.pending = None;
        }
    }

    fn configure(&mut self, configuration: Configuration) -> Result<(), RadioErrorKind> {
        let id = self.id();
        self.submit(RadioCommand::Configure { id, configuration })
    }

    /// Transmit `psdu` under `security`; also report the security header
    /// fields the radio wrote, whatever the outcome.
    async fn send(
        &mut self,
        outgoing: Outgoing<'_>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> (
        Result<Option<PsduMeta>, RadioErrorKind>,
        Option<AppliedSecurity>,
    ) {
        let Outgoing {
            psdu,
            channel: number,
            power,
            cca_threshold,
            tx_at,
            security,
        } = outgoing;
        self.settle().await;
        let channel = match channel(number) {
            Ok(channel) => channel,
            Err(error) => return (Err(error), None),
        };
        let Some(frame) = psdu_mac(psdu).and_then(|mac| FrameView::new(mac).ok()) else {
            return (Err(RadioErrorKind::TxInvalid), None);
        };
        if let Some(threshold) = cca_threshold
            && let Err(error) = self.apply_cca_threshold(threshold)
        {
            return (Err(error), None);
        }
        let id = self.id();
        // The threshold's presence asks for a CCA.
        // A delayed frame starts at its time (`esp_ieee802154_transmit_at`).
        let mode = match (tx_at, cca_threshold.is_some()) {
            (Some(at), cca) => TxMode::Scheduled {
                at: self.radio_timestamp(at),
                cca,
            },
            (None, true) => TxMode::ClearChannelAssessment,
            (None, false) => TxMode::Direct,
        };
        if let Err(error) = self.submit(RadioCommand::Transmit(TxRequest {
            id,
            frame,
            channel,
            mode,
            transmit_power_dbm: Some(power),
            max_frame_retries: 0,
            security,
            interface: Interface::PRIMARY,
        })) {
            return (Err(error), None);
        }
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        let Terminal::Transmitted(Ieee802154RadioEvent::TransmitDone {
            status,
            acknowledgement,
            security,
            ..
        }) = terminal
        else {
            return (Err(RadioErrorKind::Other), None);
        };
        if let Some(failure) = transmit_failure(status) {
            let error = match failure {
                TransmitFailure::ChannelAccess => RadioErrorKind::TxFailed,
                TransmitFailure::NoAcknowledgement => RadioErrorKind::RxAckTimeout,
                TransmitFailure::InvalidAcknowledgement => RadioErrorKind::RxAckInvalid,
                TransmitFailure::InvalidFrame => RadioErrorKind::TxInvalid,
                TransmitFailure::Other => RadioErrorKind::Other,
            };
            return (Err(error), security);
        }
        let ack = match (acknowledgement, ack_psdu_buf) {
            (Some(ack), Some(buffer)) => {
                write_psdu(ack.frame.as_bytes(), buffer).map(|len| PsduMeta {
                    len,
                    channel: ack.metadata.channel.get(),
                    rssi: Some(ack.metadata.rssi_dbm),
                    lqi: Some(ack.metadata.link_quality),
                    ack: None,
                    timestamp: ack.metadata.timestamp.map(RadioTimestamp::as_micros),
                })
            }
            _ => None,
        };
        (Ok(ack), security)
    }
}

fn channel(number: u8) -> Result<Channel, RadioErrorKind> {
    Channel::new(number).map_err(|_| RadioErrorKind::Other)
}

impl<M, H, const EVENTS: usize, const QUEUE: usize> Radio
    for OpenThreadRadio<'_, '_, M, H, EVENTS, QUEUE>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
{
    type Error = RadioErrorKind;

    async fn init(&mut self) -> Result<RadioCaps, Self::Error> {
        self.settle().await;
        let id = self.id();
        match self.runtime.submit(RadioCommand::Enable { id }) {
            Ok(_)
            | Err(oer_esp32s31_ieee802154_runtime::Ieee802154RuntimeError::Rejected(
                CommandError::AlreadyEnabled,
            )) => {}
            Err(_) => return Err(RadioErrorKind::Other),
        }
        // The radio secures frames and enhanced ACKs with OpenThread's keys,
        // starting from the port's zeroed ones.
        self.runtime
            .with_mac_keys(|keys| {
                keys.get_or_insert(PORT_INITIAL_KEYS);
            })
            .map_err(|_| RadioErrorKind::Other)?;
        // Link margins are measured from the receive sensitivity the radio
        // reports; ESP-IDF's port leaves the noise floor at zero.
        let noise_floor = self.defaults.receive_sensitivity_dbm;
        self.runtime
            .with_enhanced_ack(|generator| {
                let mut installed = Ieee802154EnhancedAckGenerator::new();
                installed.probing().set_noise_floor(noise_floor);
                *generator = Some(installed);
            })
            .map_err(|_| RadioErrorKind::Other)?;
        self.clock = Some(self.runtime.clock().map_err(|_| RadioErrorKind::Other)?);
        // The radio holds the threshold OpenThread starts from.
        self.cca_threshold = None;
        self.apply_cca_threshold(self.defaults.cca_threshold_dbm)?;
        // And the empty source-match table it starts from: every poll is
        // answered with frame pending.
        self.src_match = SrcMatchConfig::new();
        for configuration in [
            Configuration::ResetPendingTable(PendingTableHalf::Short),
            Configuration::ResetPendingTable(PendingTableHalf::Extended),
            Configuration::PendingMode(pending_mode(false)),
        ] {
            self.configure(configuration)?;
        }
        Ok(RadioCaps {
            phy: OPEN_THREAD_RADIO_CAPABILITIES,
            mac: MacCapabilities::all(),
            receive_sensitivity: self.defaults.receive_sensitivity_dbm,
            default_tx_power: self.defaults.tx_power_dbm,
            default_cca_threshold: self.defaults.cca_threshold_dbm,
            csl_accuracy: CSL_ACCURACY_PPM,
            csl_uncertainty: CSL_UNCERTAINTY,
        })
    }

    async fn set_config(&mut self, config: &Config) -> Result<(), Self::Error> {
        self.settle().await;
        if let Some(pan_id) = config.pan_id {
            self.configure(Configuration::PanId(pan_id))?;
        }
        if let Some(short) = config.short_addr {
            self.configure(Configuration::ShortAddress(short))?;
        }
        if let Some(extended) = config.ext_addr {
            self.configure(Configuration::ExtendedAddress(extended_address(extended)))?;
        }
        self.configure(Configuration::Promiscuous(config.promiscuous))
    }

    async fn set_src_match_config(&mut self, config: &SrcMatchConfig) -> Result<(), Self::Error> {
        self.settle().await;
        if config.enabled != self.src_match.enabled {
            self.configure(Configuration::PendingMode(pending_mode(config.enabled)))?;
        }
        let old = core::mem::replace(&mut self.src_match, config.clone());
        let mut result = Ok(());
        pending_changes(
            &old.short_addrs,
            &config.short_addrs,
            &old.ext_addrs,
            &config.ext_addrs,
            |change| {
                // As ESP-IDF's port, a full table is no error.
                match self.submit_configuration(change) {
                    Ok(()) | Err(PendingRefusal::TableFull) => {}
                    Err(PendingRefusal::Other) if result.is_ok() => {
                        result = Err(RadioErrorKind::Other);
                    }
                    Err(PendingRefusal::Other) => {}
                }
            },
        );
        result
    }

    async fn set_receive(&mut self, number: u8) -> Result<(), Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let id = self.id();
        self.submit(RadioCommand::Receive { id, channel })
    }

    async fn set_sleep(&mut self) -> Result<(), Self::Error> {
        self.settle().await;
        let id = self.id();
        self.submit(RadioCommand::Sleep { id })
    }

    async fn energy_scan(&mut self, number: u8, duration_millis: u16) -> Result<i8, Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let id = self.id();
        self.submit(RadioCommand::EnergyScan(EnergyScanRequest {
            id,
            channel,
            duration_us: scan_micros(duration_millis),
        }))?;
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        match terminal {
            Terminal::Scanned(Some(energy)) => Ok(energy),
            _ => Err(RadioErrorKind::Other),
        }
    }

    /// Transmit `psdu` as given: the radio does not secure it.
    async fn transmit(
        &mut self,
        psdu: &[u8],
        number: u8,
        power: i8,
        cca_threshold: Option<i8>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        let security = TxSecurity::Processed;
        let outgoing = Outgoing {
            psdu,
            channel: number,
            power,
            cca_threshold,
            tx_at: None,
            security,
        };
        self.send(outgoing, ack_psdu_buf).await.0
    }

    async fn transmit_frame(
        &mut self,
        frame: &mut TxFrame<'_>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        let security = tx_security(frame.retransmission, frame.security_processed);
        let (result, applied) = self
            .send(
                Outgoing {
                    psdu: frame.psdu,
                    channel: frame.channel,
                    power: frame.power,
                    cca_threshold: frame.cca_threshold,
                    tx_at: frame.tx_at,
                    security,
                },
                ack_psdu_buf,
            )
            .await;
        if let Some(applied) = applied {
            frame.header_updated = write_applied_security(applied, frame.psdu);
        }
        result
    }

    fn clock(&self) -> RadioClock {
        self.clock.unwrap_or(openthread::embassy_radio_clock)
    }

    async fn receive_at(
        &mut self,
        number: u8,
        start: u32,
        duration: u32,
    ) -> Result<(), Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let id = self.id();
        let start = self.radio_timestamp(start);
        self.submit(RadioCommand::ScheduledReceive(ScheduledReceiveRequest {
            id,
            channel,
            start,
            duration_us: duration,
        }))
    }

    async fn set_csl(&mut self, csl: CslConfig) -> Result<(), Self::Error> {
        self.runtime
            .with_csl(|installed| {
                *installed = Ieee802154Csl {
                    period: csl_period(csl.period),
                    sample_time: csl.sample_time,
                }
            })
            .map_err(|_| RadioErrorKind::Other)
    }

    async fn set_enh_ack_probing(
        &mut self,
        config: &EnhAckProbingConfig,
    ) -> Result<(), Self::Error> {
        self.runtime
            .with_enhanced_ack(|generator| {
                if let Some(generator) = generator {
                    replace_enh_ack_probing(
                        generator.probing(),
                        config.initiators.iter().map(|initiator| {
                            (
                                initiator.short_address,
                                initiator.ext_address,
                                LinkMetrics {
                                    pdu_count: initiator.metrics.pdu_count,
                                    lqi: initiator.metrics.lqi,
                                    link_margin: initiator.metrics.link_margin,
                                    rssi: initiator.metrics.rssi,
                                },
                            )
                        }),
                    );
                }
            })
            .map_err(|_| RadioErrorKind::Other)
    }

    async fn set_mac_keys(&mut self, keys: &MacKeys) -> Result<(), Self::Error> {
        self.runtime
            .with_mac_keys(|installed| {
                set_mac_keys(
                    installed,
                    keys.key_id,
                    keys.previous,
                    keys.current,
                    keys.next,
                );
            })
            .map_err(|_| RadioErrorKind::Other)
    }

    async fn set_mac_frame_counter(
        &mut self,
        update: FrameCounterUpdate,
    ) -> Result<(), Self::Error> {
        let (counter, if_larger) = match update {
            FrameCounterUpdate::Set(counter) => (counter, false),
            FrameCounterUpdate::SetIfLarger(counter) => (counter, true),
        };
        self.runtime
            .with_mac_keys(|keys| set_frame_counter(keys, counter, if_larger))
            .map_err(|_| RadioErrorKind::Other)
    }

    async fn receive(&mut self, psdu_buf: &mut [u8]) -> Result<PsduMeta, Self::Error> {
        let frame = loop {
            if let Some(frame) = self.received.pop_front() {
                break frame;
            }
            match self.runtime.next_event().await {
                Ok(Ieee802154RadioEvent::Received(frame)) => break frame,
                // The end of an abandoned operation.
                Ok(event) => {
                    if let Some(id) = self.pending
                        && terminal_of(&event) == Some(id)
                    {
                        self.pending = None;
                    }
                }
                Err(_) => {}
            }
        };
        let len = write_psdu(frame.frame.as_bytes(), psdu_buf).ok_or(RadioErrorKind::RxInvalid)?;
        let sent = frame.metadata.sent_acknowledgement;
        Ok(PsduMeta {
            len,
            channel: frame.metadata.channel.get(),
            rssi: Some(frame.metadata.rssi_dbm),
            lqi: Some(frame.metadata.link_quality),
            timestamp: frame.metadata.timestamp.map(RadioTimestamp::as_micros),
            ack: Some(SentAck {
                frame_pending: sent.frame_pending,
                security: sent_ack_security(sent).map(|(frame_counter, key_id)| AckSecurity {
                    frame_counter,
                    key_id,
                }),
            }),
        })
    }
}

/// The request a terminal event ends.
fn terminal_of(event: &Ieee802154RadioEvent) -> Option<RequestId> {
    match *event {
        Ieee802154RadioEvent::TransmitDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanFailed { id }
        | Ieee802154RadioEvent::ClearChannelAssessmentDone { id, .. }
        | Ieee802154RadioEvent::ClearChannelAssessmentFailed { id }
        | Ieee802154RadioEvent::ScheduledReceiveDone { id } => Some(id),
        Ieee802154RadioEvent::Fault { id, .. } => id,
        Ieee802154RadioEvent::Received(_) => None,
    }
}
