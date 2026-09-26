//! Bounded IEEE 802.15.4 diagnostic requests and observations.

use serde::{Deserialize, Serialize};

/// Bounds for one IEEE 802.15.4 `EVENT_STATUS` observation probe.
///
/// `poll_limit` bounds every target-side wait loop. `timer_threshold` is the
/// target-defined timer separation used to make the two timer observations
/// distinct; it is intentionally a protocol value rather than an MMIO layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154EventStatusProbeRequest {
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154EventStatusProbeRequest {
    /// Returns whether both probe bounds are finite and supported by the wire
    /// contract.
    pub const fn validate(self) -> bool {
        self.poll_limit >= 1
            && self.poll_limit <= 1_000_000
            && self.timer_threshold >= 1
            && self.timer_threshold <= 1_000
    }
}

/// Terminal observation reached by an IEEE 802.15.4 `EVENT_STATUS` probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154EventStatusProbeStop {
    Complete,
    UnsupportedSetup,
    RouteNotQuiesced,
    ResetNotClear,
    EventEnableReadbackMismatch,
    PostEnableStatusNotClear,
    TimerActivityTimeout,
    DualLatchTimeout,
    SelectiveAcknowledgeMismatch,
    DistinctFirstLatchTimeout,
    DistinctSecondLatchTimeout,
    CleanupNotClear,
}

/// Target-neutral semantic classification of one complete MAC event sample.
///
/// `UnexpectedNamed` retains a source-confirmed combination which is outside
/// the probe vocabulary. `Unclassified` retains a physical event for which no
/// reviewed semantic identity exists. Neither variant exposes register
/// positions or can be replayed as a write image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154ObservedEventState {
    #[default]
    Clear,
    Timer0Only,
    Timer1Only,
    Timer0AndTimer1,
    EdDoneOnly,
    EdDoneAndTimer0,
    RxAbortOnly,
    RxAbortWithOther,
    EdDoneWithOther,
    EdDoneAndRxAbortWithOther,
    UnexpectedNamed,
    Unclassified,
}

/// Target-neutral semantic readback of a validation event window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154ValidationEventEnableState {
    #[default]
    AllMasked,
    TimerPairOnly,
    EdDoneTimer0RxAbortOnly,
    Unexpected,
}

/// Target-neutral semantic readback of the validation RX-abort window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154ValidationRxAbortEnableState {
    #[default]
    AllMasked,
    EdOperationReasonsOnly,
    Unexpected,
}

/// Target-neutral semantic readback of the fixed validation ED duration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154ValidationEdDurationState {
    ValidationEight,
    #[default]
    Other,
}

/// One source-confirmed receive-abort reason retained by HIL evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154RxAbortReason {
    RxStop,
    SfdTimeout,
    CrcError,
    InvalidLength,
    FilterFail,
    NoRss,
    CoexistenceBreak,
    UnexpectedAck,
    RxRestart,
    TxAckTimeout,
    TxAckStop,
    TxAckCoexistenceBreak,
    EnhancedAckSecurityError,
    EdAbort,
    EdStop,
    EdCoexistenceReject,
}

/// Semantic classification of one sampled receive-abort reason field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154RxAbortObservation {
    Named(Ieee802154RxAbortReason),
    Unclassified,
}

/// Semantic snapshots from one bounded IEEE 802.15.4 `EVENT_STATUS` probe.
///
/// This evidence is observation only. Even a [`Ieee802154EventStatusProbeStop::Complete`]
/// result does not prove same-bit concurrency, level-triggered retrigger
/// behavior, or readiness of a production interrupt path.
/// `dual_observed_events` is the union of the first bounded wait;
/// `dual_latched_events` is its terminal sample. `cleanup_pending_events` is
/// the observation after delivery is masked again; it may hide a retained
/// latch and is therefore not the source of the best-effort cleanup selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154EventStatusProbeEvidence {
    pub stop: Ieee802154EventStatusProbeStop,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub timer0_value_before_start: u32,
    pub timer1_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer1_value_min: u32,
    pub timer1_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub timer1_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub dual_observed_events: Ieee802154ObservedEventState,
    pub dual_latched_events: Ieee802154ObservedEventState,
    pub after_timer0_ack_events: Ieee802154ObservedEventState,
    pub after_timer1_ack_events: Ieee802154ObservedEventState,
    pub distinct_snapshot_events: Ieee802154ObservedEventState,
    pub distinct_before_ack_events: Ieee802154ObservedEventState,
    pub distinct_after_ack_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
}

