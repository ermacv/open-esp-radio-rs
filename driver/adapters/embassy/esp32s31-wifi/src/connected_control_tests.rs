use core::{
    future::{Future, ready},
    pin::Pin,
};

use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
    rx_ampdu::{RxBlockAckRequest, RxBlockAckSnapshot},
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::{HardwareOwnedTxDma, LegacyRate, PreparedTxDma, TxCompletion, TxHardware, TxSlot},
    tx_ampdu::{BlockAckAction, STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckSessions},
    tx_runtime::WifiTxRuntimePolicy,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    Esp32s31SingleMpduTx, SingleMpduTxConfig, SingleMpduTxOutcome, WifiTxPowerPair,
    WifiTxPowerProfile, WifiTxTimer,
};
use open_esp_radio_ieee80211::station::{StaDisconnect, StaDisconnectKind, StaTxSequenceCounters};
use open_esp_radio_ieee80211::station_beacon::{StaBeaconObservation, StaTimObservation};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;
use open_esp_radio_ieee80211::wmm::WmmAccessCategory;
use open_esp_radio_wifi_softmac::{MacRxMetadata, MacTxPlan};
use open_esp_radio_wifi_sta::power_save::StaPowerSaveState;

use crate::{
    control_mailbox::ConnectedControlResources,
    rx_reorder::{RxReorderCommand, RxReorderCommandResources, try_receive_rx_reorder_command},
    wdev::{WdevControlProgress, WifiTxProgress, WifiTxWake},
};

use super::*;

const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

#[derive(Default)]
struct Hardware {
    station_tsf: u64,
    prepare: bool,
    completion: Option<MacTxCompletionRegisters>,
    programmed: Option<S31RxBlockAckAgreement>,
    cleared: [Option<u8>; 4],
    clear_count: usize,
    he_tid: [Option<(u8, bool)>; 4],
    he_count: usize,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        self.prepare
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8, _plcp0: u32) {}

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
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
            MacTxDetachReason::Collision | MacTxDetachReason::Timeout => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }
}

impl RxBlockAckHardware for Hardware {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.programmed = Some(agreement.validate()?);
        Ok(())
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        self.cleared[self.clear_count] = Some(hardware_index);
        self.clear_count += 1;
        Ok(())
    }

    fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        tid: u8,
        starting_sequence: u16,
        window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        if tid > 7 {
            return Err(S31RxBlockAckAgreementError::Tid(tid));
        }
        if starting_sequence > 0x0fff {
            return Err(S31RxBlockAckAgreementError::StartingSequence(
                starting_sequence,
            ));
        }
        if window == 0 || window > 0x7f {
            return Err(S31RxBlockAckAgreementError::Window(window));
        }
        Ok(())
    }

    fn program_extra_softap_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.programmed = Some(agreement.validate()?);
        Ok(())
    }

    fn clear_extra_softap_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.clear_rx_block_ack(hardware_index)
    }

    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        starting_sequence: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        if starting_sequence > 0x0fff {
            return Err(S31RxBlockAckAgreementError::StartingSequence(
                starting_sequence,
            ));
        }
        Ok(())
    }
}

impl ConnectedControlHardware for Hardware {
    fn station_tsf(&mut self) -> u64 {
        self.station_tsf
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if tid >= 8 {
            return Err(S31RxBlockAckAgreementError::Tid(tid));
        }
        self.he_tid[self.he_count] = Some((tid, enabled));
        self.he_count += 1;
        Ok(())
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

fn completion(status: u8) -> MacTxCompletionRegisters {
    MacTxCompletionRegisters {
        aux_a: 0,
        aux_b: 0,
        aux_c: 0,
        primary: u32::from(status) << 12,
        alternate: 0,
        trigger_flow: false,
    }
}

fn make_tx<'a>(
    slot: Pin<&'a mut TxSlot<512>>,
    hardware: &mut Hardware,
    attempt_limit: u8,
) -> Esp32s31SingleMpduTx<'a, Power, fn() -> u32, Timer, 512> {
    fn entropy() -> u32 {
        0x1234_5678
    }

