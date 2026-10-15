//! The hardware side of the ESP32-S31 lower-MAC backend.
//!
//! [`LowerMacCore`] is the state the backend's driver owns by value: the interfaces, the installed keys, the receive Block Ack
//! banks, and the one published attempt of each EDCA queue. It drives the
//! ESP32-S31 register seams through a caller-supplied [`LowerMacHardware`]
//! and never waits: the runtime owns the event queues, the interrupt
//! entries, the publication deadlines, the software queue of each access
//! category ahead of the core and the asynchronous PHY retune an `Enable`
//! needs, and serves the phase contract
//! (`oer_ieee80211_lower_mac::phase`) over this core.
//!
//! # Checked operations
//!
//! The phase contract's types rule out every refusal a caller controls, so
//! each checked operation takes values its caller's types guarantee
//! ([`MpduValues`], [`CheckedKeyScope`], [`RxBlockAckValues`],
//! [`CoreVif`]) and refuses nothing: its state preconditions are invariants
//! the core asserts in debug builds, and what the hardware refuses is a
//! [`LowerMacFault`].
//!
//! # Transmit buffers
//!
//! A transmit buffer is a whole ordinary TX slot ([`TxSlot`]): the pinned
//! descriptor and source buffer the hardware reads, whose MPDU follows the
//! TX metadata word. The caller holds the slot, encodes the MPDU into it
//! and submits it with the attempt; it comes back with the attempt's
//! completion ([`LowerMacSink::tx_completed`]). The backend numbers a fresh
//! protected MPDU at its first transmission ([`LowerMacCore::number`]): the
//! key's software packet-number allocator writes its CCMP header. Only
//! `Ok(())` confirms that write; a [`CcmpNumberingError`] prevents TX
//! preparation, including when the key only receives.
//!
//! # One publication per attempt, one attempt per EDCA queue
//!
//! An attempt is published through
//! [`OrdinaryTxOwner::start_queued_single_attempt`]: exactly one descriptor
//! publication at the submitted rate with the caller's protection, backoff
//! and power ceiling. Its completion, a collision detach or the hardware
//! timeout abort ends it; the driver carries known-result retries out by
//! submitting the same MPDU again (`MpduValues::retry`).
//!
//! The chip's pure completion decoder returns `Observed(status)` or
//! `NoEffect`, or a `CompletionFault`. Only Observed constructs completed
//! metadata and consumes the published attempt through `finish(status)`.
//! NoEffect retains the same descriptor, deadline and attempt until its real
//! result or hardware timeout. SecurityKeyError and InvalidStatus propagate
//! through `LowerMacFault::Tx` to the existing poisoned-install path; they
//! return no TX terminal or slot. TimedOut ends the attempt with an unknown
//! result, and the backend never republishes it.
//!
//! The S31 MAC has four ordinary EDCA queues (voice, video, best effort,
//! background), each with its own descriptor, completion bank and latched
//! completion, timeout and collision state
//! (`pac/src/wifi/mac/tx/queue.rs`); the vendor publishes them concurrently
//! (`ppTxPkt` maps user priorities onto them) and handles their collisions
//! and timeouts per queue (`lmacProcessCollisions`,
//! `lmacProcessAllTxTimeout`). The core holds one attempt per queue
//! within the core; the driver's software queue holds the
//! next ones.
//!
//! The MAC interrupt's task events do not name a queue, so one
//! [`LowerMacCore::service`] call offers the same edge to every published
//! queue, and each claims only its own queue's state; an edge no queue
//! claims is ignored, and a lost one still ends in the publication
//! deadline's quarantine. A timeout abort forces the MAC-wide CCA for its
//! 16 µs settle, so aborts run one at a time: a queue whose timeout arrives
//! while another settles keeps its latched timeout and is aborted right
//! after that settle ends.
//!
//! # Limits
//!
//! The S31 draws its backoff in software, so only `Backoff::Slots` is
//! accepted. Coexistence maps `CoexPriority::Normal` onto the static
//! per-access-category priority of
//! [`LegacyTxQueue::vendor_data_packet_priority`] and
//! `CoexPriority::Elevated` onto the radio system's connection-frame
//! priorities. An individually addressed frame without acknowledgement is
//! published at legacy rates only, the one program with a response field;
//! hardware has not confirmed its on-air behaviour. A published attempt
//! cannot be withdrawn: the S31 abort path needs the queue's hardware
//! timeout edge. A-MPDU transmission returns in step 2 (#563); access-point
//! TBTT schedules are not implemented, as the qualification catalog
//! records.

use core::pin::Pin;

#[cfg(test)]
use oer_time::Clock;

use oer_esp32s31_hal::types::{MacPti, StaTbttSchedule};
use oer_ieee80211_lower_mac::TsfGeneration;
use oer_ieee80211_mac::tsf::TsfInstant;

use crate::station_tsf::{StationTsf, StationTsfHardware};
use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    crypto::{
        ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpKeyHardware, CcmpTxPacketNumberError,
        CryptoKeyError, StaGroupCcmpSlot, StaPairwiseCcmpSlot, install_ap_group_ccmp,
        install_ap_pairwise_ccmp, install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    init::{
        MacSnifferHardware, StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware,
        StaNoiseFloorHardware,
    },
    portable,
    rx::{
        NormalizedRxFrame,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{
        HeEdcaTxopLimit, HtAmpduDensity, LegacyTxQueue, TxError, TxPhyRate, TxSlot,
        ampdu::{HeAmpduPolicy, HtAmpduHardware},
        runtime::he_txop_limit,
    },
};
use oer_ieee80211_lower_mac::{
    BandSet, BeaconTimingCapabilities, BlockAckReport, Channel, CoexPriority, CoexPrioritySet,
    LinkMetric, LowerMacCapabilities, MacAddress, PhyFormatSet, PhyRate, Protection, RateSupport,
    ReceiveFilter, RxEvidence, RxMeta, RxMetadata, SignalMetadata, TbttEvent, TsfTimingError,
    TxStatus, VifRole, VifRoleSet, VifTsf, WidthSet,
};
use oer_ieee80211_mac::{
    ccmp::CCMP_HEADER_LEN,
    channel::WifiChannel,
    management::BROADCAST_ADDRESS,
    phy::{HeMcs, HtMcs},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_ieee80211_softmac::MacTxPlan;

pub mod ampdu;

pub use ampdu::{
    AmpduBacking, AmpduBackingSource, AmpduFormat, AmpduOutcome, AmpduPlan, AmpduProgress,
    AmpduPublication, ESP32S31_AMPDU_MAX_LENGTH, Esp32s31AmpduBuffer, Esp32s31AmpduOwner, NoAmpdu,
    PublishedAmpdu, esp32s31_ampdu_capabilities,
};

use crate::{
    ampdu_tx::AmpduTxRoleAdapter,
    ordinary_tx::{
        MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS, OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOutcome,
        OrdinaryTxOwner, OrdinaryTxPlan, QueuedSingleAttempt, QueuedSingleAttemptProgress,
        QueuedSingleAttemptRefused, SingleAttempt, SingleAttemptProtection, TX_CCMP_MIC_SIZE,
        TX_FCS_SIZE, TX_METADATA_SIZE, WifiTxEntropy, WifiTxPowerProfile,
    },
    tx::WifiTxWake,
};

/// The ordinary EDCA queues, each holding one attempt in flight.
pub const LOWER_MAC_TX_QUEUES: usize = 4;

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

const CCMP_128_KEY_BYTES: usize = 16;
/// Frame Control, Duration and Address 1: the bytes a subframe's check
/// reads.
const MIN_FRAME_LENGTH: usize = 10;
const RX_BLOCK_ACK_BANKS: usize = RESOURCES.rx_block_ack_entries as usize;

/// The longest PSDU (MPDU, MIC and FCS) every transmit program carries:
/// the legacy program's 12-bit length field
/// (the `signal` check of `MacLegacyTxProgram::new`, which
/// `TxSlot::submit_legacy` turns into `TxError::Invalid`); the HE program
/// takes up to 0x3ff7.
const MAX_PROGRAM_PSDU: usize = 0x0fff;
/// Light-sleep wake lead the vendor power manager publishes beside the
/// station TBTT lead (`g_pm_cfg[16]`, as
/// `roles/esp32s31/ieee80211/sta/src/modem_sleep.rs` records it). The port
/// reports TBTTs only; it keeps the vendor relation between the two leads.
const STATION_TBTT_WAKE_WINDOW_MICROS: u16 = 1_500;

/// The receive rules the station context provides: its link policy (six)
/// for the BSS-member rules, and the ESP-NOW policy (six, mode two) that
/// also admits management frames of other BSSs.
const STATION_RECEIVE_FILTERS: ReceiveFilter =
    ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT);

/// One received MPDU's metadata on the ESP32-S31: the portable fields, and
/// the frame's RSSI from its receive prefix and the PHY's noise floor when
/// it was received, which every frame carries. The S31 therefore measures
/// every frame's signal ([`SignalMetadata`]), and a rate selection that
/// needs it can use this port's metadata directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxMeta {
    common: RxMeta,
    rssi_dbm: i8,
    noise_floor_dbm: i8,
}

