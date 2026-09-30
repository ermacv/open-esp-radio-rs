//! Sans-IO ESP32-S31 backend of the IEEE 802.11 lower-MAC port.
//!
//! [`LowerMacCore`] holds the state the port needs between calls (interfaces,
//! installed keys, receive Block Ack banks, the one attempt in flight and the
//! lifecycle state) and drives the existing ESP32-S31 register seams through
//! a caller-supplied [`LowerMacHardware`]. It never waits: the runtime layer
//! owns the event queue, the interrupt entry, the publication deadline and
//! the asynchronous PHY retune an `Enable` needs, and implements
//! `Ieee80211LowerMacPort` over this core.
//!
//! # One submission, one publication
//!
//! An admitted MPDU is published through
//! [`OrdinaryTxOwner::start_single_attempt`]: exactly one descriptor
//! publication at the submitted rate with the caller's protection. Its
//! completion, a collision detach or the hardware timeout abort ends the
//! attempt; the owner's retry ladder, rate fallback and BSS protection
//! selection are not applied on this path.
//!
//! # What the ESP32-S31 does not do here
//!
//! A-MPDU attempts, per-attempt power limits, TBTT events, the access-point
//! TSF other than its reset, coexistence priority hints and receive filters
//! other than "nothing" and a BSS member's are refused as unsupported: the
//! existing seams have no transaction for them. A published attempt cannot be
//! withdrawn: the S31 abort path needs the queue's hardware timeout edge, so
//! `Cancel` ends only an attempt the backend still holds.

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    crypto::{
        ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot,
        StaPairwiseCcmpSlot, install_ap_group_ccmp, install_ap_pairwise_ccmp,
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    init::StaLinkRxPolicyHardware,
    portable,
    rx::{
        NormalizedRxFrame,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{LegacyTxQueue, TxHardware, TxPhyRate},
};
use oer_ieee80211_lower_mac::{
    BandSet, BlockAckReport, Channel, Cipher, KeyHandle, KeyInstall, KeyScope, KeySelector,
    LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacCapabilities, LowerMacSetting,
    MacAddress, Protection, RateSupport, ReceiveFilter, RxBlockAckAgreement, RxMeta, SettingError,
    SubmitError, Tsf, TxAttempt, TxCompletion, TxFault, TxId, TxPayload, TxPower, TxResponse,
    TxStatus, VifConfig, VifId, VifRole, WidthSet,
};
use oer_ieee80211_mac::{
    channel::{Band, WifiChannel},
    phy::{HeMcs, HtMcs},
};
use oer_ieee80211_softmac::MacTxPlan;

use crate::{
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan,
        SingleAttemptProtection, TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE, WifiTxEntropy,
        WifiTxPowerProfile, WifiTxTimer,
    },
    tx::{WifiTxProgress, WifiTxWake},
};

/// Interfaces the core configures at once: the station context (MAC
/// interface zero) and the access-point context (interface one).
pub const LOWER_MAC_VIFS: u8 = 2;

const RESOURCES: oer_ieee80211_softmac::MacResourceLimits =
    ESP32S31_MAC_SERVICE_CAPABILITIES.resources;

/// Keys installed at once: the station's pairwise and group slots and the
/// access point's pairwise and group slots the driver exposes.
pub const LOWER_MAC_KEY_SLOTS: usize = (RESOURCES.station_pairwise_ccmp_slots
    + RESOURCES.station_group_ccmp_slots
    + RESOURCES.access_point_pairwise_ccmp_slots
    + RESOURCES.access_point_group_ccmp_slots) as usize;

/// Power-save holds the core keeps at once.
pub const LOWER_MAC_HOLDS: usize = 8;

const CCMP_128_KEY_BYTES: usize = 16;
const RX_BLOCK_ACK_BANKS: usize = RESOURCES.rx_block_ack_entries as usize;
/// First Frame Control byte of a BlockAckReq: control type, subtype eight.
const BLOCK_ACK_REQUEST_FRAME_CONTROL: u8 = 0x84;
/// Frame Control, Duration and Address 1: the bytes the owner reads.
const MIN_FRAME_LENGTH: usize = 10;

