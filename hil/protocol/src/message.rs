use core::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 79;
/// Maximum number of independently accounted transport flows in one network
/// interface session.
///
/// Two flows are sufficient for the first physical multi-client AP cell. The
/// fixed bound keeps the no-alloc wire contract explicit and does not change
/// the target rule that one session owns one network interface.
pub const SESSION_FLOW_CAPACITY: usize = 2;
// Keep command envelopes small: startup artifacts are transferred as an
// ordered CRC-protected stream, so a large per-command inline buffer only
// inflates UART queues and executor futures without improving semantics.
pub const STARTUP_ARTIFACT_CHUNK_MAX_LEN: usize = 160;
// Keep the largest protocol enum comfortably below one RX frame. This value
// bounds executor poll-stack pressure as well as wire latency; complete MPDUs
// are reconstructed from ordered chunks on the host.
pub const WIFI_MONITOR_FRAME_CHUNK_MAX_LEN: usize = 160;
pub const WPA2_SSID_MAX_LEN: usize = 32;
pub const WPA2_PASSPHRASE_MIN_LEN: usize = 8;
pub const WPA2_PASSPHRASE_MAX_LEN: usize = 63;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol_version: u16,
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub body: T,
}

/// Message direction encoded in the fixed wire header.
///
/// Keeping this outside the postcard body lets a decoder reject a frame sent
/// to the wrong endpoint before interpreting the command or event enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WireKind {
    Command = 1,
    Event = 2,
}

pub trait WireBody {
    const WIRE_KIND: WireKind;
}

impl<T> Envelope<T> {
    pub const fn new(
        boot_id: u64,
        message_sequence: u32,
        session_id: u64,
        request_id: u32,
        body: T,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            boot_id,
            message_sequence,
            session_id,
            request_id,
            body,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Transport {
    Udp,
    Tcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Direction {
    Rx,
    Tx,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Completion {
    DurationMillis(u32),
    TransferBytes(u64),
    HostStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ipv4Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlowConfig {
    pub payload_bytes: u16,
    pub offered_rate_bps: Option<u64>,
    /// Number of datagrams admitted at one offered-rate deadline.
    ///
    /// `None` selects the target's bounded throughput-oriented default. A
    /// small explicit value lets fairness HIL describe sparse packet bursts
    /// without turning a low average bitrate into one queue-sized burst.
    pub pacing_group_datagrams: Option<u8>,
}

/// One independently identifiable flow inside a network-interface session.
///
/// `peer` is the target's UDP transmit destination when `target_tx` is present.
/// For receive-only sessions it may be absent when there is exactly one flow;
/// multi-flow receive sessions require a peer so the target can classify the
/// source without combining independent sequence spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionFlowConfig {
    pub flow_id: u8,
    pub peer: Option<Ipv4Endpoint>,
    pub target_rx: Option<FlowConfig>,
    pub target_tx: Option<FlowConfig>,
}

/// Link properties that must be true before a measured transport session may
/// advertise readiness.
///
/// Correctness cells deliberately use [`Self::NONE`]: a standards-compliant
/// peer may reject aggregation and the data plane must still work. Throughput
/// cells can require one negotiated TX BlockAck TID so an absent AddBA
/// response is reported as unavailable test precondition instead of being
/// misclassified as slow S-MPDU performance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionLinkRequirements {
    pub tx_block_ack_tid: Option<u8>,
}

impl SessionLinkRequirements {
    pub const NONE: Self = Self {
        tx_block_ack_tid: None,
    };

    pub const fn tx_block_ack(tid: u8) -> Self {
        Self {
            tx_block_ack_tid: Some(tid),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Exact network endpoint that owns this transport session. The physical
    /// radio may expose both endpoints during one same-channel STA+AP epoch;
    /// transport ownership must therefore never be inferred from the Wi-Fi
    /// role or from whichever stack became ready first.
    pub network_interface: WifiNetworkInterface,
    pub transport: Transport,
    pub direction: Direction,
    pub completion: Completion,
    /// Contiguous flow table. Slot zero is always occupied. A second occupied
    /// slot represents another UDP peer on the same network interface and is
    /// accepted only when [`FeatureCapabilities::udp_multi_flow`] is true.
    pub flows: [Option<SessionFlowConfig>; SESSION_FLOW_CAPACITY],
    pub link_requirements: SessionLinkRequirements,
}

impl SessionConfig {
    pub const fn primary_flow(self) -> Option<SessionFlowConfig> {
        self.flows[0]
    }

    pub fn active_flow_count(self) -> usize {
        self.flows.iter().flatten().count()
    }

    /// Validate the target-neutral bounded session shape.
    ///
    /// Runtime feature availability remains the target's responsibility. This
    /// method owns the wire invariants so host/model tests exercise the same
    /// rules as embedded admission.
    pub fn structurally_valid(self, maximum_payload_bytes: u16, udp_multi_flow: bool) -> bool {
        let flow_count = self.active_flow_count();
        if flow_count == 0 || self.flows[0].is_none() {
            return false;
        }
        let mut saw_empty = false;
        for flow in self.flows {
            match flow {
                Some(_) if saw_empty => return false,
                Some(_) => {}
                None => saw_empty = true,
            }
        }
        if flow_count > 1 && (self.transport != Transport::Udp || !udp_multi_flow) {
            return false;
        }

        let valid_flow = |flow: FlowConfig| {
            flow.payload_bytes >= 64
                && flow.payload_bytes <= maximum_payload_bytes
                && flow
                    .offered_rate_bps
                    .is_none_or(|rate| (1_000..=1_000_000_000).contains(&rate))
                && flow
                    .pacing_group_datagrams
                    .is_none_or(|datagrams| datagrams != 0 && flow.offered_rate_bps.is_some())
        };
        let identities_valid = self
            .flows
            .iter()
            .flatten()
            .enumerate()
            .all(|(index, flow)| {
                self.flows[..index]
                    .iter()
                    .flatten()
                    .all(|earlier| earlier.flow_id != flow.flow_id)
                    && flow.peer.is_none_or(|peer| {
                        peer.port != 0
                            && self.flows[..index]
                                .iter()
                                .flatten()
                                .filter_map(|earlier| earlier.peer)
                                .all(|earlier| earlier != peer)
                    })
            });
        let pacing_valid = self.flows.iter().flatten().all(|flow| {
            flow.target_rx
                .is_none_or(|rx| rx.pacing_group_datagrams.is_none())
                && flow.target_tx.is_none_or(|tx| {
                    tx.pacing_group_datagrams.is_none() || self.transport == Transport::Udp
                })
        });
        let peers_valid = match (self.transport, self.direction) {
            (Transport::Tcp, _) => {
                flow_count == 1 && self.flows.iter().flatten().all(|flow| flow.peer.is_none())
            }
            (Transport::Udp, Direction::Rx) if flow_count == 1 => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_none())
            }
            (Transport::Udp, Direction::Rx) => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_some())
            }
            (Transport::Udp, Direction::Tx | Direction::Bidirectional) => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_some())
            }
        };
        let direction_valid = match self.direction {
            Direction::Rx => self
                .flows
                .iter()
                .flatten()
                .all(|flow| flow.target_rx.is_some_and(valid_flow) && flow.target_tx.is_none()),
            Direction::Tx => self
                .flows
                .iter()
                .flatten()
                .all(|flow| flow.target_rx.is_none() && flow.target_tx.is_some_and(valid_flow)),
            Direction::Bidirectional => self.flows.iter().flatten().all(|flow| {
                flow.target_rx.is_some_and(valid_flow) && flow.target_tx.is_some_and(valid_flow)
            }),
        };
        let link_requirements_valid = match self.link_requirements.tx_block_ack_tid {
            None => true,
            Some(tid) => {
                tid < 8
                    && matches!(self.direction, Direction::Tx | Direction::Bidirectional)
                    && self
                        .flows
                        .iter()
                        .flatten()
                        .all(|flow| flow.target_tx.is_some())
            }
        };

