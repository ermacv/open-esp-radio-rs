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
    tx::{HardwareOwnedTxDma, LegacyRate, PreparedTxDma, TxSlot, TxSlotState},
    tx_protection::{
        ErpProtectionMode, HtProtectionMode, TxProtectionAdmissionError, TxProtectionMechanism,
        TxProtectionReason, TxProtectionRequest, WifiTxProtectionPolicy,
    },
    tx_runtime::VENDOR_SHORT_RETRY_LIMIT,
};
use open_esp_radio_ieee80211::{
    channel::WifiChannel,
    extensions::espressif::esp_now::{EspNowDestination, EspNowRandomValue, EspNowUnicastAddress},
};
use open_esp_radio_wifi_softmac::{
    EspNowConfig, EspNowPeerConfig, EspNowPhyMode, EspNowProtocol, MacTxResult,
    interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
};

use super::*;

const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

#[derive(Default)]
struct Hardware {
    prepare: bool,
    publications: u8,
    completion: Option<MacTxCompletionObservation>,
    timeout: bool,
    abort_requests: usize,
    timeout_detaches: usize,
    collision: bool,
    legacy: Option<(u8, MacLegacyTxProgram)>,
    cleared_key: Option<u8>,
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

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.cleared_key = Some(index);
    }
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
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        self.abort_requests += 1;
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
            MacTxDetachReason::Collision if !self.collision => MacTxDetachOutcome::NoEvent,
            MacTxDetachReason::Timeout => {
                self.timeout_detaches += 1;
                self.timeout = false;
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
            MacTxDetachReason::Collision => {
                self.collision = false;
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
        }
    }
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
struct TestTimer {
    now: u64,
    settled: u64,
    pending_wait: bool,
}

impl WifiTxTimer for TestTimer {
    fn now_micros(&self) -> u64 {
        self.now
    }

    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        core::future::poll_fn(move |_| {
            if self.pending_wait {
                core::task::Poll::Pending
            } else {
                self.now = deadline_micros;
                core::task::Poll::Ready(())
            }
        })
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

fn ethernet() -> [u8; 18] {
    [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x08, 0x00, 1, 2,
        3, 4,
    ]
}

fn entropy() -> u32 {
    0x1234_5678
}

fn make_tx<'a>(
    slot: Pin<&'a mut TxSlot<512>>,
    hardware: &mut Hardware,
    attempt_limit: u8,
) -> Esp32s31SingleMpduTx<'a, Power, fn() -> u32, TestTimer, 512> {
    let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
    Esp32s31SingleMpduTx::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: TestTimer::default(),
        },
        ConnectedTxHandoff {
            security: ConnectedTxSecurity::Wpa2Personal(key),
            sequences: StaTxSequenceCounters::new(7),
            config: SingleMpduTxConfig {
                station_address: [2, 3, 4, 5, 6, 7],
                bssid: BSSID,
                peer_qos: true,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::BestEffort.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: attempt_limit,
                    publication_timeout_micros: 250_000,
                },
            },
        },
    )
}

fn esp_now_protocol(
    phy_mode: EspNowPhyMode,
) -> (
    EspNowProtocol<1>,
    open_esp_radio_wifi_softmac::EspNowPeerId,
    BoundVirtualInterface,
    WifiChannel,
) {
    let station = BoundVirtualInterface::new(
        VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [2, 3, 4, 5, 6, 7]),
        ChannelContextId::PRIMARY,
    );
    let channel = WifiChannel::mhz20(1).unwrap();
    let mut protocol = EspNowProtocol::new(EspNowConfig::new(station, channel).unwrap());
    let peer = protocol
        .add_peer(
            EspNowPeerConfig::plaintext(
                EspNowDestination::Unicast(
                    EspNowUnicastAddress::new([0x30, 0x31, 0x32, 0x33, 0x34, 0x35]).unwrap(),
                ),
                channel,
            )
            .with_phy_mode(phy_mode),
        )
        .unwrap();
    (protocol, peer, station, channel)
}