/// What the ESP32-S31 lower MAC supports through this core.
///
/// The rates are those `TxPhyRate::try_from(PhyRate)` accepts: DSSS/CCK and
/// OFDM, HT MCS 0-7 at 20 and 40 MHz, and HE SU MCS 0-9 at 20 MHz with BCC
/// or LDPC and DCM; the DCM MCS limits are checked at submission. The
/// services are exactly the hardware-owned operations of
/// [`ESP32S31_MAC_SERVICE_CAPABILITIES`]. `tx_queues` counts the EDCA
/// parameter sets an attempt contends with; one attempt is in flight at a
/// time. `max_ampdu_subframes` is zero: A-MPDU attempts are not supported.
pub const ESP32S31_LOWER_MAC_CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
    bands: BandSet::GHZ2_4,
    widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
    rates: RateSupport {
        dsss_cck: true,
        ofdm: true,
        ht_max_mcs: HtMcs::new(7),
        he_max_mcs: HeMcs::new(9),
        he_max_bandwidth_mhz: 20,
        he_dcm: true,
        he_ldpc: true,
        spatial_streams: 1,
    },
    services: ESP32S31_MAC_SERVICE_CAPABILITIES
        .operations
        .hardware_services(),
    vifs: LOWER_MAC_VIFS,
    tx_queues: RESOURCES.ordinary_tx_queues,
    max_ampdu_subframes: 0,
    key_slots: LOWER_MAC_KEY_SLOTS as u8,
    rx_block_ack_agreements: RESOURCES.rx_block_ack_entries,
    rx_block_ack_max_tid: RESOURCES.rx_block_ack_max_tid,
    rx_block_ack_max_window: RESOURCES.rx_block_ack_max_window,
};

/// The station TSF of the MAC's interface-zero timer.
pub trait StationTsfHardware {
    fn station_tsf(&mut self) -> u64;
    fn set_station_tsf(&mut self, value: u64);
}

/// Every register seam the core drives.
pub trait LowerMacHardware:
    TxHardware
    + CcmpKeyHardware
    + RxBlockAckHardware
    + StaApRegisterHardware
    + StaLinkRxPolicyHardware
    + ApRxPolicyHardware
    + ApTsfHardware
    + StationTsfHardware
{
}

impl<H> LowerMacHardware for H where
    H: TxHardware
        + CcmpKeyHardware
        + RxBlockAckHardware
        + StaApRegisterHardware
        + StaLinkRxPolicyHardware
        + ApRxPolicyHardware
        + ApTsfHardware
        + StationTsfHardware
{
}

/// Receiver of the events one core call produces.
pub trait LowerMacSink {
    fn tx_completed(&mut self, completion: TxCompletion);
    fn lifecycle(&mut self, event: LifecycleEvent);
}

/// Why the backend's state is unknown: the port is poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacFault {
    /// The ordinary TX owner failed or quarantined its descriptor.
    Tx(OrdinaryTxError),
    /// A receive Block Ack bank did not read back as programmed.
    RxBlockAckReadback,
    /// The channel retune of an `Enable` failed.
    Retune,
}

/// What a lifecycle command needs from the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleStart {
    /// The command is admitted; its terminal event follows through the sink.
    Admitted,
    /// Retune to the channel, then call [`LowerMacCore::finish_retune`].
    Retune(WifiChannel),
}

/// Configuration of one core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LowerMacConfig {
    /// The station address the cold start published; the station seams have
    /// no transaction that changes it.
    pub station_address: MacAddress,
    /// The channel the radio is tuned to.
    pub channel: WifiChannel,
    /// Executor watchdog of one publication.
    pub publication_timeout_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortState {
    Disabled,
    Enabling,
    Enabled,
    Quiescing,
    Quiesced,
    Disabling,
}

enum KeyToken {
    StaPairwise(StaPairwiseCcmpSlot),
    StaGroup(StaGroupCcmpSlot),
    ApPairwise(ApPairwiseCcmpSlot),
    ApGroup(ApGroupCcmpSlot),
}

impl KeyToken {
    const fn hardware_index(&self) -> u8 {
        match self {
            Self::StaPairwise(slot) => slot.hardware_index(),
            Self::StaGroup(slot) => slot.hardware_index(),
            Self::ApPairwise(slot) => slot.hardware_index(),
            Self::ApGroup(slot) => slot.hardware_index(),
        }
    }

    fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        match self {
            Self::StaPairwise(slot) => slot.clear(hardware),
            Self::StaGroup(slot) => slot.clear(hardware),
            Self::ApPairwise(slot) => slot.clear(hardware),
            Self::ApGroup(slot) => slot.clear(hardware),
        }
    }
}

