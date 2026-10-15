use core::{cell::RefCell, pin::Pin};
use std::{boxed::Box, vec::Vec};

use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacLegacyTxResponse, MacStaApReceivePlan,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxProtection,
    MacTxQueueDetached,
};
use oer_esp32s31_ieee80211_mac::{
    irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT},
    rx::RxPhyInfo,
    tx::{
        HardwareOwnedTxDma, PreparedTxDma, TxHardware,
        ampdu::{HtAmpduTxStorage, RetainedAmpduDmaStorage, RetainedDmaAmpduTx},
        runtime::WifiTxRuntimePolicy,
    },
};
use oer_ieee80211_lower_mac::{HardwareServices, PhyRate, RxEvidence};
use oer_ieee80211_mac::tsf::TsfInstant;
use oer_ieee80211_mac::{
    channel::ChannelWidth,
    phy::{self, FecCoding, HeGiLtf, LegacyRate, PpduBandwidth, SpatialStreams},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

use oer_memory::{
    DmaIndexReturn, PinnedDmaTxPool, PinnedDmaTxRadioLease, ReturningStableDmaBacking,
};

use super::*;
use crate::ordinary_tx::{WifiTxPowerPair, WifiTxResources};
use crate::station_tsf::StationTsfWrite;
use oer_ieee80211_lower_mac::TSF_DRIFT_PPM;

const STATION: MacAddress = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const ACCESS_POINT: MacAddress = [0x02, 0x11, 0x12, 0x13, 0x14, 0x15];
const BSSID: MacAddress = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const PEER: MacAddress = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
const OTHER_BSS: MacAddress = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45];
const TIMEOUT: u64 = 250_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StationPolicy {
    Link(MacAddress),
    OtherBss(MacAddress),
}

/// The hardware index of the best-effort queue, where the helpers' attempts
/// go.
const BE: usize = 2;

/// Register owner model with the four ordinary queues' latched state.
#[derive(Default)]
struct Hardware {
    legacy: Vec<(u8, MacLegacyTxProgram)>,
    ht: Vec<(u8, MacHtTxProgram)>,
    he: Vec<(u8, MacHeTxProgram)>,
    completion: [Option<MacTxCompletionObservation>; 4],
    block_ack_completion: [Option<MacHtAmpduCompletionObservation>; 4],
    timeout_pending: [bool; 4],
    collision_pending: [bool; 4],
    /// The MAC-wide CCA a timeout abort forces until its detach.
    cca_forced: bool,
    installed_keys: Vec<(u8, MacCcmpKeyIdentity)>,
    cleared_keys: Vec<u8>,
    rx_block_acks: Vec<S31RxBlockAckAgreement>,
    cleared_rx_block_acks: Vec<u8>,
    station_policy: Option<StationPolicy>,
    station_disabled: usize,
    access_point_policy: Option<MacAddress>,
    access_point_disabled: usize,
    access_point_tsf_resets: usize,
    promiscuous: bool,
    station_tsf: u64,
    tbtt: Option<StaTbttSchedule>,
    tbtt_stops: usize,
    tx_blocked: bool,
    rx_beacon_pti: Option<MacPti>,
    rx_beacon_pti_clears: usize,
}

impl Hardware {
    fn publications(&self) -> usize {
        self.legacy.len() + self.ht.len() + self.he.len()
    }
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy.push((queue, program));
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        self.ht.push((queue, program));
        true
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        self.he.push((queue, program));
        true
    }

    fn start_bound_he_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion[usize::from(queue)].take()
    }

    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.block_ack_completion[usize::from(queue)].take()
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        if !self.timeout_pending[usize::from(queue)] {
            return false;
        }
        assert!(!self.cca_forced, "one timeout abort forces CCA at a time");
        self.cca_forced = true;
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let queue = usize::from(queue);
        let pending = match reason {
            MacTxDetachReason::Timeout => {
                let pending = core::mem::take(&mut self.timeout_pending[queue]);
                if pending {
                    self.cca_forced = false;
                }
                pending
            }
            MacTxDetachReason::Collision => core::mem::take(&mut self.collision_pending[queue]),
            MacTxDetachReason::Completed => true,
        };
        if pending {
            MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                expected_descriptor_head,
            )))
        } else {
            MacTxDetachOutcome::NoEvent
        }
    }
}

impl HtAmpduHardware for Hardware {
    fn prepare_he_trigger_based_queue(
        &mut self,
        _policy: MacHeTbTidLimit,
        _reservation: MacHeTbLinkReservation,
        _tid: MacHeTid,
        _mpdu_lengths: &[u16],
        _queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        unreachable!("the port publishes no Trigger-based aggregate")
    }

    fn clear_he_trigger_based_queue(&mut self, _reservation: MacHeTbLinkReservation) {
        unreachable!("the port publishes no Trigger-based aggregate")
    }
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        if self.installed_keys.iter().any(|(slot, _)| *slot == index) {
            return MacKeyInstallOutcome::Occupied;
        }
        self.installed_keys.push((index, identity));
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.installed_keys.retain(|(slot, _)| *slot != index);
        self.cleared_keys.push(index);
    }
}

impl RxBlockAckHardware for Hardware {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let agreement = agreement.validate()?;
        self.rx_block_acks.push(agreement);
        Ok(())
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.rx_block_acks
            .retain(|agreement| agreement.hardware_index != hardware_index);
        self.cleared_rx_block_acks.push(hardware_index);
        Ok(())
    }

    fn reset_rx_block_ack_window(
        &mut self,
        _hardware_index: u8,
        _tid: u8,
        _starting_sequence: SequenceNumber,
        _window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!("the port never moves a receive window")
    }

    fn program_extra_softap_rx_block_ack(
        &mut self,
        _agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!("the port programs ordinary banks only")
    }

    fn clear_extra_softap_rx_block_ack(
        &mut self,
        _hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!("the port programs ordinary banks only")
    }

    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        _hardware_index: u8,
        _starting_sequence: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!("the port programs ordinary banks only")
    }
}

impl StaApRegisterHardware for Hardware {
    fn apply_sta_ap_receive_registers(&mut self, _plan: MacStaApReceivePlan) {
        unreachable!("the port applies each role's policy on its own")
    }

    fn disable_station_receive_registers(&mut self) {
        self.station_policy = None;
        self.station_disabled += 1;
    }

    fn disable_access_point_receive_registers(&mut self) {
        unreachable!("the access point closes through its link-policy seam")
    }

    fn disable_all_role_receive_registers(&mut self) {
        unreachable!("the port closes one role at a time")
    }
}

impl StaNoiseFloorHardware for Hardware {
    fn read_noise_floor_dbm(&self) -> i8 {
        -96
    }
}

impl StaLinkRxPolicyHardware for Hardware {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.station_policy = Some(StationPolicy::Link(bssid));
    }
}

impl StaEspNowRxPolicyHardware for Hardware {
    fn apply_sta_esp_now_policy(&mut self, bssid: [u8; 6]) {
        self.station_policy = Some(StationPolicy::OtherBss(bssid));
    }
}

impl MacSnifferHardware for Hardware {
    fn configure_open_promiscuous_receive(&mut self) {
        self.promiscuous = true;
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.promiscuous = false;
    }
}

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.access_point_policy = Some(access_point);
    }

    fn disable_ap_link_policy(&mut self) {
        self.access_point_policy = None;
        self.access_point_disabled += 1;
    }
}

impl ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
        self.access_point_tsf_resets += 1;
    }

    fn stop_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
        unreachable!("the port never stops the access-point TSF")
    }
}

impl StationTsfHardware for Hardware {
    fn station_tsf(&mut self) -> u64 {
        self.station_tsf
    }

    fn set_station_tsf(&mut self, _: StationTsfWrite, value: u64) {
        self.station_tsf = value;
    }
}

impl StationTbttHardware for Hardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        self.tbtt = Some(schedule);
    }

    fn stop_station_tbtt(&mut self) {
        self.tbtt = None;
        self.tbtt_stops += 1;
    }
}

impl TxGateHardware for Hardware {
    fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.tx_blocked = blocked;
    }
}

impl RxBeaconPriorityHardware for Hardware {
    fn set_rx_beacon_pti(&mut self, pti: MacPti) {
        self.rx_beacon_pti = Some(pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        self.rx_beacon_pti_clears += 1;
    }
}

/// A radio system whose beacon-window event has priority 9, its connection
/// event 6 and its slice event 4.
struct RadioNine;

impl RadioCoexPriorities for RadioNine {
    fn beacon_window_pti(&self) -> MacPti {
        MacPti::new(9).unwrap()
    }

