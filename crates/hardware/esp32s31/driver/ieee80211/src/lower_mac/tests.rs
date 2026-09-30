use core::{future::ready, pin::Pin};
use std::vec::Vec;

use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacStaApReceivePlan, MacTxCompletionObservation,
    MacTxDetachOutcome, MacTxDetachReason, MacTxProtection, MacTxQueueDetached,
};
use oer_esp32s31_ieee80211_mac::{
    irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT},
    rx::RxPhyInfo,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, runtime::WifiTxRuntimePolicy},
};
use oer_ieee80211_lower_mac::{
    AmpduSubmission, HardwareServices, KeyInstall, LowerMacEvent, PhyRate, RxEvidence, TbttSchedule,
};
use oer_ieee80211_mac::{
    channel::ChannelWidth,
    phy::{self, FecCoding, HeGiLtf, LegacyRate, PpduBandwidth, SpatialStreams},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

use super::*;
use crate::ordinary_tx::{WifiTxPowerPair, WifiTxResources};

const STATION: MacAddress = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const ACCESS_POINT: MacAddress = [0x02, 0x11, 0x12, 0x13, 0x14, 0x15];
const BSSID: MacAddress = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const PEER: MacAddress = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
const STA: VifId = VifId(0);
const AP: VifId = VifId(1);
const TIMEOUT: u64 = 250_000;

#[derive(Default)]
struct Hardware {
    legacy: Vec<(u8, MacLegacyTxProgram)>,
    ht: Vec<(u8, MacHtTxProgram)>,
    he: Vec<(u8, MacHeTxProgram)>,
    completion: Option<MacTxCompletionObservation>,
    block_ack_completion: Option<MacHtAmpduCompletionObservation>,
    timeout_pending: bool,
    collision_pending: bool,
    installed_keys: Vec<(u8, MacCcmpKeyIdentity)>,
    cleared_keys: Vec<u8>,
    rx_block_acks: Vec<S31RxBlockAckAgreement>,
    cleared_rx_block_acks: Vec<u8>,
    station_policy: Option<MacAddress>,
    station_disabled: usize,
    access_point_policy: Option<MacAddress>,
    access_point_disabled: usize,
    access_point_tsf_resets: usize,
    station_tsf: u64,
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

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn take_block_ack_completion(&mut self, _queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.block_ack_completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        self.timeout_pending
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let pending = match reason {
            MacTxDetachReason::Timeout => core::mem::take(&mut self.timeout_pending),
            MacTxDetachReason::Collision => core::mem::take(&mut self.collision_pending),
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
        self.station_policy = Some(bssid);
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

    fn set_station_tsf(&mut self, value: u64) {
        self.station_tsf = value;
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

#[derive(Default)]
struct Timer {
    now: u64,
}

impl WifiTxTimer for Timer {
    fn now_micros(&self) -> u64 {
        self.now
    }

    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.now = deadline_micros;
        ready(())
    }

    fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        self.now += micros;
        ready(())
    }
}

fn entropy() -> u32 {
    0x1234_5678
}

type Core<'a> = LowerMacCore<'a, Power, fn() -> u32, Timer, 512>;

#[derive(Default)]
struct Events {
    completions: Vec<TxCompletion>,
    lifecycle: Vec<LifecycleEvent>,
}

impl LowerMacSink for Events {
    fn tx_completed(&mut self, completion: TxCompletion) {
        self.completions.push(completion);
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        self.lifecycle.push(event);
    }
}

fn channel(number: u8) -> WifiChannel {
    WifiChannel::mhz20(number).unwrap()
}

fn core(slot: Pin<&mut TxSlot<512>>) -> Core<'_> {
    LowerMacCore::new(
        OrdinaryTxOwner::new(WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: Timer::default(),
        }),
        LowerMacConfig {
            station_address: STATION,
            channel: channel(6),
            publication_timeout_micros: TIMEOUT,
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
fn enabled<'a>(slot: Pin<&'a mut TxSlot<512>>, hardware: &mut Hardware) -> Core<'a> {
    let mut core = core(slot);
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

fn attempt(id: u32, frame: &[u8]) -> TxAttempt<'_> {
    TxAttempt {
        id: TxId(id),
        vif: STA,
        access_category: WmmAccessCategory::BestEffort,
        payload: TxPayload::Mpdu {
            frame,
            response: TxResponse::Ack,
        },
        rate: PhyRate::Legacy(LegacyRate::Ofdm24M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
    }
}

fn interrupt(events: u32) -> WifiTxWake {
    WifiTxWake::Interrupt { events }
}

/// Deliver a completion with `status` and return the completion event.
fn complete(core: &mut Core<'_>, hardware: &mut Hardware, status: u8) -> Vec<TxCompletion> {
    complete_with(core, hardware, status, 0)
}

fn complete_with(
    core: &mut Core<'_>,
    hardware: &mut Hardware,
    status: u8,
    detail: u8,
) -> Vec<TxCompletion> {
    hardware.completion = Some(MacTxCompletionObservation::new_model(status, detail));
    let mut events = Events::default();
    core.service(hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
        .unwrap();
    events.completions
}

#[test]
fn capabilities_are_the_s31_mac_services_on_2_4_ghz() {
    let caps = ESP32S31_LOWER_MAC_CAPABILITIES;
    assert_eq!(
        caps.services,
        ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services()
    );
    assert!(caps.services.contains(HardwareServices::FCS));
    assert!(!caps.services.contains(HardwareServices::RETRY_POLICY));
    assert!(caps.supports_channel(Channel::ghz2_4(6, ChannelWidth::Mhz40Above).unwrap()));
    assert!(!caps.supports_channel(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()));
    assert_eq!(caps.vifs, 2);
    assert_eq!(caps.max_ampdu_subframes, 0);
    assert_eq!(usize::from(caps.key_slots), LOWER_MAC_KEY_SLOTS);
    assert_eq!(caps.rx_block_ack_agreements, RESOURCES.rx_block_ack_entries);
}

#[test]
fn an_attempt_is_one_publication_and_its_success_is_reported() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);

    assert_eq!(core.submit(&mut hardware, attempt(1, &frame)), Ok(Ok(())));
    assert_eq!(hardware.legacy.len(), 1);
    assert_eq!(hardware.legacy[0].1.interface(), MacInterface::Station);
    assert_eq!(core.next_deadline_micros(), Some(TIMEOUT));

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
    assert_eq!(core.next_deadline_micros(), None);
    assert_eq!(core.submit(&mut hardware, attempt(2, &frame)), Ok(Ok(())));
}

#[test]
fn an_unacknowledged_attempt_is_reported_and_never_published_again() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
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
        assert_eq!(core.submit(&mut hardware, attempt(id, &frame)), Ok(Ok(())));
        let completions = complete_with(&mut core, &mut hardware, status, detail);
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, TxId(id));
        assert_eq!(completions[0].status, expected);
        assert_eq!(completions[0].ack_snr_db, None);
        assert_eq!(hardware.publications(), published + 1);
    }
    // The Retry bit the ladder sets before a re-publication is untouched.
    assert_eq!(core.tx.buffer_mut().unwrap()[TX_METADATA_SIZE + 1], 0x01);
}

