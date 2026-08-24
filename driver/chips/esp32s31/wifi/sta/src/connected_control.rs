//! Executor-independent control state for one connected ESP32-S31 station.
//!
//! This module owns station BlockAck protocol, beacon-loss and power-save
//! transitions. The finite core does not own the physical RX BlockAck banks:
//! its caller supplies the one VIF-aware bank owner shared by STA and AP. A
//! runtime adapter supplies at most one received control event, a bounded
//! reorder-command sink and the shared TX owner.  No mailbox, executor timer
//! or task wakeup is part of this state machine.

use crate::connected_rx::ConnectedRxControlEvent;
use open_esp_radio_esp32s31_wifi::datapath::{DatapathControlContext, DatapathControlProgress};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    rx_ampdu::{
        RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessions, RxBlockAckSessionsError,
        RxReorderCommand, RxReorderCommandError,
    },
    rx_ampdu_hw::S31RxBlockAckAgreementError,
    tx::TxHardware,
    tx_ampdu::{
        BlockAckAction, STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckResponse,
        StaTxBlockAckResponseDisposition, StaTxBlockAckSessions, StaTxBlockAckSessionsError,
        TxBlockAckResponse,
    },
};
use open_esp_radio_ieee80211::station::{StaDisconnect, StaDisconnectKind};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;
use open_esp_radio_wifi_sta::{
    link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError, StaBeaconMonitor},
    power_save::{
        StaDozePermit, StaPowerManagementTxCompletion, StaPowerManagementTxOutcome,
        StaPowerSaveDecision, StaPowerSaveOpportunity, StaPowerSavePlanner, StaPowerSavePolicy,
        StaPowerSaveState, StaTrafficState, UnexpectedStaPowerManagementCompletion,
    },
};

use crate::{
    connected_control_hardware::{ConnectedControlHardware, StationDozeHardwareError},
    single_mpdu_tx::{
        ActionTxConfig, Esp32s31SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    },
};

const fn earliest_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left < right { left } else { right }),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

// Complete `libnet80211.a[ieee80211_sta.o]::send_ap_probe` rearms
// `mgd_probe_send_timeout` for 500 ms. Its timeout process retries a bounded
// five times before returning to the disconnect path.
const BEACON_PROBE_INTERVAL_MICROS: u64 = 500_000;
const BEACON_PROBE_ATTEMPT_LIMIT: u8 = 5;

/// Protocol reason which ended one connected station epoch.
///
/// This remains below the public radio facade: applications normally need
/// only link state, while the station lifecycle and qualification harness need
/// the exact cause in order to choose and verify reconnect policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedDisconnectReason {
    BeaconLoss,
    PeerDeauthentication {
        reason_code: u16,
    },
    PeerDisassociation {
        reason_code: u16,
    },
    /// The executor-side bounded mailbox lost a semantic control event.
    /// Continuing would make the connected protocol state unknowable, so the
    /// complete station epoch must be torn down and rebuilt.
    ControlMailboxOverflow,
    ActiveStateRestoreFailed,
    GroupKeyHandshakeFailed,
}

impl From<StaDisconnect> for ConnectedDisconnectReason {
    fn from(disconnect: StaDisconnect) -> Self {
        match disconnect.kind {
            StaDisconnectKind::Deauthentication => Self::PeerDeauthentication {
                reason_code: disconnect.reason_code,
            },
            StaDisconnectKind::Disassociation => Self::PeerDisassociation {
                reason_code: disconnect.reason_code,
            },
        }
    }
}

/// Control frame currently owning the shared ordinary TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlTxKind {
    RxAddbaResponse { tid: u8 },
    TxAddbaRequest { tid: u8 },
    BeaconProbe,
    PowerManagement(StaPowerManagement),
}

enum ControlInFlight {
    RxAddba(RxBlockAckActivation),
    TxAddba { tid: u8 },
    BeaconProbe,
    PowerManagement(StaPowerManagement),
}

impl ControlInFlight {
    fn kind(&self) -> ConnectedControlTxKind {
        match self {
            Self::RxAddba(activation) => ConnectedControlTxKind::RxAddbaResponse {
                tid: activation.negotiated().tid,
            },
            Self::TxAddba { tid } => ConnectedControlTxKind::TxAddbaRequest { tid: *tid },
            Self::BeaconProbe => ConnectedControlTxKind::BeaconProbe,
            Self::PowerManagement(mode) => ConnectedControlTxKind::PowerManagement(*mode),
        }
    }
}

