#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc control outcomes carry the exact reusable or faulted owner"
)]
#![expect(
    clippy::result_large_err,
    reason = "control failure returns the exact affine owner for teardown"
)]

//! Embassy delivery adapter for ESP32-S31 connected-station control.
//!
//! Protocol state and finite transitions live in the chip STA crate.  This
//! module owns only the bounded event receiver, deadline wait and reorder
//! command sender required to schedule that core on Embassy.

use core::future::Future;

use embassy_futures::select::{Either, select};
use embassy_time::{Instant, Timer};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::crypto::{
    CryptoKeyError, StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError, StaGroupCcmpSlot,
};
pub use open_esp_radio_esp32s31_wifi_mac::rx_ampdu::{RxReorderCommand, RxReorderCommandError};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxControlEvent, StaCcmpRxReplayControlEndpoint, StaCcmpRxReplayError,
};
pub use open_esp_radio_esp32s31_wifi_sta::{
    connected_control::{
        ConnectedControlError, ConnectedControlPorts, ConnectedControlReorder, ConnectedControlTx,
        ConnectedControlTxFailure, ConnectedControlTxKind, ConnectedDisconnectReason,
        ConnectedHeControlRuntimeEvidence, ConnectedHeControlRuntimeOutcome,
        ConnectedHeControlRuntimeRejection, ConnectedIndividualTwtRuntimeEvidence,
        ConnectedIndividualTwtRuntimeOutcome, Esp32s31ConnectedControlCore, HeNdpaRuntimeRequest,
        HeTriggerRuntimeRequest,
    },
    connected_control_hardware::{
        ConnectedControlHardware, StationDozeHardwareError, StationIndividualTwtHardwareError,
        StationIndividualTwtHardwareStage, StationIndividualTwtUnsupportedStage,
        StationLowPowerFrontierStatus, StationLowPowerHardwareFrontier,
        station_low_power_hardware_frontier,
    },
    hardware_beacon_monitor::{
        StationBeaconMonitorBinding, StationHardwareBeaconMonitorEpoch,
        StationHardwareBeaconMonitorFrontier, StationHardwareBeaconMonitorStopped,
    },
};
use open_esp_radio_ieee80211::twt::IndividualTwtFlowId;
use open_esp_radio_wifi_sta::{
    link_monitor::{StaBeaconLossConfig, StaBeaconMonitor},
    power_save::{
        StaDozePermit, StaDozePrepareError, StaDozeRestore, StaDozeRestoreFailure, StaDozeRestored,
        StaPowerSavePlanner, StaPowerSavePolicy, StaPowerSaveState, StaPreparedDoze,
    },
    twt::{
        IndividualTwtProposal, IndividualTwtRequester, IndividualTwtRequesterConfig,
        IndividualTwtWakePlan,
    },
};
use open_esp_radio_wpa2::{
    aes::{SoftwareAesKeyUnwrapError, Wpa2SoftwareAes},
    keys::Wpa2KeyKind,
    supplicant::{
        Wpa2ConnectedAction, Wpa2ConnectedProcessError, Wpa2ConnectedSupplicant,
        Wpa2ConnectedSupplicantError,
    },
};

use crate::{
    datapath::rx::reorder::{RxReorderCommandSender, try_send_rx_reorder_command},
    datapath::services::DatapathControlService,
    datapath::{DatapathControlContext, DatapathControlProgress},
    roles::concurrent::Esp32s31StaApRxBlockAck,
    roles::station::control_mailbox::{ConnectedControlReceiver, ConnectedSecurityFrame},
};

/// Executor deadline capability kept outside the finite control core.
pub trait ConnectedControlTimer {
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTimer
    for open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::Esp32s31SingleMpduTx<
        '_,
        P,
        E,
        T,
        BUFFER_SIZE,
    >
where
    P: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerProfile,
    E: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxEntropy,
    T: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxTimer,
{
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::Esp32s31SingleMpduTx::wait_until_micros(
            self,
            deadline_micros,
        )
    }
}