/// Bounds for one IEEE 802.15.4 ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154EdEventProbeRequest {
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154EdEventProbeRequest {
    /// Return whether both target-side bounds are finite and supported.
    pub const fn validate(self) -> bool {
        self.poll_limit >= 1
            && self.poll_limit <= 1_000_000
            && self.timer_threshold >= 1
            && self.timer_threshold <= 1_000
    }
}

/// Terminal classification from the ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154EdEventProbeStop {
    Complete,
    ProductionEdFailed,
    UnsupportedSetup,
    RouteNotQuiesced,
    ResetNotClear,
    EdDurationReadbackMismatch,
    EventEnableReadbackMismatch,
    RxAbortEnableReadbackMismatch,
    PostEnableStatusNotClear,
    TimerActivityTimeout,
    PairLatchTimeout,
    EdAborted,
    UnexpectedEvent,
    SelectiveWriteMismatch,
    CleanupNotClear,
}

/// Checkpoint retained when a production polled ED invariant fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154PolledEdStage {
    Prepare,
    StartEventWindow,
    StartCommand,
    Poll,
    TerminalSample,
    AcknowledgeTerminalEvent,
    Cleanup,
}

/// Semantic enable-mask observation retained without exposing a writable
/// register image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154PolledEdMaskState {
    AllMasked,
    OperationOnly,
    Unexpected,
}

/// Complete terminal evidence from one production polled ED attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154PolledEdOutcome {
    NotRun,
    Complete {
        rss_code: i8,
        polls: u32,
    },
    Aborted {
        event_status: Ieee802154ObservedEventState,
        rx_abort_reason: Ieee802154RxAbortObservation,
        polls: u32,
    },
    Timeout {
        polls: u32,
    },
    CpuInterruptRouteAttached {
        stage: Ieee802154PolledEdStage,
    },
    UnexpectedEventMask {
        stage: Ieee802154PolledEdStage,
        observed: Ieee802154PolledEdMaskState,
    },
    UnexpectedRxAbortMask {
        stage: Ieee802154PolledEdStage,
        observed: Ieee802154PolledEdMaskState,
    },
    StaleEventStatus {
        event_status: Ieee802154ObservedEventState,
    },
    UnexpectedTerminalStatus {
        event_status: Ieee802154ObservedEventState,
    },
    UnexpectedAcknowledgedEvents {
        event_status: Ieee802154ObservedEventState,
    },
    ConflictingTerminalEvents {
        event_status: Ieee802154ObservedEventState,
    },
}

/// Complete semantic evidence from one bounded ED-DONE/TIMER0 discriminator.
///
/// A successful result proves only the selected ED-DONE/TIMER0 relation in
/// this reset-isolated transaction; it is not a register-wide W1C or
/// production ED-readiness claim. `rx_abort_reason` is present only when
/// RX-ABORT was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154EdEventProbeEvidence {
    pub stop: Ieee802154EdEventProbeStop,
    pub production_ed_first: Ieee802154PolledEdOutcome,
    pub production_ed_second: Option<Ieee802154PolledEdOutcome>,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub rx_abort_enable_before: Ieee802154ValidationRxAbortEnableState,
    pub rx_abort_enable_active: Ieee802154ValidationRxAbortEnableState,
    pub rx_abort_enable_after: Ieee802154ValidationRxAbortEnableState,
    pub ed_duration_before: Ieee802154ValidationEdDurationState,
    pub ed_duration_active: Ieee802154ValidationEdDurationState,
    pub ed_duration_after: Ieee802154ValidationEdDurationState,
    pub timer0_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub observed_events: Ieee802154ObservedEventState,
    pub terminal_events: Ieee802154ObservedEventState,
    pub after_ed_done_write_events: Ieee802154ObservedEventState,
    pub after_timer0_write_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
    pub rx_abort_reason: Option<Ieee802154RxAbortObservation>,
    pub stop_command_issued: bool,
    pub cleanup_clear: bool,
}

