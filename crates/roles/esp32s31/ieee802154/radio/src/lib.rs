#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The portable IEEE 802.15.4 radio contract (`oer-ieee802154`) over the
//! ESP32-S31 MAC engine ported from the public ESP-IDF driver.
//!
//! [`Ieee802154Radio`] admits each [`RadioCommand`] through the portable
//! [`RadioStateMachine`], starts the corresponding engine operation and
//! translates engine notifications into correlated [`RadioEvent`] values for
//! a [`Ieee802154RadioSink`]. It names no executor and holds no hardware: the
//! caller passes the LL port to each entry and serializes the entries, as
//! the runtime does under its lock.
//!
//! The composition owns power, clocks, PHY and the MAC initialization; the
//! portable `Enable` and `Disable` commands only open and close admission.
//! Receive-ring slots never outlive one delivery: a frame is lent to the
//! sink and its slot returns to the ring before the entry ends.

#[cfg(test)]
extern crate std;

use oer_esp32s31_hal::ieee802154::{
    Ieee802154Channel,
    ll::{Ieee802154LowLevel, Ieee802154RxStatus},
};
use oer_esp32s31_ieee802154::engine::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154Environment, Ieee802154FrameInfo,
    Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154TxError,
};
use oer_ieee802154::{
    AcceptedCommand, Channel, CommandError, Configuration, FcsStatus, FramePending, FrameView,
    RadioCapabilities, RadioCommand, RadioEvent, RadioFault, RadioState, RadioStateMachine,
    RadioTimestamp, ReceivedFrame, RequestId, RestingState, RxMetadata, SecurityStatus, TxMode,
    TxStatus,
};

/// The portable capabilities the engine implements.
///
/// CSMA/CA, security offload and source matching have no portable command
/// yet and are not advertised.
pub const IEEE802154_RADIO_CAPABILITIES: RadioCapabilities = RadioCapabilities::NONE
    .union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
    .union(RadioCapabilities::ENERGY_SCAN)
    .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SCHEDULED_TRANSMIT)
    .union(RadioCapabilities::TRANSMIT_POWER)
    .union(RadioCapabilities::PROMISCUOUS)
    .union(RadioCapabilities::RECEIVE_TIMESTAMP)
    .union(RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT);

/// Builds the enhanced ACK for a received 2015 frame inside the interrupt
/// handler (`esp_ieee802154_enh_ack_generator`); `false` refuses, and the
/// frame is delivered without an ACK.
pub type Ieee802154EnhancedAckGenerator =
    fn(frame: &[u8; FRAME_SIZE], info: &Ieee802154FrameInfo, ack: &mut [u8; FRAME_SIZE]) -> bool;

/// Platform services the engine calls during an entry.
#[derive(Clone, Copy)]
pub struct Ieee802154Platform {
    /// The monotonic microsecond clock (`esp_timer_get_time`); the engine
    /// truncates it to the vendor's wrapping 32-bit timer domain, and receive
    /// timestamps use it as the radio epoch.
    pub now_micros: fn() -> u64,
    /// The enhanced-ACK generator; `None` refuses every enhanced ACK.
    pub enhanced_ack: Option<Ieee802154EnhancedAckGenerator>,
}

/// Receives the correlated portable events of one entry. Frames are lent
/// only for the call.
pub trait Ieee802154RadioSink {
    /// Deliver one event.
    fn event(&mut self, event: RadioEvent<'_>);
}

/// One engine notification the portable contract carries.
#[derive(Clone, Copy)]
enum Notification {
    Received(Ieee802154RxSlot),
    Transmitted(Option<Ieee802154RxSlot>),
    TransmitFailed(Ieee802154TxError),
    EnergyDetected(i8),
    ClearChannelAssessed { busy: bool },
    MeasurementFailed,
}

/// Notifications one engine entry can raise.
const NOTIFICATIONS: usize = 8;

/// The engine environment of one entry: notifications are collected and
/// translated after the engine returns.
struct Collector {
    platform: Ieee802154Platform,
    notifications: [Option<Notification>; NOTIFICATIONS],
}

impl Collector {
    const fn new(platform: Ieee802154Platform) -> Self {
        Self {
            platform,
            notifications: [None; NOTIFICATIONS],
        }
    }

    fn push(&mut self, notification: Notification) {
        let free = self
            .notifications
            .iter_mut()
            .find(|entry| entry.is_none())
            .expect("one engine entry raises at most eight notifications");
        *free = Some(notification);
    }
}

impl Ieee802154Environment for Collector {
    fn now_micros(&mut self) -> u64 {
        (self.platform.now_micros)()
    }

    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        _frame: &[u8; FRAME_SIZE],
        _info: &Ieee802154FrameInfo,
    ) {
        self.push(Notification::Received(slot));
    }

