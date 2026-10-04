//! Sans-IO ESP32-S31 backend of the IEEE 802.11 lower-MAC port.
//!
//! [`LowerMacCore`] holds the state the port needs between calls (interfaces,
//! installed keys, receive Block Ack banks, the transmit buffers, the
//! attempt in flight on each EDCA queue and the lifecycle state) and drives
//! the existing ESP32-S31 register seams through a caller-supplied
//! [`LowerMacHardware`]. It never waits: the runtime layer owns the event
//! queue, the interrupt entries, the publication deadlines and the
//! asynchronous PHY retune an `Enable` needs, and implements
//! `Ieee80211LowerMacPort` and the extensions the S31 has
//! (`LowerMacAmpdu`, `LowerMacBeaconTiming` for the station,
//! `LowerMacMonitor`) over this core.
//!
//! # One submission, one publication
//!
//! An admitted MPDU is published through
//! [`OrdinaryTxOwner::start_queued_single_attempt`]: exactly one descriptor
//! publication at the submitted rate with the caller's protection, backoff
//! and power ceiling. Its completion, a collision detach or the hardware
//! timeout abort ends the attempt; the owner's retry ladder, rate fallback,
//! backoff draw and BSS protection selection are not applied on this path.
//! An admitted HT A-MPDU is one aggregate publication ([`ampdu`]) whose
//! completion reports the recipient's BlockAck.
//!
//! # One attempt per EDCA queue
//!
//! The S31 MAC has four ordinary EDCA queues (voice, video, best effort,
//! background), each with its own descriptor, completion bank and latched
//! completion, timeout and collision state
//! (`pac/src/wifi/mac/tx/queue.rs`); the vendor publishes them concurrently
//! (`ppTxPkt` maps user priorities onto them) and handles their collisions
//! and timeouts per queue (`lmacProcessCollisions`,
//! `lmacProcessAllTxTimeout`). The core therefore holds one publication slot
//! per queue: an attempt occupies the queue of its access category, and a
//! second attempt for an occupied queue is `Busy`. The ordinary TX owner
//! publishes each MPDU and detaches it as a [`QueuedSingleAttempt`], and
//! each aggregate keeps its own aggregate owner, so up to four attempts are
//! in flight.
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
//! # Transmit buffers
//!
//! The port's MPDU buffers are ordinary TX slots ([`TxSlot`]): the pinned
//! descriptor and source buffer the hardware reads. A lent buffer is a whole
//! slot, the caller encodes the MPDU into its DMA buffer, and the queued
//! attempt publishes that slot, so the frame is published where it was
//! written; the slot is lent again once its attempt completes. The ordinary
//! owner keeps one idle slot of its own; `TX_BUFFERS` spare slots are lent,
//! so four of them keep every queue busy. Aggregate buffers are described in
//! [`ampdu`].
//!
//! # Limits
//!
//! The S31 draws its backoff in software, so only `Backoff::Slots` is
//! accepted. Coexistence maps only `CoexPriority::Normal`, onto the static
//! per-access-category priority of
//! [`LegacyTxQueue::vendor_data_packet_priority`]; which arbitration events
//! the other levels select is a pending policy decision. An individually
//! addressed frame without acknowledgement is published at legacy rates
//! only, the one program with a response field; hardware has not confirmed
//! its on-air behaviour. A published attempt cannot be withdrawn: the S31
//! abort path needs the queue's hardware timeout edge, so `Cancel` ends only
//! an attempt the backend still holds, and there is no
//! `LowerMacCancelPublished`. Aggregates are HT only. Access-point TBTT
//! schedules are not implemented; the qualification catalog records why.

use core::pin::Pin;

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
        ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot,
        StaPairwiseCcmpSlot, install_ap_group_ccmp, install_ap_pairwise_ccmp,
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    init::{MacSnifferHardware, StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware},
    portable,
    rx::{
        NormalizedRxFrame,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{
        HeEdcaTxopLimit, HtAmpduDensity, LegacyTxQueue, TxError, TxPhyRate, TxSlot, TxSlotState,
        ampdu::{HeAmpduPolicy, HtAmpduHardware, HtAmpduTxError},
        runtime::he_txop_limit,
    },
};
use oer_ieee80211_lower_mac::{
    AmpduBuffer, AmpduCapabilities, Backoff, BandSet, BeaconTimingCapabilities, BlockAckReport,
    CancelError, Channel, Cipher, CoexPriority, CoexPrioritySet, FailureClass, KeyHandle,
    KeyInstall, KeyScope, KeySelector, LifecycleCommand, LifecycleError, LifecycleEvent,
    LowerMacCapabilities, LowerMacSetting, MacAddress, MonitorCapabilities, MpduAttempt,
    PhyFormatSet, PhyRate, Protection, RateSupport, ReceiveFilter, Refused, RxBeaconPriority,
    RxBlockAckAgreement, RxMeta, SettingError, SubmitError, TbttEvent, TbttSchedule, TxBuffer,
    TxCompletion, TxFault, TxId, TxPayload, TxPower, TxResponse, TxStatus, VifConfig, VifId,
    VifRole, VifRoleSet, VifTsf, WidthSet,
};
use oer_ieee80211_mac::{
    channel::{Band, WifiChannel},
    management::BROADCAST_ADDRESS,
    phy::{HeMcs, HtMcs},
    qos::WmmAccessCategory,
};
use oer_ieee80211_softmac::MacTxPlan;

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

pub mod ampdu;

pub use ampdu::{
    AmpduBacking, AmpduBackingSource, ESP32S31_AMPDU_MAX_LENGTH, Esp32s31AmpduAttempt,
    Esp32s31AmpduBuffer, Esp32s31AmpduOwner, NoAmpdu, esp32s31_ampdu_capabilities,
};
use ampdu::{AmpduFormat, AmpduPlan, AmpduProgress, AmpduPublication, PublishedAmpdu};

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
const RX_BLOCK_ACK_BANKS: usize = RESOURCES.rx_block_ack_entries as usize;
/// First Frame Control byte of a BlockAckReq: control type, subtype eight.
const BLOCK_ACK_REQUEST_FRAME_CONTROL: u8 = 0x84;
/// Frame Control, Duration and Address 1: the bytes the owner reads.
const MIN_FRAME_LENGTH: usize = 10;
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

