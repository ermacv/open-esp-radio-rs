//! `openthread_radio::Radio` over an IEEE 802.15.4 radio port.

use heapless::Deque;
use oer_ieee802154::{
    AppliedSecurity, Channel, CommandError, Configuration, CorrelationIds, CslReceiver,
    EnergyScanRequest, EnhancedAckGeneration, EventsLost, FrameCounterUpdate, FrameView,
    Ieee802154Instant, Ieee802154RadioPort, Interface, LifecycleCommand, LifecycleError,
    LifecycleEvent, LinkMetrics, PendingTableHalf, ProbingInitiator, RadioCommand, RadioEvent,
    RadioSetting, RadioState, ReceivedFrame, RequestId, ScheduledReceiveRequest, SettingError,
    TimeSync, TxMode, TxRequest, TxSecurity,
};
use openthread_radio::{
    AckSecurity, Capabilities, Config, CslConfig, ENH_ACK_PROBING_CAPACITY, EnhAckProbingConfig,
    MacCapabilities, MacKeys, PsduMeta, Radio, RadioCaps, RadioClock, RadioErrorKind, RadioRssi,
    SentAck, SrcMatchConfig, TxFrame,
};

use crate::frames::{
    CSL_ACCURACY_PPM, CSL_UNCERTAINTY, TransmitFailure, csl_period, extended_address,
    pending_changes, pending_mode, probing_initiator, psdu_mac, radio_time, scan_micros,
    sent_ack_security, transmit_failure, tx_security, write_applied_security, write_psdu,
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

impl OpenThreadRadioDefaults {
    /// The Kconfig default `CONFIG_IEEE802154_CCA_THRESHOLD` of ESP-IDF
    /// (`esp_ieee802154_get_cca_threshold`), in dBm.
    pub const ESP_IDF_CCA_THRESHOLD_DBM: i8 = -75;

    /// The figures ESP-IDF's OpenThread port reports for a chip whose
    /// driver gives every channel `tx_power_dbm` (`ieee802154_pib_init`
    /// takes the highest level of the chip's transmit power table,
    /// `esp_ieee802154_get_txpower`) and whose receive sensitivity is
    /// `receive_sensitivity_dbm` (`IEEE802154_RX_SENSITIVITY` of the chip's
    /// `ieee802154_ll.h`, `esp_ieee802154_get_receive_sensitivity`), with
    /// the Kconfig CCA threshold [`Self::ESP_IDF_CCA_THRESHOLD_DBM`].
    pub const fn esp_idf(tx_power_dbm: i8, receive_sensitivity_dbm: i8) -> Self {
        Self {
            tx_power_dbm,
            cca_threshold_dbm: Self::ESP_IDF_CCA_THRESHOLD_DBM,
            receive_sensitivity_dbm,
        }
    }
}

/// An IEEE 802.15.4 radio port as an OpenThread radio.
///
/// Frames that arrive while a transmission or energy scan runs wait in a
/// queue of `QUEUE` frames for [`Radio::receive`]; a full queue drops the
/// newest, as the trait allows. A transmission whose future OpenThread drops
/// finishes in the backend; the next operation first waits for its end.
///
/// The adapter is the port's one event consumer; the composition polls the
/// backend's runner beside it. When the port reports [`EventsLost`], an
/// operation whose terminal event may be in the gap is cancelled by its
/// identity (or, without cancellation, judged by the radio's state), and a
/// receive reports the lost frames to OpenThread as `RxFailed`.
// CAPABILITY: ieee802154-product-stacks-thread
pub struct OpenThreadRadio<'r, P: Ieee802154RadioPort, const QUEUE: usize> {
    port: &'r P,
    defaults: OpenThreadRadioDefaults,
    /// Received-frame events.
    received: Deque<P::Event, QUEUE>,
    /// Received frames were lost since OpenThread last received.
    receive_lost: bool,
    /// An operation whose terminal event is still owed.
    pending: Option<RequestId>,
    ids: CorrelationIds,
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
    /// The Time IE the radio fills at the SFD.
    time_sync: Option<TimeSync>,
}

/// The terminal event of one operation.
enum Terminal<E> {
    Transmitted(E),
    Scanned(Option<i8>),
    /// The event was lost with an overflow, or the radio faulted.
    Lost,
}

