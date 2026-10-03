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
use oer_ieee80211_lower_mac::{
    AmpduPayload, HardwareServices, LowerMacEvent, PhyRate, RxEvidence, TxAttempt, time_units,
};
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
use oer_time::Duration;

use super::*;
use crate::ordinary_tx::{WifiTxPowerPair, WifiTxResources};
use crate::station_tsf::StationTsfWrite;
use oer_ieee80211_lower_mac::TSF_DRIFT_PPM;

const STATION: MacAddress = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const ACCESS_POINT: MacAddress = [0x02, 0x11, 0x12, 0x13, 0x14, 0x15];
const BSSID: MacAddress = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const PEER: MacAddress = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
const OTHER_BSS: MacAddress = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45];
const STA: VifId = VifId(0);
const AP: VifId = VifId(1);
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
    fn reset_and_start_access_point_tsf(&mut self) {
        self.access_point_tsf_resets += 1;
    }

    fn stop_access_point_tsf(&mut self) {
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

const SPARE: usize = 4;
/// Subframes of one aggregate.
const SUBFRAMES: usize = 4;
/// Bytes of one subframe backing.
const BACKING: usize = 2048;
const BACKINGS: usize = 8;

type Lease = PinnedDmaTxRadioLease<'static, BACKING, 0, 0>;
type Backing = ReturningStableDmaBacking<Lease, &'static FreeBackings>;
/// A request the core must refuse: the frame, the edit that makes it
/// invalid and the refusal.
type MpduRefusal<'a> = (
    &'a [u8],
    fn(&mut Esp32s31MpduAttempt<'static, 512>),
    SubmitError,
);
/// An aggregate edit the core must refuse, and the refusal.
type AmpduRefusal = (
    fn(&mut Esp32s31AmpduAttempt<'static, Backings, SUBFRAMES>),
    SubmitError,
);

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

type Core = LowerMacCore<'static, Power, fn() -> u32, Timer, 512, SPARE, Backings, SUBFRAMES, 2>;

#[derive(Default)]
struct Events {
    completions: Vec<TxCompletion>,
    lifecycle: Vec<LifecycleEvent>,
    tbtts: Vec<TbttEvent>,
}

impl LowerMacSink for Events {
    fn tx_completed(&mut self, completion: TxCompletion) {
        self.completions.push(completion);
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        self.lifecycle.push(event);
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
    Pin::static_mut(Box::leak(Box::new(TxSlot::new_model())))
}

fn core() -> Core {
    LowerMacCore::with_ampdu(
        OrdinaryTxOwner::new(WifiTxResources {
            slot: slot(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: Timer::default(),
        }),
        [slot(), slot(), slot(), slot()],
        [ampdu_owner(), ampdu_owner()],
        backings(),
        LowerMacConfig {
            station_address: STATION,
            channel: channel(6),
            publication_timeout: oer_time::Duration::from_micros(TIMEOUT),
            tsf_epoch: 1,
        },
    )
}

fn station() -> VifConfig {
    VifConfig {
        address: STATION,
        role: VifRole::Station,
        bssid: Some(BSSID),
        receive: ReceiveFilter::BSS_MEMBER,
    }
}

fn access_point() -> VifConfig {
    VifConfig {
        address: ACCESS_POINT,
        role: VifRole::AccessPoint,
        bssid: None,
        receive: ReceiveFilter::BSS_MEMBER,
    }
}

/// An enabled core with a station interface on channel 6.
fn enabled(hardware: &mut Hardware) -> Core {
    let mut core = core();
    assert_eq!(
        core.apply(
            hardware,
            LowerMacSetting::Vif {
                vif: STA,
                config: Some(station()),
            }
        ),
        Ok(Ok(()))
    );
    let mut events = Events::default();
    assert_eq!(
        core.lifecycle(LifecycleCommand::Enable, &mut events),
        Ok(LifecycleStart::Admitted)
    );
    assert_eq!(events.lifecycle, [LifecycleEvent::Enabled]);
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

/// An attempt carrying `frame` in a slot the core lends.
fn attempt(core: &mut Core, id: u32, frame: &[u8]) -> Esp32s31MpduAttempt<'static, 512> {
    let mut buffer = core.tx_buffer(frame.len()).expect("a spare slot");
    buffer.frame_mut().copy_from_slice(frame);
    TxAttempt {
        id: TxId(id),
        vif: STA,
        access_category: WmmAccessCategory::BestEffort,
        payload: TxPayload {
            frame: buffer,
            response: TxResponse::Ack,
        },
        rate: PhyRate::Legacy(LegacyRate::Ofdm24M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(7),
        coex: CoexPriority::Normal,
    }
}

/// Submit and hand a refused attempt's slot back to the core.
fn submit(
    core: &mut Core,
    hardware: &mut Hardware,
    attempt: Esp32s31MpduAttempt<'static, 512>,
) -> Result<Result<(), SubmitError>, LowerMacFault> {
    Ok(match core.submit(hardware, attempt)? {
        Ok(()) => Ok(()),
        Err(refused) => {
            core.release_tx_buffer(refused.attempt.payload.frame);
            Err(refused.error)
        }
    })
}

fn interrupt(events: u32) -> WifiTxWake {
    WifiTxWake::Interrupt { events }
}

/// Deliver a completion with `status` and return the completion event.
fn complete(core: &mut Core, hardware: &mut Hardware, status: u8) -> Vec<TxCompletion> {
    complete_with(core, hardware, status, 0)
}

fn complete_with(
    core: &mut Core,
    hardware: &mut Hardware,
    status: u8,
    detail: u8,
) -> Vec<TxCompletion> {
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(status, detail));
    let mut events = Events::default();
    core.service(hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    events.completions
}

fn spare_slots(core: &Core) -> usize {
    core.spare.iter().flatten().count()
}

/// The single attempt published on queue `index`.
fn queued(core: &Core, index: usize) -> &QueuedSingleAttempt<'static, 512> {
    match core.queues[index].as_ref().map(|attempt| &attempt.work) {
        Some(Work::Mpdu(queued)) => queued,
        _ => panic!("queue {index} holds a published MPDU"),
    }
}

#[test]
fn capabilities_are_the_s31_limits_on_2_4_ghz() {
    let caps = esp32s31_lower_mac_capabilities(512);
    assert_eq!(caps, core().capabilities());
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
    assert!(!caps.coex_priorities.contains(CoexPriority::Elevated));
    assert!(
        caps.individual_no_ack
            .contains_rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(caps.access_point_receive_filters, ReceiveFilter::BSS_MEMBER);
    assert_eq!(usize::from(caps.key_slots), LOWER_MAC_KEY_SLOTS);
    assert_eq!(caps.rx_block_ack_agreements, RESOURCES.rx_block_ack_entries);
}

#[test]
fn an_attempt_is_published_from_the_slot_it_was_written_in() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = attempt(&mut core, 1, &frame);
    let lent = request.payload.frame.slot.as_ref().buffer_address();
    assert_eq!(spare_slots(&core), SPARE - 1);
    assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
    // The queue publishes the lent slot; it is lent again after completion.
    assert_eq!(queued(&core, BE).slot().buffer_address(), lent);
    assert_eq!(spare_slots(&core), SPARE - 1);
    assert_eq!(hardware.legacy.len(), 1);
    assert_eq!(hardware.legacy[0].1.interface(), MacInterface::Station);
    assert_eq!(
        core.next_deadline(),
        Some(oer_time::Instant::from_micros(TIMEOUT))
    );

    let completions = complete(&mut core, &mut hardware, 0);
    assert_eq!(
        completions,
        [TxCompletion {
            id: TxId(1),
            status: TxStatus::Success,
            ack_rssi_dbm: None,
            ack_snr_db: Some(0x60),
            block_ack: None,
        }]
    );
    assert_eq!(core.next_deadline(), None);
    assert_eq!(spare_slots(&core), SPARE);
    let next = attempt(&mut core, 2, &frame);
    assert_eq!(submit(&mut core, &mut hardware, next), Ok(Ok(())));
}

#[test]
fn buffers_are_bounded_and_a_released_one_is_lent_again() {
    let mut core = core();
    let longest = usize::from(core.capabilities().max_mpdu_length);
    assert!(core.tx_buffer(longest + 1).is_none());
    let first = core.tx_buffer(longest).unwrap();
    let rest: Vec<_> = (1..SPARE).map(|_| core.tx_buffer(24).unwrap()).collect();
    assert!(core.tx_buffer(24).is_none());
    core.release_tx_buffer(first);
    for buffer in rest {
        core.release_tx_buffer(buffer);
    }
    assert_eq!(spare_slots(&core), SPARE);
    assert!(core.tx_buffer(24).is_some());
}

#[test]
fn the_callers_backoff_is_counted_down_instead_of_a_draw() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    for (id, slots) in [(1, 0), (2, 700), (3, 1023)] {
        let mut request = attempt(&mut core, id, &frame);
        request.backoff = Backoff::Slots(slots);
        submit(&mut core, &mut hardware, request).unwrap().unwrap();
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
    for (id, power, data, control) in [
        (1, TxPower::Calibrated, 5, (5, 6)),
        (2, TxPower::MaxDbm(3), 3, (3, 3)),
        (3, TxPower::MaxDbm(10), 5, (5, 6)),
        (4, TxPower::MaxDbm(0), 0, (0, 0)),
    ] {
        let mut request = attempt(&mut core, id, &frame);
        request.power = power;
        request.protection = Protection::RtsCts;
        submit(&mut core, &mut hardware, request).unwrap().unwrap();
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
    // attempt after its single publication; the owner's ladder would
    // re-publish the first three.
    for (id, status, detail, expected) in [
        (1, 5, 0, TxStatus::AckTimeout),
        (2, 2, 0, TxStatus::CtsTimeout),
        (3, 4, 1, TxStatus::Collision),
        (4, 4, 0xc0, TxStatus::Fault(TxFault::KeyUnavailable)),
    ] {
        let published = hardware.publications();
        let request = attempt(&mut core, id, &frame);
        assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
        let completions = complete_with(&mut core, &mut hardware, status, detail);
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, TxId(id));
        assert_eq!(completions[0].status, expected);
        assert_eq!(completions[0].ack_snr_db, None);
        assert_eq!(hardware.publications(), published + 1);
    }
    // The Retry bit the ladder sets before a re-publication is untouched.
    let published = core
        .spare
        .iter_mut()
        .flatten()
        .map(|slot| slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1])
        .filter(|flags| *flags != 0)
        .collect::<Vec<_>>();
    assert!(!published.is_empty());
    assert!(published.iter().all(|flags| *flags == 0x01));
}

#[test]
fn a_collision_detach_ends_the_attempt() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = attempt(&mut core, 7, &frame);
    submit(&mut core, &mut hardware, request).unwrap().unwrap();
    hardware.collision_pending[BE] = true;
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_COLLISION), &mut events)
        .unwrap();
    assert_eq!(events.completions.len(), 1);
    assert_eq!(events.completions[0].status, TxStatus::Collision);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_hardware_timeout_ends_the_attempt_as_aborted() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    let request = attempt(&mut core, 3, &frame);
    submit(&mut core, &mut hardware, request).unwrap().unwrap();
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
    assert_eq!(events.completions[0].status, TxStatus::Aborted);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_block_ack_request_reports_its_block_ack() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let mut frame = [0; 24];
    frame[0] = BLOCK_ACK_REQUEST_FRAME_CONTROL;
    frame[4..10].copy_from_slice(&BSSID);
    let mut request = attempt(&mut core, 4, &frame);
    request.payload.response = TxResponse::BlockAck;

    submit(&mut core, &mut hardware, request).unwrap().unwrap();
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
    assert_eq!(
        events.completions[0].block_ack,
        Some(BlockAckReport {
            start_sequence: SequenceNumber::new(100).unwrap(),
            bitmap: 0b1011,
        })
    );

    // A BlockAckReq at an HT rate has no single-frame BlockAck program.
    let mut request = attempt(&mut core, 5, &frame);
    request.payload.response = TxResponse::BlockAck;
    request.rate = PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(0).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
    );
    assert_eq!(
        submit(&mut core, &mut hardware, request),
        Ok(Err(SubmitError::UnsupportedRate))
    );
}