        identities_valid
            && pacing_valid
            && peers_valid
            && direction_valid
            && link_requirements_valid
            && matches!(self.completion, Completion::DurationMillis(duration) if (1..=300_000).contains(&duration))
    }
}

/// Data-plane readiness together with the exact link requirement proved by
/// the target before accepting measured traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReady {
    pub direction: Direction,
    pub tx_block_ack_tid: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureCapabilities {
    pub udp: bool,
    pub tcp: bool,
    pub rx: bool,
    pub tx: bool,
    pub bidirectional: bool,
    pub runtime_initialization: bool,
    pub runtime_configuration: bool,
    pub structured_evidence: bool,
    /// One UDP session can execute and account more than one peer flow.
    /// Merely carrying the bounded flow table on the wire does not imply this
    /// capability.
    pub udp_multi_flow: bool,
    /// This image accepts one opaque, host-owned startup artifact and can
    /// return its current value after initialization.
    pub startup_artifact: bool,
    /// This image can stop one healthy connected STA epoch at a safe runner
    /// boundary and use the returned owners to exercise reassociation.
    pub station_epoch_control: bool,
    /// This image exposes explicit role-neutral Wi-Fi lifecycle commands.
    pub wifi_role_control: bool,
    /// This image can materialize and stop the bounded WPA2-Personal access
    /// point role described by [`WifiAccessPointRequest`].
    pub wifi_access_point: bool,
    /// This image can materialize one same-channel station plus access point
    /// owner and expose both network endpoints at the same time.
    pub simultaneous_station_access_point: bool,
    /// This image can run one finite normalized monitor capture and export
    /// typed frame chunks without using UART text as a data protocol.
    pub wifi_monitor_capture: bool,
    /// This image reliably reports connected generations and proved peer-loss
    /// transitions independently of lossy text diagnostics.
    pub station_lifecycle_events: bool,
    /// This image installs driver-side value observers. Performance images
    /// leave the observation graph absent from the compiled datapath.
    pub driver_observation_evidence: bool,
    /// UDP RX sessions can return typed evidence for every delivery frontier
    /// from post-reorder publication through the application socket.
    pub rx_delivery_evidence: bool,
    /// This image instruments bounded Embassy task poll residence. Ordinary
    /// qualification images deliberately omit this timing perturbation.
    pub task_poll_evidence: bool,
    /// This image also links invasive alternative TX backing and
    /// materialization implementations for same-ELF A/B experiments. Such an
    /// image is not a production residence budget even when the runtime
    /// selects direct SRAM.
    pub tx_architecture_probe: bool,
    /// This image additionally instruments the intrusive Core0 RX phase and
    /// service histograms. It is separate from ordinary task-poll residence.
    pub core0_rx_cycle_evidence: bool,
    /// This image samples MAC interrupt publication timestamps in the hard
    /// ISR. Ordinary correctness and performance images keep that extended
    /// SRAM call graph absent.
    pub mac_irq_evidence: bool,
    /// Ordinary thread/task stacks live in external PSRAM while trap and CLIC
    /// interrupt contexts use dedicated per-hart internal-SRAM stacks.
    pub psram_task_stack: bool,
    /// This image publishes aggregate cooperative network scheduler evidence.
    pub network_scheduler_evidence: bool,
    /// Startup provisioning can select the data-plane executor topology
    /// without requiring another firmware image.
    pub data_plane_placement: bool,
    /// This image can compare Embassy alarm deadlines with the target's
    /// monotonic clock before radio initialization.
    pub timebase_probe: bool,
    /// This image can run the bounded IEEE 802.15.4 `EVENT_STATUS` observation
    /// probe and return its typed snapshots.
    pub ieee802154_event_status_probe: bool,
    /// This image can run the bounded ED-DONE/TIMER0 selective-write
    /// discriminator and retain RX-ABORT diagnostics.
    pub ieee802154_ed_event_probe: bool,
}

/// Bounded alarm/clock agreement probe. It is intentionally independent of
/// Wi-Fi initialization so a broken platform timer cannot qualify radio code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimebaseProbeRequest {
    pub intervals: u16,
    pub period_micros: u32,
}

impl TimebaseProbeRequest {
    pub const fn validate(self) -> bool {
        self.intervals >= 2
            && self.intervals <= 100
            && self.period_micros >= 1_000
            && self.period_micros <= 1_000_000
    }
}

/// Target-side timing evidence for one timebase probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimebaseProbeEvidence {
    pub intervals: u16,
    pub period_micros: u32,
    pub elapsed_micros: u64,
    pub minimum_interval_micros: u32,
    pub maximum_interval_micros: u32,
    pub early_intervals: u16,
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupArtifactChunkError {
    EmptyArtifact,
    EmptyChunk,
    ChunkTooLarge,
    Range,
}

impl fmt::Display for StartupArtifactChunkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArtifact => formatter.write_str("startup artifact must not be empty"),
            Self::EmptyChunk => formatter.write_str("startup artifact chunk must not be empty"),
            Self::ChunkTooLarge => formatter.write_str("startup artifact chunk is too large"),
            Self::Range => formatter.write_str("startup artifact chunk range is invalid"),
        }
    }
}

/// One ordered fragment of a target-defined, host-owned startup artifact.
///
/// The protocol deliberately assigns no meaning to the artifact bytes. The
/// selected target adapter owns the exact length, validation and conversion
/// into a typed runtime value. Chunks are contiguous and carry a digest of the
/// complete artifact so a receiver never treats a partial transfer as valid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartupArtifactChunk {
    total_length: u16,
    offset: u16,
    crc32c: u32,
    bytes: heapless::Vec<u8, STARTUP_ARTIFACT_CHUNK_MAX_LEN>,
}

impl StartupArtifactChunk {
    pub fn try_new(
        total_length: u16,
        offset: u16,
        crc32c: u32,
        bytes: &[u8],
    ) -> Result<Self, StartupArtifactChunkError> {
        let mut chunk = Self {
            total_length,
            offset,
            crc32c,
            bytes: heapless::Vec::new(),
        };
        chunk
            .bytes
            .extend_from_slice(bytes)
            .map_err(|_| StartupArtifactChunkError::ChunkTooLarge)?;
        chunk.validate()?;
        Ok(chunk)
    }

    pub fn validate(&self) -> Result<(), StartupArtifactChunkError> {
        if self.total_length == 0 {
            return Err(StartupArtifactChunkError::EmptyArtifact);
        }
        if self.bytes.is_empty() {
            return Err(StartupArtifactChunkError::EmptyChunk);
        }
        let end = usize::from(self.offset)
            .checked_add(self.bytes.len())
            .ok_or(StartupArtifactChunkError::Range)?;
        if end > usize::from(self.total_length) {
            return Err(StartupArtifactChunkError::Range);
        }
        Ok(())
    }

    pub const fn total_length(&self) -> u16 {
        self.total_length
    }

    pub const fn offset(&self) -> u16 {
        self.offset
    }

