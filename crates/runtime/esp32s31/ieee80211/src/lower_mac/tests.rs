use core::{future::ready, pin::Pin};
use std::vec::Vec;

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHtAmpduCompletionObservation, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacStaApReceivePlan, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached, StaTbttSchedule,
};
use oer_esp32s31_ieee80211::{
    lower_mac::{LowerMacConfig, StationTbttHardware, StationTsfHardware, TxGateHardware},
    ordinary_tx::{OrdinaryTxOwner, WifiTxPowerPair, WifiTxResources},
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
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxHardware, TxSlot, runtime::WifiTxRuntimePolicy},
};
use oer_ieee80211_lower_mac::{
    Backoff, Channel, ChannelWidth, CoexPriority, FailureClass, KeySelector, MacAddress, PhyRate,
    Protection, ReceiveFilter, SubmitError, TxAttempt, TxBuffer, TxId, TxPayload, TxPower,
    TxResponse, TxStatus, VifConfig, VifRole,
};
use oer_ieee80211_mac::{phy::LegacyRate, qos::WmmAccessCategory, sequence::SequenceNumber};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

use super::*;

const STATION: MacAddress = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const BSSID: MacAddress = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const STA: VifId = VifId(0);

/// Register owner model: records publications and returns the completion a
/// test installs; the receive-policy, key, Block Ack and TSF seams accept.
#[derive(Default)]
struct Hardware {
    legacy: Vec<MacLegacyTxProgram>,
    completion: Option<MacTxCompletionObservation>,
    station_tsf: u64,
    tbtt: Option<StaTbttSchedule>,
}

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

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn take_block_ack_completion(&mut self, _queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        None
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

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _access_point: [u8; 6]) {}
    fn disable_ap_link_policy(&mut self) {}
}

impl ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(&mut self) {}
    fn stop_access_point_tsf(&mut self) {}
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

/// A model clock far from any deadline, so the watchdog never fires.
#[derive(Default)]
struct ModelTimer;

impl WifiTxTimer for ModelTimer {
    fn now_micros(&self) -> u64 {
        0
    }

    fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
    }

    fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
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
            publication_timeout_micros: 250_000,
        },
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

fn next_owned(port: &Port) -> Result<&'static Esp32s31LowerMacEvent<64>, EventsLost> {
    // Views borrow the owned event; tests compare leaked copies.
    block_on(port.next_event()).map(|event| &*std::boxed::Box::leak(std::boxed::Box::new(event)))
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
    let mut buffer = port.tx_buffer(frame.len()).expect("a spare slot");
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
    let port = Port::new();
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
    // The one spare slot was published; the owner's previous slot is lent
    // next.
    let spare = port.tx_buffer(frame.len()).unwrap();
    assert!(port.tx_buffer(frame.len()).is_none());
    port.release_tx_buffer(spare);
    with_hardware(&port, |hardware| {
        hardware.completion = Some(MacTxCompletionObservation::new_model(5, 0));
    });
    port.on_interrupt(EVENT_TX_COMPLETE);
    let Ok(LowerMacEvent::TxCompleted(completion)) = next(&port) else {
        panic!("the attempt completes");
    };
    assert_eq!(completion.id, TxId(1));
    assert_eq!(completion.status, TxStatus::AckTimeout);
    assert_eq!(with_hardware(&port, |hardware| hardware.legacy.len()), 1);

    assert_eq!(port.set_tsf(STA, Tsf(7)), Ok(Ok(())));
    assert_eq!(port.tsf(STA), Ok(Ok(Tsf(7))));
    assert_eq!(port.now(), Ok(RadioInstant::from_micros(0)));
}

#[test]
fn enable_after_a_channel_change_retunes_in_next_event() {
    let port = Port::new();
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
    let port = Port::new();
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
    let port = Port::new();
    install(&port, true);
    let tbtt = oer_esp32s31_hal::types::MacPowerInterruptObservation::from_semantic_events(
        false, false, false, false, true, false,
    );
    // Without a schedule the edge reports nothing.
    port.on_power_interrupt(tbtt);
    assert!(port.events.try_receive().is_err());

    let schedule = TbttSchedule {
        beacon_interval_tu: 100,
        next: Tsf(1_000_000),
        lead_micros: 3_000,
    };
    assert_eq!(port.set_tbtt(STA, Some(schedule)), Ok(Ok(())));
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
            vif: STA,
            tsf: Tsf(1_000_000)
        })
    );
    assert_eq!(
        port.beacon_timing_capabilities().tbtt,
        oer_ieee80211_lower_mac::VifRoleSet::STATION
    );
}

#[test]
fn received_frames_are_copied_and_overflow_is_reported_once() {
    let port = Port::new();
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
    let received = |mpdu: &'static [u8]| NormalizedRxFrame {
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
    };
    let mpdu: &'static [u8] = std::boxed::Box::leak(std::boxed::Box::new(mpdu));
    let long: &'static [u8] = std::boxed::Box::leak(std::boxed::Box::new([0_u8; 65]));

    // Two fit the queue, the third is lost; a frame longer than `FRAME` is
    // lost as well.
    for _ in 0..3 {
        port.on_received(&received(mpdu));
    }
    port.on_received(&received(long));
    assert_eq!(next(&port), Err(EventsLost));
    for _ in 0..2 {
        let Ok(LowerMacEvent::Received { frame, meta }) = next(&port) else {
            panic!("a received frame");
        };
        assert_eq!(frame, mpdu);
        assert_eq!(
            meta.channel,
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
        );
    }
    assert!(port.events.try_receive().is_err());
}