#[test]
fn ht_and_he_attempts_use_their_programs_and_unsendable_rates_are_refused() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    let ht = |mcs, bandwidth| {
        PhyRate::Ht(phy::HtRate::new(phy::HtMcs::new(mcs).unwrap(), bandwidth, true).unwrap())
    };
    let he = |streams, bandwidth| {
        PhyRate::He(
            phy::HeRate::new(
                phy::HeMcs::new(7).unwrap(),
                SpatialStreams::new(streams).unwrap(),
                bandwidth,
                HeGiLtf::Ltf2xGi800Ns,
                FecCoding::Ldpc,
                false,
            )
            .unwrap(),
        )
    };

    let mut ht_attempt = attempt(&mut core, 1, &frame);
    ht_attempt.rate = ht(7, PpduBandwidth::Mhz20);
    submit(&mut core, &mut hardware, ht_attempt)
        .unwrap()
        .unwrap();
    assert_eq!(hardware.ht.len(), 1);
    complete(&mut core, &mut hardware, 0);

    let mut he_attempt = attempt(&mut core, 2, &frame);
    he_attempt.rate = he(1, PpduBandwidth::Mhz20);
    submit(&mut core, &mut hardware, he_attempt)
        .unwrap()
        .unwrap();
    assert_eq!(hardware.he.len(), 1);
    complete(&mut core, &mut hardware, 0);

    for rate in [
        // Two spatial streams.
        ht(8, PpduBandwidth::Mhz20),
        // Forty megahertz on a twenty-megahertz channel.
        ht(7, PpduBandwidth::Mhz40),
        he(2, PpduBandwidth::Mhz20),
        he(1, PpduBandwidth::Mhz40),
    ] {
        let mut refused = attempt(&mut core, 3, &frame);
        refused.rate = rate;
        assert_eq!(
            submit(&mut core, &mut hardware, refused),
            Ok(Err(SubmitError::UnsupportedRate))
        );
    }
    assert_eq!(hardware.publications(), 2);
}