    pub const fn crc32c(&self) -> u32 {
        self.crc32c
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.total_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StartupArtifactDisposition {
    /// No retained artifact was supplied; initialization created one.
    Created,
    /// The supplied artifact was accepted and restored.
    Restored,
    /// The supplied artifact was rejected and full initialization replaced it.
    Replaced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartupArtifactStatus {
    pub disposition: StartupArtifactDisposition,
    pub total_length: u16,
    pub initialization_elapsed_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkCredentialsError {
    SsidLength,
    PassphraseLength,
}

impl fmt::Display for NetworkCredentialsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SsidLength => formatter.write_str("SSID must contain 1..=32 bytes"),
            Self::PassphraseLength => {
                formatter.write_str("WPA2 passphrase must contain 8..=63 bytes")
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkCredentials {
    ssid: [u8; WPA2_SSID_MAX_LEN],
    ssid_length: u8,
    passphrase: heapless::Vec<u8, WPA2_PASSPHRASE_MAX_LEN>,
}

impl NetworkCredentials {
    pub fn try_new(ssid: &[u8], passphrase: &[u8]) -> Result<Self, NetworkCredentialsError> {
        if ssid.is_empty() || ssid.len() > WPA2_SSID_MAX_LEN {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        let mut credentials = Self {
            ssid: [0; WPA2_SSID_MAX_LEN],
            ssid_length: ssid.len() as u8,
            passphrase: heapless::Vec::new(),
        };
        credentials.ssid[..ssid.len()].copy_from_slice(ssid);
        credentials
            .passphrase
            .extend_from_slice(passphrase)
            .map_err(|_| NetworkCredentialsError::PassphraseLength)?;
        Ok(credentials)
    }

    pub fn validate(&self) -> Result<(), NetworkCredentialsError> {
        let ssid_length = usize::from(self.ssid_length);
        if ssid_length == 0 || ssid_length > self.ssid.len() {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&self.passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        Ok(())
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_length)]
    }

    pub fn passphrase(&self) -> &[u8] {
        self.passphrase.as_slice()
    }

    pub fn clear_passphrase(&mut self) {
        self.passphrase.as_mut_slice().zeroize();
        self.passphrase.clear();
    }
}

impl fmt::Debug for NetworkCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkCredentials")
            .field("ssid_length", &self.ssid_length)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

impl Drop for NetworkCredentials {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.ssid_length = 0;
        self.clear_passphrase();
    }
}

/// IPv4 policy selected by the host for this boot.
///
/// Keeping this in startup provisioning lets one qualified firmware image run
/// against both an ordinary DHCP network and an isolated HIL access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NetworkIpv4Configuration {
    Dhcp,
    Static {
        address: [u8; 4],
        prefix_length: u8,
        gateway: Option<[u8; 4]>,
    },
}

impl NetworkIpv4Configuration {
    pub fn validate(self) -> bool {
        match self {
            Self::Dhcp => true,
            Self::Static {
                address,
                prefix_length,
                gateway,
            } => {
                prefix_length <= 32
                    && address != [0, 0, 0, 0]
                    && address != [255, 255, 255, 255]
                    && match gateway {
                        Some(gateway) => gateway != [0, 0, 0, 0] && gateway != [255, 255, 255, 255],
                        None => true,
                    }
            }
        }
    }
}

/// Executor placement selected once, before any Wi-Fi worker or IP stack is
/// materialized. Radio and RX protocol ownership remain on CPU0 in both modes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiDataPlanePlacement {
    /// Radio, RX protocol, IP stack and socket workloads share CPU0.
    SingleCore,
    /// Only the IP stack and socket workloads move to CPU1.
    #[default]
    SplitRadioNetwork,
}

/// IPv4/UDP receive checksum policy selected before the network stack starts.
///
/// The diagnostic variant exists only for a same-image HIL cost experiment;
/// it is not a claim that the Wi-Fi MAC performs transport checksum offload.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxChecksumPolicy {
    /// Validate IPv4 and UDP receive checksums in the software IP stack.
    #[default]
    Software,
    /// Trust the isolated HIL traffic generator and skip IPv4/UDP RX checks.
    AssumeValidDiagnostic,
}

/// IPv4 UDP transmit checksum policy selected before the network stack starts.
///
/// IPv4 permits a zero UDP checksum. The diagnostic variant uses that wire
/// representation to isolate software checksum cost without claiming hardware
/// offload or disabling the mandatory IPv4 header checksum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxUdpChecksumPolicy {
    /// Generate the IPv4 UDP checksum in the software IP stack.
    #[default]
    Software,
    /// Emit a zero IPv4 UDP checksum for a same-image HIL cost experiment.
    OmitIpv4Diagnostic,
}

/// Backing selected for one same-image TX ownership-boundary experiment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxBufferPolicy {
    /// Let the network stack format directly into the final DMA-visible slot.
    #[default]
    DirectDma,
    /// Same direct-DMA owner path, but expose ordinary FIFO egress to Xarxa.
    /// This is a same-ELF control for the resolved-egress scheduler; it is not
    /// a supported production policy.
    DirectDmaFifoDiagnostic,
    /// Keep resolved-egress selection but retain socket-originated network
    /// wakes while the cooperative runner is already waiting for a physical
    /// TX credit. This is a same-image control for wake suppression only.
    DirectDmaWakeStormControlDiagnostic,
    /// Keep resolved-egress selection and wake suppression, but return from
    /// Xarxa after every one packet. This is a same-image control for bounded
    /// multi-packet socket dispatch.
    DirectDmaSingleDispatchControlDiagnostic,
    /// Keep the complete keyed direct-SRAM path but disable only the affine
    /// lifecycle-demand mirror. This is a same-ELF CPU-cost control.
    DirectDmaEgressControlDisabledDiagnostic,
    /// Keep the authoritative egress-control path but disable only the
    /// per-frame AP egress-identity observer. This is a same-ELF Core0
    /// attribution control and does not change admission or radio metadata.
    DirectDmaEgressIdentityObservationDisabledDiagnostic,
    /// Keep direct DMA backing, but have the HIL producer publish one bounded
    /// destination-homogeneous burst at a time. This isolates packet-selection
    /// order from physical SRAM capacity without changing the radio datapath.
    DirectDmaEgressBurstDiagnostic,
    /// Format in ordinary task memory, then copy once into the final DMA slot.
    /// This measures materialization cost but does not yet add software queues.
    PsramStagingCopyDiagnostic,
    /// Keep driver-side peer/TID scheduling, but execute the selected
    /// PSRAM-to-SRAM batch materialization in the network-core driver poll.
    Core1MaterializationDiagnostic,
    /// Keep Wi-Fi descriptors in internal SRAM but publish PSRAM packet-buffer
    /// addresses after an explicit cache writeback. Hardware support is not
    /// assumed; this value exists only for the bounded DMA-address HIL.
    PsramDirectDmaDiagnostic,
}

/// Ordinary shared-RX admission selected before the radio stack starts.
///
/// Both variants live in one diagnostic ELF. The deferred variant preserves
/// the former immediately-ready async edge solely to measure its cost without
/// changing code layout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxAdmissionPolicy {
    /// The retained staging slot is the complete network publication credit.
    #[default]
    SynchronousShared,
    /// Poll an immediately-ready future before publishing the same slot.
    DeferredReadyDiagnostic,
}

/// Core0 dispatch policy selected before the radio stack starts.
///
/// Both variants are linked into one coarse diagnostic image. The direct
/// variant changes only processing of an already-completed, in-order ordinary
/// data frame; DMA polling cadence and the outer cooperative budget remain
/// identical.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxDispatchPolicy {
    /// Preserve the general asynchronous protocol dispatch path.
    #[default]
    Asynchronous,
    /// Use synchronous run-to-completion for the qualified immediate BA case.
    DirectImmediateDiagnostic,
}

/// Continuation policy for a masked RX drain epoch in one coarse image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxContinuationPolicy {
    /// Preserve the production immediate software repost.
    #[default]
    ImmediateSoftwareProbe,
    /// Restore the level-triggered source after a recycled-only turn.
    LevelIrqDiagnostic,
    /// Retain source masking and repoll after 64 microseconds.
    DelayedProbe64Diagnostic,
    /// Retain source masking and repoll after 128 microseconds.
    DelayedProbe128Diagnostic,
    /// Retain source masking and repoll after 256 microseconds.
    DelayedProbe256Diagnostic,
    /// Retain source masking and repoll after 512 microseconds.
    DelayedProbe512Diagnostic,
    /// Retain source masking and repoll after 1024 microseconds.
    DelayedProbe1024Diagnostic,
    /// Select a bounded window from the completed physical batch geometry.
    AdaptiveProbeDiagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitializationConfiguration {
    pub ipv4: NetworkIpv4Configuration,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_admission: WifiRxAdmissionPolicy,
    pub rx_dispatch: WifiRxDispatchPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
}