#[test]
fn completion_releases_the_slot_and_network_lease_boundary() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);

    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);

    assert_eq!(
        tx.start(&mut hardware, &ethernet()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
    assert_eq!(hardware.publications, 1);
    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(matches!(
        tx.take_last_outcome(),
        Some(SingleMpduTxOutcome::Success(report))
            if report
                .completion
                .is_some_and(|completion| completion.status() == 0)
                && report.status.attempts == 1
    ));
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
}

#[test]
fn rejected_lr_frontier_does_not_consume_the_shared_sequence() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    let (protocol, peer, station, channel) = esp_now_protocol(EspNowPhyMode::LongRange);
    let config = Esp32s31EspNowTxConfig::new(4, 250_000).unwrap();

    let error = tx
        .start_esp_now_v1_plaintext(
            &mut hardware,
            &protocol,
            peer,
            EspNowRandomValue::new([1, 2, 3, 4]),
            &[9, 8, 7],
            channel,
            station,
            config,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        SingleMpduEspNowTxError::Backend(Esp32s31EspNowTxError::LongRangeUnsupported(_))
    ));
    assert_eq!(tx.sequences.peek_non_qos(), 7);
    assert_eq!(hardware.publications, 0);
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
}

#[test]
fn successful_esp_now_publication_commits_one_sequence_exactly_once() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    let (protocol, peer, station, channel) = esp_now_protocol(EspNowPhyMode::LegacyDsss1M);

    assert_eq!(
        tx.start_esp_now_v1_plaintext(
            &mut hardware,
            &protocol,
            peer,
            EspNowRandomValue::new([1, 2, 3, 4]),
            &[9, 8, 7],
            channel,
            station,
            Esp32s31EspNowTxConfig::new(4, 250_000).unwrap(),
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.sequences.peek_non_qos(), 8);
    assert_eq!(hardware.publications, 1);
}

#[test]
fn required_erp_protection_fails_before_sequence_dma_or_publication() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    tx.policy_mut()
        .install_protection(WifiTxProtectionPolicy::new(
            ErpProtectionMode::CtsToSelf,
            HtProtectionMode::None,
            None,
        ));
    let sequence = tx.sequences.peek_qos(0);

    assert_eq!(
        tx.start(&mut hardware, &ethernet()),
        Err(SingleMpduTxError::Protection(
            TxProtectionAdmissionError::PhysicalPublicationUnverified {
                request: TxProtectionRequest {
                    mechanism: TxProtectionMechanism::CtsToSelf,
                    reason: TxProtectionReason::ErpUseProtection,
                }
            }
        ))
    );
    assert_eq!(tx.sequences.peek_qos(0), sequence);
    assert_eq!(hardware.publications, 0);
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    assert_eq!(tx.slot_state(), TxSlotState::Free);
}

#[test]
fn dscp_selects_the_matching_hardware_queue_qos_tid_and_sequence_space() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    let mut frame = ethernet();
    frame[14] = 0x45;
    frame[15] = 46 << 2;

    assert_eq!(tx.start(&mut hardware, &frame), Ok(WifiTxProgress::Pending));
    let (queue, program) = hardware.legacy.expect("classified legacy queue image");
    assert_eq!(queue, LegacyTxQueue::Voice.hardware_index());
    assert_eq!(
        program.scheduler_priority(),
        LegacyTxQueue::Voice.vendor_data_scheduler_priority()
    );
    assert_eq!(
        program.packet_priority(),
        LegacyTxQueue::Voice.vendor_data_packet_priority()
    );
    assert_eq!(tx.sequences.peek_qos(6), Some(8));
    assert_eq!(tx.sequences.peek_qos(0), Some(7));

    hardware.completion = Some(completion(5));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.policy().contention_exponent(LegacyTxQueue::Voice), 3);
    assert_eq!(
        tx.policy().contention_exponent(LegacyTxQueue::BestEffort),
        4
    );

    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(tx.policy().contention_exponent(LegacyTxQueue::Voice), 2);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_eq!(bytes[TX_METADATA_SIZE + 24] & 0x0f, 6);

    let voice = tx.select_network_traffic(&frame).unwrap();
    assert!(matches!(
        tx.start_with_traffic(&mut hardware, &ethernet(), voice),
        Err(SingleMpduTxError::TrafficSelectionMismatch { provided, .. })
            if provided == voice
    ));
}

#[test]
fn idle_connected_owner_returns_descriptor_key_and_sequences_for_teardown() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let tx = make_tx(slot.as_mut(), &mut hardware, 4);

    let (resources, handoff) = match tx.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("idle connected TX must decompose"),
    };

    assert_eq!(resources.slot.state(), TxSlotState::Free);
    assert_eq!(handoff.sequences.peek_non_qos(), 7);
    let ConnectedTxSecurity::Wpa2Personal(key) = handoff.security else {
        panic!("WPA2 test owner must return its pairwise key");
    };
    let key_index = key.hardware_index();
    key.clear(&mut hardware);
    assert_eq!(hardware.cleared_key, Some(key_index));
}