    let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
    Esp32s31SingleMpduTx::new(
        open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxHandoff {
            key,
            sequences: StaTxSequenceCounters::new(7),
            config: SingleMpduTxConfig {
                station_address: STATION,
                bssid: BSSID,
                peer_qos: true,
                exchange: MacTxPlan {
                    access_category: WmmAccessCategory::BestEffort,
                    initial_rate: open_esp_radio_esp32s31_wifi_mac::tx::TxPhyRate::Legacy(
                        LegacyRate::Ofdm54M,
                    ),
                    publication_limit: attempt_limit,
                    publication_timeout_micros: 250_000,
                },
            },
        },
    )
}

fn finish_tx(
    hardware: &mut Hardware,
    tx: &mut Esp32s31SingleMpduTx<'_, Power, fn() -> u32, Timer, 512>,
    status: u8,
) {
    hardware.completion = Some(completion(status));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
}

fn idle_beacon() -> StaBeaconObservation {
    StaBeaconObservation {
        timestamp_tsf: 1_000_000,
        // The association-owned policy below deliberately differs.
        interval_tu: 500,
        capability_information: 0,
        tim: Some(StaTimObservation {
            dtim_count: 1,
            dtim_period: 3,
            unicast_buffered: false,
            group_buffered: false,
        }),
    }
}

fn beacon_event(observation: StaBeaconObservation) -> ConnectedRxEvent<'static> {
    ConnectedRxEvent::Beacon {
        observation,
        metadata: MacRxMetadata::unavailable(),
    }
}

fn power_save_policy() -> StaPowerSavePolicy {
    StaPowerSavePolicy::new(100, 2_000).unwrap()
}

#[test]
fn initial_tx_block_ack_requests_follow_zero_seven_five_and_arm_alarms() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.queue_initial_tx_block_ack(2);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    for tid in STA_TX_BLOCK_ACK_TIDS {
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WdevControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WdevControlProgress::More)
        );
        assert!(control.tx_block_ack().alarm(tid).is_some());
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Idle)
    );

    embassy_futures::block_on(control.wait_ready(&mut tx));
    for tid in STA_TX_BLOCK_ACK_TIDS {
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WdevControlProgress::More)
        );
        assert_eq!(control.last_expired_tid(), Some(tid));
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending),
        "a missing ADDBA response consumes one bounded retry"
    );
    assert!(control.tx_block_ack().alarm(0).is_some());
    finish_tx(&mut hardware, &mut tx, 0);
}

#[test]
fn rx_addba_hardware_is_committed_only_after_response_tx_success() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    )
    .with_rx_reorder_commands(reorder_sender);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    let action = BlockAckAction::AddbaRequest {
        dialog_token: 9,
        tid: 3,
        immediate: true,
        amsdu: false,
        window: 16,
        timeout_tu: 0,
        starting_sequence: 0x123,
    };
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[0; 9],
    });

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    let agreement = hardware.programmed.unwrap();
    let snapshot = RxBlockAckSnapshot {
        hardware_index: agreement.hardware_index,
        interface: open_esp_radio_esp32s31_wifi_mac::MacInterface::Station,
        peer: BSSID,
        tid: 3,
        starting_sequence: 0x123,
        window: 16,
    };
    assert_eq!(agreement.tid, 3);
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Start(snapshot))
    );

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        control.rx_block_ack().snapshots()[usize::from(agreement.hardware_index)]
            .unwrap()
            .tid,
        3
    );
    assert_eq!(hardware.clear_count, 0);

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::Delba {
            tid: 3,
            initiator: true,
            reason: 37,
        },
        body: &[0; 6],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Stop(snapshot.identity()))
    );
    assert_eq!(hardware.cleared[0], Some(agreement.hardware_index));
}

#[test]
fn failed_rx_addba_response_rolls_back_hardware_and_software() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    )
    .with_rx_reorder_commands(reorder_sender);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    let action = BlockAckAction::AddbaRequest {
        dialog_token: 9,
        tid: 3,
        immediate: true,
        amsdu: false,
        window: 16,
        timeout_tu: 0,
        starting_sequence: 0x123,
    };
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    let agreement = hardware.programmed.unwrap();
    let snapshot = RxBlockAckSnapshot {
        hardware_index: agreement.hardware_index,
        interface: open_esp_radio_esp32s31_wifi_mac::MacInterface::Station,
        peer: BSSID,
        tid: 3,
        starting_sequence: 0x123,
        window: 16,
    };
    let hardware_index = agreement.hardware_index;
    assert!(matches!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Start(observed)) if observed == snapshot
    ));

    // Status 2 is the vendor CTS-timeout retry path. Use the terminal
    // status-1 RTS error to exercise rollback after a failed response.
    finish_tx(&mut hardware, &mut tx, 1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(hardware.cleared[0], Some(hardware_index));
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Stop(snapshot.identity()))
    );
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .iter()
            .all(Option::is_none)
    );
    assert!(matches!(
        control.last_tx_failure(),
        Some(ConnectedControlTxFailure {
            kind: ConnectedControlTxKind::RxAddbaResponse { tid: 3 },
            outcome: SingleMpduTxOutcome::HardwareFailure(report),
        })
        if matches!(report.completion, Some(TxCompletion { status: 1, .. }))
    ));
}