impl InitializationConfiguration {
    pub fn validate(self) -> bool {
        self.ipv4.validate()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub features: FeatureCapabilities,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Command {
    GetCapabilities,
    /// Return the boot-lifetime CPU stack high-water marks. This diagnostic
    /// query is valid only outside an active traffic session.
    QueryStackUsage,
    /// Return boot-lifetime transport and serialized-text health counters.
    QueryLinkHealth,
    /// Compare alarm deadlines with the monotonic clock before initializing
    /// the radio/network runtime.
    ProbeTimebase(TimebaseProbeRequest),
    /// Run one bounded, observation-only IEEE 802.15.4 `EVENT_STATUS` probe.
    ProbeIeee802154EventStatus(Ieee802154EventStatusProbeRequest),
    /// Run the bounded ED-DONE/TIMER0 selective-write discriminator.
    ProbeIeee802154EdEvent(Ieee802154EdEventProbeRequest),
    UploadStartupArtifact(StartupArtifactChunk),
    /// Initialize calibration and the network stack without materializing a
    /// Wi-Fi role. This command is accepted exactly once per boot.
    Initialize(InitializationConfiguration),
    Configure(SessionConfig),
    Arm,
    Start,
    /// Request one connected STA teardown/reassociation cycle. This is a HIL
    /// lifecycle operation, not a transport-session stop and not evidence of
    /// peer link loss.
    CycleStationEpoch,
    /// Stop the active station and return to role-neutral Wi-Fi ownership.
    StopStation,
    /// Materialize a station from the role-neutral Wi-Fi owner.
    StartStation(NetworkCredentials),
    /// Run one finite standalone scan and return to role-neutral ownership.
    ScanWifi(WifiScanRequest),
    /// Materialize a standalone monitor from the role-neutral Wi-Fi owner.
    StartMonitor(WifiMonitorRequest),
    /// Stop the active monitor and return to role-neutral Wi-Fi ownership.
    StopMonitor,
    /// Materialize one bounded WPA2-Personal access point from role-neutral
    /// Wi-Fi ownership. Credentials belong to this AP epoch and are cleared
    /// when the command value is dropped.
    StartAccessPoint(WifiAccessPointRequest),
    /// Stop the active access point and return to role-neutral Wi-Fi ownership.
    StopAccessPoint,
    /// Materialize one same-channel upstream station plus downstream SoftAP.
    StartStationAccessPoint(WifiStationAccessPointRequest),
    /// Stop both paired roles and return every physical owner to idle.
    StopStationAccessPoint,
    /// Run one finite monitor epoch, export its captured frames, return to
    /// idle and publish a terminal capture summary.
    CaptureMonitor(WifiMonitorCaptureRequest),
    /// Return the current operation state and retained session identities.
    GetStatus,
    /// Cancel a configured or armed session before any workload starts.
    Cancel,
    /// Replay the retained evidence for a completed session.
    ReplayResult,
    /// Explicitly discard a terminal result and return to idle ownership.
    Recover,
    AcknowledgeResult,
}

impl WireBody for Command {
    const WIRE_KIND: WireKind = WireKind::Command;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionState {
    Booting,
    WaitingForInitialization,
    Idle,
    Configured,
    Armed,
    Running,
    Draining,
    Finished,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateChange {
    pub previous: SessionState,
    pub current: SessionState,
}

/// Query result used to recover after an uncertain UART response without
/// guessing whether a session was configured, started or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationStatus {
    pub state: SessionState,
    pub configured_session_id: Option<u64>,
    pub completed_session_id: Option<u64>,
}

/// Target-observed ownership edges for one requested station epoch cycle.
///
/// This is deliberately semantic rather than target-specific: the target
/// adapter may implement the individual operations differently, but it may
/// publish completion only after every owned resource crossed these four
/// finite boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationEpochEvidence {
    pub runner_stopped: bool,
    pub scan_owners_returned: bool,
    pub join_completed: bool,
    pub wdev_runner_started: bool,
}

/// Wi-Fi role represented by the target-side application owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRole {
    Idle,
    Station,
    Monitor,
    AccessPoint,
    StationAccessPoint,
}

/// Logical network endpoint backed by the shared physical Wi-Fi owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiNetworkInterface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRoleOperation {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRoleFailureReason {
    Rejected,
    HardwareFault,
    GenerationMismatch,
}

/// Terminal correlated failure for a role command. This prevents the host
/// from turning a target-side ownership fault into an opaque timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiRoleFailureEvidence {
    pub role: WifiRole,
    pub operation: WifiRoleOperation,
    pub reason: WifiRoleFailureReason,
}

/// Complete target-neutral configuration for the first AP role.
///
/// Beacon interval (100 TU) and DTIM period (2) are driver guarantees. Channel
/// width and client admission remain explicit test inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiChannelWidth {
    Mhz20,
    Mhz40Above,
    Mhz40Below,
}

impl WifiChannelWidth {
    pub const fn bandwidth_mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40Above | Self::Mhz40Below => 40,
        }
    }

    pub const fn admits_primary(self, channel: u8) -> bool {
        match self {
            Self::Mhz20 => channel >= 1 && channel <= 13,
            Self::Mhz40Above => channel >= 1 && channel <= 9,
            Self::Mhz40Below => channel >= 5 && channel <= 13,
        }
    }
}

/// Security mode selected for one explicitly started access-point epoch.
///
/// Keeping the choice on the wire lets one firmware image perform causal
/// Open/WPA2 comparisons instead of comparing feature-dependent ELF layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiAccessPointSecurity {
    Open,
    Wpa2Personal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiAccessPointRequest {
    pub credentials: NetworkCredentials,
    pub security: WifiAccessPointSecurity,
    pub channel: u8,
    pub channel_width: WifiChannelWidth,
    pub client_limit: u8,
    /// IP configuration owned by the HIL application while the AP role is
    /// active. This is deliberately outside the radio-driver request.
    pub ipv4: NetworkIpv4Configuration,
}

/// One upstream station plus one same-channel downstream SoftAP request.
///
/// The AP channel is explicit and the production driver rejects the complete
/// request unless the upstream association negotiates exactly that channel
/// and width. The HIL layer does not add a channel-switching fallback.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiStationAccessPointRequest {
    pub station_credentials: NetworkCredentials,
    pub access_point: WifiAccessPointRequest,
}

impl WifiStationAccessPointRequest {
    pub fn validate(&self) -> Result<(), WifiAccessPointRequestError> {
        self.station_credentials
            .validate()
            .map_err(WifiAccessPointRequestError::Credentials)?;
        self.access_point.validate()
    }
}

impl WifiAccessPointRequest {
    pub fn validate(&self) -> Result<(), WifiAccessPointRequestError> {
        self.credentials
            .validate()
            .map_err(WifiAccessPointRequestError::Credentials)?;
        if !self.channel_width.admits_primary(self.channel) {
            return Err(WifiAccessPointRequestError::Channel);
        }
        if !(1..=15).contains(&self.client_limit) {
            return Err(WifiAccessPointRequestError::ClientLimit);
        }
        if !matches!(
            self.ipv4,
            NetworkIpv4Configuration::Static { gateway: None, .. }
        ) || !self.ipv4.validate()
        {
            return Err(WifiAccessPointRequestError::Ipv4);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiAccessPointRequestError {
    Credentials(NetworkCredentialsError),
    Channel,
    ClientLimit,
    Ipv4,
}

impl fmt::Display for WifiAccessPointRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(error) => error.fmt(formatter),
            Self::Channel => formatter
                .write_str("AP primary channel and secondary-channel geometry are inconsistent"),
            Self::ClientLimit => formatter.write_str("AP client limit must be in 1..=15"),
            Self::Ipv4 => formatter
                .write_str("AP mode requires a valid gateway-free static IPv4 configuration"),
        }
    }
}

/// Compact, target-neutral standalone scan request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiScanRequest {
    /// Bit zero selects channel 1 and bit twelve selects channel 13.
    pub channel_mask_2_4_ghz: u16,
    pub dwell_millis: u16,
}

