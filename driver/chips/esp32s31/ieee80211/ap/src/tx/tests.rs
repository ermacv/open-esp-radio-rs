use core::{
    future::{Future, ready},
    pin::pin,
};

use open_esp_radio_esp32s31_hal::types::{
    MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, TxSlotState},
    tx_runtime::WifiTxRuntimePolicy,
};

use super::*;

#[derive(Default)]
struct Hardware {
    prepare: bool,
    publications: u8,
    legacy_program: Option<MacLegacyTxProgram>,
    completion: Option<MacTxCompletionObservation>,
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy_program = Some(program);
        self.prepare
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications += 1;
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
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

#[derive(Clone, Copy)]
struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 1,
            alternate: 1,
        }
    }
}

struct Timer;

impl WifiTxTimer for Timer {
    fn now_micros(&self) -> u64 {
        1
    }

    fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
    }

    fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

#[test]
fn peer_rate_mapping_preserves_every_advertised_bg_rate() {
    assert_eq!(Esp32s31ApTxClass::Data.initial_rate(), LegacyRate::Ofdm24M);
    assert_eq!(peer_legacy_rate(108), LegacyRate::Ofdm54M);
    assert_eq!(peer_legacy_rate(96), LegacyRate::Ofdm48M);
    assert_eq!(peer_legacy_rate(72), LegacyRate::Ofdm36M);
    assert_eq!(peer_legacy_rate(48), LegacyRate::Ofdm24M);
    assert_eq!(peer_legacy_rate(22), LegacyRate::Cck11MLong);
    assert_eq!(peer_legacy_rate(0), LegacyRate::Dsss1MLong);
    for class in [
        Esp32s31ApTxClass::Beacon,
        Esp32s31ApTxClass::Management,
        Esp32s31ApTxClass::Eapol,
    ] {
        assert_eq!(class.initial_rate(), LegacyRate::Dsss1MLong);
    }
    assert_eq!(
        Esp32s31ApTxClass::Beacon.publication_limit(LegacyRate::Dsss1MLong),
        1
    );
    assert_eq!(
        Esp32s31ApTxClass::Management.publication_limit(LegacyRate::Dsss1MLong),
        32
    );
    assert_eq!(
        Esp32s31ApTxClass::Data.publication_limit(LegacyRate::Ofdm54M),
        32
    );
}

#[test]
fn peer_ht_rate_requires_matching_bss_and_peer_width() {
    use open_esp_radio_ieee80211::ht::{ht_capability_ie, ht_peer_capabilities};

    let ht40 = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
    let wide_peer =
        ht_peer_capabilities(&ht_capability_ie(crate::profile::HT_CAPABILITIES, ht40)).unwrap();
    assert_eq!(
        peer_ht_rate(ht40, wide_peer),
        Some(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );

    let narrow_peer = ht_peer_capabilities(&ht_capability_ie(
        crate::profile::HT_CAPABILITIES,
        WifiChannel::mhz20(6).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        peer_ht_rate(ht40, narrow_peer),
        Some(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        ))
    );
}

#[test]
fn ap_mcs32_request_reaches_the_shared_frontier_without_replacing_fallback() {
    use open_esp_radio_esp32s31_wifi_mac::tx::{
        HtDuplicateTxEvidenceGaps, HtDuplicateTxRejection, HtDuplicateTxUnavailable,
    };
    use open_esp_radio_ieee80211::ht::{HtDuplicateMcs32, ht_capability_ie, ht_peer_capabilities};

    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
    let mut capability = ht_capability_ie(crate::profile::HT_CAPABILITIES, channel);
    HtDuplicateMcs32::new().advertise_receive_only(&mut capability);
    let peer = ht_peer_capabilities(&capability).unwrap();
    let fallback = peer_ht_rate(channel, peer).unwrap();
    assert_eq!(fallback.mcs, HtMcs::Mcs7);
    assert_eq!(fallback.channel_width, HtChannelWidth::Mhz40);
    assert_eq!(
        peer_ht_duplicate_rate(channel, peer),
        Some(HtDuplicateRate::new(HtGuardInterval::Short400Ns))
    );

    let request = HtDuplicateCertificationRequest::new(
        HtChannelWidth::Mhz40,
        HtGuardInterval::Short400Ns,
        5_484,
    );
    let selection = peer_ht_duplicate_tx_selection(channel, Some(peer), Some(request));
    assert_eq!(selection.plan(), None);
    assert_eq!(
        selection.rejection(),
        Some(HtDuplicateTxRejection::Hardware(
            HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(
                HtDuplicateTxEvidenceGaps::ESP32S31,
            )
        ))
    );
    assert_eq!(peer_ht_rate(channel, peer), Some(fallback));
}

#[test]
fn idle_ap_tx_lends_and_resumes_the_exact_ordinary_owner() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let request = HtDuplicateCertificationRequest::new(
        HtChannelWidth::Mhz40,
        HtGuardInterval::Long800Ns,
        5_484,
    );
    let mut tx = Esp32s31ApTx::new(
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: Timer,
        },
        Esp32s31ApTxConfig {
            publication_timeout_micros: 7_500,
        },
    );
    tx.set_ht_duplicate_certification_request(Some(request));

    let (resources, parked) = tx
        .try_park()
        .unwrap_or_else(|_| panic!("idle AP TX must lend its descriptor"));
    assert_eq!(resources.slot.state(), TxSlotState::Free);

    let tx = Esp32s31ApTx::resume(resources, parked);
    assert_eq!(tx.publication_timeout_micros(), 7_500);
    assert_eq!(tx.ht_duplicate_certification_request(), Some(request));
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    assert!(tx.try_into_resources().is_ok());
}