#[test]
fn a_collision_detach_ends_the_attempt() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);

    core.submit(&mut hardware, attempt(7, &frame))
        .unwrap()
        .unwrap();
    hardware.collision_pending = true;
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_COLLISION), &mut events)
        .unwrap();
    assert_eq!(events.completions.len(), 1);
    assert_eq!(events.completions[0].status, TxStatus::Collision);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_hardware_timeout_ends_the_attempt_as_aborted() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);

    core.submit(&mut hardware, attempt(3, &frame))
        .unwrap()
        .unwrap();
    hardware.timeout_pending = true;
    let mut events = Events::default();
    core.service(&mut hardware, interrupt(EVENT_TX_TIMEOUT), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    let settle = core.next_deadline_micros().unwrap();
    core.tx.timer.now = settle;
    core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
        .unwrap();
    assert_eq!(events.completions.len(), 1);
    assert_eq!(events.completions[0].status, TxStatus::Aborted);
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_block_ack_request_reports_its_block_ack() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let mut frame = [0; 24];
    frame[0] = BLOCK_ACK_REQUEST_FRAME_CONTROL;
    frame[4..10].copy_from_slice(&BSSID);
    let mut request = attempt(4, &frame);
    request.payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::BlockAck,
    };

    core.submit(&mut hardware, request).unwrap().unwrap();
    hardware.block_ack_completion = Some(MacHtAmpduCompletionObservation::new_model(
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
    request.id = TxId(5);
    request.rate = PhyRate::Ht(
        phy::HtRate::new(phy::HtMcs::new(0).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
    );
    assert_eq!(
        core.submit(&mut hardware, request),
        Ok(Err(SubmitError::UnsupportedRate))
    );
}

#[test]
fn ht_and_he_attempts_use_their_programs_and_unsendable_rates_are_refused() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
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

    let mut ht_attempt = attempt(1, &frame);
    ht_attempt.rate = ht(7, PpduBandwidth::Mhz20);
    core.submit(&mut hardware, ht_attempt).unwrap().unwrap();
    assert_eq!(hardware.ht.len(), 1);
    complete(&mut core, &mut hardware, 0);

    let mut he_attempt = attempt(2, &frame);
    he_attempt.rate = he(1, PpduBandwidth::Mhz20);
    core.submit(&mut hardware, he_attempt).unwrap().unwrap();
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
        let mut refused = attempt(3, &frame);
        refused.rate = rate;
        assert_eq!(
            core.submit(&mut hardware, refused),
            Ok(Err(SubmitError::UnsupportedRate))
        );
    }
    assert_eq!(hardware.publications(), 2);
}