/// Compact, target-neutral standalone monitor request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorRequest {
    pub channel: u8,
    /// Zero retains the complete frame; a nonzero value truncates captures.
    pub snapshot_length: u16,
}

/// Finite typed monitor export request. Duration is owned by the target so a
/// congested serial link cannot postpone the role stop indefinitely.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorCaptureRequest {
    pub channel: u8,
    pub snapshot_length: u16,
    pub duration_millis: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiMonitorEvidenceSource {
    Hardware,
    Protocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorObserved<T> {
    pub source: WifiMonitorEvidenceSource,
    pub value: T,
}

/// Typed transport form of the ESP32-S31 receive vector. The raw hardware
/// rate code remains explicitly scoped to its decoded PHY format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorPhyEvidence {
    pub format: WifiMonitorPhyFormat,
    pub hardware_rate_code: u8,
    pub he_siga1: u32,
    pub he_siga2: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiMonitorPhyFormat {
    Dot11b,
    Ofdm,
    Ht,
    Vht,
    HeSu,
    HeMu,
    HeExtendedRangeSu,
    HeTriggerBased,
    VhtMu,
    Unknown(u8),
}

/// One independently checksummed piece of one captured normalized MPDU.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorFrameChunk {
    pub generation: u32,
    pub frame_sequence: u32,
    pub dequeued_at_micros: u64,
    pub captured_length: u16,
    pub logical_length: u16,
    pub offset: u16,
    pub channel: Option<WifiMonitorObserved<u8>>,
    pub rssi_dbm: Option<WifiMonitorObserved<i8>>,
    pub rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
    bytes: heapless::Vec<u8, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN>,
}

impl WifiMonitorFrameChunk {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        generation: u32,
        frame_sequence: u32,
        dequeued_at_micros: u64,
        captured_length: u16,
        logical_length: u16,
        offset: u16,
        channel: Option<WifiMonitorObserved<u8>>,
        rssi_dbm: Option<WifiMonitorObserved<i8>>,
        rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
        bytes: &[u8],
    ) -> Result<Self, WifiMonitorFrameChunkError> {
        if captured_length == 0 || bytes.is_empty() {
            return Err(WifiMonitorFrameChunkError::Empty);
        }
        let end = usize::from(offset)
            .checked_add(bytes.len())
            .ok_or(WifiMonitorFrameChunkError::Range)?;
        if end > usize::from(captured_length) {
            return Err(WifiMonitorFrameChunkError::Range);
        }
        let mut body = heapless::Vec::new();
        body.extend_from_slice(bytes)
            .map_err(|_| WifiMonitorFrameChunkError::TooLarge)?;
        Ok(Self {
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes: body,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.captured_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiMonitorFrameChunkError {
    Empty,
    TooLarge,
    Range,
}

/// Completion of one explicit role ownership transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiRoleTransitionEvidence {
    pub previous: WifiRole,
    pub current: WifiRole,
    pub generation: u32,
}

/// Bounded scan evidence. The complete BSS table stays in the driver API and
/// is intentionally not expanded to fit the UART protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiScanEvidence {
    pub generation: u32,
    pub elapsed_micros: u64,
    pub observed_frames: u32,
    pub unique_bss: u8,
    pub dropped_unique_bss: u32,
    pub configured_ssid_found: bool,
    pub configured_ssid_channel: u8,
    pub configured_ssid_rssi_dbm: i8,
}

/// Observations retained by the HIL consumer during one monitor epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorEvidence {
    pub generation: u32,
    pub elapsed_micros: u64,
    pub channel: u8,
    pub captured_frames: u32,
    pub captured_bytes: u64,
    pub generation_mismatches: u32,
    /// Frames whose RX metadata explicitly named another channel.
    pub channel_mismatches: u32,
    /// Frames for which the backend did not expose per-frame channel data.
    pub channel_unavailable: u32,
    /// Most recent explicit hardware/protocol channel, or zero if unavailable.
    pub last_observed_channel: u8,
    /// Capture-pool publication counters for this exact generation.
    pub published_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub discarded_frames: u32,
    /// Frames whose complete ordered chunk set was admitted to the protocol
    /// queue before this terminal evidence.
    pub exported_frames: u32,
}

/// Complete interval delta of the MAC/baseband RX statistics made available
/// by the target. Counter names whose hardware meaning is not yet established
/// remain explicitly vendor-shaped instead of being assigned a false cause.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMacRxHardwareEvidence {
    pub mpdu_count: u16,
    pub data_success: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bluetooth_block_error: u16,
    pub frequency_hop_error: u16,
    /// Vendor-named terminal RX state counter. Its exact semantic meaning is
    /// intentionally not inferred by the target-neutral HIL protocol.
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
    pub brx_agc_error: u16,
    pub brx_error: u16,
    pub nrx_error: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
    pub rx_hang: u8,
    pub tx_hang: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

/// Bounded evidence retained for one access-point ownership epoch.
///
/// These counters describe MAC/runtime work only. IP services and host-side
/// client observations belong to the qualification report, not the driver.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiAccessPointEvidence {
    pub generation: u32,
    pub channel: u8,
    pub bandwidth_mhz: u16,
    pub beacons_transmitted: u32,
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    /// Longest network data transaction; excludes chained AP control frames.
    pub maximum_network_tx_pending_micros: u32,
    pub network_tx_attempts_at_maximum_pending: u8,
    pub maximum_rx_service_micros: u32,
    pub maximum_rx_dma_service_micros: u32,
    pub total_rx_dma_service_micros: u32,
    pub rx_dma_service_calls: u32,
    pub maximum_rx_protocol_service_micros: u32,
    pub maximum_rx_protected_data_service_micros: u32,
    pub total_rx_protected_data_service_micros: u32,
    pub maximum_rx_management_service_micros: u32,
    pub maximum_rx_eapol_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub authentication_responses: u32,
    pub association_responses: u32,
    /// Successful controlled-port openings, including re-authorizations.
    pub authorized_peers: u32,
    /// Maximum number of peers admitted at the same time.
    pub maximum_associated_peers: u8,
    /// Maximum number of controlled ports open at the same time.
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
    pub authentication_timeouts: u32,
    pub wpa2_response_windows: u32,
    pub wpa2_pending_on_stop: u32,
    pub wpa2_retransmissions: u32,
    pub wpa2_handshake_failures: u32,
    pub wpa2_handshake_timeouts: u32,
    pub inactivity_timeouts: u32,
    pub disassociations_prepared: u32,
    /// Disconnect frames accepted by the target hardware TX owner.
    pub disassociations_published: u32,
    /// Published disconnect frames whose terminal completion reported an ACK.
    pub disassociations_acknowledged: u32,
    pub deauthentications_prepared: u32,
    pub deauthentications_published: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
    /// Peer-originated RX ADDBA responses completed by the AP hardware owner.
    pub rx_block_ack_responses_transmitted: u32,
    /// Complete vendor-shaped RX units made visible to the AP protocol path.
    pub completed_rx_units: u32,
    pub completed_rx_descriptors: u32,
    /// Descriptors safely rearmed and returned to DMA during the live epoch.
    pub recycled_rx_descriptors: u32,
    /// Hardware MAC/baseband counter increments across the complete AP epoch.
    pub rx_hardware: WifiMacRxHardwareEvidence,
    /// Completed descriptors retained until the walker is stopped because a
    /// later completion was still serving as their generation guard.
    pub retained_rx_descriptors: u32,
    /// Complete units intentionally discarded after their payload was observed.
    pub discarded_rx_units: u32,
    /// Bulk protected units discarded after upper-copy saturation while DMA
    /// ownership was recycled immediately.
    pub rx_overload_discarded_units: u32,
    pub rx_critical_reserve_admissions: u32,
    pub rx_critical_admission_blocked: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    /// Protected HT data MPDUs observed by the target RX boundary.
    pub rx_ht_data_frames: u32,
    /// Protected HT MPDUs whose copied HT-SIG Aggregation bit is set.
    /// The descriptor contract does not expose PPDU boundaries or depth.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    pub rx_rssi_samples: u32,
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    /// Protected HT40 data MPDUs grouped by hardware-observed MCS0..MCS7.
    pub rx_ht40_mcs_frames: [u32; 8],
    /// Protected HT40 data MPDUs observed with the 800 ns guard interval.
    pub rx_ht40_long_gi_frames: u32,
    /// Protected HT40 data MPDUs observed with the 400 ns guard interval.
    pub rx_ht40_short_gi_frames: u32,
    /// AP network A-MPDU transactions started with a typed HT vector.
    pub tx_ht_aggregates: u32,
    /// AP network A-MPDU transactions started with HT40 MCS7.
    pub tx_ht40_mcs7_aggregates: u32,
    pub data_frames_transmitted: u32,
    /// Total hardware publications for data MPDUs, including retries.
    pub data_tx_attempts: u32,
    /// Data MPDUs which required more than one hardware publication.
    pub data_tx_retried_frames: u32,
    pub data_tx_maximum_attempts: u8,
    /// Lowest terminal legacy/HT rate observed after any retry ladder.
    pub data_tx_minimum_final_rate_kbps: u32,
    pub data_tx_ack_snr_samples: u32,
    pub data_tx_minimum_ack_snr_db: i8,
    pub data_tx_maximum_ack_snr_db: i8,
    pub tx_ack_timeout_retries: u32,
    pub tx_cts_timeout_retries: u32,
    pub tx_collision_retries: u32,
    pub tx_hardware_failures: u8,
    pub tx_hardware_timeouts: u8,
    pub tx_collision_limits: u8,
    pub tx_last_hardware_status: u8,
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub rx_reorder_buffered_mpdus: u32,
    pub rx_reorder_dispatched_mpdus: u32,
    pub rx_reorder_hardware_window_resets: u32,
    pub rx_reorder_gap_timeouts: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
}