/// Finite ownership released when one connected control epoch stops.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub discarded_events: u8,
    pub in_flight: Option<ConnectedControlTxKind>,
    /// Consumed association admission owner. Its current form proves that the
    /// automatic monitor stopped before any hardware mutation.
    pub hardware_beacon_monitor: Option<StationHardwareBeaconMonitorStopped>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedWpa2SecurityFailure {
    Protocol(Wpa2ConnectedSupplicantError),
    KeyUnwrap(SoftwareAesKeyUnwrapError),
    InvalidGroupKeyKind,
    InvalidGroupKeyMaterial(CryptoKeyError),
    ReplayRotation(StaCcmpRxReplayError),
    SameKeyIdGenerationUnavailable,
    RetiredKeyIdGenerationUnavailable { key_id: u8 },
    KeyReplace(StaGroupCcmpReplaceError),
    KeyInstall(CryptoKeyError),
    TxStart(open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::SingleMpduTxError),
    TxOutcome(open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::SingleMpduTxOutcome),
    MissingTxOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedWpa2SecurityEvidence {
    pub replay_counter: u64,
    pub group_message1: u32,
    pub duplicate_message3: u32,
    pub ignored_duplicate_message3: u32,
    pub last_ignored_duplicate_message3: Option<Wpa2ConnectedSupplicantError>,
    pub installed: u32,
    pub retransmitted: u32,
    pub tx_in_flight: bool,
    pub last_failure: Option<ConnectedWpa2SecurityFailure>,
}

/// Association-scoped WPA2 state and the unique installed GTK authority.
///
/// This owner lives inside connected control while RX may publish Group
/// Message 1. It is explicitly recovered after IRQ/task quiescence, before
/// the ordinary station teardown clears the hardware key.
pub struct ConnectedWpa2Security {
    supplicant: Wpa2ConnectedSupplicant,
    group: StaGroupCcmpSlot,
    group_material: StaGroupCcmpKeyMaterial,
    used_group_key_ids: u8,
    replay: StaCcmpRxReplayControlEndpoint,
    unwrap: Wpa2SoftwareAes,
    tx_in_flight: bool,
    group_message1: u32,
    duplicate_message3: u32,
    ignored_duplicate_message3: u32,
    last_ignored_duplicate_message3: Option<Wpa2ConnectedSupplicantError>,
    installed: u32,
    retransmitted: u32,
    last_failure: Option<ConnectedWpa2SecurityFailure>,
}

impl ConnectedWpa2Security {
    pub const fn new(
        supplicant: Wpa2ConnectedSupplicant,
        group: StaGroupCcmpSlot,
        group_material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayControlEndpoint,
    ) -> Self {
        let used_group_key_ids = 1_u8 << group_material.key_id();
        Self {
            supplicant,
            group,
            group_material,
            used_group_key_ids,
            replay,
            unwrap: Wpa2SoftwareAes::new(),
            tx_in_flight: false,
            group_message1: 0,
            duplicate_message3: 0,
            ignored_duplicate_message3: 0,
            last_ignored_duplicate_message3: None,
            installed: 0,
            retransmitted: 0,
            last_failure: None,
        }
    }

    const fn tx_in_flight(&self) -> bool {
        self.tx_in_flight
    }

    pub const fn evidence(&self) -> ConnectedWpa2SecurityEvidence {
        ConnectedWpa2SecurityEvidence {
            replay_counter: self.supplicant.replay_counter(),
            group_message1: self.group_message1,
            duplicate_message3: self.duplicate_message3,
            ignored_duplicate_message3: self.ignored_duplicate_message3,
            last_ignored_duplicate_message3: self.last_ignored_duplicate_message3,
            installed: self.installed,
            retransmitted: self.retransmitted,
            tx_in_flight: self.tx_in_flight,
            last_failure: self.last_failure,
        }
    }

    pub fn into_parts(mut self) -> (Wpa2ConnectedSupplicant, StaGroupCcmpSlot) {
        // Connected lifecycle calls this only after the RX task has returned.
        // A stale stop still leaves the resource quarantined by its endpoint
        // Drop implementation and cannot reopen group publication.
        let _ = self.replay.stop();
        (self.supplicant, self.group)
    }

    fn fail(
        &mut self,
        failure: ConnectedWpa2SecurityFailure,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.last_failure = Some(failure);
        DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
    }

    fn complete_tx<X: ConnectedControlTx>(
        &mut self,
        tx: &mut X,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        let Some(outcome) = tx.take_last_outcome() else {
            return self.fail(ConnectedWpa2SecurityFailure::MissingTxOutcome);
        };
        self.tx_in_flight = false;
        if outcome.is_success() {
            DatapathControlProgress::More
        } else {
            self.fail(ConnectedWpa2SecurityFailure::TxOutcome(outcome))
        }
    }

    async fn process<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: ConnectedSecurityFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        match frame {
            ConnectedSecurityFrame::Protected(frame) => {
                self.process_group_message1(hardware, tx, frame).await
            }
            ConnectedSecurityFrame::Unprotected(frame) => {
                self.process_duplicate_message3(hardware, tx, frame)
            }
        }
    }

    fn process_duplicate_message3<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: open_esp_radio_wpa2::OwnedEapolFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.duplicate_message3 = self.duplicate_message3.saturating_add(1);
        let response = match self.supplicant.on_duplicate_message3(frame) {
            Ok(response) => response,
            Err(error) => {
                // A connected peer or nearby attacker may inject malformed or
                // stale plaintext EAPOL. It has no authority to tear down the
                // installed association; only an exact authenticated duplicate
                // M3 is actionable.
                self.ignored_duplicate_message3 = self.ignored_duplicate_message3.saturating_add(1);
                self.last_ignored_duplicate_message3 = Some(error);
                return DatapathControlProgress::More;
            }
        };
        self.retransmitted = self.retransmitted.saturating_add(1);
        match tx.start_protected_eapol(hardware, response.as_bytes()) {
            Ok(progress) => {
                self.tx_in_flight = true;
                progress
            }
            Err(error) => self.fail(ConnectedWpa2SecurityFailure::TxStart(error)),
        }
    }

    async fn process_group_message1<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: open_esp_radio_wpa2::OwnedEapolFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.group_message1 = self.group_message1.saturating_add(1);
        let action = match self
            .supplicant
            .on_group_message1(frame, &mut self.unwrap)
            .await
        {
            Ok(action) => action,
            Err(Wpa2ConnectedProcessError::Supplicant(error)) => {
                return self.fail(ConnectedWpa2SecurityFailure::Protocol(error));
            }
            Err(Wpa2ConnectedProcessError::KeyUnwrap(error)) => {
                return self.fail(ConnectedWpa2SecurityFailure::KeyUnwrap(error));
            }
        };
        let response = match action {
            Wpa2ConnectedAction::Retransmit(response) => {
                self.retransmitted = self.retransmitted.saturating_add(1);
                response
            }
            Wpa2ConnectedAction::InstallGroupKey(request) => {
                let Wpa2KeyKind::Group { key_id, .. } = request.group().kind() else {
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(ConnectedWpa2SecurityFailure::InvalidGroupKeyKind);
                };
                let replacement =
                    match StaGroupCcmpKeyMaterial::new(key_id, *request.group().key().as_bytes()) {
                        Ok(replacement) => replacement,
                        Err(error) => {
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            return self.fail(
                                ConnectedWpa2SecurityFailure::InvalidGroupKeyMaterial(error),
                            );
                        }
                    };
                let same_key_id = self.group_material.key_id() == replacement.key_id();
                let same_temporal_key = self.group_material.same_temporal_key(&replacement);
                if same_key_id && !same_temporal_key {
                    // S31 exposes one STA group slot and no RX descriptor key
                    // generation. A frame authenticated with the old GTK can
                    // already be staged when control receives Group M1. With
                    // the same logical KeyID it cannot be distinguished after
                    // a key replacement, so do not manufacture unsafe support.
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(ConnectedWpa2SecurityFailure::SameKeyIdGenerationUnavailable);
                }
                let key_id_mask = 1_u8 << replacement.key_id();
                if !same_key_id && self.used_group_key_ids & key_id_mask != 0 {
                    // The retired key ID may still label an authenticated
                    // frame in the RX staging pipeline. Reusing it for a new
                    // generation would make that frame indistinguishable
                    // from replacement traffic after hardware installation.
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(
                        ConnectedWpa2SecurityFailure::RetiredKeyIdGenerationUnavailable {
                            key_id: replacement.key_id(),
                        },
                    );
                }
                let prepared = match self
                    .replay
                    .prepare_group_rotation(key_id, *request.group().receive_sequence())
                {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                };
                let installing = match self.replay.begin_group_rotation(prepared) {
                    Ok(installing) => installing,
                    Err(error) => {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                };

                if same_key_id {
                    // An authenticated repeat of the exact GTK changes no
                    // hardware generation. Rotating only the RSC is safe even
                    // when old frames are staged because they authenticate
                    // under the still-current key.
                    if let Err(error) = self.replay.commit_group_rotation(installing) {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                } else {
                    match hardware.replace_sta_group_ccmp(
                        &mut self.group,
                        &self.group_material,
                        &replacement,
                    ) {
                        Ok(()) => {
                            if let Err(error) = self.replay.commit_group_rotation(installing) {
                                // Hardware now contains the replacement. The
                                // replay resource quarantines group RX on a
                                // failed commit; outer disconnect clears the
                                // slot without publishing a mixed epoch.
                                let _ = self.supplicant.complete_group_key_install(request, false);
                                return self
                                    .fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                            }
                            self.group_material = replacement;
                            self.used_group_key_ids |= key_id_mask;
                        }
                        Err(error @ StaGroupCcmpReplaceError::ReplacementRolledBack(_)) => {
                            let abort = self.replay.abort_group_rotation(installing);
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            if let Err(replay_error) = abort {
                                return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(
                                    replay_error,
                                ));
                            }
                            return self.fail(ConnectedWpa2SecurityFailure::KeyReplace(error));
                        }
                        Err(error) => {
                            self.replay.quarantine_group_rotation(installing);
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            return self.fail(ConnectedWpa2SecurityFailure::KeyReplace(error));
                        }
                    }
                }
                match self.supplicant.complete_group_key_install(request, true) {
                    Ok(response) => {
                        self.installed = self.installed.saturating_add(1);
                        response
                    }
                    Err(error) => {
                        return self.fail(ConnectedWpa2SecurityFailure::Protocol(error));
                    }
                }
            }
        };
        match tx.start_protected_eapol(hardware, response.as_bytes()) {
            Ok(progress) => {
                self.tx_in_flight = true;
                progress
            }
            Err(error) => self.fail(ConnectedWpa2SecurityFailure::TxStart(error)),
        }
    }
}

