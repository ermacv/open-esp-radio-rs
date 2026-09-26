//! `openthread::Radio` over the IEEE 802.15.4 runtime.

use embassy_sync::blocking_mutex::raw::RawMutex;
use heapless::Deque;
use oer_esp32s31_hal::ieee802154::ll::Ieee802154LowLevel;
use oer_esp32s31_ieee802154_runtime::{
    Ieee802154EnhancedAckGenerator, Ieee802154OwnedFrame, Ieee802154RadioEvent, Ieee802154Runtime,
};
use oer_ieee802154::{
    Channel, CommandError, Configuration, EnergyScanRequest, FrameView, RadioCommand, RadioState,
    RequestId, TxMode, TxRequest, TxSecurity,
};
use openthread::{
    Capabilities, Config, MacCapabilities, PsduMeta, Radio, RadioCaps, RadioErrorKind,
    SrcMatchConfig,
};

use crate::frames::{
    TransmitFailure, extended_address, extended_pending_address, pending_mode, psdu_mac,
    scan_micros, short_pending_address, transmit_failure, write_psdu,
};

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
        }
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
        // 2015 frames are acknowledged; OpenThread passes no keys, so
        // secured ones are not.
        self.runtime
            .with_enhanced_ack(|generator| *generator = Some(Ieee802154EnhancedAckGenerator::new()))
            .map_err(|_| RadioErrorKind::Other)?;
        Ok(RadioCaps {
            phy: Capabilities::ACK_TIMEOUT
                .union(Capabilities::ENERGY_SCAN)
                .union(Capabilities::SLEEP_TO_TX),
            mac: MacCapabilities::all(),
            receive_sensitivity: self.defaults.receive_sensitivity_dbm,
            default_tx_power: self.defaults.tx_power_dbm,
            default_cca_threshold: self.defaults.cca_threshold_dbm,
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
        self.runtime
            .with_pending_table(|table| {
                *table = oer_ieee802154::PendingTable::new();
                for &short in &config.short_addrs {
                    let _ = table.add(short_pending_address(short));
                }
                for &extended in &config.ext_addrs {
                    let _ = table.add(extended_pending_address(extended));
                }
            })
            .map_err(|_| RadioErrorKind::Other)?;
        self.runtime
            .set_pending_mode(pending_mode(config.enabled))
            .map_err(|_| RadioErrorKind::Other)
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

    async fn transmit(
        &mut self,
        psdu: &[u8],
        number: u8,
        power: i8,
        cca_threshold: Option<i8>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let frame = psdu_mac(psdu)
            .and_then(|mac| FrameView::new(mac).ok())
            .ok_or(RadioErrorKind::TxInvalid)?;
        let id = self.id();
        // The threshold stays the radio's own; its presence asks for a CCA.
        let mode = if cca_threshold.is_some() {
            TxMode::ClearChannelAssessment
        } else {
            TxMode::Direct
        };
        self.submit(RadioCommand::Transmit(TxRequest {
            id,
            frame,
            channel,
            mode,
            transmit_power_dbm: Some(power),
            max_frame_retries: 0,
            security: TxSecurity::Radio,
        }))?;
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        let Terminal::Transmitted(Ieee802154RadioEvent::TransmitDone {
            status,
            acknowledgement,
            ..
        }) = terminal
        else {
            return Err(RadioErrorKind::Other);
        };
        if let Some(failure) = transmit_failure(status) {
            return Err(match failure {
                TransmitFailure::ChannelAccess => RadioErrorKind::TxFailed,
                TransmitFailure::NoAcknowledgement => RadioErrorKind::RxAckTimeout,
                TransmitFailure::InvalidAcknowledgement => RadioErrorKind::RxAckInvalid,
                TransmitFailure::InvalidFrame => RadioErrorKind::TxInvalid,
                TransmitFailure::Other => RadioErrorKind::Other,
            });
        }
        Ok(match (acknowledgement, ack_psdu_buf) {
            (Some(ack), Some(buffer)) => {
                write_psdu(ack.frame.as_bytes(), buffer).map(|len| PsduMeta {
                    len,
                    channel: ack.metadata.channel.get(),
                    rssi: Some(ack.metadata.rssi_dbm),
                    lqi: Some(ack.metadata.link_quality),
                })
            }
            _ => None,
        })
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
        Ok(PsduMeta {
            len,
            channel: frame.metadata.channel.get(),
            rssi: Some(frame.metadata.rssi_dbm),
            lqi: Some(frame.metadata.link_quality),
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
        | Ieee802154RadioEvent::ClearChannelAssessmentFailed { id } => Some(id),
        Ieee802154RadioEvent::Fault { id, .. } => id,
        Ieee802154RadioEvent::Received(_) => None,
    }
}
