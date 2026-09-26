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
    Ieee802154Channel, Ieee802154MultipanIndex,
    ll::{Ieee802154LowLevel, Ieee802154RxStatus},
};
use oer_esp32s31_ieee802154::engine::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EnhancedAck, Ieee802154Environment,
    Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154TxError,
};
use oer_ieee802154::{
    AcceptedCommand, AppliedSecurity, AttemptFailure, Channel, CommandError, Configuration, CsmaCa,
    FcsStatus, FramePending, FrameRetries, FrameVersion, FrameView, KeyIdMode, MacKeys, PhrFrame,
    RadioCapabilities, RadioCommand, RadioEvent, RadioFault, RadioState, RadioStateMachine,
    RadioTimestamp, ReceivedFrame, RequestId, RestingState, RetryStart, RxMetadata, SecurityStatus,
    SentAcknowledgement, TxMode, TxSecurity, TxStatus, generate_enhanced_ack,
};

/// The portable capabilities the role implements.
///
/// Source matching has no portable command yet and is not advertised.
pub const IEEE802154_RADIO_CAPABILITIES: RadioCapabilities = RadioCapabilities::NONE
    .union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
    .union(RadioCapabilities::CSMA_CA)
    .union(RadioCapabilities::TRANSMIT_RETRIES)
    .union(RadioCapabilities::SECURITY_OFFLOAD)
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
/// header IEs, and secures the ACK of a secured frame with the radio's
/// [`MacKeys`] ([`Ieee802154Radio::mac_keys`]): the next frame counter and
/// the key of the frame's key index. Without keys, or when the frame cannot
/// be acknowledged, the generator refuses and the engine delivers the frame
/// without an ACK. The port rebuilds its
/// CSL and link-metrics IEs per frame; here the IEs are the caller's bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154EnhancedAckGenerator {
    header_ies: [u8; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
    header_ies_len: usize,
}