#[test]
fn the_callers_protection_is_published() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);

    for (id, protection, expected) in [
        (1, Protection::RtsCts, MacTxProtection::RtsCts),
        (2, Protection::CtsToSelf, MacTxProtection::CtsToSelf),
        (3, Protection::None, MacTxProtection::None),
    ] {
        let mut request = attempt(&mut core, id, &frame);
        request.protection = protection;
        submit(&mut core, &mut hardware, request).unwrap().unwrap();
        assert_eq!(
            hardware.legacy.last().unwrap().1.control().protection,
            expected
        );
        complete(&mut core, &mut hardware, 0);
    }
}

#[test]
fn submissions_outside_the_limits_are_refused_without_publication() {
    let mut hardware = Hardware::default();
    let mut core = core();
    let frame = data_frame(BSSID);
    let group = data_frame([0xff; 6]);

    let request = attempt(&mut core, 1, &frame);
    assert_eq!(
        submit(&mut core, &mut hardware, request),
        Ok(Err(SubmitError::Disabled))
    );
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: STA,
            config: Some(station()),
        },
    )
    .unwrap()
    .unwrap();
    core.lifecycle(LifecycleCommand::Enable, &mut Events::default())
        .unwrap();

    let ht = PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(0).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
    );
    let refusals: [MpduRefusal<'_>; 10] = [
        (&frame, |request| request.vif = AP, SubmitError::UnknownVif),
        (
            &frame,
            |request| request.backoff = Backoff::HardwareDraw { cw_exponent: 4 },
            SubmitError::Unsupported,
        ),
        (
            &frame,
            |request| request.backoff = Backoff::Slots(1024),
            SubmitError::Unsupported,
        ),
        (
            &frame,
            |request| request.power = TxPower::MaxDbm(-1),
            SubmitError::Unsupported,
        ),
        (
            &frame,
            |request| request.coex = CoexPriority::Elevated,
            SubmitError::Unsupported,
        ),
        (&group, |_| {}, SubmitError::Unsupported),
        (&frame[..9], |_| {}, SubmitError::InvalidLength),
        (
            &frame,
            |request| request.key = KeySelector::Key(KeyHandle(0)),
            SubmitError::UnknownKey,
        ),
        (
            &frame,
            |request| request.payload.response = TxResponse::BlockAck,
            SubmitError::Unsupported,
        ),
        (&frame, |_| {}, SubmitError::Busy),
    ];
    for (index, (bytes, mutate, expected)) in refusals.into_iter().enumerate() {
        if expected == SubmitError::Busy {
            let first = attempt(&mut core, 100, &frame);
            submit(&mut core, &mut hardware, first).unwrap().unwrap();
            let duplicate = attempt(&mut core, 100, &frame);
            assert_eq!(
                submit(&mut core, &mut hardware, duplicate),
                Ok(Err(SubmitError::DuplicateId))
            );
        }
        let mut request = attempt(&mut core, index as u32, bytes);
        mutate(&mut request);
        assert_eq!(
            submit(&mut core, &mut hardware, request),
            Ok(Err(expected)),
            "refusal {index}"
        );
        assert_eq!(
            hardware.publications(),
            usize::from(expected == SubmitError::Busy)
        );
    }
    // A unicast frame without acknowledgement only at a legacy rate.
    complete(&mut core, &mut hardware, 0);
    let mut no_ack = attempt(&mut core, 200, &frame);
    no_ack.payload.response = TxResponse::None;
    no_ack.rate = ht;
    assert_eq!(
        submit(&mut core, &mut hardware, no_ack),
        Ok(Err(SubmitError::Unsupported))
    );
    // Every refused slot came back.
    assert_eq!(spare_slots(&core), SPARE);
}

#[test]
fn a_legacy_unicast_frame_is_sent_without_acknowledgement_on_request() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    let mut request = attempt(&mut core, 1, &frame);
    request.payload.response = TxResponse::None;
    submit(&mut core, &mut hardware, request).unwrap().unwrap();
    assert_eq!(hardware.legacy[0].1.response(), MacLegacyTxResponse::None);
    assert_eq!(
        complete(&mut core, &mut hardware, 0)[0].status,
        TxStatus::Success
    );
}

#[test]
fn a_group_frame_solicits_no_response() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame([0xff; 6]);
    let mut request = attempt(&mut core, 1, &frame);
    request.payload.response = TxResponse::None;
    assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
    assert_eq!(hardware.legacy[0].1.response(), MacLegacyTxResponse::None);
    assert_eq!(
        complete(&mut core, &mut hardware, 0)[0].status,
        TxStatus::Success
    );
}

#[test]
fn channels_outside_2_4_ghz_are_refused_and_enable_retunes() {
    let mut hardware = Hardware::default();
    let mut core = core();
    let mut events = Events::default();

    assert_eq!(
        core.apply(
            &mut hardware,
            LowerMacSetting::Channel(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap())
        ),
        Ok(Err(SettingError::UnsupportedChannel))
    );
    let eleven = Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap();
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::Channel(eleven)),
        Ok(Ok(()))
    );
    assert_eq!(
        core.lifecycle(LifecycleCommand::Enable, &mut events),
        Ok(LifecycleStart::Retune(channel(11)))
    );
    assert!(events.lifecycle.is_empty());
    assert_eq!(
        core.lifecycle(LifecycleCommand::Enable, &mut events),
        Err(LifecycleError::AlreadyInState)
    );
    core.finish_retune(true, &mut events);
    assert_eq!(events.lifecycle, [LifecycleEvent::Enabled]);
    assert_eq!(core.channel(), eleven);

    // Retuning needs the MAC stopped.
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::Channel(eleven)),
        Ok(Err(SettingError::Busy))
    );
    core.lifecycle(LifecycleCommand::Disable, &mut events)
        .unwrap();
    // The radio is already on channel 11: no retune.
    assert_eq!(
        core.lifecycle(LifecycleCommand::Enable, &mut events),
        Ok(LifecycleStart::Admitted)
    );
    assert_eq!(
        events.lifecycle,
        [
            LifecycleEvent::Enabled,
            LifecycleEvent::Disabled,
            LifecycleEvent::Enabled
        ]
    );
}

#[test]
fn a_failed_retune_fails_enable_recoverably_and_leaves_the_port_disabled() {
    let mut hardware = Hardware::default();
    let mut core = core();
    let mut events = Events::default();
    core.apply(
        &mut hardware,
        LowerMacSetting::Channel(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap()),
    )
    .unwrap()
    .unwrap();
    core.lifecycle(LifecycleCommand::Enable, &mut events)
        .unwrap();
    core.finish_retune(false, &mut events);
    assert_eq!(
        events.lifecycle,
        [LifecycleEvent::Failed {
            command: LifecycleCommand::Enable,
            class: oer_ieee80211_lower_mac::FailureClass::Recoverable,
        }]
    );
    assert_eq!(core.state, PortState::Disabled);
    // The channel stays configured: the next Enable retunes again.
    assert_eq!(
        core.lifecycle(LifecycleCommand::Enable, &mut events),
        Ok(LifecycleStart::Retune(channel(1)))
    );
}

