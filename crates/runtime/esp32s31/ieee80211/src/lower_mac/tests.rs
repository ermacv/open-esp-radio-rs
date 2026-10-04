use core::{cell::RefCell as TestCell, future::ready, pin::Pin};
use std::vec::Vec;

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacStaApReceivePlan, MacTxCompletionObservation,
    MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached, StaTbttSchedule,
};
use oer_esp32s31_ieee80211::lower_mac::{AmpduBacking, AmpduBackingSource, Esp32s31AmpduOwner};
use oer_esp32s31_ieee80211::{
    lower_mac::{
        BeaconWindowPriority, LowerMacConfig, RxBeaconPriorityHardware, StationTbttHardware,
        TxGateHardware,
    },
    ordinary_tx::{OrdinaryTxOwner, WifiTxPowerPair, WifiTxResources},
    station_tsf::{StationTsfHardware, StationTsfWrite},
};
use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    crypto::CcmpKeyHardware,
    init::{MacSnifferHardware, StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware},
    irq::EVENT_TX_COMPLETE,
    rx::{
        RxPhyInfo,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{
        HardwareOwnedTxDma, PreparedTxDma, TxHardware, TxSlot,
        ampdu::{HtAmpduHardware, HtAmpduTxStorage, RetainedAmpduDmaStorage, RetainedDmaAmpduTx},
        runtime::WifiTxRuntimePolicy,
    },
};
use oer_ieee80211_lower_mac::{
    AmpduBuffer, AmpduPayload, Backoff, BlockAckReport, Channel, ChannelWidth, CoexPriority,
    FailureClass, KeySelector, MacAddress, PhyFormatSet, PhyRate, PortError, Protection,
    ReceiveFilter, SubmitError, TxAttempt, TxBuffer, TxId, TxPayload, TxPower, TxResponse,
    TxStatus, VifConfig, VifRole,
};
use oer_ieee80211_mac::{
    phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};
use oer_memory::{
    DmaIndexReturn, PinnedDmaTxPool, PinnedDmaTxRadioLease, ReturningStableDmaBacking,
};

use super::*;

const STATION: MacAddress = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const BSSID: MacAddress = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const STA: VifId = VifId(0);

/// Register owner model: records publications and returns the completion a
/// test installs; the receive-policy, key, Block Ack and TSF seams accept.
#[derive(Default)]
struct Hardware {
    legacy: Vec<MacLegacyTxProgram>,
    ht: Vec<MacHtTxProgram>,
    /// Completions by queue hardware index.
    completion: [Option<MacTxCompletionObservation>; 4],
    block_ack_completion: [Option<MacHtAmpduCompletionObservation>; 4],
    station_tsf: u64,
    tbtt: Option<StaTbttSchedule>,
}

/// The hardware index of the best-effort queue.
const BE: usize = 2;
/// The hardware index of the voice queue.
const VO: usize = 0;

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy.push(program);
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        self.ht.push(program);
        true
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion[usize::from(queue)].take()
    }

    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.block_ack_completion[usize::from(queue)].take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<U>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> U,
    ) -> MacTxDetachOutcome<U> {
        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Timeout | MacTxDetachReason::Collision => {
                MacTxDetachOutcome::NoEvent
            }
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
        _index: u8,
        _identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

impl RxBlockAckHardware for Hardware {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        agreement.validate().map(|_| ())
    }

    fn clear_rx_block_ack(&mut self, _index: u8) -> Result<(), S31RxBlockAckAgreementError> {
        Ok(())
    }

    fn reset_rx_block_ack_window(
        &mut self,
        _index: u8,
        _tid: u8,
        _start: SequenceNumber,
        _window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        Ok(())
    }

    fn program_extra_softap_rx_block_ack(
        &mut self,
        _agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        Ok(())
    }

    fn clear_extra_softap_rx_block_ack(
        &mut self,
        _index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        Ok(())
    }

    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        _index: u8,
        _start: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        Ok(())
    }
}