#[test]
fn the_callers_protection_is_published() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);

    for (id, protection, expected) in [
        (1, Protection::RtsCts, MacTxProtection::RtsCts),
        (2, Protection::CtsToSelf, MacTxProtection::CtsToSelf),
        (3, Protection::None, MacTxProtection::None),
    ] {
        let mut request = attempt(id, &frame);
        request.protection = protection;
        core.submit(&mut hardware, request).unwrap().unwrap();
        assert_eq!(
            hardware.legacy.last().unwrap().1.control().protection,
            expected
        );
        complete(&mut core, &mut hardware, 0);
    }
}

#[test]
fn submissions_the_backend_cannot_send_are_refused_without_publication() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = core(slot.as_mut());
    let frame = data_frame(BSSID);
    let group = data_frame([0xff; 6]);
    let subframes: [&[u8]; 1] = [&frame];

    assert_eq!(
        core.submit(&mut hardware, attempt(1, &frame)),
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

    let mut unknown_vif = attempt(1, &frame);
    unknown_vif.vif = AP;
    let mut ampdu = attempt(1, &frame);
    ampdu.payload = TxPayload::Ampdu(AmpduSubmission {
        subframes: &subframes,
        tid: 0,
        min_mpdu_start_spacing: 0,
    });
    let mut limited = attempt(1, &frame);
    limited.power = TxPower::MaxDbm(10);
    let mut no_ack = attempt(1, &frame);
    no_ack.payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::None,
    };
    let mut group_ack = attempt(1, &group);
    group_ack.payload = TxPayload::Mpdu {
        frame: &group,
        response: TxResponse::Ack,
    };
    let short = attempt(1, &frame[..9]);
    let long_frame = [0; 512];
    let mut long = attempt(1, &long_frame);
    long.payload = TxPayload::Mpdu {
        frame: &long_frame,
        response: TxResponse::None,
    };
    let mut unknown_key = attempt(1, &frame);
    unknown_key.key = KeySelector::Key(KeyHandle(0));

    for (request, expected) in [
        (unknown_vif, SubmitError::UnknownVif),
        (ampdu, SubmitError::Unsupported),
        (limited, SubmitError::Unsupported),
        (no_ack, SubmitError::Unsupported),
        (group_ack, SubmitError::Unsupported),
        (short, SubmitError::InvalidLength),
        (long, SubmitError::InvalidLength),
        (unknown_key, SubmitError::UnknownKey),
    ] {
        assert_eq!(core.submit(&mut hardware, request), Ok(Err(expected)));
    }
    assert_eq!(hardware.publications(), 0);

    core.submit(&mut hardware, attempt(1, &frame))
        .unwrap()
        .unwrap();
    assert_eq!(
        core.submit(&mut hardware, attempt(1, &frame)),
        Ok(Err(SubmitError::DuplicateId))
    );
    assert_eq!(
        core.submit(&mut hardware, attempt(2, &frame)),
        Ok(Err(SubmitError::Busy))
    );
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn a_group_frame_solicits_no_response() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame([0xff; 6]);
    let mut request = attempt(1, &frame);
    request.payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::None,
    };
    assert_eq!(core.submit(&mut hardware, request), Ok(Ok(())));
    assert_eq!(
        complete(&mut core, &mut hardware, 0)[0].status,
        TxStatus::Success
    );
}