/// What the ESP32-S31 lower MAC accepts through this core, with TX slots of
/// `buffer_size` bytes.
///
/// The rates are those `TxPhyRate::try_from(PhyRate)` accepts: DSSS/CCK and
/// OFDM, HT MCS 0-7 at 20 and 40 MHz, and HE SU MCS 0-9 at 20 MHz with BCC
/// or LDPC and DCM; the DCM MCS limits are checked at submission. The
/// services are exactly the hardware-owned operations of
/// [`ESP32S31_MAC_SERVICE_CAPABILITIES`]. The longest MPDU leaves room for
/// the TX metadata word, the CCMP MIC and the FCS in the four-byte-aligned
/// descriptor capacity. Each of the four ordinary EDCA queues holds one
/// attempt. 5 GHz is absent from the ESP32-S31.
// CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-concurrent-queue-attempts
pub const fn esp32s31_lower_mac_capabilities(buffer_size: usize) -> LowerMacCapabilities {
    let usable =
        (buffer_size & !3).saturating_sub(TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE);
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
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: STATION_RECEIVE_FILTERS,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        key_slots: LOWER_MAC_KEY_SLOTS as u8,
        rx_block_ack_agreements: RESOURCES.rx_block_ack_entries,
        rx_block_ack_max_tid: RESOURCES.rx_block_ack_max_tid,
        rx_block_ack_max_window: RESOURCES.rx_block_ack_max_window,
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

/// The open promiscuous policy replaces the role receive policies, so it
/// runs only while no interface receives.
pub const ESP32S31_MONITOR_CAPABILITIES: MonitorCapabilities = MonitorCapabilities {
    with_receiving_interfaces: false,
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

/// The radio system's coexistence priority of its beacon-window event
/// (`coex_pti_get(0)`), which beacon reception asks for the air with.
pub trait BeaconWindowPriority {
    fn beacon_window_pti(&self) -> MacPti;
}

/// Every register seam the core drives; [`HtAmpduHardware`] includes the
/// ordinary queues' `TxHardware`.
pub trait LowerMacHardware:
    HtAmpduHardware
    + CcmpKeyHardware
    + RxBlockAckHardware
    + StaApRegisterHardware
    + StaLinkRxPolicyHardware
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
pub trait LowerMacSink {
    fn tx_completed(&mut self, completion: TxCompletion);
    fn lifecycle(&mut self, event: LifecycleEvent);
    fn tbtt(&mut self, event: TbttEvent);
}

/// Why the backend's state is unknown: the port is poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacFault {
    /// The ordinary TX owner failed or quarantined its descriptor.
    Tx(OrdinaryTxError),
    /// An aggregate owner failed or quarantined its descriptor chain.
    Ampdu(HtAmpduTxError),
    /// A receive Block Ack bank did not read back as programmed.
    RxBlockAckReadback,
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
    pub publication_timeout: oer_time::Duration,
    /// The epoch of the core's TSF relations, a number no other owner of a
    /// TSF relation took (`MacClockHandle::tsf_epoch`).
    pub tsf_epoch: u32,
}

/// One lent transmit buffer: a whole ordinary TX slot, whose DMA buffer
/// holds the MPDU after the TX metadata word. It goes back to the core by
/// submission or [`LowerMacCore::release_tx_buffer`]; a dropped buffer's
/// slot is lost until the core is rebuilt over new storage.
pub struct Esp32s31TxBuffer<'slot, const BUFFER_SIZE: usize> {
    slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    len: usize,
}

impl<const BUFFER_SIZE: usize> TxBuffer for Esp32s31TxBuffer<'_, BUFFER_SIZE> {
    fn len(&self) -> usize {
        self.len
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        let buffer = self
            .slot
            .as_mut()
            .buffer_mut()
            .expect("a lent slot stays free until it is submitted");
        &mut buffer[TX_METADATA_SIZE..TX_METADATA_SIZE + self.len]
    }
}

impl<const BUFFER_SIZE: usize> core::fmt::Debug for Esp32s31TxBuffer<'_, BUFFER_SIZE> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Esp32s31TxBuffer")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// A submission of the core: one MPDU in a lent slot.
pub type Esp32s31MpduAttempt<'slot, const BUFFER_SIZE: usize> =
    MpduAttempt<Esp32s31TxBuffer<'slot, BUFFER_SIZE>>;

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

/// What one queue's attempt holds.
enum Work<'slot, S: AmpduBacking, const BUFFER_SIZE: usize, const AMPDU_SLOTS: usize> {
    /// An MPDU in its lent slot, not yet published: the transmit gate is
    /// closed.
    HeldMpdu {
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        plan: OrdinaryTxPlan,
        single: SingleAttempt,
    },
    Mpdu(QueuedSingleAttempt<'slot, BUFFER_SIZE>),
    /// An aggregate in its lent buffer, not yet published.
    HeldAmpdu {
        buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>,
        plan: AmpduPlan,
    },
    Ampdu(PublishedAmpdu<'slot, S::Backing, AMPDU_SLOTS>),
}

impl<S: AmpduBacking, const BUFFER_SIZE: usize, const AMPDU_SLOTS: usize>
    Work<'_, S, BUFFER_SIZE, AMPDU_SLOTS>
{
    const fn held(&self) -> bool {
        matches!(self, Self::HeldMpdu { .. } | Self::HeldAmpdu { .. })
    }

    /// The pending deadline of a published attempt, and whether it is the
    /// end of a timeout abort's settle.
    const fn deadline(&self) -> Option<(oer_time::Instant, bool)> {
        match self {
            Self::Mpdu(queued) => Some((queued.deadline(), queued.abort_settling())),
            Self::Ampdu(published) => Some((published.deadline(), published.abort_settling())),
            Self::HeldMpdu { .. } | Self::HeldAmpdu { .. } => None,
        }
    }
}

/// The attempt occupying one queue.
struct Attempt<'slot, S: AmpduBacking, const BUFFER_SIZE: usize, const AMPDU_SLOTS: usize> {
    id: TxId,
    vif: VifId,
    key: Option<KeyHandle>,
    work: Work<'slot, S, BUFFER_SIZE, AMPDU_SLOTS>,
}

/// The running station TBTT schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StationTbtt {
    vif: VifId,
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

/// The ESP32-S31 lower-MAC state between port calls.
///
/// `S` lends the stable memory of aggregate subframes; each of the
/// `AMPDU_BUFFERS` aggregate owners carries up to `AMPDU_SLOTS` of them. A
/// core built with [`LowerMacCore::new`] has no aggregate owner.
pub struct LowerMacCore<
    'slot,
    P,
    E,
    T,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