impl StaApRegisterHardware for Hardware {
    fn apply_sta_ap_receive_registers(&mut self, _plan: MacStaApReceivePlan) {}
    fn disable_station_receive_registers(&mut self) {}
    fn disable_access_point_receive_registers(&mut self) {}
    fn disable_all_role_receive_registers(&mut self) {}
}

impl StaLinkRxPolicyHardware for Hardware {
    fn apply_sta_link_policy(&mut self, _bssid: [u8; 6]) {}
}

impl StaEspNowRxPolicyHardware for Hardware {
    fn apply_sta_esp_now_policy(&mut self, _bssid: [u8; 6]) {}
}

impl MacSnifferHardware for Hardware {
    fn configure_open_promiscuous_receive(&mut self) {}
    fn disable_open_promiscuous_receive(&mut self) {}
}

impl StationTbttHardware for Hardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        self.tbtt = Some(schedule);
    }

    fn stop_station_tbtt(&mut self) {
        self.tbtt = None;
    }
}

impl TxGateHardware for Hardware {
    fn set_power_save_tx_block(&mut self, _blocked: bool) {}
}

/// A radio system whose beacon-window event has priority zero.
struct BeaconWindowZero;

impl BeaconWindowPriority for BeaconWindowZero {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        oer_esp32s31_hal::types::MacPti::new(0).unwrap()
    }
}

impl RxBeaconPriorityHardware for Hardware {
    fn set_rx_beacon_pti(&mut self, _pti: oer_esp32s31_hal::types::MacPti) {}
    fn clear_rx_beacon_pti(&mut self) {}
}

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _access_point: [u8; 6]) {}
    fn disable_ap_link_policy(&mut self) {}
}

impl ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
    }
    fn stop_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
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

struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

/// A model clock far from any deadline, so the watchdog never fires.
#[derive(Default)]
struct ModelTimer;

impl oer_time::Clock for ModelTimer {
    fn now(&self) -> oer_time::Instant {
        oer_time::Instant::EPOCH
    }
}

impl oer_time::Timer for ModelTimer {
    fn wait_until(&self, _deadline: oer_time::Instant) -> impl Future<Output = ()> {
        ready(())
    }
}

/// The MAC local time the model clock reads at its monotonic epoch.
const MAC_AT_EPOCH: u32 = 5_000;

impl crate::mac_clock::ReceptionClock for ModelTimer {
    fn snapshot(&self) -> Option<crate::mac_clock::MacClockSnapshot> {
        crate::mac_clock::MacClockStorage::<embassy_sync::blocking_mutex::raw::NoopRawMutex, _, _>::new(ModelTimer)
            .start(crate::mac_clock::FixedCounter(MAC_AT_EPOCH))
            .snapshot()
    }

    fn on_rf_wake(&self) {}

    fn counter_reading(&self) -> Option<(u32, oer_time::Instant)> {
        None
    }
}

/// Records every retune and answers with `accept`.
struct Retune {
    accept: bool,
    tuned: &'static std::sync::Mutex<Vec<WifiChannel>>,
}

impl LowerMacRetune for Retune {
    fn retune(&mut self, channel: WifiChannel) -> impl Future<Output = bool> {
        self.tuned.lock().unwrap().push(channel);
        ready(self.accept)
    }
}

fn entropy() -> u32 {
    0x1234_5678
}

type Port = Esp32s31LowerMac<
    'static,
    CriticalSectionRawMutex,
    Power,
    fn() -> u32,
    ModelTimer,
    Hardware,
    Retune,
    512,
    1,
    2,
    64,
>;

/// A model slot in permanently retained storage, as target SRAM is.
fn slot() -> Pin<&'static mut TxSlot<512>> {
    Pin::static_mut(std::boxed::Box::leak(std::boxed::Box::new(
        TxSlot::new_model(),
    )))
}

fn install(port: &Port, accept_retune: bool) -> &'static std::sync::Mutex<Vec<WifiChannel>> {
    install_core(
        port,
        accept_retune,
        oer_time::Duration::from_micros(250_000),
    )
}