impl Ieee802154EnhancedAckGenerator {
    /// A generator without header IEs.
    pub const fn new() -> Self {
        Self {
            header_ies: [0; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
            header_ies_len: 0,
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

    fn generate(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
        security: &mut RadioSecurity,
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
            Some(header) => {
                let Some(keys) = security.keys.as_mut() else {
                    return Ieee802154EnhancedAck::Refused;
                };
                let frame_counter = keys.frame_counter();
                let Some(key) = keys.secure(&mut generated) else {
                    return Ieee802154EnhancedAck::Refused;
                };
                // The port's `s_ack_frame_counter` and `s_ack_key_id`.
                security.enhanced_ack = Some(AppliedSecurity {
                    frame_counter,
                    key_id: header.key_index,
                });
                Some(key)
            }
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

/// Why a transmission waits before its next attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Delay {
    /// A CSMA-CA backoff (`kStateCsmaBackoff`).
    CsmaBackoff,
    /// The random delay before a retry after a missing acknowledgement
    /// (`kStateDelayBeforeRetx`).
    Retransmission(RetryStart),
}

/// Where a transmission stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// A delay must start: [`Ieee802154Radio::take_delay`].
    DelayDue(Delay),
    /// The delay runs; [`Ieee802154Radio::delay_elapsed`] ends it.
    Delaying(Delay),
    /// An attempt is on the air.
    Attempting,
}

/// The transmit security the upper layer armed for the next transmission.
#[derive(Clone, Copy)]
struct ArmedSecurity {
    key: [u8; 16],
    address: [u8; 8],
}

/// One transmission as OpenThread `SubMac` runs it over ESP-IDF's radio,
/// which makes one attempt per transmit: CSMA-CA backoffs, retries and the
/// delays before them. Every attempt after the first arms the transmission's
/// security again, as ESP-IDF's OpenThread port arms it in each
/// `otPlatRadioTransmit`; the engine clears it when an attempt ends.
struct Transmission {
    /// The `[PHR, MAC..., FCS]` image each attempt transmits.
    image: [u8; FRAME_SIZE],
    mode: TxMode,
    csma: CsmaCa,
    retries: FrameRetries,
    security: Option<ArmedSecurity>,
    /// Whether the engine still holds `security`.
    armed: bool,
    /// Who secures the frame when no security is armed.
    tx_security: TxSecurity,
    /// The security header fields an attempt wrote into the frame.
    applied: Option<AppliedSecurity>,
    phase: Phase,
}

impl Transmission {
    fn image(&self) -> &[u8] {
        &self.image[..=usize::from(self.image[0])]
    }

    const fn max_backoffs(&self) -> u8 {
        match self.mode {
            TxMode::CsmaCa { max_backoffs } => max_backoffs,
            _ => 0,
        }
    }

    /// `SubMac::StartCsmaBackoff`: back off before a CSMA-CA attempt, or
    /// attempt at once.
    fn start_access<L>(
        &mut self,
        engine: &mut Ieee802154Engine<'_>,
        ll: &mut L,
        env: &mut Collector<'_>,
    ) where
        L: Ieee802154LowLevel + ?Sized,
    {
        if matches!(self.mode, TxMode::CsmaCa { .. }) && self.csma.backoff_micros(0).is_some() {
            self.start_delay(Delay::CsmaBackoff, engine, ll, env);
        } else {
            self.attempt(engine, ll, env);
        }
    }

    /// `SubMac::StartTimerForBackoff`: wait receiving on the transmit
    /// channel when the radio receives when idle, otherwise asleep.
    fn start_delay<L>(
        &mut self,
        delay: Delay,
        engine: &mut Ieee802154Engine<'_>,
        ll: &mut L,
        env: &mut Collector<'_>,
    ) where
        L: Ieee802154LowLevel + ?Sized,
    {
        self.phase = Phase::DelayDue(delay);
        self.armed = false;
        if engine.pib().rx_when_idle() {
            engine.receive(ll, env);
        } else {
            engine.sleep(ll, env);
        }
    }

    /// `SubMac::BeginTransmit`: one attempt in the request's channel access,
    /// secured as `otPlatRadioTransmit` secures it.
    fn attempt<L>(&mut self, engine: &mut Ieee802154Engine<'_>, ll: &mut L, env: &mut Collector<'_>)
    where
        L: Ieee802154LowLevel + ?Sized,
    {
        if let Some(security) = self.security {
            if !self.armed {
                engine.set_transmit_security(ll, self.image(), &security.key, &security.address);
            }
        } else if let Some(keys) = env.security.keys.as_mut()
            && self.tx_security != TxSecurity::Processed
            && PhrFrame::new(self.image()).security_enabled()
        {
            // The port's transmit security: a new frame counter and key index
            // unless this retransmits the frame, the current key, and the
            // extended address read for key identifier mode 1.
            let retransmission =
                self.tx_security == TxSecurity::Retransmission || self.retries.retries() > 0;
            let security = keys.transmit_security(retransmission);
            let length = usize::from(self.image[0]) + 1;
            let mode = security.apply(&mut self.image[..length]);
            if mode == Some(KeyIdMode::Index) {
                env.security.address =
                    ll.multipan_extended_address(Ieee802154MultipanIndex::CONTEXT0);
            }
            if let Some(applied) = mode.and_then(|mode| security.applied(mode)) {
                self.applied = Some(applied);
            }
            engine.set_transmit_security(ll, self.image(), &security.key, &env.security.address);
        }
        self.armed = false;
        self.phase = Phase::Attempting;
        let image = self.image();
        let started = match self.mode {
            TxMode::Direct => engine.transmit(ll, env, image, false),
            TxMode::ClearChannelAssessment | TxMode::CsmaCa { .. } => {
                engine.transmit(ll, env, image, true)
            }
            TxMode::Scheduled { at } => {
                engine.transmit_at(ll, env, image, false, at.as_micros() as u32)
            }
        };
        started.expect("a validated MAC frame fits one DMA frame");
    }
}

/// What the role does after a notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Follow {
    Nothing,
    /// Resume receive on the resting channel an operation left.
    Restore(Ieee802154Channel),
    /// Acquire the channel for the transmission's next attempt.
    Access,
    /// Wait before the transmission's next attempt.
    Delay(Delay),
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
    Received(Ieee802154RxSlot, SentAcknowledgement),
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
struct Collector<'role> {
    platform: Ieee802154Platform,
    enhanced_ack: &'role mut Option<Ieee802154EnhancedAckGenerator>,
    security: &'role mut RadioSecurity,
    notifications: [Option<Notification>; NOTIFICATIONS],
}

/// The security state the radio keeps for the stack, as ESP-IDF's
/// OpenThread port keeps it in its statics.
#[derive(Default)]
struct RadioSecurity {
    /// Keys and frame counter of secured transmissions and enhanced ACKs.
    keys: Option<MacKeys>,
    /// The port's `s_security_addr`: the extended address of the last key
    /// identifier mode 1 transmission.
    address: [u8; 8],
    /// The security of the enhanced ACK generated since the last received
    /// frame (`s_with_security_enh_ack`).
    enhanced_ack: Option<AppliedSecurity>,
}

type Notifications = [Option<Notification>; NOTIFICATIONS];

impl<'role> Collector<'role> {
    const fn new(
        platform: Ieee802154Platform,
        enhanced_ack: &'role mut Option<Ieee802154EnhancedAckGenerator>,
        security: &'role mut RadioSecurity,
    ) -> Self {
        Self {
            platform,
            enhanced_ack,
            security,
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
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        // `ot_radio_receive_done`: the secured enhanced ACK belongs to a
        // 2015 frame that requested one; it is forgotten either way.
        let frame = PhrFrame::new(frame);
        let enhanced = frame.ack_required() && frame.version() == FrameVersion::V2015;
        let security = self.security.enhanced_ack.take().filter(|_| enhanced);
        self.push(Notification::Received(
            slot,
            SentAcknowledgement {
                frame_pending: info.pending,
                security,
            },
        ));
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
            Some(generator) => generator.generate(frame, info, ack, self.security),
            None => Ieee802154EnhancedAck::Refused,
        }
    }
}

/// How ESP-IDF's OpenThread port reports a failed attempt to `SubMac`: a
/// busy channel, an abort and a coexistence rejection as a channel-access
/// failure, a missing or invalid acknowledgement as no acknowledgement. A
/// security failure is not an attempt outcome it reports.
const fn attempt_failure(error: Ieee802154TxError) -> Option<AttemptFailure> {
    match error {
        Ieee802154TxError::CcaBusy | Ieee802154TxError::Abort | Ieee802154TxError::Coexist => {
            Some(AttemptFailure::ChannelAccess)
        }
        Ieee802154TxError::NoAck | Ieee802154TxError::InvalidAck => {
            Some(AttemptFailure::NoAcknowledgement)
        }
        Ieee802154TxError::Security => None,
    }
}

impl Transmission {
    /// `SubMac::HandleTransmitDone` after a failed attempt: back off again
    /// while CSMA-CA backoffs remain, otherwise retry while retries remain.
    /// `Err` ends the transmission, with the status a CSMA-CA transmission
    /// reports for a channel it could not acquire.
    fn retry(&mut self, failure: AttemptFailure) -> Result<Follow, Option<TxStatus>> {
        if failure == AttemptFailure::ChannelAccess
            && matches!(self.mode, TxMode::CsmaCa { .. })
            && self.csma.channel_busy()
        {
            return Ok(Follow::Access);
        }
        self.csma = CsmaCa::new(self.max_backoffs());
        match self.retries.retry(failure) {
            Some(RetryStart::Now) => Ok(Follow::Access),
            Some(start) => Ok(Follow::Delay(Delay::Retransmission(start))),
            None if failure == AttemptFailure::ChannelAccess
                && matches!(self.mode, TxMode::CsmaCa { .. }) =>
            {
                Err(Some(TxStatus::ChannelBusy))
            }
            None => Err(None),
        }
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
            sent_acknowledgement: SentAcknowledgement::NONE,
        },
    })
}