impl Esp32s31RxMeta {
    /// The same metadata with the frame's receive stamp, which the runtime
    /// converts from the MAC local time.
    pub const fn with_timestamp(
        mut self,
        timestamp: RxEvidence<oer_ieee80211_lower_mac::Ieee80211Stamp>,
    ) -> Self {
        self.common.timestamp = timestamp;
        self
    }
}

impl RxMetadata for Esp32s31RxMeta {
    fn common(&self) -> &RxMeta {
        &self.common
    }
}

impl SignalMetadata for Esp32s31RxMeta {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
    }
}

/// What the ESP32-S31 lower MAC accepts through this core, with TX slots of
/// `buffer_size` bytes.
///
/// The rates are those `TxPhyRate::try_from(PhyRate)` accepts: DSSS/CCK and
/// OFDM, HT MCS 0-7 at 20 and 40 MHz, and HE SU MCS 0-9 at 20 MHz with BCC
/// or LDPC and DCM; the DCM MCS limits are checked at submission. The
/// services are exactly the hardware-owned operations of
/// [`ESP32S31_MAC_SERVICE_CAPABILITIES`]. The longest MPDU leaves room for
/// the TX metadata word, the CCMP MIC and the FCS in the four-byte-aligned
/// descriptor capacity, and with the MIC and the FCS fits the transmit
/// programs' PSDU length field (the legacy program's 12 bits). Each ordinary
/// EDCA queue holds one attempt. 5 GHz is absent from the ESP32-S31.
// CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-concurrent-queue-attempts
pub const fn esp32s31_lower_mac_capabilities(buffer_size: usize) -> LowerMacCapabilities {
    let buffered =
        (buffer_size & !3).saturating_sub(TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE);
    // The transmit programs' length fields bound a PSDU too.
    let programmable = MAX_PROGRAM_PSDU - TX_CCMP_MIC_SIZE - TX_FCS_SIZE;
    let usable = if buffered < programmable {
        buffered
    } else {
        programmable
    };
    LowerMacCapabilities {
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
        tx_queues: LOWER_MAC_TX_QUEUES as u8,
        max_mpdu_length: if usable > u16::MAX as usize {
            u16::MAX
        } else {
            usable as u16
        },
        max_backoff_slots: MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS,
        tx_power_ceiling_min_dbm: Some(0),
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal)
            .union(CoexPrioritySet::only(CoexPriority::Elevated)),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: STATION_RECEIVE_FILTERS,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::PROBE_REQUESTS),
        key_slots: LOWER_MAC_KEY_SLOTS as u8,
        rx_block_ack_agreements: RESOURCES.rx_block_ack_entries,
        rx_block_ack_max_tid: RESOURCES.rx_block_ack_max_tid,
        rx_block_ack_max_window: RESOURCES.rx_block_ack_max_window,
        // The bounds `EdcaContentionParameters::new` checks.
        edca_max_aifsn: 0x0f,
        edca_max_cw_exponent: oer_esp32s31_ieee80211_mac::edca::MAX_HARDWARE_ECW_EXPONENT,
    }
}

/// The station TSF is read, set and scheduled; the access-point TSF only
/// restarts from zero.
pub const ESP32S31_BEACON_TIMING_CAPABILITIES: BeaconTimingCapabilities =
    BeaconTimingCapabilities {
        tsf_read: VifRoleSet::STATION,
        tsf_set: VifRoleSet::STATION,
        tsf_restart: VifRoleSet::STATION.union(VifRoleSet::ACCESS_POINT),
        tbtt: VifRoleSet::STATION,
    };

/// The station TBTT schedule of the MAC's interface-zero timer, whose event
/// fires on the power interrupt (`MacPowerWakeCause::StaTbtt`).
pub trait StationTbttHardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule);
    fn stop_station_tbtt(&mut self);
}

/// The power-save block of every ordinary TX queue.
pub trait TxGateHardware {
    fn set_power_save_tx_block(&mut self, blocked: bool);
}

/// The beacon receive priority registers (`hal_set_rx_beacon_pti`,
/// `hal_clear_rx_beacon_pti`).
pub trait RxBeaconPriorityHardware {
    /// Receive beacons at `pti`, the beacon and the shared priority alike.
    fn set_rx_beacon_pti(&mut self, pti: MacPti);
    /// Withdraw the beacon receive priority request.
    fn clear_rx_beacon_pti(&mut self);
}

/// The coexistence priorities of the radio system the core shares its RF
/// with, as it last published them.
pub trait RadioCoexPriorities {
    /// The priority of the beacon-window event (`coex_pti_get(0)`), which
    /// beacon reception asks for the air with.
    fn beacon_window_pti(&self) -> MacPti;

    /// The priorities a connection frame carries under the reconnect
    /// policy (`CoexPriority::Elevated`): event 46's as the packet
    /// priority, and the lesser of it and the slice event's as the
    /// scheduler priority.
    ///
    /// SOURCE(esp32s31): complete pinned `libpp[pp.o]::pp_coex_tx_request`
    /// and `libpp[hal_mac.o]::mac_tx_set_pti`.
    fn connection_frame_priorities(&self) -> ConnectionFramePriorities;
}

/// The packet and scheduler priorities of a connection frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionFramePriorities {
    pub packet: u8,
    pub scheduler: u8,
}

/// The priority count the vendor publishes beside a connection frame's
/// priority under the reconnect policy.
const CONNECTION_FRAME_PRIORITY_COUNT: u16 = 4_000;

/// Every register seam the core drives; [`HtAmpduHardware`] includes the
/// ordinary queues' `TxHardware`.
pub trait LowerMacHardware:
    HtAmpduHardware
    + CcmpKeyHardware
    + RxBlockAckHardware
    + StaApRegisterHardware
    + StaLinkRxPolicyHardware
    + StaNoiseFloorHardware
    + StaEspNowRxPolicyHardware
    + MacSnifferHardware
    + ApRxPolicyHardware
    + ApTsfHardware
    + StationTsfHardware
    + StationTbttHardware
    + TxGateHardware
    + RxBeaconPriorityHardware
{
}

impl<H> LowerMacHardware for H where
    H: HtAmpduHardware
        + CcmpKeyHardware
        + RxBlockAckHardware
        + StaApRegisterHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + StaEspNowRxPolicyHardware
        + MacSnifferHardware
        + ApRxPolicyHardware
        + ApTsfHardware
        + StationTsfHardware
        + StationTbttHardware
        + TxGateHardware
        + RxBeaconPriorityHardware
{
}

/// Receiver of the events one core call produces.
pub trait LowerMacSink<'slot, const BUFFER_SIZE: usize> {
    /// The attempt of `queue`, submitted at `rate`, ended as `end`: its
    /// slot comes back with it.
    fn tx_completed(
        &mut self,
        queue: WmmAccessCategory,
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        end: PublicationEnd,
        rate: PhyRate,
    );
    /// A disable the core started has ended: no attempt is left.
    fn disabled(&mut self);
    /// The station's TBTT event announced the TBTT `event` names.
    fn tbtt(&mut self, event: TbttEvent);
}

/// Why the backend's state is unknown: the port is poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacFault {
    /// The ordinary TX owner failed or quarantined its descriptor.
    Tx(OrdinaryTxError),
    /// A receive Block Ack bank did not read back as programmed.
    RxBlockAckReadback,
    /// The hardware refused a checked key.
    Key(CryptoKeyError),
    /// The hardware refused a checked receive Block Ack agreement.
    RxBlockAck(S31RxBlockAckAgreementError),
    /// An agreement found no free bank, which a slot of the backend's
    /// shared pool always holds.
    NoRxBlockAckBank,
    /// The port's owner abandoned the backend: a fail-stop or a dropped
    /// owner. Only a reset restores it.
    Abandoned,
}

/// How one attempt ended, as the ESP32-S31 observed it. The backend maps it
/// onto the portable completion. A detached hardware timeout is
/// `TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)`: whether
/// its PPDU reached the air is unknown. An executor watchdog without a
/// qualified detach instead poisons and quarantines the publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationEnd {
    /// Its completion arrived: the hardware's status, the ACK's SNR and a
    /// BlockAck the attempt solicited.
    Completed {
        status: TxStatus,
        ack_snr_db: Option<i8>,
        block_ack: Option<BlockAckReport>,
    },
    /// An internal collision detached it: the publication did not reach
    /// the air.
    Collision,
    /// The queue's hardware timeout and its abort: whether the PPDU reached
    /// the air is unknown.
    TimedOut,
    /// Held behind the closed gate or a disable, it was never published.
    Withdrawn,
}

/// What an `Enable` needs from the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleStart {
    /// The port is enabled.
    Enabled,
    /// Retune to the channel, then call [`LowerMacCore::finish_retune`].
    Retune(WifiChannel),
}

/// One interface's configuration: its role, address, BSSID and the frames
/// it receives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreVif {
    pub role: VifRole,
    pub address: MacAddress,
    pub bssid: Option<MacAddress>,
    pub receive: ReceiveFilter,
}

impl CoreVif {
    /// The interface's BSS: its BSSID, or an access point's address.
    fn bss(&self) -> Option<MacAddress> {
        match self.role {
            VifRole::Station => self.bssid,
            VifRole::AccessPoint => Some(self.address),
        }
    }