/// Install with an already expired publication watchdog.
fn install_with_timeout(port: &Port, publication_timeout: oer_time::Duration) {
    install_core(port, true, publication_timeout);
}

fn install_core(
    port: &Port,
    accept_retune: bool,
    publication_timeout: oer_time::Duration,
) -> &'static std::sync::Mutex<Vec<WifiChannel>> {
    let tuned = std::boxed::Box::leak(std::boxed::Box::new(std::sync::Mutex::new(Vec::new())));
    let core = LowerMacCore::new(
        OrdinaryTxOwner::new(WifiTxResources {
            slot: slot(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: ModelTimer,
        }),
        [slot()],
        LowerMacConfig {
            station_address: STATION,
            channel: WifiChannel::mhz20(6).unwrap(),
            publication_timeout,
            tsf_epoch: 1,
        },
        &BeaconWindowZero,
    );
    assert!(
        port.install(Esp32s31LowerMacParts {
            core,
            hardware: Hardware::default(),
            retune: Retune {
                accept: accept_retune,
                tuned,
            },
        })
        .is_ok()
    );
    port.apply(LowerMacSetting::Vif {
        vif: STA,
        config: Some(VifConfig {
            address: STATION,
            role: VifRole::Station,
            bssid: Some(BSSID),
            receive: ReceiveFilter::BSS_MEMBER,
        }),
    })
    .unwrap()
    .unwrap();
    tuned
}

/// Take the next event while polling the port's runner beside it, as a
/// composition does.
fn take<P: Ieee80211LowerMacPort>(
    port: &P,
    run: impl Future<Output = Infallible>,
) -> Result<P::Event, EventsLost> {
    match block_on(embassy_futures::select::select(port.next_event(), run)) {
        embassy_futures::select::Either::First(event) => event,
        embassy_futures::select::Either::Second(never) => match never {},
    }
}

fn next_owned(port: &Port) -> Result<&'static Esp32s31LowerMacEvent<64>, EventsLost> {
    // Views borrow the owned event; tests compare leaked copies.
    take(port, port.run()).map(|event| &*std::boxed::Box::leak(std::boxed::Box::new(event)))
}

fn next(port: &Port) -> Result<LowerMacEvent<'static>, EventsLost> {
    next_owned(port).map(Port::view)
}

fn data_frame() -> [u8; 26] {
    let mut frame = [0; 26];
    frame[0] = 0x88;
    frame[4..10].copy_from_slice(&BSSID);
    frame[10..16].copy_from_slice(&STATION);
    frame
}

fn attempt(port: &Port, id: u32, frame: &[u8]) -> Esp32s31MpduAttempt<'static, 512> {
    let mut buffer = port
        .tx_buffer(frame.len())
        .ok()
        .flatten()
        .expect("a spare slot");
    buffer.frame_mut().copy_from_slice(frame);
    TxAttempt {
        id: TxId(id),
        vif: STA,
        access_category: WmmAccessCategory::BestEffort,
        payload: TxPayload {
            frame: buffer,
            response: TxResponse::Ack,
        },
        rate: PhyRate::Legacy(LegacyRate::Ofdm12M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    }
}

/// Submit, handing a refused attempt's buffer back.
fn submit(
    port: &Port,
    attempt: Esp32s31MpduAttempt<'static, 512>,
) -> Result<Result<(), SubmitError>, Esp32s31LowerMacError> {
    Ok(port.submit(attempt)?.map_err(|refused| {
        port.release_tx_buffer(refused.attempt.payload.frame);
        refused.error
    }))
}

fn with_hardware<U>(port: &Port, entry: impl FnOnce(&mut Hardware) -> U) -> U {
    port.installed
        .lock(|installed| entry(&mut installed.borrow_mut().as_mut().unwrap().hardware))
}

