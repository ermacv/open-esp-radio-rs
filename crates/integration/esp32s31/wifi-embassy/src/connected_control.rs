//! Owned connected-station control scheduler.
//!
//! RX dispatch publishes fixed semantic events into an Embassy mailbox. This
//! owner consumes at most one event per scheduling step, applies finite PAC
//! transactions, and publishes Action frames through the same pinned TX slot
//! used by ordinary network data. It contains no logging, NVS, RTOS callback
//! or vendor context layout.

use core::future::Future;

use embassy_futures::select::{Either, select};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_pac::{MacHeTid, RadioRegisters};
use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::ConnectedRxControlEvent,
    rx_ampdu::{StaRxBlockAckActivation, StaRxBlockAckSessions, StaRxBlockAckSessionsError},
    rx_ampdu_hw::{self, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::TxHardware,
    tx_ampdu::{
        BlockAckAction, STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckResponse, StaTxBlockAckSessions,
        StaTxBlockAckSessionsError, TxBlockAckResponse,
    },
};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;

use crate::{
    backend::Esp32s31ControlService,
    link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError, StaBeaconMonitor},
    runner::{WifiControlContext, WifiControlProgress},
    rx_backend::ConnectedControlReceiver,
    rx_reorder::{
        RxReorderCommand, RxReorderCommandError, RxReorderCommandSender,
        try_send_rx_reorder_command,
    },
    single_mpdu_tx::{
        ActionTxConfig, Esp32s31SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    },
    station_power_save::{
        StaDozePermit, StaPowerManagementTxCompletion, StaPowerManagementTxOutcome,
        StaPowerSaveDecision, StaPowerSaveOpportunity, StaPowerSavePlanner, StaPowerSavePolicy,
        StaPowerSaveState, StaTrafficState, UnexpectedStaPowerManagementCompletion,
    },
};

/// PAC authority required by connected BlockAck control.
pub trait ConnectedControlHardware: TxHardware {
    fn station_tsf(&mut self) -> u64;

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError>;

    fn clear_rx_block_ack(&mut self, hardware_index: u8)
    -> Result<(), S31RxBlockAckAgreementError>;

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError>;
}

impl ConnectedControlHardware for RadioRegisters {
    fn station_tsf(&mut self) -> u64 {
        RadioRegisters::station_tsf(self)
    }

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::program(self, agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::clear(self, hardware_index)
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let tid = MacHeTid::new(tid).ok_or(S31RxBlockAckAgreementError::Tid(tid))?;
        RadioRegisters::set_he_trigger_based_tid_enabled(self, tid, enabled);
        Ok(())
    }
}

/// Minimal connected-TX capability consumed by the BlockAck control plane.
///
/// Keeping this interface independent of buffer sizes and concrete TX owners
/// lets the same control state machine drive both an ordinary-only bring-up
/// fixture and the production aggregate owner. The implementation remains
/// monomorphized; this is not a dynamic runtime adapter.
pub trait ConnectedControlTx {
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome>;

    fn now_micros(&self) -> u64;

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16>;

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiControlProgress, SingleMpduTxError>;

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiControlProgress, SingleMpduTxError>;