#[test]
fn channels_outside_2_4_ghz_are_refused_and_enable_retunes() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = core(slot.as_mut());
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
    core.finish_retune(true, &mut events).unwrap();
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
fn a_failed_retune_poisons_and_leaves_the_port_disabled() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = core(slot.as_mut());
    let mut events = Events::default();
    core.apply(
        &mut hardware,
        LowerMacSetting::Channel(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap()),
    )
    .unwrap()
    .unwrap();
    core.lifecycle(LifecycleCommand::Enable, &mut events)
        .unwrap();
    assert_eq!(
        core.finish_retune(false, &mut events),
        Err(LowerMacFault::Retune)
    );
    assert!(events.lifecycle.is_empty());
    assert_eq!(core.state, PortState::Disabled);
}

#[test]
fn receive_filters_map_onto_the_role_policies() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = core(slot.as_mut());
    let configure = |core: &mut Core<'_>, hardware: &mut Hardware, vif, config| {
        core.apply(hardware, LowerMacSetting::Vif { vif, config })
    };

    configure(&mut core, &mut hardware, STA, Some(station()))
        .unwrap()
        .unwrap();
    assert_eq!(hardware.station_policy, Some(BSSID));
    configure(&mut core, &mut hardware, AP, Some(access_point()))
        .unwrap()
        .unwrap();
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // Filters without a register transaction, a second station, a station
    // address other than the published one and an access point whose BSSID
    // is not its address are refused before any register write.
    let refused = [
        (
            STA,
            VifConfig {
                receive: ReceiveFilter::PROMISCUOUS,
                ..station()
            },
            SettingError::Unsupported,
        ),
        (
            STA,
            VifConfig {
                receive: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
                ..station()
            },
            SettingError::Unsupported,
        ),
        (
            STA,
            VifConfig {
                receive: ReceiveFilter::OWN_UNICAST,
                ..station()
            },
            SettingError::Unsupported,
        ),
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
            configure(&mut core, &mut hardware, vif, Some(config)),
            Ok(Err(expected))
        );
    }
    assert_eq!(hardware.station_policy, Some(BSSID));
    assert_eq!(hardware.access_point_policy, Some(ACCESS_POINT));

    // No receive filter closes the role's context; removal closes it too.
    configure(
        &mut core,
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
    configure(&mut core, &mut hardware, AP, None)
        .unwrap()
        .unwrap();
    assert_eq!(hardware.access_point_disabled, 1);
    assert_eq!(
        configure(&mut core, &mut hardware, AP, None),
        Ok(Err(SettingError::UnknownVif))
    );
}

#[test]
fn keys_install_into_their_role_slots_and_protect_attempts() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
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
    let mut protected = attempt(1, &frame);
    protected.key = KeySelector::Key(pairwise);
    core.submit(&mut hardware, protected).unwrap().unwrap();
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
    let mut stale = attempt(2, &frame);
    stale.key = KeySelector::Key(pairwise);
    assert_eq!(
        core.submit(&mut hardware, stale),
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
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
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
fn the_station_tsf_is_set_and_read_and_the_access_point_tsf_only_resets() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    core.apply(
        &mut hardware,
        LowerMacSetting::Vif {
            vif: AP,
            config: Some(access_point()),
        },
    )
    .unwrap()
    .unwrap();
    let set = |vif, tsf| LowerMacSetting::SetTsf { vif, tsf: Tsf(tsf) };

    assert_eq!(core.apply(&mut hardware, set(STA, 123_456)), Ok(Ok(())));
    assert_eq!(core.tsf(&mut hardware, STA), Ok(Tsf(123_456)));
    assert_eq!(core.apply(&mut hardware, set(AP, 0)), Ok(Ok(())));
    assert_eq!(hardware.access_point_tsf_resets, 1);
    assert_eq!(
        core.apply(&mut hardware, set(AP, 5)),
        Ok(Err(SettingError::Unsupported))
    );
    assert_eq!(core.tsf(&mut hardware, AP), Err(SettingError::Unsupported));
    assert_eq!(
        core.tsf(&mut hardware, VifId(2)),
        Err(SettingError::UnknownVif)
    );
}

#[test]
fn tbtt_reports_and_coexistence_hints_are_unsupported() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    for setting in [
        LowerMacSetting::Tbtt {
            vif: STA,
            schedule: Some(TbttSchedule {
                beacon_interval_tu: 100,
                next: Tsf(0),
            }),
        },
        LowerMacSetting::CoexPriority(Default::default()),
    ] {
        assert_eq!(
            core.apply(&mut hardware, setting),
            Ok(Err(SettingError::Unsupported))
        );
    }
}

