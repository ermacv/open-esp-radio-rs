use core::{
    future::{Future, ready},
    pin::Pin,
};

use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacStaReceivePolicySnapshot,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    crypto::{
        CcmpKeyHardware, CryptoKeyError, StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError,
        StaGroupCcmpSlot, install_sta_group_ccmp, install_sta_pairwise_ccmp,
        replace_sta_group_ccmp_with_rollback,
    },
    rx_ampdu::{RxBlockAckRequest, RxBlockAckSnapshot},
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::{HardwareOwnedTxDma, LegacyRate, PreparedTxDma, TxHardware, TxSlot},
    tx_ampdu::{BlockAckAction, STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckSessions},
    tx_runtime::WifiTxRuntimePolicy,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxEvent, ConnectedRxSink, StaCcmpRxReplayEpoch, StaCcmpRxReplayResource,
    StaCcmpRxReplayRxEndpoint,
};
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    Esp32s31SingleMpduTx, SingleMpduTxConfig, SingleMpduTxOutcome, WifiTxPowerPair,
    WifiTxPowerProfile, WifiTxTimer,
};
use open_esp_radio_ieee80211::station::{StaDisconnect, StaDisconnectKind, StaTxSequenceCounters};
use open_esp_radio_ieee80211::station_beacon::{StaBeaconObservation, StaTimObservation};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;
use open_esp_radio_ieee80211::twt::{
    IndividualTwtAction, IndividualTwtControl, IndividualTwtFlowId, IndividualTwtFlowType,
    IndividualTwtParameterSet, IndividualTwtSetup, IndividualTwtSetupCommand,
};
use open_esp_radio_ieee80211::wmm::WmmAccessCategory;
use open_esp_radio_wifi_softmac::{MacRxMetadata, MacTxPlan};
use open_esp_radio_wifi_sta::power_save::StaPowerSaveState;
use open_esp_radio_wpa2::{
    OwnedEapolFrame, Pmk, Ptk, PtkContext, Wpa2Interface,
    aes::{Wpa2SoftwareAes, software_aes128_key_wrap},
    frames::{OwnedRsnIe, Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame},
    supplicant::{Wpa2ConnectedSupplicant, Wpa2StaSupplicant, Wpa2StaSupplicantAction},
};

use crate::{
    datapath::rx::reorder::{
        RxReorderCommand, RxReorderCommandResources, try_receive_rx_reorder_command,
    },
    datapath::{DatapathControlProgress, WifiTxProgress, WifiTxWake},
    roles::station::control_mailbox::ConnectedControlResources,
};

use super::*;

