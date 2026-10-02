//! Vendor station power management for one associated station.
//!
//! This is the connected-station part of the vendor power manager
//! (`libpp.a[pm.o]`, `pm_coex.o`) as an executor-independent state machine.
//! Every input is one vendor entry point; every effect is a [`PmAction`] the
//! caller performs in order: frames to send, coexistence requests, TX queue
//! gating, RF sleep and wake, timers and the station TBTT schedule.
//!
//! Under active coexistence a station follows the schedule's Wi-Fi slices
//! even when power save is off: it advertises power save and blocks its TX
//! queues when its slice ends, wakes at its next slice or beacon, and
//! requests the air for its slice, its beacon window and buffered traffic.
//! Without coexistence, sleep type `None` keeps the station awake and the
//! modem sleep types follow the DTIM or the listen interval.
//!
//! Only an associated station is modelled. The vendor's disconnected power
//! save, connectionless wake windows, TWT, mesh and hardware beacon
//! monitoring are separate features and absent here.
//!
//! SOURCE: complete pinned `libpp.a[pm.o]` and `libpp.a[pm_coex.o]` of
//! espressif/esp32-wifi-lib `af55a0ca`; each method names the function it
//! follows.

use oer_ieee80211_trace::{PowerState, PowerStateTrace};

/// Beacon receive window requested at each TBTT (`g_pm_cfg[20]`).
pub const BEACON_WINDOW_MICROS: u32 = 25_000;
/// Beacon receive window while a Bluetooth isochronous preemption runs.
pub const PREEMPTED_BEACON_WINDOW_MICROS: u32 = 15_000;
/// Second beacon-time argument (`g_pm_cfg[24]`).
pub const BEACON_TIME_MICROS: u32 = 10_000;
/// Second beacon-time argument while a preemption runs.
pub const PREEMPTED_BEACON_TIME_MICROS: u32 = 5_000;
/// Sleep delay after the last activity, and the margin before a slice end
/// at which a power-save station leaves the air (`g_pm_cfg[36]`).
pub const SLEEP_DELAY_MICROS: u32 = 5_000;
/// Slice-end margin of a station that is not in power save.
pub const IDLE_SLICE_MARGIN_MICROS: u32 = 1_000;
/// Idle time after which an awake modem-sleep station sleeps again
/// (`g_pm_cfg[32]`).
pub const ACTIVE_TIMEOUT_MICROS: u32 = 50_000;
/// Time a station keeps receiving buffered group traffic (`g_pm_cfg[72]`).
pub const DREAM_TIMEOUT_MICROS: u32 = 15_000;
/// TBTT event lead without coexistence (`g_pm_cfg[8]`).
pub const TBTT_AHEAD_MICROS: u16 = 3_000;
/// TBTT event lead while another radio shares the air (`g_pm_cfg[12]`).
pub const SHARED_TBTT_AHEAD_MICROS: u16 = 3_500;
/// Wake lead added to the TBTT lead (`g_pm_cfg[16]`).
pub const TBTT_WAKE_WINDOW_MICROS: u16 = 1_500;
/// Minimum slice time left for a unicast wake or a TX wake.
pub const WAKE_SLICE_THRESHOLD_MICROS: u32 = 5_000;
/// Minimum slice time left for a group-traffic wake.
pub const GROUP_SLICE_THRESHOLD_MICROS: u32 = 1_000;
/// Minimum slice time left to keep the slice request.
pub const SLICE_REQUEST_THRESHOLD_MICROS: u32 = 5_000;
/// Consecutive active Null failures after which the station stops retrying.
pub const NULL_RETRY_LIMIT: u8 = 15;
/// Beacon interval assumed until the access point advertises one.
pub const DEFAULT_BEACON_INTERVAL_MICROS: u32 = 0x19_000;
/// Coexistence schedule interval unit, in microseconds.
const SCHEDULE_INTERVAL_UNIT_MICROS: u32 = 100;

/// Coexistence events Wi-Fi requests from power management.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmCoexEvent {
    /// Event 0: the beacon receive window at a TBTT.
    BeaconWindow,
    /// Event 1: the Wi-Fi slice, or awake time inside it.
    Slice,
    /// Event 2: buffered group traffic after a DTIM.
    GroupTraffic,
}

impl PmCoexEvent {
    /// The vendor event number.
    pub const fn id(self) -> u8 {
        match self {
            Self::BeaconWindow => 0,
            Self::Slice => 1,
            Self::GroupTraffic => 2,
        }
    }
}

/// The station's configured power-save type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepType {
    /// No power save: the station sleeps only for coexistence.
    None,
    /// Modem sleep waking for every DTIM.
    MinModem,
    /// Modem sleep waking at the listen interval.
    MaxModem,
}

impl From<crate::request::StationPowerMode> for SleepType {
    fn from(mode: crate::request::StationPowerMode) -> Self {
        use crate::request::StationPowerMode;
        match mode {
            StationPowerMode::None => Self::None,
            StationPowerMode::MinModem => Self::MinModem,
            StationPowerMode::MaxModem(_) => Self::MaxModem,
        }
    }
}

/// Power-management state (`g_pm[1]`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmState {
    /// Awake with power save not advertised.
    Awake,
    /// Power save advertised; the modem is still awake.
    PowerSave,
    /// Power save advertised and the station's RF asleep.
    Dozing,
}

impl PmState {
    const fn traced(self) -> PowerState {
        match self {
            Self::Awake => PowerState::Awake,
            Self::PowerSave => PowerState::PowerSave,
            Self::Dozing => PowerState::Dozing,
        }
    }
}

/// Timers the power manager arms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmTimer {
    /// End of the Wi-Fi slice (`g_pm+240`).
    SliceEnd,
    /// Idle time before an awake modem-sleep station sleeps (`g_pm+200`).
    Active,
    /// Delay before a power-save station dozes (`g_pm+260`).
    SleepDelay,
    /// Time to keep receiving buffered group traffic (`g_pm+180`).
    Dream,
    /// End of a Bluetooth isochronous preemption (`g_pm+1140`).
    Preemption,
}

/// One station TBTT schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTbttSchedule {
    pub first_tbtt_tsf: u64,
    pub interval_micros: u32,
    pub ahead_micros: u16,
    pub wake_ahead_micros: u16,
}