#[test]
fn an_attempt_completes_through_the_interrupt_entry_and_the_queue() {
    let port = Port::new(ModelTimer);
    install(&port, true);
    let frame = data_frame();

    assert_eq!(port.capabilities(), esp32s31_lower_mac_capabilities(512));
    assert_eq!(
        submit(&port, attempt(&port, 1, &frame)),
        Ok(Err(SubmitError::Disabled))
    );
    assert_eq!(port.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );

    // A NoAck completion is reported, not retried.
    assert_eq!(submit(&port, attempt(&port, 1, &frame)), Ok(Ok(())));
    // The one spare slot is published until the attempt completes.
    assert!(matches!(port.tx_buffer(frame.len()), Ok(None)));
    with_hardware(&port, |hardware| {
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(5, 0));
    });
    port.on_interrupt(EVENT_TX_COMPLETE);
    let Ok(LowerMacEvent::TxCompleted(completion)) = next(&port) else {
        panic!("the attempt completes");
    };
    assert_eq!(completion.id, TxId(1));
    assert_eq!(completion.status, TxStatus::AckTimeout);
    assert_eq!(with_hardware(&port, |hardware| hardware.legacy.len()), 1);
    let spare = port.tx_buffer(frame.len()).unwrap().unwrap();
    port.release_tx_buffer(spare);

    let seven = VifTsf::new(STA, oer_ieee80211_mac::tsf::TsfInstant::from_micros(7));
    assert_eq!(port.set_tsf(seven), Ok(Ok(())));
    assert_eq!(port.tsf(STA), Ok(Ok(seven)));
    // The port's radio clock is the MAC local time.
    assert_eq!(
        port.now(),
        Ok(Ieee80211Instant::from_micros(u64::from(MAC_AT_EPOCH)))
    );
    assert!(matches!(
        port.clock_info().epoch,
        oer_ieee80211_lower_mac::RadioEpoch::Affine { .. }
    ));
    let sample = port.clock_sample().unwrap();
    assert_eq!(
        sample.radio,
        Ieee80211Instant::from_micros(u64::from(MAC_AT_EPOCH))
    );
    assert_eq!(sample.monotonic, oer_time::Instant::EPOCH);
}

#[test]
fn enable_after_a_channel_change_retunes_in_the_runner() {
    let port = Port::new(ModelTimer);
    let tuned = install(&port, true);

    assert_eq!(
        port.apply(LowerMacSetting::Channel(
            Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()
        )),
        Ok(Err(SettingError::UnsupportedChannel))
    );
    port.apply(LowerMacSetting::Channel(
        Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap(),
    ))
    .unwrap()
    .unwrap();
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    assert_eq!(*tuned.lock().unwrap(), [WifiChannel::mhz20(11).unwrap()]);
}

#[test]
fn a_refused_retune_fails_enable_recoverably() {
    let port = Port::new(ModelTimer);
    install(&port, false);
    port.apply(LowerMacSetting::Channel(
        Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap(),
    ))
    .unwrap()
    .unwrap();
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Failed {
            command: LifecycleCommand::Enable,
            class: FailureClass::Recoverable,
        }))
    );
    // The port is usable and disabled.
    assert_eq!(
        submit(&port, attempt(&port, 1, &data_frame())),
        Ok(Err(SubmitError::Disabled))
    );
    assert_eq!(port.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
}