    /// Whether the receive filter admits `frame`, an MPDU without FCS: the
    /// hardware policies pass a superset of the requested rules, which
    /// this narrows.
    fn admits(&self, frame: &[u8]) -> bool {
        let filter = self.receive;
        let Some(address1) = frame_address(frame, 4) else {
            return false;
        };
        let frame_type = (frame[0] >> 2) & 0b11;
        let subtype = frame[0] >> 4;
        if frame_type == FRAME_TYPE_CONTROL {
            return filter.contains(ReceiveFilter::OWN_CONTROL) && address1 == self.address;
        }
        let management = frame_type == FRAME_TYPE_MANAGEMENT;
        if management
            && matches!(subtype, SUBTYPE_BEACON | SUBTYPE_PROBE_RESPONSE)
            && filter.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT)
        {
            return true;
        }
        if management
            && subtype == SUBTYPE_PROBE_REQUEST
            && filter.contains(ReceiveFilter::PROBE_REQUESTS)
        {
            let bssid = frame_bssid(frame);
            return (address1 == self.address || address1 == BROADCAST_ADDRESS)
                && (bssid == Some(BROADCAST_ADDRESS) || bssid == self.bss() && bssid.is_some());
        }
        if filter.contains(ReceiveFilter::OWN_UNICAST) && address1 == self.address {
            return true;
        }
        let Some(bss) = self.bss() else {
            return false;
        };
        let in_bss = frame_bssid(frame).is_some_and(|bssid| bssid == bss);
        if management && subtype == SUBTYPE_BEACON {
            return in_bss && filter.contains(ReceiveFilter::OWN_BSS_BEACONS);
        }
        address1[0] & 1 != 0 && in_bss && filter.contains(ReceiveFilter::OWN_BSS_GROUP)
    }
}

const FRAME_TYPE_MANAGEMENT: u8 = 0;
const FRAME_TYPE_CONTROL: u8 = 1;
const FRAME_TYPE_DATA: u8 = 2;
const SUBTYPE_PROBE_REQUEST: u8 = 4;
const SUBTYPE_PROBE_RESPONSE: u8 = 5;
const SUBTYPE_BEACON: u8 = 8;

fn frame_address(frame: &[u8], offset: usize) -> Option<MacAddress> {
    frame.get(offset..offset + 6)?.try_into().ok()
}

/// The BSSID a management or data frame carries.
fn frame_bssid(frame: &[u8]) -> Option<MacAddress> {
    match (frame.first()? >> 2) & 0b11 {
        FRAME_TYPE_MANAGEMENT => frame_address(frame, 16),
        FRAME_TYPE_DATA => match frame.get(1)? & 0b11 {
            0b00 => frame_address(frame, 16),
            // From the DS: Address 2 is the BSSID.
            0b10 => frame_address(frame, 10),
            // To the DS: Address 1 is the BSSID.
            0b01 => frame_address(frame, 4),
            _ => None,
        },
        _ => None,
    }
}

/// An installed key, by the core's index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyIndex(u8);

/// Why a fresh MPDU has not received a CCMP header and must not be
/// submitted as numbered protected TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpNumberingError {
    /// The index no longer names an installed key.
    MissingKey,
    /// A station group key is valid for reception only; it cannot prepare TX.
    ReceiveOnlyKey,
    /// Software cannot currently write the slot's buffer.
    BufferUnavailable(TxError),
    /// The CCMP header does not fit after the metadata and MAC header,
    /// including when the offset overflows.
    InsufficientSpace,
    /// The key cannot allocate another transmit packet number.
    PacketNumber(CcmpTxPacketNumberError),
}

/// A receive Block Ack agreement whose values a caller's types guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckValues {
    pub role: VifRole,
    pub peer: MacAddress,
    pub tid: u8,
    pub start_sequence: SequenceNumber,
    pub window: u8,
}

/// The beacon receive priority ([`LowerMacCore::set_rx_beacon_priority`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBeaconPriority {
    /// The radio system's beacon-window priority.
    BeaconWindow,
    /// Priority zero.
    Zero,
    /// No beacon receive priority request.
    Cleared,
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
    pub publication_timeout: oer_time::Duration,
    /// The epoch of the core's TSF relations, a number no other owner of a
    /// TSF relation took (`MacClockHandle::tsf_epoch`).
    pub tsf_epoch: u32,
}

/// The owners a disabled, idle [`LowerMacCore`] is built from, which
/// [`LowerMacCore::dismantle`] returns.
pub struct LowerMacCoreParts<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    pub config: LowerMacConfig,
    pub coex: &'slot dyn RadioCoexPriorities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortState {
    Disabled,
    Enabling,
    Enabled,
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
    token: KeyToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckEntry {
    role: VifRole,
    peer: MacAddress,
    tid: u8,
}

/// What one queue's attempt holds.
enum Work<'slot, const BUFFER_SIZE: usize> {
    /// An MPDU in its slot, not yet published: the transmit gate is
    /// closed.
    Held {
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        plan: OrdinaryTxPlan,
        single: SingleAttempt,
    },
    Published(QueuedSingleAttempt<'slot, BUFFER_SIZE>),
}

impl<const BUFFER_SIZE: usize> Work<'_, BUFFER_SIZE> {
    const fn held(&self) -> bool {
        matches!(self, Self::Held { .. })
    }

    /// The pending deadline of a published attempt, and whether it is the
    /// end of a timeout abort's settle.
    const fn deadline(&self) -> Option<(oer_time::Instant, bool)> {
        match self {
            Self::Published(queued) => Some((queued.deadline(), queued.abort_settling())),
            Self::Held { .. } => None,
        }
    }
}

/// The attempt occupying one queue.
struct Attempt<'slot, const BUFFER_SIZE: usize> {
    role: VifRole,
    key: Option<KeyIndex>,
    /// The rate of its publication, which its completion reports.
    rate: PhyRate,
    work: Work<'slot, BUFFER_SIZE>,
}

/// The running station TBTT schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StationTbtt {
    first: u64,
    interval: u64,
}

impl StationTbtt {
    /// The TBTT an event at station TSF `now` announces: the first one at
    /// or after `now`, since the event fires its lead before it.
    const fn announced(self, now: u64) -> u64 {
        if now <= self.first {
            return self.first;
        }
        let periods = (now - self.first).div_ceil(self.interval);
        self.first
            .saturating_add(periods.saturating_mul(self.interval))
    }
}