impl<'r, P: Ieee802154RadioPort, const QUEUE: usize> OpenThreadRadio<'r, P, QUEUE> {
    /// Drive `port`, which the composition started. OpenThread reads the
    /// port's clock and live RSSI through [`PortClock`] and [`PortRssi`],
    /// which the composition passes to its constructor.
    pub const fn new(port: &'r P, defaults: OpenThreadRadioDefaults) -> Self {
        Self {
            port,
            defaults,
            received: Deque::new(),
            receive_lost: false,
            pending: None,
            ids: CorrelationIds::starting_at(1),
            cca_threshold: None,
            src_match: SrcMatchConfig::new(),
        }
    }

    /// Admit one configuration, telling a full pending table apart.
    fn submit_configuration(&mut self, configuration: Configuration) -> Result<(), PendingRefusal> {
        let id = self.id();
        match self
            .port
            .submit(RadioCommand::Configure { id, configuration })
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(CommandError::PendingTableFull)) => Err(PendingRefusal::TableFull),
            _ => Err(PendingRefusal::Other),
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
    fn radio_timestamp(&self, low: u32) -> Ieee802154Instant {
        Ieee802154Instant::from_micros(radio_time(PortClock::new(self.port).now_micros(), low))
    }

    /// A request identifier outside the backend-reserved range (the
    /// Espressif runtime's own pause, resume and lifecycle).
    fn id(&mut self) -> RequestId {
        self.ids.next()
    }

    /// Whether the operation `id`, whose terminal event may have been lost,
    /// still runs: a cancellation either ends it through its terminal event
    /// or is refused because it already ended. A port without cancellation
    /// is judged by its state.
    fn still_running(&mut self, id: RequestId) -> bool {
        let cancel = self.id();
        match self.port.submit(RadioCommand::Cancel {
            id: cancel,
            target: id,
        }) {
            Ok(Ok(_)) => true,
            Ok(Err(CommandError::Unsupported { .. })) => !matches!(
                self.port.state(),
                Ok(RadioState::Resting(_) | RadioState::Disabled) | Err(_)
            ),
            _ => false,
        }
    }

    /// Enable the radio through the port's lifecycle and wait for its
    /// terminal event, queueing frames meanwhile.
    async fn enable(&mut self) -> Result<(), RadioErrorKind> {
        match self.port.lifecycle(LifecycleCommand::Enable) {
            Ok(Ok(())) => {}
            Ok(Err(LifecycleError::AlreadyInState)) => return Ok(()),
            _ => return Err(RadioErrorKind::Other),
        }
        loop {
            let event = match self.port.next_event().await {
                Ok(event) => event,
                // The terminal may be in the gap: the state tells.
                Err(EventsLost) => {
                    self.receive_lost = true;
                    return match self.port.state() {
                        Ok(RadioState::Disabled) | Err(_) => Err(RadioErrorKind::Other),
                        Ok(_) => Ok(()),
                    };
                }
            };
            match P::view(&event) {
                RadioEvent::Received(_) => {
                    if self.received.push_back(event).is_err() {
                        self.receive_lost = true;
                    }
                }
                RadioEvent::Lifecycle(LifecycleEvent::Enabled) => return Ok(()),
                RadioEvent::Lifecycle(LifecycleEvent::Failed { .. }) | RadioEvent::Poisoned(_) => {
                    return Err(RadioErrorKind::Other);
                }
                _ => {}
            }
        }
    }

    fn submit(&mut self, command: RadioCommand<'_>) -> Result<(), RadioErrorKind> {
        match self.port.submit(command) {
            Ok(Ok(_)) => Ok(()),
            _ => Err(RadioErrorKind::Other),
        }
    }

    /// Change one setting of the port.
    fn apply(&self, setting: RadioSetting<'_>) -> Result<(), RadioErrorKind> {
        match self.port.apply(setting) {
            Ok(Ok(())) => Ok(()),
            _ => Err(RadioErrorKind::Other),
        }
    }

