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
    FRAME_SIZE, Ieee802154Engine, Ieee802154EnhancedAck, Ieee802154Environment,
    Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154TxError,
};
use oer_ieee802154::{
    AcceptedCommand, Channel, CommandError, Configuration, CsmaCa, FcsStatus, FramePending,
    FrameView, MacKeys, RadioCapabilities, RadioCommand, RadioEvent, RadioFault, RadioState,
    RadioStateMachine, RadioTimestamp, ReceivedFrame, RequestId, RestingState, RxMetadata,
    SecurityStatus, TxMode, TxStatus, generate_enhanced_ack,
};

/// The portable capabilities the role implements.
///
/// Security offload and source matching have no portable command yet and
/// are not advertised.
pub const IEEE802154_RADIO_CAPABILITIES: RadioCapabilities = RadioCapabilities::NONE
    .union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
    .union(RadioCapabilities::CSMA_CA)
    .union(RadioCapabilities::ENERGY_SCAN)
    .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SCHEDULED_TRANSMIT)
    .union(RadioCapabilities::TRANSMIT_POWER)
    .union(RadioCapabilities::PROMISCUOUS)
    .union(RadioCapabilities::RECEIVE_TIMESTAMP)
    .union(RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT);

/// Header IE bytes an enhanced ACK carries at most (OpenThread
/// `OT_ACK_IE_MAX_SIZE`).
pub const IEEE802154_ENHANCED_ACK_IE_CAPACITY: usize = 16;

/// The header IEs exceed [`IEEE802154_ENHANCED_ACK_IE_CAPACITY`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EnhancedAckIeTooLong;

/// The enhanced-ACK generator of ESP-IDF's OpenThread port
/// (`ot_radio_enh_ack_generator` of `esp_openthread_radio.c` at ESP-IDF
/// `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`), run inside the interrupt
/// handler for each received 2015 frame that requests an ACK.
///
/// It builds the ACK with [`generate_enhanced_ack`], carrying the configured
/// header IEs, and secures the ACK of a secured frame with [`MacKeys`]:
/// the next frame counter and the key of the frame's key index. Without
/// keys, or when the frame cannot be acknowledged, the generator refuses
/// and the engine delivers the frame without an ACK. The port rebuilds its
/// CSL and link-metrics IEs per frame; here the IEs are the caller's bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154EnhancedAckGenerator {
    header_ies: [u8; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
    header_ies_len: usize,
    keys: Option<MacKeys>,
}