    fn connection_frame_priorities(&self) -> ConnectionFramePriorities {
        ConnectionFramePriorities {
            packet: 6,
            scheduler: 4,
        }
    }
}

struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

type Timer = oer_time_virtual::SkipClock;

fn entropy() -> u32 {
    0x1234_5678
}

type Core = LowerMacCore<'static, Power, fn() -> u32, Timer, 512>;

#[derive(Default)]
struct Events {
    completions: Vec<PublicationEnd>,
    /// The rate of each ended attempt, in order.
    rates: Vec<PhyRate>,
    /// The queue of each completion, in order.
    queues: Vec<WmmAccessCategory>,
    /// The slots the completions gave back.
    slots: Vec<Pin<&'static mut TxSlot<512>>>,
    disabled: usize,
    tbtts: Vec<TbttEvent>,
}

impl LowerMacSink<'static, 512> for Events {
    fn tx_completed(
        &mut self,
        queue: WmmAccessCategory,
        slot: Pin<&'static mut TxSlot<512>>,
        end: PublicationEnd,
        rate: PhyRate,
    ) {
        self.queues.push(queue);
        self.slots.push(slot);
        self.completions.push(end);
        self.rates.push(rate);
    }

    fn disabled(&mut self) {
        self.disabled += 1;
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.tbtts.push(event);
    }
}

fn channel(number: u8) -> WifiChannel {
    WifiChannel::mhz20(number).unwrap()
}

/// A model slot in permanently retained storage, as target SRAM is.
fn slot() -> Pin<&'static mut TxSlot<512>> {
    Pin::static_mut(Box::leak(Box::new(TxSlot::new_model().unwrap())))
}

fn core() -> Core {
    LowerMacCore::new(
        OrdinaryTxOwner::new(WifiTxResources {
            slot: slot(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: Timer::default(),
        }),
        LowerMacConfig {
            station_address: STATION,
            channel: channel(6),
            publication_timeout: oer_time::Duration::from_micros(TIMEOUT),
            tsf_epoch: 1,
        },
        &RadioNine,
    )
}

fn station() -> CoreVif {
    CoreVif {
        address: STATION,
        role: VifRole::Station,
        bssid: Some(BSSID),
        receive: ReceiveFilter::BSS_MEMBER,
    }
}

fn access_point() -> CoreVif {
    CoreVif {
        address: ACCESS_POINT,
        role: VifRole::AccessPoint,
        bssid: None,
        receive: ReceiveFilter::BSS_MEMBER,
    }
}

/// An enabled core with a station interface on channel 6.
fn enabled(hardware: &mut Hardware) -> Core {
    let mut core = core();
    core.configure_checked_vif(hardware, station()).unwrap();
    assert_eq!(core.enable_checked(), LifecycleStart::Enabled);
    core
}

/// A QoS Data MPDU from the station to the BSS.
fn data_frame(receiver: MacAddress) -> [u8; 26] {
    let mut frame = [0; 26];
    frame[0] = 0x88;
    frame[1] = 0x01;
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&STATION);
    frame[16..22].copy_from_slice(&BSSID);
    frame
}

/// The rate the attempts go at by default, 24 Mb/s OFDM.
const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);

/// The station's best-effort attempt of `frame` at 24 Mb/s, in its own
/// slot, acknowledged unless `no_ack`.
fn values(frame: &[u8], no_ack: bool) -> MpduValues<'static, 512> {
    let mut slot = slot();
    slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()]
        .copy_from_slice(frame);
    MpduValues {
        role: VifRole::Station,
        access_category: WmmAccessCategory::BestEffort,
        slot,
        len: frame.len(),
        no_ack,
        rate: TxPhyRate::try_from(OFDM24).unwrap(),
        reported: OFDM24,
        protection: Protection::None,
        key: None,
        power_ceiling_dbm: None,
        backoff_slots: 7,
        coex: CoexPriority::Normal,
        retry: false,
    }
}

/// `values` at `rate`.
fn at_rate(mut values: MpduValues<'static, 512>, rate: PhyRate) -> MpduValues<'static, 512> {
    values.rate = TxPhyRate::try_from(rate).unwrap();
    values.reported = rate;
    values
}

/// Submit, the fault without its slot.
fn submit(
    core: &mut Core,
    hardware: &mut Hardware,
    values: MpduValues<'static, 512>,
) -> Result<(), LowerMacFault> {
    core.submit_checked(hardware, values)
        .map_err(|refused| refused.fault)
}

/// A publication's end by its completion, without a BlockAck.
fn completed(status: TxStatus, ack_snr_db: Option<i8>) -> PublicationEnd {
    PublicationEnd::Completed {
        status,
        ack_snr_db,
        block_ack: None,
    }
}

/// The hardware status of a completed publication.
fn status_of(end: &PublicationEnd) -> Option<TxStatus> {
    match end {
        PublicationEnd::Completed { status, .. } => Some(*status),
        _ => None,
    }
}

fn interrupt(events: u32) -> WifiTxWake {
    WifiTxWake::Interrupt { events }
}

/// Deliver a completion with `status` and return the completion event.
fn complete(core: &mut Core, hardware: &mut Hardware, status: u8) -> Vec<PublicationEnd> {
    complete_with(core, hardware, status, 0)
}

fn complete_with(
    core: &mut Core,
    hardware: &mut Hardware,
    status: u8,
    detail: u8,
) -> Vec<PublicationEnd> {
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(status, detail));
    let mut events = Events::default();
    core.service(hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    events.completions
}

/// The single attempt published on queue `index`.
fn queued(core: &Core, index: usize) -> &QueuedSingleAttempt<'static, 512> {
    match core.queues[index].as_ref().map(|attempt| &attempt.work) {
        Some(Work::Published(queued)) => queued,
        _ => panic!("queue {index} holds a published MPDU"),
    }
}

#[test]
fn capabilities_are_the_s31_limits_on_2_4_ghz() {
    let caps = esp32s31_lower_mac_capabilities(512);
    assert_eq!(
        caps.services,
        ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services()
    );
    assert!(caps.services.contains(HardwareServices::FCS));
    assert!(!caps.services.contains(HardwareServices::RETRY_POLICY));
    assert!(!caps.services.contains(HardwareServices::BACKOFF_DRAW));
    assert!(caps.supports_channel(Channel::ghz2_4(6, ChannelWidth::Mhz40Above).unwrap()));
    assert!(!caps.supports_channel(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()));
    assert_eq!(caps.vifs, 2);
    assert_eq!(caps.tx_queues, 4);
    assert_eq!(caps.tx_queue(WmmAccessCategory::Voice), 3);
    // Metadata word, MPDU, MIC and FCS fill the slot.
    assert_eq!(
        usize::from(caps.max_mpdu_length) + TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE,
        512
    );
    assert_eq!(caps.max_backoff_slots, 1023);
    assert!(caps.coex_priorities.contains(CoexPriority::Normal));
    assert!(caps.coex_priorities.contains(CoexPriority::Elevated));
    assert!(!caps.coex_priorities.contains(CoexPriority::Critical));
    assert!(
        caps.individual_no_ack
            .contains_rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(
        caps.access_point_receive_filters,
        ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS)
    );
    assert_eq!(usize::from(caps.key_slots), LOWER_MAC_KEY_SLOTS);
    assert_eq!(caps.rx_block_ack_agreements, RESOURCES.rx_block_ack_entries);
}

#[test]
fn an_attempt_is_published_from_the_slot_it_was_written_in() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = values(&frame, false);
    let written = request.slot.as_ref().buffer_address();
    assert_eq!(submit(&mut core, &mut hardware, request), Ok(()));
    // The queue publishes the slot the MPDU was written in, which comes
    // back with its completion.
    assert_eq!(queued(&core, BE).slot().buffer_address(), written);
    assert!(core.queue_busy(WmmAccessCategory::BestEffort));
    assert_eq!(hardware.legacy.len(), 1);
    assert_eq!(hardware.legacy[0].1.interface(), MacInterface::Station);
    assert_eq!(
        core.next_deadline(),
        Some(oer_time::Instant::from_micros(TIMEOUT))
    );

    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert_eq!(
        events.completions,
        [completed(TxStatus::Success, Some(0x60))]
    );
    assert_eq!(events.queues, [WmmAccessCategory::BestEffort]);
    assert_eq!(events.slots[0].as_ref().buffer_address(), written);
    assert_eq!(core.next_deadline(), None);
    assert!(!core.queue_busy(WmmAccessCategory::BestEffort));
    let next = values(&frame, false);
    assert_eq!(submit(&mut core, &mut hardware, next), Ok(()));
}

#[test]
fn the_callers_backoff_is_counted_down_instead_of_a_draw() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    for slots in [0, 700, 1023] {
        let mut request = values(&frame, false);
        request.backoff_slots = slots;
        submit(&mut core, &mut hardware, request).unwrap();
        assert_eq!(hardware.legacy.last().unwrap().1.contention_window(), slots);
        assert_eq!(
            queued(&core, BE).slot().work().backoff_slots,
            u32::from(slots)
        );
        complete(&mut core, &mut hardware, 5);
    }
}

