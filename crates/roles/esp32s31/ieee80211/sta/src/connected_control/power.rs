//! Execution of the station power manager inside the connected control core.
//!
//! [`ModemSleep`] decides; this module performs its effects. Station TBTT,
//! TX-queue gating and beacon receive programming are MAC registers the
//! control core owns and are written at once. Coexistence requests, RF
//! sleep and wake and the release of held network frames belong to owners
//! outside the core; they are queued as [`ConnectedPowerCommand`]s for the
//! runtime, which drains them in order after each control step.

use oer_esp32s31_hal::types::{MacPti, StaTbttSchedule};

use oer_ieee80211_mac::station_power_save::StaPowerManagement;

use oer_esp32s31_ieee80211::datapath::{DatapathControlContext, DatapathControlProgress};

use crate::{
    hardware::control::ConnectedControlHardware,
    modem_sleep::{
        CoexPhaseView, CoexView, ModemSleep, PmAction, PmActions, PmBeacon, PmClock, PmCoexEvent,
        PmState, PmTimer, PmTraffic, SleepType,
    },
};

use super::{
    ConnectedControlCore, ConnectedControlError, ConnectedControlTx, ConnectedDisconnectReason,
    ControlInFlight, earliest_deadline,
};

/// Capacity of the queue of power commands waiting for the runtime.
pub const POWER_COMMAND_CAPACITY: usize = 16;

/// A power-management effect performed outside the control core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedPowerCommand {
    /// Request the air for a coexistence event.
    CoexRequest {
        event: PmCoexEvent,
        duration_micros: u32,
    },
    /// Withdraw a coexistence event request.
    CoexRelease(PmCoexEvent),
    /// Set the coexistence schedule interval, in 100 µs units.
    SetCoexInterval(u32),
    /// Restart the coexistence phases at phase 0.
    RestartCoexPhases,
    /// Set the coexistence flexible period.
    SetCoexFlexiblePeriod(u8),
    /// Put the station's RF to sleep, keeping its registration.
    RfSleep,
    /// Wake the station's RF.
    RfWake,
}

/// What the TX owner did with network data frames since control last read
/// it: `pm_tx_data_process` and `pm_tx_data_done_process` of the vendor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkTxPowerReport {
    /// A network data transaction started.
    pub started: bool,
    /// A network data transaction ended; `true` when the peer acknowledged
    /// at least one of its frames.
    pub completed: Option<bool>,
}

impl NetworkTxPowerReport {
    pub const fn is_empty(self) -> bool {
        !self.started && self.completed.is_none()
    }
}

/// Coexistence as the runtime reads it before a control step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerCoexSnapshot {
    pub view: CoexView,
    /// The receive priority of the beacon-window event.
    pub beacon_pti: MacPti,
}

const TIMERS: [PmTimer; 5] = [
    PmTimer::SliceEnd,
    PmTimer::Active,
    PmTimer::SleepDelay,
    PmTimer::Dream,
    PmTimer::Preemption,
];

const fn timer_index(timer: PmTimer) -> usize {
    match timer {
        PmTimer::SliceEnd => 0,
        PmTimer::Active => 1,
        PmTimer::SleepDelay => 2,
        PmTimer::Dream => 3,
        PmTimer::Preemption => 4,
    }
}

/// Power management of one association and the inputs waiting for it.
#[derive(Debug)]
pub(super) struct ConnectedPower {
    pub(super) engine: ModemSleep,
    /// The join beacon of an association whose power management has not
    /// started yet.
    start: Option<PmBeacon>,
    /// The coexistence snapshot of a started association.
    coex: Option<PowerCoexSnapshot>,
    deadlines: [Option<u64>; 5],
    tbtt: bool,
    phase: Option<CoexPhaseView>,
    preemption: Option<Option<u64>>,
    /// Nulls requested while another control frame owned the TX path.
    nulls: [Option<bool>; 2],
    commands: [Option<ConnectedPowerCommand>; POWER_COMMAND_CAPACITY],
    command_head: usize,
    command_len: usize,
    /// The MAC TX queues are blocked for power management.
    pub(super) tx_blocked: bool,
    /// A network frame waits for the station to leave power save.
    network_held: bool,
    /// The current network frame was offered to power management.
    network_offered: bool,
}

impl ConnectedPower {
    pub(super) const fn new() -> Self {
        Self {
            engine: ModemSleep::new(SleepType::None),
            start: None,
            coex: None,
            deadlines: [None; 5],
            tbtt: false,
            phase: None,
            preemption: None,
            nulls: [None; 2],
            commands: [None; POWER_COMMAND_CAPACITY],
            command_head: 0,
            command_len: 0,
            tx_blocked: false,
            network_held: false,
            network_offered: false,
        }
    }