#[test]
fn station_tbtts_arrive_through_the_power_interrupt() {
    let port = Port::new(ModelTimer);
    install(&port, true);
    let tbtt = oer_esp32s31_hal::types::MacPowerInterruptObservation::from_semantic_events(
        false, false, false, false, true, false,
    );
    // Without a schedule the edge reports nothing.
    port.on_power_interrupt(tbtt);
    assert!(port.queues.take().is_none());

    let schedule = TbttSchedule {
        next: VifTsf::new(
            STA,
            oer_ieee80211_mac::tsf::TsfInstant::from_micros(1_000_000),
        ),
        beacon_interval: oer_ieee80211_mac::tsf::time_units(100),
        lead: oer_time::Duration::from_micros(3_000),
    };
    assert_eq!(port.set_tbtt(schedule), Ok(Ok(())));
    assert_eq!(
        with_hardware(&port, |hardware| hardware
            .tbtt
            .map(|tbtt| tbtt.interval_micros)),
        Some(102_400)
    );
    with_hardware(&port, |hardware| hardware.station_tsf = 997_000);
    port.on_power_interrupt(tbtt);
    let event = next_owned(&port).unwrap();
    assert_eq!(Port::view(event), LowerMacEvent::Extension);
    assert_eq!(
        Port::tbtt(event),
        Some(TbttEvent {
            tbtt: VifTsf::new(
                STA,
                oer_ieee80211_mac::tsf::TsfInstant::from_micros(1_000_000)
            )
        })
    );
    assert_eq!(
        port.beacon_timing_capabilities().tbtt,
        oer_ieee80211_lower_mac::VifRoleSet::STATION
    );
}

#[test]
fn received_frames_are_copied_and_overflow_is_reported_once() {
    let port = Port::new(ModelTimer);
    install(&port, true);
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    // A frame to the station: the station's receive rules admit it.
    let mut mpdu = data_frame();
    mpdu[4..10].copy_from_slice(&STATION);
    mpdu[10..16].copy_from_slice(&BSSID);
    let received = |mpdu: &'static [u8], stamp: Option<u32>| NormalizedRxFrame {
        mpdu,
        metadata: MacRxMetadata {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::<RxPhyInfo>::Unavailable,
            rssi_dbm: MacRxEvidence::HardwareObserved(-50),
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::Unavailable,
            ampdu: MacRxEvidence::Unavailable,
            amsdu: MacRxEvidence::Unavailable,
        },
        logical_length: mpdu.len(),
        stamp,
    };
    let mpdu: &'static [u8] = std::boxed::Box::leak(std::boxed::Box::new(mpdu));
    // A frame to the station as well, one byte longer than `FRAME`.
    let mut long = [0_u8; 65];
    long[..mpdu.len()].copy_from_slice(mpdu);
    let long: &'static [u8] = std::boxed::Box::leak(std::boxed::Box::new(long));

    // A frame longer than `FRAME` is reported as such, not as a loss.
    port.on_received(&received(long, None));
    assert_eq!(next(&port), Ok(LowerMacEvent::RxTooLong { length: 65 }));

    // Two fit the queue, the third is lost and reported after them. The
    // first was stamped now, the second carries a stamp later than now,
    // which has no place.
    port.on_received(&received(mpdu, Some(MAC_AT_EPOCH)));
    port.on_received(&received(mpdu, Some(MAC_AT_EPOCH + 1_000)));
    port.on_received(&received(mpdu, None));
    let mut stamps = std::vec::Vec::new();
    for _ in 0..2 {
        let Ok(LowerMacEvent::Received { frame, meta }) = next(&port) else {
            panic!("a received frame");
        };
        assert_eq!(frame, mpdu);
        assert_eq!(
            meta.channel,
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
        );
        stamps.push(meta.timestamp);
    }
    let sample = port.clock_sample().unwrap();
    let RxEvidence::HardwareObserved(stamp) = stamps[0] else {
        panic!("the receive stamp is a hardware observation");
    };
    assert_eq!(
        stamp.at,
        Ieee80211Instant::from_micros(u64::from(MAC_AT_EPOCH))
    );
    assert_eq!(
        port.clock_info()
            .to_monotonic_with(stamp, &sample)
            .map(|projected| projected.at),
        Ok(oer_time::Instant::EPOCH)
    );
    assert_eq!(stamps[1], RxEvidence::Unavailable);
    assert_eq!(next(&port), Err(EventsLost));
    assert!(port.queues.take().is_none());
}