#[test]
fn a_power_ceiling_bounds_the_data_and_the_control_frame() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    // The profile's calibrated codes are 5 and 6.
    for (ceiling, data, control) in [
        (None, 5, (5, 6)),
        (Some(3), 3, (3, 3)),
        (Some(10), 5, (5, 6)),
        (Some(0), 0, (0, 0)),
    ] {
        let mut request = values(&frame, false);
        request.power_ceiling_dbm = ceiling;
        request.protection = Protection::RtsCts;
        submit(&mut core, &mut hardware, request).unwrap();
        let program = hardware.legacy.last().unwrap().1;
        assert_eq!(program.data_power(), data);
        let control_frame = program.control();
        assert_eq!(
            (control_frame.power_primary, control_frame.power_alternate),
            control
        );
        complete(&mut core, &mut hardware, 0);
    }
}

#[test]
fn an_unacknowledged_attempt_is_reported_and_never_published_again() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    // ACK timeout, CTS timeout and a collision completion each end the
    // attempt after its single publication: the driver carries the retry
    // plan out, the core never publishes again on its own.
    let mut slots = Vec::new();
    for (status, detail, expected) in [
        (5, 0, TxStatus::AckTimeout),
        (2, 0, TxStatus::CtsTimeout),
        (4, 1, TxStatus::Collision),
    ] {
        let published = hardware.publications();
        let request = values(&frame, false);
        assert_eq!(submit(&mut core, &mut hardware, request), Ok(()));
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(status, detail));
        let mut events = Events::default();
        core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
            .unwrap();
        assert_eq!(events.completions.len(), 1);
        assert_eq!(events.completions[0], completed(expected, None));
        assert_eq!(hardware.publications(), published + 1);
        slots.extend(events.slots);
    }
    // The core sets no Retry bit of its own.
    assert!(
        slots
            .iter_mut()
            .all(|slot| slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1] == 0x01)
    );
}

#[test]
fn a_no_effect_rts_event_retains_its_published_owner_until_the_real_result() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let request = values(&data_frame(BSSID), false);
    let address = request.slot.as_ref().buffer_address();
    submit(&mut core, &mut hardware, request).unwrap();
    let deadline = core.next_deadline();
    let mut events = Events::default();
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(1, 2));
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    assert!(events.slots.is_empty());
    assert_eq!(hardware.publications(), 1);
    assert_eq!(queued(&core, BE).slot().buffer_address(), address);
    assert_eq!(
        queued(&core, BE).slot().state(),
        oer_esp32s31_ieee80211_mac::tx::TxSlotState::HardwareOwned
    );
    assert_eq!(core.next_deadline(), deadline);
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert_eq!(
        events.completions,
        [completed(TxStatus::Success, Some(0x60))]
    );
    assert_eq!(events.slots.len(), 1);
    assert_eq!(events.slots[0].as_ref().buffer_address(), address);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_no_effect_rts_event_can_end_only_at_the_later_hardware_timeout() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
    let mut events = Events::default();
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(1, 0xff));
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    hardware.timeout_pending[BE] = true;
    core.service(&mut hardware, interrupt(EVENT_TX_TIMEOUT), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    let settle = core.next_deadline().unwrap();
    core.tx.timer.advance_to(settle);
    core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
        .unwrap();
    assert_eq!(events.completions, [PublicationEnd::TimedOut]);
    assert_eq!(events.slots.len(), 1);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn review_no_effect_cannot_postpone_an_expired_publication_watchdog() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
    let deadline = core.next_deadline().unwrap();
    core.tx.timer.advance_to(deadline);
    let mut events = Events::default();
    // An RTS diagnostic is not a qualified stop. It must not hide an already
    // expired watchdog or make that deadline wait depend on further input.
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(1, 2));
    let result = core.service(&mut hardware, WifiTxWake::Deadline, &mut events);
    assert!(result.is_err(), "expired watchdog was bypassed: {result:?}");
    assert!(events.completions.is_empty());
}

#[test]
fn completion_faults_return_the_existing_backend_fault_without_a_tx_terminal() {
    use oer_esp32s31_ieee80211_mac::tx::CompletionFault;
    for (status, detail, fault) in [
        (1, 0xc0, CompletionFault::SecurityKeyError),
        (4, 0xc0, CompletionFault::SecurityKeyError),
        (3, 0, CompletionFault::InvalidStatus { status: 3 }),
        (9, 0, CompletionFault::InvalidStatus { status: 9 }),
    ] {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware);
        submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(status, detail));
        let mut events = Events::default();
        assert_eq!(
            core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events),
            Err(LowerMacFault::Tx(OrdinaryTxError::Tx(TxError::Completion(
                fault
            ))))
        );
        assert!(events.completions.is_empty());
        assert!(events.slots.is_empty());
        assert_eq!(hardware.publications(), 1);
    }
}

#[test]
fn a_transmission_the_plan_sends_again_carries_the_retry_bit() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let mut request = values(&data_frame(BSSID), false);
    request.retry = true;
    submit(&mut core, &mut hardware, request).unwrap();
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert_eq!(
        events.slots[0].as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1],
        0x09
    );
}

#[test]
fn a_collision_detach_ends_the_attempt_without_a_transmission() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = values(&frame, false);
    submit(&mut core, &mut hardware, request).unwrap();
    hardware.collision_pending[BE] = true;
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_COLLISION), &mut events)
        .unwrap();
    assert_eq!(events.completions, [PublicationEnd::Collision]);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_hardware_timeout_ends_the_publication_with_an_unknown_result() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = values(&frame, false);
    submit(&mut core, &mut hardware, request).unwrap();
    hardware.timeout_pending[BE] = true;
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_TIMEOUT), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    let settle = core.next_deadline().unwrap();
    core.tx
        .timer
        .advance_to(oer_time::Instant::from_micros(settle.as_micros()));
    core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
        .unwrap();
    assert_eq!(events.completions.len(), 1);
    assert_eq!(events.completions[0], PublicationEnd::TimedOut);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_block_ack_request_reports_its_block_ack() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let mut frame = [0; 24];
    // A BlockAckReq: control type, subtype eight.
    frame[0] = 0x84;
    frame[4..10].copy_from_slice(&BSSID);

    submit(&mut core, &mut hardware, values(&frame, false)).unwrap();
    assert_eq!(
        hardware.legacy[0].1.response(),
        MacLegacyTxResponse::BlockAck
    );
    hardware.block_ack_completion[BE] = Some(MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(0, 0),
        0,
        100,
        0b1011,
        true,
    ));
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert!(matches!(
        events.completions[0],
        PublicationEnd::Completed {
            status: TxStatus::Success,
            block_ack: Some(BlockAckReport { start_sequence, bitmap: 0b1011 }),
            ..
        } if start_sequence == SequenceNumber::new(100).unwrap()
    ));
}

#[test]
fn review_bar_requires_the_solicited_block_ack_result_bit() {
    for (status, received, expected) in [
        (0, true, TxStatus::Success),
        (0, false, TxStatus::AckTimeout),
        (5, true, TxStatus::Success),
        (5, false, TxStatus::AckTimeout),
        (2, true, TxStatus::Success),
        (2, false, TxStatus::CtsTimeout),
    ] {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware);
        let mut frame = [0; 24];
        frame[0] = 0x84;
        frame[4..10].copy_from_slice(&BSSID);
        submit(&mut core, &mut hardware, values(&frame, false)).unwrap();
        hardware.block_ack_completion[BE] =
            Some(block_ack_completion(status, 100, 0b1011, received));
        let mut events = Events::default();
        core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
            .unwrap();
        match events.completions[0] {
            PublicationEnd::Completed {
                status: actual,
                block_ack,
                ..
            } => {
                assert_eq!(
                    actual, expected,
                    "BAR status={status}, BA-received={received}"
                );
                assert_eq!(block_ack.is_some(), received);
            }
            other => panic!("wrong terminal: {other:?}"),
        }
    }
}

#[test]
fn review_bar_status_zero_without_block_ack_is_not_delivered() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let mut frame = [0; 24];
    frame[0] = 0x84;
    frame[4..10].copy_from_slice(&BSSID);
    submit(&mut core, &mut hardware, values(&frame, false)).unwrap();
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b1011, false));
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert!(matches!(
        events.completions[0],
        PublicationEnd::Completed {
            status: TxStatus::AckTimeout,
            block_ack: None,
            ..
        }
    ));
}

#[test]
fn review_aggregate_no_effect_cannot_postpone_expired_watchdog() {
    let mut hardware = Hardware::default();
    let core = enabled(&mut hardware);
    let source = backings();
    let published = publish_ampdu(
        &core,
        &mut hardware,
        aggregate_of(source, 2, 26),
        ampdu_values(ht_rate(7)),
    )
    .unwrap();
    let deadline = published.deadline();
    hardware.block_ack_completion[BE] = Some(block_ack_completion(1, 0, 0, false));
    let result = ampdu::service(
        &mut hardware,
        published,
        WifiTxWake::Deadline,
        true,
        deadline,
    );
    assert!(result.is_err(), "NoEffect hid an expired A-MPDU watchdog");
}