/// One effect of the power manager, performed in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmAction {
    /// Send a Null Data frame advertising (`true`) or leaving power save.
    SendNull { power_save: bool },
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
    /// Block the MAC TX queues, as the slice end does.
    BlockTx,
    /// Unblock the MAC TX queues.
    UnblockTx,
    /// Put the station's RF to sleep, keeping its registration.
    RfSleep,
    /// Wake the station's RF.
    RfWake,
    /// Arm a timer to expire after the given time.
    Arm { timer: PmTimer, after_micros: u64 },
    /// Disarm a timer.
    Disarm(PmTimer),
    /// Transmit the frames held while the station was in power save.
    ReleaseHeldFrames,
    /// Program the station TBTT schedule.
    StartTbtt(PmTbttSchedule),
    /// Replace the TBTT interval.
    SetTbttInterval(u32),
    /// Stop the station TBTT.
    StopTbtt,
    /// Replace the TBTT lead and the wake lead beside it.
    SetTbttAhead {
        ahead_micros: u16,
        wake_ahead_micros: u16,
    },
    /// Publish the beacon-window event's priority as the beacon receive
    /// priority (`true`), or priority zero (`false`).
    ///
    /// SOURCE(esp32s31): complete pinned `libpp.a[pm_coex.o]::
    /// pm_coex_update_rx_beacon_pti` passes `coex_pti_get(0)`, or zero, as
    /// both arguments of `hal_set_rx_beacon_pti`. It selects event 1 only
    /// under the hardware beacon monitor, which this model leaves out.
    RxBeaconPriority(bool),
    /// Clear the beacon receive priority after a beacon.
    ClearRxBeaconPriority,
    /// Set the hardware beacon receive time.
    RxBeaconTime {
        window_micros: u32,
        time_micros: u32,
    },
}

/// Capacity of one input's effect list.
pub const PM_ACTION_CAPACITY: usize = 24;

/// The ordered effects of one input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmActions {
    items: [Option<PmAction>; PM_ACTION_CAPACITY],
    len: usize,
}

impl Default for PmActions {
    fn default() -> Self {
        Self::new()
    }
}

impl PmActions {
    pub const fn new() -> Self {
        Self {
            items: [None; PM_ACTION_CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, action: PmAction) {
        assert!(
            self.len < PM_ACTION_CAPACITY,
            "one power-management input exceeds its effect capacity"
        );
        self.items[self.len] = Some(action);
        self.len += 1;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = PmAction> + '_ {
        self.items[..self.len].iter().filter_map(|action| *action)
    }
}

/// Coexistence as Wi-Fi reads it at one instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexView {
    /// `coex_status_get` for Wi-Fi: coexistence started and another radio
    /// publishes status.
    pub active: bool,
    /// `coex_schm_curr_period_get`.
    pub current_period: u8,
    /// `coex_schm_flexible_period_get`.
    pub flexible_period: u8,
    /// `coex_schm_interval_get`, in 100 µs units.
    pub interval: u32,
    /// Wi-Fi share of phase 0, `coex_schm_get_phase_by_idx(0)`.
    pub phase0_share_percent: u8,
}

impl CoexView {
    /// Coexistence inactive.
    pub const INACTIVE: Self = Self {
        active: false,
        current_period: 0,
        flexible_period: 0,
        interval: 0,
        phase0_share_percent: 0,
    };

    /// `pm_coex_schm_overall_period_get`: the larger of the current and the
    /// flexible period.
    pub const fn overall_period(self) -> u8 {
        if self.current_period < self.flexible_period {
            self.flexible_period
        } else {
            self.current_period
        }
    }

    /// One schedule cycle, in microseconds.
    const fn cycle_micros(self) -> u64 {
        self.overall_period() as u64 * self.interval as u64 * SCHEDULE_INTERVAL_UNIT_MICROS as u64
    }
}

/// One coexistence phase as Wi-Fi's phase callback reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhaseView {
    /// The phase's share of the period, in percent.
    pub share_percent: u8,
    /// The phase's Wi-Fi flags.
    pub wifi: u8,
}

impl CoexPhaseView {
    /// Wi-Fi owns this phase: it requests its slice and may transmit.
    pub const WIFI_SLICE: u8 = 0x02;
    /// This phase ends a Wi-Fi slice: arm the slice-end timer.
    pub const SLICE_END: u8 = 0x01;
    /// This phase lends the air to Wi-Fi without its own slice.
    pub const SHARED: u8 = 0x04;
}

/// The beacon a station received from its access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmBeacon {
    /// The beacon's timestamp in the station TSF.
    pub timestamp_tsf: u64,
    /// The advertised beacon interval, in time units.
    pub interval_tu: u16,
    /// The TIM of the beacon, when present.
    pub tim: Option<PmTim>,
}

/// The part of a TIM power management reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTim {
    pub dtim_count: u8,
    pub dtim_period: u8,
    /// The access point buffers individually addressed traffic for us.
    pub unicast: bool,
    /// The access point buffers group traffic after this DTIM.
    pub group: bool,
}

/// The Null in-flight flags before a [`PmAction::SendNull`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmNullFlags {
    active_in_flight: bool,
    power_save_in_flight: bool,
}

/// Outcome of `pm_go_to_wake`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WakeResult {
    NotStarted,
    /// The wake waits for the next slice (vendor return 1).
    Deferred,
    /// The station is awake (vendor return 2).
    Awake,
    /// A Null leaving power save is in flight (vendor return 3).
    Waking,
}

/// The clock one power-management input reads.
///
/// The vendor keeps timer deadlines in `esp_timer_get_time` and measures the
/// station's position in the schedule cycle between two readings of the
/// free-running MAC microsecond counter at `0x2010_d800`. Only differences
/// of that counter matter, so one monotonic microsecond clock serves both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmClock {
    /// Monotonic time, in microseconds.
    pub now_micros: u64,
}

/// What the TX path reports when power management asks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTraffic {
    /// `ppCheckTxIdle` is nonzero: a frame waits in a TX queue.
    pub tx_pending: bool,
    /// `ppCheckTxConnTrafficIdle` is nonzero: connection traffic waits.
    pub connection_pending: bool,
}