impl Ieee802154EnhancedAckGenerator {
    /// A generator without header IEs or keys: secured frames are refused.
    pub const fn new() -> Self {
        Self {
            header_ies: [0; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
            header_ies_len: 0,
            keys: None,
        }
    }

    /// The header IEs every ACK carries.
    pub fn header_ies(&self) -> &[u8] {
        &self.header_ies[..self.header_ies_len]
    }

    /// Replace the header IEs.
    ///
    /// # Errors
    ///
    /// The IEs exceed [`IEEE802154_ENHANCED_ACK_IE_CAPACITY`]; nothing
    /// changes.
    pub fn set_header_ies(&mut self, ies: &[u8]) -> Result<(), Ieee802154EnhancedAckIeTooLong> {
        let target = self
            .header_ies
            .get_mut(..ies.len())
            .ok_or(Ieee802154EnhancedAckIeTooLong)?;
        target.copy_from_slice(ies);
        self.header_ies_len = ies.len();
        Ok(())
    }

    /// The MAC keys and frame counter, when secured ACKs are admitted.
    pub fn keys(&mut self) -> &mut Option<MacKeys> {
        &mut self.keys
    }

    fn generate(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> Ieee802154EnhancedAck {
        // The PHR counts the RSSI and LQI bytes that replace the FCS.
        let length = usize::from(frame[0] & 0x7f);
        let Some(received) = length
            .checked_sub(1)
            .and_then(|end| frame.get(1..end))
            .and_then(|bytes| FrameView::new(bytes).ok())
        else {
            return Ieee802154EnhancedAck::Refused;
        };
        let header_ies = &self.header_ies[..self.header_ies_len];
        let Ok(mut generated) = generate_enhanced_ack(received, info.pending, header_ies) else {
            return Ieee802154EnhancedAck::Refused;
        };
        let key = match generated.security() {
            Some(_) => match self
                .keys
                .as_mut()
                .and_then(|keys| keys.secure(&mut generated))
            {
                Some(key) => Some(key),
                None => return Ieee802154EnhancedAck::Refused,
            },
            None => None,
        };
        let bytes = generated.bytes();
        // The PHR counts the FCS the hardware appends.
        ack[0] = (bytes.len() + 2) as u8;
        ack[1..=bytes.len()].copy_from_slice(bytes);
        key.map_or(Ieee802154EnhancedAck::Generated, |key| {
            Ieee802154EnhancedAck::Secured { key }
        })
    }
}

/// Platform services the engine calls during an entry.
#[derive(Clone, Copy)]
pub struct Ieee802154Platform {
    /// The monotonic microsecond clock (`esp_timer_get_time`); the engine
    /// truncates it to the vendor's wrapping 32-bit timer domain, and receive
    /// timestamps use it as the radio epoch.
    pub now_micros: fn() -> u64,
    /// A uniform random word for each CSMA-CA backoff, as OpenThread draws
    /// one from its non-cryptographic generator.
    pub random: fn() -> u32,
}

/// Where a CSMA-CA transmission stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CsmaPhase {
    /// A backoff must start: [`Ieee802154Radio::take_csma_backoff`].
    BackoffDue,
    /// The backoff runs; [`Ieee802154Radio::csma_attempt`] ends it.
    BackingOff,
    /// A CCA attempt is on the air.
    Attempting,
}

/// A CSMA-CA transmission, as OpenThread `SubMac` runs one over a radio
/// that performs a single CCA per transmission.
struct CsmaTransmission {
    /// The `[PHR, MAC..., FCS]` image each attempt transmits.
    image: [u8; FRAME_SIZE],
    csma: CsmaCa,
    phase: CsmaPhase,
}

impl CsmaTransmission {
    fn image(&self) -> &[u8] {
        &self.image[..=usize::from(self.image[0])]
    }
}

/// What the role does after a notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Follow {
    Nothing,
    /// Resume receive on the resting channel an operation left.
    Restore(Ieee802154Channel),
    /// A busy channel backs a CSMA-CA transmission off again.
    Backoff,
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
struct Collector<'generator> {
    platform: Ieee802154Platform,
    enhanced_ack: &'generator mut Option<Ieee802154EnhancedAckGenerator>,
    notifications: [Option<Notification>; NOTIFICATIONS],
}

type Notifications = [Option<Notification>; NOTIFICATIONS];

