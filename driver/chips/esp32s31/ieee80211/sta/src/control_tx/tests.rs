use core::{
    future::{Future, ready},
    pin::Pin,
};

use open_esp_radio_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, TxSlotState},
    tx_protection::{
        ErpProtectionMode, HtProtectionMode, TxProtectionAdmissionError, TxProtectionMechanism,
        TxProtectionReason, TxProtectionRequest, WifiTxProtectionPolicy,
    },
};
use open_esp_radio_ieee80211::station::StaTxSequenceCounters;

use super::*;
use crate::single_mpdu_tx::{ConnectedTxSecurity, SingleMpduTxConfig};
use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;

#[derive(Default)]
struct Hardware {
    prepare: bool,
    publications: u8,
    completions: [Option<MacTxCompletionObservation>; 2],
    completion_index: usize,
    timeout: bool,
    legacy: Option<(u8, MacLegacyTxProgram)>,
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy = Some((queue, program));
        self.prepare
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications += 1;
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        let completion = self.completions.get_mut(self.completion_index)?.take();
        if completion.is_some() {
            self.completion_index += 1;
        }
        completion
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        self.timeout
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Timeout if !self.timeout => MacTxDetachOutcome::NoEvent,
            MacTxDetachReason::Timeout => {
                self.timeout = false;
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Collision => MacTxDetachOutcome::NoEvent,
        }
    }
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

#[derive(Clone, Copy)]
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
    settled: u64,
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
        self.settled += micros;
        ready(())
    }
}

fn completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
}

fn make_tx<'a>(
    slot: Pin<&'a mut TxSlot<256>>,
) -> Esp32s31ControlTx<'a, Power, fn() -> u32, Timer, 256> {
    fn entropy() -> u32 {
        0x1234_5678
    }
    Esp32s31ControlTx::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        ControlTxConfig {
            unicast_attempt_limit: 2,
            completion_timeout_us: 10,
            poll_interval_us: 1,
        },
    )
}

#[test]
fn authentication_is_encoded_and_completed_by_the_shared_owner() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 7,
        },
    ));

    assert!(matches!(result, Ok(completion) if completion.status() == 0));
    assert_eq!(hardware.publications, 1);
    let (_, program) = hardware.legacy.expect("management publication");
    assert_eq!(program.interface(), MacInterface::Station);
    assert_eq!(program.scheduler_priority(), 1);
    assert_eq!(program.packet_priority(), 1);
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_eq!(&bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2], &[0xb0, 0]);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
        &[0x70, 0]
    );
}

#[test]
fn protected_control_preflight_rejects_before_encode_or_dma() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());
    tx.install_tx_protection_policy(WifiTxProtectionPolicy::new(
        ErpProtectionMode::CtsToSelf,
        HtProtectionMode::None,
        None,
    ));
    let result = crate::test_support::block_on(tx.transmit_protected_data(
        &mut hardware,
        StaProtectedDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 7,
            user_priority: 0,
            peer_qos: true,
            ccmp_header: [1, 0, 0, 0x20, 0, 0, 0, 0],
            ether_type: 0x888e,
            payload: &[1, 2, 3, 4],
        },
        LegacyTxQueue::Voice,
        TxPhyRate::Legacy(LegacyRate::Ofdm24M),
        1,
    ));
    assert_eq!(
        result,
        Err(ControlTxError::Protection(
            TxProtectionAdmissionError::PhysicalPublicationUnverified {
                request: TxProtectionRequest {
                    mechanism: TxProtectionMechanism::CtsToSelf,
                    reason: TxProtectionReason::ErpUseProtection,
                },
            },
        ))
    );
    assert_eq!(hardware.publications, 0);
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    assert!(
        tx.ordinary
            .slot
            .as_mut()
            .buffer_mut()
            .unwrap()
            .iter()
            .all(|byte| *byte == 0)
    );
}

#[test]
fn ack_timeout_reuses_sequence_and_marks_the_retry_bit() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(5)), Some(completion(0))],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 11,
        },
    ));

    assert!(result.is_ok());
    assert_eq!(hardware.publications, 2);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_ne!(bytes[TX_METADATA_SIZE + 1] & 0x08, 0);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
        &[0xb0, 0]
    );
}