impl PmTraffic {
    pub const IDLE: Self = Self {
        tx_pending: false,
        connection_pending: false,
    };
}

/// Power management of one associated station (`g_pm`).
#[derive(Clone, Debug, Eq, PartialEq)]
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-legacy-station-power-save, coex-coexistence-policy-and-scheduler-tbtt-anchored-periods
pub struct ModemSleep {
    sleep_type: SleepType,
    /// `[14]`: power management runs for an associated station.
    started: bool,
    /// `[1]`
    state: PmState,
    /// `[26]`: the station is inside its Wi-Fi slice.
    in_slice: bool,
    /// `[27]`: a wake waits for the next Wi-Fi slice or TBTT.
    wake_deferred: bool,
    /// `[21]`: a Null leaving power save is in flight.
    active_null_in_flight: bool,
    /// `[22]`: a Null entering power save is in flight.
    power_save_null_in_flight: bool,
    /// `[19]`: a beacon is expected since the last TBTT.
    beacon_expected: bool,
    /// `[1137]`: a beacon was still expected when the last slice ended.
    beacon_expected_at_slice_end: bool,
    /// `[20]`: buffered traffic is being received.
    receiving: bool,
    /// `[9]`: failed Nulls leaving power save.
    active_null_failures: u8,
    /// `[10]`: failed Nulls entering power save.
    power_save_null_failures: u8,
    /// `[18]`: program the TBTT schedule at the next beacon.
    update_tbtt_at_next_beacon: bool,
    /// `[17]`: a beacon has been parsed since the start.
    beacon_parsed: bool,
    /// `[52]`
    beacon_interval_micros: u32,
    /// `[11]`
    dtim_period: u8,
    /// `[12]`: the overall period the TBTT interval was last derived from.
    tbtt_period: u8,
    /// `[112]`: end of the Wi-Fi slice, monotonic.
    slice_end: u64,
    /// `[120]`: when the station leaves the slice, monotonic.
    slice_deadline: u64,
    /// `[432]`: rest of the cycle after the Wi-Fi slice.
    cycle_remainder: u64,
    /// `[96]`: time of the TBTT the cycle position counts from.
    cycle_anchor: u64,
    /// `[440]`: the sleep-delay timer is armed.
    sleep_delay_armed: bool,
    /// `[292]`: outstanding explicit wake holds.
    wake_holds: u32,
    /// `[76]`: activity since the last TBTT.
    activity: u32,
    /// `[1139]`: a Bluetooth isochronous preemption runs.
    preempted: bool,
    /// `[1160]`: when the preemption ends, monotonic.
    preemption_end: u64,
    /// `[60]`: the beacon window requested at each TBTT.
    beacon_window_micros: u32,
    /// `[56]`: the active timeout.
    active_timeout_micros: u32,
    /// `[44]`: the TBTT lead.
    tbtt_ahead_micros: u16,
    /// `[48]`: the wake window added to the TBTT lead.
    tbtt_window_micros: u16,
    /// `[13]`: coexistence activity at the last parameter update.
    coex_was_active: bool,
}

impl ModemSleep {
    /// Stopped power management with the vendor attach defaults.
    ///
    /// SOURCE(esp32s31): `pm_attach`.
    pub const fn new(sleep_type: SleepType) -> Self {
        Self {
            sleep_type,
            started: false,
            state: PmState::Awake,
            in_slice: true,
            wake_deferred: false,
            active_null_in_flight: false,
            power_save_null_in_flight: false,
            beacon_expected: false,
            beacon_expected_at_slice_end: false,
            receiving: false,
            active_null_failures: 0,
            power_save_null_failures: 0,
            update_tbtt_at_next_beacon: false,
            beacon_parsed: false,
            beacon_interval_micros: DEFAULT_BEACON_INTERVAL_MICROS,
            dtim_period: 1,
            tbtt_period: 0,
            slice_end: 0,
            slice_deadline: 0,
            cycle_remainder: 0,
            cycle_anchor: 0,
            sleep_delay_armed: false,
            wake_holds: 0,
            activity: 0,
            preempted: false,
            preemption_end: 0,
            beacon_window_micros: BEACON_WINDOW_MICROS,
            active_timeout_micros: ACTIVE_TIMEOUT_MICROS,
            tbtt_ahead_micros: TBTT_AHEAD_MICROS,
            tbtt_window_micros: TBTT_WAKE_WINDOW_MICROS,
            coex_was_active: false,
        }
    }

    pub const fn state(&self) -> PmState {
        self.state
    }

    /// Move to `state`, recording an actual change.
    fn enter(&mut self, state: PmState) {
        if self.state != state {
            oer_trace::emit(&PowerStateTrace {
                from: self.state.traced(),
                to: state.traced(),
            });
        }
        self.state = state;
    }

    pub const fn is_started(&self) -> bool {
        self.started
    }

    pub const fn in_slice(&self) -> bool {
        self.in_slice
    }

    pub const fn beacon_interval_micros(&self) -> u32 {
        self.beacon_interval_micros
    }

    /// `pm_check_state`: whether the station may leave the air.
    ///
    /// Without coexistence a station with sleep type `None` stays awake.
    /// Under coexistence it sleeps even then.
    fn may_sleep(&mut self, coex: CoexView, actions: &mut PmActions) -> bool {
        if self.sleep_type == SleepType::None && !coex.active {
            if self.state != PmState::Awake {
                self.dream(coex, None, actions);
                self.enter(PmState::Awake);
            }
            return false;
        }
        if !self.started && self.state != PmState::Awake {
            self.dream(coex, None, actions);
            self.enter(PmState::Awake);
        }
        self.started
    }

    const fn is_sleeping(&self) -> bool {
        matches!(self.state, PmState::Dozing)
    }

    fn incr_activity(&mut self) {
        self.activity = self.activity.wrapping_add(1);
        if self.activity == 0 {
            self.activity = 1;
        }
    }