struct InstalledKey {
    vif: VifId,
    token: KeyToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckEntry {
    vif: VifId,
    peer: MacAddress,
    tid: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Hold {
    vif: VifId,
    peer: Option<MacAddress>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttemptPhase {
    /// Copied into the descriptor buffer, not yet published: a power-save
    /// hold covers its receiver.
    Held {
        plan: OrdinaryTxPlan,
        protection: SingleAttemptProtection,
    },
    Published,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Attempt {
    id: TxId,
    vif: VifId,
    key: Option<KeyHandle>,
    receiver: MacAddress,
    phase: AttemptPhase,
}

/// The ESP32-S31 lower-MAC state between port calls.
pub struct LowerMacCore<'slot, P, E, T, const BUFFER_SIZE: usize> {
    tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: LowerMacConfig,
    state: PortState,
    /// Report `Quiesced` before `Disabled`: a `Disable` overtook a `Quiesce`.
    quiesce_pending: bool,
    /// The configured channel; the radio is tuned to it unless a retune is
    /// pending.
    channel: WifiChannel,
    tuned: WifiChannel,
    vifs: [Option<VifConfig>; LOWER_MAC_VIFS as usize],
    keys: [Option<InstalledKey>; LOWER_MAC_KEY_SLOTS],
    rx_block_acks: [Option<RxBlockAckEntry>; RX_BLOCK_ACK_BANKS],
    holds: [Option<Hold>; LOWER_MAC_HOLDS],
    attempt: Option<Attempt>,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> LowerMacCore<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// A disabled core over an idle ordinary TX owner.
    pub fn new(tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>, config: LowerMacConfig) -> Self {
        Self {
            tx,
            config,
            state: PortState::Disabled,
            quiesce_pending: false,
            channel: config.channel,
            tuned: config.channel,
            vifs: [None; LOWER_MAC_VIFS as usize],
            keys: [const { None }; LOWER_MAC_KEY_SLOTS],
            rx_block_acks: [None; RX_BLOCK_ACK_BANKS],
            holds: [None; LOWER_MAC_HOLDS],
            attempt: None,
        }
    }

    pub const fn capabilities(&self) -> LowerMacCapabilities {
        ESP32S31_LOWER_MAC_CAPABILITIES
    }

    /// The channel received frames are reported on.
    pub fn channel(&self) -> Channel {
        Channel::from_wifi_channel(self.channel)
    }

    /// The radio clock: the ordinary TX owner's timer.
    pub fn now_micros(&self) -> u64 {
        self.tx.now_micros()
    }

    /// The deadline of the published attempt or its abort settle, which the
    /// runtime turns into [`WifiTxWake::Deadline`].
    pub fn next_deadline_micros(&self) -> Option<u64> {
        match self.attempt {
            Some(Attempt {
                phase: AttemptPhase::Published,
                ..
            }) => self.tx.next_deadline_micros(),
            _ => None,
        }
    }

    /// Borrow the ordinary TX owner, for diagnostics.
    pub const fn tx(&self) -> &OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE> {
        &self.tx
    }

    const fn receiving(&self) -> bool {
        matches!(
            self.state,
            PortState::Enabled | PortState::Quiescing | PortState::Quiesced
        )
    }

    fn vif(&self, vif: VifId) -> Option<&VifConfig> {
        self.vifs.get(usize::from(vif.0)).and_then(Option::as_ref)
    }

    /// Admit one attempt.
    pub fn submit<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        attempt: TxAttempt<'_>,
    ) -> Result<Result<(), SubmitError>, LowerMacFault> {
        if self.state != PortState::Enabled {
            return Ok(Err(SubmitError::Disabled));
        }
        if let Some(active) = self.attempt {
            return Ok(Err(if active.id == attempt.id {
                SubmitError::DuplicateId
            } else {
                SubmitError::Busy
            }));
        }
        let Some(vif) = self.vif(attempt.vif).copied() else {
            return Ok(Err(SubmitError::UnknownVif));
        };
        let (frame, response) = match attempt.payload {
            TxPayload::Mpdu { frame, response } => (frame, response),
            TxPayload::Ampdu(_) => return Ok(Err(SubmitError::Unsupported)),
        };
        let Ok(rate) = TxPhyRate::try_from(attempt.rate) else {
            return Ok(Err(SubmitError::UnsupportedRate));
        };
        if !ESP32S31_LOWER_MAC_CAPABILITIES.supports_rate(attempt.rate, self.channel()) {
            return Ok(Err(SubmitError::UnsupportedRate));
        }
        if !matches!(attempt.power, TxPower::Calibrated) {
            return Ok(Err(SubmitError::Unsupported));
        }
        let (key, hardware_key_selector, hardware_mic_length) = match attempt.key {
            KeySelector::Plaintext => (None, 0, 0),
            KeySelector::Key(handle) => match self.key(handle) {
                Some(installed) if installed.vif == attempt.vif => (
                    Some(handle),
                    installed.token.hardware_index(),
                    TX_CCMP_MIC_SIZE,
                ),
                _ => return Ok(Err(SubmitError::UnknownKey)),
            },
        };
        if frame.len() < MIN_FRAME_LENGTH
            || TX_METADATA_SIZE + frame.len() + hardware_mic_length + TX_FCS_SIZE > BUFFER_SIZE
        {
            return Ok(Err(SubmitError::InvalidLength));
        }
        let receiver: MacAddress = frame[4..10].try_into().expect("checked length");
        // The owner derives the solicited response from the frame; a request
        // it cannot express, such as a No-Ack individually addressed frame,
        // is refused rather than sent with another response.
        let solicited = if receiver[0] & 1 != 0 {
            TxResponse::None
        } else if frame[0] == BLOCK_ACK_REQUEST_FRAME_CONTROL {
            TxResponse::BlockAck
        } else {
            TxResponse::Ack
        };
        if solicited != response {
            return Ok(Err(SubmitError::Unsupported));
        }
        if response == TxResponse::BlockAck && !matches!(rate, TxPhyRate::Legacy(_)) {
            return Ok(Err(SubmitError::UnsupportedRate));
        }

        let queue = LegacyTxQueue::from_access_category(attempt.access_category);
        let plan = OrdinaryTxPlan {
            frame_length: frame.len(),
            descriptor_capacity: None,
            exchange: MacTxPlan {
                access_category: attempt.access_category,
                initial_rate: rate,
                publication_limit: 1,
                publication_timeout_micros: self.config.publication_timeout_micros,
            },
            hardware_mic_length,
            hardware_key_selector,
            interface: match vif.role {
                VifRole::Station => OrdinaryTxInterface::Station,
                VifRole::AccessPoint => OrdinaryTxInterface::AccessPoint,
            },
            scheduler_priority: queue.vendor_data_scheduler_priority(),
            packet_priority: queue.vendor_data_packet_priority(),
            priority_count: 1,
        };
        let protection = match attempt.protection {
            Protection::None => SingleAttemptProtection::None,
            Protection::RtsCts => SingleAttemptProtection::RtsCts,
            Protection::CtsToSelf => SingleAttemptProtection::CtsToSelf,
        };
        let buffer = self.tx.buffer_mut().map_err(LowerMacFault::Tx)?;
        buffer[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()].copy_from_slice(frame);

        let mut admitted = Attempt {
            id: attempt.id,
            vif: attempt.vif,
            key,
            receiver,
            phase: AttemptPhase::Held { plan, protection },
        };
        if !self.held(&admitted) {
            match self.tx.start_single_attempt(hardware, plan, protection) {
                Ok(_) => admitted.phase = AttemptPhase::Published,
                Err(OrdinaryTxError::BufferSizeOverflow) => {
                    return Ok(Err(SubmitError::InvalidLength));
                }
                Err(error) => return Err(LowerMacFault::Tx(error)),
            }
        }
        self.attempt = Some(admitted);
        Ok(Ok(()))
    }

    fn held(&self, attempt: &Attempt) -> bool {
        self.holds.iter().flatten().any(|hold| {
            hold.vif == attempt.vif && hold.peer.is_none_or(|peer| peer == attempt.receiver)
        })
    }

    fn key(&self, handle: KeyHandle) -> Option<&InstalledKey> {
        self.keys
            .get(usize::from(handle.0))
            .and_then(Option::as_ref)
    }

    /// Consume one interrupt or deadline edge of the published attempt.
    pub fn service<H: TxHardware, S: LowerMacSink>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut S,
    ) -> Result<(), LowerMacFault> {
        let Some(attempt) = self.attempt else {
            return Ok(());
        };
        if attempt.phase != AttemptPhase::Published {
            return Ok(());
        }
        match self.tx.service(hardware, wake).map_err(LowerMacFault::Tx)? {
            WifiTxProgress::Pending => Ok(()),
            WifiTxProgress::Complete => {
                let outcome = self
                    .tx
                    .take_last_outcome()
                    .expect("a completed transaction records its outcome");
                self.attempt = None;
                sink.tx_completed(completion(attempt.id, outcome));
                self.settle(sink);
                Ok(())
            }
        }
    }

    /// Finish lifecycle transitions that waited for the attempt.
    fn settle<S: LowerMacSink>(&mut self, sink: &mut S) {
        if self.attempt.is_some() {
            return;
        }
        match self.state {
            PortState::Quiescing => {
                self.state = PortState::Quiesced;
                sink.lifecycle(LifecycleEvent::Quiesced);
            }
            PortState::Disabling => {
                if core::mem::take(&mut self.quiesce_pending) {
                    sink.lifecycle(LifecycleEvent::Quiesced);
                }
                self.state = PortState::Disabled;
                sink.lifecycle(LifecycleEvent::Disabled);
            }
            PortState::Disabled
            | PortState::Enabling
            | PortState::Enabled
            | PortState::Quiesced => {}
        }
    }

    /// The owned view of one received MPDU, or `None` while the port does
    /// not receive.
    pub fn received<'frame>(
        &self,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], RxMeta)> {
        self.receiving().then(|| {
            (
                frame.mpdu,
                portable::rx_meta(frame.metadata, self.channel()),
            )
        })
    }

    /// Start one lifecycle command.
    pub fn lifecycle<S: LowerMacSink>(
        &mut self,
        command: LifecycleCommand,
        sink: &mut S,
    ) -> Result<LifecycleStart, LifecycleError> {
        match command {
            LifecycleCommand::Enable => match self.state {
                PortState::Disabled if self.channel != self.tuned => {
                    self.state = PortState::Enabling;
                    Ok(LifecycleStart::Retune(self.channel))
                }
                PortState::Disabled | PortState::Quiesced => {
                    self.state = PortState::Enabled;
                    sink.lifecycle(LifecycleEvent::Enabled);
                    Ok(LifecycleStart::Admitted)
                }
                PortState::Enabling
                | PortState::Enabled
                | PortState::Quiescing
                | PortState::Disabling => Err(LifecycleError::AlreadyInState),
            },
            LifecycleCommand::Quiesce => match self.state {
                PortState::Enabled => {
                    self.state = PortState::Quiescing;
                    self.settle(sink);
                    Ok(LifecycleStart::Admitted)
                }
                PortState::Disabled
                | PortState::Enabling
                | PortState::Quiescing
                | PortState::Quiesced
                | PortState::Disabling => Err(LifecycleError::AlreadyInState),
            },
            LifecycleCommand::Disable => match self.state {
                PortState::Enabled | PortState::Quiescing | PortState::Quiesced => {
                    self.quiesce_pending = self.state == PortState::Quiescing;
                    self.state = PortState::Disabling;
                    self.abort_held(sink);
                    self.settle(sink);
                    Ok(LifecycleStart::Admitted)
                }
                PortState::Disabled | PortState::Enabling | PortState::Disabling => {
                    Err(LifecycleError::AlreadyInState)
                }
            },
            LifecycleCommand::Cancel(id) => match self.attempt {
                Some(attempt) if attempt.id == id => {
                    // A published descriptor has no software abort on the
                    // S31: it ends with its own completion.
                    self.abort_held(sink);
                    self.settle(sink);
                    Ok(LifecycleStart::Admitted)
                }
                _ => Err(LifecycleError::UnknownAttempt),
            },
        }
    }

    /// End a held attempt with [`TxStatus::Aborted`]: it never reached the
    /// hardware.
    fn abort_held<S: LowerMacSink>(&mut self, sink: &mut S) {
        if let Some(Attempt {
            id,
            phase: AttemptPhase::Held { .. },
            ..
        }) = self.attempt
        {
            self.attempt = None;
            sink.tx_completed(TxCompletion {
                id,
                status: TxStatus::Aborted,
                ack_rssi_dbm: None,
                ack_snr_db: None,
                block_ack: None,
            });
        }
    }

    /// Complete an `Enable` after the runtime retuned the radio.
    pub fn finish_retune<S: LowerMacSink>(
        &mut self,
        retuned: bool,
        sink: &mut S,
    ) -> Result<(), LowerMacFault> {
        if self.state != PortState::Enabling {
            return Ok(());
        }
        if !retuned {
            self.state = PortState::Disabled;
            return Err(LowerMacFault::Retune);
        }
        self.tuned = self.channel;
        self.state = PortState::Enabled;
        sink.lifecycle(LifecycleEvent::Enabled);
        Ok(())
    }

    /// Apply one setting.
    pub fn apply<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        setting: LowerMacSetting,
    ) -> Result<Result<(), SettingError>, LowerMacFault> {
        match setting {
            LowerMacSetting::Channel(channel) => Ok(self.set_channel(channel)),
            LowerMacSetting::Vif { vif, config } => Ok(self.configure_vif(hardware, vif, config)),
            LowerMacSetting::RemoveKey(handle) => Ok(self.remove_key(hardware, handle)),
            LowerMacSetting::AddRxBlockAck(agreement) => self.add_rx_block_ack(hardware, agreement),
            LowerMacSetting::RemoveRxBlockAck { vif, peer, tid } => {
                self.remove_rx_block_ack(hardware, vif, peer, tid)
            }
            LowerMacSetting::SetTsf { vif, tsf } => Ok(self.set_tsf(hardware, vif, tsf)),
            LowerMacSetting::PowerSaveTxBlock { vif, peer, blocked } => {
                self.power_save_hold(hardware, vif, peer, blocked)
            }
            // No transaction of the S31 seams reports TBTTs as events, and
            // Wi-Fi has no reviewed mapping from a coexistence level to its
            // arbitration events.
            LowerMacSetting::Tbtt { .. } | LowerMacSetting::CoexPriority(_) => {
                Ok(Err(SettingError::Unsupported))
            }
        }
    }

    fn set_channel(&mut self, channel: Channel) -> Result<(), SettingError> {
        if channel.band() != Band::Ghz2_4 {
            return Err(SettingError::UnsupportedChannel);
        }
        let channel =
            WifiChannel::try_from(channel).map_err(|_| SettingError::UnsupportedChannel)?;
        // Retuning needs the MAC's DMA and interrupt service stopped, which
        // only a disabled port guarantees; `Enable` retunes.
        if self.state != PortState::Disabled {
            return Err(SettingError::Busy);
        }
        self.channel = channel;
        Ok(())
    }

    fn configure_vif<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        vif: VifId,
        config: Option<VifConfig>,
    ) -> Result<(), SettingError> {
        let index = usize::from(vif.0);
        if index >= self.vifs.len() {
            return Err(SettingError::UnknownVif);
        }
        if self.attempt.is_some_and(|attempt| attempt.vif == vif) {
            return Err(SettingError::Busy);
        }
        let Some(config) = config else {
            if self.vifs[index].is_none() {
                return Err(SettingError::UnknownVif);
            }
            self.remove_vif(hardware, vif);
            return Ok(());
        };
        let other_role = self.vifs.iter().enumerate().any(|(other, configured)| {
            other != index && configured.is_some_and(|configured| configured.role == config.role)
        });
        if other_role {
            return Err(SettingError::UnsupportedRole);
        }
        let policy = receive_policy(&self.config, &config)?;
        if self.vifs[index].is_some_and(|previous| previous.role != config.role) {
            self.remove_vif(hardware, vif);
        }
        match policy {
            ReceivePolicy::StationDisabled => hardware.disable_station_receive_registers(),
            ReceivePolicy::Station { bssid } => hardware.apply_sta_link_policy(bssid),
            ReceivePolicy::AccessPointDisabled => hardware.disable_ap_link_policy(),
            ReceivePolicy::AccessPoint { address } => hardware.apply_ap_link_policy(address),
        }
        self.vifs[index] = Some(config);
        Ok(())
    }

    /// Close the interface's receive context and clear its keys and
    /// receive Block Ack banks.
    fn remove_vif<H: LowerMacHardware>(&mut self, hardware: &mut H, vif: VifId) {
        let Some(previous) = self.vifs[usize::from(vif.0)].take() else {
            return;
        };
        for slot in &mut self.keys {
            if slot.as_ref().is_some_and(|key| key.vif == vif) {
                slot.take().expect("checked above").token.clear(hardware);
            }
        }
        for (index, bank) in self.rx_block_acks.iter_mut().enumerate() {
            if bank.is_some_and(|entry| entry.vif == vif) {
                *bank = None;
                // Only an index above the bank count fails.
                let _ = hardware.clear_rx_block_ack(index as u8);
            }
        }
        for hold in &mut self.holds {
            if hold.is_some_and(|hold| hold.vif == vif) {
                *hold = None;
            }
        }
        match previous.role {
            VifRole::Station => hardware.disable_station_receive_registers(),
            VifRole::AccessPoint => hardware.disable_ap_link_policy(),
        }
    }

    /// Install a key and return its handle.
    pub fn install_key<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        key: KeyInstall<'_>,
    ) -> Result<KeyHandle, SettingError> {
        let vif = self.vif(key.vif).copied().ok_or(SettingError::UnknownVif)?;
        let temporal_key: &[u8; CCMP_128_KEY_BYTES] = match key.cipher {
            Cipher::Ccmp128 => key.key.try_into().map_err(|_| SettingError::InvalidKey)?,
        };
        let free = self
            .keys
            .iter()
            .position(Option::is_none)
            .ok_or(SettingError::NoKeySlot)?;
        let count = |matches: fn(&KeyToken) -> bool| {
            self.keys
                .iter()
                .flatten()
                .filter(|installed| matches(&installed.token))
                .count()
        };
        let token = match (vif.role, key.scope) {
            (VifRole::Station, KeyScope::Pairwise { peer }) => {
                if count(|token| matches!(token, KeyToken::StaPairwise(_)))
                    >= usize::from(RESOURCES.station_pairwise_ccmp_slots)
                {
                    return Err(SettingError::NoKeySlot);
                }
                install_sta_pairwise_ccmp(hardware, peer, temporal_key).map(KeyToken::StaPairwise)
            }
            (VifRole::Station, KeyScope::Group { key_id }) => {
                if count(|token| matches!(token, KeyToken::StaGroup(_)))
                    >= usize::from(RESOURCES.station_group_ccmp_slots)
                {
                    return Err(SettingError::NoKeySlot);
                }
                install_sta_group_ccmp(hardware, key_id, temporal_key).map(KeyToken::StaGroup)
            }
            (VifRole::AccessPoint, KeyScope::Pairwise { peer }) => {
                // The S31 AP pairwise slot is derived from an association
                // identifier; the backend picks the lowest free one.
                let association_id = (1..=u16::from(RESOURCES.access_point_pairwise_ccmp_slots))
                    .find(|association_id| {
                        !self.keys.iter().flatten().any(|installed| {
                            matches!(&installed.token,
                                KeyToken::ApPairwise(slot) if slot.association_id() == *association_id)
                        })
                    })
                    .ok_or(SettingError::NoKeySlot)?;
                install_ap_pairwise_ccmp(hardware, peer, association_id, temporal_key)
                    .map(KeyToken::ApPairwise)
            }
            (VifRole::AccessPoint, KeyScope::Group { key_id }) => {
                if count(|token| matches!(token, KeyToken::ApGroup(_)))
                    >= usize::from(RESOURCES.access_point_group_ccmp_slots)
                {
                    return Err(SettingError::NoKeySlot);
                }
                install_ap_group_ccmp(hardware, key_id, temporal_key).map(KeyToken::ApGroup)
            }
        }
        .map_err(|error| match error {
            CryptoKeyError::Occupied | CryptoKeyError::InvalidAccessPointAssociationId => {
                SettingError::NoKeySlot
            }
            CryptoKeyError::InvalidGroupKeyId | CryptoKeyError::HardwareRejected => {
                SettingError::InvalidKey
            }
        })?;
        self.keys[free] = Some(InstalledKey {
            vif: key.vif,
            token,
        });
        Ok(KeyHandle(free as u8))
    }

    fn remove_key<H: CcmpKeyHardware>(
        &mut self,
        hardware: &mut H,
        handle: KeyHandle,
    ) -> Result<(), SettingError> {
        if self.key(handle).is_none() {
            return Err(SettingError::UnknownKey);
        }
        if self
            .attempt
            .is_some_and(|attempt| attempt.key == Some(handle))
        {
            return Err(SettingError::Busy);
        }
        self.keys[usize::from(handle.0)]
            .take()
            .expect("checked above")
            .token
            .clear(hardware);
        Ok(())
    }

    fn add_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        agreement: RxBlockAckAgreement,
    ) -> Result<Result<(), SettingError>, LowerMacFault> {
        let Some(vif) = self.vif(agreement.vif).copied() else {
            return Ok(Err(SettingError::UnknownVif));
        };
        if agreement.tid > RESOURCES.rx_block_ack_max_tid
            || agreement.window == 0
            || agreement.window > RESOURCES.rx_block_ack_max_window
            || self
                .rx_block_ack_bank(agreement.vif, agreement.peer, agreement.tid)
                .is_some()
        {
            return Ok(Err(SettingError::InvalidBlockAck));
        }
        let Some(bank) = self.rx_block_acks.iter().position(Option::is_none) else {
            return Ok(Err(SettingError::NoBlockAckSlot));
        };
        match hardware.program_rx_block_ack(S31RxBlockAckAgreement {
            hardware_index: bank as u8,
            interface: mac_interface(vif.role),
            peer: agreement.peer,
            tid: agreement.tid,
            starting_sequence: agreement.start_sequence,
            window: agreement.window,
        }) {
            Ok(()) => {}
            Err(S31RxBlockAckAgreementError::HardwareReadbackMismatch) => {
                return Err(LowerMacFault::RxBlockAckReadback);
            }
            Err(_) => return Ok(Err(SettingError::InvalidBlockAck)),
        }
        self.rx_block_acks[bank] = Some(RxBlockAckEntry {
            vif: agreement.vif,
            peer: agreement.peer,
            tid: agreement.tid,
        });
        Ok(Ok(()))
    }

    fn rx_block_ack_bank(&self, vif: VifId, peer: MacAddress, tid: u8) -> Option<usize> {
        self.rx_block_acks
            .iter()
            .position(|entry| *entry == Some(RxBlockAckEntry { vif, peer, tid }))
    }

    fn remove_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        vif: VifId,
        peer: MacAddress,
        tid: u8,
    ) -> Result<Result<(), SettingError>, LowerMacFault> {
        if self.vif(vif).is_none() {
            return Ok(Err(SettingError::UnknownVif));
        }
        let Some(bank) = self.rx_block_ack_bank(vif, peer, tid) else {
            return Ok(Err(SettingError::InvalidBlockAck));
        };
        if hardware.clear_rx_block_ack(bank as u8).is_err() {
            return Ok(Err(SettingError::InvalidBlockAck));
        }
        self.rx_block_acks[bank] = None;
        Ok(Ok(()))
    }

    fn set_tsf<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        vif: VifId,
        tsf: Tsf,
    ) -> Result<(), SettingError> {
        match self.vif(vif).ok_or(SettingError::UnknownVif)?.role {
            VifRole::Station => hardware.set_station_tsf(tsf.0),
            // The access-point TSF seam only resets and starts the timer.
            VifRole::AccessPoint if tsf == Tsf(0) => hardware.reset_and_start_access_point_tsf(),
            VifRole::AccessPoint => return Err(SettingError::Unsupported),
        }
        Ok(())
    }

    /// Read an interface's TSF: the station timer; the access-point timer
    /// has no read seam.
    pub fn tsf<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
        vif: VifId,
    ) -> Result<Tsf, SettingError> {
        match self.vif(vif).ok_or(SettingError::UnknownVif)?.role {
            VifRole::Station => Ok(Tsf(hardware.station_tsf())),
            VifRole::AccessPoint => Err(SettingError::Unsupported),
        }
    }

    /// Hold or release attempts. A hold keeps an admitted attempt in the
    /// descriptor buffer without publishing it; the release publishes it.
    fn power_save_hold<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        vif: VifId,
        peer: Option<MacAddress>,
        blocked: bool,
    ) -> Result<Result<(), SettingError>, LowerMacFault> {
        if self.vif(vif).is_none() {
            return Ok(Err(SettingError::UnknownVif));
        }
        let hold = Hold { vif, peer };
        if blocked {
            if !self.holds.contains(&Some(hold)) {
                let Some(free) = self.holds.iter().position(Option::is_none) else {
                    return Ok(Err(SettingError::Unsupported));
                };
                self.holds[free] = Some(hold);
            }
            return Ok(Ok(()));
        }
        for entry in &mut self.holds {
            if *entry == Some(hold) {
                *entry = None;
            }
        }
        if let Some(mut attempt) = self.attempt
            && let AttemptPhase::Held { plan, protection } = attempt.phase
            && !self.held(&attempt)
        {
            self.tx
                .start_single_attempt(hardware, plan, protection)
                .map_err(LowerMacFault::Tx)?;
            attempt.phase = AttemptPhase::Published;
            self.attempt = Some(attempt);
        }
        Ok(Ok(()))
    }
}