/// Runtime-neutral sink for semantic RX reorder commands.
pub trait ConnectedControlReorder {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError>;
}

/// Mutually borrowed capabilities used by one finite connected-control step.
/// Grouping them makes the ownership boundary explicit and prevents event and
/// scheduling policy arguments from being confused with hardware owners.
pub struct ConnectedControlPorts<'a, H, X, R, const PEER_CAPACITY: usize> {
    pub hardware: &'a mut H,
    pub tx: &'a mut X,
    pub reorder: &'a mut R,
    pub rx_block_ack: &'a mut RxBlockAckSessions<PEER_CAPACITY>,
}

/// Shared ordinary-TX capability consumed by connected control.
pub trait ConnectedControlTx {
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome>;

    fn now_micros(&self) -> u64;

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16>;

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    /// Publish the negotiated TX BlockAck agreement for one TID.
    ///
    /// `None` stops aggregation. Keeping the exact negotiated window here is
    /// essential: an operational boolean cannot prevent the data path from
    /// publishing more MPDUs than the peer's reorder window can retain.
    fn set_tx_block_ack_agreement(&mut self, tid: u8, agreement: Option<(u16, bool)>);
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTx
    for Esp32s31SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
where
    P: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerProfile,
    E: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxEntropy,
    T: open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxTimer,
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        Esp32s31SingleMpduTx::take_last_outcome(self)
    }

    fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_beacon_probe(self, hardware)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn now_micros(&self) -> u64 {
        Esp32s31SingleMpduTx::now_micros(self)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        Esp32s31SingleMpduTx::peek_qos_sequence(self, tid)
    }

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_action(self, hardware, body, config)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_power_management_null(self, hardware, power_management)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_protected_eapol(self, hardware, payload)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn set_tx_block_ack_agreement(&mut self, _tid: u8, _agreement: Option<(u16, bool)>) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedControlTxFailure {
    pub kind: ConnectedControlTxKind,
    pub outcome: SingleMpduTxOutcome,
}

/// Executor-independent portion of connected-control shutdown evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlCoreShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub in_flight: Option<ConnectedControlTxKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlError {
    RxSession(RxBlockAckSessionsError),
    TxSession(StaTxBlockAckSessionsError),
    Hardware(S31RxBlockAckAgreementError),
    Tx(SingleMpduTxError),
    MissingTxOutcome,
    MissingQosSequence(u8),
    BeaconDeadline(StaBeaconLossConfigError),
    PowerSaveCompletion(UnexpectedStaPowerManagementCompletion),
    MissingPowerSavePlanner,
    PowerSaveDeadlineOverflow,
    DozeHardware(StationDozeHardwareError),
    RxReorderCommand(RxReorderCommandError),
}

impl From<RxBlockAckSessionsError> for ConnectedControlError {
    fn from(error: RxBlockAckSessionsError) -> Self {
        Self::RxSession(error)
    }
}

impl From<StaTxBlockAckSessionsError> for ConnectedControlError {
    fn from(error: StaTxBlockAckSessionsError) -> Self {
        Self::TxSession(error)
    }
}

impl From<S31RxBlockAckAgreementError> for ConnectedControlError {
    fn from(error: S31RxBlockAckAgreementError) -> Self {
        Self::Hardware(error)
    }
}

impl From<SingleMpduTxError> for ConnectedControlError {
    fn from(error: SingleMpduTxError) -> Self {
        Self::Tx(error)
    }
}

impl From<StaBeaconLossConfigError> for ConnectedControlError {
    fn from(error: StaBeaconLossConfigError) -> Self {
        Self::BeaconDeadline(error)
    }
}

impl From<UnexpectedStaPowerManagementCompletion> for ConnectedControlError {
    fn from(error: UnexpectedStaPowerManagementCompletion) -> Self {
        Self::PowerSaveCompletion(error)
    }
}

impl From<StationDozeHardwareError> for ConnectedControlError {
    fn from(error: StationDozeHardwareError) -> Self {
        Self::DozeHardware(error)
    }
}

impl From<RxReorderCommandError> for ConnectedControlError {
    fn from(error: RxReorderCommandError) -> Self {
        Self::RxReorderCommand(error)
    }
}