    /// `pm_start`: an association completed. `join_beacon` is the access
    /// point's last beacon or probe response before the association; its
    /// timestamp places the first TBTT schedule, as the vendor passes its
    /// node timestamp without a TIM.
    pub fn start(&mut self, join_beacon: PmBeacon, coex: CoexView, actions: &mut PmActions) {
        if self.started {
            return;
        }
        self.active_null_in_flight = false;
        self.power_save_null_in_flight = false;
        self.activity = 0;
        self.beacon_interval_micros = if join_beacon.interval_tu == 0 {
            DEFAULT_BEACON_INTERVAL_MICROS
        } else {
            u32::from(join_beacon.interval_tu) << 10
        };
        if coex.active {
            actions.push(PmAction::SetCoexInterval(
                self.beacon_interval_micros / SCHEDULE_INTERVAL_UNIT_MICROS,
            ));
        }
        self.coex_pwr_update(coex, actions);
        self.coex_was_active = coex.active;
        self.update_tbtt_at_next_beacon = true;
        self.cycle_anchor = 0;
        self.update_next_tbtt(
            PmBeacon {
                tim: None,
                ..join_beacon
            },
            coex,
            actions,
        );
        actions.push(PmAction::RxBeaconPriority(true));
        actions.push(PmAction::RxBeaconTime {
            window_micros: self.beacon_window_micros,
            time_micros: BEACON_TIME_MICROS,
        });
        self.enable_active_timer(actions);
        self.started = true;
        self.preempted = false;
    }

    /// `pm_stop`: the association ends.
    pub fn stop(&mut self, coex: CoexView, actions: &mut PmActions) {
        if !self.started {
            return;
        }
        actions.push(PmAction::UnblockTx);
        self.disable_sleep_delay_timer(actions);
        actions.push(PmAction::Disarm(PmTimer::Active));
        actions.push(PmAction::StopTbtt);
        actions.push(PmAction::RxBeaconPriority(false));
        actions.push(PmAction::Disarm(PmTimer::Dream));
        if self.state != PmState::Awake {
            self.dream(coex, None, actions);
            self.enter(PmState::Awake);
            actions.push(PmAction::ReleaseHeldFrames);
        }
        self.dtim_period = 1;
        self.in_slice = true;
        self.wake_deferred = false;
        self.started = false;
        self.beacon_parsed = false;
        actions.push(PmAction::Disarm(PmTimer::SliceEnd));
        self.coex_pwr_update(coex, actions);
    }

    /// `pm_coex_pwr_update` with the vendor default coexistence power
    /// configuration off: the flexible period is one.
    fn coex_pwr_update(&mut self, coex: CoexView, actions: &mut PmActions) {
        actions.push(PmAction::SetCoexFlexiblePeriod(1));
        let coex = CoexView {
            flexible_period: 1,
            ..coex
        };
        self.tbtt_period = coex.overall_period();
        if self.started && coex.active {
            self.update_tbtt_at_next_beacon = true;
        }
    }

    fn enable_active_timer(&mut self, actions: &mut PmActions) {
        actions.push(PmAction::Disarm(PmTimer::Active));
        if self.sleep_type != SleepType::None {
            actions.push(PmAction::Arm {
                timer: PmTimer::Active,
                after_micros: u64::from(self.active_timeout_micros),
            });
        }
    }

    fn enable_sleep_delay_timer(&mut self, actions: &mut PmActions) {
        actions.push(PmAction::Disarm(PmTimer::SleepDelay));
        actions.push(PmAction::Arm {
            timer: PmTimer::SleepDelay,
            after_micros: u64::from(SLEEP_DELAY_MICROS),
        });
        self.sleep_delay_armed = true;
    }

    fn disable_sleep_delay_timer(&mut self, actions: &mut PmActions) {
        actions.push(PmAction::Disarm(PmTimer::SleepDelay));
        self.sleep_delay_armed = false;
    }

    fn enable_dream_timer(&mut self, actions: &mut PmActions) {
        actions.push(PmAction::Disarm(PmTimer::Dream));
        actions.push(PmAction::Arm {
            timer: PmTimer::Dream,
            after_micros: u64::from(DREAM_TIMEOUT_MICROS),
        });
    }

    /// `pm_dream`: wake a dozing station's RF and return to advertised power
    /// save; the slice-end timer is re-armed when still inside the slice.
    fn dream(&mut self, _coex: CoexView, _clock: Option<PmClock>, actions: &mut PmActions) {
        if self.state == PmState::Dozing {
            actions.push(PmAction::RfWake);
        }
        self.enter(PmState::PowerSave);
    }

    /// `pm_coex_recalculate_wifi_time_slice`: place the station in the cycle
    /// counted from the TBTT anchor and re-arm the slice-end timer.
    fn recalculate_slice(
        &mut self,
        clock: PmClock,
        force: bool,
        coex: CoexView,
        actions: &mut PmActions,
    ) {
        if !coex.active {
            return;
        }
        let now = clock.now_micros;
        if !force && now < self.slice_end.wrapping_add(self.cycle_remainder) {
            return;
        }
        let cycle = coex.cycle_micros();
        let offset = if cycle == 0 {
            0
        } else {
            clock.now_micros.wrapping_sub(self.cycle_anchor) % cycle
        };
        let slice = u64::from(coex.phase0_share_percent)
            * u64::from(coex.interval)
            * u64::from(coex.current_period);
        self.slice_end = now.wrapping_sub(offset).wrapping_add(slice);
        self.slice_deadline = self.slice_end.wrapping_sub(if self.started {
            u64::from(SLEEP_DELAY_MICROS)
        } else {
            u64::from(IDLE_SLICE_MARGIN_MICROS)
        });
        self.cycle_remainder = cycle.wrapping_sub(slice);
        actions.push(PmAction::Disarm(PmTimer::SliceEnd));
        if now < self.slice_deadline {
            self.in_slice = true;
            actions.push(PmAction::Arm {
                timer: PmTimer::SliceEnd,
                after_micros: self.slice_deadline - now,
            });
        } else {
            self.in_slice = false;
        }
    }

    /// `pm_is_in_wifi_slice_threshold`: whether at least `threshold` of the
    /// Wi-Fi slice remains, and of the preemption gap when one runs.
    fn in_slice_threshold(
        &mut self,
        clock: PmClock,
        threshold: u32,
        coex: CoexView,
        actions: &mut PmActions,
    ) -> bool {
        self.recalculate_slice(clock, false, coex, actions);
        if !coex.active {
            return true;
        }
        let now = clock.now_micros;
        let threshold = u64::from(threshold);
        if now >= self.slice_deadline || self.slice_deadline - now < threshold {
            return false;
        }
        if self.preempted && (now >= self.preemption_end || self.preemption_end - now < threshold) {
            return false;
        }
        true
    }