#[test]
fn a_receive_overflow_does_not_drop_completions() {
    let port = Port::new(ModelTimer);
    install(&port, true);
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    let mut mpdu = data_frame();
    mpdu[4..10].copy_from_slice(&STATION);
    mpdu[10..16].copy_from_slice(&BSSID);
    let mpdu: &'static [u8] = std::boxed::Box::leak(std::boxed::Box::new(mpdu));
    assert_eq!(submit(&port, attempt(&port, 7, &data_frame())), Ok(Ok(())));
    for _ in 0..5 {
        port.on_received(&NormalizedRxFrame {
            mpdu,
            metadata: MacRxMetadata {
                channel: MacRxEvidence::Unavailable,
                rate: MacRxEvidence::<RxPhyInfo>::Unavailable,
                rssi_dbm: MacRxEvidence::HardwareObserved(-50),
                crypto: MacRxEvidence::Unavailable,
                s_mpdu: MacRxEvidence::Unavailable,
                ampdu: MacRxEvidence::Unavailable,
                amsdu: MacRxEvidence::Unavailable,
            },
            logical_length: mpdu.len(),
            stamp: None,
        });
    }
    with_hardware(&port, |hardware| {
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    });
    port.on_interrupt(EVENT_TX_COMPLETE);
    // The completion has its own queue and is taken first.
    let Ok(LowerMacEvent::TxCompleted(completion)) = next(&port) else {
        panic!("the completion survives the receive burst");
    };
    assert_eq!(completion.id, TxId(7));
}

/// Poison the port through the locked entry, as a core fault does.
fn poison(port: &Port) {
    assert!(matches!(
        port.with_core(|_, _, _| Err::<(), _>(LowerMacFault::RxBlockAckReadback)),
        Err(Esp32s31LowerMacError::Poisoned(_))
    ));
}

#[test]
fn a_poisoned_port_reports_queued_events_then_its_terminal_event() {
    let port = Port::new(ModelTimer);
    assert_eq!(
        port.lifecycle(LifecycleCommand::Enable)
            .unwrap_err()
            .class(),
        FailureClass::Rejected
    );
    install(&port, true);
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    poison(&port);
    // The terminal event of the command admitted before the fault first.
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    for _ in 0..2 {
        assert_eq!(next(&port), Ok(LowerMacEvent::Poisoned(Poisoned)));
    }
    let error = port.tx_buffer(26).unwrap_err();
    assert_eq!(error.class(), FailureClass::Poisoned);
    assert!(port.cancel(TxId(1)).unwrap_err().is_poisoned());
}

/// The runner of a poisoned port with an expired publication deadline
/// waits instead of servicing the deadline again and again.
#[test]
fn the_runner_of_a_poisoned_port_does_not_spin_on_an_expired_deadline() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let port: &'static Port =
            std::boxed::Box::leak(std::boxed::Box::new(Port::new(ModelTimer)));
        install_with_timeout(port, oer_time::Duration::from_micros(0));
        port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
        let _ = next(port);
        assert_eq!(submit(port, attempt(port, 1, &data_frame())), Ok(Ok(())));
        poison(port);
        let mut run = core::pin::pin!(port.run());
        let pending = (0..3).all(|_| {
            run.as_mut()
                .poll(&mut core::task::Context::from_waker(
                    core::task::Waker::noop(),
                ))
                .is_pending()
        });
        let _ = done.send(pending);
    });
    assert_eq!(
        finished.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(true)
    );
}

/// A deadline the core keeps after servicing it (its clock has not reached
/// it) waits for the next wake instead of being serviced at once again.
#[test]
fn the_runner_services_a_kept_deadline_once_per_wake() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let port: &'static Port =
            std::boxed::Box::leak(std::boxed::Box::new(Port::new(ModelTimer)));
        install_with_timeout(port, oer_time::Duration::from_micros(0));
        port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
        let _ = next(port);
        assert_eq!(submit(port, attempt(port, 1, &data_frame())), Ok(Ok(())));
        let mut run = core::pin::pin!(port.run());
        let pending = (0..3).all(|_| {
            run.as_mut()
                .poll(&mut core::task::Context::from_waker(
                    core::task::Waker::noop(),
                ))
                .is_pending()
        });
        let _ = done.send(pending);
    });
    assert_eq!(
        finished.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(true)
    );
}