> {
    tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    /// Idle slots the core lends; `None` while lent or in flight.
    spare: [Option<Pin<&'slot mut TxSlot<BUFFER_SIZE>>>; TX_BUFFERS],
    /// Idle aggregate owners the core lends; `None` while lent or in flight.
    ampdu_owners: [Option<Esp32s31AmpduOwner<'slot, S::Backing, AMPDU_SLOTS>>; AMPDU_BUFFERS],
    ampdu_source: Option<&'slot S>,
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
    gate_open: bool,
    monitor: bool,
    tbtt: Option<StationTbtt>,
    /// The owner of the station TSF writes and its relation.
    station_tsf: StationTsf,
    /// The relation of the access-point TSF, which only restarts.
    access_point_tsf: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf,
    /// The attempt of each ordinary queue, by its hardware index
    /// ([`LegacyTxQueue::hardware_index`]).
    queues: [Option<Attempt<'slot, S, BUFFER_SIZE, AMPDU_SLOTS>>; LOWER_MAC_TX_QUEUES],
    /// The radio system's beacon-window priority, which beacon reception
    /// asks for the air with.
    beacon_window: &'slot dyn BeaconWindowPriority,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize, const TX_BUFFERS: usize>
    LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// A disabled core without aggregates over an idle ordinary TX owner
    /// and the idle slots it lends as transmit buffers.
    /// `beacon_window` is the radio system the core shares its RF with:
    /// beacon reception asks for the air at its beacon-window priority.
    pub fn new(
        tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        spare: [Pin<&'slot mut TxSlot<BUFFER_SIZE>>; TX_BUFFERS],
        config: LowerMacConfig,
        beacon_window: &'slot dyn BeaconWindowPriority,
    ) -> Self {
        Self::build(tx, spare, [], None, config, beacon_window)
    }
}

impl<
    'slot,
    P,
    E,
    T,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    S: AmpduBacking,
{
    fn build(
        tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        spare: [Pin<&'slot mut TxSlot<BUFFER_SIZE>>; TX_BUFFERS],
        ampdu: [Esp32s31AmpduOwner<'slot, S::Backing, AMPDU_SLOTS>; AMPDU_BUFFERS],
        ampdu_source: Option<&'slot S>,
        config: LowerMacConfig,
        beacon_window: &'slot dyn BeaconWindowPriority,
    ) -> Self {
        Self {
            tx,
            spare: spare.map(Some),
            ampdu_owners: ampdu.map(Some),
            ampdu_source,
            config,
            state: PortState::Disabled,
            quiesce_pending: false,
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
            beacon_window,
        }
    }

    pub const fn capabilities(&self) -> LowerMacCapabilities {
        esp32s31_lower_mac_capabilities(BUFFER_SIZE)
    }

    pub const fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(AMPDU_SLOTS)
    }

    /// The channel received frames are reported on.
    pub fn channel(&self) -> Channel {
        Channel::from_wifi_channel(self.channel)
    }

    /// The radio clock: the ordinary TX owner's timer.
    pub fn now(&self) -> oer_time::Instant {
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

    fn attempts(&self) -> impl Iterator<Item = &Attempt<'slot, S, BUFFER_SIZE, AMPDU_SLOTS>> {
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

    /// Lend an idle slot for an MPDU of `len` bytes.
    pub fn tx_buffer(&mut self, len: usize) -> Option<Esp32s31TxBuffer<'slot, BUFFER_SIZE>> {
        if len > usize::from(self.capabilities().max_mpdu_length) {
            return None;
        }
        let spare = self.spare.iter_mut().find(|slot| {
            slot.as_ref()
                .is_some_and(|slot| slot.state() == TxSlotState::Free)
        })?;
        let slot = spare.take()?;
        Some(Esp32s31TxBuffer { slot, len })
    }

    /// Take back a lent slot.
    pub fn release_tx_buffer(&mut self, buffer: Esp32s31TxBuffer<'slot, BUFFER_SIZE>) {
        self.return_slot(buffer.slot);
    }

    fn return_slot(&mut self, slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>) {
        // Every slot the core lends has a free place: the count of places
        // equals the count of lendable slots.
        if let Some(place) = self.spare.iter_mut().find(|place| place.is_none()) {
            *place = Some(slot);
        }
    }

    /// Take back a lent aggregate: its subframes return to their source.
    pub fn release_ampdu_buffer(&mut self, buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>) {
        self.return_ampdu_owner(buffer.into_owner());
    }

    fn return_ampdu_owner(&mut self, owner: Esp32s31AmpduOwner<'slot, S::Backing, AMPDU_SLOTS>) {
        if let Some(place) = self.ampdu_owners.iter_mut().find(|place| place.is_none()) {
            *place = Some(owner);
        }
    }

    /// Admit one attempt. A refused attempt comes back with its buffer.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the refusal hands the caller's attempt back by value"
    )]
    pub fn submit<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        mut attempt: Esp32s31MpduAttempt<'slot, BUFFER_SIZE>,
    ) -> Result<Result<(), Refused<Esp32s31MpduAttempt<'slot, BUFFER_SIZE>>>, LowerMacFault> {
        let admission = match self.admit(&mut attempt) {
            Ok(admission) => admission,
            Err(error) => return Ok(Err(Refused { error, attempt })),
        };
        let MpduAttempt {
            id,
            vif,
            access_category,
            payload:
                TxPayload {
                    frame: Esp32s31TxBuffer { slot, len },
                    response,
                },
            rate,
            protection,
            key,
            power,
            backoff,
            coex,
        } = attempt;
        let work = if self.gate_open {
            // Publish from the caller's slot.
            match self.tx.start_queued_single_attempt(
                hardware,
                slot,
                admission.plan,
                admission.single,
            ) {
                Ok(queued) => Work::Mpdu(queued),
                Err(QueuedSingleAttemptRefused {
                    error:
                        OrdinaryTxError::BufferSizeOverflow | OrdinaryTxError::Tx(TxError::Invalid),
                    slot,
                }) => {
                    // Nothing was reserved: hand the caller's slot back.
                    return Ok(Err(Refused {
                        error: SubmitError::InvalidLength,
                        attempt: MpduAttempt {
                            id,
                            vif,
                            access_category,
                            payload: TxPayload {
                                frame: Esp32s31TxBuffer { slot, len },
                                response,
                            },
                            rate,
                            protection,
                            key,
                            power,
                            backoff,
                            coex,
                        },
                    }));
                }
                Err(QueuedSingleAttemptRefused { error, slot }) => {
                    self.return_slot(slot);
                    return Err(LowerMacFault::Tx(error));
                }
            }
        } else {
            Work::HeldMpdu {
                slot,
                plan: admission.plan,
                single: admission.single,
            }
        };
        self.queues[usize::from(admission.queue.hardware_index())] = Some(Attempt {
            id,
            vif,
            key: admission.key,
            work,
        });
        Ok(Ok(()))
    }

    /// Check what every attempt shares against the port's state and limits:
    /// identity, queue, interface, rate, power, backoff, coexistence and key.
    #[allow(
        clippy::too_many_arguments,
        reason = "the fields every attempt shares, borrowed from either payload"
    )]
    fn admit_common(
        &self,
        id: TxId,
        vif: VifId,
        access_category: WmmAccessCategory,
        rate: PhyRate,
        protection: Protection,
        key: KeySelector,
        power: TxPower,
        backoff: Backoff,
        coex: CoexPriority,
    ) -> Result<CommonAdmission, SubmitError> {
        if self.state != PortState::Enabled {
            return Err(SubmitError::Disabled);
        }
        if self.attempts().any(|active| active.id == id) {
            return Err(SubmitError::DuplicateId);
        }
        let queue = LegacyTxQueue::from_access_category(access_category);
        if self.queues[usize::from(queue.hardware_index())].is_some() {
            return Err(SubmitError::Busy);
        }
        let vif_config = self.vif(vif).copied().ok_or(SubmitError::UnknownVif)?;
        let tx_rate = TxPhyRate::try_from(rate).map_err(|_| SubmitError::UnsupportedRate)?;
        let capabilities = self.capabilities();
        if !capabilities.supports_rate(rate, self.channel()) {
            return Err(SubmitError::UnsupportedRate);
        }
        let power_ceiling_dbm = match power {
            TxPower::Calibrated => None,
            TxPower::MaxDbm(dbm)
                if capabilities
                    .tx_power_ceiling_min_dbm
                    .is_some_and(|floor| dbm >= floor) =>
            {
                Some(dbm)
            }
            TxPower::MaxDbm(_) => return Err(SubmitError::Unsupported),
        };
        let backoff_slots = match backoff {
            Backoff::Slots(slots) if capabilities.supports_backoff(backoff) => slots,
            Backoff::Slots(_) | Backoff::HardwareDraw { .. } => {
                return Err(SubmitError::Unsupported);
            }
        };
        if !capabilities.coex_priorities.contains(coex) {
            return Err(SubmitError::Unsupported);
        }
        let (key, hardware_key_selector, hardware_mic_length) = match key {
            KeySelector::Plaintext => (None, 0, 0),
            KeySelector::Key(handle) => match self.key(handle) {
                Some(installed) if installed.vif == vif => (
                    Some(handle),
                    installed.token.hardware_index(),
                    TX_CCMP_MIC_SIZE,
                ),
                _ => return Err(SubmitError::UnknownKey),
            },
        };
        Ok(CommonAdmission {
            queue,
            role: vif_config.role,
            rate: tx_rate,
            key,
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
        })
    }

    /// Check one MPDU attempt against the port's state and limits and plan
    /// its publication.
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-individual-no-ack, wifi-interfaces-and-operating-modes-lower-mac-port-coexistence-levels
    fn admit(
        &self,
        attempt: &mut Esp32s31MpduAttempt<'slot, BUFFER_SIZE>,
    ) -> Result<Admission, SubmitError> {
        let common = self.admit_common(
            attempt.id,
            attempt.vif,
            attempt.access_category,
            attempt.rate,
            attempt.protection,
            attempt.key,
            attempt.power,
            attempt.backoff,
            attempt.coex,
        )?;
        let capabilities = self.capabilities();
        let length = attempt.payload.frame.len();
        if length < MIN_FRAME_LENGTH
            || TX_METADATA_SIZE + length + common.hardware_mic_length + TX_FCS_SIZE > BUFFER_SIZE
        {
            return Err(SubmitError::InvalidLength);
        }
        let frame = attempt.payload.frame.frame_mut();
        let group = frame[4] & 1 != 0;
        let block_ack_request = frame[0] == BLOCK_ACK_REQUEST_FRAME_CONTROL;
        // The owner derives the solicited response from the frame; a
        // request it cannot express is refused rather than sent with
        // another response.
        let no_ack = match (attempt.payload.response, group, block_ack_request) {
            (TxResponse::None, true, _) => false,
            (TxResponse::BlockAck, false, true) => {
                if !matches!(common.rate, TxPhyRate::Legacy(_)) {
                    return Err(SubmitError::UnsupportedRate);
                }
                false
            }
            (TxResponse::Ack, false, false) => false,
            (TxResponse::None, false, false)
                if capabilities.individual_no_ack.contains_rate(attempt.rate) =>
            {
                true
            }
            _ => return Err(SubmitError::Unsupported),
        };

        let queue = common.queue;
        let plan = OrdinaryTxPlan {
            frame_length: length,
            descriptor_capacity: None,
            exchange: MacTxPlan {
                access_category: attempt.access_category,
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
            // `CoexPriority::Normal`: the static priority vendor data
            // encapsulation assigns the access category.
            scheduler_priority: queue.vendor_data_scheduler_priority(),
            packet_priority: queue.vendor_data_packet_priority(),
            priority_count: 1,
        };
        Ok(Admission {
            queue,
            plan,
            single: SingleAttempt {
                no_ack,
                ..common.single
            },
            key: common.key,
        })
    }

    fn key(&self, handle: KeyHandle) -> Option<&InstalledKey> {
        self.keys
            .get(usize::from(handle.0))
            .and_then(Option::as_ref)
    }

    /// Consume one interrupt or deadline edge of the published attempts.
    ///
    /// Every published queue is offered the edge and claims only its own
    /// queue's state. When a timeout abort ends, the CCA it forced is
    /// released, and every queue is offered a timeout edge once more: one
    /// whose timeout arrived during the settle aborts now.
    pub fn service<H: HtAmpduHardware, K: LowerMacSink>(
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
                let Attempt { id, vif, key, work } = attempt;
                let (work, ended) = match work {
                    held @ (Work::HeldMpdu { .. } | Work::HeldAmpdu { .. }) => (Some(held), None),
                    Work::Mpdu(queued) => {
                        let settling = queued.abort_settling();
                        match self
                            .tx
                            .service_queued_single_attempt(
                                hardware,
                                queued,
                                wake,
                                may_begin_timeout_abort,
                            )
                            .map_err(LowerMacFault::Tx)?
                        {
                            QueuedSingleAttemptProgress::Pending(queued) => {
                                (Some(Work::Mpdu(queued)), None)
                            }
                            QueuedSingleAttemptProgress::Complete { outcome, slot } => {
                                self.return_slot(slot);
                                (None, Some((completion(id, outcome), settling)))
                            }
                        }
                    }
                    Work::Ampdu(published) => {
                        let settling = published.abort_settling();
                        let now = self.tx.now();
                        match ampdu::service(
                            hardware,
                            published,
                            wake,
                            may_begin_timeout_abort,
                            now,
                        )
                        .map_err(LowerMacFault::Ampdu)?
                        {
                            AmpduProgress::Pending(published) => {
                                (Some(Work::Ampdu(published)), None)
                            }
                            AmpduProgress::Complete { outcome, owner } => {
                                self.return_ampdu_owner(owner);
                                (None, Some((outcome.completion(id), settling)))
                            }
                        }
                    }
                };
                if let Some(work) = work {
                    self.queues[index] = Some(Attempt { id, vif, key, work });
                }
                if let Some((completion, settled)) = ended {
                    abort_ended |= settled;
                    sink.tx_completed(completion);
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

    /// Finish lifecycle transitions that waited for the attempts.
    fn settle<K: LowerMacSink>(&mut self, sink: &mut K) {
        if self.attempts().next().is_some() {
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
    /// not receive or no receive rule admits it. The hardware policies pass
    /// a superset of the requested rules; this narrows it to them.
    pub fn received<'frame>(
        &self,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], RxMeta)> {
        let admitted = self.monitor || self.vifs.iter().flatten().any(|vif| vif.admits(frame.mpdu));
        (self.receiving() && admitted).then(|| {
            (
                frame.mpdu,
                portable::rx_meta(frame.metadata, self.channel()),
            )
        })
    }

    /// Start one lifecycle command.
    pub fn lifecycle<K: LowerMacSink>(
        &mut self,
        command: LifecycleCommand,
        sink: &mut K,
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
                PortState::Enabling | PortState::Enabled => Err(LifecycleError::AlreadyInState),
                PortState::Quiescing | PortState::Disabling => Err(LifecycleError::Busy),
            },
            LifecycleCommand::Quiesce => match self.state {
                PortState::Enabled => {
                    self.state = PortState::Quiescing;
                    self.settle(sink);
                    Ok(LifecycleStart::Admitted)
                }
                PortState::Quiescing | PortState::Quiesced => Err(LifecycleError::AlreadyInState),
                PortState::Disabled | PortState::Enabling | PortState::Disabling => {
                    Err(LifecycleError::InvalidState)
                }
            },
            LifecycleCommand::Disable => match self.state {
                PortState::Enabled | PortState::Quiescing | PortState::Quiesced => {
                    self.quiesce_pending = self.state == PortState::Quiescing;
                    self.state = PortState::Disabling;
                    for index in 0..LOWER_MAC_TX_QUEUES {
                        self.abort_held(index, sink);
                    }
                    self.settle(sink);
                    Ok(LifecycleStart::Admitted)
                }
                PortState::Disabled | PortState::Disabling => Err(LifecycleError::AlreadyInState),
                PortState::Enabling => Err(LifecycleError::Busy),
            },
        }
    }

    /// End one admitted attempt: a held one at once as
    /// [`TxStatus::Aborted`], a published one with its own completion.
    pub fn cancel<K: LowerMacSink>(&mut self, id: TxId, sink: &mut K) -> Result<(), CancelError> {
        let Some(index) = self
            .queues
            .iter()
            .position(|attempt| attempt.as_ref().is_some_and(|attempt| attempt.id == id))
        else {
            return Err(CancelError::NotRunning);
        };
        // A published descriptor has no software abort on the S31: it ends
        // with its own completion.
        self.abort_held(index, sink);
        self.settle(sink);
        Ok(())
    }

    /// End a queue's held attempt with [`TxStatus::Aborted`]: it never
    /// reached the hardware, and its slot or aggregate goes back to the
    /// core.
    fn abort_held<K: LowerMacSink>(&mut self, index: usize, sink: &mut K) {
        if !self.queues[index]
            .as_ref()
            .is_some_and(|attempt| attempt.work.held())
        {
            return;
        }
        let Some(attempt) = self.queues[index].take() else {
            return;
        };
        match attempt.work {
            Work::HeldMpdu { slot, .. } => self.return_slot(slot),
            Work::HeldAmpdu { buffer, .. } => self.release_ampdu_buffer(buffer),
            Work::Mpdu(_) | Work::Ampdu(_) => unreachable!("only a held attempt is aborted"),
        }
        sink.tx_completed(TxCompletion {
            id: attempt.id,
            status: TxStatus::Aborted,
            ack_rssi_dbm: None,
            ack_snr_db: None,
            block_ack: None,
        });
    }

    /// Complete an `Enable` after the runtime retuned the radio. A refused
    /// retune fails the command recoverably: the port stays disabled on the
    /// channel it was tuned to.
    pub fn finish_retune<K: LowerMacSink>(&mut self, retuned: bool, sink: &mut K) {
        if self.state != PortState::Enabling {
            return;
        }
        if !retuned {
            self.state = PortState::Disabled;
            sink.lifecycle(LifecycleEvent::Failed {
                command: LifecycleCommand::Enable,
                class: FailureClass::Recoverable,
            });
            return;
        }
        self.tuned = self.channel;
        self.state = PortState::Enabled;
        sink.lifecycle(LifecycleEvent::Enabled);
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
            LowerMacSetting::TxGate { open } => self.set_tx_gate(hardware, open),
            // The policy reads each attempt's AIFSN from the set; it
            // validates all four records before installing any.
            LowerMacSetting::Edca(parameters) => Ok(self
                .tx
                .policy_mut()
                .install_wmm(parameters)
                .map_err(|_| SettingError::Unsupported)),
            LowerMacSetting::RxBeaconPriority(priority) => {
                Ok(self.set_rx_beacon_priority(hardware, priority))
            }
            LowerMacSetting::HeBssColor { vif, color } => Ok(self.set_he_bss_color(vif, color)),
        }
    }

    /// The station's HE BSS color, which every HE PPDU it sends carries:
    /// the access point has no HE on the S31.
    fn set_he_bss_color(&mut self, vif: VifId, color: u8) -> Result<(), SettingError> {
        match self.vifs.get(usize::from(vif.0)).copied().flatten() {
            Some(config) if config.role == VifRole::Station => {}
            Some(_) => return Err(SettingError::UnsupportedRole),
            None => return Err(SettingError::UnknownVif),
        }
        if color > 63 {
            return Err(SettingError::Unsupported);
        }
        self.tx.policy_mut().install_he_bss_color(color);
        Ok(())
    }

    // SOURCE(esp32s31): complete pinned `libpp.a[pm_coex.o]::pm_coex_update_rx_beacon_pti`
    // passes `coex_pti_get(0)`, or zero, as both arguments of
    // `hal_set_rx_beacon_pti`.
    fn set_rx_beacon_priority<H: RxBeaconPriorityHardware>(
        &mut self,
        hardware: &mut H,
        priority: RxBeaconPriority,
    ) -> Result<(), SettingError> {
        match priority {
            RxBeaconPriority::BeaconWindow => {
                hardware.set_rx_beacon_pti(self.beacon_window.beacon_window_pti());
            }
            RxBeaconPriority::Zero => hardware
                .set_rx_beacon_pti(MacPti::new(0).expect("priority zero is a valid MAC priority")),
            RxBeaconPriority::Cleared => hardware.clear_rx_beacon_pti(),
        }
        Ok(())
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
        if self.channel != channel {
            self.break_tsf_relation(VifRole::Station);
            self.break_tsf_relation(VifRole::AccessPoint);
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
        if self.attempts().any(|attempt| attempt.vif == vif) {
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
        // The open promiscuous policy replaces the role policies.
        if self.monitor && !config.receive.is_empty() {
            return Err(SettingError::Unsupported);
        }
        let policy = receive_policy(&self.config, &config)?;
        if self.vifs[index].is_some_and(|previous| previous.role != config.role) {
            self.remove_vif(hardware, vif);
        }
        match policy {
            ReceivePolicy::StationDisabled => hardware.disable_station_receive_registers(),
            ReceivePolicy::Station { bssid } => hardware.apply_sta_link_policy(bssid),
            ReceivePolicy::StationOtherBss { bssid } => hardware.apply_sta_esp_now_policy(bssid),
            ReceivePolicy::AccessPointDisabled => hardware.disable_ap_link_policy(),
            ReceivePolicy::AccessPoint { address } => hardware.apply_ap_link_policy(address),
        }
        self.vifs[index] = Some(config);
        // A new association, a reassociation or a roam: the interface
        // follows another TSF.
        self.break_tsf_relation(config.role);
        Ok(())
    }

    /// Close the interface's receive context and clear its keys, receive
    /// Block Ack banks and TBTT schedule.
    fn remove_vif<H: LowerMacHardware>(&mut self, hardware: &mut H, vif: VifId) {
        let Some(previous) = self.vifs[usize::from(vif.0)].take() else {
            return;
        };
        self.break_tsf_relation(previous.role);
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
        if self.tbtt.is_some_and(|tbtt| tbtt.vif == vif) {
            self.tbtt = None;
            hardware.stop_station_tbtt();
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
        if self.attempts().any(|attempt| attempt.key == Some(handle)) {
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

    /// Open or close the transmit gate: the power-save block of every
    /// ordinary queue. Closing it while an attempt is published is refused
    /// (`Busy`), because the blocked queue would end that attempt with a
    /// hardware timeout; an attempt admitted behind the closed gate is held
    /// in its lent slot or aggregate and published when the gate opens.
    fn set_tx_gate<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        open: bool,
    ) -> Result<Result<(), SettingError>, LowerMacFault> {
        if !open {
            if self.attempts().any(|attempt| !attempt.work.held()) {
                return Ok(Err(SettingError::Busy));
            }
            hardware.set_power_save_tx_block(true);
            self.gate_open = false;
            return Ok(Ok(()));
        }
        hardware.set_power_save_tx_block(false);
        self.gate_open = true;
        for index in 0..LOWER_MAC_TX_QUEUES {
            let Some(attempt) = self.queues[index].take() else {
                continue;
            };
            let Attempt { id, vif, key, work } = attempt;
            let work = match work {
                Work::HeldMpdu { slot, plan, single } => {
                    match self
                        .tx
                        .start_queued_single_attempt(hardware, slot, plan, single)
                    {
                        Ok(queued) => Work::Mpdu(queued),
                        Err(QueuedSingleAttemptRefused { error, slot }) => {
                            self.return_slot(slot);
                            return Err(LowerMacFault::Tx(error));
                        }
                    }
                }
                Work::HeldAmpdu { buffer, plan } => {
                    Work::Ampdu(self.publish_ampdu(hardware, buffer, plan)?)
                }
                published @ (Work::Mpdu(_) | Work::Ampdu(_)) => published,
            };
            self.queues[index] = Some(Attempt { id, vif, key, work });
        }
        Ok(Ok(()))
    }

    fn publish_ampdu<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>,
        plan: AmpduPlan,
    ) -> Result<PublishedAmpdu<'slot, S::Backing, AMPDU_SLOTS>, LowerMacFault> {
        ampdu::publish(
            hardware,
            buffer,
            plan,
            self.tx.now(),
            self.config.publication_timeout,
        )
        .map_err(LowerMacFault::Ampdu)
    }

    /// Read an interface's TSF: the station timer; the access-point timer
    /// has no read seam.
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-access-point-tsf
    pub fn tsf<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
        vif: VifId,
    ) -> Result<VifTsf, SettingError> {
        self.tsf_reading(hardware, vif).map(|(tsf, _)| tsf)
    }

    /// An interface's TSF and the generation of its relation, for a
    /// [`TsfSample`](oer_ieee80211_lower_mac::TsfSample) the backend pairs
    /// with its radio clock.
    pub fn tsf_reading<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
        vif: VifId,
    ) -> Result<(VifTsf, TsfGeneration), SettingError> {
        let role = self.vif(vif).ok_or(SettingError::UnknownVif)?.role;
        if !ESP32S31_BEACON_TIMING_CAPABILITIES.tsf_read.contains(role) {
            return Err(SettingError::Unsupported);
        }
        let at = self.station_tsf.read(hardware);
        Ok((VifTsf::new(vif, at), self.station_tsf.generation()))
    }

    /// Start a new generation of the TSF relation of the interface with
    /// `role`.
    fn break_tsf_relation(&mut self, role: VifRole) {
        match role {
            VifRole::Station => self.station_tsf.break_relation(),
            VifRole::AccessPoint => self.access_point_tsf.break_relation(),
        }
    }

    /// Set an interface's TSF: the station timer to any value, the
    /// access-point timer only back to zero through its reset seam. A
    /// station set beyond the drift its relation allows, and an access-point
    /// restart, start a new generation of the relation.
    pub fn set_tsf<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        tsf: VifTsf,
    ) -> Result<(), SettingError> {
        match self.vif(tsf.vif).ok_or(SettingError::UnknownVif)?.role {
            VifRole::Station => {
                self.station_tsf.set(hardware, tsf.at);
            }
            VifRole::AccessPoint if tsf.at.as_micros() == 0 => {
                self.access_point_tsf.restart(hardware);
            }
            VifRole::AccessPoint => return Err(SettingError::Unsupported),
        }
        Ok(())
    }

    /// Program the station TBTT schedule. The event fires the schedule's
    /// lead before each TBTT; the wake lead published beside it keeps the
    /// vendor's window above the lead.
    pub fn set_tbtt<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        schedule: TbttSchedule,
    ) -> Result<(), SettingError> {
        let vif = schedule.next.vif;
        self.tbtt_role(vif)?;
        let interval_micros = u32::try_from(schedule.beacon_interval.as_micros())
            .ok()
            .filter(|&interval| interval != 0)
            .ok_or(SettingError::Unsupported)?;
        let ahead_micros =
            u16::try_from(schedule.lead.as_micros()).map_err(|_| SettingError::Unsupported)?;
        let wake_ahead_micros = ahead_micros
            .checked_add(STATION_TBTT_WAKE_WINDOW_MICROS)
            .ok_or(SettingError::Unsupported)?;
        let first = schedule.next.at.as_micros();
        hardware.start_station_tbtt(StaTbttSchedule {
            first_tbtt_tsf: first,
            interval_micros,
            ahead_micros,
            wake_ahead_micros,
        });
        self.tbtt = Some(StationTbtt {
            vif,
            first,
            interval: u64::from(interval_micros),
        });
        Ok(())
    }

    /// Stop an interface's station TBTT schedule.
    pub fn stop_tbtt<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        vif: VifId,
    ) -> Result<(), SettingError> {
        self.tbtt_role(vif)?;
        if self.tbtt.is_some_and(|tbtt| tbtt.vif == vif) {
            self.tbtt = None;
            hardware.stop_station_tbtt();
        }
        Ok(())
    }

    /// Whether `vif` is configured with a role whose TBTT schedule the
    /// backend programs.
    fn tbtt_role(&self, vif: VifId) -> Result<(), SettingError> {
        let role = self.vif(vif).ok_or(SettingError::UnknownVif)?.role;
        if !ESP32S31_BEACON_TIMING_CAPABILITIES.tbtt.contains(role) {
            return Err(SettingError::Unsupported);
        }
        Ok(())
    }

    /// Report the station TBTT whose event the power interrupt delivered
    /// (`MacPowerWakeCause::StaTbtt`). Without a schedule the edge is stale
    /// and reports nothing.
    pub fn station_tbtt<H: StationTsfHardware, K: LowerMacSink>(
        &self,
        hardware: &mut H,
        sink: &mut K,
    ) {
        if let Some(tbtt) = self.tbtt {
            let now = self.station_tsf.read(hardware).as_micros();
            let at = TsfInstant::from_micros(tbtt.announced(now));
            sink.tbtt(TbttEvent {
                tbtt: VifTsf::new(tbtt.vif, at),
            });
        }
    }

    /// Start or stop monitor reception through the open promiscuous
    /// policy. It runs only while no interface receives.
    pub fn set_monitor<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        enabled: bool,
    ) -> Result<(), SettingError> {
        if enabled {
            if self
                .vifs
                .iter()
                .flatten()
                .any(|vif| !vif.receive.is_empty())
            {
                return Err(SettingError::Unsupported);
            }
            hardware.configure_open_promiscuous_receive();
        } else if self.monitor {
            hardware.disable_open_promiscuous_receive();
        }
        self.monitor = enabled;
        Ok(())
    }
}