#[test]
fn ht_and_he_attempts_use_their_programs() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    let ht = PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap(),
    );
    let he = PhyRate::He(
        phy::HeRate::new(
            phy::HeMcs::new(7).unwrap(),
            SpatialStreams::new(1).unwrap(),
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Ldpc,
            false,
        )
        .unwrap(),
    );

    submit(&mut core, &mut hardware, at_rate(values(&frame, false), ht)).unwrap();
    assert_eq!(hardware.ht.len(), 1);
    complete(&mut core, &mut hardware, 0);

    submit(&mut core, &mut hardware, at_rate(values(&frame, false), he)).unwrap();
    assert_eq!(hardware.he.len(), 1);
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    // The end names the rate the attempt went at.
    assert_eq!(events.rates, [he]);
    assert_eq!(hardware.publications(), 2);
}

#[test]
fn the_callers_protection_is_published() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    for (protection, expected) in [
        (Protection::RtsCts, MacTxProtection::RtsCts),
        (Protection::CtsToSelf, MacTxProtection::CtsToSelf),
        (Protection::None, MacTxProtection::None),
    ] {
        let mut request = values(&frame, false);
        request.protection = protection;
        submit(&mut core, &mut hardware, request).unwrap();
        assert_eq!(
            hardware.legacy.last().unwrap().1.control().protection,
            expected
        );
        complete(&mut core, &mut hardware, 0);
    }
}

#[test]
fn a_legacy_unicast_frame_is_sent_without_acknowledgement_on_request() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    submit(&mut core, &mut hardware, values(&frame, true)).unwrap();
    assert_eq!(hardware.legacy[0].1.response(), MacLegacyTxResponse::None);
    assert_eq!(
        status_of(&complete(&mut core, &mut hardware, 0)[0]),
        Some(TxStatus::Success)
    );
}

#[test]
fn a_group_frame_solicits_no_response() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame([0xff; 6]);
    assert_eq!(
        submit(&mut core, &mut hardware, values(&frame, false)),
        Ok(())
    );
    assert_eq!(hardware.legacy[0].1.response(), MacLegacyTxResponse::None);
    assert_eq!(
        status_of(&complete(&mut core, &mut hardware, 0)[0]),
        Some(TxStatus::Success)
    );
}

#[test]
fn a_channel_change_retunes_at_enable() {
    let mut core = core();
    let mut events = Events::default();

    core.set_checked_channel(channel(11));
    assert_eq!(core.enable_checked(), LifecycleStart::Retune(channel(11)));
    assert!(core.finish_retune(true));
    assert_eq!(
        core.channel(),
        Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap()
    );

    core.disable_checked(&mut events);
    assert_eq!(events.disabled, 1);
    // The radio is already on channel 11: no retune.
    assert_eq!(core.enable_checked(), LifecycleStart::Enabled);
}

#[test]
fn a_failed_retune_fails_enable_recoverably_and_leaves_the_port_disabled() {
    let mut core = core();
    core.set_checked_channel(channel(1));
    assert_eq!(core.enable_checked(), LifecycleStart::Retune(channel(1)));
    assert!(!core.finish_retune(false));
    assert_eq!(core.state, PortState::Disabled);
    // The channel stays configured: the next Enable retunes again.
    assert_eq!(core.enable_checked(), LifecycleStart::Retune(channel(1)));
}

#[test]
fn receive_filters_map_onto_the_smallest_superset_policy() {
    let mut hardware = Hardware::default();
    let mut core = core();

    for (receive, bssid, expected) in [
        (
            ReceiveFilter::BSS_MEMBER,
            Some(BSSID),
            StationPolicy::Link(BSSID),
        ),
        // A subset of a policy's rules takes that policy.
        (
            ReceiveFilter::OWN_UNICAST,
            Some(BSSID),
            StationPolicy::Link(BSSID),
        ),
        (
            ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
            Some(BSSID),
            StationPolicy::OtherBss(BSSID),
        ),
        // Scanning outside a BSS matches the broadcast BSSID.
        (
            ReceiveFilter::OTHER_BSS_MANAGEMENT,
            None,
            StationPolicy::OtherBss([0xff; 6]),
        ),
    ] {
        core.configure_checked_vif(
            &mut hardware,
            CoreVif {
                receive,
                bssid,
                ..station()
            },
        )
        .unwrap();
        assert_eq!(hardware.station_policy, Some(expected));
    }
    core.configure_checked_vif(&mut hardware, access_point())
        .unwrap();
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // Rules no policy provides a superset of, a station address other than
    // the published one and an access point whose BSSID is not its address
    // change no register.
    for config in [
        CoreVif {
            bssid: None,
            ..station()
        },
        CoreVif {
            address: PEER,
            ..station()
        },
        CoreVif {
            receive: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
            ..access_point()
        },
        CoreVif {
            bssid: Some(PEER),
            ..access_point()
        },
    ] {
        assert_eq!(
            core.configure_checked_vif(&mut hardware, config),
            Err(UnsupportedVif)
        );
    }
    assert_eq!(
        hardware.station_policy,
        Some(StationPolicy::OtherBss([0xff; 6]))
    );
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // No receive filter closes the role's context.
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            receive: ReceiveFilter::NONE,
            bssid: None,
            ..station()
        },
    )
    .unwrap();
    assert_eq!(hardware.station_disabled, 1);
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            receive: ReceiveFilter::NONE,
            ..access_point()
        },
    )
    .unwrap();
    assert_eq!(hardware.access_point_disabled, 1);
}

fn normalized(mpdu: &[u8]) -> NormalizedRxFrame<'_> {
    NormalizedRxFrame {
        mpdu_offset: 0,
        mpdu,
        metadata: MacRxMetadata {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::<RxPhyInfo>::Unavailable,
            rssi_dbm: MacRxEvidence::HardwareObserved(-42),
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::HardwareObserved(false),
            ampdu: MacRxEvidence::Unavailable,
            amsdu: MacRxEvidence::Unavailable,
        },
        rssi_dbm: -42,
        logical_length: mpdu.len(),
        stamp: None,
    }
}

/// A management frame of `subtype` in `bssid` to `receiver`.
fn management(subtype: u8, receiver: MacAddress, bssid: MacAddress) -> [u8; 24] {
    let mut frame = [0; 24];
    frame[0] = subtype << 4;
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&bssid);
    frame[16..22].copy_from_slice(&bssid);
    frame
}

#[test]
fn received_frames_are_narrowed_to_the_requested_rules() {
    let mut hardware = Hardware::default();
    let mut core = core();
    let unicast = data_frame(STATION);
    let beacon = management(8, [0xff; 6], BSSID);
    let other_beacon = management(8, [0xff; 6], OTHER_BSS);

    assert!(core.received(&hardware, &normalized(&unicast)).is_none());
    // Unicast only: the link policy passes the BSS's beacons as well.
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            receive: ReceiveFilter::OWN_UNICAST,
            ..station()
        },
    )
    .unwrap();
    assert_eq!(core.enable_checked(), LifecycleStart::Enabled);
    let (bytes, received) = core.received(&hardware, &normalized(&unicast)).unwrap();
    // The frame carries the PHY's noise-floor estimate and its signal over
    // it.
    assert_eq!(received.link_metric(), LinkMetric::new(-42, -96));
    let meta = *received.common();
    assert_eq!(meta.noise_floor_dbm, RxEvidence::HardwareObserved(-96));
    assert_eq!(bytes, unicast);
    assert_eq!(meta.channel, Channel::from_wifi_channel(channel(6)));
    assert_eq!(meta.rssi_dbm, RxEvidence::HardwareObserved(-42));
    assert!(core.received(&hardware, &normalized(&beacon)).is_none());

    // Scanning while joined: the ESP-NOW policy passes everything the
    // filter names, and nothing else is delivered.
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            receive: ReceiveFilter::OWN_BSS_BEACONS.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
            ..station()
        },
    )
    .unwrap();
    assert!(core.received(&hardware, &normalized(&beacon)).is_some());
    assert!(
        core.received(&hardware, &normalized(&other_beacon))
            .is_some()
    );
    assert!(core.received(&hardware, &normalized(&unicast)).is_none());
}

#[test]
fn monitor_reception_runs_only_while_no_interface_receives() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let other_beacon = management(8, [0xff; 6], OTHER_BSS);

    assert_eq!(core.set_monitor(&mut hardware, true), Err(UnsupportedVif));
    assert!(!hardware.promiscuous);
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            receive: ReceiveFilter::NONE,
            ..station()
        },
    )
    .unwrap();
    assert_eq!(core.set_monitor(&mut hardware, true), Ok(()));
    assert!(hardware.promiscuous);
    assert!(
        core.received(&hardware, &normalized(&other_beacon))
            .is_some()
    );
    assert_eq!(core.set_monitor(&mut hardware, false), Ok(()));
    assert!(!hardware.promiscuous);
    assert!(
        core.received(&hardware, &normalized(&other_beacon))
            .is_none()
    );
}