/// The ESP32-S31 lower-MAC state the backend's driver owns: the hardware
/// side of the port, one published attempt per EDCA queue.
/// Diagnostic callers cannot borrow the nested transmitter:
/// ```compile_fail,E0599
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// use oer_esp32s31_ieee80211::lower_mac::LowerMacCore;
/// use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};
/// fn borrow_tx<P: WifiTxPowerProfile, E: WifiTxEntropy, T: oer_time::Timer, const N: usize>(
///     core: &LowerMacCore<'_, P, E, T, N>,
/// ) {
///     core.tx();
/// }
/// ```
/// A diagnostic can read the next publication deadline directly:
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// use oer_esp32s31_ieee80211::lower_mac::LowerMacCore;
/// use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};
/// fn deadline<P: WifiTxPowerProfile, E: WifiTxEntropy, T: oer_time::Timer, const N: usize>(
///     core: &LowerMacCore<'_, P, E, T, N>,
/// ) -> Option<oer_time::Instant> {
///     core.next_deadline()
/// }
/// ```
pub struct LowerMacCore<'slot, P, E, T, const BUFFER_SIZE: usize> {
    tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: LowerMacConfig,
    state: PortState,
    /// The configured channel; the radio is tuned to it unless a retune is
    /// pending.
    channel: WifiChannel,
    tuned: WifiChannel,
    vifs: [Option<CoreVif>; LOWER_MAC_VIFS as usize],
    keys: [Option<InstalledKey>; LOWER_MAC_KEY_SLOTS],
    rx_block_acks: [Option<RxBlockAckEntry>; RX_BLOCK_ACK_BANKS],
    gate_open: bool,
    monitor: bool,
    tbtt: Option<StationTbtt>,
    /// The owner of the station TSF writes and its relation.
    station_tsf: StationTsf,
    /// The relation of the access-point TSF, which only restarts.
    access_point_tsf: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf,
    /// The attempt of each ordinary queue, by its hardware index
    /// ([`LegacyTxQueue::hardware_index`]).
    queues: [Option<Attempt<'slot, BUFFER_SIZE>>; LOWER_MAC_TX_QUEUES],
    /// The radio system's coexistence priorities: the beacon window's and
    /// a connection frame's.
    coex: &'slot dyn RadioCoexPriorities,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> LowerMacCore<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// A disabled core over an idle ordinary TX owner. `coex` is the radio
    /// system the core shares its RF with: beacon reception and connection
    /// frames ask for the air at its priorities.
    pub fn new(
        tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        config: LowerMacConfig,
        coex: &'slot dyn RadioCoexPriorities,
    ) -> Self {
        Self {
            tx,
            config,
            state: PortState::Disabled,
            channel: config.channel,
            tuned: config.channel,
            vifs: [None; LOWER_MAC_VIFS as usize],
            keys: [const { None }; LOWER_MAC_KEY_SLOTS],
            rx_block_acks: [None; RX_BLOCK_ACK_BANKS],
            gate_open: true,
            monitor: false,
            tbtt: None,
            station_tsf: StationTsf::new(config.tsf_epoch),
            access_point_tsf: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf::new(
                config.tsf_epoch,
            ),
            queues: [const { None }; LOWER_MAC_TX_QUEUES],
            coex,
        }
    }

    /// A disabled core over the owners a dismantled core returned: the
    /// state of the core they came from is gone, as at a radio start.
    pub fn from_parts(parts: LowerMacCoreParts<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        let LowerMacCoreParts { tx, config, coex } = parts;
        Self::new(tx, config, coex)
    }

    /// Take the core apart into the owners it was built from, for a later
    /// core over the same storage: a radio stop and start, or a port
    /// reinstall. Only a disabled core with no attempt admitted comes
    /// apart; another is returned unchanged.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc driver returns the unconsumed core by value"
    )]
    pub fn dismantle(self) -> Result<LowerMacCoreParts<'slot, P, E, T, BUFFER_SIZE>, Self> {
        if !self.idle_disabled() {
            return Err(self);
        }
        Ok(LowerMacCoreParts {
            tx: self.tx,
            config: self.config,
            coex: self.coex,
        })
    }

    /// The channel the radio is tuned to: the configured one unless an
    /// `Enable` still awaits its retune.
    pub const fn tuned_channel(&self) -> WifiChannel {
        self.tuned
    }

    /// Whether the core is disabled with no attempt admitted: what a
    /// radio stop, [`Self::dismantle`] and [`Self::radio_tuned_to`] need.
    pub fn idle_disabled(&self) -> bool {
        self.state == PortState::Disabled && self.attempts().next().is_none()
    }

    /// Record that the radio is tuned to `channel`, as the radio start that
    /// hands the core its hardware reports it: the next `Enable` retunes
    /// when the configured channel differs. `false`, and nothing recorded,
    /// unless the core is disabled and idle.
    pub fn radio_tuned_to(&mut self, channel: WifiChannel) -> bool {
        if !self.idle_disabled() {
            return false;
        }
        self.tuned = channel;
        true
    }

    /// The channel received frames are reported on.
    pub fn channel(&self) -> Channel {
        Channel::from_wifi_channel(self.channel)
    }

    /// The radio clock: the ordinary TX owner's timer.
    #[cfg(test)]
    fn now(&self) -> oer_time::Instant {
        self.tx.now()
    }

    /// The earliest deadline of a published attempt or of the timeout
    /// abort settling, which the runtime turns into [`WifiTxWake::Deadline`].
    /// While an abort settles, no other queue's deadline comes before its
    /// end: that queue's abort waits for it.
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        let deadlines = || {
            self.queues
                .iter()
                .flatten()
                .filter_map(|attempt| attempt.work.deadline())
        };
        let settle = deadlines()
            .filter(|(_, settling)| *settling)
            .map(|(deadline, _)| deadline)
            .min();
        deadlines()
            .map(|(deadline, _)| settle.map_or(deadline, |settle| deadline.max(settle)))
            .min()
    }

    /// A backoff for an attempt of `access_category`, drawn by the vendor's
    /// policy from the category's contention window: the phase contract
    /// leaves the backoff to the backend, and the S31 draws it in
    /// software.
    #[cfg(test)]
    fn draw_backoff(&mut self, access_category: WmmAccessCategory) -> u16 {
        self.tx
            .contention_publication(LegacyTxQueue::from_access_category(access_category))
            .1
    }

    /// Whether the queue of `access_category` holds an attempt.
    #[cfg(test)]
    fn queue_busy(&self, access_category: WmmAccessCategory) -> bool {
        let queue = LegacyTxQueue::from_access_category(access_category);
        self.queues[usize::from(queue.hardware_index())].is_some()
    }

    const fn receiving(&self) -> bool {
        matches!(self.state, PortState::Enabled)
    }

    fn attempts(&self) -> impl Iterator<Item = &Attempt<'slot, BUFFER_SIZE>> {
        self.queues.iter().flatten()
    }

    /// Whether a queue's timeout abort forces CCA now.
    fn abort_settling(&self) -> bool {
        self.attempts().any(|attempt| {
            attempt
                .work
                .deadline()
                .is_some_and(|(_, settling)| settling)
        })
    }

    /// Write the CCMP header of the next packet number of `key` into the
    /// fresh MPDU in `slot`, after its `header_len` octets of MAC header:
    /// the backend numbers a fresh MPDU at its first transmission.
    ///
    /// `Ok(())` means the complete CCMP header was written. Buffer access
    /// and header space are checked before a packet number is allocated.
    /// On any error the buffer and the key's packet number are unchanged;
    /// the caller must not submit the MPDU as numbered protected TX.
    ///
    /// # Errors
    ///
    /// A missing key, a receive-only station group key, an unavailable
    /// buffer, insufficient header space or packet-number exhaustion is a
    /// distinct [`CcmpNumberingError`]. A receive-only key remains valid
    /// for reception, but never reports successful TX preparation here.
    pub fn number(
        &mut self,
        key: KeyIndex,
        slot: &mut Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        header_len: usize,
    ) -> Result<(), CcmpNumberingError> {
        let installed = self
            .keys
            .get_mut(usize::from(key.0))
            .and_then(Option::as_mut)
            .ok_or(CcmpNumberingError::MissingKey)?;
        let buffer = slot
            .as_mut()
            .buffer_mut()
            .map_err(CcmpNumberingError::BufferUnavailable)?;
        let at = TX_METADATA_SIZE
            .checked_add(header_len)
            .ok_or(CcmpNumberingError::InsufficientSpace)?;
        let place = buffer
            .get_mut(at..)
            .and_then(|remaining| remaining.get_mut(..CCMP_HEADER_LEN))
            .ok_or(CcmpNumberingError::InsufficientSpace)?;
        let header = match &mut installed.token {
            KeyToken::StaPairwise(key) => key.next_tx_ccmp_header(),
            KeyToken::StaGroup(_) => return Err(CcmpNumberingError::ReceiveOnlyKey),
            KeyToken::ApPairwise(key) => key.next_tx_ccmp_header(),
            KeyToken::ApGroup(key) => key.next_tx_ccmp_header(),
        }
        .map_err(CcmpNumberingError::PacketNumber)?;
        place.copy_from_slice(&header);
        Ok(())
    }

    /// Publish an MPDU whose values and state a caller's types guarantee
    /// ([`MpduValues`] states which), or hold it behind a closed gate: one
    /// call, so no other operation comes between its plan and its
    /// publication. `retry` marks a transmission of an MPDU the plan sends
    /// again: its Retry bit is set and its CCMP header kept.
    ///
    /// # Errors
    ///
    /// A fault of the transmit owner, with the slot.
    #[allow(clippy::result_large_err, reason = "the slot goes back by value")]
    pub fn submit_checked<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        values: MpduValues<'slot, BUFFER_SIZE>,
    ) -> Result<(), SubmitFault<'slot, BUFFER_SIZE>> {
        let MpduValues {
            role,
            access_category,
            mut slot,
            len,
            no_ack,
            rate,
            reported,
            protection,
            key,
            power_ceiling_dbm,
            backoff_slots,
            coex,
            retry,
        } = values;
        debug_assert_eq!(self.state, PortState::Enabled, "a phase submits");
        if retry {
            let marked = slot.as_mut().buffer_mut().map_err(OrdinaryTxError::from)
                .and_then(|buffer| crate::ordinary_tx::mark_retry(buffer, len));
            if let Err(error) = marked {
                return Err(SubmitFault { fault: LowerMacFault::Tx(error), slot });
            }
        }
        let common = self.plan_common(
            access_category,
            role,
            rate,
            protection,
            key,
            power_ceiling_dbm,
            backoff_slots,
        );
        let admission = self.plan_mpdu(common, len, access_category, coex, no_ack);
        let index = usize::from(admission.queue.hardware_index());
        let work = if self.gate_open {
            match self.tx.start_queued_single_attempt(
                hardware,
                slot,
                admission.plan,
                admission.single,
            ) {
                Ok(queued) => Work::Published(queued),
                Err(QueuedSingleAttemptRefused { error, slot }) => {
                    return Err(SubmitFault {
                        fault: LowerMacFault::Tx(error),
                        slot,
                    });
                }
            }
        } else {
            Work::Held {
                slot,
                plan: admission.plan,
                single: admission.single,
            }
        };
        self.queues[index] = Some(Attempt {
            role,
            key: admission.key,
            rate: reported,
            work,
        });
        Ok(())
    }

    /// What every attempt shares, planned from values its caller's types
    /// guarantee: `key`, when one, is installed.
    #[allow(clippy::too_many_arguments, reason = "the fields every attempt shares")]
    fn plan_common(
        &self,
        access_category: WmmAccessCategory,
        role: VifRole,
        rate: TxPhyRate,
        protection: Protection,
        key: Option<KeyIndex>,
        power_ceiling_dbm: Option<i8>,
        backoff_slots: u16,
    ) -> CommonAdmission {
        let selector = key
            .and_then(|key| self.key(key))
            .map(|installed| installed.token.hardware_index());
        let (hardware_key_selector, hardware_mic_length) = match selector {
            None => (0, 0),
            Some(index) => (index, TX_CCMP_MIC_SIZE),
        };
        CommonAdmission {
            queue: LegacyTxQueue::from_access_category(access_category),
            role,
            rate,
            key: selector.and(key),
            hardware_key_selector,
            hardware_mic_length,
            single: SingleAttempt {
                protection: match protection {
                    Protection::None => SingleAttemptProtection::None,
                    Protection::RtsCts => SingleAttemptProtection::RtsCts,
                    Protection::CtsToSelf => SingleAttemptProtection::CtsToSelf,
                },
                backoff_slots,
                power_ceiling_dbm,
                no_ack: false,
            },
        }
    }

    /// Plan one MPDU's publication from its common values.
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-coexistence-levels
    fn plan_mpdu(
        &self,
        common: CommonAdmission,
        length: usize,
        access_category: WmmAccessCategory,
        coex: CoexPriority,
        no_ack: bool,
    ) -> Admission {
        let queue = common.queue;
        let (scheduler_priority, packet_priority, priority_count) = match coex {
            // A connection frame under the radio system's reconnect policy.
            CoexPriority::Elevated => {
                let connection = self.coex.connection_frame_priorities();
                (
                    connection.scheduler,
                    connection.packet,
                    CONNECTION_FRAME_PRIORITY_COUNT,
                )
            }
            // The static priority vendor data encapsulation assigns the
            // access category.
            _ => (
                queue.vendor_data_scheduler_priority(),
                queue.vendor_data_packet_priority(),
                1,
            ),
        };
        let plan = OrdinaryTxPlan {
            frame_length: length,
            descriptor_capacity: None,
            exchange: MacTxPlan {
                access_category,
                initial_rate: common.rate,
                publication_limit: 1,
                publication_timeout: self.config.publication_timeout,
            },
            hardware_mic_length: common.hardware_mic_length,
            hardware_key_selector: common.hardware_key_selector,
            interface: match common.role {
                VifRole::Station => OrdinaryTxInterface::Station,
                VifRole::AccessPoint => OrdinaryTxInterface::AccessPoint,
            },
            scheduler_priority,
            packet_priority,
            priority_count,
        };
        Admission {
            queue,
            plan,
            single: SingleAttempt {
                no_ack,
                ..common.single
            },
            key: common.key,
        }
    }

    fn key(&self, key: KeyIndex) -> Option<&InstalledKey> {
        self.keys.get(usize::from(key.0)).and_then(Option::as_ref)
    }

    /// Plan an aggregate whose values a caller's types guarantee
    /// ([`AmpduValues`]): a pure function of the core's state, the
    /// aggregate's subframes and its values, which [`ampdu::publish`]
    /// carries out. Every check the aggregate owner would make at commit is
    /// made here first, so a refused aggregate keeps its subframes.
    ///
    /// # Errors
    ///
    /// Why the aggregate cannot go as one publication.
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-a-mpdu
    pub fn plan_ampdu<S: AmpduBacking, const SLOTS: usize>(
        &self,
        values: AmpduValues,
        buffer: &mut Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    ) -> Result<AmpduPlan, AmpduRefusal> {
        let AmpduValues {
            role,
            access_category,
            rate,
            protection,
            key,
            power_ceiling_dbm,
            backoff_slots,
            coex,
            min_mpdu_start_spacing,
        } = values;
        // Connection frames are single MPDUs: an aggregate is ordinary data.
        if coex != CoexPriority::Normal {
            return Err(AmpduRefusal::ElevatedCoexistence);
        }
        let common = self.plan_common(
            access_category,
            role,
            rate,
            protection,
            key,
            power_ceiling_dbm,
            backoff_slots,
        );
        let capabilities = esp32s31_ampdu_capabilities(SLOTS);
        let count = buffer.subframes();
        if count == 0 {
            return Err(AmpduRefusal::Empty);
        }
        if count > usize::from(capabilities.max_subframes) {
            return Err(AmpduRefusal::TooManySubframes);
        }
        if min_mpdu_start_spacing > 7 {
            return Err(AmpduRefusal::StartSpacing);
        }
        if buffer.lengths().any(|len| len < MIN_FRAME_LENGTH) {
            return Err(AmpduRefusal::ShortSubframe);
        }
        // One recipient answers the aggregate with its BlockAck.
        if buffer.first_mpdu().is_none_or(|first| first[4] & 1 != 0) {
            return Err(AmpduRefusal::GroupRecipient);
        }

        let queue = common.queue;
        let ceiling = common.single.power_ceiling_dbm;
        let txop_limit = self
            .tx
            .policy()
            .access_policy(queue)
            .txop_limit_units_32_us();
        let density = HtAmpduDensity::from_ampdu_parameters((min_mpdu_start_spacing & 0x07) << 2);
        let mic_and_fcs = common.hardware_mic_length + TX_FCS_SIZE;
        let (power_code, format) = match common.rate {
            TxPhyRate::Ht(rate) => {
                // The access category's TXOP limit is the caller's to keep.
                // The owner's own length rule: the rate's ceiling and the
                // declared maximum it was configured with.
                let mut budget = buffer
                    .owner
                    .ht_length_budget(rate)
                    .map_err(|_| AmpduRefusal::Length)?;
                for psdu in buffer.lengths().map(|len| len + mic_and_fcs) {
                    let psdu = u32::try_from(psdu).map_err(|_| AmpduRefusal::Length)?;
                    budget.push(psdu, 0).map_err(|_| AmpduRefusal::Length)?;
                }
                (rate.power_lookup_code(), None)
            }
            TxPhyRate::He(rate) => {
                // The HE program carries the access category's advertised
                // TXOP limit; the core sets no ceiling of its own.
                let txop = he_txop_limit(txop_limit, HeEdcaTxopLimit::DEFAULT)
                    .map_err(|_| AmpduRefusal::Length)?;
                let policy = HeAmpduPolicy::new(rate, density, txop);
                let mut budget = buffer
                    .owner
                    .he_length_budget(policy)
                    .map_err(|_| AmpduRefusal::Length)?;
                for psdu in buffer.lengths().map(|len| len + mic_and_fcs) {
                    let psdu = u16::try_from(psdu).map_err(|_| AmpduRefusal::Length)?;
                    let delimiters = rate
                        .ampdu_empty_delimiters(psdu, density)
                        .ok_or(AmpduRefusal::Length)?;
                    budget
                        .push(u32::from(psdu), delimiters)
                        .map_err(|_| AmpduRefusal::Length)?;
                }
                (rate.power_lookup_code(), Some(policy))
            }
            TxPhyRate::Legacy(_) => return Err(AmpduRefusal::LegacyRate),
        };
        let data_power = self.tx.single_attempt_power_pair(power_code, ceiling);
        let publication = AmpduPublication {
            data_power_primary: data_power.primary as u8,
            data_power_alternate: data_power.alternate as u8,
            control: self.tx.single_attempt_control_frame(
                common.rate,
                common.single.protection,
                ceiling,
            ),
            aifsn: self.tx.policy().contention_parameters(queue).aifsn(),
            contention_window: common.single.backoff_slots,
            // `CoexPriority::Normal`, as for an MPDU.
            scheduler_priority: queue.vendor_data_scheduler_priority(),
            packet_priority: queue.vendor_data_packet_priority(),
        };
        let format = match (common.rate, format) {
            (TxPhyRate::Ht(rate), _) => AmpduFormat::Ht {
                rate,
                protection_spacing: ampdu::protection_spacing(min_mpdu_start_spacing),
            },
            (_, Some(policy)) => AmpduFormat::He {
                policy,
                bss_color: self.tx.policy().he_bss_color(),
            },
            (_, None) => return Err(AmpduRefusal::LegacyRate),
        };
        Ok(AmpduPlan {
            queue,
            format,
            role: AmpduTxRoleAdapter {
                interface: mac_interface(common.role),
                hardware_key_selector: common.hardware_key_selector,
            },
            hardware_mic_length: common.hardware_mic_length as u8,
            publication,
        })
    }

    /// Consume one interrupt or deadline edge of the published attempts.
    ///
    /// Every published queue is offered the edge and claims only its own
    /// queue's state. When a timeout abort ends, the CCA it forced is
    /// released, and every queue is offered a timeout edge once more: one
    /// whose timeout arrived during the settle aborts now.
    ///
    /// # Errors
    ///
    /// A fault of the transmit owner.
    pub fn service<H: HtAmpduHardware, K: LowerMacSink<'slot, BUFFER_SIZE>>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        let mut wake = wake;
        loop {
            let mut abort_ended = false;
            for index in 0..LOWER_MAC_TX_QUEUES {
                let Some(attempt) = self.queues[index].take() else {
                    continue;
                };
                let may_begin_timeout_abort = !self.abort_settling();
                let Attempt {
                    role,
                    key,
                    rate,
                    work,
                } = attempt;
                let queued = match work {
                    held @ Work::Held { .. } => {
                        self.queues[index] = Some(Attempt {
                            role,
                            key,
                            rate,
                            work: held,
                        });
                        continue;
                    }
                    Work::Published(queued) => queued,
                };
                let settling = queued.abort_settling();
                match self
                    .tx
                    .service_queued_single_attempt(hardware, queued, wake, may_begin_timeout_abort)
                    .map_err(LowerMacFault::Tx)?
                {
                    QueuedSingleAttemptProgress::Pending(queued) => {
                        self.queues[index] = Some(Attempt {
                            role,
                            key,
                            rate,
                            work: Work::Published(queued),
                        });
                    }
                    QueuedSingleAttemptProgress::Complete { outcome, slot } => {
                        abort_ended |= settling;
                        sink.tx_completed(
                            queue_category(index),
                            slot,
                            publication_end(outcome),
                            rate,
                        );
                    }
                }
            }
            if !abort_ended {
                break;
            }
            wake = WifiTxWake::Interrupt {
                events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_TIMEOUT,
            };
        }
        self.settle(sink);
        Ok(())
    }

    /// Finish a disable that waited for the attempts.
    fn settle<K: LowerMacSink<'slot, BUFFER_SIZE>>(&mut self, sink: &mut K) {
        if self.state == PortState::Disabling && self.attempts().next().is_none() {
            self.state = PortState::Disabled;
            sink.disabled();
        }
    }

    /// The owned view of one received MPDU, or `None` while the port does
    /// not receive or no receive rule admits it. The hardware policies pass
    /// a superset of the requested rules; this narrows it to them.
    ///
    /// The receive prefix carries no noise floor; the frame's is the PHY's
    /// noise-floor estimate when it is received, the value the S31 station
    /// reads for its rate control (`StaNoiseFloorHardware`).
    pub fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        let admitted = self.monitor || self.vifs.iter().flatten().any(|vif| vif.admits(frame.mpdu));
        (self.receiving() && admitted).then(|| {
            let noise_floor_dbm = hardware.read_noise_floor_dbm();
            let mut common = portable::rx_meta(frame.metadata, self.channel());
            common.noise_floor_dbm = RxEvidence::HardwareObserved(noise_floor_dbm);
            (
                frame.mpdu,
                Esp32s31RxMeta {
                    common,
                    rssi_dbm: frame.rssi_dbm,
                    noise_floor_dbm,
                },
            )
        })
    }

    /// Enable a disabled port: at once on the tuned channel, or after the
    /// retune [`LifecycleStart::Retune`] asks for.
    pub fn enable_checked(&mut self) -> LifecycleStart {
        debug_assert_eq!(self.state, PortState::Disabled, "a disabled port enables");
        if self.channel == self.tuned {
            self.state = PortState::Enabled;
            LifecycleStart::Enabled
        } else {
            self.state = PortState::Enabling;
            LifecycleStart::Retune(self.channel)
        }
    }

    /// Complete an `Enable` after the runtime retuned the radio: `true`
    /// once enabled. A refused retune leaves the port disabled on the
    /// channel it was tuned to.
    pub fn finish_retune(&mut self, retuned: bool) -> bool {
        if self.state != PortState::Enabling {
            return false;
        }
        if !retuned {
            self.state = PortState::Disabled;
            return false;
        }
        self.tuned = self.channel;
        self.state = PortState::Enabled;
        true
    }

    /// Disable an enabled port: every held attempt ends as aborted at once,
    /// the published ones with their own completions, then
    /// [`LowerMacSink::disabled`] follows.
    pub fn disable_checked<K: LowerMacSink<'slot, BUFFER_SIZE>>(&mut self, sink: &mut K) {
        debug_assert_eq!(self.state, PortState::Enabled, "an enabled port disables");
        self.state = PortState::Disabling;
        for index in 0..LOWER_MAC_TX_QUEUES {
            self.abort_held(index, sink);
        }
        self.settle(sink);
    }

    /// End a queue's held attempt as [`PublicationEnd::Withdrawn`]: it
    /// never reached the hardware, and its slot comes back with it.
    fn abort_held<K: LowerMacSink<'slot, BUFFER_SIZE>>(&mut self, index: usize, sink: &mut K) {
        if !self.queues[index]
            .as_ref()
            .is_some_and(|attempt| attempt.work.held())
        {
            return;
        }
        let Some(Attempt {
            work: Work::Held { slot, .. },
            rate,
            ..
        }) = self.queues[index].take()
        else {
            return;
        };
        sink.tx_completed(queue_category(index), slot, PublicationEnd::Withdrawn, rate);
    }

    /// Return every held attempt without publishing it.
    #[cfg(test)]
    fn withdraw_held_attempts<K: LowerMacSink<'slot, BUFFER_SIZE>>(&mut self, sink: &mut K) {
        for index in 0..LOWER_MAC_TX_QUEUES {
            self.abort_held(index, sink);
        }
    }

    /// The station's HE BSS color, at most 63.
    pub fn set_checked_he_bss_color(&mut self, color: u8) {
        debug_assert!(color <= 63, "a BSS color is at most 63");
        self.tx.policy_mut().install_he_bss_color(color);
    }

    /// The beacon receive priority.
    // SOURCE(esp32s31): complete pinned `libpp[pm_coex.o]::pm_coex_update_rx_beacon_pti`
    // passes `coex_pti_get(0)`, or zero, as both arguments of
    // `hal_set_rx_beacon_pti`.
    pub fn set_rx_beacon_priority<H: RxBeaconPriorityHardware>(
        &mut self,
        hardware: &mut H,
        priority: RxBeaconPriority,
    ) {
        match priority {
            RxBeaconPriority::BeaconWindow => {
                hardware.set_rx_beacon_pti(self.coex.beacon_window_pti());
            }
            RxBeaconPriority::Zero => hardware.set_rx_beacon_pti(PRIORITY_ZERO),
            RxBeaconPriority::Cleared => hardware.clear_rx_beacon_pti(),
        }
    }

    /// The EDCA parameters every queue's attempts contend with, whose
    /// records lie within the capabilities' limits: a set the policy
    /// refuses leaves the installed one.
    ///
    /// # Errors
    ///
    /// The record the policy refused.
    pub fn set_checked_edca(
        &mut self,
        parameters: oer_ieee80211_mac::extensions::wmm::WmmParameterSet,
    ) -> Result<(), oer_esp32s31_ieee80211_mac::edca::EdcaParametersError> {
        self.tx.policy_mut().install_wmm(parameters)
    }

    /// Tune the next `Enable` to `channel`, on a disabled port.
    pub fn set_checked_channel(&mut self, channel: WifiChannel) {
        debug_assert_eq!(self.state, PortState::Disabled, "a disabled port retunes");
        if self.channel != channel {
            self.break_tsf_relation(VifRole::Station);
            self.break_tsf_relation(VifRole::AccessPoint);
        }
        self.channel = channel;
    }

    /// Configure an interface whose configuration a caller's types
    /// guarantee: no attempt of its role in flight, no monitor reception
    /// while it receives anything, its receive rules within the role's
    /// limits, a station's address the cold start's
    /// ([`Self::station_address`]) and a BSSID whenever it receives
    /// anything but other BSSs' management, an access point's address
    /// individual and its BSSID none or its address.
    ///
    /// # Errors
    ///
    /// A configuration the receive policies cannot take: nothing changed.
    pub fn configure_checked_vif<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        config: CoreVif,
    ) -> Result<(), UnsupportedVif> {
        debug_assert!(
            !self.attempts().any(|attempt| attempt.role == config.role),
            "an interface's frames in flight are waited for"
        );
        let policy = receive_policy(&self.config, &config)?;
        self.apply_vif(hardware, config, policy);
        Ok(())
    }

    /// The station address the cold start published.
    pub const fn station_address(&self) -> MacAddress {
        self.config.station_address
    }

    /// Apply `config` and its receive `policy` to its role's interface.
    fn apply_vif<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        config: CoreVif,
        policy: ReceivePolicy,
    ) {
        match policy {
            ReceivePolicy::StationDisabled => hardware.disable_station_receive_registers(),
            ReceivePolicy::Station { bssid } => hardware.apply_sta_link_policy(bssid),
            ReceivePolicy::StationOtherBss { bssid } => hardware.apply_sta_esp_now_policy(bssid),
            ReceivePolicy::AccessPointDisabled => hardware.disable_ap_link_policy(),
            ReceivePolicy::AccessPoint { address } => hardware.apply_ap_link_policy(address),
        }
        let previous = self.vifs[config.role as usize].replace(config);
        // A new association, a reassociation or a roam: the interface
        // follows another TSF.
        if previous.is_none_or(|previous| previous.bss() != config.bss()) {
            self.break_tsf_relation(config.role);
        }
    }

    /// Install a key whose values and state a caller's types guarantee:
    /// the scope's slots have room (an access point's `association_id` is
    /// free), and a group key identifier is a valid one.
    ///
    /// # Errors
    ///
    /// The hardware refused the key.
    pub fn install_checked_key<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        scope: CheckedKeyScope,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<KeyIndex, LowerMacFault> {
        let free = self
            .keys
            .iter()
            .position(Option::is_none)
            .ok_or(LowerMacFault::Key(CryptoKeyError::Occupied))?;
        let token = match scope {
            CheckedKeyScope::StationPairwise { peer } => {
                install_sta_pairwise_ccmp(hardware, peer, temporal_key).map(KeyToken::StaPairwise)
            }
            CheckedKeyScope::StationGroup { key_id } => {
                install_sta_group_ccmp(hardware, key_id, temporal_key).map(KeyToken::StaGroup)
            }
            CheckedKeyScope::AccessPointPairwise {
                peer,
                association_id,
            } => install_ap_pairwise_ccmp(hardware, peer, association_id, temporal_key)
                .map(KeyToken::ApPairwise),
            CheckedKeyScope::AccessPointGroup { key_id } => {
                install_ap_group_ccmp(hardware, key_id, temporal_key).map(KeyToken::ApGroup)
            }
        }
        .map_err(LowerMacFault::Key)?;
        self.keys[free] = Some(InstalledKey { token });
        Ok(KeyIndex(free as u8))
    }

    /// Whether an attempt in flight uses `key`.
    fn key_in_use(&self, key: KeyIndex) -> bool {
        self.attempts().any(|attempt| attempt.key == Some(key))
    }

    /// Remove an installed key no attempt in flight uses; another index is
    /// nothing to remove.
    pub fn remove_checked_key<H: CcmpKeyHardware>(&mut self, hardware: &mut H, key: KeyIndex) {
        debug_assert!(!self.key_in_use(key), "a key in use is waited for");
        if let Some(installed) = self.keys.get_mut(usize::from(key.0)).and_then(Option::take) {
            installed.token.clear(hardware);
        }
    }

    /// Start an agreement whose values a caller's types guarantee: its TID
    /// and window within the limits, its peer individual. It replaces the
    /// agreement of the same peer and TID in its bank, or takes a free one.
    ///
    /// # Errors
    ///
    /// No bank is free, or the hardware refused or misread the agreement.
    pub fn start_checked_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        agreement: RxBlockAckValues,
    ) -> Result<(), LowerMacFault> {
        let bank = self
            .rx_block_ack_bank(agreement.role, agreement.peer, agreement.tid)
            .or_else(|| self.rx_block_acks.iter().position(Option::is_none))
            .ok_or(LowerMacFault::NoRxBlockAckBank)?;
        hardware
            .program_rx_block_ack(S31RxBlockAckAgreement {
                hardware_index: bank as u8,
                interface: mac_interface(agreement.role),
                peer: agreement.peer,
                tid: agreement.tid,
                starting_sequence: agreement.start_sequence,
                window: u16::from(agreement.window),
            })
            .map_err(|error| match error {
                S31RxBlockAckAgreementError::HardwareReadbackMismatch => {
                    LowerMacFault::RxBlockAckReadback
                }
                error => LowerMacFault::RxBlockAck(error),
            })?;
        self.rx_block_acks[bank] = Some(RxBlockAckEntry {
            role: agreement.role,
            peer: agreement.peer,
            tid: agreement.tid,
        });
        Ok(())
    }

    /// End the agreement of `role` with `peer` on `tid`: `false` when none
    /// exists.
    ///
    /// # Errors
    ///
    /// The hardware refused to clear its bank.
    pub fn end_checked_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        role: VifRole,
        peer: MacAddress,
        tid: u8,
    ) -> Result<bool, LowerMacFault> {
        let Some(bank) = self.rx_block_ack_bank(role, peer, tid) else {
            return Ok(false);
        };
        hardware
            .clear_rx_block_ack(bank as u8)
            .map_err(LowerMacFault::RxBlockAck)?;
        self.rx_block_acks[bank] = None;
        Ok(true)
    }

    fn rx_block_ack_bank(&self, role: VifRole, peer: MacAddress, tid: u8) -> Option<usize> {
        self.rx_block_acks
            .iter()
            .position(|entry| *entry == Some(RxBlockAckEntry { role, peer, tid }))
    }

    /// Stop publishing at once: every attempt admitted from now on is held
    /// in its slot, as behind a closed gate, while the published ones
    /// finish on the air. The first step of closing the gate under
    /// traffic; [`Self::block_held_queues`] is the second.
    pub fn hold_new_attempts(&mut self) {
        self.gate_open = false;
    }

    /// Whether no attempt is published: every admitted one is held.
    pub fn published_idle(&self) -> bool {
        !self.attempts().any(|attempt| !attempt.work.held())
    }

    /// Block every ordinary queue once [`Self::hold_new_attempts`] held the
    /// new attempts and no attempt is published
    /// ([`Self::published_idle`]): the gate is closed.
    pub fn block_held_queues<H: LowerMacHardware>(&mut self, hardware: &mut H) {
        debug_assert!(!self.gate_open, "new attempts are held first");
        debug_assert!(self.published_idle(), "no attempt is published");
        hardware.set_power_save_tx_block(true);
    }

    /// Open the transmit gate: the power-save block lifts, and every held
    /// attempt is published.
    ///
    /// # Errors
    ///
    /// A fault of the transmit owner, with the slot it could not publish.
    #[allow(clippy::result_large_err, reason = "the slot goes back by value")]
    pub fn open_gate<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), SubmitFault<'slot, BUFFER_SIZE>> {
        hardware.set_power_save_tx_block(false);
        self.gate_open = true;
        for index in 0..LOWER_MAC_TX_QUEUES {
            let Some(attempt) = self.queues[index].take() else {
                continue;
            };
            let Attempt {
                role,
                key,
                rate,
                work,
            } = attempt;
            let work = match work {
                Work::Held { slot, plan, single } => {
                    match self
                        .tx
                        .start_queued_single_attempt(hardware, slot, plan, single)
                    {
                        Ok(queued) => Work::Published(queued),
                        Err(QueuedSingleAttemptRefused { error, slot }) => {
                            return Err(SubmitFault {
                                fault: LowerMacFault::Tx(error),
                                slot,
                            });
                        }
                    }
                }
                published @ Work::Published(_) => published,
            };
            self.queues[index] = Some(Attempt {
                role,
                key,
                rate,
                work,
            });
        }
        Ok(())
    }

    /// The station's TSF and the generation of its relation, for a
    /// [`TsfSample`](oer_ieee80211_lower_mac::TsfSample) the backend pairs
    /// with its radio clock: the access-point timer has no read seam.
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-access-point-tsf
    pub fn station_tsf_reading<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
    ) -> (VifTsf, TsfGeneration) {
        let at = self.station_tsf.read(hardware);
        (
            VifTsf {
                vif: VifRole::Station,
                at,
            },
            self.station_tsf.generation(),
        )
    }

    /// Start a new generation of the TSF relation of the interface with
    /// `role`.
    fn break_tsf_relation(&mut self, role: VifRole) {
        match role {
            VifRole::Station => self.station_tsf.break_relation(),
            VifRole::AccessPoint => self.access_point_tsf.break_relation(),
        }
    }

    /// Set the station's TSF. A set beyond the drift its relation allows
    /// starts a new generation of the relation.
    ///
    /// # Errors
    ///
    /// The set the relation cannot take.
    pub fn set_station_tsf<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        at: TsfInstant,
    ) -> Result<(), TsfTimingError> {
        self.station_tsf.set(hardware, at).map(|_| ())
    }

    /// Restart the access point's TSF from zero, a new generation of its
    /// relation.
    pub fn restart_access_point_tsf<H: LowerMacHardware>(&mut self, hardware: &mut H) {
        self.access_point_tsf.restart(hardware);
    }

    /// Program the station TBTT schedule. The event fires the schedule's
    /// lead before each TBTT; the wake lead published beside it keeps the
    /// vendor's window above the lead.
    ///
    /// # Errors
    ///
    /// A schedule the timer registers cannot hold: nothing changed.
    pub fn set_station_tbtt<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        next: TsfInstant,
        beacon_interval_micros: u64,
        lead_micros: u64,
    ) -> Result<(), UnsupportedTbtt> {
        let interval_micros = u32::try_from(beacon_interval_micros)
            .ok()
            .filter(|&interval| interval != 0)
            .ok_or(UnsupportedTbtt)?;
        let ahead_micros = u16::try_from(lead_micros).map_err(|_| UnsupportedTbtt)?;
        let wake_ahead_micros = ahead_micros
            .checked_add(STATION_TBTT_WAKE_WINDOW_MICROS)
            .ok_or(UnsupportedTbtt)?;
        let first = next.as_micros();
        hardware.start_station_tbtt(StaTbttSchedule {
            first_tbtt_tsf: first,
            interval_micros,
            ahead_micros,
            wake_ahead_micros,
        });
        self.tbtt = Some(StationTbtt {
            first,
            interval: u64::from(interval_micros),
        });
        Ok(())
    }

    /// Stop the station's TBTT schedule.
    pub fn stop_station_tbtt<H: LowerMacHardware>(&mut self, hardware: &mut H) {
        if self.tbtt.take().is_some() {
            hardware.stop_station_tbtt();
        }
    }

    /// Report the station TBTT whose event the power interrupt delivered
    /// (`MacPowerWakeCause::StaTbtt`). Without a schedule the edge is stale
    /// and reports nothing.
    pub fn station_tbtt<H: StationTsfHardware, K: LowerMacSink<'slot, BUFFER_SIZE>>(
        &self,
        hardware: &mut H,
        sink: &mut K,
    ) {
        if let Some(tbtt) = self.tbtt {
            let now = self.station_tsf.read(hardware).as_micros();
            let at = TsfInstant::from_micros(tbtt.announced(now));
            sink.tbtt(TbttEvent {
                tbtt: VifTsf {
                    vif: VifRole::Station,
                    at,
                },
            });
        }
    }

    /// Start or stop monitor reception through the open promiscuous
    /// policy, which runs only while no interface receives.
    ///
    /// # Errors
    ///
    /// An interface receives: nothing changed.
    pub fn set_monitor<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        enabled: bool,
    ) -> Result<(), UnsupportedVif> {
        if enabled {
            if self
                .vifs
                .iter()
                .flatten()
                .any(|vif| !vif.receive.is_empty())
            {
                return Err(UnsupportedVif);
            }
            hardware.configure_open_promiscuous_receive();
        } else if self.monitor {
            hardware.disable_open_promiscuous_receive();
        }
        self.monitor = enabled;
        Ok(())
    }
}