#[test]
fn uninstall_keeps_the_loss_of_discarded_and_owed_events() {
    let port = Port::new(ModelTimer);
    install(&port, true);
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert!(port.uninstall().is_some());
    install(&port, true);
    // The Enabled terminal was discarded: the consumer learns of the gap.
    assert_eq!(next(&port), Err(EventsLost));
    assert!(port.queues.take().is_none());
}

const BACKING: usize = 256;
const BACKINGS: usize = 4;

type Backing =
    ReturningStableDmaBacking<PinnedDmaTxRadioLease<'static, BACKING, 0, 0>, &'static FreeBackings>;

#[derive(Default)]
struct FreeBackings(TestCell<Vec<u8>>);

impl DmaIndexReturn for &'static FreeBackings {
    fn return_index(&self, index: u8) {
        self.0.borrow_mut().push(index);
    }
}

/// Subframe memory from a pinned DMA TX pool.
struct Backings {
    pool: &'static PinnedDmaTxPool<BACKING, 0, 0, BACKINGS>,
    free: &'static FreeBackings,
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

type AmpduPort = Esp32s31LowerMac<
    'static,
    CriticalSectionRawMutex,
    Power,
    fn() -> u32,
    ModelTimer,
    Hardware,
    Retune,
    512,
    4,
    8,
    64,
    Backings,
    2,
    1,
>;

fn install_ampdu(port: &AmpduPort) -> &'static Backings {
    let pool = PinnedDmaTxPool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let free: &'static FreeBackings = std::boxed::Box::leak(std::boxed::Box::default());
    free.0.borrow_mut().extend(0..BACKINGS as u8);
    let backings: &'static Backings = std::boxed::Box::leak(std::boxed::Box::new(Backings {
        pool: Pin::into_ref(pool).get_ref(),
        free,
    }));
    let owner: Esp32s31AmpduOwner<'static, Backing, 2> = RetainedDmaAmpduTx::new_model(
        Pin::static_mut(std::boxed::Box::leak(std::boxed::Box::new(
            HtAmpduTxStorage::new(),
        ))),
        std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
    )
    .unwrap();
    let core = LowerMacCore::with_ampdu(
        OrdinaryTxOwner::new(WifiTxResources {
            slot: slot(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: ModelTimer,
        }),
        [slot(), slot(), slot(), slot()],
        [owner],
        backings,
        LowerMacConfig {
            station_address: STATION,
            channel: WifiChannel::mhz20(6).unwrap(),
            publication_timeout: oer_time::Duration::from_micros(250_000),
            tsf_epoch: 1,
        },
        &BeaconWindowZero,
    );
    let tuned = std::boxed::Box::leak(std::boxed::Box::new(std::sync::Mutex::new(Vec::new())));
    assert!(
        port.install(Esp32s31LowerMacParts {
            core,
            hardware: Hardware::default(),
            retune: Retune {
                accept: true,
                tuned,
            },
        })
        .is_ok()
    );
    port.apply(LowerMacSetting::Vif {
        vif: STA,
        config: Some(VifConfig {
            address: STATION,
            role: VifRole::Station,
            bssid: Some(BSSID),
            receive: ReceiveFilter::BSS_MEMBER,
        }),
    })
    .unwrap()
    .unwrap();
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        take(port, port.run())
            .map(|event| event.portable() == LowerMacEvent::Lifecycle(LifecycleEvent::Enabled)),
        Ok(true)
    );
    backings
}

fn next_completion(port: &AmpduPort) -> TxCompletion {
    match take(port, port.run()) {
        Ok(Esp32s31LowerMacEvent::TxCompleted(completion)) => completion,
        _ => panic!("a completion"),
    }
}