#[test]
fn keys_install_into_their_role_slots_and_protect_attempts() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.configure_checked_vif(&mut hardware, access_point())
        .unwrap();
    static KEY: [u8; 16] = [0x5a; 16];

    let pairwise = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationPairwise { peer: BSSID },
            &KEY,
        )
        .unwrap();
    assert_eq!(hardware.installed_keys[0].0, 4);
    core.install_checked_key(
        &mut hardware,
        CheckedKeyScope::AccessPointPairwise {
            peer: PEER,
            association_id: 1,
        },
        &KEY,
    )
    .unwrap();
    // The access point's pairwise key of association identifier one.
    assert_eq!(hardware.installed_keys[1].0, 8);

    let frame = data_frame(BSSID);
    let mut protected = values(&frame, false);
    protected.key = Some(pairwise);
    submit(&mut core, &mut hardware, protected).unwrap();
    assert_eq!(hardware.legacy.len(), 1);
    assert!(core.key_in_use(pairwise));
    complete(&mut core, &mut hardware, 0);
    assert!(!core.key_in_use(pairwise));
    core.remove_checked_key(&mut hardware, pairwise);
    assert_eq!(hardware.cleared_keys, [4]);
    // A removed index is nothing to remove.
    core.remove_checked_key(&mut hardware, pairwise);
    assert_eq!(hardware.cleared_keys, [4]);
}

#[test]
fn a_fresh_protected_mpdu_is_numbered_by_its_key_and_the_station_group_key_refuses_tx() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    static KEY: [u8; 16] = [0x5a; 16];
    let pairwise = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationPairwise { peer: BSSID },
            &KEY,
        )
        .unwrap();
    let group = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationGroup {
                key_id: oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            },
            &KEY,
        )
        .unwrap();
    let mut slot = slot();
    // The vendor's step of three: a new key emits PN 3, then 6.
    for expected in [3, 6] {
        assert_eq!(core.number(pairwise, &mut slot, 26), Ok(()));
        let header = &slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 26..][..8];
        assert_eq!(header, &[expected, 0, 0, 0x20, 0, 0, 0, 0]);
    }
    // The station's group key remains installed for RX, but cannot prepare TX.
    slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 26..][..8].fill(0xee);
    assert_eq!(
        core.number(group, &mut slot, 26),
        Err(CcmpNumberingError::ReceiveOnlyKey)
    );
    assert_eq!(
        slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 26..][..8],
        [0xee; 8]
    );
}

#[test]
fn numbering_refuses_a_removed_key_without_writing_the_buffer() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    static KEY: [u8; 16] = [0x5a; 16];
    let key = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationPairwise { peer: BSSID },
            &KEY,
        )
        .unwrap();
    core.remove_checked_key(&mut hardware, key);
    let mut slot = slot();
    slot.as_mut().buffer_mut().unwrap().fill(0xee);

    assert_eq!(
        core.number(key, &mut slot, 26),
        Err(CcmpNumberingError::MissingKey)
    );
    assert_eq!(slot.as_mut().buffer_mut().unwrap(), &[0xee; 512]);
}

#[test]
fn numbering_refuses_an_unavailable_buffer_without_allocating_a_packet_number() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    static KEY: [u8; 16] = [0x5a; 16];
    let key = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationPairwise { peer: BSSID },
            &KEY,
        )
        .unwrap();
    let mut slot = slot();
    slot.as_mut().buffer_mut().unwrap().fill(0xee);
    let cookie = slot.as_mut().reserve(512, 64).unwrap();

    assert_eq!(
        core.number(key, &mut slot, 26),
        Err(CcmpNumberingError::BufferUnavailable(TxError::Busy))
    );
    slot.as_mut().cancel_reservation(cookie).unwrap();
    assert_eq!(slot.as_mut().buffer_mut().unwrap(), &[0xee; 512]);
    assert_eq!(core.number(key, &mut slot, 26), Ok(()));
    assert_eq!(
        slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 26..][..8],
        [3, 0, 0, 0x20, 0, 0, 0, 0]
    );
}

#[test]
fn numbering_checks_header_space_and_offset_overflow_before_allocating_a_packet_number() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    static KEY: [u8; 16] = [0x5a; 16];
    let key = core
        .install_checked_key(
            &mut hardware,
            CheckedKeyScope::StationPairwise { peer: BSSID },
            &KEY,
        )
        .unwrap();
    let mut slot = slot();
    slot.as_mut().buffer_mut().unwrap().fill(0xee);
    let last_header_len = 512 - TX_METADATA_SIZE - CCMP_HEADER_LEN;

    for header_len in [
        last_header_len + 1,
        512,
        usize::MAX - TX_METADATA_SIZE,
        usize::MAX,
    ] {
        assert_eq!(
            core.number(key, &mut slot, header_len),
            Err(CcmpNumberingError::InsufficientSpace)
        );
        assert_eq!(slot.as_mut().buffer_mut().unwrap(), &[0xee; 512]);
    }
    assert_eq!(core.number(key, &mut slot, last_header_len), Ok(()));
    assert_eq!(
        slot.as_mut().buffer_mut().unwrap()[512 - CCMP_HEADER_LEN..],
        [3, 0, 0, 0x20, 0, 0, 0, 0]
    );
}

#[test]
fn receive_block_ack_agreements_use_the_ordinary_banks() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let agreement = |tid, window| RxBlockAckValues {
        role: VifRole::Station,
        peer: BSSID,
        tid,
        start_sequence: SequenceNumber::new(10).unwrap(),
        window,
    };

    core.start_checked_rx_block_ack(&mut hardware, agreement(0, 32))
        .unwrap();
    assert_eq!(hardware.rx_block_acks[0].interface, MacInterface::Station);
    assert_eq!(hardware.rx_block_acks[0].window, 32);
    // An agreement of the same peer and TID replaces it in its bank.
    core.start_checked_rx_block_ack(&mut hardware, agreement(0, 16))
        .unwrap();
    assert_eq!(hardware.rx_block_acks[1].hardware_index, 0);
    assert_eq!(hardware.rx_block_acks[1].window, 16);
    for tid in 1..RESOURCES.rx_block_ack_entries {
        core.start_checked_rx_block_ack(&mut hardware, agreement(tid, 8))
            .unwrap();
    }
    // Every bank taken: an agreement of another peer finds none.
    let mut other_peer = agreement(0, 8);
    other_peer.peer = PEER;
    assert_eq!(
        core.start_checked_rx_block_ack(&mut hardware, other_peer),
        Err(LowerMacFault::NoRxBlockAckBank)
    );

    assert_eq!(
        core.end_checked_rx_block_ack(&mut hardware, VifRole::Station, BSSID, 0),
        Ok(true)
    );
    assert_eq!(hardware.cleared_rx_block_acks, [0]);
    assert_eq!(
        core.end_checked_rx_block_ack(&mut hardware, VifRole::Station, BSSID, 0),
        Ok(false)
    );
}

#[test]
fn the_station_tsf_is_set_and_read_and_the_access_point_tsf_only_restarts() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);

    assert_eq!(
        core.set_station_tsf(&mut hardware, TsfInstant::from_micros(123_456)),
        Ok(())
    );
    assert_eq!(
        core.station_tsf_reading(&mut hardware).0,
        VifTsf {
            vif: VifRole::Station,
            at: TsfInstant::from_micros(123_456),
        }
    );
    core.restart_access_point_tsf(&mut hardware);
    assert_eq!(hardware.access_point_tsf_resets, 1);
    let caps = ESP32S31_BEACON_TIMING_CAPABILITIES;
    assert!(caps.tsf_restart.contains(VifRole::AccessPoint));
    assert!(!caps.tsf_read.contains(VifRole::AccessPoint));
}

#[test]
fn the_station_tbtt_schedule_is_programmed_and_its_events_announce_the_next_tbtt() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let next = TsfInstant::from_micros(1_000_000);
    assert_eq!(
        core.set_station_tbtt(&mut hardware, next, 102_400, 3_000),
        Ok(())
    );
    assert_eq!(
        hardware.tbtt,
        Some(StaTbttSchedule {
            first_tbtt_tsf: 1_000_000,
            interval_micros: 102_400,
            ahead_micros: 3_000,
            wake_ahead_micros: 4_500,
        })
    );

    let mut events = Events::default();
    for (now, announced) in [
        (997_000, 1_000_000),
        (1_099_400, 1_102_400),
        // A missed event does not shift the schedule.
        (1_304_000, 1_307_200),
    ] {
        hardware.station_tsf = now;
        core.station_tbtt(&mut hardware, &mut events);
        assert_eq!(
            events.tbtts.pop(),
            Some(TbttEvent {
                tbtt: VifTsf {
                    vif: VifRole::Station,
                    at: TsfInstant::from_micros(announced),
                }
            })
        );
    }

    for (interval, lead) in [
        (0, 3_000),
        (u64::from(u32::MAX) + 1, 3_000),
        (102_400, u64::from(u16::MAX)),
    ] {
        assert_eq!(
            core.set_station_tbtt(&mut hardware, next, interval, lead),
            Err(UnsupportedTbtt)
        );
    }

    core.stop_station_tbtt(&mut hardware);
    assert_eq!(hardware.tbtt, None);
    assert_eq!(hardware.tbtt_stops, 1);
    // A stale edge after the stop reports nothing.
    core.station_tbtt(&mut hardware, &mut events);
    assert!(events.tbtts.is_empty());
}