impl<'generator> Collector<'generator> {
    const fn new(
        platform: Ieee802154Platform,
        enhanced_ack: &'generator mut Option<Ieee802154EnhancedAckGenerator>,
    ) -> Self {
        Self {
            platform,
            enhanced_ack,
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

impl Ieee802154Environment for Collector<'_> {
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
    ) -> Ieee802154EnhancedAck {
        match self.enhanced_ack {
            Some(generator) => generator.generate(frame, info, ack),
            None => Ieee802154EnhancedAck::Refused,
        }
    }
}

/// Receive on the transmission's channel during a CSMA-CA backoff when the
/// radio receives when idle, otherwise sleep (`SubMac::StartTimerForBackoff`).
fn idle_for_backoff<L, E>(engine: &mut Ieee802154Engine<'_>, ll: &mut L, env: &mut E)
where
    L: Ieee802154LowLevel + ?Sized,
    E: Ieee802154Environment + ?Sized,
{
    if engine.pib().rx_when_idle() {
        engine.receive(ll, env);
    } else {
        engine.sleep(ll, env);
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
    enhanced_ack: Option<Ieee802154EnhancedAckGenerator>,
    csma: Option<CsmaTransmission>,
}

impl<'storage> Ieee802154Radio<'storage> {
    /// Admit portable commands to an engine the composition has enabled and
    /// initialized. The role starts `Disabled` until `Enable` is admitted.
    pub const fn new(engine: Ieee802154Engine<'storage>, platform: Ieee802154Platform) -> Self {
        Self {
            engine,
            machine: RadioStateMachine::new(IEEE802154_RADIO_CAPABILITIES),
            platform,
            enhanced_ack: None,
            csma: None,
        }
    }

    /// The enhanced-ACK generator; `None` refuses every enhanced ACK.
    pub fn enhanced_ack(&mut self) -> &mut Option<Ieee802154EnhancedAckGenerator> {
        &mut self.enhanced_ack
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
        let mut collector = Collector::new(self.platform, &mut self.enhanced_ack);
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
                let stored = image;
                let image = &image[..bytes.len() + 3];
                let started = match request.mode {
                    TxMode::Direct => engine.transmit(ll, &mut collector, image, false),
                    TxMode::ClearChannelAssessment => {
                        engine.transmit(ll, &mut collector, image, true)
                    }
                    TxMode::Scheduled { at } => {
                        engine.transmit_at(ll, &mut collector, image, false, at.as_micros() as u32)
                    }
                    // `SubMac::StartCsmaBackoff`: back off first, or transmit
                    // with one CCA when no backoff is allowed.
                    TxMode::CsmaCa { max_backoffs } => {
                        let csma = CsmaCa::new(max_backoffs);
                        let backs_off = csma.backoff_micros(0).is_some();
                        self.csma = Some(CsmaTransmission {
                            image: stored,
                            csma,
                            phase: if backs_off {
                                CsmaPhase::BackoffDue
                            } else {
                                CsmaPhase::Attempting
                            },
                        });
                        if backs_off {
                            idle_for_backoff(engine, ll, &mut collector);
                            Ok(())
                        } else {
                            engine.transmit(ll, &mut collector, image, true)
                        }
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
        let notifications = collector.notifications;
        self.deliver(ll, notifications, Some(flushed_on), sink);
        Ok(accepted)
    }

    /// The backoff a CSMA-CA transmission must wait now, in microseconds
    /// (`SubMac::StartTimerForBackoff`); the caller runs the timer and then
    /// calls [`Self::csma_attempt`]. `None` when no backoff is due.
    pub fn take_csma_backoff(&mut self) -> Option<u32> {
        let transmission = self.csma.as_mut()?;
        if transmission.phase != CsmaPhase::BackoffDue {
            return None;
        }
        transmission.phase = CsmaPhase::BackingOff;
        transmission.csma.backoff_micros((self.platform.random)())
    }

    /// End the running CSMA-CA backoff: transmit with one CCA
    /// (`SubMac::BeginTransmit`), and deliver the events the start produced.
    pub fn csma_attempt<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        sink: &mut S,
    ) {
        let Some(transmission) = self
            .csma
            .as_mut()
            .filter(|transmission| transmission.phase == CsmaPhase::BackingOff)
        else {
            return;
        };
        transmission.phase = CsmaPhase::Attempting;
        let receiving = self.backoff_receive_channel();
        let mut collector = Collector::new(self.platform, &mut self.enhanced_ack);
        let transmission = self.csma.as_ref().expect("the attempt's transmission");
        self.engine
            .transmit(ll, &mut collector, transmission.image(), true)
            .expect("a validated MAC frame fits one DMA frame");
        let notifications = collector.notifications;
        self.deliver(ll, notifications, Some(receiving), sink);
    }

    /// The channel a backoff receives on, when the radio receives when idle.
    fn backoff_receive_channel(&mut self) -> Option<Channel> {
        let pib = self.engine.pib();
        if pib.rx_when_idle() {
            Channel::new(pib.channel().number()).ok()
        } else {
            None
        }
    }

    /// The engine interrupt handler; deliver the events it produced.
    pub fn isr<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        sink: &mut S,
    ) {
        let mut collector = Collector::new(self.platform, &mut self.enhanced_ack);
        self.engine.isr(ll, &mut collector);
        let notifications = collector.notifications;
        self.deliver(ll, notifications, None, sink);
    }

    /// Translate collected notifications. Frames flushed by an engine call
    /// the role made (`flushed` is `Some`, naming the receive channel if the
    /// radio was receiving) belong to the receive that call ended, so they
    /// are delivered without admission checks.
    fn deliver<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        notifications: Notifications,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) {
        let mut pending = notifications;
        let mut flushed = flushed;
        loop {
            let mut follow = Follow::Nothing;
            for notification in pending.into_iter().flatten() {
                match self.translate(notification, flushed, sink) {
                    Follow::Nothing => {}
                    next => follow = next,
                }
            }
            match follow {
                Follow::Nothing => return,
                Follow::Restore(channel) => {
                    // Resume receive on the resting channel an operation left.
                    let mut collector = Collector::new(self.platform, &mut self.enhanced_ack);
                    let previous = self.engine.pib().channel();
                    self.engine.pib().set_channel(channel);
                    self.engine.receive(ll, &mut collector);
                    pending = collector.notifications;
                    flushed = Channel::new(previous.number()).ok().map(Some);
                }
                Follow::Backoff => {
                    let receiving = self.backoff_receive_channel();
                    let mut collector = Collector::new(self.platform, &mut self.enhanced_ack);
                    idle_for_backoff(&mut self.engine, ll, &mut collector);
                    pending = collector.notifications;
                    flushed = Some(receiving);
                }
            }
        }
    }

    /// Deliver one notification, and say what follows it: resuming receive
    /// on the resting channel a terminal event left, or another CSMA-CA
    /// backoff.
    fn translate<S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        notification: Notification,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) -> Follow {
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
                Follow::Nothing
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
                        Follow::Nothing
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
                    return Follow::Nothing;
                };
                // ESP-IDF's OpenThread port reports a busy channel, an abort
                // and a coexistence rejection as a channel-access failure,
                // which `SubMac` backs off from while backoffs remain.
                let channel_access = matches!(
                    error,
                    Ieee802154TxError::CcaBusy
                        | Ieee802154TxError::Abort
                        | Ieee802154TxError::Coexist
                );
                let status = match self.csma.as_mut() {
                    Some(transmission) if channel_access => {
                        if transmission.csma.channel_busy() {
                            transmission.phase = CsmaPhase::BackoffDue;
                            return Follow::Backoff;
                        }
                        TxStatus::ChannelBusy
                    }
                    _ => tx_status(error),
                };
                self.finish(
                    RadioEvent::TransmitDone {
                        id,
                        status,
                        acknowledgement: None,
                    },
                    sink,
                )
            }
            Notification::EnergyDetected(power) => {
                let Some(id) = id else {
                    self.fault(sink);
                    return Follow::Nothing;
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
                    return Follow::Nothing;
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
                    Follow::Nothing
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
    ) -> Follow {
        self.csma = None;
        let operation_channel = self.engine.pib().channel();
        if self.machine.observe(event).is_err() {
            self.fault(sink);
            return Follow::Nothing;
        }
        sink.event(event);
        match self.machine.state() {
            RadioState::Resting(RestingState::Receiving { channel })
                if hal_channel(channel) != operation_channel =>
            {
                Follow::Restore(hal_channel(channel))
            }
            _ => Follow::Nothing,
        }
    }

    fn fault<S: Ieee802154RadioSink + ?Sized>(&mut self, sink: &mut S) {
        self.csma = None;
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
