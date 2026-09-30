use core::{future::ready, pin::Pin};
use std::vec::Vec;

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHtAmpduCompletionObservation, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacStaApReceivePlan, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use oer_esp32s31_ieee80211::{
    lower_mac::{LowerMacConfig, StationTsfHardware},
    ordinary_tx::{OrdinaryTxOwner, WifiTxPowerPair, WifiTxResources},
};
use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    crypto::CcmpKeyHardware,
    init::StaLinkRxPolicyHardware,
    irq::EVENT_TX_COMPLETE,
    rx::{
        RxPhyInfo,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxHardware, TxSlot, runtime::WifiTxRuntimePolicy},
};
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, KeySelector, MacAddress, PhyRate, Protection, ReceiveFilter, TxId,
    TxPayload, TxPower, TxResponse, TxStatus, VifConfig, VifRole,
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
        7
    }

    fn set_station_tsf(&mut self, _value: u64) {}
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

type Port<'a> = Esp32s31LowerMac<
    'a,
    CriticalSectionRawMutex,
    Power,
    fn() -> u32,
    ModelTimer,
    Hardware,
    Retune,
    512,
    2,
    64,
>;

fn install<'a>(
    port: &Port<'a>,
    slot: Pin<&'a mut TxSlot<512>>,
    accept_retune: bool,
) -> &'static std::sync::Mutex<Vec<WifiChannel>> {
    let tuned = std::boxed::Box::leak(std::boxed::Box::new(std::sync::Mutex::new(Vec::new())));
    let core = LowerMacCore::new(
        OrdinaryTxOwner::new(WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: entropy as fn() -> u32,
            timer: ModelTimer,
        }),
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

fn next(port: &Port<'_>) -> Result<LowerMacEvent<'static>, EventsLost> {
    // Views borrow the owned event; tests compare leaked copies.
    block_on(port.next_event()).map(|event| {
        let event: &'static _ = std::boxed::Box::leak(std::boxed::Box::new(event));
        Port::view(event)
    })
}

fn data_frame() -> [u8; 26] {
    let mut frame = [0; 26];
    frame[0] = 0x88;
    frame[4..10].copy_from_slice(&BSSID);
    frame[10..16].copy_from_slice(&STATION);
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
        rate: PhyRate::Legacy(LegacyRate::Ofdm12M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
    }
}

fn with_hardware<U>(port: &Port<'_>, entry: impl FnOnce(&mut Hardware) -> U) -> U {
    port.installed
        .lock(|installed| entry(&mut installed.borrow_mut().as_mut().unwrap().hardware))
}

#[test]
fn an_attempt_completes_through_the_interrupt_entry_and_the_queue() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let port = Port::new();
    install(&port, slot.as_mut(), true);
    let frame = data_frame();

    assert_eq!(port.capabilities(), ESP32S31_LOWER_MAC_CAPABILITIES);
    assert_eq!(
        port.submit(attempt(1, &frame)),
        Ok(Err(SubmitError::Disabled))
    );
    assert_eq!(port.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );

    // A NoAck completion is reported, not retried.
    assert_eq!(port.submit(attempt(1, &frame)), Ok(Ok(())));
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

    assert_eq!(port.tsf(STA), Ok(Ok(Tsf(7))));
    assert_eq!(port.now(), Ok(RadioInstant::from_micros(0)));
}

#[test]
fn enable_after_a_channel_change_retunes_in_next_event() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let port = Port::new();
    let tuned = install(&port, slot.as_mut(), true);

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
fn a_refused_retune_poisons_the_port() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let port = Port::new();
    install(&port, slot.as_mut(), false);
    port.apply(LowerMacSetting::Channel(
        Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap(),
    ))
    .unwrap()
    .unwrap();
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();

    // The retune runs inside the wait, before the queued event ends it.
    port.events
        .try_send(Esp32s31LowerMacEvent::Lifecycle(LifecycleEvent::Disabled))
        .unwrap();
    let _ = next(&port);
    assert_eq!(
        port.submit(attempt(1, &data_frame())),
        Err(Esp32s31LowerMacError::Poisoned(LowerMacFault::Retune))
    );
}

#[test]
fn received_frames_are_copied_and_overflow_is_reported_once() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let port = Port::new();
    install(&port, slot.as_mut(), true);
    port.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    assert_eq!(
        next(&port),
        Ok(LowerMacEvent::Lifecycle(LifecycleEvent::Enabled))
    );
    let mpdu = data_frame();
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