#[test]
fn beacons_are_received_at_the_radio_system_s_beacon_window_priority() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.set_rx_beacon_priority(&mut hardware, RxBeaconPriority::BeaconWindow);
    assert_eq!(hardware.rx_beacon_pti, MacPti::new(9));
    core.set_rx_beacon_priority(&mut hardware, RxBeaconPriority::Zero);
    assert_eq!(hardware.rx_beacon_pti, MacPti::new(0));
    core.set_rx_beacon_priority(&mut hardware, RxBeaconPriority::Cleared);
    assert_eq!(hardware.rx_beacon_pti_clears, 1);
}

#[test]
fn a_closed_gate_blocks_the_queues_and_holds_the_attempt_until_it_opens() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    core.hold_new_attempts();
    assert!(core.published_idle());
    core.block_held_queues(&mut hardware);
    assert!(hardware.tx_blocked);
    assert_eq!(
        submit(&mut core, &mut hardware, values(&frame, false)),
        Ok(())
    );
    assert_eq!(hardware.publications(), 0);
    assert_eq!(core.next_deadline(), None);
    core.open_gate(&mut hardware)
        .map_err(|refused| refused.fault)
        .unwrap();
    assert!(!hardware.tx_blocked);
    assert_eq!(hardware.publications(), 1);
    assert_eq!(
        status_of(&complete(&mut core, &mut hardware, 0)[0]),
        Some(TxStatus::Success)
    );

    // A held attempt the closed gate gives back ends without a
    // transmission, its slot with it.
    core.hold_new_attempts();
    core.block_held_queues(&mut hardware);
    submit(&mut core, &mut hardware, values(&frame, false)).unwrap();
    let mut events = Events::default();
    core.withdraw_held_attempts(&mut events);
    assert_eq!(events.completions, [PublicationEnd::Withdrawn]);
    assert_eq!(events.slots.len(), 1);
    assert_eq!(hardware.publications(), 1);
}

/// The station's attempt of `category` carrying a QoS Data MPDU to the BSS.
fn values_on(category: WmmAccessCategory) -> MpduValues<'static, 512> {
    let mut request = values(&data_frame(BSSID), false);
    request.access_category = category;
    request
}

/// The hardware index of the queue an access category occupies.
fn queue_of(category: WmmAccessCategory) -> usize {
    usize::from(LegacyTxQueue::from_access_category(category).hardware_index())
}

/// Serve `wake`: the queues of the attempts it ended, in order.
fn service(core: &mut Core, hardware: &mut Hardware, wake: WifiTxWake) -> Vec<WmmAccessCategory> {
    let mut events = Events::default();
    core.service(hardware, wake, &mut events).unwrap();
    events.queues
}

#[test]
fn each_queue_holds_one_attempt_and_they_complete_in_any_order() {
    use WmmAccessCategory::{Background, BestEffort, Video, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);

    for category in [Voice, BestEffort, Video, Background] {
        assert!(!core.queue_busy(category));
        assert_eq!(
            submit(&mut core, &mut hardware, values_on(category)),
            Ok(())
        );
        assert!(core.queue_busy(category));
    }
    // Four attempts in flight, one publication each, one per queue.
    let mut queues: Vec<u8> = hardware.legacy.iter().map(|(queue, _)| *queue).collect();
    queues.sort_unstable();
    assert_eq!(queues, [0, 1, 2, 3]);
    assert_eq!(
        core.next_deadline(),
        Some(oer_time::Instant::from_micros(TIMEOUT))
    );

    // The last queue completes first; the others stay published.
    hardware.completion[queue_of(Background)] = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE)),
        [Background]
    );
    assert_eq!(core.queues.iter().flatten().count(), 3);

    // Two queues completing under one coalesced edge each report theirs.
    hardware.completion[queue_of(Voice)] = Some(MacTxCompletionObservation::new_model(5, 0));
    hardware.completion[queue_of(Video)] = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE)),
        [Voice, Video]
    );

    // An edge no queue shows is ignored.
    assert!(service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE)).is_empty());
    assert_eq!(complete(&mut core, &mut hardware, 0).len(), 1);

    // No attempt was published twice.
    assert_eq!(hardware.publications(), 4);
    assert_eq!(submit(&mut core, &mut hardware, values_on(Voice)), Ok(()));
}

#[test]
fn a_collision_or_timeout_on_one_queue_leaves_the_others_published() {
    use WmmAccessCategory::{BestEffort, Video, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    for category in [Voice, BestEffort, Video] {
        submit(&mut core, &mut hardware, values_on(category)).unwrap();
    }

    hardware.collision_pending[queue_of(BestEffort)] = true;
    assert_eq!(
        service(&mut core, &mut hardware, interrupt(EVENT_COLLISION)),
        [BestEffort]
    );
    assert!(core.queues[queue_of(Voice)].is_some());
    assert!(core.queues[queue_of(Video)].is_some());

    // A timeout forces CCA for its settle; a second queue's timeout during
    // that settle waits for it instead of forcing CCA again.
    hardware.timeout_pending[queue_of(Voice)] = true;
    assert!(service(&mut core, &mut hardware, interrupt(EVENT_TX_TIMEOUT)).is_empty());
    let settle = core.next_deadline().unwrap();
    assert_eq!(settle, oer_time::Instant::from_micros(16));
    hardware.timeout_pending[queue_of(Video)] = true;
    assert!(service(&mut core, &mut hardware, interrupt(EVENT_TX_TIMEOUT)).is_empty());
    assert_eq!(core.next_deadline(), Some(settle));

    core.tx
        .timer
        .advance_to(oer_time::Instant::from_micros(settle.as_micros()));
    assert_eq!(
        service(&mut core, &mut hardware, WifiTxWake::Deadline),
        [Voice]
    );
    // The waiting queue's abort began as the first settle ended.
    assert!(hardware.cca_forced);
    let settle = core.next_deadline().unwrap();
    assert_eq!(settle, oer_time::Instant::from_micros(32));
    core.tx
        .timer
        .advance_to(oer_time::Instant::from_micros(settle.as_micros()));
    assert_eq!(
        service(&mut core, &mut hardware, WifiTxWake::Deadline),
        [Video]
    );
    assert!(!hardware.cca_forced);
    assert_eq!(hardware.publications(), 3);
}

#[test]
fn a_closed_gate_holds_every_queue_and_opening_it_publishes_them() {
    use WmmAccessCategory::{BestEffort, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.hold_new_attempts();
    core.block_held_queues(&mut hardware);
    for category in [Voice, BestEffort] {
        submit(&mut core, &mut hardware, values_on(category)).unwrap();
    }
    assert_eq!(hardware.publications(), 0);
    assert_eq!(core.next_deadline(), None);

    core.open_gate(&mut hardware)
        .map_err(|refused| refused.fault)
        .unwrap();
    assert_eq!(hardware.legacy.len(), 2);
    assert!(!core.published_idle());
}

/// The WMM set of an access point whose best-effort category has a TXOP
/// limit of `txop_units_32_us`.
fn wmm_with_best_effort_txop(
    txop_units_32_us: u16,
) -> oer_ieee80211_mac::extensions::wmm::WmmParameterSet {
    use oer_ieee80211_mac::extensions::wmm::WmmAcParameters;
    let category = |aifsn, txop| WmmAcParameters {
        admission_control_mandatory: false,
        aifsn,
        ecw_min: 4,
        ecw_max: 10,
        txop_limit_units_32_us: txop,
    };
    oer_ieee80211_mac::extensions::wmm::WmmParameterSet::new(
        0,
        false,
        [
            category(3, txop_units_32_us),
            category(7, 0),
            category(2, 94),
            category(2, 47),
        ],
    )
}

#[test]
fn an_elevated_mpdu_carries_the_radio_system_s_connection_priorities() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    assert!(
        esp32s31_lower_mac_capabilities(512)
            .coex_priorities
            .contains(CoexPriority::Elevated)
    );
    let mut request = values(&data_frame(BSSID), false);
    request.coex = CoexPriority::Elevated;
    submit(&mut core, &mut hardware, request).unwrap();
    let program = hardware.legacy[0].1;
    assert_eq!(
        (
            program.scheduler_priority(),
            program.packet_priority(),
            program.priority_count()
        ),
        (4, 6, CONNECTION_FRAME_PRIORITY_COUNT)
    );
    complete(&mut core, &mut hardware, 0);
    submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
    assert_eq!(hardware.legacy[1].1.priority_count(), 1);
}

#[test]
fn the_station_s_he_bss_color_is_installed() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.set_checked_he_bss_color(63);
    assert_eq!(core.tx.policy().he_bss_color(), 63);
}