#[test]
fn required_protection_blocks_ap_aggregate_and_ordinary_retry_series() {
    use open_esp_radio_esp32s31_wifi_mac::tx_protection::{
        ErpProtectionMode, HtProtectionMode, TxProtectionAdmissionError, TxProtectionMechanism,
        TxProtectionReason, TxProtectionRequest, WifiTxProtectionPolicy,
    };

    let mut slot = pin!(TxSlot::<256>::new_model());
    let mut tx = Esp32s31ApTx::new(
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: Timer,
        },
        Esp32s31ApTxConfig {
            publication_timeout_micros: 1_000,
        },
    );
    tx.install_tx_protection_policy(WifiTxProtectionPolicy::new(
        ErpProtectionMode::None,
        HtProtectionMode::NonHtMixed,
        None,
    ));
    let rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz20,
    );

    assert_eq!(
        tx.require_unprotected_ht_aggregate(rate),
        Err(TxProtectionAdmissionError::PhysicalPublicationUnverified {
            request: TxProtectionRequest {
                mechanism: TxProtectionMechanism::RtsCts,
                reason: TxProtectionReason::Ht(HtProtectionMode::NonHtMixed),
            },
        })
    );
    tx.install_tx_protection_policy(WifiTxProtectionPolicy::new(
        ErpProtectionMode::CtsToSelf,
        HtProtectionMode::None,
        None,
    ));
    assert_eq!(
        tx.require_unprotected_data_retry_series(LegacyRate::Ofdm24M, false),
        Err(Esp32s31ApTxError::Ordinary(OrdinaryTxError::Protection(
            TxProtectionAdmissionError::PhysicalPublicationUnverified {
                request: TxProtectionRequest {
                    mechanism: TxProtectionMechanism::CtsToSelf,
                    reason: TxProtectionReason::ErpUseProtection,
                },
            },
        ),))
    );
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
}

#[test]
fn beacon_is_one_publication_and_resources_return_only_after_completion() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let resources = WifiTxResources {
        slot: slot.as_mut(),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: || 0,
        timer: Timer,
    };
    let mut tx = Esp32s31ApTx::new(
        resources,
        Esp32s31ApTxConfig {
            publication_timeout_micros: 1_000,
        },
    );
    let mut hardware = Hardware {
        prepare: true,
        completion: Some(MacTxCompletionObservation::new_model(0, 0)),
        ..Hardware::default()
    };
    let mut beacon = [0; 24];
    beacon[4] = 0xff;
    assert_eq!(
        tx.start_encoded(&mut hardware, Esp32s31ApTxClass::Beacon, &beacon),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
    assert_eq!(hardware.publications, 1);
    assert_eq!(
        hardware.legacy_program.unwrap().interface(),
        MacInterface::AccessPoint
    );

    let progress = tx.service(
        &mut hardware,
        WifiTxWake::Interrupt {
            events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
        },
    );
    assert_eq!(progress, Ok(WifiTxProgress::Complete));
    assert!(tx.take_last_outcome().unwrap().is_success());
    assert!(tx.try_into_resources().is_ok());
}

#[test]
fn aggregate_retry_uses_the_next_edca_contention_window() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let resources = WifiTxResources {
        slot: slot.as_mut(),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: || u32::MAX,
        timer: Timer,
    };
    let mut tx = Esp32s31ApTx::new(
        resources,
        Esp32s31ApTxConfig {
            publication_timeout_micros: 1_000,
        },
    );
    let rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );

    let initial = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
    assert_eq!(initial.contention_window, 15);

    tx.record_aggregate_retry_failure();
    let retry = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
    assert_eq!(retry.contention_window, 31);

    tx.record_aggregate_success();
    let next_exchange = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
    assert_eq!(next_exchange.contention_window, 15);

    tx.record_aggregate_retry_failure();
    tx.reset_aggregate_contention();
    let after_terminal_failure = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
    assert_eq!(after_terminal_failure.contention_window, 15);
}