#[test]
fn receive_filters_map_onto_the_smallest_superset_policy() {
    let mut hardware = Hardware::default();
    let mut core = core();
    let mut configure = |hardware: &mut Hardware, vif, config| {
        core.apply(hardware, LowerMacSetting::Vif { vif, config })
    };

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
        configure(
            &mut hardware,
            STA,
            Some(VifConfig {
                receive,
                bssid,
                ..station()
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(hardware.station_policy, Some(expected));
    }
    configure(&mut hardware, AP, Some(access_point()))
        .unwrap()
        .unwrap();
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // Rules no policy provides a superset of, a second station, a station
    // address other than the published one and an access point whose BSSID
    // is not its address are refused before any register write.
    let refused = [
        (
            STA,
            VifConfig {
                bssid: None,
                ..station()
            },
            SettingError::Unsupported,
        ),
        (
            STA,
            VifConfig {
                address: PEER,
                ..station()
            },
            SettingError::Unsupported,
        ),
        (
            AP,
            VifConfig {
                receive: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
                ..access_point()
            },
            SettingError::Unsupported,
        ),
        (
            AP,
            VifConfig {
                bssid: Some(PEER),
                ..access_point()
            },
            SettingError::Unsupported,
        ),
        (AP, station(), SettingError::UnsupportedRole),
        (VifId(2), station(), SettingError::UnknownVif),
    ];
    for (vif, config, expected) in refused {
        assert_eq!(
            configure(&mut hardware, vif, Some(config)),
            Ok(Err(expected))
        );
    }
    assert_eq!(
        hardware.station_policy,
        Some(StationPolicy::OtherBss([0xff; 6]))
    );
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // No receive filter closes the role's context; removal closes it too.
    configure(
        &mut hardware,
        STA,
        Some(VifConfig {
            receive: ReceiveFilter::NONE,
            bssid: None,
            ..station()
        }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(hardware.station_disabled, 1);
    configure(&mut hardware, AP, None).unwrap().unwrap();
    assert_eq!(hardware.access_point_disabled, 1);
    assert_eq!(
        configure(&mut hardware, AP, None),
        Ok(Err(SettingError::UnknownVif))
    );
}

fn normalized(mpdu: &[u8]) -> NormalizedRxFrame<'_> {
    NormalizedRxFrame {
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

    assert!(core.received(&normalized(&unicast)).is_none());
    // Unicast only: the link policy passes the BSS's beacons as well.
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: STA,
            config: Some(VifConfig {
                receive: ReceiveFilter::OWN_UNICAST,
                ..station()
            }),
        },
    )
    .unwrap()
    .unwrap();
    core.lifecycle(LifecycleCommand::Enable, &mut Events::default())
        .unwrap();
    let (bytes, meta) = core.received(&normalized(&unicast)).unwrap();
    assert_eq!(bytes, unicast);
    assert_eq!(meta.channel, Channel::from_wifi_channel(channel(6)));
    assert_eq!(meta.rssi_dbm, RxEvidence::HardwareObserved(-42));
    assert_eq!(meta.noise_floor_dbm, RxEvidence::Unavailable);
    let view = LowerMacEvent::Received { frame: bytes, meta };
    assert!(matches!(view, LowerMacEvent::Received { .. }));
    assert!(core.received(&normalized(&beacon)).is_none());

    // Scanning while joined: the ESP-NOW policy passes everything the
    // filter names, and nothing else is delivered.
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: STA,
            config: Some(VifConfig {
                receive: ReceiveFilter::OWN_BSS_BEACONS.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
                ..station()
            }),
        },
    )
    .unwrap()
    .unwrap();
    assert!(core.received(&normalized(&beacon)).is_some());
    assert!(core.received(&normalized(&other_beacon)).is_some());
    assert!(core.received(&normalized(&unicast)).is_none());
}

#[test]
fn monitor_reception_runs_only_while_no_interface_receives() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let other_beacon = management(8, [0xff; 6], OTHER_BSS);

    assert_eq!(
        core.set_monitor(&mut hardware, true),
        Err(SettingError::Unsupported)
    );
    assert!(!hardware.promiscuous);
    let quiet = LowerMacSetting::Vif {
        vif: STA,
        config: Some(VifConfig {
            receive: ReceiveFilter::NONE,
            ..station()
        }),
    };
    core.apply(&mut hardware, quiet).unwrap().unwrap();
    assert_eq!(core.set_monitor(&mut hardware, true), Ok(()));
    assert!(hardware.promiscuous);
    assert!(core.received(&normalized(&other_beacon)).is_some());
    // An interface cannot start receiving under the promiscuous policy.
    assert_eq!(
        core.apply(
            &mut hardware,
            LowerMacSetting::Vif {
                vif: STA,
                config: Some(station()),
            }
        ),
        Ok(Err(SettingError::Unsupported))
    );
    assert_eq!(core.set_monitor(&mut hardware, false), Ok(()));
    assert!(!hardware.promiscuous);
    assert!(core.received(&normalized(&other_beacon)).is_none());
}

#[test]
fn keys_install_into_their_role_slots_and_protect_attempts() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: AP,
            config: Some(access_point()),
        },
    )
    .unwrap()
    .unwrap();
    let install = |vif, scope, key: &'static [u8]| KeyInstall {
        vif,
        cipher: Cipher::Ccmp128,
        scope,
        key,
    };
    static KEY: [u8; 16] = [0x5a; 16];
    static SHORT: [u8; 15] = [0x5a; 15];

    let pairwise = core
        .install_key(
            &mut hardware,
            install(STA, KeyScope::Pairwise { peer: BSSID }, &KEY),
        )
        .unwrap();
    assert_eq!(hardware.installed_keys[0].0, 4);
    assert_eq!(
        core.install_key(
            &mut hardware,
            install(STA, KeyScope::Pairwise { peer: BSSID }, &KEY)
        ),
        Err(SettingError::NoKeySlot)
    );
    assert_eq!(
        core.install_key(
            &mut hardware,
            install(STA, KeyScope::Group { key_id: 1 }, &SHORT)
        ),
        Err(SettingError::InvalidKey)
    );
    assert_eq!(
        core.install_key(
            &mut hardware,
            install(STA, KeyScope::Group { key_id: 4 }, &KEY)
        ),
        Err(SettingError::InvalidKey)
    );
    assert_eq!(
        core.install_key(
            &mut hardware,
            install(VifId(1), KeyScope::Pairwise { peer: PEER }, &KEY)
        )
        .map(|_| ()),
        Ok(())
    );
    // The access point's first pairwise key takes the slot of association
    // identifier one.
    assert_eq!(hardware.installed_keys[1].0, 8);

    let frame = data_frame(BSSID);
    let mut protected = attempt(&mut core, 1, &frame);
    protected.key = KeySelector::Key(pairwise);
    submit(&mut core, &mut hardware, protected)
        .unwrap()
        .unwrap();
    assert_eq!(hardware.legacy.len(), 1);
    // The attempt's key cannot go while the attempt is in flight.
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::RemoveKey(pairwise)),
        Ok(Err(SettingError::Busy))
    );
    complete(&mut core, &mut hardware, 0);
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::RemoveKey(pairwise)),
        Ok(Ok(()))
    );
    assert_eq!(hardware.cleared_keys, [4]);
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::RemoveKey(pairwise)),
        Ok(Err(SettingError::UnknownKey))
    );
    let mut stale = attempt(&mut core, 2, &frame);
    stale.key = KeySelector::Key(pairwise);
    assert_eq!(
        submit(&mut core, &mut hardware, stale),
        Ok(Err(SubmitError::UnknownKey))
    );

    // Removing an interface clears its keys.
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: AP,
            config: None,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(hardware.cleared_keys, [4, 8]);
}