const fn mac_interface(role: VifRole) -> MacInterface {
    match role {
        VifRole::Station => MacInterface::Station,
        VifRole::AccessPoint => MacInterface::AccessPoint,
    }
}

/// The one receive-policy transaction a configuration maps onto.
enum ReceivePolicy {
    StationDisabled,
    Station { bssid: MacAddress },
    AccessPointDisabled,
    AccessPoint { address: MacAddress },
}

/// Map a receive filter onto the S31 receive-policy seams: nothing, or the
/// complete BSS-member policy of the role (station policy six, access-point
/// policy eight). Other combinations have no register transaction.
fn receive_policy(config: &LowerMacConfig, vif: &VifConfig) -> Result<ReceivePolicy, SettingError> {
    match vif.role {
        VifRole::Station => {
            // The station address is published by the cold start only.
            if vif.address != config.station_address {
                return Err(SettingError::Unsupported);
            }
            match (vif.receive, vif.bssid) {
                (ReceiveFilter::NONE, _) => Ok(ReceivePolicy::StationDisabled),
                (ReceiveFilter::BSS_MEMBER, Some(bssid)) => Ok(ReceivePolicy::Station { bssid }),
                _ => Err(SettingError::Unsupported),
            }
        }
        VifRole::AccessPoint => {
            if vif.address[0] & 1 != 0 || vif.bssid.is_some_and(|bssid| bssid != vif.address) {
                return Err(SettingError::Unsupported);
            }
            match vif.receive {
                ReceiveFilter::NONE => Ok(ReceivePolicy::AccessPointDisabled),
                ReceiveFilter::BSS_MEMBER => Ok(ReceivePolicy::AccessPoint {
                    address: vif.address,
                }),
                _ => Err(SettingError::Unsupported),
            }
        }
    }
}

/// The portable completion of one single-attempt transaction.
fn completion(id: TxId, outcome: OrdinaryTxOutcome) -> TxCompletion {
    let report = outcome.report();
    let status = match outcome {
        OrdinaryTxOutcome::Success(_) => TxStatus::Success,
        OrdinaryTxOutcome::HardwareFailure(report) => report
            .completion
            .map_or(TxStatus::Fault(TxFault::Unrecognized), |completion| {
                completion.tx_status()
            }),
        // The hardware timeout edge and its abort: whether the PPDU reached
        // the air is unknown.
        OrdinaryTxOutcome::HardwareTimeout(_) => TxStatus::Aborted,
        OrdinaryTxOutcome::CollisionLimit(_) => TxStatus::Collision,
    };
    TxCompletion {
        id,
        status,
        // The S31 completion carries an ACK SNR sample but no ACK RSSI.
        ack_rssi_dbm: None,
        ack_snr_db: report.status.ack_snr_db,
        block_ack: report.block_ack.map(|observation| BlockAckReport {
            start_sequence: observation.block_ack.starting_sequence,
            bitmap: observation.block_ack.bitmap,
        }),
    }
}

#[cfg(all(test, not(target_pointer_width = "32")))]
mod tests;