/// Terminal evidence for one explicit same-channel STA+AP stop transaction.
///
/// The role transition proves that every shared physical owner returned to
/// idle; the AP report preserves beacon and fairness-relevant observations
/// from that exact paired generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiStationAccessPointStopEvidence {
    pub transition: WifiRoleTransitionEvidence,
    pub access_point: WifiAccessPointEvidence,
}

impl StationEpochEvidence {
    pub const COMPLETE: Self = Self {
        runner_stopped: true,
        scan_owners_returned: true,
        join_completed: true,
        wdev_runner_started: true,
    };

    pub const fn is_complete(self) -> bool {
        self.runner_stopped
            && self.scan_owners_returned
            && self.join_completed
            && self.wdev_runner_started
    }
}

/// Why a connected station generation returned to candidate selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationDisconnectReason {
    /// The connected beacon monitor proved that the selected AP disappeared.
    BeaconLoss,
    /// The AP sent an IEEE 802.11 deauthentication frame.
    PeerDeauthentication { reason_code: u16 },
    /// The AP sent an IEEE 802.11 disassociation frame.
    PeerDisassociation { reason_code: u16 },
    /// Restoring active power-management state failed at the peer-visible TX edge.
    ActiveStateRestoreFailed,
    /// Connected-state WPA2 Group Key Handshake failed closed.
    GroupKeyHandshakeFailed,
    /// Another connected link policy returned the peer owner without claiming
    /// a beacon deadline; this must not qualify an AP-loss test.
    LinkPolicy,
    /// The host/application requested a healthy connected-epoch cycle.
    ReconnectRequested,
    /// The bounded connected-control mailbox overflowed, so the event stream
    /// can no longer be processed as complete.
    ///
    /// Kept at the end so the discriminants of the existing wire vocabulary
    /// remain stable.
    ControlMailboxOverflow,
}

/// Stable station stage vocabulary used by HIL lifecycle evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationFailureStage {
    CandidateSelection,
    Authentication,
    Association,
    Security,
    Connected,
    Hardware,
}

/// Target-independent classification of a failed station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationAttemptFailureReason {
    /// A complete candidate scan did not find the configured network.
    NoCandidate,
    /// A finite peer protocol exchange failed or timed out.
    PeerProtocol,
    /// Hardware ownership or a bounded hardware transaction failed.
    Hardware,
    /// The adapter observed an impossible production ownership contract.
    ContractViolation,
}