    /// Mirror the protocol session into the data scheduler. Ordinary-only
    /// fixtures deliberately ignore this edge; aggregate owners use it as
    /// the sole permission to publish an A-MPDU for the TID.
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

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Esp32s31SingleMpduTx::wait_until_micros(self, deadline_micros)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        Esp32s31SingleMpduTx::peek_qos_sequence(self, tid)
    }

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_action(self, hardware, body, config)
            .map(|_| WifiControlProgress::TxPending)
    }

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_power_management_null(self, hardware, power_management)
            .map(|_| WifiControlProgress::TxPending)
    }

    fn set_tx_block_ack_operational(&mut self, _tid: u8, _operational: bool) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlTxKind {
    RxAddbaResponse { tid: u8 },
    TxAddbaRequest { tid: u8 },
    PowerManagement(StaPowerManagement),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedControlTxFailure {
    pub kind: ConnectedControlTxKind,
    pub outcome: SingleMpduTxOutcome,
}

/// Finite ownership released when one connected control epoch stops.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub discarded_events: u8,
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

/// Unique consumer for connected control events and BlockAck state.
pub struct Esp32s31ConnectedControl<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    peer: [u8; 6],
    he_enabled: bool,
    rx_block_ack: StaRxBlockAckSessions,
    tx_block_ack: StaTxBlockAckSessions,
    initial_tx_block_ack: [bool; 3],
    in_flight: Option<ControlInFlight>,
    last_event: Option<ConnectedRxControlEvent>,
    last_tx_failure: Option<ConnectedControlTxFailure>,
    last_expired_tid: Option<u8>,
    beacon_monitor: Option<StaBeaconMonitor>,
    beacon_lost: bool,
    power_save: Option<StaPowerSavePlanner>,
    pending_doze_permit: Option<StaDozePermit>,
    rx_reorder_commands: Option<RxReorderCommandSender<'resources, M>>,
}