impl<
    'slot,
    P,
    E,
    T,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    S: AmpduBackingSource,
{
    /// A disabled core that also lends `ampdu` idle aggregate owners, whose
    /// subframes `source` backs. `beacon_window` is as for [`Self::new`].
    pub fn with_ampdu(
        tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        spare: [Pin<&'slot mut TxSlot<BUFFER_SIZE>>; TX_BUFFERS],
        ampdu: [Esp32s31AmpduOwner<'slot, S::Backing, AMPDU_SLOTS>; AMPDU_BUFFERS],
        source: &'slot S,
        config: LowerMacConfig,
        beacon_window: &'slot dyn BeaconWindowPriority,
    ) -> Self {
        Self::build(tx, spare, ampdu, Some(source), config, beacon_window)
    }

    /// Lend an idle aggregate owner; `None` without aggregate owners or
    /// while every one is lent or in flight.
    pub fn ampdu_buffer(&mut self) -> Option<Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>> {
        let source = self.ampdu_source?;
        let place = self.ampdu_owners.iter_mut().find(|owner| owner.is_some())?;
        let mut owner = place.take()?;
        // The owner is idle here, so the limit always applies.
        if owner
            .configure_max_aggregate_bytes(ESP32S31_AMPDU_MAX_LENGTH)
            .is_err()
        {
            *place = Some(owner);
            return None;
        }
        Some(Esp32s31AmpduBuffer::new(owner, source))
    }

    /// Admit one HT A-MPDU attempt. A refused attempt comes back with its
    /// aggregate untouched.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the refusal hands the caller's attempt back by value"
    )]
    // CAPABILITY: wifi-interfaces-and-operating-modes-lower-mac-port-a-mpdu
    pub fn submit_ampdu<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        mut attempt: Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>,
    ) -> Result<Result<(), Refused<Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>>>, LowerMacFault>
    {
        let plan = match self.admit_ampdu(&mut attempt) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(Refused { error, attempt })),
        };
        let key = match attempt.key {
            KeySelector::Plaintext => None,
            KeySelector::Key(handle) => Some(handle),
        };
        let buffer = attempt.payload.subframes;
        let work = if self.gate_open {
            Work::Ampdu(self.publish_ampdu(hardware, buffer, plan)?)
        } else {
            Work::HeldAmpdu { buffer, plan }
        };
        self.queues[usize::from(plan.queue.hardware_index())] = Some(Attempt {
            id: attempt.id,
            vif: attempt.vif,
            key,
            work,
        });
        Ok(Ok(()))
    }

    /// Check one aggregate against the port's state and limits and plan
    /// its publication. Every check the aggregate owner would make at
    /// commit is made here first, so a refusal hands the subframes back.
    fn admit_ampdu(
        &self,
        attempt: &mut Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>,
    ) -> Result<AmpduPlan, SubmitError> {
        let common = self.admit_common(
            attempt.id,
            attempt.vif,
            attempt.access_category,
            attempt.rate,
            attempt.protection,
            attempt.key,
            attempt.power,
            attempt.backoff,
            attempt.coex,
        )?;
        let capabilities = self.ampdu_capabilities();
        let buffer = &mut attempt.payload.subframes;
        let count = buffer.subframes();
        if count == 0 {
            return Err(SubmitError::InvalidLength);
        }
        if count > usize::from(capabilities.max_subframes)
            || attempt.payload.min_mpdu_start_spacing > 7
        {
            return Err(SubmitError::Unsupported);
        }
        if buffer.lengths().any(|len| len < MIN_FRAME_LENGTH) {
            return Err(SubmitError::InvalidLength);
        }
        // One recipient answers the aggregate with its BlockAck.
        if buffer.first_mpdu().is_none_or(|first| first[4] & 1 != 0) {
            return Err(SubmitError::Unsupported);
        }

        let queue = common.queue;
        let ceiling = common.single.power_ceiling_dbm;
        let txop_limit = self
            .tx
            .policy()
            .access_policy(queue)
            .txop_limit_units_32_us();
        let density = HtAmpduDensity::from_ampdu_parameters(
            (attempt.payload.min_mpdu_start_spacing & 0x07) << 2,
        );
        let psdu_lengths = buffer
            .lengths()
            .map(|len| len + common.hardware_mic_length + TX_FCS_SIZE);
        let (power_code, format) = match common.rate {
            TxPhyRate::Ht(rate) => {
                // The access category's TXOP limit is the caller's to keep.
                // The owner's own length rule: the rate's ceiling and the
                // declared maximum it was configured with when lent.
                let mut budget = buffer
                    .owner
                    .ht_length_budget(rate)
                    .map_err(|_| SubmitError::Unsupported)?;
                for psdu in psdu_lengths {
                    let psdu = u32::try_from(psdu).map_err(|_| SubmitError::Unsupported)?;
                    budget.push(psdu, 0).map_err(|_| SubmitError::Unsupported)?;
                }
                (rate.power_lookup_code(), None)
            }
            TxPhyRate::He(rate) => {
                // The HE program carries the access category's advertised
                // TXOP limit; the core sets no ceiling of its own.
                let txop = he_txop_limit(txop_limit, HeEdcaTxopLimit::DEFAULT)
                    .map_err(|_| SubmitError::Unsupported)?;
                let policy = HeAmpduPolicy::new(rate, density, txop);
                let mut budget = buffer
                    .owner
                    .he_length_budget(policy)
                    .map_err(|_| SubmitError::Unsupported)?;
                for psdu in psdu_lengths {
                    let psdu = u16::try_from(psdu).map_err(|_| SubmitError::Unsupported)?;
                    let delimiters = rate
                        .ampdu_empty_delimiters(psdu, density)
                        .ok_or(SubmitError::Unsupported)?;
                    budget
                        .push(u32::from(psdu), delimiters)
                        .map_err(|_| SubmitError::Unsupported)?;
                }
                (rate.power_lookup_code(), Some(policy))
            }
            TxPhyRate::Legacy(_) => return Err(SubmitError::Unsupported),
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
                protection_spacing: ampdu::protection_spacing(
                    attempt.payload.min_mpdu_start_spacing,
                ),
            },
            (_, Some(policy)) => AmpduFormat::He {
                policy,
                bss_color: self.tx.policy().he_bss_color(),
            },
            (_, None) => return Err(SubmitError::Unsupported),
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
}