#[test]
fn the_tsf_relation_breaks_at_a_jump_a_channel_change_and_a_new_bss() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let generation = |core: &Core, hardware: &mut Hardware| core.station_tsf_reading(hardware).1;

    // The first set starts a generation.
    let start = generation(&core, &mut hardware);
    core.set_station_tsf(&mut hardware, TsfInstant::from_micros(1_000_000))
        .unwrap();
    let followed = generation(&core, &mut hardware);
    assert_ne!(followed, start);

    let allowed = |elapsed: u64| {
        (elapsed * u64::from(TSF_DRIFT_PPM)).div_ceil(1_000_000)
            + crate::station_tsf::STATION_TSF_SAMPLE_UNCERTAINTY.as_micros()
    };
    let interval = 102_400;

    // After ten missed beacons the follow corrects the drift of ten
    // intervals: more than one interval allows, within what ten allow.
    let mut last = 1_000_000;
    let reading = last + 10 * interval;
    let corrected = reading + allowed(10 * interval);
    assert!(corrected - reading > allowed(interval));
    hardware.station_tsf = reading;
    core.set_station_tsf(&mut hardware, TsfInstant::from_micros(corrected))
        .unwrap();
    assert_eq!(generation(&core, &mut hardware), followed);
    last = corrected;

    // At the next beacon, one interval later, the same correction is a
    // real jump.
    let reading = last + interval;
    hardware.station_tsf = reading;
    core.set_station_tsf(
        &mut hardware,
        TsfInstant::from_micros(reading + allowed(interval) + 1),
    )
    .unwrap();
    let jumped = generation(&core, &mut hardware);
    assert_ne!(jumped, followed);

    // The same BSS again keeps the relation; another BSS (a roam) breaks
    // it.
    core.configure_checked_vif(&mut hardware, station())
        .unwrap();
    assert_eq!(generation(&core, &mut hardware), jumped);
    core.configure_checked_vif(
        &mut hardware,
        CoreVif {
            bssid: Some(OTHER_BSS),
            ..station()
        },
    )
    .unwrap();
    assert_ne!(generation(&core, &mut hardware), jumped);
}

#[test]
fn only_a_disabled_idle_core_comes_apart() {
    let mut hardware = Hardware::default();
    let core = enabled(&mut hardware);
    // An enabled core keeps running: it is returned unchanged.
    let Err(mut core) = core.dismantle() else {
        panic!("an enabled core does not come apart");
    };
    let mut events = Events::default();
    core.disable_checked(&mut events);
    assert_eq!(events.disabled, 1);
    let Ok(parts) = core.dismantle() else {
        panic!("a disabled idle core comes apart");
    };
    assert_eq!(parts.config.channel, channel(6));
    // The rebuilt core is disabled, as at a radio start.
    let mut rebuilt = Core::from_parts(parts);
    assert_eq!(rebuilt.enable_checked(), LifecycleStart::Enabled);
}

#[test]
fn a_disable_ends_once_the_published_attempt_completes() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
    let mut events = Events::default();
    core.disable_checked(&mut events);
    assert_eq!(events.disabled, 0);
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert_eq!(events.completions.len(), 1);
    assert_eq!(events.disabled, 1);
    assert!(core.idle_disabled());
}

#[test]
fn a_core_handed_a_radio_on_another_channel_retunes_at_enable() {
    let mut core = core();
    // The radio start left the PHY on channel 11; the core is configured
    // for channel 6.
    assert!(core.radio_tuned_to(channel(11)));
    assert_eq!(core.enable_checked(), LifecycleStart::Retune(channel(6)));
    // An enabling core records nothing.
    assert!(!core.radio_tuned_to(channel(1)));
}

#[test]
fn the_longest_mpdu_is_bounded_by_the_transmit_programs_too() {
    // A large buffer does not admit a PSDU the legacy program cannot carry.
    let capabilities = esp32s31_lower_mac_capabilities(8192);
    assert_eq!(
        usize::from(capabilities.max_mpdu_length),
        MAX_PROGRAM_PSDU - TX_CCMP_MIC_SIZE - TX_FCS_SIZE
    );
    // The model's buffers stay the bound below it.
    assert!(esp32s31_lower_mac_capabilities(512).max_mpdu_length < capabilities.max_mpdu_length);
}

#[test]
fn an_edca_set_outside_the_limits_leaves_the_installed_one() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.set_checked_edca(wmm_with_best_effort_txop(94))
        .unwrap();
    // A best-effort ECWmax of 11 lies outside the hardware's exponents.
    let record = |aifsn, ecw_max| oer_ieee80211_mac::extensions::wmm::WmmAcParameters {
        admission_control_mandatory: false,
        aifsn,
        ecw_min: 4,
        ecw_max,
        txop_limit_units_32_us: 47,
    };
    let outside = oer_ieee80211_mac::extensions::wmm::WmmParameterSet::new(
        0,
        false,
        [record(3, 11), record(7, 10), record(2, 10), record(2, 10)],
    );
    assert!(core.set_checked_edca(outside).is_err());
    assert_eq!(
        core.tx
            .policy()
            .access_policy(LegacyTxQueue::BestEffort)
            .txop_limit_units_32_us(),
        94
    );
}

/// Subframes of one aggregate.
const SUBFRAMES: usize = 4;
/// Bytes of one subframe backing.
const BACKING: usize = 2048;
const BACKINGS: usize = 8;

type Lease = PinnedDmaTxRadioLease<'static, BACKING, 0, 0>;
type Backing = ReturningStableDmaBacking<Lease, &'static FreeBackings>;

/// The pool indices of free backings.
#[derive(Default)]
struct FreeBackings(RefCell<Vec<u8>>);

impl DmaIndexReturn for &'static FreeBackings {
    fn return_index(&self, index: u8) {
        self.0.borrow_mut().push(index);
    }
}

/// Subframe memory from a pinned DMA TX pool, as the station's network TX
/// path lends it.
struct Backings {
    pool: &'static PinnedDmaTxPool<BACKING, 0, 0, BACKINGS>,
    free: &'static FreeBackings,
}

impl Backings {
    fn free(&self) -> usize {
        self.free.0.borrow().len()
    }
}

impl AmpduBacking for Backings {
    type Backing = Backing;
}

impl AmpduBackingSource for Backings {
    fn backing(&self) -> Option<Backing> {
        let index = self.free.0.borrow_mut().pop()?;
        self.pool.claim_network(index).publish(BACKING, |_| ());
        Some(ReturningStableDmaBacking::new(
            self.pool.claim_radio(index),
            self.free,
        ))
    }
}

fn backings() -> &'static Backings {
    let pool = PinnedDmaTxPool::pin_static(Box::leak(Box::new(PinnedDmaTxPool::new())));
    let free: &'static FreeBackings = Box::leak(Box::default());
    free.0.borrow_mut().extend((0..BACKINGS as u8).rev());
    Box::leak(Box::new(Backings {
        pool: Pin::into_ref(pool).get_ref(),
        free,
    }))
}

fn ampdu_owner() -> Esp32s31AmpduOwner<'static, Backing, SUBFRAMES> {
    RetainedDmaAmpduTx::new_model(
        Pin::static_mut(Box::leak(Box::new(HtAmpduTxStorage::new()))),
        Box::leak(Box::new(RetainedAmpduDmaStorage::new())),
    )
    .unwrap()
}

type Aggregate = Esp32s31AmpduBuffer<'static, Backings, SUBFRAMES>;

fn ht_rate(mcs: u8) -> PhyRate {
    PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(mcs).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
    )
}

fn he_rate() -> PhyRate {
    PhyRate::He(
        phy::HeRate::new(
            phy::HeMcs::new(7).unwrap(),
            SpatialStreams::new(1).unwrap(),
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Ldpc,
            false,
        )
        .unwrap(),
    )
}

/// An aggregate of `count` QoS Data MPDUs of `len` bytes to the BSS, with
/// sequence numbers from 100.
fn aggregate_of(source: &'static Backings, count: usize, len: usize) -> Aggregate {
    let mut buffer = Esp32s31AmpduBuffer::new(ampdu_owner(), source)
        .ok()
        .expect("an idle aggregate owner");
    for index in 0..count {
        let mpdu = buffer.push_mpdu(len).expect("a backing");
        mpdu[..26].copy_from_slice(&data_frame(BSSID));
        mpdu[22..24].copy_from_slice(&((100 + index as u16) << 4).to_le_bytes());
    }
    buffer
}

/// The station's best-effort aggregate values at `rate`.
fn ampdu_values(rate: PhyRate) -> AmpduValues {
    AmpduValues {
        role: VifRole::Station,
        access_category: WmmAccessCategory::BestEffort,
        rate: TxPhyRate::try_from(rate).unwrap(),
        protection: Protection::None,
        key: None,
        power_ceiling_dbm: None,
        backoff_slots: 9,
        coex: CoexPriority::Normal,
        min_mpdu_start_spacing: 5,
    }
}

/// Plan and publish `buffer`: a refused one keeps its subframes, which
/// dropping it returns.
fn publish_ampdu(
    core: &Core,
    hardware: &mut Hardware,
    mut buffer: Aggregate,
    values: AmpduValues,
) -> Result<PublishedAmpdu<'static, Backing, SUBFRAMES>, AmpduRefusal> {
    let plan = core.plan_ampdu(values, &mut buffer)?;
    Ok(ampdu::publish(
        hardware,
        buffer,
        plan,
        core.now(),
        oer_time::Duration::from_micros(TIMEOUT),
    )
    .unwrap())
}