#[test]
fn tx_addba_response_and_delba_toggle_he_tid_ownership() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.queue_initial_tx_block_ack(1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 42,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(control.stale_tx_block_ack_responses(), 1);
    assert_eq!(control.last_stale_tx_block_ack_token(), Some(42));
    assert!(control.tx_block_ack().alarm(0).is_some());

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 1,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(hardware.he_tid[0], Some((0, true)));

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::Delba {
            tid: 0,
            initiator: false,
            reason: 37,
        },
        body: &[0; 6],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(hardware.he_tid[1], Some((0, false)));
}

#[test]
fn beacon_loss_disconnects_only_after_bounded_active_probes() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_beacon_loss(StaBeaconLossConfig::new(100, 3).unwrap());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Idle)
    );
    for _ in 0..5 {
        embassy_futures::block_on(control.wait_ready(&mut tx));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WdevControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WdevControlProgress::More)
        );
    }
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Exit(
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::BeaconLoss
        ))
    );
    assert!(control.beacon_lost());
    assert_eq!(
        hardware.he_tid[..3],
        [Some((0, false)), Some((7, false)), Some((5, false))]
    );
}

#[test]
fn associated_probe_response_cancels_beacon_loss_recovery() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_beacon_loss(StaBeaconLossConfig::new(100, 3).unwrap());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Idle)
    );
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );

    publisher.publish(ConnectedRxEvent::ProbeResponse);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert!(!control.beacon_lost());
    assert_eq!(
        control.beacon_monitor().unwrap().deadline_micros(),
        Some(614_400)
    );
}

#[test]
fn peer_deauthentication_disconnects_with_its_reason_code() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    publisher.publish(ConnectedRxEvent::PeerDisconnect(StaDisconnect {
        kind: StaDisconnectKind::Deauthentication,
        reason_code: 4,
    }));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Exit(
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::PeerDeauthentication {
                reason_code: 4,
            }
        ))
    );
}

#[test]
fn shutdown_clears_rx_tx_block_ack_and_discards_late_control_events() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaRequest {
            dialog_token: 9,
            tid: 3,
            immediate: true,
            amsdu: false,
            window: 16,
            timeout_tu: 0,
            starting_sequence: 0x123,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );

    control.queue_initial_tx_block_ack(1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 1,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    publisher.publish(beacon_event(idle_beacon()));

    assert_eq!(
        control.shutdown(&mut hardware, &mut tx),
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: 1,
            tx_block_ack_sessions: 1,
            discarded_events: 1,
            in_flight: None,
        })
    );
    assert_eq!(hardware.cleared[0], Some(0));
    assert_eq!(
        hardware.he_tid,
        [
            Some((0, true)),
            Some((0, false)),
            Some((7, false)),
            Some((5, false)),
        ]
    );
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .into_iter()
            .all(|agreement| agreement.is_none())
    );
    assert!(
        STA_TX_BLOCK_ACK_TIDS.into_iter().all(|tid| control
            .tx_block_ack()
            .operational(tid)
            .is_none()
            && control.tx_block_ack().alarm(tid).is_none())
    );
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Idle)
    );
}

#[test]
fn station_shutdown_preserves_access_point_rx_block_ack_banks() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control
        .rx_block_ack
        .offer(RxBlockAckRequest {
            interface: MacInterface::AccessPoint,
            peer: [0x30, 0x31, 0x32, 0x33, 0x34, 0x35],
            dialog_token: 1,
            tid: 5,
            immediate: true,
            requested_window: 16,
            timeout_tu: 0,
            starting_sequence: 7,
        })
        .unwrap();
    let activation = control.rx_block_ack.begin_pending().unwrap().unwrap();
    let ap_agreement = activation.negotiated();
    control.rx_block_ack.commit(activation).unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    assert_eq!(
        control.shutdown(&mut hardware, &mut tx),
        Ok(ConnectedControlShutdown::default())
    );
    assert_eq!(hardware.clear_count, 0);
    assert_eq!(
        control
            .rx_block_ack()
            .snapshots_for(MacInterface::AccessPoint)[usize::from(ap_agreement.hardware_index)],
        Some(ap_agreement)
    );
}