    fn receive_sfd_done(&mut self) {}

    fn transmit_done(&mut self, _frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>) {
        self.push(Notification::Transmitted(ack.map(|ack| ack.slot)));
    }

    fn transmit_failed(&mut self, _frame: &[u8; FRAME_SIZE], error: Ieee802154TxError) {
        self.push(Notification::TransmitFailed(error));
    }

    fn transmit_sfd_done(&mut self, _frame: &[u8; FRAME_SIZE]) {}

    fn energy_detect_done(&mut self, power: i8) {
        self.push(Notification::EnergyDetected(power));
    }

    fn cca_done(&mut self, busy: bool) {
        self.push(Notification::ClearChannelAssessed { busy });
    }

    fn ed_failed(&mut self, _status: Ieee802154RxStatus) {
        self.push(Notification::MeasurementFailed);
    }

    fn receive_at_done(&mut self) {}

    fn generate_enhanced_ack(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> bool {
        self.platform
            .enhanced_ack
            .is_some_and(|generate| generate(frame, info, ack))
    }
}

const fn tx_status(error: Ieee802154TxError) -> TxStatus {
    match error {
        Ieee802154TxError::CcaBusy => TxStatus::ChannelBusy,
        Ieee802154TxError::Abort => TxStatus::Aborted,
        Ieee802154TxError::NoAck => TxStatus::NoAcknowledgement,
        Ieee802154TxError::InvalidAck => TxStatus::InvalidAcknowledgement,
        Ieee802154TxError::Coexist => TxStatus::CoexistenceRejected,
        Ieee802154TxError::Security => TxStatus::SecurityFailure,
    }
}

fn hal_channel(channel: Channel) -> Ieee802154Channel {
    Ieee802154Channel::new(channel.get()).expect("portable and HAL channels share 11 through 26")
}

/// Lend the MAC bytes of a received image: the PHR length counts two
/// trailing bytes that carry RSSI and LQI in place of the FCS.
///
/// RSSI and LQI are read from those bytes, because a frame a stop flushes
/// never had its frame information updated (vendor `stop_rx`); such a frame
/// also reports channel zero, so `flushed_on` names the receive channel.
fn received_frame<'image>(
    image: &'image [u8; FRAME_SIZE],
    info: &Ieee802154FrameInfo,
    acknowledgement: bool,
    flushed_on: Option<Channel>,
) -> Option<ReceivedFrame<'image>> {
    let length = usize::from(image[0] & 0x7f);
    let frame = FrameView::new(image.get(1..length.checked_sub(1)?)?).ok()?;
    let channel = match Channel::new(info.channel) {
        Ok(channel) => channel,
        Err(_) => flushed_on?,
    };
    let frame_pending = if acknowledgement {
        if frame.bytes()[0] & 0x10 == 0 {
            FramePending::Clear
        } else {
            FramePending::Set
        }
    } else {
        FramePending::Unavailable
    };
    Some(ReceivedFrame {
        frame,
        metadata: RxMetadata {
            channel,
            // The S31 RSSI compensation is zero.
            rssi_dbm: image[length - 1] as i8,
            link_quality: image[length],
            timestamp: Some(RadioTimestamp::from_micros(info.timestamp)),
            fcs: FcsStatus::Valid,
            security: SecurityStatus::Unprocessed,
            frame_pending,
        },
    })
}

/// The radio role: the engine behind the portable state machine.
pub struct Ieee802154Radio<'storage> {
    engine: Ieee802154Engine<'storage>,
    machine: RadioStateMachine,
    platform: Ieee802154Platform,
}

impl<'storage> Ieee802154Radio<'storage> {
    /// Admit portable commands to an engine the composition has enabled and
    /// initialized. The role starts `Disabled` until `Enable` is admitted.
    pub const fn new(engine: Ieee802154Engine<'storage>, platform: Ieee802154Platform) -> Self {
        Self {
            engine,
            machine: RadioStateMachine::new(IEEE802154_RADIO_CAPABILITIES),
            platform,
        }
    }