#[test]
fn active_connected_owner_rejects_teardown_without_losing_transaction() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    assert_eq!(
        tx.start(&mut hardware, &ethernet()),
        Ok(WifiTxProgress::Pending)
    );

    let mut tx = match tx.try_into_parts() {
        Err(tx) => tx,
        Ok(_) => panic!("hardware-owned TX must reject decomposition"),
    };
    assert!(tx.active());
    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(tx.try_into_parts().is_ok());
}

#[test]
fn connected_action_uses_the_shared_slot_as_plaintext_voice_tx() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
    let body = [3, 2, 0, 0, 37, 0];

    assert_eq!(
        tx.start_action(&mut hardware, &body, ActionTxConfig::RX_ADDBA_RESPONSE,),
        Ok(WifiTxProgress::Pending)
    );
    let (queue, program) = hardware.legacy.expect("legacy queue image");
    assert_eq!(queue, 0);
    assert_eq!(program.interface(), MacInterface::Station);
    assert_eq!(program.scheduler_priority(), 0);
    assert_eq!(program.packet_priority(), 0);
    assert_eq!(program.signal(), 34);

    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 34);
    assert_eq!(&bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2], &[0xd0, 0]);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 10..TX_METADATA_SIZE + 16],
        &[2, 3, 4, 5, 6, 7]
    );
    assert_eq!(&bytes[TX_METADATA_SIZE + 24..TX_METADATA_SIZE + 30], &body);
}

#[test]
fn power_save_null_uses_shared_retried_tx_and_exact_pm_bit() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);

    assert_eq!(
        tx.start_power_management_null(&mut hardware, StaPowerManagement::PowerSave),
        Ok(WifiTxProgress::Pending)
    );
    let (queue, program) = hardware.legacy.expect("legacy queue image");
    assert_eq!(queue, 0);
    assert_eq!(program.scheduler_priority(), 1);
    assert_eq!(program.packet_priority(), 1);
    // PLCP length includes the four-byte hardware FCS.
    assert_eq!(program.signal(), 28);

    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(matches!(
        tx.take_last_outcome(),
        Some(SingleMpduTxOutcome::Success(_))
    ));
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_eq!(
        u16::from_le_bytes(
            bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2]
                .try_into()
                .unwrap()
        ),
        0x1148
    );
    assert_eq!(&bytes[TX_METADATA_SIZE + 4..TX_METADATA_SIZE + 10], &BSSID);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 10..TX_METADATA_SIZE + 16],
        &[2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn ack_timeout_republishes_the_same_encoded_mpdu_with_retry_bit() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
    tx.start(&mut hardware, &ethernet()).unwrap();
    hardware.completion = Some(completion(5));

    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.publications, 2);
    let active = tx
        .ordinary
        .active_snapshot()
        .expect("normal retry rate remains available")
        .expect("ACK timeout retains an active publication");
    assert_eq!(active.counters.mpdu, 1);
    assert_eq!(active.counters.short, 1);
    assert_eq!(active.counters.long, 0);
    assert_eq!(active.publications, 2);
    assert_eq!(active.current_rate, TxPhyRate::Legacy(LegacyRate::Ofdm54M));
    assert!(active.retry_bit_set);
    assert_eq!(active.retries.ack_timeouts, 1);
    hardware.completion = Some(completion(0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_ne!(
        tx.ordinary.slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1] & 0x08,
        0
    );
    let report = tx
        .take_last_outcome()
        .expect("successful retried exchange")
        .report();
    assert_eq!(report.status.result, MacTxResult::Transmitted);
    assert_eq!(report.status.attempts, 2);
    assert_eq!(
        report.status.final_rate,
        TxPhyRate::Legacy(LegacyRate::Ofdm54M)
    );
    assert_eq!(report.status.acknowledged, Some(true));
}