#[test]
fn eapol_uses_the_recovered_voice_data_priority() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_unprotected_data(
        &mut hardware,
        StaDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 8,
            ether_type: 0x888e,
            payload: &[1, 2, 3, 4],
        },
    ));

    assert!(result.is_ok());
    let (_, program) = hardware.legacy.expect("EAPOL publication");
    assert_eq!(program.scheduler_priority(), 3);
    assert_eq!(program.packet_priority(), 3);
}

#[test]
fn missing_hardware_timeout_edge_quarantines_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    {
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_open_authentication(
            &mut hardware,
            OpenAuthenticationRequest {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: 13,
            },
        ));

        assert_eq!(
            result,
            Err(ControlTxError::RadioResetRequired(
                TxResetReason::ExecutorDeadline
            ))
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::ResetRequired);
        assert!(tx.ordinary.slot.as_mut().reserve(64, 32).is_err());
    }
    let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(slot)));
    assert!(drop_result.is_ok());
}

#[test]
fn passive_fallback_requires_a_proven_quiescent_tx_owner() {
    assert!(ControlTxError::HardwareTimeout.retains_quiescent_owner());
    assert!(ControlTxError::CollisionLimit.retains_quiescent_owner());
    assert!(!ControlTxError::Busy.retains_quiescent_owner());
    assert!(
        !ControlTxError::RadioResetRequired(TxResetReason::ExecutorDeadline)
            .retains_quiescent_owner()
    );
}

#[test]
fn connected_handoff_preserves_the_descriptor_and_association_policy() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut());
    tx.install_he_bss_color(37);
    let key = install_sta_pairwise_ccmp(
        &mut hardware,
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        &[0x5a; 16],
    )
    .unwrap();

    let connected = tx
        .try_into_connected(ConnectedTxHandoff {
            security: ConnectedTxSecurity::Wpa2Personal(key),
            sequences: StaTxSequenceCounters::new(9),
            config: SingleMpduTxConfig {
                station_address: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                peer_qos: true,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::BestEffort.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: 2,
                    publication_timeout_micros: 10,
                },
            },
        })
        .unwrap_or_else(|_| panic!("idle owner must transfer"));

    assert_eq!(connected.policy().he_bss_color(), 37);
    assert!(!connected.active());
}

#[test]
fn active_handoff_returns_tx_and_crypto_resources_for_later_retry() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());
    let frame_length = OpenAuthenticationRequest {
        source: [2, 3, 4, 5, 6, 7],
        bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: 15,
    }
    .encode(&mut tx.ordinary.buffer_mut().unwrap()[TX_METADATA_SIZE..])
    .unwrap();
    tx.ordinary
        .start(
            &mut hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::Voice.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                    publication_limit: 1,
                    publication_timeout_micros: 10,
                },
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                interface: open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
            },
        )
        .unwrap();
    let key = install_sta_pairwise_ccmp(
        &mut hardware,
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        &[0x5a; 16],
    )
    .unwrap();
    let key_index = key.hardware_index();
    let handoff = ConnectedTxHandoff {
        security: ConnectedTxSecurity::Wpa2Personal(key),
        sequences: StaTxSequenceCounters::new(9),
        config: SingleMpduTxConfig {
            station_address: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            peer_qos: true,
            exchange: MacTxPlan {
                access_category: LegacyTxQueue::BestEffort.access_category(),
                initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                publication_limit: 2,
                publication_timeout_micros: 10,
            },
        },
    };

    let (mut tx, handoff) = match tx.try_into_connected(handoff) {
        Err(resources) => resources,
        Ok(_) => panic!("hardware-owned descriptor must reject handoff"),
    };
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::HardwareOwned);
    assert!(matches!(
        &handoff.security,
        ConnectedTxSecurity::Wpa2Personal(key) if key.hardware_index() == key_index
    ));

    hardware.completions[0] = Some(completion(0));
    assert_eq!(
        crate::test_support::block_on(tx.ordinary.service_polling(&mut hardware, 1)),
        Ok(WifiTxProgress::Complete)
    );
    assert!(tx.try_into_connected(handoff).is_ok());
}