/// What every admitted attempt shares.
struct CommonAdmission {
    queue: LegacyTxQueue,
    role: VifRole,
    rate: TxPhyRate,
    key: Option<KeyHandle>,
    hardware_key_selector: u8,
    hardware_mic_length: usize,
    /// Protection, backoff and power ceiling; `no_ack` is the MPDU's.
    single: SingleAttempt,
}

/// An MPDU submission that passed admission.
struct Admission {
    queue: LegacyTxQueue,
    plan: OrdinaryTxPlan,
    single: SingleAttempt,
    key: Option<KeyHandle>,
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
    /// Access-point policy eight: the BSS-member rules.
    AccessPoint {
        address: MacAddress,
    },
}

/// Map a receive filter onto the smallest S31 receive policy that passes a
/// superset of it; [`LowerMacCore::received`] narrows the superset.
fn receive_policy(config: &LowerMacConfig, vif: &VifConfig) -> Result<ReceivePolicy, SettingError> {
    let limits = esp32s31_lower_mac_capabilities(0).receive_filters(vif.role);
    if !limits.contains(vif.receive) {
        return Err(SettingError::Unsupported);
    }
    match vif.role {
        VifRole::Station => {
            // The station address is published by the cold start only.
            if vif.address != config.station_address {
                return Err(SettingError::Unsupported);
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
                None => Err(SettingError::Unsupported),
                Some(bssid) if other_bss => Ok(ReceivePolicy::StationOtherBss { bssid }),
                Some(bssid) => Ok(ReceivePolicy::Station { bssid }),
            }
        }
        VifRole::AccessPoint => {
            if vif.address[0] & 1 != 0 || vif.bssid.is_some_and(|bssid| bssid != vif.address) {
                return Err(SettingError::Unsupported);
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