#[test]
fn a_power_save_hold_keeps_the_attempt_unpublished_until_released() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);
    let hold = |peer, blocked| LowerMacSetting::PowerSaveTxBlock {
        vif: STA,
        peer,
        blocked,
    };

    core.apply(&mut hardware, hold(Some(BSSID), true))
        .unwrap()
        .unwrap();
    assert_eq!(core.submit(&mut hardware, attempt(1, &frame)), Ok(Ok(())));
    assert_eq!(hardware.publications(), 0);
    assert_eq!(core.next_deadline_micros(), None);
    // Releasing another peer leaves the hold in place.
    core.apply(&mut hardware, hold(Some(PEER), false))
        .unwrap()
        .unwrap();
    assert_eq!(hardware.publications(), 0);
    core.apply(&mut hardware, hold(Some(BSSID), false))
        .unwrap()
        .unwrap();
    assert_eq!(hardware.publications(), 1);
    assert_eq!(
        complete(&mut core, &mut hardware, 0)[0].status,
        TxStatus::Success
    );

    // Cancelling a held attempt ends it without a publication.
    core.apply(&mut hardware, hold(None, true))
        .unwrap()
        .unwrap();
    core.submit(&mut hardware, attempt(2, &frame))
        .unwrap()
        .unwrap();
    let mut events = Events::default();
    assert_eq!(
        core.lifecycle(LifecycleCommand::Cancel(TxId(2)), &mut events),
        Ok(LifecycleStart::Admitted)
    );
    assert_eq!(events.completions[0].status, TxStatus::Aborted);
    assert_eq!(hardware.publications(), 1);
    assert_eq!(
        core.apply(
            &mut hardware,
            LowerMacSetting::PowerSaveTxBlock {
                vif: AP,
                peer: None,
                blocked: true
            }
        ),
        Ok(Err(SettingError::UnknownVif))
    );
}

#[test]
fn quiesce_and_disable_end_after_the_published_attempt() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
    let frame = data_frame(BSSID);
    let mut events = Events::default();

    core.submit(&mut hardware, attempt(1, &frame))
        .unwrap()
        .unwrap();
    core.lifecycle(LifecycleCommand::Quiesce, &mut events)
        .unwrap();
    assert!(events.lifecycle.is_empty());
    assert_eq!(
        core.submit(&mut hardware, attempt(2, &frame)),
        Ok(Err(SubmitError::Disabled))
    );
    // A published attempt cannot be withdrawn: the cancel is admitted and
    // the attempt ends with its own completion.
    core.lifecycle(LifecycleCommand::Cancel(TxId(1)), &mut events)
        .unwrap();
    assert!(events.completions.is_empty());
    assert_eq!(
        core.lifecycle(LifecycleCommand::Cancel(TxId(9)), &mut events),
        Err(LifecycleError::UnknownAttempt)
    );
    core.lifecycle(LifecycleCommand::Disable, &mut events)
        .unwrap();
    assert!(events.lifecycle.is_empty());

    hardware.completion = Some(MacTxCompletionObservation::new_model(0, 0));
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
    assert_eq!(
        core.submit(&mut hardware, attempt(2, &frame)),
        Ok(Err(SubmitError::Disabled))
    );
}

#[test]
fn quiesce_without_an_attempt_ends_at_once_and_enable_resumes() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = enabled(slot.as_mut(), &mut hardware);
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

#[test]
fn received_frames_carry_the_configured_channel_while_receiving() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut core = core(slot.as_mut());
    let mpdu = data_frame(STATION);
    let frame = NormalizedRxFrame {
        mpdu: &mpdu,
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
    };

    assert!(core.received(&frame).is_none());
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
    let (bytes, meta) = core.received(&frame).unwrap();
    assert_eq!(bytes, mpdu);
    assert_eq!(meta.channel, Channel::from_wifi_channel(channel(6)));
    assert_eq!(meta.rssi_dbm, RxEvidence::HardwareObserved(-42));
    assert_eq!(meta.noise_floor_dbm, RxEvidence::Unavailable);
    let view = LowerMacEvent::Received { frame: bytes, meta };
    assert!(matches!(view, LowerMacEvent::Received { .. }));
}