    /// Return the engine, for the composition's MAC teardown.
    pub fn into_engine(self) -> Ieee802154Engine<'storage> {
        self.engine
    }

    /// The portable state.
    pub const fn state(&self) -> RadioState {
        self.machine.state()
    }

    /// The engine, for vendor configuration the portable contract has no
    /// command for (pending table, transmit security, multi-PAN identity).
    /// Operation entries must go through [`Self::submit`].
    pub fn engine(&mut self) -> &mut Ieee802154Engine<'storage> {
        &mut self.engine
    }

    /// Admit `command`, start its engine operation and deliver the events
    /// the start itself produced.
    ///
    /// # Errors
    ///
    /// The portable state machine rejected the command; nothing ran.
    pub fn submit<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        command: RadioCommand<'_>,
        sink: &mut S,
    ) -> Result<AcceptedCommand, CommandError> {
        let accepted = self.machine.admit(command)?;
        let mut collector = Collector::new(self.platform);
        let engine = &mut self.engine;
        match command {
            RadioCommand::Enable { .. }
            | RadioCommand::Disable { .. }
            | RadioCommand::Sleep { .. } => {
                engine.pib().set_rx_when_idle(false);
                engine.sleep(ll, &mut collector);
            }
            RadioCommand::Receive { channel, .. } => {
                engine.pib().set_channel(hal_channel(channel));
                engine.pib().set_rx_when_idle(true);
                engine.receive(ll, &mut collector);
            }
            RadioCommand::Configure { configuration, .. } => match configuration {
                Configuration::PanId(panid) => engine.set_panid(ll, panid),
                Configuration::ShortAddress(address) => engine.set_short_address(ll, address),
                Configuration::ExtendedAddress(address) => {
                    engine.set_extended_address(ll, address);
                }
                Configuration::Promiscuous(enable) => engine.pib().set_promiscuous(enable),
                Configuration::AutomaticAcknowledgement(enable) => {
                    engine.pib().set_auto_ack_tx(enable);
                }
                Configuration::TransmitPowerDbm(power) => {
                    engine.pib().set_power_table([power; 16]);
                }
            },
            RadioCommand::Transmit(request) => {
                let channel = hal_channel(request.channel);
                engine.pib().set_channel(channel);
                if let Some(power) = request.transmit_power_dbm {
                    engine.pib().set_power_for_channel(channel, power);
                }
                let bytes = request.frame.bytes();
                let mut image = [0; FRAME_SIZE];
                // The PHR counts the two FCS bytes the hardware appends.
                image[0] = (bytes.len() + 2) as u8;
                image[1..=bytes.len()].copy_from_slice(bytes);
                let image = &image[..bytes.len() + 3];
                let started = match request.mode {
                    TxMode::Direct => engine.transmit(ll, &mut collector, image, false),
                    TxMode::ClearChannelAssessment => {
                        engine.transmit(ll, &mut collector, image, true)
                    }
                    TxMode::Scheduled { at } => {
                        engine.transmit_at(ll, &mut collector, image, false, at.as_micros() as u32)
                    }
                    TxMode::CsmaCa { .. } => {
                        unreachable!("CSMA/CA is not advertised, so admission rejects it")
                    }
                };
                started.expect("a validated MAC frame fits one DMA frame");
            }
            RadioCommand::EnergyScan(request) => {
                engine.pib().set_channel(hal_channel(request.channel));
                let symbols = request.duration_us.div_ceil(16).min(u32::from(u16::MAX)) as u16;
                engine.energy_detect(ll, &mut collector, symbols);
            }
            RadioCommand::ClearChannelAssessment { channel, .. } => {
                engine.pib().set_channel(hal_channel(channel));
                engine.cca(ll, &mut collector);
            }
        }
        let flushed_on = match accepted.previous {
            RadioState::Resting(RestingState::Receiving { channel }) => Some(channel),
            _ => None,
        };
        self.deliver(ll, collector, Some(flushed_on), sink);
        Ok(accepted)
    }

    /// The engine interrupt handler; deliver the events it produced.
    pub fn isr<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        sink: &mut S,
    ) {
        let mut collector = Collector::new(self.platform);
        self.engine.isr(ll, &mut collector);
        self.deliver(ll, collector, None, sink);
    }

    /// Translate collected notifications. Frames flushed by an engine call
    /// the role made (`flushed` is `Some`, naming the receive channel if the
    /// radio was receiving) belong to the receive that call ended, so they
    /// are delivered without admission checks.
    fn deliver<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        collector: Collector,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) {
        let mut pending = collector;
        let mut flushed = flushed;
        loop {
            let mut restore = None;
            for notification in pending.notifications.into_iter().flatten() {
                if let Some(channel) = self.translate(notification, flushed, sink) {
                    restore = Some(channel);
                }
            }
            let Some(channel) = restore else {
                return;
            };
            // Resume receive on the resting channel an operation left.
            let mut collector = Collector::new(self.platform);
            let previous = self.engine.pib().channel();
            self.engine.pib().set_channel(channel);
            self.engine.receive(ll, &mut collector);
            pending = collector;
            flushed = Channel::new(previous.number()).ok().map(Some);
        }
    }

    /// Deliver one notification; return the channel to resume receive on
    /// when a terminal event left the engine on another channel.
    fn translate<S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        notification: Notification,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) -> Option<Ieee802154Channel> {
        let id = match self.machine.state() {
            RadioState::Transmitting { id, .. }
            | RadioState::EnergyScanning { id, .. }
            | RadioState::AssessingChannel { id, .. } => Some(id),
            RadioState::Disabled | RadioState::Resting(_) => None,
        };
        match notification {
            Notification::Received(slot) => {
                let (image, info) = self.engine.rx_frame(slot);
                if let Some(frame) = received_frame(&image, &info, false, flushed.flatten()) {
                    let event = RadioEvent::Received(frame);
                    if flushed.is_some() || self.machine.observe(event).is_ok() {
                        sink.event(event);
                    }
                }
                let _ = self.engine.receive_handle_done(slot);
                None
            }
            Notification::Transmitted(ack) => {
                let restore = match id {
                    Some(id) => {
                        let acknowledgement = ack.map(|slot| self.engine.rx_frame(slot));
                        let event = RadioEvent::TransmitDone {
                            id,
                            status: TxStatus::Success,
                            acknowledgement: acknowledgement
                                .as_ref()
                                .and_then(|(image, info)| received_frame(image, info, true, None)),
                        };
                        self.finish(event, sink)
                    }
                    None => {
                        self.fault(sink);
                        None
                    }
                };
                if let Some(slot) = ack {
                    let _ = self.engine.receive_handle_done(slot);
                }
                restore
            }
            Notification::TransmitFailed(error) => {
                let Some(id) = id else {
                    self.fault(sink);
                    return None;
                };
                self.finish(
                    RadioEvent::TransmitDone {
                        id,
                        status: tx_status(error),
                        acknowledgement: None,
                    },
                    sink,
                )
            }
            Notification::EnergyDetected(power) => {
                let Some(id) = id else {
                    self.fault(sink);
                    return None;
                };
                self.finish(
                    RadioEvent::EnergyScanDone {
                        id,
                        energy_dbm: power,
                    },
                    sink,
                )
            }
            Notification::ClearChannelAssessed { busy } => {
                let Some(id) = id else {
                    self.fault(sink);
                    return None;
                };
                self.finish(
                    RadioEvent::ClearChannelAssessmentDone { id, idle: !busy },
                    sink,
                )
            }
            Notification::MeasurementFailed => match self.machine.state() {
                RadioState::EnergyScanning { id, .. } => {
                    self.finish(RadioEvent::EnergyScanFailed { id }, sink)
                }
                RadioState::AssessingChannel { id, .. } => {
                    self.finish(RadioEvent::ClearChannelAssessmentFailed { id }, sink)
                }
                _ => {
                    self.fault(sink);
                    None
                }
            },
        }
    }

    /// Deliver one terminal event; an event the state machine rejects is a
    /// broken event sequence and disables the radio.
    fn finish<S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        event: RadioEvent<'_>,
        sink: &mut S,
    ) -> Option<Ieee802154Channel> {
        let operation_channel = self.engine.pib().channel();
        if self.machine.observe(event).is_err() {
            self.fault(sink);
            return None;
        }
        sink.event(event);
        match self.machine.state() {
            RadioState::Resting(RestingState::Receiving { channel })
                if hal_channel(channel) != operation_channel =>
            {
                Some(hal_channel(channel))
            }
            _ => None,
        }
    }

    fn fault<S: Ieee802154RadioSink + ?Sized>(&mut self, sink: &mut S) {
        let id: Option<RequestId> = match self.machine.state() {
            RadioState::Transmitting { id, .. }
            | RadioState::EnergyScanning { id, .. }
            | RadioState::AssessingChannel { id, .. } => Some(id),
            RadioState::Disabled | RadioState::Resting(_) => None,
        };
        let event = RadioEvent::Fault {
            id,
            fault: RadioFault::InvalidEventSequence,
        };
        if self.machine.observe(event).is_ok() {
            sink.event(event);
        }
    }
}

#[cfg(test)]
mod tests;