    /// Start a new association in place, without a temporary of the whole
    /// state on the caller's stack.
    fn restart(&mut self, sleep_type: SleepType, join_beacon: PmBeacon, coex: PowerCoexSnapshot) {
        self.engine = ModemSleep::new(sleep_type);
        self.start = Some(join_beacon);
        self.coex = Some(coex);
        self.deadlines = [None; 5];
        self.tbtt = false;
        self.phase = None;
        self.preemption = None;
        self.nulls = [None; 2];
        self.commands.fill(None);
        self.command_head = 0;
        self.command_len = 0;
        self.tx_blocked = false;
        self.network_held = false;
        self.network_offered = false;
    }

    pub(super) fn coex_view(&self) -> CoexView {
        match self.coex {
            Some(coex) => coex.view,
            None => CoexView::INACTIVE,
        }
    }

    pub(super) fn has_input(&self) -> bool {
        self.start.is_some()
            || self.tbtt
            || self.phase.is_some()
            || self.preemption.is_some()
            || self.nulls[0].is_some()
    }

    pub(super) fn next_deadline(&self) -> Option<u64> {
        self.deadlines.iter().fold(None, |earliest, deadline| {
            earliest_deadline(earliest, *deadline)
        })
    }

    /// Whether frames other than power management's Null must wait: the
    /// TX queues are blocked outside the Wi-Fi slice, or the RF sleeps.
    pub(super) fn blocks_tx(&self) -> bool {
        self.tx_blocked || self.engine.state() == PmState::Dozing
    }

    pub(super) fn push_command(
        &mut self,
        command: ConnectedPowerCommand,
    ) -> Result<(), ConnectedControlError> {
        if self.command_len == POWER_COMMAND_CAPACITY {
            return Err(ConnectedControlError::PowerCommandOverflow);
        }
        let index = (self.command_head + self.command_len) % POWER_COMMAND_CAPACITY;
        self.commands[index] = Some(command);
        self.command_len += 1;
        Ok(())
    }

    fn take_command(&mut self) -> Option<ConnectedPowerCommand> {
        if self.command_len == 0 {
            return None;
        }
        let command = self.commands[self.command_head].take();
        self.command_head = (self.command_head + 1) % POWER_COMMAND_CAPACITY;
        self.command_len -= 1;
        command
    }

    fn take_expired_timer(&mut self, now_micros: u64) -> Option<PmTimer> {
        let (index, _) = self
            .deadlines
            .iter()
            .enumerate()
            .filter_map(|(index, deadline)| deadline.map(|deadline| (index, deadline)))
            .filter(|(_, deadline)| *deadline <= now_micros)
            .min_by_key(|(_, deadline)| *deadline)?;
        self.deadlines[index] = None;
        Some(TIMERS[index])
    }
}

impl ConnectedControlCore {
    /// Start power management for this association. `join_beacon` is the
    /// access point's last beacon or probe response before it; the next
    /// control step places the TBTT schedule from its timestamp.
    pub fn enable_power_management(
        &mut self,
        sleep_type: SleepType,
        join_beacon: PmBeacon,
        coex: PowerCoexSnapshot,
    ) {
        self.power.restart(sleep_type, join_beacon, coex);
    }

    /// The station's power management.
    pub const fn power_management(&self) -> &ModemSleep {
        &self.power.engine
    }

    /// Power management with its pending inputs, timers and commands, for
    /// diagnostics.
    pub fn power_debug(&self) -> &dyn core::fmt::Debug {
        &self.power
    }

    /// Whether data frames of the access point matter to power management:
    /// the station advertises power save.
    pub fn wants_power_save_data(&self) -> bool {
        self.power.engine.state() != PmState::Awake
    }

    /// Whether control frames other than power management's Null must wait
    /// for the station's next Wi-Fi slice.
    pub fn power_blocks_tx(&self) -> bool {
        self.power.blocks_tx()
    }

    /// Whether the datapath may publish a network frame now.
    pub fn admits_network_tx(&self) -> bool {
        !self.power.blocks_tx() && !self.power.network_held
    }

    /// Whether a queued network frame must be offered to power management
    /// before the datapath may publish it.
    pub fn network_tx_needs_offer(&self, context: DatapathControlContext) -> bool {
        self.power.engine.is_started()
            && context.network_tx_pending
            && !self.power.network_offered
            && self.power.engine.state() != PmState::Awake
    }