#[test]
fn receive_block_ack_agreements_use_the_ordinary_banks() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let agreement = |tid, window| RxBlockAckAgreement {
        vif: STA,
        peer: BSSID,
        tid,
        start_sequence: SequenceNumber::new(10).unwrap(),
        window,
    };

    assert_eq!(
        core.apply(
            &mut hardware,
            LowerMacSetting::AddRxBlockAck(agreement(0, 32))
        ),
        Ok(Ok(()))
    );
    assert_eq!(hardware.rx_block_acks[0].interface, MacInterface::Station);
    assert_eq!(hardware.rx_block_acks[0].window, 32);
    for refused in [
        agreement(0, 32),
        agreement(8, 32),
        agreement(1, 0),
        agreement(1, RESOURCES.rx_block_ack_max_window + 1),
    ] {
        assert_eq!(
            core.apply(&mut hardware, LowerMacSetting::AddRxBlockAck(refused)),
            Ok(Err(SettingError::InvalidBlockAck))
        );
    }
    for tid in 1..RESOURCES.rx_block_ack_entries {
        core.apply(
            &mut hardware,
            LowerMacSetting::AddRxBlockAck(agreement(tid, 8)),
        )
        .unwrap()
        .unwrap();
    }
    let mut unknown_vif = agreement(0, 8);
    unknown_vif.vif = AP;
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::AddRxBlockAck(unknown_vif)),
        Ok(Err(SettingError::UnknownVif))
    );
    let mut other_peer = agreement(0, 8);
    other_peer.peer = PEER;
    assert_eq!(
        core.apply(&mut hardware, LowerMacSetting::AddRxBlockAck(other_peer)),
        Ok(Err(SettingError::NoBlockAckSlot))
    );

    let remove = |tid| LowerMacSetting::RemoveRxBlockAck {
        vif: STA,
        peer: BSSID,
        tid,
    };
    assert_eq!(core.apply(&mut hardware, remove(0)), Ok(Ok(())));
    assert_eq!(hardware.cleared_rx_block_acks, [0]);
    assert_eq!(
        core.apply(&mut hardware, remove(0)),
        Ok(Err(SettingError::InvalidBlockAck))
    );
}

#[test]
fn the_station_tsf_is_set_and_read_and_the_access_point_tsf_only_restarts() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: AP,
            config: Some(access_point()),
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        core.set_tsf(
            &mut hardware,
            VifTsf::new(STA, TsfInstant::from_micros(123_456))
        ),
        Ok(())
    );
    assert_eq!(
        core.tsf(&mut hardware, STA),
        Ok(VifTsf::new(STA, TsfInstant::from_micros(123_456)))
    );
    assert_eq!(
        core.set_tsf(&mut hardware, VifTsf::new(AP, TsfInstant::from_micros(0))),
        Ok(())
    );
    assert_eq!(hardware.access_point_tsf_resets, 1);
    assert_eq!(
        core.set_tsf(&mut hardware, VifTsf::new(AP, TsfInstant::from_micros(5))),
        Err(SettingError::Unsupported)
    );
    assert_eq!(core.tsf(&mut hardware, AP), Err(SettingError::Unsupported));
    assert_eq!(
        core.tsf(&mut hardware, VifId(2)),
        Err(SettingError::UnknownVif)
    );
    let caps = ESP32S31_BEACON_TIMING_CAPABILITIES;
    assert!(caps.tsf_restart.contains(VifRole::AccessPoint));
    assert!(!caps.tsf_read.contains(VifRole::AccessPoint));
}

#[test]
fn the_station_tbtt_schedule_is_programmed_and_its_events_announce_the_next_tbtt() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let schedule = TbttSchedule {
        next: VifTsf::new(STA, TsfInstant::from_micros(1_000_000)),
        beacon_interval: time_units(100),
        lead: Duration::from_micros(3_000),
    };
    assert_eq!(core.set_tbtt(&mut hardware, schedule), Ok(()));
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
                tbtt: VifTsf::new(STA, TsfInstant::from_micros(announced))
            })
        );
    }

    for refused in [
        TbttSchedule {
            beacon_interval: Duration::ZERO,
            ..schedule
        },
        TbttSchedule {
            beacon_interval: Duration::from_micros(u64::from(u32::MAX) + 1),
            ..schedule
        },
        TbttSchedule {
            lead: Duration::from_micros(u64::from(u16::MAX)),
            ..schedule
        },
    ] {
        assert_eq!(
            core.set_tbtt(&mut hardware, refused),
            Err(SettingError::Unsupported)
        );
    }
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: AP,
            config: Some(access_point()),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        core.set_tbtt(
            &mut hardware,
            TbttSchedule {
                next: VifTsf::new(AP, TsfInstant::from_micros(1_000_000)),
                ..schedule
            }
        ),
        Err(SettingError::Unsupported)
    );

    assert_eq!(core.stop_tbtt(&mut hardware, STA), Ok(()));
    assert_eq!(hardware.tbtt, None);
    // A stale edge after the stop reports nothing.
    core.station_tbtt(&mut hardware, &mut events);
    assert!(events.tbtts.is_empty());

    // Removing the station stops its schedule.
    core.set_tbtt(&mut hardware, schedule).unwrap();
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: STA,
            config: None,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(hardware.tbtt, None);
    assert_eq!(hardware.tbtt_stops, 2);
}

#[test]
fn a_closed_gate_blocks_the_queues_and_holds_the_attempt_until_it_opens() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    let gate = |open| LowerMacSetting::TxGate { open };

    core.apply(&mut hardware, gate(false)).unwrap().unwrap();
    assert!(hardware.tx_blocked);
    let request = attempt(&mut core, 1, &frame);
    assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
    assert_eq!(hardware.publications(), 0);
    assert_eq!(core.next_deadline(), None);
    core.apply(&mut hardware, gate(true)).unwrap().unwrap();
    assert!(!hardware.tx_blocked);
    assert_eq!(hardware.publications(), 1);
    // A published attempt keeps the gate open: the blocked queue would
    // end it with a hardware timeout.
    assert_eq!(
        core.apply(&mut hardware, gate(false)),
        Ok(Err(SettingError::Busy))
    );
    assert!(!hardware.tx_blocked);
    assert_eq!(
        complete(&mut core, &mut hardware, 0)[0].status,
        TxStatus::Success
    );

    // Cancelling a held attempt ends it without a publication.
    core.apply(&mut hardware, gate(false)).unwrap().unwrap();
    let request = attempt(&mut core, 2, &frame);
    submit(&mut core, &mut hardware, request).unwrap().unwrap();
    let mut events = Events::default();
    assert_eq!(core.cancel(TxId(2), &mut events), Ok(()));
    assert_eq!(events.completions[0].status, TxStatus::Aborted);
    assert_eq!(hardware.publications(), 1);
    assert_eq!(spare_slots(&core), SPARE);
}

