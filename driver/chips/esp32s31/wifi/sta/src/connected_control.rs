//! Executor-independent control state for one connected ESP32-S31 station.
//!
//! This module owns BlockAck, beacon-loss and power-save transitions.  A
//! runtime adapter supplies at most one received control event, a bounded
//! reorder-command sink and the shared TX owner.  No mailbox, executor timer
//! or task wakeup is part of this state machine.

use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::ConnectedRxControlEvent,
    rx_ampdu::{StaRxBlockAckActivation, StaRxBlockAckSessions, StaRxBlockAckSessionsError},
    rx_ampdu_hw::S31RxBlockAckAgreementError,
    tx::TxHardware,
    tx_ampdu::{
        BlockAckAction, STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckResponse, StaTxBlockAckSessions,
        StaTxBlockAckSessionsError, TxBlockAckResponse,
    },
};
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
    connected_control_hardware::ConnectedControlHardware,
    single_mpdu_tx::{
        ActionTxConfig, Esp32s31SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    },
};

/// Coherent runner-owned scheduling facts supplied to one control step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlContext {
    pub network_tx_pending: bool,
}

impl ConnectedControlContext {
    pub const IDLE: Self = Self {
        network_tx_pending: false,
    };
}

/// Result of one finite connected-control transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlProgress {
    Idle,
    More,
    TxPending,
    Disconnected,
}

/// Control frame currently owning the shared ordinary TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlTxKind {
    RxAddbaResponse { tid: u8 },
    TxAddbaRequest { tid: u8 },
    PowerManagement(StaPowerManagement),
}

enum ControlInFlight {
    RxAddba(StaRxBlockAckActivation),
    TxAddba { tid: u8 },
    PowerManagement(StaPowerManagement),
}

impl ControlInFlight {
    fn kind(&self) -> ConnectedControlTxKind {
        match self {
            Self::RxAddba(activation) => ConnectedControlTxKind::RxAddbaResponse {
                tid: activation.negotiated().tid,
            },
            Self::TxAddba { tid } => ConnectedControlTxKind::TxAddbaRequest { tid: *tid },
            Self::PowerManagement(mode) => ConnectedControlTxKind::PowerManagement(*mode),
        }
    }
}

/// Semantic command from connected control to the RX reorder owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommand {
    Start {
        tid: u8,
        starting_sequence: u16,
        window: u16,
    },
    Stop {
        tid: u8,
    },
    StopAll,
}

/// Bounded reorder-command publication failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommandError {
    Full(RxReorderCommand),
}

/// Runtime-neutral sink for semantic RX reorder commands.
pub trait ConnectedControlReorder {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError>;
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
    ) -> Result<ConnectedControlProgress, SingleMpduTxError>;

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<ConnectedControlProgress, SingleMpduTxError>;

    fn set_tx_block_ack_operational(&mut self, tid: u8, operational: bool);
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTx
    for Esp32s31SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
where
    P: crate::ordinary_tx::WifiTxPowerProfile,
    E: crate::ordinary_tx::WifiTxEntropy,
    T: crate::ordinary_tx::WifiTxTimer,
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        Esp32s31SingleMpduTx::take_last_outcome(self)
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
    ) -> Result<ConnectedControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_action(self, hardware, body, config)
            .map(|_| ConnectedControlProgress::TxPending)
    }

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<ConnectedControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_power_management_null(self, hardware, power_management)
            .map(|_| ConnectedControlProgress::TxPending)
    }

    fn set_tx_block_ack_operational(&mut self, _tid: u8, _operational: bool) {}
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
    RxSession(StaRxBlockAckSessionsError),
    TxSession(StaTxBlockAckSessionsError),
    Hardware(S31RxBlockAckAgreementError),
    Tx(SingleMpduTxError),
    MissingTxOutcome,
    MissingQosSequence(u8),
    BeaconDeadline(StaBeaconLossConfigError),
    PowerSaveCompletion(UnexpectedStaPowerManagementCompletion),
    MissingPowerSavePlanner,
    RxReorderCommand(RxReorderCommandError),
}