/// Serve a completion edge of the published aggregate: how it ended.
fn end_ampdu(
    core: &Core,
    hardware: &mut Hardware,
    published: PublishedAmpdu<'static, Backing, SUBFRAMES>,
) -> PublicationEnd {
    match ampdu::service(
        hardware,
        published,
        interrupt(EVENT_TX_COMPLETE),
        true,
        core.now(),
    )
    .unwrap()
    {
        AmpduProgress::Complete { outcome, .. } => outcome.end(),
        AmpduProgress::Pending(_) => panic!("the aggregate ends"),
    }
}

fn block_ack_completion(
    status: u8,
    start: u16,
    bitmap: u64,
    received: bool,
) -> MacHtAmpduCompletionObservation {
    MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(status, 0),
        0,
        start,
        bitmap,
        received,
    )
}

#[test]
fn an_aggregate_is_planned_published_once_and_reports_its_block_ack() {
    let mut hardware = Hardware::default();
    let core = enabled(&mut hardware);
    let source = backings();
    let capabilities = esp32s31_ampdu_capabilities(SUBFRAMES);
    assert_eq!(capabilities.max_length, 6_490);
    assert!(
        !capabilities
            .formats
            .contains_rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
    );

    let buffer = aggregate_of(source, 3, 26);
    assert_eq!(source.free(), BACKINGS - 3);
    let published = publish_ampdu(&core, &mut hardware, buffer, ampdu_values(ht_rate(7))).unwrap();
    assert_eq!(hardware.ht.len(), 1);
    assert_eq!(usize::from(hardware.ht[0].0), BE);
    assert_eq!(hardware.ht[0].1.control().protection, MacTxProtection::None);
    assert_eq!(published.owner().frame_count(), 3);
    assert_eq!(published.owner().work().backoff_slots, 9);
    assert_eq!(
        published.deadline(),
        oer_time::Instant::from_micros(TIMEOUT)
    );

    // A BlockAck arrived: the end carries its window, and the subframes go
    // back to their source.
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b101, true));
    assert_eq!(
        end_ampdu(&core, &mut hardware, published),
        PublicationEnd::Completed {
            status: TxStatus::Success,
            ack_snr_db: Some(0x60),
            block_ack: Some(BlockAckReport {
                start_sequence: SequenceNumber::new(100).unwrap(),
                bitmap: 0b101,
            }),
        }
    );
    assert_eq!(source.free(), BACKINGS);

    // The BlockAck result decides, not the completion status; without it
    // the stale bitmap words are not reported.
    for (status, received, expected) in [
        (5, true, TxStatus::Success),
        (5, false, TxStatus::AckTimeout),
        (0, false, TxStatus::AckTimeout),
        (2, false, TxStatus::CtsTimeout),
    ] {
        let buffer = aggregate_of(source, 2, 26);
        let published =
            publish_ampdu(&core, &mut hardware, buffer, ampdu_values(ht_rate(7))).unwrap();
        hardware.block_ack_completion[BE] = Some(block_ack_completion(status, 100, 0b01, received));
        let end = end_ampdu(&core, &mut hardware, published);
        assert_eq!(status_of(&end), Some(expected));
        assert_eq!(
            matches!(
                end,
                PublicationEnd::Completed {
                    block_ack: Some(_),
                    ..
                }
            ),
            received
        );
    }
    // One publication per aggregate, never a retained retry.
    assert_eq!(hardware.ht.len(), 5);
    assert_eq!(source.free(), BACKINGS);
}

#[test]
fn he_aggregates_publish_an_he_program_with_the_station_bss_color() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let source = backings();
    assert!(
        esp32s31_ampdu_capabilities(SUBFRAMES)
            .formats
            .contains_rate(he_rate())
    );
    core.set_checked_he_bss_color(0x2a);

    let mut buffer = aggregate_of(source, 3, 26);
    let plan = core
        .plan_ampdu(ampdu_values(he_rate()), &mut buffer)
        .unwrap();
    assert!(matches!(
        plan.format,
        AmpduFormat::He {
            bss_color: 0x2a,
            ..
        }
    ));
    let published = ampdu::publish(
        &mut hardware,
        buffer,
        plan,
        core.now(),
        oer_time::Duration::from_micros(TIMEOUT),
    )
    .unwrap();
    assert_eq!(hardware.he.len(), 1);
    assert_eq!(usize::from(hardware.he[0].0), BE);
    assert!(hardware.ht.is_empty());
    assert_eq!(published.owner().frame_count(), 3);

    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b111, true));
    assert_eq!(
        status_of(&end_ampdu(&core, &mut hardware, published)),
        Some(TxStatus::Success)
    );
    assert_eq!(source.free(), BACKINGS);
}

#[test]
fn an_access_category_with_a_txop_limit_takes_ht_and_he_aggregates() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let source = backings();
    core.set_checked_edca(wmm_with_best_effort_txop(94))
        .unwrap();
    // The caller keeps an aggregate within the limit: the core refuses
    // neither format by it.
    let published = publish_ampdu(
        &core,
        &mut hardware,
        aggregate_of(source, 2, 26),
        ampdu_values(ht_rate(7)),
    )
    .unwrap();
    assert_eq!(hardware.ht.len(), 1);
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b11, true));
    end_ampdu(&core, &mut hardware, published);
    publish_ampdu(
        &core,
        &mut hardware,
        aggregate_of(source, 2, 26),
        ampdu_values(he_rate()),
    )
    .unwrap();
    assert_eq!(hardware.he.len(), 1);
}

#[test]
fn aggregates_outside_the_limits_are_refused_with_their_subframes() {
    let mut hardware = Hardware::default();
    let core = enabled(&mut hardware);
    let source = backings();
    let refused = |values: AmpduValues, buffer: Aggregate, hardware: &mut Hardware| {
        publish_ampdu(&core, hardware, buffer, values).err()
    };

    // Non-HT aggregates are outside the declared formats; a start spacing
    // above 7 is no encoding; a connection frame's level is not data's.
    let legacy = ampdu_values(PhyRate::Legacy(LegacyRate::Ofdm24M));
    assert_eq!(
        refused(legacy, aggregate_of(source, 2, 26), &mut hardware),
        Some(AmpduRefusal::LegacyRate)
    );
    let mut spacing = ampdu_values(ht_rate(7));
    spacing.min_mpdu_start_spacing = 8;
    assert_eq!(
        refused(spacing, aggregate_of(source, 2, 26), &mut hardware),
        Some(AmpduRefusal::StartSpacing)
    );
    let mut elevated = ampdu_values(ht_rate(7));
    elevated.coex = CoexPriority::Elevated;
    assert_eq!(
        refused(elevated, aggregate_of(source, 2, 26), &mut hardware),
        Some(AmpduRefusal::ElevatedCoexistence)
    );

    // An empty aggregate, a group receiver and one longer than the declared
    // maximum length.
    assert_eq!(
        refused(
            ampdu_values(ht_rate(7)),
            aggregate_of(source, 0, 26),
            &mut hardware
        ),
        Some(AmpduRefusal::Empty)
    );
    let mut group = aggregate_of(source, 0, 26);
    group
        .push_mpdu(26)
        .unwrap()
        .copy_from_slice(&data_frame([0xff; 6]));
    assert_eq!(
        refused(ampdu_values(ht_rate(7)), group, &mut hardware),
        Some(AmpduRefusal::GroupRecipient)
    );
    assert_eq!(
        refused(
            ampdu_values(ht_rate(7)),
            aggregate_of(source, 4, 2_000),
            &mut hardware
        ),
        Some(AmpduRefusal::Length)
    );
    let fits = publish_ampdu(
        &core,
        &mut hardware,
        aggregate_of(source, 3, 2_000),
        ampdu_values(ht_rate(7)),
    )
    .unwrap();
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b111, true));
    end_ampdu(&core, &mut hardware, fits);

    // No more subframes than the owner's slots, nor MPDUs the backings
    // cannot hold with their metadata, MIC and FCS.
    let mut buffer = aggregate_of(source, 0, 26);
    for _ in 0..SUBFRAMES {
        assert!(buffer.push_mpdu(26).is_some());
    }
    assert!(buffer.push_mpdu(26).is_none());
    assert_eq!(buffer.subframes(), SUBFRAMES);
    drop(buffer.into_owner());
    let mut buffer = aggregate_of(source, 0, 26);
    assert!(buffer.push_mpdu(BACKING - 20).is_some());
    assert!(buffer.push_mpdu(BACKING - 19).is_none());
    drop(buffer);

    // Refused and dropped aggregates published nothing and kept no
    // backing.
    assert_eq!(hardware.ht.len(), 1);
    assert_eq!(source.free(), BACKINGS);
}

#[test]
fn the_backoff_is_drawn_from_the_category_s_contention_window() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    // The vendor defaults: best effort's CWmin is 15 slots.
    for _ in 0..16 {
        assert!(core.draw_backoff(WmmAccessCategory::BestEffort) <= 15);
    }
}