#[test]
fn quiesce_and_disable_end_after_the_published_attempt() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let frame = data_frame(BSSID);
    let mut events = Events::default();

    let request = attempt(&mut core, 1, &frame);
    submit(&mut core, &mut hardware, request).unwrap().unwrap();
    core.lifecycle(LifecycleCommand::Quiesce, &mut events)
        .unwrap();
    assert!(events.lifecycle.is_empty());
    let request = attempt(&mut core, 2, &frame);
    assert_eq!(
        submit(&mut core, &mut hardware, request),
        Ok(Err(SubmitError::Disabled))
    );
    // A published attempt cannot be withdrawn: the cancel is admitted and
    // the attempt ends with its own completion.
    core.cancel(TxId(1), &mut events).unwrap();
    assert!(events.completions.is_empty());
    assert_eq!(
        core.cancel(TxId(9), &mut events),
        Err(CancelError::NotRunning)
    );
    core.lifecycle(LifecycleCommand::Disable, &mut events)
        .unwrap();
    assert!(events.lifecycle.is_empty());

    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    core.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    assert_eq!(events.completions[0].status, TxStatus::Success);
    assert_eq!(
        events.lifecycle,
        [LifecycleEvent::Quiesced, LifecycleEvent::Disabled]
    );
    assert_eq!(
        core.lifecycle(LifecycleCommand::Disable, &mut events),
        Err(LifecycleError::AlreadyInState)
    );
    let request = attempt(&mut core, 2, &frame);
    assert_eq!(
        submit(&mut core, &mut hardware, request),
        Ok(Err(SubmitError::Disabled))
    );
}

#[test]
fn quiesce_without_an_attempt_ends_at_once_and_enable_resumes() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let mut events = Events::default();
    core.lifecycle(LifecycleCommand::Quiesce, &mut events)
        .unwrap();
    assert_eq!(events.lifecycle, [LifecycleEvent::Quiesced]);
    core.lifecycle(LifecycleCommand::Enable, &mut events)
        .unwrap();
    assert_eq!(
        events.lifecycle,
        [LifecycleEvent::Quiesced, LifecycleEvent::Enabled]
    );
}

/// An attempt of `category` carrying a QoS Data MPDU to the BSS.
fn attempt_on(
    core: &mut Core,
    id: u32,
    category: WmmAccessCategory,
) -> Esp32s31MpduAttempt<'static, 512> {
    let mut request = attempt(core, id, &data_frame(BSSID));
    request.access_category = category;
    request
}

/// The hardware index of the queue an access category occupies.
fn queue_of(category: WmmAccessCategory) -> usize {
    usize::from(LegacyTxQueue::from_access_category(category).hardware_index())
}

fn service(core: &mut Core, hardware: &mut Hardware, wake: WifiTxWake) -> Vec<TxCompletion> {
    let mut events = Events::default();
    core.service(hardware, wake, &mut events).unwrap();
    events.completions
}

fn ids(completions: &[TxCompletion]) -> Vec<u32> {
    completions
        .iter()
        .map(|completion| completion.id.0)
        .collect()
}

#[test]
fn each_queue_holds_one_attempt_and_they_complete_in_any_order() {
    use WmmAccessCategory::{Background, BestEffort, Video, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);

    for (id, category) in [(1, Voice), (2, BestEffort)] {
        let request = attempt_on(&mut core, id, category);
        assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
    }
    // An occupied queue is busy; the identity is checked across queues.
    let busy = attempt_on(&mut core, 3, Voice);
    assert_eq!(
        submit(&mut core, &mut hardware, busy),
        Ok(Err(SubmitError::Busy))
    );
    let duplicate = attempt_on(&mut core, 1, Video);
    assert_eq!(
        submit(&mut core, &mut hardware, duplicate),
        Ok(Err(SubmitError::DuplicateId))
    );
    for (id, category) in [(4, Video), (5, Background)] {
        let request = attempt_on(&mut core, id, category);
        assert_eq!(submit(&mut core, &mut hardware, request), Ok(Ok(())));
    }
    // Four attempts in flight, one publication each, one per queue.
    let mut queues: Vec<u8> = hardware.legacy.iter().map(|(queue, _)| *queue).collect();
    queues.sort_unstable();
    assert_eq!(queues, [0, 1, 2, 3]);
    assert_eq!(spare_slots(&core), 0);
    assert_eq!(
        core.next_deadline(),
        Some(oer_time::Instant::from_micros(TIMEOUT))
    );

    // The last queue completes first; the others stay published.
    hardware.completion[queue_of(Background)] = Some(MacTxCompletionObservation::new_model(0, 0));
    let completions = service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE));
    assert_eq!(ids(&completions), [5]);
    assert_eq!(completions[0].status, TxStatus::Success);
    assert_eq!(core.queues.iter().flatten().count(), 3);

    // Two queues completing under one coalesced edge each report theirs.
    hardware.completion[queue_of(Voice)] = Some(MacTxCompletionObservation::new_model(5, 0));
    hardware.completion[queue_of(Video)] = Some(MacTxCompletionObservation::new_model(0, 0));
    let completions = service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE));
    assert_eq!(ids(&completions), [1, 4]);
    assert_eq!(completions[0].status, TxStatus::AckTimeout);
    assert_eq!(completions[1].status, TxStatus::Success);

    // An edge no queue shows is ignored.
    assert!(service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE)).is_empty());
    assert_eq!(ids(&complete(&mut core, &mut hardware, 0)), [2]);

    // No attempt was published twice, and every slot came back.
    assert_eq!(hardware.publications(), 4);
    assert_eq!(spare_slots(&core), SPARE);
    let again = attempt_on(&mut core, 6, Voice);
    assert_eq!(submit(&mut core, &mut hardware, again), Ok(Ok(())));
}