#[derive(Default)]
struct ConnectedControlObservations {
    last_event: Option<ConnectedRxControlEvent>,
    last_tx_failure: Option<ConnectedControlTxFailure>,
    last_expired_tid: Option<u8>,
    stale_tx_block_ack_responses: u32,
    last_stale_tx_block_ack_token: Option<u8>,
}

/// Complete protocol state for one ESP32-S31 station association.
pub struct Esp32s31ConnectedControlCore {
    peer: [u8; 6],
    he_enabled: bool,
    tx_block_ack: StaTxBlockAckSessions,
    initial_tx_block_ack: [bool; 3],
    tx_block_ack_attempts_remaining: [u8; 3],
    in_flight: Option<ControlInFlight>,
    beacon_monitor: Option<StaBeaconMonitor>,
    beacon_probe_attempts: u8,
    beacon_lost: bool,
    power_save: Option<StaPowerSavePlanner>,
    pending_doze_permit: Option<StaDozePermit>,
    power_save_wake_deadline_micros: Option<u64>,
    observations: ConnectedControlObservations,
}

impl Esp32s31ConnectedControlCore {
    pub fn new(peer: [u8; 6], he_enabled: bool, tx_block_ack: StaTxBlockAckSessions) -> Self {
        Self {
            peer,
            he_enabled,
            tx_block_ack,
            initial_tx_block_ack: [false; 3],
            tx_block_ack_attempts_remaining: [0; 3],
            in_flight: None,
            beacon_monitor: None,
            beacon_probe_attempts: 0,
            beacon_lost: false,
            power_save: None,
            pending_doze_permit: None,
            power_save_wake_deadline_micros: None,
            observations: ConnectedControlObservations::default(),
        }
    }

    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.beacon_monitor = Some(StaBeaconMonitor::new(config));
        self.beacon_probe_attempts = 0;
        self.beacon_lost = false;
    }

    pub fn enable_power_save(&mut self, policy: StaPowerSavePolicy) {
        self.power_save = Some(StaPowerSavePlanner::new(policy));
        self.pending_doze_permit = None;
        self.power_save_wake_deadline_micros = None;
    }

    /// Queue a bounded number of ADDBA publications for each recovered STA
    /// TID. A missing response or failed action-frame TX consumes one attempt
    /// and leaves the next one pending; an explicit peer response is terminal.
    pub fn queue_initial_tx_block_ack(&mut self, attempt_limit: u8) {
        debug_assert!(attempt_limit != 0);
        self.initial_tx_block_ack.fill(true);
        self.tx_block_ack_attempts_remaining.fill(attempt_limit);
    }

    pub const fn tx_block_ack(&self) -> &StaTxBlockAckSessions {
        &self.tx_block_ack
    }

    pub const fn last_event(&self) -> Option<ConnectedRxControlEvent> {
        self.observations.last_event
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.observations.last_tx_failure
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.observations.last_expired_tid
    }

    pub const fn stale_tx_block_ack_responses(&self) -> u32 {
        self.observations.stale_tx_block_ack_responses
    }

    pub const fn last_stale_tx_block_ack_token(&self) -> Option<u8> {
        self.observations.last_stale_tx_block_ack_token
    }

    pub const fn beacon_monitor(&self) -> Option<&StaBeaconMonitor> {
        self.beacon_monitor.as_ref()
    }

    pub const fn beacon_lost(&self) -> bool {
        self.beacon_lost
    }

    pub const fn power_save(&self) -> Option<&StaPowerSavePlanner> {
        self.power_save.as_ref()
    }

    pub fn take_doze_permit(&mut self) -> Option<StaDozePermit> {
        self.pending_doze_permit.take()
    }

    pub const fn power_save_wake_deadline_micros(&self) -> Option<u64> {
        self.power_save_wake_deadline_micros
    }

    /// Whether the next step must consume the shared TX completion before a
    /// newly delivered control event.
    pub const fn tx_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn has_immediate_work(&self, control_event_pending: bool) -> bool {
        self.in_flight.is_some()
            || control_event_pending
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    pub fn next_alarm_deadline(&self) -> Option<u64> {
        let block_ack = STA_TX_BLOCK_ACK_TIDS
            .into_iter()
            .filter_map(|tid| self.tx_block_ack.alarm(tid))
            .map(|alarm| alarm.deadline_us)
            .min();
        let link = self
            .beacon_monitor
            .as_ref()
            .and_then(StaBeaconMonitor::deadline_micros);
        earliest_deadline(
            earliest_deadline(block_ack, link),
            self.power_save_wake_deadline_micros,
        )
    }

    pub fn shutdown<H, X, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
    ) -> Result<ConnectedControlCoreShutdown, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let in_flight_kind = self.in_flight.as_ref().map(ControlInFlight::kind);
        if let Some(in_flight) = self.in_flight.take() {
            match in_flight {
                ControlInFlight::RxAddba(activation) => {
                    if let Err(error) =
                        hardware.clear_rx_block_ack(activation.hardware().hardware_index)
                    {
                        self.in_flight = Some(ControlInFlight::RxAddba(activation));
                        return Err(error.into());
                    }
                    rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_agreement(tid, None);
                }
                ControlInFlight::BeaconProbe => {}
                ControlInFlight::PowerManagement(_) => {}
            }
        }

        let mut rx_block_ack_agreements = 0_u8;
        for agreement in rx_block_ack
            .snapshots_for(MacInterface::Station)
            .into_iter()
            .flatten()
        {
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
            let stopped = rx_block_ack.stop(MacInterface::Station, self.peer, agreement.tid);
            debug_assert_eq!(stopped, Some(agreement));
            rx_block_ack_agreements = rx_block_ack_agreements.saturating_add(1);
        }
        rx_block_ack.prepare_interface(MacInterface::Station)?;

        let mut tx_block_ack_sessions = 0_u8;
        for tid in STA_TX_BLOCK_ACK_TIDS {
            if self.tx_block_ack.operational(tid).is_some()
                || self.tx_block_ack.alarm(tid).is_some()
            {
                tx_block_ack_sessions = tx_block_ack_sessions.saturating_add(1);
            }
            self.tx_block_ack.stop(tid);
            tx.set_tx_block_ack_agreement(tid, None);
            if self.he_enabled {
                hardware.set_he_tid_enabled(tid, false)?;
            }
        }
        self.initial_tx_block_ack.fill(false);
        self.tx_block_ack_attempts_remaining.fill(0);
        self.beacon_monitor = None;
        self.beacon_probe_attempts = 0;
        self.beacon_lost = false;
        self.power_save = None;
        self.pending_doze_permit = None;
        self.power_save_wake_deadline_micros = None;

        Ok(ConnectedControlCoreShutdown {
            rx_block_ack_agreements,
            tx_block_ack_sessions,
            in_flight: in_flight_kind,
        })
    }

    /// Execute at most one finite transition.
    pub fn service_step<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        ports: ConnectedControlPorts<'_, H, X, R, PEER_CAPACITY>,
        event: Option<ConnectedRxControlEvent>,
        control_event_pending: bool,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        let ConnectedControlPorts {
            hardware,
            tx,
            reorder,
            rx_block_ack,
        } = ports;
        if let Some(monitor) = &mut self.beacon_monitor {
            monitor.arm(tx.now_micros())?;
        }
        if let Some(in_flight) = self.in_flight.take() {
            let outcome = tx
                .take_last_outcome()
                .ok_or(ConnectedControlError::MissingTxOutcome)?;
            let success = outcome.is_success();
            if !success {
                self.observations.last_tx_failure = Some(ConnectedControlTxFailure {
                    kind: in_flight.kind(),
                    outcome,
                });
            }
            match in_flight {
                ControlInFlight::RxAddba(activation) if success => {
                    rx_block_ack.commit(activation)?;
                }
                ControlInFlight::RxAddba(activation) => {
                    hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
                    reorder.publish(RxReorderCommand::Stop(activation.negotiated().identity()))?;
                    rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { .. } if success => {}
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_agreement(tid, None);
                    if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                        .into_iter()
                        .position(|candidate| candidate == tid)
                        && self.tx_block_ack_attempts_remaining[index] != 0
                    {
                        self.initial_tx_block_ack[index] = true;
                    }
                }
                ControlInFlight::BeaconProbe => {}
                ControlInFlight::PowerManagement(advertised) => {
                    let completion = StaPowerManagementTxCompletion {
                        advertised,
                        outcome: if success {
                            StaPowerManagementTxOutcome::Acknowledged
                        } else {
                            StaPowerManagementTxOutcome::Failed
                        },
                        station_tsf: hardware.station_tsf(),
                    };
                    let mut decision = self
                        .power_save
                        .as_mut()
                        .ok_or(ConnectedControlError::MissingPowerSavePlanner)?
                        .complete_power_management(completion)?;

                    if advertised == StaPowerManagement::Active && !success {
                        self.pending_doze_permit = None;
                        return Ok(DatapathControlProgress::Exit(
                            ConnectedDisconnectReason::ActiveStateRestoreFailed,
                        ));
                    }
                    if self.has_pending_traffic(context, control_event_pending)
                        && self
                            .power_save
                            .as_ref()
                            .is_some_and(|planner| planner.state() == StaPowerSaveState::PowerSave)
                    {
                        decision = self
                            .power_save
                            .as_mut()
                            .expect("power-save planner was checked above")
                            .request_active();
                    }
                    return self.apply_power_save_decision(hardware, tx, decision);
                }
            }
            return Ok(DatapathControlProgress::More);
        }

        let now_micros = tx.now_micros();
        if self
            .power_save_wake_deadline_micros
            .is_some_and(|deadline| now_micros >= deadline)
        {
            // The hardware/Embassy wake boundary has been reached. Remain in
            // AP-visible power-save while listening to the mandatory beacon;
            // buffered traffic or a local TX request will separately drive
            // the acknowledged PM=0 transition.
            self.power_save_wake_deadline_micros = None;
            self.pending_doze_permit = None;
            return Ok(DatapathControlProgress::More);
        }

        if let Some(event) = event {
            self.observations.last_event = Some(event);
            return self.apply_event(
                ConnectedControlPorts {
                    hardware,
                    tx,
                    reorder,
                    rx_block_ack,
                },
                event,
                context,
                control_event_pending,
            );
        }

        if let Some(tid) = self.tx_block_ack.expire_next(now_micros) {
            tx.set_tx_block_ack_agreement(tid, None);
            self.observations.last_expired_tid = Some(tid);
            if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                .into_iter()
                .position(|candidate| candidate == tid)
                && self.tx_block_ack_attempts_remaining[index] != 0
            {
                self.initial_tx_block_ack[index] = true;
            }
            return Ok(DatapathControlProgress::More);
        }
        if self
            .beacon_monitor
            .as_ref()
            .is_some_and(|monitor| monitor.expired(now_micros))
        {
            if self.beacon_probe_attempts < BEACON_PROBE_ATTEMPT_LIMIT {
                return self.start_beacon_probe(hardware, tx);
            }
            return self.disconnect_for_beacon_loss(hardware, tx, reorder, rx_block_ack);
        }

        if self.has_pending_traffic(context, control_event_pending)
            && self
                .power_save
                .as_ref()
                .is_some_and(|planner| planner.state() == StaPowerSaveState::PowerSave)
        {
            let decision = self
                .power_save
                .as_mut()
                .expect("power-save planner was checked above")
                .request_active();
            return self.apply_power_save_decision(hardware, tx, decision);
        }

        if let Some(index) = self
            .initial_tx_block_ack
            .iter()
            .position(|pending| *pending)
        {
            self.initial_tx_block_ack[index] = false;
            self.tx_block_ack_attempts_remaining[index] -= 1;
            let tid = STA_TX_BLOCK_ACK_TIDS[index];
            return self.start_tx_addba(hardware, tx, tid);
        }

        Ok(DatapathControlProgress::Idle)
    }

    fn apply_event<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        ports: ConnectedControlPorts<'_, H, X, R, PEER_CAPACITY>,
        event: ConnectedRxControlEvent,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        let ConnectedControlPorts {
            hardware,
            tx,
            reorder,
            rx_block_ack,
        } = ports;
        if let ConnectedRxControlEvent::PeerDisconnect(disconnect) = event {
            return Ok(DatapathControlProgress::Exit(disconnect.into()));
        }
        if let ConnectedRxControlEvent::Beacon(observation) = event {
            self.beacon_probe_attempts = 0;
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.observe(tx.now_micros(), observation)?;
            }
            if self.power_save.is_some() {
                let traffic = if self.has_pending_traffic(context, control_event_pending) {
                    StaTrafficState::Pending
                } else {
                    StaTrafficState::Quiescent
                };
                let opportunity = StaPowerSaveOpportunity {
                    beacon: observation,
                    station_tsf: hardware.station_tsf(),
                    traffic,
                };
                let decision = {
                    let planner = self
                        .power_save
                        .as_mut()
                        .expect("power-save planner was checked above");
                    let mut decision = planner.observe_beacon(opportunity);
                    if decision.requires_active_advertisement()
                        && planner.state() == StaPowerSaveState::PowerSave
                    {
                        decision = planner.request_active();
                    }
                    decision
                };
                return self.apply_power_save_decision(hardware, tx, decision);
            }
            return Ok(DatapathControlProgress::More);
        }
        if let ConnectedRxControlEvent::ProbeResponse = event {
            self.beacon_probe_attempts = 0;
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.observe_reachability(tx.now_micros())?;
            }
            return Ok(DatapathControlProgress::More);
        }
        let ConnectedRxControlEvent::BlockAck(action) = event else {
            return Ok(DatapathControlProgress::More);
        };
        match action {
            BlockAckAction::AddbaRequest {
                dialog_token,
                tid,
                immediate,
                window,
                timeout_tu,
                starting_sequence,
                ..
            } => {
                rx_block_ack.offer(RxBlockAckRequest {
                    interface: MacInterface::Station,
                    peer: self.peer,
                    dialog_token,
                    tid,
                    immediate,
                    requested_window: window,
                    timeout_tu,
                    starting_sequence,
                })?;
                let Some(activation) = rx_block_ack.begin_pending()? else {
                    return Ok(DatapathControlProgress::More);
                };
                self.start_rx_addba_response(hardware, tx, reorder, rx_block_ack, activation)
            }
            BlockAckAction::AddbaResponse { .. } => {
                let response = match self.tx_block_ack.on_response_action(action)? {
                    StaTxBlockAckResponseDisposition::Matched(response) => response,
                    StaTxBlockAckResponseDisposition::StaleDialogToken(token) => {
                        self.observations.stale_tx_block_ack_responses = self
                            .observations
                            .stale_tx_block_ack_responses
                            .saturating_add(1);
                        self.observations.last_stale_tx_block_ack_token = Some(token);
                        return Ok(DatapathControlProgress::More);
                    }
                };
                let StaTxBlockAckResponse { tid, response } = response;
                if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                    .into_iter()
                    .position(|candidate| candidate == tid)
                {
                    self.tx_block_ack_attempts_remaining[index] = 0;
                    self.initial_tx_block_ack[index] = false;
                }
                let negotiated_agreement = match response {
                    TxBlockAckResponse::Operational(agreement) => {
                        Some((agreement.window, agreement.amsdu))
                    }
                    TxBlockAckResponse::Rejected(_) => None,
                };
                tx.set_tx_block_ack_agreement(tid, negotiated_agreement);
                if let TxBlockAckResponse::Operational(agreement) = response {
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(agreement.tid, true)?;
                    }
                } else if self.he_enabled {
                    hardware.set_he_tid_enabled(tid, false)?;
                }
                Ok(DatapathControlProgress::More)
            }
            BlockAckAction::Delba { tid, initiator, .. } => {
                if initiator {
                    if let Some(agreement) =
                        rx_block_ack.stop(MacInterface::Station, self.peer, tid)
                    {
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                        reorder.publish(RxReorderCommand::Stop(agreement.identity()))?;
                    }
                } else {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_agreement(tid, None);
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(tid, false)?;
                    }
                }
                Ok(DatapathControlProgress::More)
            }
        }
    }

    fn has_pending_traffic(
        &self,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> bool {
        context.network_tx_pending
            || context.stop_pending
            || control_event_pending
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    fn start_beacon_probe<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let now_micros = tx.now_micros();
        self.beacon_monitor
            .as_mut()
            .expect("beacon probes require an enabled beacon monitor")
            .wait_for_reachability(now_micros, BEACON_PROBE_INTERVAL_MICROS)?;
        let progress = tx.start_beacon_probe(hardware)?;
        self.beacon_probe_attempts += 1;
        self.in_flight = Some(ControlInFlight::BeaconProbe);
        Ok(progress)
    }

    fn disconnect_for_beacon_loss<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        for agreement in rx_block_ack
            .snapshots_for(MacInterface::Station)
            .into_iter()
            .flatten()
        {
            rx_block_ack.stop(MacInterface::Station, self.peer, agreement.tid);
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        reorder.publish(RxReorderCommand::StopInterface(MacInterface::Station))?;
        for tid in STA_TX_BLOCK_ACK_TIDS {
            self.tx_block_ack.stop(tid);
            tx.set_tx_block_ack_agreement(tid, None);
            if self.he_enabled {
                hardware.set_he_tid_enabled(tid, false)?;
            }
        }
        self.beacon_probe_attempts = 0;
        self.beacon_lost = true;
        self.pending_doze_permit = None;
        self.power_save_wake_deadline_micros = None;
        Ok(DatapathControlProgress::Exit(
            ConnectedDisconnectReason::BeaconLoss,
        ))
    }

    fn apply_power_save_decision<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        decision: StaPowerSaveDecision,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match decision {
            StaPowerSaveDecision::PermitDoze(permit) => {
                let station_tsf = hardware.station_tsf();
                let until_wake = permit.wake_tsf.wrapping_sub(station_tsf);
                if until_wake == 0 || until_wake > i64::MAX as u64 {
                    self.pending_doze_permit = None;
                    self.power_save_wake_deadline_micros = None;
                    return Ok(DatapathControlProgress::More);
                }
                self.power_save_wake_deadline_micros = Some(
                    tx.now_micros()
                        .checked_add(until_wake)
                        .ok_or(ConnectedControlError::PowerSaveDeadlineOverflow)?,
                );
                self.pending_doze_permit = Some(permit);
                Ok(DatapathControlProgress::More)
            }
            StaPowerSaveDecision::SendPowerManagement(power_management) => {
                self.pending_doze_permit = None;
                self.power_save_wake_deadline_micros = None;
                let progress = tx.start_power_management_null(hardware, power_management)?;
                self.in_flight = Some(ControlInFlight::PowerManagement(power_management));
                Ok(progress)
            }
            StaPowerSaveDecision::StayAwake(_) => {
                self.pending_doze_permit = None;
                self.power_save_wake_deadline_micros = None;
                Ok(DatapathControlProgress::More)
            }
        }
    }

    fn start_rx_addba_response<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
        activation: RxBlockAckActivation,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = reorder.publish(RxReorderCommand::Stop(replaced.identity())) {
                hardware.clear_rx_block_ack(replaced.hardware_index)?;
                rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
            hardware.clear_rx_block_ack(replaced.hardware_index)?;
        }
        hardware.program_rx_block_ack(activation.hardware())?;
        let negotiated = activation.negotiated();
        if let Err(error) = reorder.publish(RxReorderCommand::Start(negotiated)) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = tx.start_action(
            hardware,
            activation.response_body(),
            ActionTxConfig::RX_ADDBA_RESPONSE,
        ) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            reorder.publish(RxReorderCommand::Stop(activation.negotiated().identity()))?;
            rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::RxAddba(activation));
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_tx_addba<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        tid: u8,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let sequence = tx
            .peek_qos_sequence(tid)
            .ok_or(ConnectedControlError::MissingQosSequence(tid))?;
        let request = self.tx_block_ack.begin(tid, sequence, tx.now_micros())?;
        if let Err(error) =
            tx.start_action(hardware, &request.body, ActionTxConfig::VENDOR_MANAGEMENT)
        {
            self.tx_block_ack.stop(tid);
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::TxAddba { tid });
        Ok(DatapathControlProgress::TxPending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> Esp32s31ConnectedControlCore {
        Esp32s31ConnectedControlCore::new(
            [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            true,
            StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
        )
    }

    #[test]
    fn readiness_combines_owned_state_with_external_event_state() {
        let mut core = core();
        assert!(!core.has_immediate_work(false));
        assert!(core.has_immediate_work(true));

        core.initial_tx_block_ack[1] = true;
        core.tx_block_ack_attempts_remaining[1] = 1;
        assert!(core.has_immediate_work(false));
        assert!(core.has_pending_traffic(DatapathControlContext::IDLE, false));
    }

    #[test]
    fn deadline_is_computed_without_an_executor_timer() {
        let mut core = core();
        assert_eq!(core.next_alarm_deadline(), None);

        core.tx_block_ack.begin(7, 23, 50).unwrap();
        assert_eq!(core.next_alarm_deadline(), Some(100_050));
    }
}