/// Most start/stop cycles one air check runs.
pub const IEEE802154_AIR_CHECK_MAX_CYCLES: usize = 4;

/// Bounds for one single-device IEEE 802.15.4 on-air check.
///
/// Each cycle starts the IEEE 802.15.4 client on the shared radio arbiter,
/// runs one energy scan, one clear-channel assessment, one direct transmit
/// without an acknowledgement request, two scheduled transmits and one receive
/// window on `channel`, then stops the client again.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154AirCheckRequest {
    /// IEEE 802.15.4 channel, 11 through 26.
    pub channel: u8,
    /// Start/stop cycles, 1 through [`IEEE802154_AIR_CHECK_MAX_CYCLES`].
    pub cycles: u8,
    /// Energy-scan duration in microseconds.
    pub energy_scan_micros: u32,
    /// Receive-window length in milliseconds.
    pub receive_window_millis: u32,
    /// Lead from the request to each scheduled transmit start, in microseconds.
    pub scheduled_lead_micros: u32,
}

impl Ieee802154AirCheckRequest {
    /// Returns whether every bound is inside the wire contract.
    pub const fn validate(self) -> bool {
        self.channel >= 11
            && self.channel <= 26
            && self.cycles >= 1
            && self.cycles as usize <= IEEE802154_AIR_CHECK_MAX_CYCLES
            && self.energy_scan_micros >= 128
            && self.energy_scan_micros <= 1_000_000
            && self.receive_window_millis >= 1
            && self.receive_window_millis <= 10_000
            && self.scheduled_lead_micros >= 1_000
            && self.scheduled_lead_micros <= 1_000_000
    }
}

/// Where an air check stopped.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154AirCheckStop {
    /// Every cycle completed.
    Complete,
    /// The image could not claim the radio.
    #[default]
    UnsupportedSetup,
    /// Bring-up of the IEEE 802.15.4 client failed.
    StartFailed,
    /// The runtime rejected a command.
    CommandRejected,
    /// No terminal event arrived in time.
    EventTimeout,
    /// The runtime's event queue overflowed.
    EventsLost,
    /// A terminal event did not match the command.
    UnexpectedEvent,
    /// Teardown of the IEEE 802.15.4 client failed.
    StopFailed,
}

/// Outcome of one energy scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154AirEnergyOutcome {
    #[default]
    NotRun,
    /// Averaged channel energy in dBm.
    Energy(i8),
    Failed,
}

/// Outcome of one clear-channel assessment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154AirCcaOutcome {
    #[default]
    NotRun,
    Clear,
    Busy,
    Failed,
}

/// Terminal status of one transmit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ieee802154AirTxOutcome {
    #[default]
    NotRun,
    Success,
    ChannelBusy,
    NoAcknowledgement,
    Aborted,
    InvalidFrame,
    HardwareFailure,
    CoexistenceRejected,
    SecurityFailure,
    InvalidAcknowledgement,
}

/// One transmit and its completion time.
///
/// Times are target-monotonic microseconds. A direct transmit's
/// `requested_at_micros` is the submission time; a scheduled transmit's is the
/// requested start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154AirTransmit {
    pub outcome: Ieee802154AirTxOutcome,
    pub requested_at_micros: u64,
    pub done_at_micros: u64,
}

/// Observations of one start/stop cycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154AirCycle {
    pub energy: Ieee802154AirEnergyOutcome,
    pub cca: Ieee802154AirCcaOutcome,
    pub direct: Ieee802154AirTransmit,
    pub scheduled: [Ieee802154AirTransmit; 2],
    /// Frames received during the receive window.
    pub received_frames: u16,
    /// Strongest receive RSSI in dBm, when any frame arrived.
    pub strongest_rssi_dbm: Option<i8>,
}

/// Terminal observation of one air check.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154AirCheckEvidence {
    pub stop: Ieee802154AirCheckStop,
    /// Cycles that completed, including teardown.
    pub completed_cycles: u8,
    pub cycles: [Ieee802154AirCycle; IEEE802154_AIR_CHECK_MAX_CYCLES],
}