impl<'resources, M: RawMutex, const CAPACITY: usize>
    Esp32s31ConnectedControl<'resources, M, CAPACITY>
{
    pub fn new(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: StaTxBlockAckSessions,
    ) -> Self {
        Self {
            receiver,
            peer,
            he_enabled,
            rx_block_ack: StaRxBlockAckSessions::new(),
            tx_block_ack,
            initial_tx_block_ack: [false; 3],
            in_flight: None,
            last_event: None,
            last_tx_failure: None,
            last_expired_tid: None,
            beacon_monitor: None,
            beacon_lost: false,
            power_save: None,
            pending_doze_permit: None,
            rx_reorder_commands: None,
        }
    }

    /// Bind the control owner to the staged-RX reorder command path.
    ///
    /// Minimal fixtures may omit this adapter. A production owner that
    /// accepts RX BlockAck agreements must install it before service starts.
    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandSender<'resources, M>,
    ) -> Self {
        self.rx_reorder_commands = Some(commands);
        self
    }

    /// Bound negotiated receive BlockAck windows to the staging resources
    /// selected by the platform composition.
    pub fn with_rx_block_ack_maximum_window(
        mut self,
        maximum_window: u16,
    ) -> Result<Self, StaRxBlockAckSessionsError> {
        self.rx_block_ack = StaRxBlockAckSessions::with_maximum_window(maximum_window)?;
        Ok(self)
    }

    /// Install the association-derived beacon-loss policy. This only arms a
    /// link-health deadline; modem sleep remains a separate explicit policy.
    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.beacon_monitor = Some(StaBeaconMonitor::new(config));
        self.beacon_lost = false;
    }

    /// Install the source-owned STA power-save policy. Construction remains
    /// opt-in; production HIL stays continuously awake until a platform sleep
    /// owner consumes the resulting permit.
    pub fn enable_power_save(&mut self, policy: StaPowerSavePolicy) {
        self.power_save = Some(StaPowerSavePlanner::new(policy));
        self.pending_doze_permit = None;
    }

    /// Queue the exact TID 0, 7, 5 negotiations initiated by the recovered
    /// vendor connection-complete path.
    pub fn queue_initial_tx_block_ack(&mut self) {
        self.initial_tx_block_ack.fill(true);
    }

    pub const fn rx_block_ack(&self) -> &StaRxBlockAckSessions {
        &self.rx_block_ack
    }

    pub const fn tx_block_ack(&self) -> &StaTxBlockAckSessions {
        &self.tx_block_ack
    }

    pub const fn last_event(&self) -> Option<ConnectedRxControlEvent> {
        self.last_event
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.last_tx_failure
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.last_expired_tid
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

    pub fn dropped_events(&self) -> u32 {
        self.receiver.dropped()
    }

    /// Remove every association-scoped control and hardware policy.
    ///
    /// The caller must first stop ISR/RX publication and wait for the staged
    /// protocol consumer to acknowledge its stop edge. This makes the event
    /// drain finite and prevents a late ADDBA command from entering the next
    /// association. The shared TX owner may still report an active hardware
    /// transaction separately; this method only revokes its BlockAck policy.
    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<ConnectedControlShutdown, ConnectedControlError>
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
        // No activation remains outside this owner after the transition
        // above. Reconstructing the fixed state discards unexecuted offers as
        // well as their association-specific dialog tokens.
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

        let mut discarded_events = 0_u8;
        while self.receiver.try_receive().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        self.beacon_monitor = None;
        self.beacon_lost = false;
        self.power_save = None;
        self.pending_doze_permit = None;

        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements,
            tx_block_ack_sessions,
            discarded_events,
            in_flight: in_flight_kind,
        })
    }

    fn has_immediate_work(&self) -> bool {
        self.in_flight.is_some()
            || self.receiver.len() != 0
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    fn next_alarm_deadline(&self) -> Option<u64> {
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

    /// Wait without consuming the event that made control work ready.
    pub fn wait_ready<'a, X>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a
    where
        X: ConnectedControlTx + 'a,
    {
        async move {
            if self.has_immediate_work() {
                return;
            }
            if let Some(deadline) = self.next_alarm_deadline() {
                match select(self.receiver.ready(), tx.wait_until_micros(deadline)).await {
                    Either::First(()) | Either::Second(()) => {}
                }
            } else {
                self.receiver.ready().await;
            }
        }
    }

    /// Execute at most one finite control transition.
    pub fn service<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
    ) -> impl Future<Output = Result<WifiControlProgress, ConnectedControlError>> + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        self.service_with_context(hardware, tx, WifiControlContext::IDLE)
    }

    /// Execute one finite transition using a coherent runner-owned view of
    /// work that has not yet entered the shared TX owner.
    pub fn service_with_context<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, ConnectedControlError>> + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        async move {
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.arm(tx.now_micros())?;
            }
            if let Some(in_flight) = self.in_flight.take() {
                let outcome = tx
                    .take_last_outcome()
                    .ok_or(ConnectedControlError::MissingTxOutcome)?;
                let success = outcome.is_success();
                if !success {
                    self.last_tx_failure = Some(ConnectedControlTxFailure {
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
                        self.publish_rx_reorder_command(RxReorderCommand::Stop {
                            tid: activation.negotiated().tid,
                        })?;
                        self.rx_block_ack.cancel(activation)?;
                    }
                    ControlInFlight::TxAddba { .. } if success => {}
                    ControlInFlight::TxAddba { tid } => {
                        self.tx_block_ack.stop(tid);
                        tx.set_tx_block_ack_operational(tid, false);
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

                        // A final PM=0 failure leaves the radio awake while
                        // the AP may still buffer for a sleeping station.
                        // Never release queued data into that split state.
                        if advertised == StaPowerManagement::Active && !success {
                            self.pending_doze_permit = None;
                            return Ok(WifiControlProgress::Disconnected);
                        }
                        if self.has_pending_traffic(context)
                            && self.power_save.as_ref().is_some_and(|planner| {
                                planner.state() == StaPowerSaveState::PowerSave
                            })
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
                return Ok(WifiControlProgress::More);
            }

            // RX is serviced before this control step. Consume its owned
            // event before applying an equal executor deadline, so a beacon
            // or ADDBA response received exactly on the boundary wins.
            if let Some(event) = self.receiver.try_receive() {
                self.last_event = Some(event);
                return self.apply_event(hardware, tx, event, context);
            }

            let now_micros = tx.now_micros();
            if let Some(tid) = self.tx_block_ack.expire_next(now_micros) {
                tx.set_tx_block_ack_operational(tid, false);
                self.last_expired_tid = Some(tid);
                return Ok(WifiControlProgress::More);
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
                self.publish_rx_reorder_command(RxReorderCommand::StopAll)?;
                for tid in STA_TX_BLOCK_ACK_TIDS {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_operational(tid, false);
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(tid, false)?;
                    }
                }
                self.beacon_lost = true;
                return Ok(WifiControlProgress::Disconnected);
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
                let tid = STA_TX_BLOCK_ACK_TIDS[index];
                return self.start_tx_addba(hardware, tx, tid);
            }

            Ok(WifiControlProgress::Idle)
        }
    }

    fn apply_event<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        event: ConnectedRxControlEvent,
        context: WifiControlContext,
    ) -> Result<WifiControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        if let ConnectedRxControlEvent::Beacon(observation) = event {
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.observe(tx.now_micros(), observation)?;
            }
            if self.power_save.is_some() {
                let traffic = if self.has_pending_traffic(context) {
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
            return Ok(WifiControlProgress::More);
        }
        let ConnectedRxControlEvent::BlockAck(action) = event else {
            return Ok(WifiControlProgress::More);
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
                    return Ok(WifiControlProgress::More);
                };
                self.start_rx_addba_response(hardware, tx, activation)
            }
            BlockAckAction::AddbaResponse { .. } => {
                let StaTxBlockAckResponse { tid, response } =
                    self.tx_block_ack.on_response_action(action)?;
                let operational = matches!(response, TxBlockAckResponse::Operational(_));
                tx.set_tx_block_ack_operational(tid, operational);
                if let TxBlockAckResponse::Operational(agreement) = response {
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(agreement.tid, true)?;
                    }
                } else if self.he_enabled {
                    hardware.set_he_tid_enabled(tid, false)?;
                }
                Ok(WifiControlProgress::More)
            }
            BlockAckAction::Delba { tid, initiator, .. } => {
                if initiator {
                    if let Some(agreement) = self.rx_block_ack.stop(tid) {
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                        self.publish_rx_reorder_command(RxReorderCommand::Stop { tid })?;
                    }
                } else {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_operational(tid, false);
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(tid, false)?;
                    }
                }
                Ok(WifiControlProgress::More)
            }
        }
    }

    fn has_pending_traffic(&self, context: WifiControlContext) -> bool {
        context.network_tx_pending
            || self.receiver.len() != 0
            || self.initial_tx_block_ack.into_iter().any(|pending| pending)
    }

    fn publish_rx_reorder_command(
        &self,
        command: RxReorderCommand,
    ) -> Result<(), RxReorderCommandError> {
        let Some(sender) = &self.rx_reorder_commands else {
            return Ok(());
        };
        try_send_rx_reorder_command(sender, command)
    }

    fn apply_power_save_decision<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        decision: StaPowerSaveDecision,
    ) -> Result<WifiControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match decision {
            StaPowerSaveDecision::PermitDoze(permit) => {
                self.pending_doze_permit = Some(permit);
                Ok(WifiControlProgress::More)
            }
            StaPowerSaveDecision::SendPowerManagement(power_management) => {
                self.pending_doze_permit = None;
                let progress = tx.start_power_management_null(hardware, power_management)?;
                self.in_flight = Some(ControlInFlight::PowerManagement(power_management));
                Ok(progress)
            }
            StaPowerSaveDecision::StayAwake(_) => {
                self.pending_doze_permit = None;
                Ok(WifiControlProgress::More)
            }
        }
    }

    fn start_rx_addba_response<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        activation: StaRxBlockAckActivation,
    ) -> Result<WifiControlProgress, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        if let Some(replaced) = activation.replaced() {
            if let Err(error) =
                self.publish_rx_reorder_command(RxReorderCommand::Stop { tid: replaced.tid })
            {
                hardware.clear_rx_block_ack(replaced.hardware_index)?;
                self.rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
            hardware.clear_rx_block_ack(replaced.hardware_index)?;
        }
        hardware.program_rx_block_ack(activation.hardware())?;
        let negotiated = activation.negotiated();
        if let Err(error) = self.publish_rx_reorder_command(RxReorderCommand::Start {
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
            self.publish_rx_reorder_command(RxReorderCommand::Stop {
                tid: activation.negotiated().tid,
            })?;
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::RxAddba(activation));
        Ok(WifiControlProgress::TxPending)
    }

    fn start_tx_addba<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        tid: u8,
    ) -> Result<WifiControlProgress, ConnectedControlError>
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
        Ok(WifiControlProgress::TxPending)
    }
}