#[test]
fn attempts_on_different_queues_complete_by_their_identity() {
    let port = AmpduPort::new(ModelTimer);
    let backings = install_ampdu(&port);
    assert_eq!(port.capabilities().tx_queues, 4);
    let capabilities = port.ampdu_capabilities();
    assert_eq!(capabilities.max_subframes, 2);
    assert_eq!(
        capabilities.formats,
        PhyFormatSet::HT.union(PhyFormatSet::HE)
    );

    // An MPDU on the voice queue.
    let mut buffer = port.tx_buffer(26).unwrap().unwrap();
    buffer.frame_mut().copy_from_slice(&data_frame());
    let mpdu = TxAttempt {
        id: TxId(1),
        vif: STA,
        access_category: WmmAccessCategory::Voice,
        payload: TxPayload {
            frame: buffer,
            response: TxResponse::Ack,
        },
        rate: PhyRate::Legacy(LegacyRate::Ofdm12M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    };
    assert!(matches!(port.submit(mpdu), Ok(Ok(()))));

    // An aggregate of two MPDUs on the best-effort queue.
    let mut aggregate = port.ampdu_buffer().unwrap().unwrap();
    assert!(matches!(port.ampdu_buffer(), Ok(None)));
    for sequence in [100_u16, 101] {
        let mpdu = aggregate.push_mpdu(26).unwrap();
        mpdu.copy_from_slice(&data_frame());
        mpdu[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    }
    assert!(aggregate.push_mpdu(26).is_none());
    let ampdu = TxAttempt {
        id: TxId(2),
        vif: STA,
        access_category: WmmAccessCategory::BestEffort,
        payload: AmpduPayload {
            subframes: aggregate,
            tid: 0,
            min_mpdu_start_spacing: 0,
        },
        rate: PhyRate::Ht(
            HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
        ),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    };
    assert!(matches!(port.submit_ampdu(ampdu), Ok(Ok(()))));
    assert_eq!(backings.free.0.borrow().len(), BACKINGS - 2);
    assert_eq!(
        with_hardware_of(&port, |hardware| (hardware.legacy.len(), hardware.ht.len())),
        (1, 1)
    );

    // The aggregate ends first, then the MPDU.
    with_hardware_of(&port, |hardware| {
        hardware.block_ack_completion[BE] = Some(MacHtAmpduCompletionObservation::new_model(
            MacTxCompletionObservation::new_model(0, 0),
            0,
            100,
            0b11,
            true,
        ));
    });
    port.on_interrupt(EVENT_TX_COMPLETE);
    let completion = next_completion(&port);
    assert_eq!(completion.id, TxId(2));
    assert_eq!(
        completion.block_ack,
        Some(BlockAckReport {
            start_sequence: SequenceNumber::new(100).unwrap(),
            bitmap: 0b11,
        })
    );
    assert_eq!(backings.free.0.borrow().len(), BACKINGS);
    with_hardware_of(&port, |hardware| {
        hardware.completion[VO] = Some(MacTxCompletionObservation::new_model(0, 0));
    });
    port.on_interrupt(EVENT_TX_COMPLETE);
    assert_eq!(next_completion(&port).id, TxId(1));

    // The aggregate owner is lent again; a refused aggregate comes back.
    let empty = port.ampdu_buffer().unwrap().unwrap();
    let Ok(Err(refused)) = port.submit_ampdu(TxAttempt {
        id: TxId(3),
        vif: STA,
        access_category: WmmAccessCategory::BestEffort,
        payload: AmpduPayload {
            subframes: empty,
            tid: 0,
            min_mpdu_start_spacing: 0,
        },
        rate: PhyRate::Ht(
            HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
        ),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    }) else {
        panic!("an empty aggregate is refused");
    };
    assert_eq!(refused.error, SubmitError::InvalidLength);
    port.release_ampdu_buffer(refused.attempt.payload.subframes);
    assert!(matches!(port.ampdu_buffer(), Ok(Some(_))));
}

fn with_hardware_of<U>(port: &AmpduPort, entry: impl FnOnce(&mut Hardware) -> U) -> U {
    port.installed
        .lock(|installed| entry(&mut installed.borrow_mut().as_mut().unwrap().hardware))
}