/// The radio role: the engine behind the portable state machine.
pub struct Ieee802154Radio<'storage> {
    engine: Ieee802154Engine<'storage>,
    machine: RadioStateMachine,
    platform: Ieee802154Platform,
    enhanced_ack: Option<Ieee802154EnhancedAckGenerator>,
    transmission: Option<Transmission>,
    next_security: Option<ArmedSecurity>,
    security: RadioSecurity,
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
            transmission: None,
            next_security: None,
            security: RadioSecurity {
                keys: None,
                address: [0; 8],
                enhanced_ack: None,
            },
        }
    }

    /// The MAC keys and frame counter the radio secures with, as ESP-IDF's
    /// OpenThread port keeps them (`otPlatRadioSetMacKey`,
    /// `otPlatRadioSetMacFrameCounter`). With keys, every attempt of a
    /// secured frame takes a new frame counter unless it retransmits the
    /// frame, and the current key index and key; the key identifier mode 1
    /// attempt reads the extended address as the nonce source, others reuse
    /// the last one read. A request's [`TxSecurity`] says who secures it:
    /// `Retransmission` keeps the counter and key index the frame carries
    /// (`mIsARetx`), `Processed` sends a frame the stack secured as given
    /// (`mIsSecurityProcessed`). The transmit completion reports the fields
    /// written, and a received frame reports the security of the enhanced
    /// ACK it was sent (`mAckFrameCounter`, `mAckKeyId`). Secured enhanced
    /// ACKs take their counter from the same keys. Security the upper layer
    /// arms with [`Self::set_transmit_security`] takes precedence for the
    /// next transmission; without keys a secured frame goes out as given.
    pub fn mac_keys(&mut self) -> &mut Option<MacKeys> {
        &mut self.security.keys
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
        let mut collector =
            Collector::new(self.platform, &mut self.enhanced_ack, &mut self.security);
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
                let security = self.next_security.take();
                let mode = request.mode;
                let mut transmission = Transmission {
                    image,
                    mode,
                    csma: CsmaCa::new(match mode {
                        TxMode::CsmaCa { max_backoffs } => max_backoffs,
                        _ => 0,
                    }),
                    retries: FrameRetries::new(request.max_frame_retries),
                    security,
                    armed: security.is_some(),
                    tx_security: request.security,
                    applied: None,
                    phase: Phase::Attempting,
                };
                transmission.start_access(engine, ll, &mut collector);
                self.transmission = Some(transmission);
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

    /// `esp_ieee802154_set_transmit_security` for the secured `[PHR, PSDU...]`
    /// image the next transmission sends. The transmission arms it again
    /// before each later attempt.
    pub fn set_transmit_security<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        frame: &[u8],
        key: &[u8; 16],
        address: &[u8; 8],
    ) {
        self.engine.set_transmit_security(ll, frame, key, address);
        self.next_security = Some(ArmedSecurity {
            key: *key,
            address: *address,
        });
    }

    /// The delay a transmission must wait now before its next attempt, in
    /// microseconds: a CSMA-CA backoff or the delay before a retry
    /// (`SubMac::StartTimerForBackoff`). The caller runs the timer and then
    /// calls [`Self::delay_elapsed`]. `None` when no delay is due.
    pub fn take_delay(&mut self) -> Option<u32> {
        let transmission = self.transmission.as_mut()?;
        let Phase::DelayDue(delay) = transmission.phase else {
            return None;
        };
        transmission.phase = Phase::Delaying(delay);
        let random = (self.platform.random)();
        Some(match delay {
            Delay::CsmaBackoff => transmission.csma.backoff_micros(random).unwrap_or(0),
            Delay::Retransmission(start) => start.delay_micros(random),
        })
    }

    /// End the running delay (`SubMac::HandleTimer`): after a CSMA-CA
    /// backoff attempt with one CCA, after a retry delay acquire the channel
    /// again; deliver the events the step produced.
    pub fn delay_elapsed<L: Ieee802154LowLevel + ?Sized, S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        ll: &mut L,
        sink: &mut S,
    ) {
        let receiving = self.backoff_receive_channel();
        let Some(transmission) = self.transmission.as_mut() else {
            return;
        };
        let Phase::Delaying(delay) = transmission.phase else {
            return;
        };
        let mut collector =
            Collector::new(self.platform, &mut self.enhanced_ack, &mut self.security);
        match delay {
            Delay::CsmaBackoff => transmission.attempt(&mut self.engine, ll, &mut collector),
            Delay::Retransmission(_) => {
                transmission.start_access(&mut self.engine, ll, &mut collector);
            }
        }
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
        let mut collector =
            Collector::new(self.platform, &mut self.enhanced_ack, &mut self.security);
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
                    let mut collector =
                        Collector::new(self.platform, &mut self.enhanced_ack, &mut self.security);
                    let previous = self.engine.pib().channel();
                    self.engine.pib().set_channel(channel);
                    self.engine.receive(ll, &mut collector);
                    pending = collector.notifications;
                    flushed = Channel::new(previous.number()).ok().map(Some);
                }
                Follow::Access | Follow::Delay(_) => {
                    let receiving = self.backoff_receive_channel();
                    let Some(transmission) = self.transmission.as_mut() else {
                        return;
                    };
                    let mut collector =
                        Collector::new(self.platform, &mut self.enhanced_ack, &mut self.security);
                    match follow {
                        Follow::Delay(delay) => {
                            transmission.start_delay(delay, &mut self.engine, ll, &mut collector);
                        }
                        _ => transmission.start_access(&mut self.engine, ll, &mut collector),
                    }
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
            Notification::Received(slot, sent) => {
                let (image, info) = self.engine.rx_frame(slot);
                if let Some(mut frame) = received_frame(&image, &info, false, flushed.flatten()) {
                    frame.metadata.sent_acknowledgement = sent;
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
                            security: self.transmission.as_ref().and_then(|t| t.applied),
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
                let mut status = tx_status(error);
                if let Some(transmission) = self.transmission.as_mut()
                    && let Some(failure) = attempt_failure(error)
                {
                    match transmission.retry(failure) {
                        Ok(follow) => return follow,
                        Err(Some(ended)) => status = ended,
                        Err(None) => {}
                    }
                }
                self.finish(
                    RadioEvent::TransmitDone {
                        id,
                        status,
                        acknowledgement: None,
                        security: self.transmission.as_ref().and_then(|t| t.applied),
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
        self.transmission = None;
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
        self.transmission = None;
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