#[test]
fn a_collision_or_timeout_on_one_queue_leaves_the_others_published() {
    use WmmAccessCategory::{BestEffort, Video, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    for (id, category) in [(1, Voice), (2, BestEffort), (3, Video)] {
        let request = attempt_on(&mut core, id, category);
        submit(&mut core, &mut hardware, request).unwrap().unwrap();
    }

    hardware.collision_pending[queue_of(BestEffort)] = true;
    let completions = service(&mut core, &mut hardware, interrupt(EVENT_COLLISION));
    assert_eq!(ids(&completions), [2]);
    assert_eq!(completions[0].status, TxStatus::Collision);
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
    let completions = service(&mut core, &mut hardware, WifiTxWake::Deadline);
    assert_eq!(ids(&completions), [1]);
    assert_eq!(completions[0].status, TxStatus::Aborted);
    // The waiting queue's abort began as the first settle ended.
    assert!(hardware.cca_forced);
    let settle = core.next_deadline().unwrap();
    assert_eq!(settle, oer_time::Instant::from_micros(32));
    core.tx
        .timer
        .advance_to(oer_time::Instant::from_micros(settle.as_micros()));
    let completions = service(&mut core, &mut hardware, WifiTxWake::Deadline);
    assert_eq!(ids(&completions), [3]);
    assert_eq!(completions[0].status, TxStatus::Aborted);
    assert!(!hardware.cca_forced);
    assert_eq!(hardware.publications(), 3);
    assert_eq!(spare_slots(&core), SPARE);
}

#[test]
fn a_closed_gate_holds_every_queue_and_opening_it_publishes_them() {
    use WmmAccessCategory::{BestEffort, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let gate = |open| LowerMacSetting::TxGate { open };
    core.apply(&mut hardware, gate(false)).unwrap().unwrap();
    for (id, category) in [(1, Voice), (2, BestEffort)] {
        let request = attempt_on(&mut core, id, category);
        submit(&mut core, &mut hardware, request).unwrap().unwrap();
    }
    let held = aggregate(&mut core, 3, WmmAccessCategory::Video, 2);
    assert_eq!(submit_ampdu(&mut core, &mut hardware, held), Ok(Ok(())));
    assert_eq!(hardware.publications(), 0);
    assert_eq!(core.next_deadline(), None);

    core.apply(&mut hardware, gate(true)).unwrap().unwrap();
    assert_eq!(hardware.legacy.len(), 2);
    assert_eq!(hardware.ht.len(), 1);
    assert_eq!(
        core.apply(&mut hardware, gate(false)),
        Ok(Err(SettingError::Busy))
    );
}

fn ht_rate(mcs: u8) -> PhyRate {
    PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(mcs).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
    )
}

/// An aggregate of `count` QoS Data MPDUs of `len` bytes to the BSS, with
/// sequence numbers from 100.
fn aggregate_of(
    core: &mut Core,
    id: u32,
    category: WmmAccessCategory,
    count: usize,
    len: usize,
) -> Esp32s31AmpduAttempt<'static, Backings, SUBFRAMES> {
    let mut buffer = core.ampdu_buffer().expect("an idle aggregate owner");
    for index in 0..count {
        let mpdu = buffer.push_mpdu(len).expect("a backing");
        mpdu[..26].copy_from_slice(&data_frame(BSSID));
        mpdu[22..24].copy_from_slice(&((100 + index as u16) << 4).to_le_bytes());
    }
    TxAttempt {
        id: TxId(id),
        vif: STA,
        access_category: category,
        payload: AmpduPayload {
            subframes: buffer,
            tid: 0,
            min_mpdu_start_spacing: 5,
        },
        rate: ht_rate(7),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(9),
        coex: CoexPriority::Normal,
    }
}

fn aggregate(
    core: &mut Core,
    id: u32,
    category: WmmAccessCategory,
    count: usize,
) -> Esp32s31AmpduAttempt<'static, Backings, SUBFRAMES> {
    aggregate_of(core, id, category, count, 26)
}

/// Submit an aggregate and hand a refused one's buffer back.
fn submit_ampdu(
    core: &mut Core,
    hardware: &mut Hardware,
    attempt: Esp32s31AmpduAttempt<'static, Backings, SUBFRAMES>,
) -> Result<Result<(), SubmitError>, LowerMacFault> {
    Ok(match core.submit_ampdu(hardware, attempt)? {
        Ok(()) => Ok(()),
        Err(refused) => {
            core.release_ampdu_buffer(refused.attempt.payload.subframes);
            Err(refused.error)
        }
    })
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

fn published_ampdu(core: &Core, index: usize) -> &PublishedAmpdu<'static, Backing, SUBFRAMES> {
    match core.queues[index].as_ref().map(|attempt| &attempt.work) {
        Some(Work::Ampdu(published)) => published,
        _ => panic!("queue {index} holds a published aggregate"),
    }
}

#[test]
fn an_aggregate_is_published_once_and_reports_its_block_ack() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let source = core.ampdu_source.unwrap();
    assert_eq!(
        core.ampdu_capabilities(),
        esp32s31_ampdu_capabilities(SUBFRAMES)
    );
    assert_eq!(core.ampdu_capabilities().max_length, 6_490);
    assert!(
        !core
            .ampdu_capabilities()
            .formats
            .contains_rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
    );

    let request = aggregate(&mut core, 1, WmmAccessCategory::BestEffort, 3);
    assert_eq!(source.free(), BACKINGS - 3);
    assert_eq!(submit_ampdu(&mut core, &mut hardware, request), Ok(Ok(())));
    assert_eq!(hardware.ht.len(), 1);
    assert_eq!(usize::from(hardware.ht[0].0), BE);
    assert_eq!(hardware.ht[0].1.control().protection, MacTxProtection::None);
    let published = published_ampdu(&core, BE);
    assert_eq!(published.owner().frame_count(), 3);
    assert_eq!(published.owner().work().backoff_slots, 9);
    assert_eq!(
        core.next_deadline(),
        Some(oer_time::Instant::from_micros(TIMEOUT))
    );

    // A BlockAck arrived: the completion carries its window, and the
    // subframes go back to their source.
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b101, true));
    let completions = service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE));
    assert_eq!(
        completions,
        [TxCompletion {
            id: TxId(1),
            status: TxStatus::Success,
            ack_rssi_dbm: None,
            ack_snr_db: Some(0x60),
            block_ack: Some(BlockAckReport {
                start_sequence: SequenceNumber::new(100).unwrap(),
                bitmap: 0b101,
            }),
        }]
    );
    assert_eq!(source.free(), BACKINGS);
    assert_eq!(hardware.publications(), 1);

    // The BlockAck result decides, not the completion status; without it
    // the stale bitmap words are not reported.
    for (id, status, received, expected) in [
        (2, 5, true, TxStatus::Success),
        (3, 5, false, TxStatus::AckTimeout),
        (4, 0, false, TxStatus::AckTimeout),
        (5, 2, false, TxStatus::CtsTimeout),
    ] {
        let request = aggregate(&mut core, id, WmmAccessCategory::BestEffort, 2);
        submit_ampdu(&mut core, &mut hardware, request)
            .unwrap()
            .unwrap();
        hardware.block_ack_completion[BE] = Some(block_ack_completion(status, 100, 0b01, received));
        let completions = service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE));
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].status, expected);
        assert_eq!(completions[0].block_ack.is_some(), received);
    }
    // One publication per aggregate, never a retained retry.
    assert_eq!(hardware.ht.len(), 5);
    assert_eq!(source.free(), BACKINGS);
}