    /// Replace the coexistence snapshot the next power input reads.
    pub fn set_power_coex(&mut self, coex: PowerCoexSnapshot) {
        if self.power.coex.is_some() {
            self.power.coex = Some(coex);
        }
    }

    /// The station TBTT fired.
    pub fn observe_tbtt(&mut self) {
        self.power.tbtt = true;
    }

    /// The coexistence schedule entered a phase that notifies Wi-Fi.
    pub fn observe_coex_phase(&mut self, phase: CoexPhaseView) {
        self.power.phase = Some(phase);
    }

    /// Bluetooth reported when its isochronous preemption ends, or that no
    /// preemption runs.
    pub fn observe_preemption_end(&mut self, end_micros: Option<u64>) {
        self.power.preemption = Some(end_micros);
    }

    /// The next power command for the runtime, in order.
    pub fn take_power_command(&mut self) -> Option<ConnectedPowerCommand> {
        self.power.take_command()
    }

    pub(super) fn power_traffic(
        &self,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> PmTraffic {
        let control = self.in_flight.is_some()
            || control_event_pending
            || self.initial_tx_block_ack.into_iter().any(|pending| pending);
        PmTraffic {
            tx_pending: context.network_tx_pending || control,
            connection_pending: control,
        }
    }

    /// Perform at most one power input: the start, a queued Null, the TBTT,
    /// a coexistence phase, a preemption report, an expired timer or the
    /// offer of a queued network frame.
    pub(super) fn service_power<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> Result<Option<DatapathControlProgress<ConnectedDisconnectReason>>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let coex = self.power.coex_view();
        let traffic = self.power_traffic(context, control_event_pending);
        let clock = power_clock(tx);
        let mut actions = PmActions::new();
        if let Some(join_beacon) = self.power.start.take() {
            self.power.engine.start(join_beacon, coex, &mut actions);
        } else if let Some(power_save) = self.power.nulls[0] {
            self.power.nulls = [self.power.nulls[1], None];
            return self.start_null(hardware, tx, power_save).map(Some);
        } else if core::mem::take(&mut self.power.tbtt) {
            self.power.engine.tbtt(clock, coex, traffic, &mut actions);
        } else if let Some(phase) = self.power.phase.take() {
            self.power
                .engine
                .coex_phase(phase, clock, coex, &mut actions);
        } else if let Some(end) = self.power.preemption.take() {
            self.power
                .engine
                .preemption_end(end, clock, coex, traffic, &mut actions);
        } else if let Some(timer) = self.power.take_expired_timer(clock.now_micros) {
            match timer {
                PmTimer::SliceEnd => self
                    .power
                    .engine
                    .slice_end_timer(coex, traffic, &mut actions),
                PmTimer::Active => self.power.engine.active_timer(coex, traffic, &mut actions),
                PmTimer::SleepDelay => {
                    self.power
                        .engine
                        .sleep_delay_timer(clock, coex, traffic, &mut actions)
                }
                PmTimer::Dream => self.power.engine.dream_timer(&mut actions),
                PmTimer::Preemption => {
                    self.power
                        .engine
                        .preemption_timer(coex, traffic, &mut actions)
                }
            }
        } else if self.power.engine.is_started() && tx.has_network_tx_report() {
            let report = tx.take_network_tx_report();
            if report.started && !core::mem::take(&mut self.power.network_offered) {
                let _ = self
                    .power
                    .engine
                    .tx_data(true, clock, coex, traffic, &mut actions);
            }
            if let Some(acknowledged) = report.completed {
                self.power
                    .engine
                    .tx_data_done(acknowledged, clock, coex, traffic, &mut actions);
            }
        } else if self.network_tx_needs_offer(context) {
            self.power.network_offered = true;
            let hold = self
                .power
                .engine
                .tx_data(true, clock, coex, traffic, &mut actions);
            self.power.network_held = hold;
        } else {
            return Ok(None);
        }
        self.apply_power_actions(hardware, tx, actions)?;
        Ok(Some(self.start_queued_null(hardware, tx)?))
    }