const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const SNONCE: [u8; 32] = [0x33; 32];
const ANONCE: [u8; 32] = [0x44; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const INITIAL_GTK: [u8; 16] = [0x11; 16];

fn ptk_context() -> PtkContext {
    PtkContext {
        authenticator_address: BSSID,
        supplicant_address: STATION,
        authenticator_nonce: ANONCE,
        supplicant_nonce: SNONCE,
    }
}

fn owned_eapol(frame: &Wpa2TxFrame<512>) -> OwnedEapolFrame<512> {
    OwnedEapolFrame::try_copy(Wpa2Interface::Station, BSSID, frame.as_bytes()).unwrap()
}

fn established_supplicant() -> (Wpa2ConnectedSupplicant, Ptk) {
    let pmk = Pmk::from_bytes([0x22; 32]);
    let peer_ptk = pmk.derive_ptk(ptk_context());
    let mut supplicant =
        Wpa2StaSupplicant::try_new(STATION, BSSID, SNONCE, &RSN, &RSN, &[]).unwrap();
    let mut aes = Wpa2SoftwareAes::new();

    let message1 = Wpa2TxFrame::<512>::message1(STATION, 1, ANONCE).unwrap();
    assert!(matches!(
        embassy_futures::block_on(supplicant.on_frame(owned_eapol(&message1), &pmk, &mut aes,)),
        Ok(Wpa2StaSupplicantAction::Transmit(_))
    ));

    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = Wpa2Gtk::new(1, false, INITIAL_GTK).unwrap();
    let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let wrapped = software_aes128_key_wrap(peer_ptk.kek(), plain.as_bytes()).unwrap();
    let message3 = Wpa2TxFrame::<512>::message3(
        STATION,
        2,
        ANONCE,
        [5, 0, 0, 0, 0, 0, 0, 0],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&peer_ptk);
    let Wpa2StaSupplicantAction::InstallKeys(request) =
        embassy_futures::block_on(supplicant.on_frame(owned_eapol(&message3), &pmk, &mut aes))
            .unwrap()
    else {
        panic!("authenticated Message 3 must establish connected WPA2 state")
    };
    assert!(matches!(
        supplicant.complete_key_install::<512>(request, true),
        Ok(Wpa2StaSupplicantAction::Transmit(_))
    ));
    (supplicant.into_connected().unwrap(), peer_ptk)
}

fn group_message1(
    ptk: &Ptk,
    replay_counter: u64,
    key_id: u8,
    key: [u8; 16],
    receive_sequence: [u8; 8],
) -> ConnectedSecurityFrame {
    let mut kde = [0; 24];
    kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
    kde[8..].copy_from_slice(&key);
    let wrapped = software_aes128_key_wrap(ptk.kek(), &kde).unwrap();
    let frame = Wpa2TxFrame::<512>::group_message1(
        STATION,
        replay_counter,
        receive_sequence,
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(ptk);
    ConnectedSecurityFrame::Protected(owned_eapol(&frame))
}

#[derive(Default)]
struct Hardware {
    station_tsf: u64,
    prepare: bool,
    completion: Option<MacTxCompletionObservation>,
    programmed: Option<S31RxBlockAckAgreement>,
    cleared: [Option<u8>; 4],
    clear_count: usize,
    he_tid: [Option<(u8, bool)>; 4],
    he_count: usize,
    key_install_count: usize,
    ccmp_reject_installs: u8,
    ccmp_clears: usize,
    twt_admit: bool,
    twt_install_count: usize,
    station_policy: Option<MacStaReceivePolicySnapshot>,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(&mut self, _index: u8, _words: &[u32; 6]) -> MacKeyInstallOutcome {
        if self.ccmp_reject_installs != 0 {
            self.ccmp_reject_installs -= 1;
            return MacKeyInstallOutcome::Rejected;
        }
        self.key_install_count += 1;
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {
        self.ccmp_clears += 1;
    }
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

    fn station_beacon_monitor_readback(&mut self) -> Option<MacStaReceivePolicySnapshot> {
        self.station_policy
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

    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        if self.twt_admit {
            Ok(())
        } else {
            Err(StationIndividualTwtHardwareError::Unsupported {
                reached: StationIndividualTwtHardwareStage::None,
                missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
            })
        }
    }

    fn install_station_individual_twt(
        &mut self,
        _agreement: &open_esp_radio_wifi_sta::twt::IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        self.twt_install_count += 1;
        Ok(())
    }

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        current: &StaGroupCcmpKeyMaterial,
        replacement: &StaGroupCcmpKeyMaterial,
    ) -> Result<(), StaGroupCcmpReplaceError> {
        replace_sta_group_ccmp_with_rollback(self, slot, current, replacement)
    }
}

fn make_connected_security(
    hardware: &mut Hardware,
) -> (ConnectedWpa2Security, StaCcmpRxReplayRxEndpoint, Ptk) {
    let (supplicant, ptk) = established_supplicant();
    let group = install_sta_group_ccmp(hardware, 1, &INITIAL_GTK).unwrap();
    let group_material = StaGroupCcmpKeyMaterial::new(1, INITIAL_GTK).unwrap();
    let replay =
        StaCcmpRxReplayEpoch::new([7, 0, 0, 0, 0, 0, 0, 0], 1, [5, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (rx, control) = resource.start(replay).unwrap();
    (
        ConnectedWpa2Security::new(supplicant, group, group_material, control),
        rx,
        ptk,
    )
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

fn completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
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
            security:
                open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxSecurity::Wpa2Personal(
                    key,
                ),
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

fn individual_twt_parameters(implicit: bool) -> IndividualTwtParameterSet {
    IndividualTwtParameterSet {
        requesting_sta: true,
        setup_command: IndividualTwtSetupCommand::Request,
        trigger: false,
        implicit,
        flow_type: IndividualTwtFlowType::Announced,
        flow_id: IndividualTwtFlowId::new(2).unwrap(),
        wake_interval_exponent: 0,
        protection: false,
        target_wake_time_tsf: 10_000,
        nominal_minimum_wake_duration: 1,
        wake_interval_mantissa: 1_024,
        twt_channel: 0,
    }
}

#[test]
fn connected_low_power_diagnostic_stays_before_physical_sleep() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (_publisher, receiver) = resources.split();
    let control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    let frontier = control.low_power_hardware_frontier();

    assert_eq!(
        frontier.station_tbtt_tsf_wake_prefix,
        StationLowPowerFrontierStatus::ReachableRollbackProbe
    );
    assert_eq!(
        frontier.station_wdevpwr_cause_binding,
        StationLowPowerFrontierStatus::MissingReviewedSemantics
    );
    assert_eq!(
        frontier.rf_phy_baseband_clock_sleep_wake,
        StationLowPowerFrontierStatus::MissingReviewedSemantics
    );
}

#[test]
fn connected_runtime_binds_beacon_frontier_but_retains_software_monitor() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (_publisher, receiver) = resources.split();
    let policy = StaBeaconLossConfig::new(100, 10).unwrap();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_beacon_loss(policy);
    control
        .enable_hardware_beacon_monitor_frontier(
            StationBeaconMonitorBinding::new(
                BSSID,
                open_esp_radio_ieee80211::station_power_save::StaAssociationId::new(7).unwrap(),
            ),
            policy,
        )
        .unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_policy: Some(MacStaReceivePolicySnapshot {
            queue_zero_policy: 0,
            queue_three_policy: 0,
            bssid: BSSID,
            association_id: 7,
            minimum_mpdu_start_spacing: 0,
            bssid_address_check_enabled: true,
            interface_is_soft_ap: false,
            interface_rx_policy_enabled: true,
            beacon_filter_control: 0,
        }),
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
    let frontier = control.hardware_beacon_monitor_frontier().unwrap();
    assert_eq!(
        frontier.reached(),
        open_esp_radio_esp32s31_wifi_sta::hardware_beacon_monitor::StationHardwareBeaconMonitorStage::BeaconMissLimitRepresentable
    );
    assert_eq!(
        frontier.blocker(),
        open_esp_radio_esp32s31_wifi_sta::hardware_beacon_monitor::StationHardwareBeaconMonitorBlocker::MissingBeaconMissTimeoutUnitConversion
    );
    assert!(!frontier.automatic_monitor_active());
    assert!(control.beacon_monitor().is_some());
}

#[test]
fn peer_accepted_explicit_twt_kicks_teardown_into_connected_tx() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = Esp32s31ConnectedControl::new(
        receiver,
        BSSID,
        false,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    );
    control.enable_individual_twt_requester(
        IndividualTwtRequesterConfig::new(1_000, 100, 1, 1).unwrap(),
    );
    control
        .queue_individual_twt_setup(
            IndividualTwtProposal {
                control: IndividualTwtControl::REQUEST,
                parameters: individual_twt_parameters(true),
            },
            0,
        )
        .unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        twt_admit: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    let mut accepted = individual_twt_parameters(false);
    accepted.requesting_sta = false;
    accepted.setup_command = IndividualTwtSetupCommand::Accept;
    publisher.publish(ConnectedRxEvent::IndividualTwt {
        action: IndividualTwtAction::Setup(IndividualTwtSetup {
            dialog_token: 1,
            control: IndividualTwtControl::REQUEST,
            parameters: accepted,
        }),
        body: &[],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(hardware.twt_install_count, 0);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending),
        "the explicit-information frontier must immediately reach teardown TX"
    );
    assert_eq!(
        control
            .individual_twt_requester()
            .unwrap()
            .status(IndividualTwtFlowId::new(2).unwrap()),
        open_esp_radio_wifi_sta::twt::IndividualTwtFlowStatus::TeardownTransmitting
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        control.individual_twt_runtime_evidence().actions_published,
        2
    );
}

const WPA2_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const WPA2_SNONCE: [u8; 32] = [3; 32];
const WPA2_ANONCE: [u8; 32] = [4; 32];

fn owned_station_eapol(frame: &Wpa2TxFrame<512>) -> OwnedEapolFrame {
    OwnedEapolFrame::try_copy(Wpa2Interface::Station, BSSID, frame.as_bytes()).unwrap()
}

struct CompletedWpa2Fixture {
    security: ConnectedWpa2Security,
    replay_rx: StaCcmpRxReplayRxEndpoint,
    duplicate_message3: OwnedEapolFrame,
    bad_mic_message3: OwnedEapolFrame,
    wrong_replay_message3: OwnedEapolFrame,
}

fn completed_wpa2_fixture(hardware: &mut Hardware) -> CompletedWpa2Fixture {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk_context = PtkContext {
        authenticator_address: BSSID,
        supplicant_address: STATION,
        authenticator_nonce: WPA2_ANONCE,
        supplicant_nonce: WPA2_SNONCE,
    };
    let ptk = pmk.derive_ptk(ptk_context);
    let mut supplicant =
        Wpa2StaSupplicant::try_new(STATION, BSSID, WPA2_SNONCE, &WPA2_RSN, &WPA2_RSN, &[]).unwrap();
    let mut aes = Wpa2SoftwareAes::new();
    let message1 = Wpa2TxFrame::<512>::message1(STATION, 1, WPA2_ANONCE).unwrap();
    let Wpa2StaSupplicantAction::Transmit(_) = embassy_futures::block_on(supplicant.on_frame(
        owned_station_eapol(&message1),
        &pmk,
        &mut aes,
    ))
    .unwrap() else {
        panic!("Message 1 must produce Message 2")
    };
    let rsn = OwnedRsnIe::<64>::try_copy(&WPA2_RSN).unwrap();
    let gtk = Wpa2Gtk::new(1, false, [0x6a; 16]).unwrap();
    let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes()).unwrap();
    let message3 =
        Wpa2TxFrame::<512>::message3(STATION, 2, WPA2_ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(&ptk);
    let Wpa2StaSupplicantAction::InstallKeys(request) = embassy_futures::block_on(
        supplicant.on_frame(owned_station_eapol(&message3), &pmk, &mut aes),
    )
    .unwrap() else {
        panic!("Message 3 must produce the initial key transaction")
    };
    let Wpa2StaSupplicantAction::Transmit(_) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("successful key publication must produce Message 4")
    };

    let wrong_replay =
        Wpa2TxFrame::<512>::message3(STATION, 3, WPA2_ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(&ptk);
    let mut forged = [0; 512];
    let length = message3.as_bytes().len();
    forged[..length].copy_from_slice(message3.as_bytes());
    forged[81] ^= 1;
    let bad_mic_message3 =
        OwnedEapolFrame::try_copy(Wpa2Interface::Station, BSSID, &forged[..length]).unwrap();
    let connected = supplicant.into_connected().unwrap();
    let group = install_sta_group_ccmp(hardware, 1, &[0x6a; 16]).unwrap();
    let group_material = StaGroupCcmpKeyMaterial::new(1, [0x6a; 16]).unwrap();
    let replay = StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap();
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (replay_rx, replay_control) = resource.start(replay).unwrap();
    CompletedWpa2Fixture {
        security: ConnectedWpa2Security::new(connected, group, group_material, replay_control),
        replay_rx,
        duplicate_message3: owned_station_eapol(&message3),
        bad_mic_message3,
        wrong_replay_message3: owned_station_eapol(&wrong_replay),
    }
}

#[test]
fn duplicate_message3_reuses_connected_key_and_pn_while_forged_frames_are_ignored() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let CompletedWpa2Fixture {
        mut security,
        mut replay_rx,
        duplicate_message3,
        bad_mic_message3,
        wrong_replay_message3,
    } = completed_wpa2_fixture(&mut hardware);
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    assert_eq!(hardware.key_install_count, 2);

    let before = tx
        .take_protected_metadata(0)
        .unwrap()
        .expect("WPA2 TX owns pairwise metadata");
    assert_eq!(before.ccmp_header, [3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(
        security.process_duplicate_message3(&mut hardware, &mut tx, duplicate_message3),
        DatapathControlProgress::TxPending
    );
    assert_eq!(hardware.key_install_count, 2);
    assert_eq!(security.evidence().duplicate_message3, 1);
    assert_eq!(security.evidence().retransmitted, 1);
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);

    assert_eq!(
        security.process_duplicate_message3(&mut hardware, &mut tx, bad_mic_message3),
        DatapathControlProgress::More
    );
    assert_eq!(
        security.process_duplicate_message3(&mut hardware, &mut tx, wrong_replay_message3),
        DatapathControlProgress::More
    );
    let evidence = security.evidence();
    assert_eq!(evidence.duplicate_message3, 3);
    assert_eq!(evidence.ignored_duplicate_message3, 2);
    assert_eq!(evidence.retransmitted, 1);
    assert!(!evidence.tx_in_flight);
    assert_eq!(evidence.last_failure, None);
    assert_eq!(hardware.key_install_count, 2);

    let (_resources, handoff) = match tx.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("completed TX must be idle"),
    };
    let open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxSecurity::Wpa2Personal(
        mut pairwise,
    ) = handoff.security
    else {
        panic!("connected TX must retain its installed pairwise key")
    };
    assert_eq!(
        pairwise.next_tx_ccmp_header().unwrap(),
        [9, 0, 0, 0x20, 0, 0, 0, 0]
    );
    let (_supplicant, group) = security.into_parts();
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
    pairwise.clear(&mut hardware);
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
            Ok(DatapathControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
        assert!(control.tx_block_ack().alarm(tid).is_some());
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );

    embassy_futures::block_on(control.wait_ready(&mut tx));
    for tid in STA_TX_BLOCK_ACK_TIDS {
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
        assert_eq!(control.last_expired_tid(), Some(tid));
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending),
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
        Ok(DatapathControlProgress::TxPending)
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
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::TxPending)
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
        Ok(DatapathControlProgress::More)
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
        if report
            .completion
            .is_some_and(|completion| completion.status() == 1)
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
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::Idle)
    );
    for _ in 0..5 {
        embassy_futures::block_on(control.wait_ready(&mut tx));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
    }
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Exit(
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
        Ok(DatapathControlProgress::Idle)
    );
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    publisher.publish(ConnectedRxEvent::ProbeResponse);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::Exit(
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::PeerDeauthentication {
                reason_code: 4,
            }
        ))
    );
}