#[test]
fn aggregates_outside_the_limits_are_refused_with_their_subframes() {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let source = core.ampdu_source.unwrap();
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

    let refusals: [AmpduRefusal; 4] = [
        // HE aggregates are outside the declared formats, as are non-HT.
        (
            |request| request.rate = PhyRate::Legacy(LegacyRate::Ofdm24M),
            SubmitError::Unsupported,
        ),
        (
            |request| request.payload.min_mpdu_start_spacing = 8,
            SubmitError::Unsupported,
        ),
        (|request| request.vif = AP, SubmitError::UnknownVif),
        (
            |request| request.backoff = Backoff::HardwareDraw { cw_exponent: 4 },
            SubmitError::Unsupported,
        ),
    ];
    for (mutate, expected) in refusals {
        let mut request = aggregate(&mut core, 1, WmmAccessCategory::BestEffort, 2);
        mutate(&mut request);
        assert_eq!(
            submit_ampdu(&mut core, &mut hardware, request),
            Ok(Err(expected))
        );
    }
    let mut request = aggregate(&mut core, 1, WmmAccessCategory::BestEffort, 2);
    request.rate = he;
    assert_eq!(
        submit_ampdu(&mut core, &mut hardware, request),
        Ok(Err(SubmitError::Unsupported))
    );

    // An empty aggregate, a group receiver and one longer than the declared
    // maximum length.
    let empty = aggregate(&mut core, 1, WmmAccessCategory::BestEffort, 0);
    assert_eq!(
        submit_ampdu(&mut core, &mut hardware, empty),
        Ok(Err(SubmitError::InvalidLength))
    );
    let mut group = aggregate(&mut core, 1, WmmAccessCategory::BestEffort, 0);
    group
        .payload
        .subframes
        .push_mpdu(26)
        .unwrap()
        .copy_from_slice(&data_frame([0xff; 6]));
    assert_eq!(
        submit_ampdu(&mut core, &mut hardware, group),
        Ok(Err(SubmitError::Unsupported))
    );
    let long = aggregate_of(&mut core, 1, WmmAccessCategory::BestEffort, 4, 2_000);
    assert_eq!(
        submit_ampdu(&mut core, &mut hardware, long),
        Ok(Err(SubmitError::Unsupported))
    );
    let fits = aggregate_of(&mut core, 1, WmmAccessCategory::BestEffort, 3, 2_000);
    assert_eq!(submit_ampdu(&mut core, &mut hardware, fits), Ok(Ok(())));
    hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 0b111, true));
    service(&mut core, &mut hardware, interrupt(EVENT_TX_COMPLETE));

    // No more subframes than the owner's slots, nor MPDUs the backings
    // cannot hold with their metadata, MIC and FCS.
    let mut buffer = core.ampdu_buffer().unwrap();
    for _ in 0..SUBFRAMES {
        assert!(buffer.push_mpdu(26).is_some());
    }
    assert!(buffer.push_mpdu(26).is_none());
    assert_eq!(buffer.subframes(), SUBFRAMES);
    core.release_ampdu_buffer(buffer);
    let mut buffer = core.ampdu_buffer().unwrap();
    assert!(buffer.push_mpdu(BACKING - 20).is_some());
    assert!(buffer.push_mpdu(BACKING - 19).is_none());
    // Every aggregate owner can be lent, and no more.
    let second = core.ampdu_buffer().unwrap();
    assert!(core.ampdu_buffer().is_none());
    core.release_ampdu_buffer(buffer);
    core.release_ampdu_buffer(second);

    // Refused and released aggregates published nothing and kept no
    // backing.
    assert_eq!(hardware.ht.len(), 1);
    assert_eq!(source.free(), BACKINGS);
}

#[test]
fn an_aggregate_shares_the_queues_with_mpdus() {
    use WmmAccessCategory::{BestEffort, Video, Voice};
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let source = core.ampdu_source.unwrap();

    let mpdu = attempt_on(&mut core, 1, BestEffort);
    submit(&mut core, &mut hardware, mpdu).unwrap().unwrap();
    let busy = aggregate(&mut core, 2, BestEffort, 2);
    assert_eq!(
        submit_ampdu(&mut core, &mut hardware, busy),
        Ok(Err(SubmitError::Busy))
    );
    for (id, category) in [(3, Voice), (4, Video)] {
        let request = aggregate(&mut core, id, category, 2);
        assert_eq!(submit_ampdu(&mut core, &mut hardware, request), Ok(Ok(())));
    }

    // A collision ends the voice aggregate only.
    hardware.collision_pending[queue_of(Voice)] = true;
    let completions = service(&mut core, &mut hardware, interrupt(EVENT_COLLISION));
    assert_eq!(ids(&completions), [3]);
    assert_eq!(completions[0].status, TxStatus::Collision);
    assert_eq!(completions[0].block_ack, None);
    assert_eq!(source.free(), BACKINGS - 2);

    // A timeout ends the video aggregate after its settle; the MPDU on the
    // best-effort queue completes meanwhile.
    hardware.timeout_pending[queue_of(Video)] = true;
    assert!(service(&mut core, &mut hardware, interrupt(EVENT_TX_TIMEOUT)).is_empty());
    assert_eq!(ids(&complete(&mut core, &mut hardware, 0)), [1]);
    core.tx.timer.advance_to(oer_time::Instant::from_micros(
        core.next_deadline().unwrap().as_micros(),
    ));
    let completions = service(&mut core, &mut hardware, WifiTxWake::Deadline);
    assert_eq!(ids(&completions), [4]);
    assert_eq!(completions[0].status, TxStatus::Aborted);
    assert_eq!(source.free(), BACKINGS);
    assert_eq!(hardware.publications(), 3);

    // A held aggregate is cancelled with its subframes returned.
    core.apply(&mut hardware, LowerMacSetting::TxGate { open: false })
        .unwrap()
        .unwrap();
    let held = aggregate(&mut core, 5, BestEffort, 2);
    submit_ampdu(&mut core, &mut hardware, held)
        .unwrap()
        .unwrap();
    let mut events = Events::default();
    core.cancel(TxId(5), &mut events).unwrap();
    assert_eq!(events.completions[0].status, TxStatus::Aborted);
    assert_eq!(source.free(), BACKINGS);
    assert_eq!(hardware.publications(), 3);
}

#[test]
fn the_tsf_relation_breaks_at_a_jump_a_channel_change_an_interface_change_and_an_access_point_restart()
 {
    let mut hardware = Hardware::default();
    let mut core = enabled(&mut hardware);
    let generation =
        |core: &Core, hardware: &mut Hardware| core.tsf_reading(hardware, STA).unwrap().1;

    // The first set starts a generation.
    let start = generation(&core, &mut hardware);
    core.set_tsf(
        &mut hardware,
        VifTsf::new(STA, TsfInstant::from_micros(1_000_000)),
    )
    .unwrap();
    let followed = generation(&core, &mut hardware);
    assert_ne!(followed, start);

    let allowed = |elapsed: u64| {
        (elapsed * u64::from(TSF_DRIFT_PPM)).div_ceil(1_000_000)
            + STATION_TSF_SAMPLE_UNCERTAINTY.as_micros()
    };
    let interval = 102_400;

    // After ten missed beacons the follow corrects the drift of ten
    // intervals: more than one interval allows, within what ten allow.
    let mut last = 1_000_000;
    let reading = last + 10 * interval;
    let corrected = reading + allowed(10 * interval);
    assert!(corrected - reading > allowed(interval));
    hardware.station_tsf = reading;
    core.set_tsf(
        &mut hardware,
        VifTsf::new(STA, TsfInstant::from_micros(corrected)),
    )
    .unwrap();
    assert_eq!(generation(&core, &mut hardware), followed);
    last = corrected;

    // At the next beacon, one interval later, the same correction is a
    // real jump.
    let reading = last + interval;
    hardware.station_tsf = reading;
    core.set_tsf(
        &mut hardware,
        VifTsf::new(
            STA,
            TsfInstant::from_micros(reading + allowed(interval) + 1),
        ),
    )
    .unwrap();
    let jumped = generation(&core, &mut hardware);
    assert_ne!(jumped, followed);

    // Reconfiguring the station (a reassociation or roam) breaks it.
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: STA,
            config: Some(station()),
        },
    )
    .unwrap()
    .unwrap();
    let reassociated = generation(&core, &mut hardware);
    assert_ne!(reassociated, jumped);
}