    /// `pm_send_nullfunc`: send a Null advertising `power_save`. The caller
    /// reports a frame it could not queue with [`Self::null_not_sent`].
    fn send_null(&mut self, power_save: bool, actions: &mut PmActions) {
        self.power_save_null_in_flight = power_save;
        self.active_null_in_flight = !power_save;
        actions.push(PmAction::SendNull { power_save });
    }

    /// The Null of the last [`PmAction::SendNull`] could not be queued, so
    /// the in-flight flags return to their state before it, as the vendor
    /// does when its Null callback fails.
    pub fn null_not_sent(&mut self, power_save: bool, before: PmNullFlags) {
        let _ = power_save;
        self.active_null_in_flight = before.active_in_flight;
        self.power_save_null_in_flight = before.power_save_in_flight;
    }

    /// The Null in-flight flags, to restore with [`Self::null_not_sent`].
    pub const fn null_flags(&self) -> PmNullFlags {
        PmNullFlags {
            active_in_flight: self.active_null_in_flight,
            power_save_in_flight: self.power_save_null_in_flight,
        }
    }

    /// `pm_go_to_sleep`: advertise power save.
    fn go_to_sleep(&mut self, coex: CoexView, actions: &mut PmActions) {
        if !self.may_sleep(coex, actions) || self.state == PmState::Dozing {
            return;
        }
        self.enter(PmState::PowerSave);
        if !self.power_save_null_in_flight {
            self.send_null(true, actions);
        }
    }