#[test]
fn beacon_received_on_exact_deadline_refreshes_before_loss_check() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_beacon_loss(StaBeaconLossConfig::new(100, 3).unwrap());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::Idle)
    );
    embassy_futures::block_on(control.wait_ready(&mut tx));
    publisher.publish(beacon_event(StaBeaconObservation {
        timestamp_tsf: 123,
        interval_tu: 100,
        capability_information: 0,
        tim: None,
    }));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert!(!control.beacon_lost());
    assert_eq!(
        control.beacon_monitor().unwrap().deadline_micros(),
        Some(614_400)
    );
}

#[test]
fn doze_permit_requires_idle_beacon_and_acknowledged_pm_one() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_power_save(power_save_policy());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_tsf: 1_000_100,
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    publisher.publish(beacon_event(idle_beacon()));

    assert_eq!(
        embassy_futures::block_on(control.service_with_context(
            &mut hardware,
            &mut tx,
            WdevControlContext::IDLE,
        )),
        Ok(WdevControlProgress::TxPending)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::AdvertisingPowerSave
    );
    assert_eq!(control.take_doze_permit(), None);

    finish_tx(&mut hardware, &mut tx, 0);
    hardware.station_tsf = 1_001_000;
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(
            &mut hardware,
            &mut tx,
            WdevControlContext::IDLE,
        )),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::PowerSave
    );
    assert_eq!(
        control.take_doze_permit(),
        Some(StaDozePermit {
            beacon_timestamp_tsf: 1_000_000,
            wake_tsf: 1_100_400,
            dtim_count: 1,
            dtim_period: 3,
        })
    );
}

#[test]
fn queued_network_traffic_blocks_pm_one() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_power_save(power_save_policy());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_tsf: 1_000_100,
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    publisher.publish(beacon_event(idle_beacon()));

    assert_eq!(
        embassy_futures::block_on(control.service_with_context(
            &mut hardware,
            &mut tx,
            WdevControlContext {
                network_tx_pending: true,
            },
        )),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::Awake
    );
    assert_eq!(control.take_doze_permit(), None);
}

#[test]
fn failed_pm_one_returns_to_awake_without_a_permit() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_power_save(power_save_policy());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_tsf: 1_000_100,
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    publisher.publish(beacon_event(idle_beacon()));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );

    finish_tx(&mut hardware, &mut tx, 5);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::Awake
    );
    assert_eq!(control.take_doze_permit(), None);
    assert!(matches!(
        control.last_tx_failure(),
        Some(ConnectedControlTxFailure {
            kind: ConnectedControlTxKind::PowerManagement(StaPowerManagement::PowerSave),
            ..
        })
    ));
}

#[test]
fn queued_network_traffic_restores_pm_zero_before_data() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_power_save(power_save_policy());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_tsf: 1_000_100,
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    publisher.publish(beacon_event(idle_beacon()));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    hardware.station_tsf = 1_001_000;
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );

    let pending = WdevControlContext {
        network_tx_pending: true,
    };
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(WdevControlProgress::TxPending)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::AdvertisingActive
    );
    assert_eq!(control.take_doze_permit(), None);

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(WdevControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::Awake
    );
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(WdevControlProgress::Idle)
    );
}

#[test]
fn failed_pm_zero_disconnects_instead_of_releasing_queued_data() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_power_save(power_save_policy());
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_tsf: 1_000_100,
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    publisher.publish(beacon_event(idle_beacon()));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    hardware.station_tsf = 1_001_000;
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(WdevControlProgress::More)
    );

    let pending = WdevControlContext {
        network_tx_pending: true,
    };
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(WdevControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 5);
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(WdevControlProgress::Exit(
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::ActiveStateRestoreFailed
        ))
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::PowerSave
    );
    assert!(matches!(
        control.last_tx_failure(),
        Some(ConnectedControlTxFailure {
            kind: ConnectedControlTxKind::PowerManagement(StaPowerManagement::Active),
            ..
        })
    ));
}