#[test]
fn timeout_retains_dma_until_settle_deadline_without_waiting_or_republication() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
    tx.start(&mut hardware, &ethernet()).unwrap();
    hardware.timeout = true;
    let timeout = WifiTxWake::Interrupt {
        events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_TIMEOUT,
    };

    assert_eq!(
        tx.service(&mut hardware, timeout),
        Ok(WifiTxProgress::Pending)
    );
    let deadline = tx.next_deadline_micros().unwrap();
    assert_eq!(deadline, tx.ordinary.timer.now + 16);
    assert_eq!(tx.ordinary.timer.settled, 0);
    assert_eq!(tx.ordinary.slot_state(), TxSlotState::HardwareOwned);
    assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
    assert!(tx.ordinary.buffer_mut().is_err());
    assert_eq!(hardware.abort_requests, 1);
    assert_eq!(hardware.timeout_detaches, 0);
    // An interrupt from the aborted exchange must not detach early or
    // turn the timeout into a successful completion.
    hardware.completion = Some(completion(0));
    tx.ordinary.timer.now = deadline - 1;
    for wake in [
        timeout,
        WifiTxWake::Deadline,
        WifiTxWake::Interrupt {
            events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
        },
    ] {
        assert_eq!(tx.service(&mut hardware, wake), Ok(WifiTxProgress::Pending));
        assert_eq!(tx.next_deadline_micros(), Some(deadline));
    }
    assert_eq!(hardware.abort_requests, 1);
    assert_eq!(hardware.timeout_detaches, 0);
    tx.ordinary.timer.now = deadline;
    assert_eq!(
        tx.service(&mut hardware, WifiTxWake::Deadline),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(hardware.timeout_detaches, 1);
    assert_eq!(tx.next_deadline_micros(), None);
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    assert_eq!(hardware.publications, 1);
    assert_eq!(
        tx.take_last_outcome()
            .expect("terminal timeout report")
            .report()
            .status
            .result,
        MacTxResult::HardwareTimeout
    );
}

#[test]
fn cancelling_poll_wait_keeps_abort_state_and_dma_ownership() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
    tx.start(&mut hardware, &ethernet()).unwrap();
    hardware.timeout = true;
    tx.ordinary.timer.pending_wait = true;
    {
        let mut service = core::pin::pin!(tx.ordinary.service_polling(&mut hardware, 1));
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        assert!(service.as_mut().poll(&mut context).is_pending());
    }
    assert!(tx.ordinary.active());
    assert_eq!(tx.ordinary.slot_state(), TxSlotState::HardwareOwned);
    assert_eq!(hardware.abort_requests, 1);
    assert_eq!(hardware.timeout_detaches, 0);
    let deadline = tx.next_deadline_micros().unwrap();
    tx.ordinary.timer.now = deadline;
    assert_eq!(
        tx.service(&mut hardware, WifiTxWake::Deadline),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(hardware.abort_requests, 1);
    assert_eq!(hardware.timeout_detaches, 1);
}

#[test]
fn collision_retries_without_marking_an_untransmitted_mpdu_as_retry() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
    tx.start(&mut hardware, &ethernet()).unwrap();
    hardware.collision = true;
    let collision = WifiTxWake::Interrupt {
        events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_COLLISION,
    };

    for collision_number in 1..=VENDOR_SHORT_RETRY_LIMIT {
        hardware.collision = true;
        let expected = if collision_number < VENDOR_SHORT_RETRY_LIMIT {
            WifiTxProgress::Pending
        } else {
            WifiTxProgress::Complete
        };
        assert_eq!(tx.service(&mut hardware, collision), Ok(expected));
    }
    assert_eq!(
        tx.ordinary.slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1] & 0x08,
        0
    );
    let report = tx
        .take_last_outcome()
        .expect("terminal collision report")
        .report();
    assert_eq!(report.status.result, MacTxResult::CollisionLimit);
    assert_eq!(report.status.attempts, VENDOR_SHORT_RETRY_LIMIT);
    assert_eq!(report.retries.collisions, VENDOR_SHORT_RETRY_LIMIT - 1);
}

#[test]
fn executor_deadline_quarantines_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    {
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
        tx.start(&mut hardware, &ethernet()).unwrap();

        let deadline = tx.next_deadline_micros().unwrap();
        tx.ordinary.timer.now = deadline - 1;
        assert_eq!(
            tx.service(&mut hardware, WifiTxWake::Deadline),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(hardware.abort_requests, 0);
        tx.ordinary.timer.now = deadline;

        assert_eq!(
            tx.service(&mut hardware, WifiTxWake::Deadline),
            Err(SingleMpduTxError::RadioResetRequired(
                TxResetReason::ExecutorDeadline
            ))
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::ResetRequired);
        assert_eq!(tx.queue_state(), MacTxQueueState::ResetRequired);
        assert!(tx.ordinary.slot.as_mut().reserve(64, 32).is_err());
    }
    let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(slot)));
    assert!(drop_result.is_ok());
}

#[test]
fn queue_rejection_cancels_the_unpublished_descriptor() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);

    assert_eq!(
        tx.start(&mut hardware, &ethernet()),
        Err(SingleMpduTxError::Tx(TxError::QueueActive))
    );
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    assert_eq!(tx.ordinary.slot.descriptor_word0(), 0);
}