struct EmbassyReorderSink<'sender, 'resources, M: RawMutex> {
    sender: Option<&'sender RxReorderCommandSender<'resources, M>>,
}

impl<M: RawMutex> ConnectedControlReorder for EmbassyReorderSink<'_, '_, M> {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError> {
        let Some(sender) = self.sender else {
            return Ok(());
        };
        try_send_rx_reorder_command(sender, command)
    }
}

/// Unique Embassy event consumer around one executor-independent control core.
pub struct Esp32s31ConnectedControl<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    core: Esp32s31ConnectedControlCore,
    rx_block_ack: Esp32s31ConnectedRxBlockAck<'resources>,
    rx_reorder_commands: Option<RxReorderCommandSender<'resources, M>>,
    security: Option<ConnectedWpa2Security>,
    deferred_control_event: Option<ConnectedRxControlEvent>,
    hardware_doze_boundary_enabled: bool,
    doze_restore: Option<StaDozeRestore>,
    last_doze_boundary_failure: Option<StationDozeBoundaryFailure>,
    hardware_beacon_monitor: Option<StationHardwareBeaconMonitorEpoch>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeBoundaryFailure {
    Prepare(StaDozePrepareError),
    Hardware(StationDozeHardwareError),
}

const fn control_event_requires_active(event: ConnectedRxControlEvent) -> bool {
    matches!(
        event,
        ConnectedRxControlEvent::BlockAck(_) | ConnectedRxControlEvent::IndividualTwt(_)
    )
}