/// MAC priority zero.
const PRIORITY_ZERO: MacPti = match MacPti::new(0) {
    Some(priority) => priority,
    None => panic!("priority zero is a MAC priority"),
};

/// A publication the transmit owner refused: its fault, and the slot that
/// goes back.
#[must_use = "retain the returned transmit slot and handle the publication fault"]
pub struct SubmitFault<'slot, const BUFFER_SIZE: usize> {
    pub fault: LowerMacFault,
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
}

/// An interface configuration the receive policies cannot take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedVif;

/// A TBTT schedule the station timer cannot hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedTbtt;

/// The access category of the queue at hardware index `index`.
fn queue_category(index: usize) -> WmmAccessCategory {
    [
        LegacyTxQueue::Voice,
        LegacyTxQueue::Video,
        LegacyTxQueue::BestEffort,
        LegacyTxQueue::Background,
    ]
    .into_iter()
    .find(|queue| usize::from(queue.hardware_index()) == index)
    .map_or(
        WmmAccessCategory::BestEffort,
        LegacyTxQueue::access_category,
    )
}

/// What every attempt shares.
struct CommonAdmission {
    queue: LegacyTxQueue,
    role: VifRole,
    rate: TxPhyRate,
    key: Option<KeyIndex>,
    hardware_key_selector: u8,
    hardware_mic_length: usize,
    /// Protection, backoff and power ceiling; `no_ack` is the MPDU's.
    single: SingleAttempt,
}