#[test]
fn mailbox_overflow_fails_closed_before_processing_an_incomplete_event_stream() {
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
    let action = BlockAckAction::Delba {
        tid: 1,
        initiator: true,
        reason: 37,
    };

    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 2, 0],
    });
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 2, 0],
    });

    assert!(control.has_immediate_work());
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Exit(
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::ControlMailboxOverflow,
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
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    control.queue_initial_tx_block_ack(1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::More)
    );
    publisher.publish(beacon_event(idle_beacon()));

    assert_eq!(
        control.shutdown(&mut hardware, &mut tx),
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: 1,
            tx_block_ack_sessions: 1,
            discarded_events: 1,
            in_flight: None,
            hardware_beacon_monitor: None,
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
        Ok(DatapathControlProgress::Idle)
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
        .rx_block_ack()
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
    let activation = control.rx_block_ack().begin_pending().unwrap().unwrap();
    let ap_agreement = activation.negotiated();
    control.rx_block_ack().commit(activation).unwrap();

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
        Ok(DatapathControlProgress::Idle)
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
        Ok(DatapathControlProgress::More)
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
            DatapathControlContext::IDLE,
        )),
        Ok(DatapathControlProgress::TxPending)
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
            DatapathControlContext::IDLE,
        )),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::PowerSave
    );
    assert_eq!(
        control.take_doze_permit(),
        Some(StaDozePermit {
            beacon_timestamp_tsf: 1_000_000,
            next_listen_tsf: 1_102_400,
            next_dtim_tsf: 1_102_400,
            wake_tsf: 1_100_400,
            wake_after_beacons: 1,
            wake_reason:
                open_esp_radio_wifi_sta::power_save::StaDozeWakeReason::ListenIntervalAndDtim,
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
            DatapathControlContext {
                network_tx_pending: true,
                stop_pending: false,
            },
        )),
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::TxPending)
    );

    finish_tx(&mut hardware, &mut tx, 5);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
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
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    hardware.station_tsf = 1_001_000;
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    let pending = DatapathControlContext {
        network_tx_pending: true,
        stop_pending: false,
    };
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(DatapathControlProgress::TxPending)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::AdvertisingActive
    );
    assert_eq!(control.take_doze_permit(), None);

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        control.power_save().unwrap().state(),
        StaPowerSaveState::Awake
    );
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(DatapathControlProgress::Idle)
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
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    hardware.station_tsf = 1_001_000;
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    let pending = DatapathControlContext {
        network_tx_pending: true,
        stop_pending: false,
    };
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 5);
    assert_eq!(
        embassy_futures::block_on(control.service_with_context(&mut hardware, &mut tx, pending,)),
        Ok(DatapathControlProgress::Exit(
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

#[test]
fn group_rekey_replaces_different_key_id_before_message2() {
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
    let installs_before = hardware.key_install_count;

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::TxPending
    );
    assert_eq!(hardware.ccmp_clears, 1);
    assert_eq!(hardware.key_install_count, installs_before + 1);
    assert_eq!(
        security.evidence(),
        ConnectedWpa2SecurityEvidence {
            replay_counter: 3,
            group_message1: 1,
            duplicate_message3: 0,
            ignored_duplicate_message3: 0,
            last_ignored_duplicate_message3: None,
            installed: 1,
            retransmitted: 0,
            tx_in_flight: true,
            last_failure: None,
        }
    );

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);
    let (_, group) = security.into_parts();
    assert_eq!(group.key_id(), 2);
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
}

#[test]
fn group_rekey_rejects_a_retired_key_id_before_hardware_mutation() {
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::TxPending
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);
    let clears_after_key_two = hardware.ccmp_clears;
    let installs_after_key_two = hardware.key_install_count;

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 4, 1, [0x33; 16], [10, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
    );
    assert_eq!(hardware.ccmp_clears, clears_after_key_two);
    assert_eq!(hardware.key_install_count, installs_after_key_two);
    assert_eq!(security.evidence().installed, 1);
    assert_eq!(security.evidence().replay_counter, 3);
    assert_eq!(
        security.evidence().last_failure,
        Some(ConnectedWpa2SecurityFailure::RetiredKeyIdGenerationUnavailable { key_id: 1 })
    );

    let (_, group) = security.into_parts();
    assert_eq!(group.key_id(), 2);
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
}