/// Reliable target-observed station lifecycle edge.
///
/// Generation zero is the initial connection. The outer lifecycle increments
/// the generation only after a connected epoch returns its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationLifecycleEvent {
    Connected {
        generation: u32,
    },
    Disconnected {
        generation: u32,
        reason: StationDisconnectReason,
    },
    /// One complete attempt returned every owner and was classified for retry.
    AttemptFailed {
        generation: u32,
        attempt: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
    /// The bounded reconnect policy returned the final owner without another
    /// hidden attempt or backoff.
    RetryExhausted {
        generation: u32,
        attempts: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub network_interface: WifiNetworkInterface,
    pub address: [u8; 4],
    pub prefix_length: u8,
    pub gateway: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub network_interface: WifiNetworkInterface,
    pub transport: Transport,
    pub direction: Direction,
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RejectReason {
    ProtocolVersion,
    BootId,
    SessionId,
    InvalidState,
    InvalidConfiguration,
    Unsupported,
    Busy,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FailureCode {
    Configuration,
    Network,
    Transport,
    Timeout,
    EvidenceOverflow,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TransportEvidence {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

/// Transport accounting for one configured [`SessionFlowConfig`].
///
/// The session-wide [`TransportEvidence`] remains the independently checked
/// sum used by existing ceiling reports. Fairness verdicts consume these
/// records and must never infer a peer split from the aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlowTransportEvidence {
    pub flow_id: u8,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

impl FlowTransportEvidence {
    pub const fn from_session_total(flow_id: u8, total: TransportEvidence) -> Self {
        Self {
            flow_id,
            rx_bytes: total.rx_bytes,
            tx_bytes: total.tx_bytes,
            rx_units: total.rx_units,
            tx_units: total.tx_units,
            elapsed_micros: total.elapsed_micros,
            transport_errors: total.transport_errors,
        }
    }

    pub const fn as_session_total(self) -> TransportEvidence {
        TransportEvidence {
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
            rx_units: self.rx_units,
            tx_units: self.tx_units,
            elapsed_micros: self.elapsed_micros,
            transport_errors: self.transport_errors,
        }
    }
}

impl TransportEvidence {
    pub fn from_flows(flows: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY]) -> Self {
        flows.iter().flatten().copied().fold(
            Self {
                rx_bytes: 0,
                tx_bytes: 0,
                rx_units: 0,
                tx_units: 0,
                elapsed_micros: 0,
                transport_errors: 0,
            },
            |mut total, flow| {
                total.rx_bytes = total.rx_bytes.saturating_add(flow.rx_bytes);
                total.tx_bytes = total.tx_bytes.saturating_add(flow.tx_bytes);
                total.rx_units = total.rx_units.saturating_add(flow.rx_units);
                total.tx_units = total.tx_units.saturating_add(flow.tx_units);
                total.elapsed_micros = total.elapsed_micros.max(flow.elapsed_micros);
                total.transport_errors =
                    total.transport_errors.saturating_add(flow.transport_errors);
                total
            },
        )
    }
}

/// Minimum typed radio evidence needed by ordinary UDP qualification. Richer
/// timing histograms remain diagnostic telemetry and never decide PASS/FAIL.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RadioEvidence {
    pub rx: Option<RxRadioEvidence>,
    pub tx: Option<TxRadioEvidence>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxRadioEvidence {
    /// ESP hardware RX format code observed for the measured interval.
    pub phy_format: u8,
    /// Complete benchmark-UDP HT40 observations at 800 ns GI.
    pub ht40_long_gi_frames: u32,
    /// Complete benchmark-UDP HT40 observations at 400 ns GI.
    pub ht40_short_gi_frames: u32,
    /// HT40 observations below MCS7. This is a subset of the two GI totals.
    pub ht40_below_mcs7_frames: u32,
    /// Benchmark vectors outside HT40 MCS0..7 geometry/format.
    pub ht_invalid_frames: u32,
    pub dma_buffer_full: u32,
    pub dma_fifo_overflow: u32,
    pub network_dropped: u32,
    pub irq_drain_saturated: u32,
    pub unhandled_irq_entries: u32,
    pub sequence_first: Option<u32>,
    pub sequence_highest: Option<u32>,
    pub sequence_gap_events: u32,
    pub sequence_forward_missing: u32,
    pub sequence_backward: u32,
    pub sequence_duplicates: u32,
    pub sequence_unsequenced: u32,
    pub s_mpdu_datagrams: u32,
    pub not_s_mpdu_datagrams: u32,
    pub s_mpdu_unavailable_datagrams: u32,
    pub s_mpdu_beacons: u32,
    pub not_s_mpdu_beacons: u32,
    pub s_mpdu_unavailable_beacons: u32,
    pub ampdu_datagrams: u32,
    pub not_ampdu_datagrams: u32,
    pub hardware_ampdu_datagrams: u32,
    pub hardware_not_ampdu_datagrams: u32,
    pub protocol_ampdu_datagrams: u32,
    pub protocol_not_ampdu_datagrams: u32,
    pub ampdu_unavailable_datagrams: u32,
    pub reorder_tid: u8,
    pub reorder_window: u16,
    pub reorder_first_samples: u32,
    pub reorder_first_tid: u8,
    pub reorder_first_start: u16,
    pub reorder_first_sequence: u16,
    pub reorder_first_distance: u16,
    pub reorder_current_occupied: u32,
    pub reorder_maximum_occupied: u32,
    pub rx_service_calls: u32,
    pub rx_frontier_histogram_samples: u32,
    pub mac_irq_entries: u32,
    pub mac_irq_classified_entries: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TxRadioEvidence {
    pub bandwidth_mhz: u16,
    pub aggregate_rate_kbps: u32,
    pub aggregates_prepared: u32,
    pub aggregate_publications: u32,
    pub aggregates_completed: u32,
    pub subframes_prepared: u32,
    pub subframes_acknowledged: u32,
    pub individual_retries: u32,
    pub hardware_timeouts: u32,
    pub collisions: u32,
    pub minimum_subframes: u8,
    pub maximum_subframes: u8,
    pub prepared_histogram: [u32; 8],
    pub stopped_at_frame_limit: u32,
    pub stopped_at_capacity_limit: u32,
    pub stopped_on_empty_queue: u32,
    pub block_ack_samples: u32,
    /// Publications for which hardware reported a physically received
    /// BlockAck frame. This is independent of bitmap coverage: a received
    /// BlockAck may acknowledge zero subframes.
    pub block_ack_received: u32,
    pub success_without_block_ack: u32,
    pub nonzero_block_ack_control: u32,
    /// BlockAck-processing samples classified by acknowledged bitmap
    /// coverage. `full + partial + empty == block_ack_samples`; `empty` also
    /// includes publications for which no BlockAck frame was received.
    pub full_block_ack: u32,
    pub partial_block_ack: u32,
    pub empty_block_ack: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_publication_to_irq_samples: u32,
}

/// Timing and software-pipeline evidence for one bounded aggregate-TX interval.
///
/// This is separate from [`TxRadioEvidence`]: radio correctness and timing are
/// independently complete protocol records, and neither is reconstructed from
/// best-effort UART text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TxAggregateTimingEvidence {
    pub preparation_micros: u32,
    pub preparation_max_micros: u32,
    pub publication_micros: u32,
    pub publication_max_micros: u32,
    pub exchange_micros: u32,
    pub exchange_max_micros: u32,
    pub first_exchanges: u32,
    pub first_exchange_micros: u32,
    pub first_exchange_max_micros: u32,
    pub retried_exchanges: u32,
    pub retry_publications: u32,
    pub retry_exchange_micros: u32,
    pub retry_exchange_max_micros: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_irq_service_micros: u32,
    pub tx_irq_service_max_micros: u32,
    pub tx_publication_to_irq_samples: u32,
    pub tx_publication_to_irq_micros: u32,
    pub tx_publication_to_irq_max_micros: u32,
    pub standby_prepared: u32,
    pub standby_published: u32,
    pub standby_cancelled: u32,
}

/// Sequence evidence collected at one finite UDP RX delivery stage.
///
/// Qualification traffic uses non-negative, non-wrapping `i32` sequence
/// numbers. Negative control markers are counted separately and never enter
/// the data-unit accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxSequenceStageEvidence {
    pub data_units: u32,
    pub first: Option<u32>,
    pub highest: Option<u32>,
    pub gap_events: u32,
    pub forward_missing: u32,
    pub late_recovered: u32,
    pub duplicates: u32,
    pub backward_unclassified: u32,
    pub first_anomaly: Option<u32>,
    pub control_markers: u32,
    pub data_after_terminal: u32,
}

/// Exact reconciliation of successful network admissions with UDP socket
/// consumption through a bounded qualification-only shadow ledger.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxConsumerLedgerEvidence {
    pub matched: u32,
    pub enqueued_not_consumed: u32,
    pub skipped_before_observed: u32,
    pub unexpected_consumer: u32,
    pub overflow: u32,
    pub first_expected: Option<u32>,
    pub first_observed: Option<u32>,
}

/// Correlation of application-level ordering defects with public QoS MAC
/// sequence/TID progression at the post-reorder frontier.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxMacOrderEvidence {
    pub backward_mac_backward: u32,
    pub backward_mac_same: u32,
    pub backward_mac_forward: u32,
    pub backward_mac_other_tid: u32,
    pub backward_mac_unavailable: u32,
}

/// Reorder decisions relevant to delivery loss during one session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxReorderDeliveryEvidence {
    pub ingress: u32,
    pub ingress_retries: u32,
    pub direct: u32,
    pub buffered: u32,
    pub released: u32,
    pub missing: u32,
    pub stale: u32,
    pub gap_expiries: u32,
    pub maximum_occupied: u32,
    pub discarded: u32,
}

/// Complete typed evidence for the three UDP RX delivery frontiers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxDeliveryEvidence {
    pub post_reorder: RxSequenceStageEvidence,
    pub network_enqueued: RxSequenceStageEvidence,
    pub udp_consumer: RxSequenceStageEvidence,
    pub consumer_ledger: RxConsumerLedgerEvidence,
    pub mac_order: RxMacOrderEvidence,
    pub reorder: RxReorderDeliveryEvidence,
    pub network_queue_full: u32,
    pub network_invalid_length: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkHealth {
    pub rx_frames: u32,
    pub rx_cobs_errors: u32,
    pub rx_checksum_errors: u32,
    pub rx_decode_errors: u32,
    pub rx_overflows: u32,
    pub tx_frames: u32,
    pub tx_dropped: u32,
    pub text_dropped: u32,
    pub text_truncated: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StackWatermark {
    pub capacity_bytes: u32,
    pub free_bytes: u32,
    pub used_bytes: u32,
    pub minimum_free_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StackUsage {
    pub cpu0: StackWatermark,
    pub cpu1: StackWatermark,
}

/// Aggregate cooperative network scheduler evidence collected since boot.
///
/// Diagnostic images are cold-booted for each qualification cell, so this is
/// also the complete scheduler interval for that cell. No per-packet trace is
/// transported over the control link.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkSchedulerEvidence {
    pub polls: u32,
    pub ingress_calls: u32,
    pub ingress_packets: u32,
    pub egress_passes: u32,
    pub egress_tx_tokens: u32,
    pub egress_blocked: u32,
    pub ingress_budget_exhausted: u32,
    pub egress_budget_exhausted: u32,
    pub started_with_ingress: u32,
    pub started_with_egress: u32,
    pub exit_drained: u32,
    pub exit_work_budget: u32,
    pub exit_egress_credit: u32,
}

/// One VIF's Core0-issued shadow grant lifecycle totals.
///
/// Airtime values are conservative HT data-PPDU estimates in 100-nanosecond
/// units. They are scheduling inputs, not measured medium occupancy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiEgressVifEvidence {
    pub grants_issued: u32,
    pub issued_frame_credits: u32,
    pub issued_modeled_airtime_100ns: u32,
    pub grants_finished: u32,
    pub used_frames: u32,
    pub used_modeled_airtime_100ns: u32,
    pub grants_unused: u32,
}