/// The scope of a checked key: its role, its kind and what places it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckedKeyScope {
    StationPairwise {
        peer: MacAddress,
    },
    StationGroup {
        key_id: oer_ieee80211_mac::ccmp::GroupKeyId,
    },
    AccessPointPairwise {
        peer: MacAddress,
        association_id: u16,
    },
    AccessPointGroup {
        key_id: oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId,
    },
}

/// The values of an MPDU attempt a caller's types guarantee: the port
/// enabled, the access category's queue free, `role` configured, `key`
/// installed for `role`, the rate carried on the channel, the power,
/// backoff and coexistence level within the capabilities, the length at
/// least the header through Address 1 and at most the capabilities'
/// `max_mpdu_length`, and `no_ack` only of an individual frame at a rate
/// that sends one without an ACK ([`LowerMacCore::submit_checked`]).
pub struct MpduValues<'slot, const BUFFER_SIZE: usize> {
    pub role: VifRole,
    pub access_category: WmmAccessCategory,
    /// The slot holding the MPDU after the TX metadata word.
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    /// The MPDU's length, without its MIC and FCS.
    pub len: usize,
    /// An individual frame sent without soliciting its ACK.
    pub no_ack: bool,
    pub rate: TxPhyRate,
    /// The rate the completion reports.
    pub reported: PhyRate,
    pub protection: Protection,
    pub key: Option<KeyIndex>,
    pub power_ceiling_dbm: Option<i8>,
    /// The backoff selected for this publication, in slots.
    pub backoff_slots: u16,
    pub coex: CoexPriority,
    /// A transmission of an MPDU the plan sends again.
    pub retry: bool,
}