    /// Start the first queued Null when the TX path is free.
    fn start_queued_null<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match self.power.nulls[0] {
            Some(power_save) if self.in_flight.is_none() => {
                self.power.nulls = [self.power.nulls[1], None];
                self.start_null(hardware, tx, power_save)
            }
            _ => Ok(DatapathControlProgress::More),
        }
    }

    fn start_null<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        power_save: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let power_management = if power_save {
            StaPowerManagement::PowerSave
        } else {
            StaPowerManagement::Active
        };
        let progress = tx.start_power_management_null(hardware, power_management)?;
        self.in_flight = Some(ControlInFlight::PowerManagement(power_management));
        Ok(progress)
    }

    /// Perform one input's effects in order.
    pub(super) fn apply_power_actions<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        actions: PmActions,
    ) -> Result<(), ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let now_micros = tx.now_micros();
        for action in actions.iter() {
            match action {
                PmAction::SendNull { power_save } => {
                    let slot = self
                        .power
                        .nulls
                        .iter_mut()
                        .find(|slot| slot.is_none())
                        .ok_or(ConnectedControlError::PowerCommandOverflow)?;
                    *slot = Some(power_save);
                }
                PmAction::CoexRequest {
                    event,
                    duration_micros,
                } => self
                    .power
                    .push_command(ConnectedPowerCommand::CoexRequest {
                        event,
                        duration_micros,
                    })?,
                PmAction::CoexRelease(event) => self
                    .power
                    .push_command(ConnectedPowerCommand::CoexRelease(event))?,
                PmAction::SetCoexInterval(interval) => self
                    .power
                    .push_command(ConnectedPowerCommand::SetCoexInterval(interval))?,
                PmAction::RestartCoexPhases => self
                    .power
                    .push_command(ConnectedPowerCommand::RestartCoexPhases)?,
                PmAction::SetCoexFlexiblePeriod(period) => self
                    .power
                    .push_command(ConnectedPowerCommand::SetCoexFlexiblePeriod(period))?,
                PmAction::RfSleep => self.power.push_command(ConnectedPowerCommand::RfSleep)?,
                PmAction::RfWake => self.power.push_command(ConnectedPowerCommand::RfWake)?,
                PmAction::BlockTx => {
                    self.power.tx_blocked = true;
                    hardware.set_power_save_tx_block(true);
                }
                PmAction::UnblockTx => {
                    self.power.tx_blocked = false;
                    hardware.set_power_save_tx_block(false);
                }
                PmAction::Arm {
                    timer,
                    after_micros,
                } => {
                    self.power.deadlines[timer_index(timer)] =
                        Some(now_micros.saturating_add(after_micros));
                }
                PmAction::Disarm(timer) => self.power.deadlines[timer_index(timer)] = None,
                PmAction::ReleaseHeldFrames => self.power.network_held = false,
                PmAction::StartTbtt(schedule) => hardware.start_station_tbtt(StaTbttSchedule {
                    first_tbtt_tsf: schedule.first_tbtt_tsf,
                    interval_micros: schedule.interval_micros,
                    ahead_micros: schedule.ahead_micros,
                    wake_ahead_micros: schedule.wake_ahead_micros,
                }),
                PmAction::SetTbttInterval(interval) => {
                    hardware.set_station_tbtt_interval(interval)?
                }
                PmAction::StopTbtt => hardware.stop_station_tbtt(),
                PmAction::SetTbttAhead {
                    ahead_micros,
                    wake_ahead_micros,
                } => hardware.set_station_tbtt_ahead(ahead_micros, wake_ahead_micros),
                PmAction::RxBeaconPriority(true) => hardware.set_rx_beacon_pti(
                    self.power
                        .coex
                        .expect("power management starts with a coexistence snapshot")
                        .beacon_pti,
                ),
                PmAction::RxBeaconPriority(false) => hardware.set_rx_beacon_pti(
                    MacPti::new(0).expect("priority zero is a valid MAC priority"),
                ),
                PmAction::ClearRxBeaconPriority => hardware.clear_rx_beacon_pti(),
                PmAction::RxBeaconTime {
                    window_micros,
                    time_micros,
                    // The register holds the window's low 16 bits, as the
                    // vendor writes it.
                } => hardware.set_rx_beacon_time(window_micros as u16, time_micros),
            }
        }
        Ok(())
    }

    /// Forget every pending power input after the association stopped.
    pub(super) fn clear_power_inputs(&mut self) {
        let engine = self.power.engine.clone();
        let mut commands = ConnectedPower::new();
        while let Some(command) = self.power.take_command() {
            // The runtime still performs the stop's releases and RF wake.
            let _ = commands.push_command(command);
        }
        self.power = commands;
        self.power.engine = engine;
    }
}

pub(super) fn power_clock<X: ConnectedControlTx>(tx: &X) -> PmClock {
    PmClock {
        now_micros: tx.now_micros(),
    }
}
