use core::{
    future::{Future, ready},
    pin::pin,
};

use open_esp_radio_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{OrdinaryTxError, WifiTxPowerPair};
use open_esp_radio_esp32s31_wifi_mac::{
    ap_policy::ApRxPolicyHardware,
    crypto::CcmpKeyHardware,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot},
    tx_protection::{
        ErpProtectionMode, HtProtectionMode, TxProtectionAdmissionError, TxProtectionMechanism,
        TxProtectionReason,
    },
    tx_runtime::WifiTxRuntimePolicy,
};
use open_esp_radio_ieee80211::{
    ap::ApAssociationSecurityObservation, beacon::WPA2_BEACON_CAPACITY, channel::WifiChannel,
    ssid::WifiSsid,
};
use open_esp_radio_wifi_ap::{AccessPointService, ApAssociationCapabilities};
use open_esp_radio_wpa2::{Pmk, frames::Wpa2Gtk};

use super::*;

#[derive(Default)]
struct Hardware {
    completion: Option<MacTxCompletionObservation>,
    publications: u8,
}

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _address: [u8; 6]) {}

    fn disable_ap_link_policy(&mut self) {}
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _index: u8,
        _identity: open_esp_radio_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

impl open_esp_radio_esp32s31_wifi_mac::ap_tsf::ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(&mut self) {}

    fn stop_access_point_tsf(&mut self) {}
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications = self.publications.saturating_add(1);
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
        descriptor: u32,
        reason: MacTxDetachReason,
        detached: impl for<'a> FnOnce(MacTxQueueDetached<'a>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Completed => {
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(descriptor)))
            }
            _ => MacTxDetachOutcome::NoEvent,
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
        0
    }

    fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
    }

    fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

#[test]
fn prepared_beacon_becomes_evidence_only_after_terminal_success() {
    let ap = [2, 0, 0, 0, 0, 1];
    let mut hardware = Hardware::default();
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::Esp32s31ApPairwiseKeyStorage::new();
    let engine = Esp32s31ApEngine::start(
        &mut hardware,
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            Wpa2Gtk::new(1, true, [7; 16]).unwrap(),
            open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
            open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
            &mut peers,
        ),
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        open_esp_radio_ieee80211::channel::WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("AP engine starts"));
    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = Esp32s31ApMac::new(
        engine,
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

    mac.publish_beacon(&mut hardware, 102_400).unwrap();
    assert_eq!(mac.observation().beacons_transmitted, 0);
    hardware.completion = Some(MacTxCompletionObservation::new_model(0, 0));
    let (progress, action) = mac
        .service_tx(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
            1,
        )
        .unwrap();
    assert_eq!(progress, WifiTxProgress::Complete);
    assert_eq!(action, Esp32s31ApTxCompletionAction::None);
    assert_eq!(mac.observation().beacons_transmitted, 1);
    assert!(mac.try_into_parts().is_ok());
}

#[test]
fn mixed_bss_protection_rejects_ordinary_and_amsdu_before_sequence_pn_or_dma() {
    let ap = [2, 0, 0, 0, 0, 1];
    let target = [2, 0, 0, 0, 0, 2];
    let legacy = [2, 0, 0, 0, 0, 3];
    let mut hardware = Hardware::default();
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::Esp32s31ApPairwiseKeyStorage::new();
    let mut service = AccessPointService::new_open(
        ap,
        open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
        open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    let open = ApAssociationSecurityObservation {
        privacy: false,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };
    service.authenticate_open(target, 1);
    let ht_ie = open_esp_radio_ieee80211::ht::ht_capability_ie(
        crate::profile::HT_CAPABILITIES,
        WifiChannel::mhz20(6).unwrap(),
    );
    service
        .associate_open(
            target,
            open,
            ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 108,
                ht: open_esp_radio_ieee80211::ht::ht_peer_capabilities(&ht_ie),
                qos_supported: true,
            },
            2,
        )
        .unwrap();
    service.authenticate_open(legacy, 3);
    service
        .associate_open(
            legacy,
            open,
            ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 22,
                ht: None,
                qos_supported: false,
            },
            4,
        )
        .unwrap();
    let engine = Esp32s31ApEngine::start(
        &mut hardware,
        service,
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("Open mixed AP starts"));
    assert_eq!(
        engine.tx_protection_policy().erp(),
        ErpProtectionMode::CtsToSelf
    );
    assert_eq!(
        engine.tx_protection_policy().ht(),
        HtProtectionMode::NonHtMixed
    );

    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = Esp32s31ApMac::new(
        engine,
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
    let mut first = [0_u8; 18];
    first[..6].copy_from_slice(&target);
    first[6..12].copy_from_slice(&ap);
    first[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    first[14..].copy_from_slice(&[1, 2, 3, 4]);
    let mut second = first;
    second[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 7]);
    second[14..].copy_from_slice(&[5, 6, 7, 8]);
    let data_sequence = mac.engine.current_data_sequence();
    let qos_sequence = mac.engine.current_qos_sequence(target, 0);
    let mut scratch = [0_u8; 256];

    let ordinary = mac
        .publish_ethernet(&mut hardware, target, &first, &mut scratch)
        .unwrap_err();
    let Esp32s31ApMacError::Transmit(Esp32s31ApTxError::Ordinary(OrdinaryTxError::Protection(
        TxProtectionAdmissionError::PhysicalPublicationUnverified { request },
    ))) = ordinary
    else {
        panic!("unexpected ordinary protection result: {ordinary:?}");
    };
    assert_eq!(request.mechanism, TxProtectionMechanism::CtsToSelf);
    assert_eq!(request.reason, TxProtectionReason::ErpUseProtection);
    assert_eq!(mac.engine.current_data_sequence(), data_sequence);
    assert_eq!(mac.engine.current_qos_sequence(target, 0), qos_sequence);

    let amsdu = mac
        .publish_amsdu_pair(&mut hardware, &first, &second, &mut scratch)
        .unwrap_err();
    assert!(matches!(
        amsdu,
        Esp32s31ApMacError::Transmit(Esp32s31ApTxError::Ordinary(OrdinaryTxError::Protection(
            TxProtectionAdmissionError::PhysicalPublicationUnverified { .. }
        )))
    ));
    assert_eq!(mac.engine.current_data_sequence(), data_sequence);
    assert_eq!(mac.engine.current_qos_sequence(target, 0), qos_sequence);
    assert_eq!(hardware.publications, 0);
    assert!(!mac.tx_pending());
}