    /// Wait for the terminal event of `id`, queueing frames meanwhile.
    async fn terminal(&mut self, id: RequestId) -> Terminal<P::Event> {
        loop {
            let event = match self.port.next_event().await {
                Ok(event) => event,
                // The terminal event may have been lost with the overflow:
                // recover the operation by its identity.
                Err(EventsLost) => {
                    self.receive_lost = true;
                    if self.still_running(id) {
                        continue;
                    }
                    return Terminal::Lost;
                }
            };
            match P::view(&event) {
                RadioEvent::Received(_) => {
                    if self.received.push_back(event).is_err() {
                        self.receive_lost = true;
                    }
                }
                RadioEvent::TransmitDone { id: done, .. } if done == id => {
                    return Terminal::Transmitted(event);
                }
                RadioEvent::EnergyScanDone {
                    id: done,
                    energy_dbm,
                } if done == id => return Terminal::Scanned(Some(energy_dbm)),
                RadioEvent::EnergyScanFailed { id: done } if done == id => {
                    return Terminal::Scanned(None);
                }
                RadioEvent::Fault { .. } | RadioEvent::Poisoned(_) => return Terminal::Lost,
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
            time_sync,
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
            time_sync,
        })) {
            return (Err(error), None);
        }
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        let Terminal::Transmitted(event) = terminal else {
            return (Err(RadioErrorKind::Other), None);
        };
        let RadioEvent::TransmitDone {
            status,
            acknowledgement,
            security,
            ..
        } = P::view(&event)
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
                write_psdu(ack.frame.bytes(), buffer).map(|len| PsduMeta {
                    len,
                    channel: ack.metadata.channel.get(),
                    rssi: Some(ack.metadata.rssi_dbm),
                    lqi: Some(ack.metadata.link_quality),
                    ack: None,
                    timestamp: ack.metadata.timestamp.map(Ieee802154Instant::as_micros),
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

impl<P: Ieee802154RadioPort, const QUEUE: usize> Radio for OpenThreadRadio<'_, P, QUEUE> {
    type Error = RadioErrorKind;

    async fn init(&mut self) -> Result<RadioCaps, Self::Error> {
        self.settle().await;
        self.enable().await?;
        // The radio secures frames and enhanced ACKs with OpenThread's keys,
        // starting from the port's zeroed ones: raising the frame counter to
        // zero installs them and keeps keys already set.
        self.apply(RadioSetting::FrameCounter {
            interface: Interface::PRIMARY,
            update: FrameCounterUpdate::SetIfLarger(0),
        })?;
        // Link margins are measured from the receive sensitivity the radio
        // reports; ESP-IDF's port leaves the noise floor at zero.
        self.apply(RadioSetting::EnhancedAck(Some(EnhancedAckGeneration {
            noise_floor_dbm: self.defaults.receive_sensitivity_dbm,
        })))?;
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
            time_sync: None,
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
                    time_sync: frame.time_sync.map(|ie| TimeSync {
                        ie_offset: ie.ie_offset,
                        sequence: ie.sequence,
                        network_time_offset: ie.network_time_offset,
                    }),
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
        self.apply(RadioSetting::Csl(CslReceiver {
            period: csl_period(csl.period),
            sample_time: csl.sample_time,
        }))
    }

    async fn set_enh_ack_probing(
        &mut self,
        config: &EnhAckProbingConfig,
    ) -> Result<(), Self::Error> {
        let initiators: heapless::Vec<ProbingInitiator, ENH_ACK_PROBING_CAPACITY> = config
            .initiators
            .iter()
            .map(|initiator| {
                probing_initiator(
                    initiator.short_address,
                    initiator.ext_address,
                    LinkMetrics {
                        pdu_count: initiator.metrics.pdu_count,
                        lqi: initiator.metrics.lqi,
                        link_margin: initiator.metrics.link_margin,
                        rssi: initiator.metrics.rssi,
                    },
                )
            })
            .collect();
        match self
            .port
            .apply(RadioSetting::EnhancedAckProbing(&initiators))
        {
            // Without a generator there is no ACK to probe in.
            Ok(Ok(())) | Ok(Err(SettingError::EnhancedAckDisabled)) => Ok(()),
            _ => Err(RadioErrorKind::Other),
        }
    }

    async fn set_mac_keys(&mut self, keys: &MacKeys) -> Result<(), Self::Error> {
        self.apply(RadioSetting::MacKeys {
            interface: Interface::PRIMARY,
            key_id: keys.key_id,
            previous: keys.previous,
            current: keys.current,
            next: keys.next,
        })
    }

    async fn set_mac_frame_counter(
        &mut self,
        update: openthread_radio::FrameCounterUpdate,
    ) -> Result<(), Self::Error> {
        let update = match update {
            openthread_radio::FrameCounterUpdate::Set(counter) => FrameCounterUpdate::Set(counter),
            openthread_radio::FrameCounterUpdate::SetIfLarger(counter) => {
                FrameCounterUpdate::SetIfLarger(counter)
            }
        };
        self.apply(RadioSetting::FrameCounter {
            interface: Interface::PRIMARY,
            update,
        })
    }

    /// A loss of received frames is reported once as `RxFailed`, after the
    /// frames queued before it.
    async fn receive(&mut self, psdu_buf: &mut [u8]) -> Result<PsduMeta, Self::Error> {
        let event = loop {
            if let Some(event) = self.received.pop_front() {
                break event;
            }
            if core::mem::take(&mut self.receive_lost) {
                return Err(RadioErrorKind::RxFailed);
            }
            match self.port.next_event().await {
                Ok(event) => match P::view(&event) {
                    RadioEvent::Received(_) => break event,
                    RadioEvent::Poisoned(_) => return Err(RadioErrorKind::Other),
                    // The end of an abandoned operation.
                    view => {
                        if let Some(id) = self.pending
                            && terminal_of(view) == Some(id)
                        {
                            self.pending = None;
                        }
                    }
                },
                // Frames, and perhaps the end of an abandoned operation,
                // are in the gap.
                Err(EventsLost) => {
                    if let Some(id) = self.pending
                        && !self.still_running(id)
                    {
                        self.pending = None;
                    }
                    return Err(RadioErrorKind::RxFailed);
                }
            }
        };
        match P::view(&event) {
            RadioEvent::Received(frame) => received_meta(frame, psdu_buf),
            // Only received frames are kept.
            _ => Err(RadioErrorKind::RxInvalid),
        }
    }
}

/// Copy a received frame into OpenThread's PSDU buffer with its metadata.
fn received_meta(
    frame: ReceivedFrame<'_>,
    psdu_buf: &mut [u8],
) -> Result<PsduMeta, RadioErrorKind> {
    let len = write_psdu(frame.frame.bytes(), psdu_buf).ok_or(RadioErrorKind::RxInvalid)?;
    let sent = frame.metadata.sent_acknowledgement;
    Ok(PsduMeta {
        len,
        channel: frame.metadata.channel.get(),
        rssi: Some(frame.metadata.rssi_dbm),
        lqi: Some(frame.metadata.link_quality),
        timestamp: frame.metadata.timestamp.map(Ieee802154Instant::as_micros),
        ack: Some(SentAck {
            frame_pending: sent.frame_pending,
            security: sent_ack_security(sent).map(|(frame_counter, key_id)| AckSecurity {
                frame_counter,
                key_id,
            }),
        }),
    })
}

/// The request a terminal event ends.
fn terminal_of(event: RadioEvent<'_>) -> Option<RequestId> {
    match event {
        RadioEvent::TransmitDone { id, .. }
        | RadioEvent::EnergyScanDone { id, .. }
        | RadioEvent::EnergyScanFailed { id }
        | RadioEvent::ClearChannelAssessmentDone { id, .. }
        | RadioEvent::ClearChannelAssessmentFailed { id }
        | RadioEvent::ScheduledReceiveDone { id } => Some(id),
        RadioEvent::Fault { id, .. } => id,
        RadioEvent::Received(_) | RadioEvent::Lifecycle(_) | RadioEvent::Poisoned(_) => None,
    }
}

#[cfg(test)]
mod tests;

/// The clock of an IEEE 802.15.4 radio port for OpenThread's synchronous
/// `otPlatRadioGetNow` and `otPlatTimeGet`: the port's
/// [`Ieee802154RadioPort::now`], the epoch of its scheduled operations and
/// receive timestamps. Pass it to the OpenThread constructor with the port
/// [`OpenThreadRadio`] drives.
pub struct PortClock<'r, P>(&'r P);

impl<'r, P: Ieee802154RadioPort> PortClock<'r, P> {
    /// The clock of `port`.
    pub const fn new(port: &'r P) -> Self {
        Self(port)
    }
}

impl<P: Ieee802154RadioPort> RadioClock for PortClock<'_, P> {
    /// # Panics
    ///
    /// When the port's clock fails: a poisoned port is terminal, and
    /// OpenThread has no way to hear of it, so no time is invented.
    fn now_micros(&self) -> u64 {
        match self.0.now() {
            Ok(now) => now.as_micros(),
            Err(_) => panic!("the IEEE 802.15.4 radio port's clock failed: the port is terminal"),
        }
    }
}

/// The live RSSI of an IEEE 802.15.4 radio port for OpenThread's synchronous
/// `otPlatRadioGetRssi`: the port's [`Ieee802154RadioPort::recent_rssi`],
/// `None` while the port cannot read it.
pub struct PortRssi<'r, P>(&'r P);

impl<'r, P: Ieee802154RadioPort> PortRssi<'r, P> {
    /// The live RSSI of `port`.
    pub const fn new(port: &'r P) -> Self {
        Self(port)
    }
}

impl<P: Ieee802154RadioPort> RadioRssi for PortRssi<'_, P> {
    fn rssi(&self) -> Option<i8> {
        self.0.recent_rssi().ok()
    }
}