impl From<StaRxBlockAckSessionsError> for ConnectedControlError {
    fn from(error: StaRxBlockAckSessionsError) -> Self {
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
}

/// Complete protocol state for one ESP32-S31 station association.
pub struct Esp32s31ConnectedControlCore {
    peer: [u8; 6],
    he_enabled: bool,
    rx_block_ack: StaRxBlockAckSessions,
    tx_block_ack: StaTxBlockAckSessions,
    initial_tx_block_ack: [bool; 3],
    tx_block_ack_attempts_remaining: [u8; 3],
    in_flight: Option<ControlInFlight>,
    beacon_monitor: Option<StaBeaconMonitor>,
    beacon_lost: bool,
    power_save: Option<StaPowerSavePlanner>,
    pending_doze_permit: Option<StaDozePermit>,
    observations: ConnectedControlObservations,
}

impl Esp32s31ConnectedControlCore {
    pub fn new(peer: [u8; 6], he_enabled: bool, tx_block_ack: StaTxBlockAckSessions) -> Self {
        Self {
            peer,
            he_enabled,
            rx_block_ack: StaRxBlockAckSessions::new(),
            tx_block_ack,
            initial_tx_block_ack: [false; 3],
            tx_block_ack_attempts_remaining: [0; 3],
            in_flight: None,
            beacon_monitor: None,
            beacon_lost: false,
            power_save: None,
            pending_doze_permit: None,
            observations: ConnectedControlObservations::default(),
        }
    }

    pub fn set_rx_block_ack_maximum_window(
        &mut self,
        maximum_window: u16,
    ) -> Result<(), StaRxBlockAckSessionsError> {
        self.rx_block_ack = StaRxBlockAckSessions::with_maximum_window(maximum_window)?;
        Ok(())
    }

    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.beacon_monitor = Some(StaBeaconMonitor::new(config));
        self.beacon_lost = false;
    }

    pub fn enable_power_save(&mut self, policy: StaPowerSavePolicy) {
        self.power_save = Some(StaPowerSavePlanner::new(policy));
        self.pending_doze_permit = None;
    }

    /// Queue a bounded number of ADDBA publications for each recovered STA
    /// TID. A missing response or failed action-frame TX consumes one attempt
    /// and leaves the next one pending; an explicit peer response is terminal.
    pub fn queue_initial_tx_block_ack(&mut self, attempt_limit: u8) {
        debug_assert!(attempt_limit != 0);
        self.initial_tx_block_ack.fill(true);
        self.tx_block_ack_attempts_remaining.fill(attempt_limit);
    }