impl<'resources, M, H, X, const CAPACITY: usize> Esp32s31ControlService<H, X>
    for Esp32s31ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx,
{
    type Error = ConnectedControlError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a {
        Esp32s31ConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        Esp32s31ConnectedControl::wait_ready(self, tx)
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
    };

    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_pac::{
        MacHeTxProgram, MacHtTxProgram, MacKeyInstallOutcome, MacLegacyTxProgram,
        MacTxCompletionRegisters,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        connected_rx::{ConnectedRxEvent, ConnectedRxSink},
        crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
        rx_ampdu_hw::S31RxBlockAckAgreement,
        tx::{LegacyRate, TxCompletion, TxSlot},
        tx_runtime::StaTxRuntimePolicy,
    };
    use open_esp_radio_ieee80211::mac_service::{MacRxMetadata, MacTxPlan};
    use open_esp_radio_ieee80211::station::StaTxSequenceCounters;
    use open_esp_radio_ieee80211::station_beacon::{StaBeaconObservation, StaTimObservation};
    use open_esp_radio_ieee80211::wmm::WmmAccessCategory;

    use crate::{
        runner::{WifiControlProgress, WifiTxProgress, WifiTxWake},
        rx_backend::ConnectedControlResources,
        rx_reorder::{RxReorderCommand, RxReorderCommandResources, try_receive_rx_reorder_command},
        single_mpdu_tx::{SingleMpduTxConfig, WifiTxPowerPair, WifiTxPowerProfile, WifiTxTimer},
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
        fn tx_descriptor_address(&self, _cpu_address: u32) -> u32 {
            0x2f00_1000
        }

        fn prepare_legacy_tx(&mut self, _queue: u8, _program: MacLegacyTxProgram) -> bool {
            self.prepare
        }

        fn start_legacy_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn prepare_ht_tx(&mut self, _queue: u8, _program: MacHtTxProgram) -> bool {
            self.prepare
        }

        fn start_ht_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn prepare_he_tx(&mut self, _queue: u8, _program: MacHeTxProgram) -> bool {
            false
        }

        fn start_he_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            self.completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            false
        }

        fn finish_tx_timeout_abort(&mut self, _queue: u8) -> Option<bool> {
            None
        }

        fn abort_tx_collision(&mut self, _queue: u8) -> bool {
            false
        }

        fn detach_completed_tx(&mut self, _queue: u8) -> bool {
            true
        }
    }

    impl ConnectedControlHardware for Hardware {
        fn station_tsf(&mut self) -> u64 {
            self.station_tsf
        }

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
            crate::single_mpdu_tx::WifiTxResources {
                slot,
                policy: StaTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy,
                timer: Timer::default(),
            },
            crate::single_mpdu_tx::ConnectedTxHandoff {
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
        control.queue_initial_tx_block_ack();
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

        for tid in STA_TX_BLOCK_ACK_TIDS {
            assert_eq!(
                embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
                Ok(WifiControlProgress::TxPending)
            );
            finish_tx(&mut hardware, &mut tx, 0);
            assert_eq!(
                embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
                Ok(WifiControlProgress::More)
            );
            assert!(control.tx_block_ack().alarm(tid).is_some());
        }
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::Idle)
        );
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
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
            Ok(WifiControlProgress::TxPending)
        );
        let agreement = hardware.programmed.unwrap();
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
            Some(RxReorderCommand::Start {
                tid: 3,
                starting_sequence: 0x123,
                window: 16,
            })
        );

        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
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
            Ok(WifiControlProgress::More)
        );
        assert_eq!(
            try_receive_rx_reorder_command(&reorder_receiver),
            Some(RxReorderCommand::Stop { tid: 3 })
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
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
            Ok(WifiControlProgress::TxPending)
        );
        let hardware_index = hardware.programmed.unwrap().hardware_index;
        assert!(matches!(
            try_receive_rx_reorder_command(&reorder_receiver),
            Some(RxReorderCommand::Start { tid: 3, .. })
        ));

        finish_tx(&mut hardware, &mut tx, 2);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
        );
        assert_eq!(hardware.cleared[0], Some(hardware_index));
        assert_eq!(
            try_receive_rx_reorder_command(&reorder_receiver),
            Some(RxReorderCommand::Stop { tid: 3 })
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
            if matches!(report.completion, Some(TxCompletion { status: 2, .. }))
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
        control.queue_initial_tx_block_ack();
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
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
            Ok(WifiControlProgress::More)
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
            Ok(WifiControlProgress::More)
        );
        assert_eq!(hardware.he_tid[1], Some((0, false)));
    }

    #[test]
    fn beacon_loss_deadline_disables_block_ack_and_disconnects() {
        let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
        let (_publisher, receiver) = resources.split();
        let mut control = Esp32s31ConnectedControl::new(
            receiver,
            BSSID,
            true,
            StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
        );
        control.enable_beacon_loss(StaBeaconLossConfig::new(100, 3).unwrap());
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::Idle)
        );
        embassy_futures::block_on(control.wait_ready(&mut tx));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::Disconnected)
        );
        assert!(control.beacon_lost());
        assert_eq!(
            hardware.he_tid[..3],
            [Some((0, false)), Some((7, false)), Some((5, false))]
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
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
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
        );

        control.queue_initial_tx_block_ack();
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
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
            Ok(WifiControlProgress::More)
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
            Ok(WifiControlProgress::Idle)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);

        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::Idle)
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
            Ok(WifiControlProgress::More)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
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
                WifiControlContext::IDLE,
            )),
            Ok(WifiControlProgress::TxPending)
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
                WifiControlContext::IDLE,
            )),
            Ok(WifiControlProgress::More)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
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
                WifiControlContext {
                    network_tx_pending: true,
                },
            )),
            Ok(WifiControlProgress::More)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            station_tsf: 1_000_100,
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        publisher.publish(beacon_event(idle_beacon()));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::TxPending)
        );

        finish_tx(&mut hardware, &mut tx, 5);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            station_tsf: 1_000_100,
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        publisher.publish(beacon_event(idle_beacon()));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        hardware.station_tsf = 1_001_000;
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
        );

        let pending = WifiControlContext {
            network_tx_pending: true,
        };
        assert_eq!(
            embassy_futures::block_on(control.service_with_context(
                &mut hardware,
                &mut tx,
                pending,
            )),
            Ok(WifiControlProgress::TxPending)
        );
        assert_eq!(
            control.power_save().unwrap().state(),
            StaPowerSaveState::AdvertisingActive
        );
        assert_eq!(control.take_doze_permit(), None);

        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service_with_context(
                &mut hardware,
                &mut tx,
                pending,
            )),
            Ok(WifiControlProgress::More)
        );
        assert_eq!(
            control.power_save().unwrap().state(),
            StaPowerSaveState::Awake
        );
        assert_eq!(
            embassy_futures::block_on(control.service_with_context(
                &mut hardware,
                &mut tx,
                pending,
            )),
            Ok(WifiControlProgress::Idle)
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
        let mut slot = core::pin::pin!(TxSlot::<512>::new());
        let mut hardware = Hardware {
            station_tsf: 1_000_100,
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 1);
        publisher.publish(beacon_event(idle_beacon()));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        hardware.station_tsf = 1_001_000;
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(WifiControlProgress::More)
        );

        let pending = WifiControlContext {
            network_tx_pending: true,
        };
        assert_eq!(
            embassy_futures::block_on(control.service_with_context(
                &mut hardware,
                &mut tx,
                pending,
            )),
            Ok(WifiControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 5);
        assert_eq!(
            embassy_futures::block_on(control.service_with_context(
                &mut hardware,
                &mut tx,
                pending,
            )),
            Ok(WifiControlProgress::Disconnected)
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
}