/// The values of an aggregate a caller's types guarantee: `role`
/// configured, `key` installed for it, the power and backoff within the
/// capabilities ([`LowerMacCore::plan_ampdu`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduValues {
    pub role: VifRole,
    pub access_category: WmmAccessCategory,
    pub rate: TxPhyRate,
    pub protection: Protection,
    pub key: Option<KeyIndex>,
    pub power_ceiling_dbm: Option<i8>,
    /// The backoff selected for this publication, in slots.
    pub backoff_slots: u16,
    pub coex: CoexPriority,
    /// The recipient's Minimum MPDU Start Spacing encoding, 0 to 7.
    pub min_mpdu_start_spacing: u8,
}

/// Why an aggregate cannot go as one publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRefusal {
    /// A connection frame's coexistence level: an aggregate is ordinary
    /// data.
    ElevatedCoexistence,
    /// No subframe.
    Empty,
    /// More subframes than the aggregate owner holds.
    TooManySubframes,
    /// A start spacing encoding above 7.
    StartSpacing,
    /// A subframe shorter than its header through Address 1.
    ShortSubframe,
    /// The first subframe is group addressed: no one recipient answers.
    GroupRecipient,
    /// Longer than the rate's ceiling, the declared maximum or the HE TXOP
    /// limit allows, with every subframe's MIC, FCS and delimiters.
    Length,
    /// A non-HT rate: aggregates go in HT or HE PPDUs.
    LegacyRate,
}