    pub const fn rx_block_ack(&self) -> &StaRxBlockAckSessions {
        &self.rx_block_ack
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
        match (
            block_ack,
            self.beacon_monitor
                .as_ref()
                .and_then(StaBeaconMonitor::deadline_micros),
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
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
                    self.rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_operational(tid, false);
                }
                ControlInFlight::PowerManagement(_) => {}
            }
        }

        let mut rx_block_ack_agreements = 0_u8;
        for agreement in self.rx_block_ack.snapshots().into_iter().flatten() {
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
            let stopped = self.rx_block_ack.stop(agreement.tid);
            debug_assert_eq!(stopped, Some(agreement));
            rx_block_ack_agreements = rx_block_ack_agreements.saturating_add(1);
        }
        let maximum_window = self.rx_block_ack.maximum_window();
        self.rx_block_ack = StaRxBlockAckSessions::with_maximum_window(maximum_window)
            .expect("an existing RX BlockAck maximum remains valid");

        let mut tx_block_ack_sessions = 0_u8;
        for tid in STA_TX_BLOCK_ACK_TIDS {
            if self.tx_block_ack.operational(tid).is_some()
                || self.tx_block_ack.alarm(tid).is_some()
            {
                tx_block_ack_sessions = tx_block_ack_sessions.saturating_add(1);
            }
            self.tx_block_ack.stop(tid);
            tx.set_tx_block_ack_operational(tid, false);
            if self.he_enabled {
                hardware.set_he_tid_enabled(tid, false)?;
            }
        }
        self.initial_tx_block_ack.fill(false);
        self.tx_block_ack_attempts_remaining.fill(0);
        self.beacon_monitor = None;
        self.beacon_lost = false;
        self.power_save = None;
        self.pending_doze_permit = None;

        Ok(ConnectedControlCoreShutdown {
            rx_block_ack_agreements,
            tx_block_ack_sessions,
            in_flight: in_flight_kind,
        })
    }

    /// Execute at most one finite transition.
    pub fn service_step<H, X, R>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        event: Option<ConnectedRxControlEvent>,
        control_event_pending: bool,
        context: ConnectedControlContext,
    ) -> Result<ConnectedControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
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
                    self.rx_block_ack.commit(activation)?;
                }
                ControlInFlight::RxAddba(activation) => {
                    hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
                    reorder.publish(RxReorderCommand::Stop {
                        tid: activation.negotiated().tid,
                    })?;
                    self.rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { .. } if success => {}
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_operational(tid, false);
                    if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                        .into_iter()
                        .position(|candidate| candidate == tid)
                        && self.tx_block_ack_attempts_remaining[index] != 0
                    {
                        self.initial_tx_block_ack[index] = true;
                    }
                }
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
                        return Ok(ConnectedControlProgress::Disconnected);
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
            return Ok(ConnectedControlProgress::More);
        }

        if let Some(event) = event {
            self.observations.last_event = Some(event);
            return self.apply_event(hardware, tx, reorder, event, context, control_event_pending);
        }

        let now_micros = tx.now_micros();
        if let Some(tid) = self.tx_block_ack.expire_next(now_micros) {
            tx.set_tx_block_ack_operational(tid, false);
            self.observations.last_expired_tid = Some(tid);
            if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                .into_iter()
                .position(|candidate| candidate == tid)
                && self.tx_block_ack_attempts_remaining[index] != 0
            {
                self.initial_tx_block_ack[index] = true;
            }
            return Ok(ConnectedControlProgress::More);
        }
        if self
            .beacon_monitor
            .as_ref()
            .is_some_and(|monitor| monitor.expired(now_micros))
        {
            for agreement in self.rx_block_ack.snapshots().into_iter().flatten() {
                self.rx_block_ack.stop(agreement.tid);
                hardware.clear_rx_block_ack(agreement.hardware_index)?;
            }
            reorder.publish(RxReorderCommand::StopAll)?;
            for tid in STA_TX_BLOCK_ACK_TIDS {
                self.tx_block_ack.stop(tid);
                tx.set_tx_block_ack_operational(tid, false);
                if self.he_enabled {
                    hardware.set_he_tid_enabled(tid, false)?;
                }
            }
            self.beacon_lost = true;
            return Ok(ConnectedControlProgress::Disconnected);
        }

        if context.network_tx_pending
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

        Ok(ConnectedControlProgress::Idle)
    }

    fn apply_event<H, X, R>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        event: ConnectedRxControlEvent,
        context: ConnectedControlContext,
        control_event_pending: bool,
    ) -> Result<ConnectedControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        if let ConnectedRxControlEvent::Beacon(observation) = event {
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
                    if matches!(decision, StaPowerSaveDecision::StayAwake(_))
                        && planner.state() == StaPowerSaveState::PowerSave
                    {
                        decision = planner.request_active();
                    }
                    decision
                };
                return self.apply_power_save_decision(hardware, tx, decision);
            }
            return Ok(ConnectedControlProgress::More);
        }
        let ConnectedRxControlEvent::BlockAck(action) = event else {
            return Ok(ConnectedControlProgress::More);
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
                self.rx_block_ack.offer(
                    dialog_token,
                    tid,
                    immediate,
                    window,
                    timeout_tu,
                    starting_sequence,
                )?;
                let Some(activation) = self.rx_block_ack.begin_pending(self.peer)? else {
                    return Ok(ConnectedControlProgress::More);
                };
                self.start_rx_addba_response(hardware, tx, reorder, activation)
            }
            BlockAckAction::AddbaResponse { .. } => {
                let StaTxBlockAckResponse { tid, response } =
                    self.tx_block_ack.on_response_action(action)?;
                if let Some(index) = STA_TX_BLOCK_ACK_TIDS
                    .into_iter()
                    .position(|candidate| candidate == tid)
                {
                    self.tx_block_ack_attempts_remaining[index] = 0;
                    self.initial_tx_block_ack[index] = false;
                }
                let operational = matches!(response, TxBlockAckResponse::Operational(_));
                tx.set_tx_block_ack_operational(tid, operational);
                if let TxBlockAckResponse::Operational(agreement) = response {
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(agreement.tid, true)?;
                    }
                } else if self.he_enabled {
                    hardware.set_he_tid_enabled(tid, false)?;
                }
                Ok(ConnectedControlProgress::More)
            }
            BlockAckAction::Delba { tid, initiator, .. } => {
                if initiator {
                    if let Some(agreement) = self.rx_block_ack.stop(tid) {
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                        reorder.publish(RxReorderCommand::Stop { tid })?;
                    }
                } else {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_operational(tid, false);
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(tid, false)?;
                    }
                }
                Ok(ConnectedControlProgress::More)
            }
        }
    }

    fn has_pending_traffic(
        &self,
        context: ConnectedControlContext,
        control_event_pending: bool,
    ) -> bool {
        context.network_tx_pending
            || control_event_pending
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    fn apply_power_save_decision<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        decision: StaPowerSaveDecision,
    ) -> Result<ConnectedControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match decision {
            StaPowerSaveDecision::PermitDoze(permit) => {
                self.pending_doze_permit = Some(permit);
                Ok(ConnectedControlProgress::More)
            }
            StaPowerSaveDecision::SendPowerManagement(power_management) => {
                self.pending_doze_permit = None;
                let progress = tx.start_power_management_null(hardware, power_management)?;
                self.in_flight = Some(ControlInFlight::PowerManagement(power_management));
                Ok(progress)
            }
            StaPowerSaveDecision::StayAwake(_) => {
                self.pending_doze_permit = None;
                Ok(ConnectedControlProgress::More)
            }
        }
    }

    fn start_rx_addba_response<H, X, R>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        activation: StaRxBlockAckActivation,
    ) -> Result<ConnectedControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = reorder.publish(RxReorderCommand::Stop { tid: replaced.tid }) {
                hardware.clear_rx_block_ack(replaced.hardware_index)?;
                self.rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
            hardware.clear_rx_block_ack(replaced.hardware_index)?;
        }
        hardware.program_rx_block_ack(activation.hardware())?;
        let negotiated = activation.negotiated();
        if let Err(error) = reorder.publish(RxReorderCommand::Start {
            tid: negotiated.tid,
            starting_sequence: negotiated.starting_sequence,
            window: negotiated.window,
        }) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = tx.start_action(
            hardware,
            activation.response_body(),
            ActionTxConfig::RX_ADDBA_RESPONSE,
        ) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            reorder.publish(RxReorderCommand::Stop {
                tid: activation.negotiated().tid,
            })?;
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::RxAddba(activation));
        Ok(ConnectedControlProgress::TxPending)
    }

    fn start_tx_addba<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        tid: u8,
    ) -> Result<ConnectedControlProgress, ConnectedControlError>
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
        Ok(ConnectedControlProgress::TxPending)
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
        assert!(core.has_pending_traffic(ConnectedControlContext::IDLE, false));
    }

    #[test]
    fn deadline_is_computed_without_an_executor_timer() {
        let mut core = core();
        assert_eq!(core.next_alarm_deadline(), None);

        core.tx_block_ack.begin(7, 23, 50).unwrap();
        assert_eq!(core.next_alarm_deadline(), Some(100_050));
    }
}