/// Typed, interval-scoped evidence from the Wi-Fi physical-egress policy.
/// Queue bytes and scheduler ownership never cross this boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiEgressPolicyEvidence {
    pub grants_issued: u32,
    pub grants_finished: u32,
    pub grants_used: u32,
    pub grants_unused: u32,
    pub progress_without_grant: u32,
    pub rejected_updates: u32,
    pub rejected_progress: u32,
    pub station: WifiEgressVifEvidence,
    pub access_point: WifiEgressVifEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EvidenceRecord {
    Transport(TransportEvidence),
    FlowTransport(FlowTransportEvidence),
    Radio(RadioEvidence),
    TxAggregateTiming(TxAggregateTimingEvidence),
    RxDelivery(RxDeliveryEvidence),
    NetworkScheduler(NetworkSchedulerEvidence),
    WifiEgressPolicy(WifiEgressPolicyEvidence),
    Link(LinkHealth),
    Stack(StackUsage),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResultSummary {
    pub passed: bool,
    pub evidence_records: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    pub summary: ResultSummary,
    pub evidence_crc32c: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Hello(Capabilities),
    /// The radio and shared Wi-Fi owner are ready in the role-neutral idle
    /// state. The request ID correlates this edge with [`Command::Initialize`].
    Initialized,
    /// Correlated response to [`Command::QueryStackUsage`].
    StackUsage(StackUsage),
    /// Correlated response to [`Command::QueryLinkHealth`].
    LinkHealth(LinkHealth),
    /// Correlated response to [`Command::ProbeTimebase`].
    TimebaseProbeCompleted(TimebaseProbeEvidence),
    /// Correlated observation from [`Command::ProbeIeee802154EventStatus`].
    ///
    /// This event does not attest to same-bit concurrency, level-triggered
    /// retrigger behavior, or production interrupt readiness.
    Ieee802154EventStatusProbeCompleted(Ieee802154EventStatusProbeEvidence),
    /// Correlated observation from [`Command::ProbeIeee802154EdEvent`].
    Ieee802154EdEventProbeCompleted(Ieee802154EdEventProbeEvidence),
    Accepted,
    Rejected(RejectReason),
    State(StateChange),
    /// Correlated response to [`Command::GetStatus`].
    OperationStatus(OperationStatus),
    /// Reliable completion acknowledgement for `CycleStationEpoch`.
    /// The envelope request ID identifies the command being completed.
    StationEpochCompleted(StationEpochEvidence),
    /// Reliable completion of `StartStation` or `StopStation`.
    WifiRoleTransitioned(WifiRoleTransitionEvidence),
    /// Reliable completion of one finite `ScanWifi` request.
    WifiScanCompleted(WifiScanEvidence),
    /// Reliable completion of `StartMonitor`.
    WifiMonitorStarted(WifiRoleTransitionEvidence),
    /// Reliable completion of `StopMonitor` and its bounded capture summary.
    WifiMonitorStopped(WifiMonitorEvidence),
    /// Reliable completion of `StartAccessPoint`.
    WifiAccessPointStarted(WifiRoleTransitionEvidence),
    /// Reliable completion of `StopAccessPoint` and its bounded AP summary.
    WifiAccessPointStopped(WifiAccessPointEvidence),
    /// Reliable completion of `StopStationAccessPoint` with the AP-side
    /// timing report retained from the same physical epoch.
    WifiStationAccessPointStopped(WifiStationAccessPointStopEvidence),
    /// Terminal failure of a correlated role start/stop command.
    WifiRoleFailed(WifiRoleFailureEvidence),
    /// One ordered chunk emitted by `CaptureMonitor`.
    WifiMonitorFrame(WifiMonitorFrameChunk),
    /// Terminal completion of one finite `CaptureMonitor` request.
    WifiMonitorCaptureCompleted(WifiMonitorEvidence),
    /// Unsolicited, reliable station generation/link transition.
    StationLifecycle(StationLifecycleEvent),
    NetworkReady(NetworkInfo),
    ServiceReady(ServiceInfo),
    /// The selected data-plane worker has consumed the session configuration
    /// and is ready for host traffic in this direction.
    SessionReady(SessionReady),
    Evidence(EvidenceRecord),
    Finished(Finished),
    Failed(FailureCode),
    StartupArtifactReady(StartupArtifactStatus),
    StartupArtifact(StartupArtifactChunk),
}

impl WireBody for Event {
    const WIRE_KIND: WireKind = WireKind::Event;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(flow_id: u8, address: [u8; 4], port: u16) -> SessionFlowConfig {
        let traffic = FlowConfig {
            payload_bytes: 1_472,
            offered_rate_bps: Some(60_000_000),
            pacing_group_datagrams: None,
        };
        SessionFlowConfig {
            flow_id,
            peer: Some(Ipv4Endpoint { address, port }),
            target_rx: Some(traffic),
            target_tx: Some(traffic),
        }
    }

    fn two_flow_session() -> SessionConfig {
        SessionConfig {
            network_interface: WifiNetworkInterface::AccessPoint,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(10_000),
            flows: [
                Some(flow(3, [192, 168, 4, 2], 9_002)),
                Some(flow(9, [192, 168, 4, 3], 9_003)),
            ],
            link_requirements: SessionLinkRequirements::NONE,
        }
    }

    #[test]
    fn multi_flow_structure_requires_the_explicit_capability() {
        let session = two_flow_session();
        assert!(!session.structurally_valid(1_472, false));
        assert!(session.structurally_valid(1_472, true));
    }

    #[test]
    fn per_flow_pacing_is_nonzero_and_owned_only_by_udp_target_tx() {
        let mut session = two_flow_session();
        session.flows[1]
            .as_mut()
            .unwrap()
            .target_tx
            .as_mut()
            .unwrap()
            .pacing_group_datagrams = Some(2);
        assert!(session.structurally_valid(1_472, true));

        session.flows[1]
            .as_mut()
            .unwrap()
            .target_tx
            .as_mut()
            .unwrap()
            .pacing_group_datagrams = Some(0);
        assert!(!session.structurally_valid(1_472, true));

        session.flows[1]
            .as_mut()
            .unwrap()
            .target_tx
            .as_mut()
            .unwrap()
            .pacing_group_datagrams = None;
        session.flows[1]
            .as_mut()
            .unwrap()
            .target_rx
            .as_mut()
            .unwrap()
            .pacing_group_datagrams = Some(2);
        assert!(!session.structurally_valid(1_472, true));
    }

    #[test]
    fn multi_flow_structure_rejects_ambiguous_identities() {
        let mut duplicate_id = two_flow_session();
        duplicate_id.flows[1].as_mut().unwrap().flow_id = 3;
        assert!(!duplicate_id.structurally_valid(1_472, true));

        let mut duplicate_peer = two_flow_session();
        duplicate_peer.flows[1].as_mut().unwrap().peer = duplicate_peer.flows[0].unwrap().peer;
        assert!(!duplicate_peer.structurally_valid(1_472, true));
    }

    #[test]
    fn multi_flow_structure_rejects_missing_peer_or_direction() {
        let mut missing_peer = two_flow_session();
        missing_peer.flows[1].as_mut().unwrap().peer = None;
        assert!(!missing_peer.structurally_valid(1_472, true));

        let mut missing_rx = two_flow_session();
        missing_rx.flows[1].as_mut().unwrap().target_rx = None;
        assert!(!missing_rx.structurally_valid(1_472, true));
    }
}