    /// `pm_go_to_wake`: leave power save when the slice allows it.
    ///
    /// Pending traffic wakes the station at once: its data frames carry
    /// power management zero. Otherwise, or in addition while enough of the
    /// slice remains, a Null leaving power save is sent. With `tx_wake` the
    /// caller is the TX path and the station always becomes awake.
    fn go_to_wake(
        &mut self,
        tx_wake: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) -> WakeResult {
        if !self.started {
            return WakeResult::NotStarted;
        }
        if !self.in_slice {
            self.wake_deferred = true;
            return WakeResult::Deferred;
        }
        if self.state == PmState::Dozing {
            self.dream(coex, Some(clock), actions);
        }
        if self.active_null_in_flight || tx_wake {
            self.active_null_in_flight = true;
            self.null_done(false, true, clock, coex, traffic, actions);
            return WakeResult::Awake;
        }
        let pending = traffic.connection_pending;
        if pending {
            self.active_null_in_flight = true;
            self.null_done(false, true, clock, coex, traffic, actions);
        }
        if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, actions) {
            self.send_null(false, actions);
            return WakeResult::Waking;
        }
        if pending {
            return WakeResult::Awake;
        }
        self.wake_deferred = true;
        WakeResult::Deferred
    }

    /// `pm_sleep`: doze when nothing keeps the station awake.
    fn sleep(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        let quiet = self.beacon_parsed
            && !self.beacon_expected
            && !self.receiving
            && !self.active_null_in_flight
            && !self.power_save_null_in_flight
            && self.started
            && self.wake_holds == 0;
        if !quiet {
            return;
        }
        let may = !traffic.tx_pending
            || (coex.active
                && (!self.in_slice || (self.preempted && clock.now_micros >= self.preemption_end)));
        if !may {
            return;
        }
        for event in [
            PmCoexEvent::Slice,
            PmCoexEvent::GroupTraffic,
            PmCoexEvent::BeaconWindow,
        ] {
            actions.push(PmAction::CoexRelease(event));
        }
        actions.push(PmAction::ClearRxBeaconPriority);
        actions.push(PmAction::RfSleep);
        self.enter(PmState::Dozing);
    }

    /// `pm_tx_null_data_done_process`: a Null completed.
    pub fn null_done(
        &mut self,
        power_save: bool,
        acknowledged: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        let succeeded =
            acknowledged || (!power_save && self.active_null_failures > NULL_RETRY_LIMIT);
        if !self.may_sleep(coex, actions) && !self.started {
            return;
        }
        if !self.active_null_in_flight && !self.power_save_null_in_flight {
            return;
        }
        if succeeded {
            if self.state == PmState::Dozing {
                self.dream(coex, Some(clock), actions);
            }
            if !power_save {
                self.active_null_failures = 0;
                if !self.active_null_in_flight {
                    return;
                }
                self.active_null_in_flight = false;
                if self.in_slice {
                    // `pm_tx_null_data_done_quick_wake_process` without TBTT
                    // quick wake admits the wake.
                    self.receiving = false;
                    self.enter(PmState::Awake);
                    self.enable_active_timer(actions);
                    actions.push(PmAction::ReleaseHeldFrames);
                } else {
                    self.wake_deferred = true;
                }
                return;
            }
            self.power_save_null_failures = 0;
            if !self.power_save_null_in_flight {
                return;
            }
            self.power_save_null_in_flight = false;
            if self.preempted && clock.now_micros >= self.preemption_end {
                self.sleep(clock, coex, traffic, actions);
            } else {
                self.enable_sleep_delay_timer(actions);
            }
            return;
        }
        if !power_save {
            if !self.active_null_in_flight {
                return;
            }
            self.active_null_in_flight = false;
            if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, actions) {
                self.active_null_failures = self.active_null_failures.saturating_add(1);
                self.send_null(false, actions);
            } else {
                self.active_null_failures = 0;
                self.wake_deferred = true;
            }
            return;
        }
        if !self.power_save_null_in_flight {
            return;
        }
        self.power_save_null_in_flight = false;
        if self.in_slice {
            self.power_save_null_failures = self.power_save_null_failures.saturating_add(1);
            self.send_null(true, actions);
        } else {
            self.power_save_null_failures = 0;
            self.wake_deferred = true;
            self.enable_sleep_delay_timer(actions);
        }
    }

    /// `pm_tbtt_process` with `pm_coex_tbtt_process`, `pm_set_next_tbtt`
    /// and `pm_update_params`: the station TBTT event fired.
    pub fn tbtt(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        let _ = self.may_sleep(coex, actions);
        if !self.started {
            actions.push(PmAction::Disarm(PmTimer::SliceEnd));
            self.in_slice = true;
            actions.push(PmAction::UnblockTx);
            return;
        }
        let beacon_was_expected = self.beacon_expected;
        self.beacon_expected = true;
        if self.state == PmState::Dozing {
            self.dream(coex, Some(clock), actions);
            self.receiving = false;
        }
        // The cycle counts from this TBTT unless the associated station
        // missed the previous beacon, or a beacon was still expected when
        // its slice ended under coexistence.
        let keep_anchor = beacon_was_expected || (coex.active && self.beacon_expected_at_slice_end);
        if !keep_anchor {
            self.cycle_anchor = clock.now_micros;
        }
        self.coex_tbtt(clock, coex, traffic, actions);
        self.set_next_tbtt(coex, actions);
        self.update_params(clock, coex, traffic, actions);
    }

    /// `pm_coex_tbtt_process`.
    fn coex_tbtt(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        actions.push(PmAction::Disarm(PmTimer::SliceEnd));
        self.in_slice = true;
        if coex.active {
            self.slice_deadline = clock.now_micros.wrapping_add(
                u64::from(coex.overall_period()) * u64::from(self.beacon_interval_micros),
            );
            actions.push(PmAction::SetCoexInterval(
                self.beacon_interval_micros / SCHEDULE_INTERVAL_UNIT_MICROS,
            ));
            actions.push(PmAction::RestartCoexPhases);
            actions.push(PmAction::RxBeaconPriority(true));
            actions.push(PmAction::CoexRequest {
                event: PmCoexEvent::BeaconWindow,
                duration_micros: self.beacon_window_micros,
            });
            self.activity = 0;
            if !self.is_sleeping() {
                actions.push(PmAction::UnblockTx);
            }
            if self.sleep_type == SleepType::None || self.wake_deferred {
                self.wake_deferred = false;
                let _ = self.go_to_wake(false, clock, coex, traffic, actions);
            }
            return;
        }
        if !self.is_sleeping() {
            actions.push(PmAction::UnblockTx);
            if traffic.connection_pending {
                let _ = self.go_to_wake(true, clock, coex, traffic, actions);
            }
        }
    }

    /// `pm_set_next_tbtt`: follow a changed schedule period.
    fn set_next_tbtt(&mut self, coex: CoexView, actions: &mut PmActions) {
        let period = coex.overall_period();
        if !coex.active || self.tbtt_period == period {
            return;
        }
        self.tbtt_period = period;
        actions.push(PmAction::SetTbttInterval(
            u32::from(period) * self.beacon_interval_micros,
        ));
        self.update_tbtt_at_next_beacon = true;
    }

    /// `pm_update_params`.
    fn update_params(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        let (active_timeout, ahead) = if coex.active {
            (
                u32::from(coex.overall_period()) * self.beacon_interval_micros,
                SHARED_TBTT_AHEAD_MICROS,
            )
        } else {
            (ACTIVE_TIMEOUT_MICROS, TBTT_AHEAD_MICROS)
        };
        self.active_timeout_micros = active_timeout;
        if self.tbtt_ahead_micros != ahead || self.tbtt_window_micros != TBTT_WAKE_WINDOW_MICROS {
            self.tbtt_ahead_micros = ahead;
            self.tbtt_window_micros = TBTT_WAKE_WINDOW_MICROS;
            actions.push(PmAction::SetTbttAhead {
                ahead_micros: ahead,
                wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + ahead,
            });
        }
        if coex.active != self.coex_was_active {
            self.update_tbtt_at_next_beacon = true;
            // A station without power save that slept only for coexistence
            // wakes when coexistence ends.
            if self.coex_was_active && self.sleep_type == SleepType::None {
                let _ = self.go_to_wake(false, clock, coex, traffic, actions);
            }
            self.coex_was_active = coex.active;
        }
    }

    /// `pm_on_beacon_rx` with `pm_parse_beacon`, `pm_update_next_tbtt`,
    /// `pm_rx_beacon_process` and `pm_process_tim`: a beacon of the
    /// associated access point arrived.
    pub fn beacon(
        &mut self,
        beacon: PmBeacon,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        if !self.started {
            return;
        }
        self.parse_beacon(beacon, coex, actions);
        self.update_next_tbtt(beacon, coex, actions);
        self.beacon_expected_at_slice_end = false;
        self.beacon_expected = false;
        let tim = beacon.tim;
        self.process_tim(
            tim.is_some_and(|tim| tim.unicast),
            tim.is_some_and(|tim| tim.group),
            clock,
            coex,
            traffic,
            actions,
        );
        actions.push(PmAction::CoexRelease(PmCoexEvent::BeaconWindow));
        actions.push(PmAction::ClearRxBeaconPriority);
    }

    /// `pm_parse_beacon`.
    fn parse_beacon(&mut self, beacon: PmBeacon, coex: CoexView, actions: &mut PmActions) {
        let interval = if beacon.interval_tu == 0 {
            DEFAULT_BEACON_INTERVAL_MICROS
        } else {
            u32::from(beacon.interval_tu) << 10
        };
        let dtim_period = beacon
            .tim
            .map_or(self.dtim_period, |tim| tim.dtim_period.max(1));
        if !self.beacon_parsed {
            self.beacon_interval_micros = interval;
            self.dtim_period = dtim_period;
            if coex.active {
                actions.push(PmAction::SetCoexInterval(
                    interval / SCHEDULE_INTERVAL_UNIT_MICROS,
                ));
            }
            self.beacon_parsed = true;
            self.update_tbtt_at_next_beacon = true;
            return;
        }
        if self.beacon_interval_micros != interval {
            self.beacon_interval_micros = interval;
            if coex.active {
                actions.push(PmAction::SetCoexInterval(
                    interval / SCHEDULE_INTERVAL_UNIT_MICROS,
                ));
            }
            self.update_tbtt_at_next_beacon = true;
        }
        if self.dtim_period != dtim_period {
            self.dtim_period = dtim_period;
            if self.sleep_type != SleepType::None {
                self.update_tbtt_at_next_beacon = true;
            }
        }
    }

    /// `pm_handle_tbtt_interval`: a TBTT interval of `beacons` beacon
    /// intervals kept in step with the DTIM. When the DTIM is not yet
    /// aligned with that period, the interval first runs up to the DTIM
    /// and is re-derived at the next beacon.
    fn handle_tbtt_interval(&mut self, beacons: u8, tim: Option<PmTim>) -> u32 {
        let Some(tim) = tim else {
            self.update_tbtt_at_next_beacon = true;
            return self.beacon_interval_micros;
        };
        let beacons = beacons.max(1);
        let period = tim.dtim_period.max(1);
        let compatible = beacons.is_multiple_of(period) || period.is_multiple_of(beacons);
        let offset = tim.dtim_count % beacons;
        if !compatible || offset == 0 {
            return u32::from(beacons) * self.beacon_interval_micros;
        }
        self.update_tbtt_at_next_beacon = true;
        u32::from(offset) * self.beacon_interval_micros
    }

    /// `pm_update_next_tbtt`: program the TBTT schedule from this beacon.
    fn update_next_tbtt(&mut self, beacon: PmBeacon, coex: CoexView, actions: &mut PmActions) {
        if !self.update_tbtt_at_next_beacon {
            return;
        }
        self.update_tbtt_at_next_beacon = false;
        let ahead = if coex.active {
            SHARED_TBTT_AHEAD_MICROS
        } else {
            TBTT_AHEAD_MICROS
        };
        self.tbtt_ahead_micros = ahead;
        self.tbtt_window_micros = TBTT_WAKE_WINDOW_MICROS;
        let interval = if coex.active {
            self.handle_tbtt_interval(coex.overall_period(), beacon.tim)
        } else {
            match self.sleep_type {
                SleepType::MinModem => self.handle_tbtt_interval(self.dtim_period, beacon.tim),
                SleepType::MaxModem | SleepType::None => self.beacon_interval_micros,
            }
        };
        let phase = beacon.timestamp_tsf % u64::from(self.beacon_interval_micros);
        actions.push(PmAction::StartTbtt(PmTbttSchedule {
            first_tbtt_tsf: beacon.timestamp_tsf - phase,
            interval_micros: interval,
            ahead_micros: ahead,
            wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + ahead,
        }));
    }

    /// `pm_process_tim`.
    fn process_tim(
        &mut self,
        unicast: bool,
        group: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        match self.state {
            PmState::PowerSave => {
                if unicast {
                    if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, actions) {
                        self.receiving = true;
                        self.request_slice_until_end(clock.now_micros, actions);
                        let _ = self.go_to_wake(false, clock, coex, traffic, actions);
                    } else {
                        self.wake_deferred = true;
                    }
                } else if group {
                    if self.in_slice_threshold(clock, GROUP_SLICE_THRESHOLD_MICROS, coex, actions) {
                        self.receiving = true;
                        self.enable_dream_timer(actions);
                        let now = clock.now_micros;
                        if now < self.slice_end {
                            let remaining = u32::try_from(self.slice_end - now).unwrap_or(u32::MAX);
                            actions.push(PmAction::CoexRequest {
                                event: PmCoexEvent::GroupTraffic,
                                duration_micros: remaining.min(self.beacon_window_micros),
                            });
                        }
                    }
                } else {
                    self.receiving = false;
                    if !self.active_null_in_flight && !self.power_save_null_in_flight {
                        self.disable_sleep_delay_timer(actions);
                        self.sleep(clock, coex, traffic, actions);
                    }
                }
            }
            PmState::Awake | PmState::Dozing => {}
        }
    }

    /// `pm_tx_data_process`: the TX path offers a frame. Returns whether the
    /// frame must wait for the station to leave power save.
    pub fn tx_data(
        &mut self,
        is_data: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) -> bool {
        if !self.may_sleep(coex, actions) {
            return false;
        }
        match self.state {
            PmState::PowerSave | PmState::Dozing => {
                let dozing = self.state == PmState::Dozing;
                let mut hold = false;
                if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, actions) {
                    self.request_slice_until_end(clock.now_micros, actions);
                    if dozing {
                        self.dream(coex, Some(clock), actions);
                    }
                    if is_data {
                        let woke = self.go_to_wake(true, clock, coex, traffic, actions);
                        hold = matches!(woke, WakeResult::NotStarted | WakeResult::Deferred);
                    }
                } else if is_data {
                    self.wake_deferred = true;
                    hold = true;
                }
                if is_data {
                    self.incr_activity();
                }
                hold
            }
            PmState::Awake => {
                if is_data {
                    if self.sleep_type != SleepType::None {
                        self.enable_active_timer(actions);
                    }
                    self.incr_activity();
                }
                false
            }
        }
    }

    /// `pm_tx_data_done_process`: a data frame completed.
    pub fn tx_data_done(
        &mut self,
        acknowledged: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        if !traffic.tx_pending && self.state == PmState::PowerSave {
            self.enable_sleep_delay_timer(actions);
        }
        if !self.may_sleep(coex, actions) || !acknowledged {
            return;
        }
        self.incr_activity();
        if !self.in_slice {
            self.wake_deferred = true;
        }
        if self.state == PmState::Awake {
            self.enable_active_timer(actions);
        }
        let _ = clock;
    }

    /// `pm_rx_data_process`: a data frame of the access point arrived.
    pub fn rx_data(
        &mut self,
        group: bool,
        more_data: bool,
        coex: CoexView,
        actions: &mut PmActions,
    ) {
        if !self.may_sleep(coex, actions) {
            return;
        }
        self.incr_activity();
        match self.state {
            PmState::PowerSave => {
                if !self.in_slice {
                    return;
                }
                if group {
                    if more_data {
                        self.receiving = true;
                        self.enable_dream_timer(actions);
                    } else {
                        self.receiving = false;
                        actions.push(PmAction::Disarm(PmTimer::Dream));
                        self.enable_sleep_delay_timer(actions);
                    }
                } else {
                    self.receiving = more_data;
                }
            }
            PmState::Awake => self.enable_active_timer(actions),
            PmState::Dozing => {}
        }
    }

    /// `pm_coex_schm_process`: the schedule entered a phase that notifies
    /// Wi-Fi.
    pub fn coex_phase(
        &mut self,
        phase: CoexPhaseView,
        clock: PmClock,
        coex: CoexView,
        actions: &mut PmActions,
    ) {
        let now = clock.now_micros;
        let slice = u64::from(phase.share_percent)
            * u64::from(coex.interval)
            * u64::from(coex.current_period);
        let slice_end = now.wrapping_add(slice);
        if phase.wifi & CoexPhaseView::WIFI_SLICE != 0 {
            actions.push(PmAction::CoexRequest {
                event: PmCoexEvent::Slice,
                duration_micros: u32::try_from(slice).unwrap_or(u32::MAX),
            });
            self.in_slice = true;
            self.slice_end = slice_end;
            self.cycle_remainder = coex.cycle_micros().wrapping_sub(slice);
            if !self.is_sleeping() {
                actions.push(PmAction::UnblockTx);
            }
        }
        if phase.wifi & CoexPhaseView::SLICE_END != 0 {
            actions.push(PmAction::Disarm(PmTimer::SliceEnd));
            let margin = if self.started {
                u64::from(SLEEP_DELAY_MICROS)
            } else {
                u64::from(IDLE_SLICE_MARGIN_MICROS)
            };
            self.slice_deadline = slice_end.wrapping_sub(margin);
            actions.push(PmAction::Arm {
                timer: PmTimer::SliceEnd,
                after_micros: self.slice_deadline.wrapping_sub(now),
            });
        }
        if phase.wifi & CoexPhaseView::SHARED != 0 {
            if !self.is_sleeping() {
                actions.push(PmAction::CoexRequest {
                    event: PmCoexEvent::Slice,
                    duration_micros: u32::try_from(slice).unwrap_or(u32::MAX),
                });
            }
            actions.push(PmAction::BlockTx);
        }
    }

    /// `pm_coex_go_to_sleep`: the slice ended, or a preemption did.
    fn coex_go_to_sleep(
        &mut self,
        from_slice_end: bool,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        if !self.started || !coex.active {
            return;
        }
        if from_slice_end {
            self.in_slice = false;
        }
        self.beacon_expected_at_slice_end = self.beacon_expected;
        self.beacon_expected = false;
        self.receiving = false;
        actions.push(PmAction::BlockTx);
        if self.state != PmState::Awake && !self.active_null_in_flight {
            if self.state == PmState::PowerSave {
                self.enable_sleep_delay_timer(actions);
            }
            return;
        }
        if self.activity != 0 || traffic.connection_pending {
            self.wake_deferred = true;
        }
        self.go_to_sleep(coex, actions);
    }

    /// The slice-end timer expired (`pm_coex_slice_timeout_process`).
    pub fn slice_end_timer(&mut self, coex: CoexView, traffic: PmTraffic, actions: &mut PmActions) {
        self.coex_go_to_sleep(true, coex, traffic, actions);
    }

    /// The preemption timer expired (`pm_coex_preemption_timeout_process`).
    pub fn preemption_timer(
        &mut self,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        self.coex_go_to_sleep(false, coex, traffic, actions);
    }

    /// The active timer expired (`pm_active_timeout_process`).
    pub fn active_timer(&mut self, coex: CoexView, traffic: PmTraffic, actions: &mut PmActions) {
        if !self.started || self.state != PmState::Awake {
            return;
        }
        if self.receiving || traffic.tx_pending {
            self.enable_active_timer(actions);
            return;
        }
        self.go_to_sleep(coex, actions);
    }

    /// The sleep-delay timer expired (`pm_sleep_delay_timeout_process`).
    pub fn sleep_delay_timer(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        self.sleep_delay_armed = false;
        if self.state == PmState::PowerSave {
            self.sleep(clock, coex, traffic, actions);
        }
    }

    /// The dream timer expired (`pm_dream_timeout_process`).
    pub fn dream_timer(&mut self, actions: &mut PmActions) {
        self.receiving = false;
        self.enable_sleep_delay_timer(actions);
    }

    /// `pm_on_coex_preemption_end`: Bluetooth reports when its isochronous
    /// preemption ends, or that the end is unknown.
    pub fn preemption_end(
        &mut self,
        end_micros: Option<u64>,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        actions: &mut PmActions,
    ) {
        let now = clock.now_micros;
        let was_preempted = self.preempted;
        self.preempted = end_micros.is_some();
        let margin = if self.started { 2_000 } else { 1_000 };
        self.preemption_end = end_micros.unwrap_or(now).wrapping_sub(margin);
        actions.push(PmAction::Disarm(PmTimer::Preemption));
        if self.preempted && now < self.preemption_end {
            actions.push(PmAction::Arm {
                timer: PmTimer::Preemption,
                after_micros: self.preemption_end - now,
            });
        }
        if self.started {
            if self.state != PmState::Awake {
                if self.wake_deferred && self.in_slice_threshold(clock, 2_000, coex, actions) {
                    self.wake_deferred = false;
                    if self.state == PmState::Dozing {
                        self.dream(coex, Some(clock), actions);
                    }
                    let _ = self.go_to_wake(false, clock, coex, traffic, actions);
                    self.request_slice_until_end(now, actions);
                } else {
                    actions.push(PmAction::Disarm(PmTimer::Preemption));
                }
            } else if self.preempted && now >= self.preemption_end {
                self.preemption_timer(coex, traffic, actions);
            }
        }
        if self.preempted != was_preempted {
            let (window, time) = if self.preempted {
                (PREEMPTED_BEACON_WINDOW_MICROS, PREEMPTED_BEACON_TIME_MICROS)
            } else {
                (BEACON_WINDOW_MICROS, BEACON_TIME_MICROS)
            };
            self.beacon_window_micros = window;
            actions.push(PmAction::RxBeaconTime {
                window_micros: window,
                time_micros: time,
            });
        }
    }

    /// Request the slice until its end when it has not ended.
    fn request_slice_until_end(&mut self, now: u64, actions: &mut PmActions) {
        if now < self.slice_end {
            actions.push(PmAction::CoexRequest {
                event: PmCoexEvent::Slice,
                duration_micros: u32::try_from(self.slice_end - now).unwrap_or(u32::MAX),
            });
        }
    }
}

#[cfg(test)]
mod tests;