const fn he_control_event_requires_active(event: ConnectedRxControlEvent) -> bool {
    matches!(
        event,
        ConnectedRxControlEvent::Trigger {
            schedule: Ok(_),
            ..
        } | ConnectedRxControlEvent::Ndpa {
            addressed_to_station: true,
            ..
        }
    )
}

enum Esp32s31ConnectedRxBlockAck<'resources> {
    Local(Esp32s31StaApRxBlockAck),
    Shared(&'resources Esp32s31StaApRxBlockAck),
}

impl Esp32s31ConnectedRxBlockAck<'_> {
    const fn sessions(&self) -> &Esp32s31StaApRxBlockAck {
        match self {
            Self::Local(sessions) => sessions,
            Self::Shared(sessions) => sessions,
        }
    }
}

impl<'resources, M: RawMutex, const CAPACITY: usize>
    Esp32s31ConnectedControl<'resources, M, CAPACITY>
{
    pub fn new(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: open_esp_radio_esp32s31_wifi_mac::tx_ampdu::StaTxBlockAckSessions,
    ) -> Self {
        Self {
            receiver,
            core: Esp32s31ConnectedControlCore::new(peer, he_enabled, tx_block_ack),
            rx_block_ack: Esp32s31ConnectedRxBlockAck::Local(Esp32s31StaApRxBlockAck::new()),
            rx_reorder_commands: None,
            security: None,
            deferred_control_event: None,
            hardware_doze_boundary_enabled: false,
            doze_restore: None,
            last_doze_boundary_failure: None,
            hardware_beacon_monitor: None,
        }
    }

    pub fn new_shared(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: open_esp_radio_esp32s31_wifi_mac::tx_ampdu::StaTxBlockAckSessions,
        rx_block_ack: &'resources Esp32s31StaApRxBlockAck,
    ) -> Self {
        Self {
            receiver,
            core: Esp32s31ConnectedControlCore::new(peer, he_enabled, tx_block_ack),
            rx_block_ack: Esp32s31ConnectedRxBlockAck::Shared(rx_block_ack),
            rx_reorder_commands: None,
            security: None,
            deferred_control_event: None,
            hardware_doze_boundary_enabled: false,
            doze_restore: None,
            last_doze_boundary_failure: None,
            hardware_beacon_monitor: None,
        }
    }

    pub fn install_wpa2_security(
        &mut self,
        security: ConnectedWpa2Security,
    ) -> Result<(), ConnectedWpa2Security> {
        if self.security.is_some() {
            return Err(security);
        }
        self.security = Some(security);
        Ok(())
    }

    pub fn take_wpa2_security(&mut self) -> Option<ConnectedWpa2Security> {
        self.security.take()
    }

    pub const fn wpa2_security(&self) -> Option<&ConnectedWpa2Security> {
        self.security.as_ref()
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandSender<'resources, M>,
    ) -> Self {
        self.rx_reorder_commands = Some(commands);
        self
    }

    pub fn with_rx_block_ack_maximum_window(
        mut self,
        maximum_window: u16,
    ) -> Result<Self, open_esp_radio_esp32s31_wifi_mac::rx_ampdu::RxBlockAckSessionsError> {
        match &mut self.rx_block_ack {
            Esp32s31ConnectedRxBlockAck::Local(sessions) => {
                *sessions = Esp32s31StaApRxBlockAck::with_maximum_window(maximum_window)?;
            }
            Esp32s31ConnectedRxBlockAck::Shared(sessions) => {
                if sessions.maximum_window() != maximum_window {
                    return Err(open_esp_radio_esp32s31_wifi_mac::rx_ampdu::RxBlockAckSessionsError::InvalidWindow(maximum_window));
                }
            }
        }
        Ok(self)
    }

    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.core.enable_beacon_loss(config);
    }

    /// Bind one non-clone automatic-monitor admission owner to this connected
    /// association. The portable software deadline remains authoritative.
    pub fn enable_hardware_beacon_monitor_frontier(
        &mut self,
        binding: StationBeaconMonitorBinding,
        policy: StaBeaconLossConfig,
    ) -> Result<(), StationHardwareBeaconMonitorEpoch> {
        let epoch = StationHardwareBeaconMonitorEpoch::new(binding, policy);
        if self.hardware_beacon_monitor.is_some() {
            return Err(epoch);
        }
        self.hardware_beacon_monitor = Some(epoch);
        Ok(())
    }

    /// Last one-shot hardware admission result for this association.
    pub const fn hardware_beacon_monitor_frontier(
        &self,
    ) -> Option<StationHardwareBeaconMonitorFrontier> {
        match self.hardware_beacon_monitor.as_ref() {
            Some(epoch) => epoch.frontier(),
            None => None,
        }
    }

    pub const fn hardware_beacon_monitor_binding(&self) -> Option<StationBeaconMonitorBinding> {
        match self.hardware_beacon_monitor.as_ref() {
            Some(epoch) => Some(epoch.binding()),
            None => None,
        }
    }

    pub fn enable_power_save(&mut self, policy: StaPowerSavePolicy) {
        self.core.enable_power_save(policy);
        self.receiver.set_power_save_delivery_armed(false);
    }

    pub fn enable_ps_poll(
        &mut self,
        association_id: open_esp_radio_ieee80211::station_power_save::StaAssociationId,
    ) {
        self.core.enable_ps_poll(association_id);
    }

    pub fn enable_individual_twt_requester(&mut self, config: IndividualTwtRequesterConfig) {
        self.core.enable_individual_twt_requester(config);
    }

    pub fn queue_individual_twt_setup(
        &mut self,
        proposal: IndividualTwtProposal,
        now_micros: u64,
    ) -> Result<(), ConnectedControlError> {
        self.core.queue_individual_twt_setup(proposal, now_micros)
    }

    pub fn queue_individual_twt_teardown<H: ConnectedControlHardware>(
        &mut self,
        hardware: &mut H,
        flow_id: IndividualTwtFlowId,
        now_micros: u64,
    ) -> Result<(), ConnectedControlError> {
        self.core
            .queue_individual_twt_teardown(hardware, flow_id, now_micros)
    }

    pub fn with_he_trigger_based(
        mut self,
        config: Option<open_esp_radio_esp32s31_wifi_mac::tx::HeTriggerBasedTxConfig>,
    ) -> Self {
        self.core = self.core.with_he_trigger_based(config);
        self
    }

    /// Drive the affine hardware-doze boundary after each logical permit.
    /// ESP32-S31 production currently reaches this boundary and records
    /// `Unsupported`; it does not claim that RF or PHY entered sleep.
    pub fn enable_hardware_doze_boundary(&mut self) {
        self.hardware_doze_boundary_enabled = true;
    }

    pub const fn last_doze_boundary_failure(&self) -> Option<StationDozeBoundaryFailure> {
        self.last_doze_boundary_failure
    }

    /// Report the reviewed source frontier without entering modem, RF, PHY,
    /// baseband or clock sleep and without reading or writing hardware.
    pub const fn low_power_hardware_frontier(&self) -> StationLowPowerHardwareFrontier {
        station_low_power_hardware_frontier()
    }

    pub fn queue_initial_tx_block_ack(&mut self, attempt_limit: u8) {
        self.core.queue_initial_tx_block_ack(attempt_limit);
    }

    pub const fn rx_block_ack(&self) -> &Esp32s31StaApRxBlockAck {
        self.rx_block_ack.sessions()
    }

    pub const fn tx_block_ack(
        &self,
    ) -> &open_esp_radio_esp32s31_wifi_mac::tx_ampdu::StaTxBlockAckSessions {
        self.core.tx_block_ack()
    }

    pub const fn last_event(
        &self,
    ) -> Option<open_esp_radio_esp32s31_wifi_sta::connected_rx::ConnectedRxControlEvent> {
        self.core.last_event()
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.core.last_tx_failure()
    }

    pub const fn tx_in_flight(&self) -> bool {
        self.core.tx_in_flight()
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.core.last_expired_tid()
    }

    pub const fn stale_tx_block_ack_responses(&self) -> u32 {
        self.core.stale_tx_block_ack_responses()
    }

    pub const fn last_stale_tx_block_ack_token(&self) -> Option<u8> {
        self.core.last_stale_tx_block_ack_token()
    }

    pub const fn he_control_runtime_evidence(&self) -> ConnectedHeControlRuntimeEvidence {
        self.core.he_control_runtime_evidence()
    }

    pub const fn individual_twt_runtime_evidence(&self) -> ConnectedIndividualTwtRuntimeEvidence {
        self.core.individual_twt_runtime_evidence()
    }

    pub const fn individual_twt_requester(&self) -> Option<&IndividualTwtRequester> {
        self.core.individual_twt_requester()
    }

    pub fn individual_twt_wake_plan(
        &self,
        station_tsf: u64,
        wake_guard_micros: u32,
    ) -> Result<Option<IndividualTwtWakePlan>, ConnectedControlError> {
        self.core
            .individual_twt_wake_plan(station_tsf, wake_guard_micros)
    }

    pub fn dropped_he_observations(&self) -> u32 {
        self.receiver.dropped_he_observations()
    }

    pub const fn beacon_monitor(&self) -> Option<&StaBeaconMonitor> {
        self.core.beacon_monitor()
    }

    pub const fn beacon_lost(&self) -> bool {
        self.core.beacon_lost()
    }

    pub const fn power_save(&self) -> Option<&StaPowerSavePlanner> {
        self.core.power_save()
    }

    pub const fn power_save_wake_deadline_micros(&self) -> Option<u64> {
        self.core.power_save_wake_deadline_micros()
    }

    pub fn take_doze_permit(&mut self) -> Option<StaDozePermit> {
        self.core.take_doze_permit()
    }

    /// Bind the next logical permit to the live hardware TSF. The returned
    /// affine token still cannot enter RF/PHY sleep by itself; callers must
    /// present it to `ConnectedControlHardware::enter_station_doze`, whose
    /// production implementation deliberately fails closed until that leaf
    /// is audited.
    pub fn prepare_doze_transaction<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<StaPreparedDoze>, StaDozePrepareError>
    where
        H: ConnectedControlHardware,
    {
        self.take_doze_permit()
            .map(|permit| StaPreparedDoze::prepare(permit, hardware.station_tsf()))
            .transpose()
    }

    pub fn enter_prepared_doze<H>(
        prepared: StaPreparedDoze,
        hardware: &mut H,
    ) -> Result<
        open_esp_radio_wifi_sta::power_save::StaDozeRestore,
        open_esp_radio_wifi_sta::power_save::StaDozeEntryFailure<StationDozeHardwareError>,
    >
    where
        H: ConnectedControlHardware,
    {
        prepared.enter_with(hardware, ConnectedControlHardware::enter_station_doze)
    }

    pub fn restore_from_doze<H>(
        restore: StaDozeRestore,
        hardware: &mut H,
    ) -> Result<StaDozeRestored, StaDozeRestoreFailure<StationDozeHardwareError>>
    where
        H: ConnectedControlHardware,
    {
        restore.restore_with(hardware, ConnectedControlHardware::restore_station_awake)
    }

    fn service_hardware_doze_boundary<H>(
        &mut self,
        hardware: &mut H,
        allow_entry: bool,
    ) -> Result<Option<DatapathControlProgress<ConnectedDisconnectReason>>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
    {
        if let Some(restore) = self.doze_restore.take() {
            match Self::restore_from_doze(restore, hardware) {
                Ok(_) => {}
                Err(failure) => {
                    self.doze_restore = Some(failure.restore);
                    return Err(failure.error.into());
                }
            }
        }
        if !self.hardware_doze_boundary_enabled || !allow_entry {
            return Ok(None);
        }
        let Some(permit) = self.core.take_doze_permit() else {
            return Ok(None);
        };
        let prepared = match StaPreparedDoze::prepare(permit, hardware.station_tsf()) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.last_doze_boundary_failure = Some(StationDozeBoundaryFailure::Prepare(error));
                return Ok(None);
            }
        };
        match Self::enter_prepared_doze(prepared, hardware) {
            Ok(restore) => {
                self.doze_restore = Some(restore);
                self.last_doze_boundary_failure = None;
                // Do not immediately restore in this same scheduler turn.
                // The next timer, mailbox, network-TX or stop edge owns wake.
                Ok(Some(DatapathControlProgress::Idle))
            }
            Err(failure) => {
                self.last_doze_boundary_failure =
                    Some(StationDozeBoundaryFailure::Hardware(failure.error));
                Ok(None)
            }
        }
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<ConnectedControlShutdown, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.receiver.set_power_save_delivery_armed(false);
        if let Some(restore) = self.doze_restore.take()
            && let Err(failure) = Self::restore_from_doze(restore, hardware)
        {
            self.doze_restore = Some(failure.restore);
            return Err(failure.error.into());
        }
        let shutdown = self
            .rx_block_ack
            .sessions()
            .with_sessions(|rx_block_ack| self.core.shutdown(hardware, tx, rx_block_ack))?;
        let hardware_beacon_monitor = self
            .hardware_beacon_monitor
            .take()
            .map(StationHardwareBeaconMonitorEpoch::stop);
        let mut discarded_events = 0_u8;
        while self.receiver.try_receive().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        while self.receiver.try_receive_security().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        if self.deferred_control_event.take().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: shutdown.rx_block_ack_agreements,
            tx_block_ack_sessions: shutdown.tx_block_ack_sessions,
            discarded_events,
            in_flight: shutdown.in_flight,
            hardware_beacon_monitor,
        })
    }

    pub fn has_immediate_work(&self) -> bool {
        self.deferred_control_event.is_some()
            || self.receiver.overflowed()
            || self
                .security
                .as_ref()
                .is_some_and(ConnectedWpa2Security::tx_in_flight)
            || self.core.has_immediate_work(!self.receiver.is_empty())
    }

    /// Earliest role-local control deadline. Reading it does not require the
    /// ordinary/A-MPDU publication capability.
    pub fn next_alarm_deadline(&self) -> Option<u64> {
        self.core.next_alarm_deadline()
    }

    /// Paired-runtime wait which deliberately owns no physical TX resource.
    /// Embassy time is the production clock used by `EmbassyWifiTxTimer`, so
    /// this preserves the standalone deadline epoch without lending DMA to a
    /// sleeping station role.
    pub async fn wait_ready_without_tx(&mut self) {
        if self.has_immediate_work() {
            return;
        }
        if let Some(deadline) = self.next_alarm_deadline() {
            match select(
                self.receiver.ready(),
                Timer::at(Instant::from_micros(deadline)),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        } else {
            self.receiver.ready().await;
        }
    }

    /// Wait without consuming the event that made control work ready.
    pub async fn wait_ready<'a, X>(&'a mut self, tx: &'a mut X)
    where
        X: ConnectedControlTx + ConnectedControlTimer + 'a,
    {
        if self.has_immediate_work() {
            return;
        }
        if let Some(deadline) = self.core.next_alarm_deadline() {
            match select(self.receiver.ready(), tx.wait_until_micros(deadline)).await {
                Either::First(()) | Either::Second(()) => {}
            }
        } else {
            self.receiver.ready().await;
        }
    }

    fn service_core_step<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        event: Option<ConnectedRxControlEvent>,
        control_event_pending: bool,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let mut reorder = EmbassyReorderSink {
            sender: self.rx_reorder_commands.as_ref(),
        };
        let result = self.rx_block_ack.sessions().with_sessions(|rx_block_ack| {
            self.core.service_step(
                ConnectedControlPorts {
                    hardware,
                    tx,
                    reorder: &mut reorder,
                    rx_block_ack,
                },
                event,
                control_event_pending,
                context,
            )
        });
        let exiting = matches!(&result, Ok(DatapathControlProgress::Exit(_)));
        self.receiver
            .set_power_save_delivery_armed(!exiting && self.core.ps_poll_delivery_armed());
        result
    }

    pub fn service<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
    ) -> impl Future<
        Output = Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>,
    > + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        self.service_with_context(hardware, tx, DatapathControlContext::IDLE)
    }

    pub async fn service_with_context<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        if let Some(epoch) = self.hardware_beacon_monitor.as_mut()
            && epoch.frontier().is_none()
        {
            let snapshot = hardware.station_beacon_monitor_readback();
            let _ = epoch.evaluate_once(snapshot);
        }
        let allow_doze_entry = !context.network_tx_pending
            && !context.stop_pending
            && self.deferred_control_event.is_none()
            && self.receiver.is_empty();
        if let Some(progress) = self.service_hardware_doze_boundary(hardware, allow_doze_entry)? {
            return Ok(progress);
        }
        // The shared ordinary-TX completion always precedes newly queued
        // RX work. Security and the generic control core can never both
        // own that transaction.
        if let Some(security) = self.security.as_mut()
            && security.tx_in_flight()
        {
            return Ok(security.complete_tx(tx));
        }

        if self.core.tx_in_flight() {
            let queued_control_pending = self.deferred_control_event.is_some()
                || if self.core.ps_poll_tx_in_flight() {
                    self.receiver.non_power_save_delivery_pending()
                } else {
                    !self.receiver.is_empty()
                };
            return self.service_core_step(hardware, tx, None, queued_control_pending, context);
        }

        if self.receiver.overflowed() {
            return Ok(DatapathControlProgress::Exit(
                ConnectedDisconnectReason::ControlMailboxOverflow,
            ));
        }

        // Security processing can publish EAPOL immediately, so retain that
        // frame until an acknowledged PM=0 transition completes. Beacons are
        // deliberately handled below while PM=1: a mandatory listen/DTIM
        // receive edge is not itself an exit from legacy power-save.
        if self.receiver.security_pending()
            && self
                .core
                .power_save()
                .is_some_and(|planner| planner.state() == StaPowerSaveState::PowerSave)
        {
            return self.service_core_step(hardware, tx, None, true, context);
        }

        // A peer disconnect keeps terminal priority once TX ownership is
        // free. GTK rekey then precedes non-terminal BlockAck work.
        if let Some(event) = self.receiver.try_receive_terminal() {
            return self.service_core_step(
                hardware,
                tx,
                Some(event),
                !self.receiver.is_empty(),
                context,
            );
        }
        if let Some(frame) = self.receiver.try_receive_security() {
            let Some(security) = self.security.as_mut() else {
                return Ok(DatapathControlProgress::Exit(
                    ConnectedDisconnectReason::GroupKeyHandshakeFailed,
                ));
            };
            return Ok(security.process(hardware, tx, frame).await);
        }
        let event = self
            .deferred_control_event
            .take()
            .or_else(|| self.receiver.try_receive_power_save_delivery())
            .or_else(|| self.receiver.try_receive_control())
            .or_else(|| self.receiver.try_receive_he_observation());
        if event.is_some_and(|event| {
            control_event_requires_active(event)
                || (self.core.he_trigger_runtime_enabled()
                    && he_control_event_requires_active(event))
        }) && self
            .core
            .power_save()
            .is_some_and(|planner| planner.state() == StaPowerSaveState::PowerSave)
        {
            self.deferred_control_event = event;
            return self.service_core_step(hardware, tx, None, true, context);
        }
        self.service_core_step(
            hardware,
            tx,
            event,
            self.deferred_control_event.is_some() || !self.receiver.is_empty(),
            context,
        )
    }
}

impl<'resources, M, H, X, const CAPACITY: usize> DatapathControlService<H, X>
    for Esp32s31ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx + ConnectedControlTimer,
{
    type Error = ConnectedControlError;
    type Exit = ConnectedDisconnectReason;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        Esp32s31ConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn ready(&self, _tx: &X, now_micros: u64) -> bool {
        self.has_immediate_work()
            || self
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= now_micros)
    }

    fn required_before_network_tx(&self) -> bool {
        self.doze_restore.is_some()
            || self
                .core
                .power_save()
                .is_some_and(|planner| planner.state() != StaPowerSaveState::Awake)
    }

    fn required_before_stop(&self) -> bool {
        self.doze_restore.is_some()
            || self
                .core
                .power_save()
                .is_some_and(|planner| planner.state() != StaPowerSaveState::Awake)
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        Esp32s31ConnectedControl::wait_ready(self, tx)
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