#[test]
fn same_key_id_rekey_is_rsc_only_for_identical_gtk_and_rejects_changed_gtk() {
    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        let installs_before = hardware.key_install_count;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 1, INITIAL_GTK, [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::TxPending
        );
        assert_eq!(hardware.ccmp_clears, 0);
        assert_eq!(hardware.key_install_count, installs_before);
        assert_eq!(security.evidence().installed, 1);
        assert_eq!(security.evidence().replay_counter, 3);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }

    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        let installs_before = hardware.key_install_count;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 1, [0x33; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 0);
        assert_eq!(hardware.key_install_count, installs_before);
        assert_eq!(security.evidence().installed, 0);
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::SameKeyIdGenerationUnavailable)
        );

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }
}

#[test]
fn group_rekey_hardware_failure_restores_old_epoch_or_quarantines() {
    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        hardware.ccmp_reject_installs = 1;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 1);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::KeyReplace(
                StaGroupCcmpReplaceError::ReplacementRolledBack(CryptoKeyError::HardwareRejected)
            ))
        );
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(security.evidence().installed, 0);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }

    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        hardware.ccmp_reject_installs = 2;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 1);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::KeyReplace(
                StaGroupCcmpReplaceError::RollbackFailed {
                    replacement: CryptoKeyError::HardwareRejected,
                    rollback: CryptoKeyError::HardwareRejected,
                }
            ))
        );
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(security.evidence().installed, 0);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }
}