/// An MPDU submission planned.
struct Admission {
    queue: LegacyTxQueue,
    plan: OrdinaryTxPlan,
    single: SingleAttempt,
    key: Option<KeyIndex>,
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
    /// Station policy six: the BSS-member rules.
    Station {
        bssid: MacAddress,
    },
    /// Station policy six, mode two: the BSS-member rules and management
    /// frames of every BSS.
    StationOtherBss {
        bssid: MacAddress,
    },
    AccessPointDisabled,
    /// Access-point policy eight: the BSS-member rules and the Probe
    /// Requests the access point answers, wildcard ones included.
    AccessPoint {
        address: MacAddress,
    },
}

/// Map a receive filter onto the smallest S31 receive policy that passes a
/// superset of it; [`LowerMacCore::received`] narrows the superset.
fn receive_policy(config: &LowerMacConfig, vif: &CoreVif) -> Result<ReceivePolicy, UnsupportedVif> {
    let limits = esp32s31_lower_mac_capabilities(0).receive_filters(vif.role);
    if !limits.contains(vif.receive) {
        return Err(UnsupportedVif);
    }
    match vif.role {
        VifRole::Station => {
            // The station address is published by the cold start only.
            if vif.address != config.station_address {
                return Err(UnsupportedVif);
            }
            let other_bss = vif.receive.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT);
            match vif.bssid {
                _ if vif.receive.is_empty() => Ok(ReceivePolicy::StationDisabled),
                // Outside a BSS only other BSSs' management is defined; the
                // policy matches the broadcast BSSID, as standalone ESP-NOW
                // reception does.
                None if vif.receive == ReceiveFilter::OTHER_BSS_MANAGEMENT => {
                    Ok(ReceivePolicy::StationOtherBss {
                        bssid: BROADCAST_ADDRESS,
                    })
                }
                None => Err(UnsupportedVif),
                Some(bssid) if other_bss => Ok(ReceivePolicy::StationOtherBss { bssid }),
                Some(bssid) => Ok(ReceivePolicy::Station { bssid }),
            }
        }
        VifRole::AccessPoint => {
            if vif.address[0] & 1 != 0 || vif.bssid.is_some_and(|bssid| bssid != vif.address) {
                return Err(UnsupportedVif);
            }
            if vif.receive.is_empty() {
                Ok(ReceivePolicy::AccessPointDisabled)
            } else {
                Ok(ReceivePolicy::AccessPoint {
                    address: vif.address,
                })
            }
        }
    }
}

/// How one publication ended.
fn publication_end(outcome: OrdinaryTxOutcome) -> PublicationEnd {
    match outcome {
        OrdinaryTxOutcome::Success(completed) | OrdinaryTxOutcome::HardwareFailure(completed) => {
            PublicationEnd::Completed {
                status: completed.status(),
                ack_snr_db: completed.report().status.ack_snr_db,
                block_ack: block_ack(completed.report()),
            }
        }
        OrdinaryTxOutcome::HardwareTimeout(_) => PublicationEnd::TimedOut,
        OrdinaryTxOutcome::CollisionLimit(_) => PublicationEnd::Collision,
    }
}

/// The BlockAck a BlockAckReq's completion carried.
fn block_ack(report: crate::ordinary_tx::OrdinaryTxReport) -> Option<BlockAckReport> {
    report.block_ack.map(|observation| BlockAckReport {
        start_sequence: observation.block_ack.starting_sequence,
        bitmap: observation.block_ack.bitmap,
    })
}

#[cfg(test)]
mod tests;
