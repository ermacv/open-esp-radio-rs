//! Host side of the simultaneous RX/TX qualification cell.
//!
//! The firmware's `bidirectional` image owns a synthetic A-MPDU uplink while
//! this runner offers a paced UDP downlink.  Qualification is deliberately
//! based on device-side RX, TX-vector, placement and DMA-health evidence; a
//! successful host `send` alone is not evidence that the radio received it.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, RxRadioEvidence, SessionConfig,
    SessionLinkRequirements, Transport, TransportEvidence, TxAggregateTimingEvidence,
    TxRadioEvidence,
};

use crate::{
    Result, evidence,
    evidence::traffic_capture::{SerialCapture, SessionEvidence, await_udp_rx_ready},
    traffic::paced_udp::{Config as PacedUdpConfig, HostTransmission, send as send_paced_udp},
    traffic::tx_traffic::{Burst, describe_bursts, receive_bursts},
    transport::lab_config::{LabConfig, StationFixtureConfig},
    transport::local_air_monitor::{LocalAirMonitorCapture, LocalAirMonitorEvidence},
    transport::local_linux_fixture::{LocalLinuxTxCapture, LocalLinuxTxEvidence},
    transport::openwrt_tx_monitor::{
        MacFrameKey, OpenWrtTxMonitorCapture, OpenWrtTxMonitorEvidence,
    },
    transport::station_fixture::{RxCapture, RxEvidence},
    transport::udp_socket::{configure_qualification_receive_buffer, open_reverse_flow},
};

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 10_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const DEFAULT_TX_PORT: u16 = 9_002;
const DEFAULT_TX_PAYLOAD: usize = 1_472;
const DEVICE_TX_SOURCE_PORT: u16 = 4_324;
const MIN_HOST_TX_DATAGRAMS: u64 = 1_000;
const MIN_QUALIFIED_SAMPLE: Duration = Duration::from_secs(4);
pub(crate) const MIN_QUALIFIED_AGGREGATES: u64 = 100;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const PSRAM_CODE_START: u64 = 0x5000_0000;
const PSRAM_CODE_END: u64 = 0x5100_0000;

/// Project captures onto the fields which both monitor drivers expose
/// consistently. ath10k's AP monitor tap omits QoS TID for these frames, while
/// the independent Intel monitor reports it. Sequence and fragment therefore
/// form the strongest honest cross-device correlation key available here.
fn project_mac_units(units: &BTreeMap<MacFrameKey, u32>) -> BTreeMap<(u16, u8), u32> {
    let mut projected = BTreeMap::new();
    for (key, count) in units {
        let total = projected
            .entry((key.sequence, key.fragment))
            .or_insert(0_u32);
        *total = total.saturating_add(*count);
    }
    projected
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phy {
    Ht40,
    He20,
}

impl Phy {
    const fn name(self) -> &'static str {
        match self {
            Self::Ht40 => "ht40",
            Self::He20 => "he20",
        }
    }

    const fn required_tx(self) -> (u16, u64) {
        match self {
            // HT40 MCS7 is 135 Mbit/s with the mandatory long guard interval
            // and 150 Mbit/s when the peer-qualified short GI is selected.
            Self::Ht40 => (40, 135_000),
            Self::He20 => (20, 114_700),
        }
    }

    const fn expected_rx_format(self) -> u8 {
        match self {
            Self::Ht40 => 2,
            Self::He20 => 4,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Options {
    address: Ipv4Addr,
    port: u16,
    rate_bps: u64,
    rx_floor_bps: Option<u64>,
    duration: Duration,
    payload: usize,
    tx_port: u16,
    tx_payload: usize,
    tx_rate_bps: Option<u64>,
    tx_floor_bps: Option<u64>,
    combined_floor_bps: Option<u64>,
    serial: PathBuf,
    phy: Phy,
}

#[derive(Debug)]
struct BidirectionalEvidence {
    host_offer: HostTransmission,
    target_rx: RxQualification,
    target_tx_floor_kbps: u64,
    host_sink: Burst,
    session: SessionEvidence,
    ampdu: AmpduEvidence,
    host_receive_buffer_bytes: usize,
    fixture_rx: Option<RxEvidence>,
    fixture_tx: Option<LocalLinuxTxEvidence>,
    tx_monitor_rx: Option<OpenWrtTxMonitorEvidence>,
    independent_air_rx: Option<LocalAirMonitorEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ThroughputSample {
    datagrams: u64,
    elapsed_us: u64,
    throughput_kbps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxSample {
    throughput_kbps: u64,
    bandwidth_mhz: u16,
    rate_kbps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AmpduSample {
    aggregates: u64,
    publications: u64,
    completed: u64,
    subframes: u64,
    acknowledged: u64,
    single: u64,
    individual_retry: u64,
    timeout: u64,
    collision: u64,
    minimum: u8,
    maximum: u8,
    stop_frame: u64,
    stop_capacity: u64,
    stop_empty: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AmpduHistogramSample {
    one: u64,
    two_three: u64,
    four_seven: u64,
    eight_fifteen: u64,
    sixteen_twentythree: u64,
    twentyfour_thirty: u64,
    thirtyone: u64,
    full32: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AmpduTimingSample {
    preparation_us: u64,
    preparation_max_us: u64,
    publication_us: u64,
    publication_max_us: u64,
    exchange_us: u64,
    exchange_max_us: u64,
    first_exchanges: u64,
    first_exchange_us: u64,
    first_exchange_max_us: u64,
    retried_exchanges: u64,
    retry_publications: u64,
    retry_exchange_us: u64,
    retry_exchange_max_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxIrqTimingSample {
    tx_irq_epochs: u64,
    tx_irq_samples: u64,
    tx_irq_skew: u64,
    tx_irq_service_us: u64,
    tx_irq_service_max_us: u64,
    tx_flight_samples: u64,
    tx_flight_us: u64,
    tx_flight_max_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AmpduBlockAckSample {
    samples: u64,
    received: u64,
    success_without: u64,
    nonzero_control: u64,
    start_outside: u64,
    start_lag_max: u64,
    full: u64,
    partial: u64,
    empty: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AmpduEvidence {
    pub(crate) aggregates: u64,
    pub(crate) publications: u64,
    pub(crate) completed: u64,
    pub(crate) subframes: u64,
    pub(crate) acknowledged: u64,
    pub(crate) single: u64,
    pub(crate) individual_retry: u64,
    pub(crate) timeout: u64,
    pub(crate) collision: u64,
    pub(crate) minimum: u8,
    pub(crate) maximum: u8,
    pub(crate) stop_frame: u64,
    pub(crate) stop_capacity: u64,
    pub(crate) stop_empty: u64,
    pub(crate) one: u64,
    pub(crate) two_three: u64,
    pub(crate) four_seven: u64,
    pub(crate) eight_fifteen: u64,
    pub(crate) sixteen_twentythree: u64,
    pub(crate) twentyfour_thirty: u64,
    pub(crate) thirtyone: u64,
    pub(crate) full32: u64,
    pub(crate) preparation_us: u64,
    pub(crate) preparation_max_us: u64,
    pub(crate) publication_us: u64,
    pub(crate) publication_max_us: u64,
    pub(crate) exchange_us: u64,
    pub(crate) exchange_max_us: u64,
    pub(crate) first_exchanges: u64,
    pub(crate) first_exchange_us: u64,
    pub(crate) first_exchange_max_us: u64,
    pub(crate) retried_exchanges: u64,
    pub(crate) retry_publications: u64,
    pub(crate) retry_exchange_us: u64,
    pub(crate) retry_exchange_max_us: u64,
    pub(crate) tx_irq_epochs: u64,
    pub(crate) tx_irq_samples: u64,
    pub(crate) tx_irq_skew: u64,
    pub(crate) tx_irq_service_us: u64,
    pub(crate) tx_irq_service_max_us: u64,
    pub(crate) tx_flight_samples: u64,
    pub(crate) tx_flight_us: u64,
    pub(crate) tx_flight_max_us: u64,
    pub(crate) standby_prepared: u64,
    pub(crate) standby_published: u64,
    pub(crate) standby_cancelled: u64,
    pub(crate) block_ack_samples: u64,
    pub(crate) block_ack_received: u64,
    pub(crate) block_ack_success_without: u64,
    pub(crate) block_ack_nonzero_control: u64,
    pub(crate) block_ack_start_outside: u64,
    pub(crate) block_ack_start_lag_max: u64,
    pub(crate) full_block_ack: u64,
    pub(crate) partial_block_ack: u64,
    pub(crate) empty_block_ack: u64,
}

impl AmpduEvidence {
    pub(crate) fn from_typed(typed: TxRadioEvidence, timing: TxAggregateTimingEvidence) -> Self {
        Self {
            aggregates: u64::from(typed.aggregates_prepared),
            publications: u64::from(typed.aggregate_publications),
            completed: u64::from(typed.aggregates_completed),
            subframes: u64::from(typed.subframes_prepared),
            acknowledged: u64::from(typed.subframes_acknowledged),
            individual_retry: u64::from(typed.individual_retries),
            timeout: u64::from(typed.hardware_timeouts),
            collision: u64::from(typed.collisions),
            minimum: typed.minimum_subframes,
            maximum: typed.maximum_subframes,
            one: u64::from(typed.prepared_histogram[0]),
            two_three: u64::from(typed.prepared_histogram[1]),
            four_seven: u64::from(typed.prepared_histogram[2]),
            eight_fifteen: u64::from(typed.prepared_histogram[3]),
            sixteen_twentythree: u64::from(typed.prepared_histogram[4]),
            twentyfour_thirty: u64::from(typed.prepared_histogram[5]),
            thirtyone: u64::from(typed.prepared_histogram[6]),
            full32: u64::from(typed.prepared_histogram[7]),
            stop_frame: u64::from(typed.stopped_at_frame_limit),
            stop_capacity: u64::from(typed.stopped_at_capacity_limit),
            stop_empty: u64::from(typed.stopped_on_empty_queue),
            preparation_us: u64::from(timing.preparation_micros),
            preparation_max_us: u64::from(timing.preparation_max_micros),
            publication_us: u64::from(timing.publication_micros),
            publication_max_us: u64::from(timing.publication_max_micros),
            exchange_us: u64::from(timing.exchange_micros),
            exchange_max_us: u64::from(timing.exchange_max_micros),
            first_exchanges: u64::from(timing.first_exchanges),
            first_exchange_us: u64::from(timing.first_exchange_micros),
            first_exchange_max_us: u64::from(timing.first_exchange_max_micros),
            retried_exchanges: u64::from(timing.retried_exchanges),
            retry_publications: u64::from(timing.retry_publications),
            retry_exchange_us: u64::from(timing.retry_exchange_micros),
            retry_exchange_max_us: u64::from(timing.retry_exchange_max_micros),
            block_ack_samples: u64::from(typed.block_ack_samples),
            block_ack_received: u64::from(typed.block_ack_received),
            block_ack_success_without: u64::from(typed.success_without_block_ack),
            block_ack_nonzero_control: u64::from(typed.nonzero_block_ack_control),
            full_block_ack: u64::from(typed.full_block_ack),
            partial_block_ack: u64::from(typed.partial_block_ack),
            empty_block_ack: u64::from(typed.empty_block_ack),
            tx_irq_epochs: u64::from(typed.tx_irq_epochs),
            tx_irq_samples: u64::from(typed.tx_irq_service_samples),
            tx_irq_skew: u64::from(typed.tx_irq_clock_skew_samples),
            tx_irq_service_us: u64::from(timing.tx_irq_service_micros),
            tx_irq_service_max_us: u64::from(timing.tx_irq_service_max_micros),
            tx_flight_samples: u64::from(typed.tx_publication_to_irq_samples),
            tx_flight_us: u64::from(timing.tx_publication_to_irq_micros),
            tx_flight_max_us: u64::from(timing.tx_publication_to_irq_max_micros),
            standby_prepared: u64::from(timing.standby_prepared),
            standby_published: u64::from(timing.standby_published),
            standby_cancelled: u64::from(timing.standby_cancelled),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn from_report(report: &DeviceReport) -> Self {
        let mut evidence = Self::default();
        for sample in &report.ampdu {
            evidence.aggregates = evidence.aggregates.saturating_add(sample.aggregates);
            evidence.publications = evidence.publications.saturating_add(sample.publications);
            evidence.completed = evidence.completed.saturating_add(sample.completed);
            evidence.subframes = evidence.subframes.saturating_add(sample.subframes);
            evidence.acknowledged = evidence.acknowledged.saturating_add(sample.acknowledged);
            evidence.single = evidence.single.saturating_add(sample.single);
            evidence.individual_retry = evidence
                .individual_retry
                .saturating_add(sample.individual_retry);
            evidence.timeout = evidence.timeout.saturating_add(sample.timeout);
            evidence.collision = evidence.collision.saturating_add(sample.collision);
            if sample.minimum != 0 {
                evidence.minimum = if evidence.minimum == 0 {
                    sample.minimum
                } else {
                    evidence.minimum.min(sample.minimum)
                };
            }
            evidence.maximum = evidence.maximum.max(sample.maximum);
            evidence.stop_frame = evidence.stop_frame.saturating_add(sample.stop_frame);
            evidence.stop_capacity = evidence.stop_capacity.saturating_add(sample.stop_capacity);
            evidence.stop_empty = evidence.stop_empty.saturating_add(sample.stop_empty);
        }
        for sample in &report.ampdu_histograms {
            evidence.one = evidence.one.saturating_add(sample.one);
            evidence.two_three = evidence.two_three.saturating_add(sample.two_three);
            evidence.four_seven = evidence.four_seven.saturating_add(sample.four_seven);
            evidence.eight_fifteen = evidence.eight_fifteen.saturating_add(sample.eight_fifteen);
            evidence.sixteen_twentythree = evidence
                .sixteen_twentythree
                .saturating_add(sample.sixteen_twentythree);
            evidence.twentyfour_thirty = evidence
                .twentyfour_thirty
                .saturating_add(sample.twentyfour_thirty);
            evidence.thirtyone = evidence.thirtyone.saturating_add(sample.thirtyone);
            evidence.full32 = evidence.full32.saturating_add(sample.full32);
        }
        for sample in &report.ampdu_timings {
            evidence.preparation_us = evidence
                .preparation_us
                .saturating_add(sample.preparation_us);
            evidence.preparation_max_us =
                evidence.preparation_max_us.max(sample.preparation_max_us);
            evidence.publication_us = evidence
                .publication_us
                .saturating_add(sample.publication_us);
            evidence.publication_max_us =
                evidence.publication_max_us.max(sample.publication_max_us);
            evidence.exchange_us = evidence.exchange_us.saturating_add(sample.exchange_us);
            evidence.exchange_max_us = evidence.exchange_max_us.max(sample.exchange_max_us);
            evidence.first_exchanges = evidence
                .first_exchanges
                .saturating_add(sample.first_exchanges);
            evidence.first_exchange_us = evidence
                .first_exchange_us
                .saturating_add(sample.first_exchange_us);
            evidence.first_exchange_max_us = evidence
                .first_exchange_max_us
                .max(sample.first_exchange_max_us);
            evidence.retried_exchanges = evidence
                .retried_exchanges
                .saturating_add(sample.retried_exchanges);
            evidence.retry_publications = evidence
                .retry_publications
                .saturating_add(sample.retry_publications);
            evidence.retry_exchange_us = evidence
                .retry_exchange_us
                .saturating_add(sample.retry_exchange_us);
            evidence.retry_exchange_max_us = evidence
                .retry_exchange_max_us
                .max(sample.retry_exchange_max_us);
        }
        for sample in &report.tx_irq_timings {
            evidence.tx_irq_epochs = evidence.tx_irq_epochs.saturating_add(sample.tx_irq_epochs);
            evidence.tx_irq_samples = evidence
                .tx_irq_samples
                .saturating_add(sample.tx_irq_samples);
            evidence.tx_irq_skew = evidence.tx_irq_skew.saturating_add(sample.tx_irq_skew);
            evidence.tx_irq_service_us = evidence
                .tx_irq_service_us
                .saturating_add(sample.tx_irq_service_us);
            evidence.tx_irq_service_max_us = evidence
                .tx_irq_service_max_us
                .max(sample.tx_irq_service_max_us);
            evidence.tx_flight_samples = evidence
                .tx_flight_samples
                .saturating_add(sample.tx_flight_samples);
            evidence.tx_flight_us = evidence.tx_flight_us.saturating_add(sample.tx_flight_us);
            evidence.tx_flight_max_us = evidence.tx_flight_max_us.max(sample.tx_flight_max_us);
        }
        for sample in &report.ampdu_block_acks {
            evidence.block_ack_samples = evidence.block_ack_samples.saturating_add(sample.samples);
            evidence.block_ack_received =
                evidence.block_ack_received.saturating_add(sample.received);
            evidence.block_ack_success_without = evidence
                .block_ack_success_without
                .saturating_add(sample.success_without);
            evidence.block_ack_nonzero_control = evidence
                .block_ack_nonzero_control
                .saturating_add(sample.nonzero_control);
            evidence.block_ack_start_outside = evidence
                .block_ack_start_outside
                .saturating_add(sample.start_outside);
            evidence.block_ack_start_lag_max =
                evidence.block_ack_start_lag_max.max(sample.start_lag_max);
            evidence.full_block_ack = evidence.full_block_ack.saturating_add(sample.full);
            evidence.partial_block_ack = evidence.partial_block_ack.saturating_add(sample.partial);
            evidence.empty_block_ack = evidence.empty_block_ack.saturating_add(sample.empty);
        }
        evidence
    }

    #[cfg(test)]
    fn histogram_total(self) -> u64 {
        self.one
            .saturating_add(self.two_three)
            .saturating_add(self.four_seven)
            .saturating_add(self.eight_fifteen)
            .saturating_add(self.sixteen_twentythree)
            .saturating_add(self.twentyfour_thirty)
            .saturating_add(self.thirtyone)
            .saturating_add(self.full32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TxQualification {
    pub(crate) throughput_floor_kbps: u64,
    pub(crate) sample_count: usize,
    pub(crate) ampdu: AmpduEvidence,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxQualification {
    pub(crate) throughput_median_kbps: u64,
    pub(crate) sample_count: usize,
    pub(crate) received_datagrams: u64,
    pub(crate) enqueued: u64,
    pub(crate) dropped: u64,
    pub(crate) he_mcs_histogram: [u64; 12],
    pub(crate) other_phy_frames: u64,
    pub(crate) s_mpdu: RxSmpduEvidence,
    pub(crate) ampdu: RxAmpduEvidence,
    pub(crate) pipeline: RxPipelineEvidence,
    pub(crate) irq: MacIrqEvidence,
    pub(crate) task_polls: TaskPollSet,
    pub(crate) sequence: UdpSequenceEvidence,
    pub(crate) order: RxOrderEvidence,
    pub(crate) reorder: RxReorderEvidence,
    pub(crate) buffer_full: u64,
    pub(crate) fifo_overflow: u64,
}

impl RxQualification {
    pub(crate) fn from_typed(transport: TransportEvidence, radio: RxRadioEvidence) -> Self {
        let throughput_median_kbps = transport
            .rx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(transport.elapsed_micros.max(1))
            .unwrap_or(0);
        Self {
            throughput_median_kbps,
            sample_count: 1,
            received_datagrams: transport.rx_units,
            enqueued: transport.rx_units,
            dropped: u64::from(radio.network_dropped),
            s_mpdu: RxSmpduEvidence {
                s_mpdu_datagrams: u64::from(radio.s_mpdu_datagrams),
                not_s_mpdu_datagrams: u64::from(radio.not_s_mpdu_datagrams),
                unavailable_datagrams: u64::from(radio.s_mpdu_unavailable_datagrams),
                s_mpdu_beacons: u64::from(radio.s_mpdu_beacons),
                not_s_mpdu_beacons: u64::from(radio.not_s_mpdu_beacons),
                unavailable_beacons: u64::from(radio.s_mpdu_unavailable_beacons),
            },
            ampdu: RxAmpduEvidence {
                ampdu_datagrams: u64::from(radio.ampdu_datagrams),
                not_ampdu_datagrams: u64::from(radio.not_ampdu_datagrams),
                hardware_ampdu_datagrams: u64::from(radio.hardware_ampdu_datagrams),
                hardware_not_ampdu_datagrams: u64::from(radio.hardware_not_ampdu_datagrams),
                protocol_ampdu_datagrams: u64::from(radio.protocol_ampdu_datagrams),
                protocol_not_ampdu_datagrams: u64::from(radio.protocol_not_ampdu_datagrams),
                unavailable_datagrams: u64::from(radio.ampdu_unavailable_datagrams),
            },
            sequence: UdpSequenceEvidence {
                intervals: 1,
                first: radio.sequence_first.map(u64::from),
                highest: radio.sequence_highest.map(u64::from),
                next: radio.sequence_highest.map(|value| u64::from(value) + 1),
                gap_events: u64::from(radio.sequence_gap_events),
                forward_missing: u64::from(radio.sequence_forward_missing),
                backward: u64::from(radio.sequence_backward),
                adjacent_duplicates: u64::from(radio.sequence_duplicates),
                unsequenced: u64::from(radio.sequence_unsequenced),
                ..UdpSequenceEvidence::default()
            },
            reorder: RxReorderEvidence {
                intervals: 1,
                last_start_tid: u64::from(radio.reorder_tid),
                window: u64::from(radio.reorder_window),
                first_samples: u64::from(radio.reorder_first_samples),
                first_tid: u64::from(radio.reorder_first_tid),
                first_start_sequence: u64::from(radio.reorder_first_start),
                first_frame_sequence: u64::from(radio.reorder_first_sequence),
                first_distance: u64::from(radio.reorder_first_distance),
                occupied: u64::from(radio.reorder_current_occupied),
                maximum_occupied: u64::from(radio.reorder_maximum_occupied),
                ..RxReorderEvidence::default()
            },
            buffer_full: u64::from(radio.dma_buffer_full),
            fifo_overflow: u64::from(radio.dma_fifo_overflow),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxSmpduEvidence {
    pub(crate) s_mpdu_datagrams: u64,
    pub(crate) not_s_mpdu_datagrams: u64,
    pub(crate) unavailable_datagrams: u64,
    pub(crate) s_mpdu_beacons: u64,
    pub(crate) not_s_mpdu_beacons: u64,
    pub(crate) unavailable_beacons: u64,
}

impl RxSmpduEvidence {
    fn merge(&mut self, sample: Self) {
        self.s_mpdu_datagrams = self
            .s_mpdu_datagrams
            .saturating_add(sample.s_mpdu_datagrams);
        self.not_s_mpdu_datagrams = self
            .not_s_mpdu_datagrams
            .saturating_add(sample.not_s_mpdu_datagrams);
        self.unavailable_datagrams = self
            .unavailable_datagrams
            .saturating_add(sample.unavailable_datagrams);
        self.s_mpdu_beacons = self.s_mpdu_beacons.saturating_add(sample.s_mpdu_beacons);
        self.not_s_mpdu_beacons = self
            .not_s_mpdu_beacons
            .saturating_add(sample.not_s_mpdu_beacons);
        self.unavailable_beacons = self
            .unavailable_beacons
            .saturating_add(sample.unavailable_beacons);
    }

    fn observed_datagrams(self) -> u64 {
        self.s_mpdu_datagrams
            .saturating_add(self.not_s_mpdu_datagrams)
    }

    fn observed_beacons(self) -> u64 {
        self.s_mpdu_beacons.saturating_add(self.not_s_mpdu_beacons)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxAmpduEvidence {
    pub(crate) ampdu_datagrams: u64,
    pub(crate) not_ampdu_datagrams: u64,
    pub(crate) hardware_ampdu_datagrams: u64,
    pub(crate) hardware_not_ampdu_datagrams: u64,
    pub(crate) protocol_ampdu_datagrams: u64,
    pub(crate) protocol_not_ampdu_datagrams: u64,
    pub(crate) unavailable_datagrams: u64,
}

impl RxAmpduEvidence {
    fn merge(&mut self, sample: Self) {
        self.ampdu_datagrams = self.ampdu_datagrams.saturating_add(sample.ampdu_datagrams);
        self.not_ampdu_datagrams = self
            .not_ampdu_datagrams
            .saturating_add(sample.not_ampdu_datagrams);
        self.hardware_ampdu_datagrams = self
            .hardware_ampdu_datagrams
            .saturating_add(sample.hardware_ampdu_datagrams);
        self.hardware_not_ampdu_datagrams = self
            .hardware_not_ampdu_datagrams
            .saturating_add(sample.hardware_not_ampdu_datagrams);
        self.protocol_ampdu_datagrams = self
            .protocol_ampdu_datagrams
            .saturating_add(sample.protocol_ampdu_datagrams);
        self.protocol_not_ampdu_datagrams = self
            .protocol_not_ampdu_datagrams
            .saturating_add(sample.protocol_not_ampdu_datagrams);
        self.unavailable_datagrams = self
            .unavailable_datagrams
            .saturating_add(sample.unavailable_datagrams);
    }

    fn hardware_observed_datagrams(self) -> u64 {
        self.hardware_ampdu_datagrams
            .saturating_add(self.hardware_not_ampdu_datagrams)
    }

    fn protocol_validated_datagrams(self) -> u64 {
        self.protocol_ampdu_datagrams
            .saturating_add(self.protocol_not_ampdu_datagrams)
    }
}

pub(crate) struct RxAssessment {
    pub(crate) rx: RxQualification,
    pub(crate) failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UdpSequenceEvidence {
    pub(crate) intervals: u64,
    pub(crate) first: Option<u64>,
    pub(crate) highest: Option<u64>,
    pub(crate) next: Option<u64>,
    pub(crate) gap_events: u64,
    pub(crate) forward_missing: u64,
    pub(crate) maximum_gap: u64,
    pub(crate) maximum_gap_at: Option<u64>,
    pub(crate) first_gap_at: Option<u64>,
    pub(crate) last_gap_at: Option<u64>,
    pub(crate) backward: u64,
    pub(crate) adjacent_duplicates: u64,
    pub(crate) unsequenced: u64,
    pub(crate) maximum_interarrival_us: u64,
    pub(crate) maximum_interarrival_at: Option<u64>,
}

impl UdpSequenceEvidence {
    fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        if self.first.is_none() {
            self.first = sample.first;
        }
        self.highest = self.highest.max(sample.highest);
        self.next = self.next.max(sample.next);
        self.gap_events = self.gap_events.saturating_add(sample.gap_events);
        self.forward_missing = self.forward_missing.saturating_add(sample.forward_missing);
        if sample.maximum_gap > self.maximum_gap {
            self.maximum_gap = sample.maximum_gap;
            self.maximum_gap_at = sample.maximum_gap_at;
        }
        if self.first_gap_at.is_none() {
            self.first_gap_at = sample.first_gap_at;
        }
        if sample.last_gap_at.is_some() {
            self.last_gap_at = sample.last_gap_at;
        }
        self.backward = self.backward.saturating_add(sample.backward);
        self.adjacent_duplicates = self
            .adjacent_duplicates
            .saturating_add(sample.adjacent_duplicates);
        self.unsequenced = self.unsequenced.saturating_add(sample.unsequenced);
        if sample.maximum_interarrival_us >= self.maximum_interarrival_us
            && sample.maximum_interarrival_at.is_some()
        {
            self.maximum_interarrival_us = sample.maximum_interarrival_us;
            self.maximum_interarrival_at = sample.maximum_interarrival_at;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxOrderEvidence {
    pub(crate) intervals: u64,
    pub(crate) gap_events: u64,
    pub(crate) forward_missing: u64,
    pub(crate) backward: u64,
    pub(crate) adjacent_duplicates: u64,
    pub(crate) backward_mac_backward: u64,
    pub(crate) backward_mac_same: u64,
    pub(crate) backward_mac_forward: u64,
    pub(crate) backward_mac_other_tid: u64,
    pub(crate) backward_mac_unavailable: u64,
}

impl RxOrderEvidence {
    fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.gap_events = self.gap_events.saturating_add(sample.gap_events);
        self.forward_missing = self.forward_missing.saturating_add(sample.forward_missing);
        self.backward = self.backward.saturating_add(sample.backward);
        self.adjacent_duplicates = self
            .adjacent_duplicates
            .saturating_add(sample.adjacent_duplicates);
        self.backward_mac_backward = self
            .backward_mac_backward
            .saturating_add(sample.backward_mac_backward);
        self.backward_mac_same = self
            .backward_mac_same
            .saturating_add(sample.backward_mac_same);
        self.backward_mac_forward = self
            .backward_mac_forward
            .saturating_add(sample.backward_mac_forward);
        self.backward_mac_other_tid = self
            .backward_mac_other_tid
            .saturating_add(sample.backward_mac_other_tid);
        self.backward_mac_unavailable = self
            .backward_mac_unavailable
            .saturating_add(sample.backward_mac_unavailable);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxReorderEvidence {
    pub(crate) intervals: u64,
    pub(crate) starts: u64,
    pub(crate) stops: u64,
    pub(crate) last_start_tid: u64,
    pub(crate) last_start_sequence: u64,
    pub(crate) window: u64,
    pub(crate) first_samples: u64,
    pub(crate) first_tid: u64,
    pub(crate) first_start_sequence: u64,
    pub(crate) first_frame_sequence: u64,
    pub(crate) first_distance: u64,
    pub(crate) buffered: u64,
    pub(crate) released: u64,
    pub(crate) missing: u64,
    pub(crate) stale: u64,
    pub(crate) gap_expiries: u64,
    pub(crate) occupied: u64,
    pub(crate) maximum_occupied: u64,
}

impl RxReorderEvidence {
    fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.starts = self.starts.saturating_add(sample.starts);
        self.stops = self.stops.saturating_add(sample.stops);
        if sample.window != 0 {
            self.last_start_tid = sample.last_start_tid;
            self.last_start_sequence = sample.last_start_sequence;
            self.window = sample.window;
        }
        if sample.first_samples != 0 {
            self.first_tid = sample.first_tid;
            self.first_start_sequence = sample.first_start_sequence;
            self.first_frame_sequence = sample.first_frame_sequence;
            self.first_distance = sample.first_distance;
        }
        self.first_samples = self.first_samples.saturating_add(sample.first_samples);
        self.buffered = self.buffered.saturating_add(sample.buffered);
        self.released = self.released.saturating_add(sample.released);
        self.missing = self.missing.saturating_add(sample.missing);
        self.stale = self.stale.saturating_add(sample.stale);
        self.gap_expiries = self.gap_expiries.saturating_add(sample.gap_expiries);
        self.occupied = sample.occupied;
        self.maximum_occupied = self.maximum_occupied.max(sample.maximum_occupied);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TaskPollEvidence {
    pub(crate) intervals: u64,
    pub(crate) polls: u64,
    pub(crate) poll_us: u64,
    pub(crate) poll_boot_max_us: u64,
    pub(crate) over_100us: u64,
    pub(crate) over_500us: u64,
    pub(crate) over_1000us: u64,
    pub(crate) over_5000us: u64,
}

impl TaskPollEvidence {
    fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.polls = self.polls.saturating_add(sample.polls);
        self.poll_us = self.poll_us.saturating_add(sample.poll_us);
        self.poll_boot_max_us = self.poll_boot_max_us.max(sample.poll_boot_max_us);
        self.over_100us = self.over_100us.saturating_add(sample.over_100us);
        self.over_500us = self.over_500us.saturating_add(sample.over_500us);
        self.over_1000us = self.over_1000us.saturating_add(sample.over_1000us);
        self.over_5000us = self.over_5000us.saturating_add(sample.over_5000us);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TaskPollSet {
    pub(crate) network: TaskPollEvidence,
    pub(crate) protocol: TaskPollEvidence,
    pub(crate) radio: TaskPollEvidence,
    pub(crate) udp_rx: TaskPollEvidence,
    pub(crate) udp_tx: TaskPollEvidence,
    pub(crate) tcp: TaskPollEvidence,
}

impl TaskPollSet {
    fn is_complete(self) -> bool {
        self.network.intervals != 0
            && self.protocol.intervals != 0
            && self.radio.intervals != 0
            && self.udp_rx.intervals != 0
            && self.udp_tx.intervals != 0
            && self.tcp.intervals != 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MacIrqEvidence {
    pub(crate) spurious_entries: u64,
    pub(crate) rx_only_entries: u64,
    pub(crate) rx_mixed_entries: u64,
    pub(crate) tx_only_entries: u64,
    pub(crate) tx_mixed_entries: u64,
    pub(crate) other_only_entries: u64,
    pub(crate) extra_nonzero_snapshots: u64,
    pub(crate) saturated_entries: u64,
    pub(crate) auxiliary_status_or: u64,
    pub(crate) unknown_status_or: u64,
}

impl MacIrqEvidence {
    fn merge(&mut self, sample: Self) {
        self.spurious_entries = self
            .spurious_entries
            .saturating_add(sample.spurious_entries);
        self.rx_only_entries = self.rx_only_entries.saturating_add(sample.rx_only_entries);
        self.rx_mixed_entries = self
            .rx_mixed_entries
            .saturating_add(sample.rx_mixed_entries);
        self.tx_only_entries = self.tx_only_entries.saturating_add(sample.tx_only_entries);
        self.tx_mixed_entries = self
            .tx_mixed_entries
            .saturating_add(sample.tx_mixed_entries);
        self.other_only_entries = self
            .other_only_entries
            .saturating_add(sample.other_only_entries);
        self.extra_nonzero_snapshots = self
            .extra_nonzero_snapshots
            .saturating_add(sample.extra_nonzero_snapshots);
        self.saturated_entries = self
            .saturated_entries
            .saturating_add(sample.saturated_entries);
        self.auxiliary_status_or |= sample.auxiliary_status_or;
        self.unknown_status_or |= sample.unknown_status_or;
    }

    pub(crate) fn classified_entries(self) -> u64 {
        self.spurious_entries
            .saturating_add(self.rx_only_entries)
            .saturating_add(self.rx_mixed_entries)
            .saturating_add(self.tx_only_entries)
            .saturating_add(self.tx_mixed_entries)
            .saturating_add(self.other_only_entries)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxPipelineEvidence {
    pub(crate) service_calls: u64,
    pub(crate) frontier_zero_services: u64,
    pub(crate) frontier_one_services: u64,
    pub(crate) frontier_two_three_services: u64,
    pub(crate) frontier_four_seven_services: u64,
    pub(crate) frontier_eight_fifteen_services: u64,
    pub(crate) frontier_sixteen_thirty_one_services: u64,
    pub(crate) frontier_thirty_two_plus_services: u64,
    pub(crate) frontier_frames: u64,
    pub(crate) admitted_frames: u64,
    pub(crate) staged_bytes: u64,
    pub(crate) stage_empty_discards: u64,
    pub(crate) stage_too_long_discards: u64,
    pub(crate) backpressured_services: u64,
    pub(crate) pool_credit_limited_services: u64,
    pub(crate) queue_credit_limited_services: u64,
    pub(crate) maximum_deferred_frames: u64,
    pub(crate) minimum_backpressured_pool_credits: u64,
    pub(crate) minimum_backpressured_queue_credits: u64,
    pub(crate) maximum_frontier: u64,
    pub(crate) maximum_admitted: u64,
    pub(crate) service_us: u64,
    pub(crate) service_max_us: u64,
    pub(crate) reload_transactions: u64,
    pub(crate) reload_us: u64,
    pub(crate) reload_max_us: u64,
    pub(crate) dma_buffer_full_increments: u64,
    pub(crate) dma_buffer_full_service_samples: u64,
    pub(crate) dma_buffer_full_between_services: u64,
    pub(crate) dma_buffer_full_during_services: u64,
    pub(crate) dma_buffer_full_between_service_samples: u64,
    pub(crate) dma_buffer_full_during_service_samples: u64,
    pub(crate) dma_buffer_full_last_service: u64,
    pub(crate) dma_buffer_full_last_phase: u64,
    pub(crate) dma_buffer_full_last_counter: u64,
    pub(crate) dma_buffer_full_last_frontier: u64,
    pub(crate) dma_buffer_full_last_admitted: u64,
    pub(crate) dma_buffer_full_last_pool_credits: u64,
    pub(crate) dma_buffer_full_last_queue_credits: u64,
    pub(crate) dma_buffer_full_last_service_us: u64,
    pub(crate) protocol_frames: u64,
    pub(crate) protocol_data_frames: u64,
    pub(crate) protocol_amsdu_mpdus: u64,
    pub(crate) protocol_amsdu_subframes: u64,
    pub(crate) protocol_units_le_1700: u64,
    pub(crate) protocol_units_1701_3400: u64,
    pub(crate) protocol_units_over_3400: u64,
    pub(crate) protocol_unit_max_bytes: u64,
    pub(crate) network_ready_waits: u64,
    pub(crate) network_ready_wait_us: u64,
    pub(crate) network_ready_wait_max_us: u64,
    pub(crate) dispatch_us: u64,
    pub(crate) dispatch_max_us: u64,
    pub(crate) network_publications: u64,
    pub(crate) network_published_bytes: u64,
    pub(crate) network_publish_us: u64,
    pub(crate) network_publish_max_us: u64,
    pub(crate) rx_irq_posts: u64,
    pub(crate) rx_irq_epochs: u64,
    pub(crate) mac_irq_entries: u64,
    pub(crate) rx_irq_coalesced_posts: u64,
    pub(crate) rx_irq_service_samples: u64,
    pub(crate) rx_irq_clock_skew_samples: u64,
    pub(crate) rx_irq_to_service_us: u64,
    pub(crate) rx_irq_to_service_max_us: u64,
}

impl RxPipelineEvidence {
    fn merge(&mut self, sample: Self) {
        let had_backpressure = self.backpressured_services != 0;
        let sample_has_backpressure = sample.backpressured_services != 0;
        self.service_calls = self.service_calls.saturating_add(sample.service_calls);
        self.frontier_zero_services = self
            .frontier_zero_services
            .saturating_add(sample.frontier_zero_services);
        self.frontier_one_services = self
            .frontier_one_services
            .saturating_add(sample.frontier_one_services);
        self.frontier_two_three_services = self
            .frontier_two_three_services
            .saturating_add(sample.frontier_two_three_services);
        self.frontier_four_seven_services = self
            .frontier_four_seven_services
            .saturating_add(sample.frontier_four_seven_services);
        self.frontier_eight_fifteen_services = self
            .frontier_eight_fifteen_services
            .saturating_add(sample.frontier_eight_fifteen_services);
        self.frontier_sixteen_thirty_one_services = self
            .frontier_sixteen_thirty_one_services
            .saturating_add(sample.frontier_sixteen_thirty_one_services);
        self.frontier_thirty_two_plus_services = self
            .frontier_thirty_two_plus_services
            .saturating_add(sample.frontier_thirty_two_plus_services);
        self.frontier_frames = self.frontier_frames.saturating_add(sample.frontier_frames);
        self.admitted_frames = self.admitted_frames.saturating_add(sample.admitted_frames);
        self.staged_bytes = self.staged_bytes.saturating_add(sample.staged_bytes);
        self.stage_empty_discards = self
            .stage_empty_discards
            .saturating_add(sample.stage_empty_discards);
        self.stage_too_long_discards = self
            .stage_too_long_discards
            .saturating_add(sample.stage_too_long_discards);
        self.backpressured_services = self
            .backpressured_services
            .saturating_add(sample.backpressured_services);
        self.pool_credit_limited_services = self
            .pool_credit_limited_services
            .saturating_add(sample.pool_credit_limited_services);
        self.queue_credit_limited_services = self
            .queue_credit_limited_services
            .saturating_add(sample.queue_credit_limited_services);
        self.maximum_deferred_frames = self
            .maximum_deferred_frames
            .max(sample.maximum_deferred_frames);
        if sample_has_backpressure {
            if had_backpressure {
                self.minimum_backpressured_pool_credits = self
                    .minimum_backpressured_pool_credits
                    .min(sample.minimum_backpressured_pool_credits);
                self.minimum_backpressured_queue_credits = self
                    .minimum_backpressured_queue_credits
                    .min(sample.minimum_backpressured_queue_credits);
            } else {
                self.minimum_backpressured_pool_credits = sample.minimum_backpressured_pool_credits;
                self.minimum_backpressured_queue_credits =
                    sample.minimum_backpressured_queue_credits;
            }
        }
        self.maximum_frontier = self.maximum_frontier.max(sample.maximum_frontier);
        self.maximum_admitted = self.maximum_admitted.max(sample.maximum_admitted);
        self.service_us = self.service_us.saturating_add(sample.service_us);
        self.service_max_us = self.service_max_us.max(sample.service_max_us);
        self.reload_transactions = self
            .reload_transactions
            .saturating_add(sample.reload_transactions);
        self.reload_us = self.reload_us.saturating_add(sample.reload_us);
        self.reload_max_us = self.reload_max_us.max(sample.reload_max_us);
        let sample_has_buffer_full = sample.dma_buffer_full_increments != 0;
        self.dma_buffer_full_increments = self
            .dma_buffer_full_increments
            .saturating_add(sample.dma_buffer_full_increments);
        self.dma_buffer_full_service_samples = self
            .dma_buffer_full_service_samples
            .saturating_add(sample.dma_buffer_full_service_samples);
        self.dma_buffer_full_between_services = self
            .dma_buffer_full_between_services
            .saturating_add(sample.dma_buffer_full_between_services);
        self.dma_buffer_full_during_services = self
            .dma_buffer_full_during_services
            .saturating_add(sample.dma_buffer_full_during_services);
        self.dma_buffer_full_between_service_samples = self
            .dma_buffer_full_between_service_samples
            .saturating_add(sample.dma_buffer_full_between_service_samples);
        self.dma_buffer_full_during_service_samples = self
            .dma_buffer_full_during_service_samples
            .saturating_add(sample.dma_buffer_full_during_service_samples);
        if sample_has_buffer_full {
            self.dma_buffer_full_last_service = sample.dma_buffer_full_last_service;
            self.dma_buffer_full_last_phase = sample.dma_buffer_full_last_phase;
            self.dma_buffer_full_last_counter = sample.dma_buffer_full_last_counter;
            self.dma_buffer_full_last_frontier = sample.dma_buffer_full_last_frontier;
            self.dma_buffer_full_last_admitted = sample.dma_buffer_full_last_admitted;
            self.dma_buffer_full_last_pool_credits = sample.dma_buffer_full_last_pool_credits;
            self.dma_buffer_full_last_queue_credits = sample.dma_buffer_full_last_queue_credits;
            self.dma_buffer_full_last_service_us = sample.dma_buffer_full_last_service_us;
        }
        self.protocol_frames = self.protocol_frames.saturating_add(sample.protocol_frames);
        self.protocol_data_frames = self
            .protocol_data_frames
            .saturating_add(sample.protocol_data_frames);
        self.protocol_amsdu_mpdus = self
            .protocol_amsdu_mpdus
            .saturating_add(sample.protocol_amsdu_mpdus);
        self.protocol_amsdu_subframes = self
            .protocol_amsdu_subframes
            .saturating_add(sample.protocol_amsdu_subframes);
        self.protocol_units_le_1700 = self
            .protocol_units_le_1700
            .saturating_add(sample.protocol_units_le_1700);
        self.protocol_units_1701_3400 = self
            .protocol_units_1701_3400
            .saturating_add(sample.protocol_units_1701_3400);
        self.protocol_units_over_3400 = self
            .protocol_units_over_3400
            .saturating_add(sample.protocol_units_over_3400);
        self.protocol_unit_max_bytes = self
            .protocol_unit_max_bytes
            .max(sample.protocol_unit_max_bytes);
        self.network_ready_waits = self
            .network_ready_waits
            .saturating_add(sample.network_ready_waits);
        self.network_ready_wait_us = self
            .network_ready_wait_us
            .saturating_add(sample.network_ready_wait_us);
        self.network_ready_wait_max_us = self
            .network_ready_wait_max_us
            .max(sample.network_ready_wait_max_us);
        self.dispatch_us = self.dispatch_us.saturating_add(sample.dispatch_us);
        self.dispatch_max_us = self.dispatch_max_us.max(sample.dispatch_max_us);
        self.network_publications = self
            .network_publications
            .saturating_add(sample.network_publications);
        self.network_published_bytes = self
            .network_published_bytes
            .saturating_add(sample.network_published_bytes);
        self.network_publish_us = self
            .network_publish_us
            .saturating_add(sample.network_publish_us);
        self.network_publish_max_us = self
            .network_publish_max_us
            .max(sample.network_publish_max_us);
        self.rx_irq_posts = self.rx_irq_posts.saturating_add(sample.rx_irq_posts);
        self.rx_irq_epochs = self.rx_irq_epochs.saturating_add(sample.rx_irq_epochs);
        self.mac_irq_entries = self.mac_irq_entries.saturating_add(sample.mac_irq_entries);
        self.rx_irq_coalesced_posts = self
            .rx_irq_coalesced_posts
            .saturating_add(sample.rx_irq_coalesced_posts);
        self.rx_irq_service_samples = self
            .rx_irq_service_samples
            .saturating_add(sample.rx_irq_service_samples);
        self.rx_irq_clock_skew_samples = self
            .rx_irq_clock_skew_samples
            .saturating_add(sample.rx_irq_clock_skew_samples);
        self.rx_irq_to_service_us = self
            .rx_irq_to_service_us
            .saturating_add(sample.rx_irq_to_service_us);
        self.rx_irq_to_service_max_us = self
            .rx_irq_to_service_max_us
            .max(sample.rx_irq_to_service_max_us);
    }

    fn frontier_histogram_total(self) -> u64 {
        self.frontier_zero_services
            .saturating_add(self.frontier_one_services)
            .saturating_add(self.frontier_two_three_services)
            .saturating_add(self.frontier_four_seven_services)
            .saturating_add(self.frontier_eight_fifteen_services)
            .saturating_add(self.frontier_sixteen_thirty_one_services)
            .saturating_add(self.frontier_thirty_two_plus_services)
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct DeviceReport {
    rx: Vec<ThroughputSample>,
    tx: Vec<TxSample>,
    ampdu: Vec<AmpduSample>,
    ampdu_histograms: Vec<AmpduHistogramSample>,
    ampdu_timings: Vec<AmpduTimingSample>,
    tx_irq_timings: Vec<TxIrqTimingSample>,
    ampdu_block_acks: Vec<AmpduBlockAckSample>,
    rx_mcs_histograms: Vec<([u64; 12], u64)>,
    rx_s_mpdu: Vec<RxSmpduEvidence>,
    rx_ampdu: Vec<RxAmpduEvidence>,
    rx_service: Vec<RxPipelineEvidence>,
    rx_dispatch: Vec<RxPipelineEvidence>,
    rx_frontier: Vec<RxPipelineEvidence>,
    mac_irq: Vec<MacIrqEvidence>,
    task_polls: TaskPollSet,
    rx_sequences: Vec<UdpSequenceEvidence>,
    rx_order: Vec<RxOrderEvidence>,
    rx_reorder: Vec<RxReorderEvidence>,
    rx_formats: Vec<u8>,
    dma_health: Vec<(u64, u64)>,
    software_health: Vec<(u64, u64)>,
    code_addresses: Vec<u64>,
    failures: Vec<String>,
}

pub(crate) struct RunPolicy {
    pub(crate) require_exact_delivery: bool,
    pub(crate) require_no_beacon_loss: bool,
    pub(crate) capture_openwrt_tx_monitor_rx: bool,
    pub(crate) capture_independent_laptop_monitor_rx: bool,
    pub(crate) require_driver_observation: bool,
}

pub(crate) fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    policy: RunPolicy,
) -> Result<()> {
    let RunPolicy {
        require_exact_delivery,
        require_no_beacon_loss,
        capture_openwrt_tx_monitor_rx,
        capture_independent_laptop_monitor_rx,
        require_driver_observation,
    } = policy;
    let mut options = parse_options(&arguments, lab)?;
    fs::create_dir_all(output)?;
    let tx_sink = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.tx_port))?;
    let host_receive_buffer_bytes = configure_qualification_receive_buffer(&tx_sink)?;
    tx_sink.set_read_timeout(Some(Duration::from_millis(100)))?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let discovered_address = match await_udp_rx_ready(
        &capture,
        lab,
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    options.address = discovered_address.address;
    tx_sink.connect(SocketAddrV4::new(options.address, DEVICE_TX_SOURCE_PORT))?;
    let host_address = match tx_sink.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("bidirectional qualification requires IPv4".into()),
    };
    // Admit the reverse flow through stateful host firewalls before `Start`.
    if let Err(error) = open_reverse_flow(&tx_sink) {
        capture.finish_to(output)?;
        return Err(error.into());
    }
    let fixture_capture = RxCapture::start(
        &lab.station_fixture,
        options.address,
        options.port,
        options.duration,
        match options.phy {
            Phy::Ht40 => crate::qualification::scenario::PhyExpectation::Ht40,
            Phy::He20 => crate::qualification::scenario::PhyExpectation::He20,
        },
    )?;
    let fixture_tx_capture = match &lab.station_fixture {
        StationFixtureConfig::LocalLinux(config) => Some(LocalLinuxTxCapture::start(
            config,
            options.address,
            DEVICE_TX_SOURCE_PORT,
            options.tx_port,
            options.duration,
            match options.phy {
                Phy::Ht40 => crate::qualification::scenario::PhyExpectation::Ht40,
                Phy::He20 => crate::qualification::scenario::PhyExpectation::He20,
            },
        )?),
        StationFixtureConfig::OpenWrt(_) | StationFixtureConfig::External(_) => None,
    };
    let tx_monitor_capture = if capture_openwrt_tx_monitor_rx {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err("OpenWrt TX-monitor evidence requires an OpenWrt station fixture".into());
        };
        Some(OpenWrtTxMonitorCapture::start(
            config,
            options.address,
            options.port,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let independent_air_capture = if capture_independent_laptop_monitor_rx {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err("independent laptop evidence requires an OpenWrt station fixture".into());
        };
        Some(LocalAirMonitorCapture::start(
            config,
            options.address,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let session = capture.start_session(SessionConfig {
        network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        peer: Some(Ipv4Endpoint {
            address: host_address.octets(),
            port: options.tx_port,
        }),
        target_rx: Some(FlowConfig {
            payload_bytes: u16::try_from(options.payload)?,
            offered_rate_bps: Some(options.rate_bps),
        }),
        target_tx: Some(FlowConfig {
            payload_bytes: u16::try_from(options.tx_payload)?,
            offered_rate_bps: options.tx_rate_bps,
        }),
        link_requirements: SessionLinkRequirements::tx_block_ack(0),
    })?;
    let receiver_duration = options.duration.saturating_add(Duration::from_secs(2));
    let expected_device = options.address;
    let receiver =
        thread::spawn(move || receive_bursts(&tx_sink, expected_device, receiver_duration));
    let host_result = send_paced_udp(PacedUdpConfig {
        address: options.address,
        port: options.port,
        rate_bps: options.rate_bps,
        duration: options.duration,
        payload: options.payload,
    });
    let tx_bursts = receiver
        .join()
        .map_err(|_| "bidirectional host TX receiver panicked")??;
    let host = host_result?;
    let structured = match capture.wait_for_session(session, Duration::from_secs(5)) {
        Ok(evidence) => evidence,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    if let Err(error) = capture.acknowledge_session(session) {
        capture.finish_to(output)?;
        return Err(error);
    }
    let fixture_rx = fixture_capture.map(RxCapture::finish).transpose()?;
    let fixture_tx = fixture_tx_capture
        .map(LocalLinuxTxCapture::finish)
        .transpose()?;
    let tx_monitor_rx = tx_monitor_capture
        .map(|capture| capture.finish(host.datagrams))
        .transpose()?;
    let independent_air_rx = independent_air_capture
        .map(LocalAirMonitorCapture::finish)
        .transpose()?;
    if let Some(evidence) = fixture_tx.as_ref() {
        fs::write(
            output.join("local-wireless-ingress.txt"),
            format!(
                "udp_packets={}\nchannel_width_mhz={}\ntx_bitrate={}\nrx_bitrate={}\n",
                evidence.udp_packets,
                evidence.channel_width_mhz,
                evidence.tx_bitrate,
                evidence.rx_bitrate,
            ),
        )?;
    }
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    let log = capture.finish_to(output)?;
    if let Some(result) = beacon_loss {
        result?;
    }
    let rx_median = structured
        .transport
        .rx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let tx_floor = structured
        .transport
        .tx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let qualified_tx_bursts: Vec<_> = tx_bursts
        .iter()
        .copied()
        .filter(|burst| burst.started_at_zero && burst.datagrams >= MIN_HOST_TX_DATAGRAMS)
        .collect();
    if qualified_tx_bursts.len() != 1 {
        return Err(format!(
            "expected one complete target-to-host burst, received {}; {}",
            qualified_tx_bursts.len(),
            describe_bursts(&tx_bursts),
        )
        .into());
    }
    let host_tx = qualified_tx_bursts[0];
    if !require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        let expected_rx_bytes = structured
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        let expected_tx_bytes = structured
            .transport
            .tx_units
            .saturating_mul(options.tx_payload as u64);
        if !structured.finished.summary.passed
            || structured.transport.transport_errors != 0
            || structured.transport.rx_bytes != expected_rx_bytes
            || structured.transport.tx_bytes != expected_tx_bytes
        {
            return Err(format!(
                "target did not complete bidirectional performance session cleanly: passed={} errors={} rx={}/{} tx={}/{}",
                structured.finished.summary.passed,
                structured.transport.transport_errors,
                structured.transport.rx_bytes,
                expected_rx_bytes,
                structured.transport.tx_bytes,
                expected_tx_bytes,
            )
            .into());
        }
        let minimum_rx_bps = options
            .rx_floor_bps
            .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
        if host.throughput_bps() < minimum_rx_bps
            || rx_median.saturating_mul(1_000) < minimum_rx_bps
        {
            return Err(format!(
                "concurrent RX is below the configured floor: required={minimum_rx_bps} host={} target={} bit/s",
                host.throughput_bps(),
                rx_median.saturating_mul(1_000),
            )
            .into());
        }
        if let Some(required_tx_bps) = options.tx_floor_bps {
            let measured_device = tx_floor.saturating_mul(1_000);
            let measured_host = host_tx.throughput_kbps().saturating_mul(1_000);
            if measured_device < required_tx_bps || measured_host < required_tx_bps {
                return Err(format!(
                    "concurrent TX is below the configured floor: required={required_tx_bps} device={measured_device} host={measured_host} bit/s"
                )
                .into());
            }
        }
        if let Some(required_combined_bps) = options.combined_floor_bps {
            let measured = rx_median.saturating_add(tx_floor).saturating_mul(1_000);
            if measured < required_combined_bps {
                return Err(format!(
                    "combined throughput is below the configured floor: required={required_combined_bps} measured={measured} bit/s"
                )
                .into());
            }
        }
        write_bidirectional_performance_report(
            output,
            BidirectionalPerformanceReport {
                options: &options,
                host_offer: host,
                host_sink: host_tx,
                structured,
                rx_kbps: rx_median,
                tx_kbps: tx_floor,
                host_receive_buffer_bytes,
            },
        )?;
        eprintln!(
            "OPENRADIOHOST result=PASS mode={}-bidirectional-performance offered_kbps={} host_kbps={} rx_kbps={rx_median} tx_kbps={tx_floor} host_tx_kbps={} combined_kbps={} report={}",
            options.phy.name(),
            options.rate_bps / 1_000,
            host.throughput_bps() / 1_000,
            host_tx.throughput_kbps(),
            rx_median.saturating_add(tx_floor),
            output.join("report.md").display(),
        );
        return Ok(());
    }
    let report = parse_device_report(&log);
    let raw_rx_radio = structured
        .radio
        .and_then(|evidence| evidence.rx)
        .ok_or("session did not publish typed RX radio evidence")?;
    let mut qualification_failure = if require_exact_delivery {
        structured.require_rx_radio(options.phy.expected_rx_format(), host.datagrams)
    } else {
        structured.require_rx_radio_health(options.phy.expected_rx_format())
    }
    .err()
    .map(|error| error.to_string());
    let text_assessment = assess_rx_report(&report, options.phy.expected_rx_format()).ok();
    if let Some(failure) = text_assessment
        .as_ref()
        .and_then(|assessment| assessment.failure.as_deref())
    {
        eprintln!("diagnostic_text_warning={failure}");
    }
    let rx = text_assessment
        .map(|assessment| assessment.rx)
        .unwrap_or_else(|| {
            let mut rx = RxQualification::from_typed(structured.transport, raw_rx_radio);
            // A failed typed acceptance result must not hide independent
            // diagnostic poll evidence from the same completed interval.
            rx.task_polls = report.task_polls;
            rx
        });
    if let Some(required_floor) = options.tx_floor_bps {
        let measured_device = tx_floor.saturating_mul(1_000);
        let measured_host = host_tx.throughput_kbps().saturating_mul(1_000);
        if measured_device < required_floor || measured_host < required_floor {
            qualification_failure.get_or_insert_with(|| format!(
                "concurrent TX is below the configured floor: required={} device={} host={} bit/s",
                required_floor, measured_device, measured_host
            ));
        }
    }
    if let Some(required_floor) = options.combined_floor_bps {
        let measured = rx_median.saturating_add(tx_floor).saturating_mul(1_000);
        if measured < required_floor {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "combined device throughput is below the configured floor: required={} measured={} bit/s",
                    required_floor, measured
                )
            });
        }
    }
    let minimum_bps = options
        .rx_floor_bps
        .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
    if host.throughput_bps() < minimum_bps {
        qualification_failure.get_or_insert_with(|| {
            String::from("host failed to offer at least 90% of the requested rate")
        });
    } else if rx.throughput_median_kbps < minimum_bps / 1_000 {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "device RX {} kbit/s is below the acceptance floor",
                rx.throughput_median_kbps,
            )
        });
    }
    if require_exact_delivery && let Some(fixture_rx) = fixture_rx.as_ref() {
        let expected_fixture_packets = host.datagrams.saturating_add(1);
        if fixture_rx.wireless_packets() != expected_fixture_packets {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "host/AP Wi-Fi egress mismatch: expected={} observed={} packets",
                    expected_fixture_packets,
                    fixture_rx.wireless_packets()
                )
            });
        }
    }
    let (required_width, minimum_rate) = options.phy.required_tx();
    let typed_tx = match structured.require_tx_radio(
        required_width,
        minimum_rate,
        u32::try_from(MIN_QUALIFIED_AGGREGATES).unwrap_or(u32::MAX),
    ) {
        Ok(evidence) => Some(evidence),
        Err(error) => {
            qualification_failure.get_or_insert_with(|| error.to_string());
            structured
                .radio
                .and_then(|radio| radio.tx)
                .zip(structured.tx_timing)
        }
    };
    {
        let evidence = structured;
        let expected_rx_bytes = evidence
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        if !evidence.finished.summary.passed || evidence.transport.transport_errors != 0 {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "target did not complete bidirectional session cleanly: passed={} errors={}",
                    evidence.finished.summary.passed, evidence.transport.transport_errors
                )
            });
        }
        if require_exact_delivery && let Some(delivery) = evidence.rx_delivery {
            let assessment = evidence::rx_delivery::assess(host.datagrams, delivery);
            if !assessment.exact() {
                qualification_failure.get_or_insert_with(|| {
                    format!(
                        "typed bidirectional RX delivery frontier is {}",
                        assessment.frontier()
                    )
                });
            }
        }
        if require_exact_delivery
            && (evidence.transport.rx_bytes != expected_rx_bytes
                || evidence.transport.rx_units != host.datagrams
                || evidence.transport.rx_bytes != host.bytes)
        {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "host/target bidirectional RX delivery mismatch: host={}/{} target={}/{}",
                    host.bytes,
                    host.datagrams,
                    evidence.transport.rx_bytes,
                    evidence.transport.rx_units
                )
            });
        }
    }
    let ampdu = typed_tx
        .map(|(tx, timing)| AmpduEvidence::from_typed(tx, timing))
        .unwrap_or_default();
    if require_exact_delivery
        && (host_tx.missing != 0 || host_tx.reordered != 0 || host_tx.duplicates != 0)
    {
        let target_tx_units = Some(structured.transport.tx_units);
        let post_block_ack_loss = target_tx_units
            .and_then(|units| post_block_ack_delivery_loss_lower_bound(ampdu, host_tx, units));
        qualification_failure.get_or_insert_with(|| format!(
            "host observed concurrent TX sequence defects: missing={} reordered={} duplicates={} \
             missing_runs={} maximum_missing_run={} maximum_missing_range={:?}..={:?} \
             target_tx_units={target_tx_units:?} ampdu_subframes={} block_acknowledged={} \
             post_block_ack_delivery_loss_lower_bound={post_block_ack_loss:?}",
            host_tx.missing,
            host_tx.reordered,
            host_tx.duplicates,
            host_tx.missing_runs,
            host_tx.maximum_missing_run,
            host_tx.maximum_missing_run_start,
            host_tx.maximum_missing_run_end,
            ampdu.subframes,
            ampdu.acknowledged,
        ));
    }
    if require_exact_delivery
        && (structured.transport.tx_units != host_tx.datagrams
            || structured.transport.tx_bytes != host_tx.bytes)
    {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "typed/host bidirectional TX mismatch: target={}/{} host={}/{}",
                structured.transport.tx_bytes,
                structured.transport.tx_units,
                host_tx.bytes,
                host_tx.datagrams
            )
        });
    }
    if require_exact_delivery
        && let Some(fixture_tx) = fixture_tx.as_ref()
        && fixture_tx.udp_packets != structured.transport.tx_units
    {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "target/local wireless bidirectional TX mismatch: target={} local_ingress={}",
                structured.transport.tx_units, fixture_tx.udp_packets
            )
        });
    }
    write_report(
        output,
        &options,
        BidirectionalEvidence {
            host_offer: host,
            target_rx: rx,
            target_tx_floor_kbps: tx_floor,
            host_sink: host_tx,
            session: structured,
            ampdu,
            host_receive_buffer_bytes,
            fixture_rx,
            fixture_tx,
            tx_monitor_rx,
            independent_air_rx,
        },
        require_exact_delivery,
        qualification_failure.as_deref(),
    )?;
    if let Some(failure) = qualification_failure {
        return Err(failure.into());
    }
    eprintln!(
        "OPENRADIOHOST result=PASS mode={}-bidirectional offered_kbps={} \
         host_kbps={} rx_median_kbps={rx_median} concurrent_tx_floor_kbps={tx_floor} \
         host_tx_kbps={} \
         host_receive_buffer_bytes={} \
         ampdu_avg_subframes={:.2} ampdu_max_subframes={} full32={} \
         combined_floor_sum_kbps={} report={}",
        options.phy.name(),
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        host_tx.throughput_kbps(),
        host_receive_buffer_bytes,
        ampdu.subframes as f64 / ampdu.aggregates.max(1) as f64,
        ampdu.maximum,
        ampdu.full32,
        rx_median.saturating_add(tx_floor),
        output.join("report.md").display(),
    );
    Ok(())
}

/// Minimum number of MPDUs positively acknowledged by the peer but absent
/// from the host's unique UDP delivery set.
///
/// The comparison is meaningful only when every typed target transmission was
/// represented by one A-MPDU subframe. A larger BlockAck set than host set
/// proves that at least the cardinality difference disappeared after the peer
/// accepted the MPDUs; it does not assume which individual sequences overlap.
pub(crate) fn post_block_ack_delivery_loss_lower_bound(
    ampdu: AmpduEvidence,
    host: Burst,
    target_tx_units: u64,
) -> Option<u64> {
    if ampdu.subframes != target_tx_units {
        return None;
    }
    let unique_host_datagrams = host.datagrams.saturating_sub(host.duplicates);
    Some(ampdu.acknowledged.saturating_sub(unique_host_datagrams))
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        address: Ipv4Addr::UNSPECIFIED,
        port: DEFAULT_PORT,
        rate_bps: DEFAULT_RATE_BPS,
        rx_floor_bps: None,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        tx_port: DEFAULT_TX_PORT,
        tx_payload: DEFAULT_TX_PAYLOAD,
        tx_rate_bps: None,
        tx_floor_bps: None,
        combined_floor_bps: None,
        serial: lab.device.serial.clone(),
        phy: Phy::He20,
    };
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("bidirectional option requires a value")?;
        match arguments[index].as_str() {
            "--rate" => options.rate_bps = parse_rate(value)?,
            "--rx-floor" => options.rx_floor_bps = Some(parse_rate(value)?),
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(5..=300).contains(&seconds) {
                    return Err("--seconds must be in 5..=300".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--payload" => {
                options.payload = value.parse::<usize>()?;
                if !(64..=1_472).contains(&options.payload) {
                    return Err("--payload must be in 64..=1472".into());
                }
            }
            "--port" => options.port = value.parse::<u16>()?,
            "--tx-payload" => {
                options.tx_payload = value.parse::<usize>()?;
                if !(64..=1_472).contains(&options.tx_payload) {
                    return Err("--tx-payload must be in 64..=1472".into());
                }
            }
            "--tx-port" => options.tx_port = value.parse::<u16>()?,
            "--tx-rate" => options.tx_rate_bps = Some(parse_rate(value)?),
            "--tx-floor" => options.tx_floor_bps = Some(parse_rate(value)?),
            "--combined-floor" => options.combined_floor_bps = Some(parse_rate(value)?),
            "--phy" => {
                options.phy = match value.as_str() {
                    "ht40" => Phy::Ht40,
                    "he20" => Phy::He20,
                    _ => return Err("--phy must be ht40 or he20".into()),
                };
            }
            other => return Err(format!("unknown bidirectional option `{other}`").into()),
        }
        index += 2;
    }
    if options.port == 0 || options.tx_port == 0 {
        return Err("--port and --tx-port must be nonzero".into());
    }
    if options
        .rx_floor_bps
        .is_some_and(|floor| floor > options.rate_bps)
    {
        return Err("--rx-floor cannot exceed --rate".into());
    }
    if options.tx_floor_bps.is_none() {
        options.tx_floor_bps = options.tx_rate_bps.map(|rate| rate.saturating_mul(9) / 10);
    }
    if options
        .tx_rate_bps
        .zip(options.tx_floor_bps)
        .is_some_and(|(rate, floor)| floor > rate)
    {
        return Err("--tx-floor cannot exceed --tx-rate".into());
    }
    if options.combined_floor_bps.is_some_and(|floor| {
        options
            .tx_rate_bps
            .and_then(|tx| options.rate_bps.checked_add(tx))
            .is_none_or(|offered| floor > offered)
    }) {
        return Err("--combined-floor requires --tx-rate and cannot exceed the offered sum".into());
    }
    Ok(options)
}

fn parse_rate(value: &str) -> Result<u64> {
    let (digits, multiplier) = match value.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1_000_u64),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1_000_000_u64),
        Some(b'g' | b'G') => (&value[..value.len() - 1], 1_000_000_000_u64),
        _ => (value, 1),
    };
    let rate = digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .ok_or("rate overflow")?;
    if !(100_000..=500_000_000).contains(&rate) {
        return Err("--rate must be in 100K..=500M".into());
    }
    Ok(rate)
}

fn parse_device_report(log: &str) -> DeviceReport {
    let mut report = DeviceReport::default();
    // Every production RX sample is followed by its path, pipeline, IRQ, PHY
    // and task-poll records. Readiness uses the same firmware endpoint and
    // therefore emits short probe samples too; their health counters must not
    // be merged into the sustained interval selected for qualification.
    let mut include_rx_interval_evidence = false;
    for line in log.lines() {
        if line.contains("OAMP aggregates=")
            && let (
                Some(aggregates),
                Some(publications),
                Some(completed),
                Some(subframes),
                Some(acknowledged),
                Some(single),
                Some(individual_retry),
                Some(timeout),
                Some(collision),
                Some(minimum),
                Some(maximum),
                Some(stop_frame),
                Some(stop_capacity),
                Some(stop_empty),
            ) = (
                field(line, "aggregates"),
                field(line, "publications"),
                field(line, "completed"),
                field(line, "subframes"),
                field(line, "acknowledged"),
                field(line, "single"),
                field(line, "individual_retry"),
                field(line, "timeout"),
                field(line, "collision"),
                field(line, "min"),
                field(line, "max"),
                field(line, "stop_frame"),
                field(line, "stop_capacity"),
                field(line, "stop_empty"),
            )
            && let (Ok(minimum), Ok(maximum)) = (u8::try_from(minimum), u8::try_from(maximum))
        {
            report.ampdu.push(AmpduSample {
                aggregates,
                publications,
                completed,
                subframes,
                acknowledged,
                single,
                individual_retry,
                timeout,
                collision,
                minimum,
                maximum,
                stop_frame,
                stop_capacity,
                stop_empty,
            });
        }
        if line.contains("OAMPH one=")
            && let (
                Some(one),
                Some(two_three),
                Some(four_seven),
                Some(eight_fifteen),
                Some(sixteen_twentythree),
                Some(twentyfour_thirty),
                Some(thirtyone),
                Some(full32),
            ) = (
                field(line, "one"),
                field(line, "two_three"),
                field(line, "four_seven"),
                field(line, "eight_fifteen"),
                field(line, "sixteen_twentythree"),
                field(line, "twentyfour_thirty"),
                field(line, "thirtyone"),
                field(line, "full32"),
            )
        {
            report.ampdu_histograms.push(AmpduHistogramSample {
                one,
                two_three,
                four_seven,
                eight_fifteen,
                sixteen_twentythree,
                twentyfour_thirty,
                thirtyone,
                full32,
            });
        }
        if line.contains("OAMPT preparation_us=")
            && let (
                Some(preparation_us),
                Some(preparation_max_us),
                Some(publication_us),
                Some(publication_max_us),
                Some(exchange_us),
                Some(exchange_max_us),
                Some(first_exchanges),
                Some(first_exchange_us),
                Some(first_exchange_max_us),
                Some(retried_exchanges),
                Some(retry_publications),
                Some(retry_exchange_us),
                Some(retry_exchange_max_us),
            ) = (
                field(line, "preparation_us"),
                field(line, "preparation_max_us"),
                field(line, "publication_us"),
                field(line, "publication_max_us"),
                field(line, "exchange_us"),
                field(line, "exchange_max_us"),
                field(line, "first_exchanges"),
                field(line, "first_exchange_us"),
                field(line, "first_exchange_max_us"),
                field(line, "retried_exchanges"),
                field(line, "retry_publications"),
                field(line, "retry_exchange_us"),
                field(line, "retry_exchange_max_us"),
            )
        {
            report.ampdu_timings.push(AmpduTimingSample {
                preparation_us,
                preparation_max_us,
                publication_us,
                publication_max_us,
                exchange_us,
                exchange_max_us,
                first_exchanges,
                first_exchange_us,
                first_exchange_max_us,
                retried_exchanges,
                retry_publications,
                retry_exchange_us,
                retry_exchange_max_us,
            });
        }
        if line.contains("OAMPI tx_irq_epochs=")
            && let (
                Some(tx_irq_epochs),
                Some(tx_irq_samples),
                Some(tx_irq_skew),
                Some(tx_irq_service_us),
                Some(tx_irq_service_max_us),
                Some(tx_flight_samples),
                Some(tx_flight_us),
                Some(tx_flight_max_us),
            ) = (
                field(line, "tx_irq_epochs"),
                field(line, "tx_irq_samples"),
                field(line, "tx_irq_skew"),
                field(line, "tx_irq_service_us"),
                field(line, "tx_irq_service_max_us"),
                field(line, "tx_flight_samples"),
                field(line, "tx_flight_us"),
                field(line, "tx_flight_max_us"),
            )
        {
            report.tx_irq_timings.push(TxIrqTimingSample {
                tx_irq_epochs,
                tx_irq_samples,
                tx_irq_skew,
                tx_irq_service_us,
                tx_irq_service_max_us,
                tx_flight_samples,
                tx_flight_us,
                tx_flight_max_us,
            });
        }
        if (line.starts_with("OAMPB ") || line.contains(" OAMPB "))
            && let (
                Some(samples),
                Some(received),
                Some(success_without),
                Some(nonzero_control),
                Some(start_outside),
                Some(start_lag_max),
                Some(full),
                Some(partial),
                Some(empty),
            ) = (
                field(line, "samples"),
                field(line, "received"),
                field(line, "success_without"),
                field(line, "nonzero_control"),
                field(line, "start_outside"),
                field(line, "start_lag_max"),
                field(line, "full"),
                field(line, "partial"),
                field(line, "empty"),
            )
        {
            report.ampdu_block_acks.push(AmpduBlockAckSample {
                samples,
                received,
                success_without,
                nonzero_control,
                start_outside,
                start_lag_max,
                full,
                partial,
                empty,
            });
        }
        if line.starts_with("ORXQ ") || line.contains(" ORXQ ") {
            let sample = (|| {
                Some(UdpSequenceEvidence {
                    intervals: 1,
                    first: optional_sequence_field(line, "first")?,
                    highest: optional_sequence_field(line, "highest")?,
                    next: optional_sequence_field(line, "next")?,
                    gap_events: field(line, "gap_events")?,
                    forward_missing: field(line, "forward_missing")?,
                    maximum_gap: field(line, "maximum_gap")?,
                    maximum_gap_at: optional_sequence_field(line, "maximum_gap_at")?,
                    first_gap_at: optional_sequence_field(line, "first_gap_at")?,
                    last_gap_at: optional_sequence_field(line, "last_gap_at")?,
                    backward: field(line, "backward")?,
                    adjacent_duplicates: field(line, "adjacent_duplicates")?,
                    unsequenced: field(line, "unsequenced")?,
                    maximum_interarrival_us: field(line, "maximum_interarrival_us")?,
                    maximum_interarrival_at: optional_sequence_field(
                        line,
                        "maximum_interarrival_at",
                    )?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_sequences.push(sample);
            }
        } else if line.starts_with("ORXO ") || line.contains(" ORXO ") {
            let sample = (|| {
                Some(RxOrderEvidence {
                    intervals: 1,
                    gap_events: field(line, "gap_events")?,
                    forward_missing: field(line, "forward_missing")?,
                    backward: field(line, "backward")?,
                    adjacent_duplicates: field(line, "adjacent_duplicates")?,
                    backward_mac_backward: field(line, "backward_mac_backward")?,
                    backward_mac_same: field(line, "backward_mac_same")?,
                    backward_mac_forward: field(line, "backward_mac_forward")?,
                    backward_mac_other_tid: field(line, "backward_mac_other_tid")?,
                    backward_mac_unavailable: field(line, "backward_mac_unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_order.push(sample);
            }
        } else if line.starts_with("ORTP ") || line.contains(" ORTP ") {
            let sample = (|| {
                Some(TaskPollEvidence {
                    intervals: 1,
                    polls: field(line, "polls")?,
                    poll_us: field(line, "poll_us")?,
                    poll_boot_max_us: field(line, "poll_boot_max_us")?,
                    over_100us: field(line, "over_100us")?,
                    over_500us: field(line, "over_500us")?,
                    over_1000us: field(line, "over_1000us")?,
                    over_5000us: field(line, "over_5000us")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                match text_field(line, "task") {
                    Some("network") => report.task_polls.network.merge(sample),
                    Some("protocol") => report.task_polls.protocol.merge(sample),
                    Some("radio") => report.task_polls.radio.merge(sample),
                    Some("udp_rx") => report.task_polls.udp_rx.merge(sample),
                    Some("udp_tx") => report.task_polls.udp_tx.merge(sample),
                    Some("tcp") => report.task_polls.tcp.merge(sample),
                    Some(_) | None => {}
                }
            }
        } else if line.starts_with("ORXS ") || line.contains(" ORXS ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    service_calls: field(line, "calls")?,
                    frontier_frames: field(line, "frontier")?,
                    admitted_frames: field(line, "admitted")?,
                    staged_bytes: field(line, "bytes")?,
                    stage_empty_discards: field(line, "discard_empty").unwrap_or(0),
                    stage_too_long_discards: field(line, "discard_long").unwrap_or(0),
                    backpressured_services: field(line, "back")?,
                    pool_credit_limited_services: field(line, "pool")?,
                    queue_credit_limited_services: field(line, "queue")?,
                    maximum_deferred_frames: field(line, "deferred_max")?,
                    minimum_backpressured_pool_credits: field(line, "pool_min")?,
                    minimum_backpressured_queue_credits: field(line, "queue_min")?,
                    maximum_frontier: field(line, "fmax")?,
                    maximum_admitted: field(line, "amax")?,
                    service_us: field(line, "service_us")?,
                    service_max_us: field(line, "service_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXD ") || line.contains(" ORXD ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    protocol_frames: field(line, "frames")?,
                    protocol_data_frames: field(line, "data")?,
                    protocol_amsdu_mpdus: field(line, "amsdu").unwrap_or(0),
                    protocol_amsdu_subframes: field(line, "amsdu_subframes").unwrap_or(0),
                    protocol_units_le_1700: field(line, "unit_le1700").unwrap_or(0),
                    protocol_units_1701_3400: field(line, "unit_1701_3400").unwrap_or(0),
                    protocol_units_over_3400: field(line, "unit_over3400").unwrap_or(0),
                    protocol_unit_max_bytes: field(line, "unit_boot_max_bytes").unwrap_or(0),
                    network_ready_waits: field(line, "waits")?,
                    network_ready_wait_us: field(line, "wait_us")?,
                    network_ready_wait_max_us: field(line, "wait_boot_max_us")?,
                    dispatch_us: field(line, "dispatch_us")?,
                    dispatch_max_us: field(line, "dispatch_boot_max_us")?,
                    network_publications: field(line, "publications")?,
                    network_published_bytes: field(line, "bytes")?,
                    network_publish_us: field(line, "publish_us")?,
                    network_publish_max_us: field(line, "publish_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_dispatch.push(sample);
            }
            if include_rx_interval_evidence
                && report.software_health.len() < report.rx.len()
                && let (Some(enqueued), Some(dropped)) =
                    (field(line, "enqueued"), field(line, "dropped"))
            {
                report.software_health.push((enqueued, dropped));
            }
        } else if line.starts_with("ORXB ") || line.contains(" ORXB ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    dma_buffer_full_increments: field(line, "increments")?,
                    dma_buffer_full_service_samples: field(line, "samples")?,
                    dma_buffer_full_between_services: field(line, "between").unwrap_or(0),
                    dma_buffer_full_during_services: field(line, "during").unwrap_or(0),
                    dma_buffer_full_between_service_samples: field(line, "between_samples")
                        .unwrap_or(0),
                    dma_buffer_full_during_service_samples: field(line, "during_samples")
                        .unwrap_or(0),
                    dma_buffer_full_last_service: field(line, "last_service")?,
                    dma_buffer_full_last_phase: field(line, "last_phase").unwrap_or(0),
                    dma_buffer_full_last_counter: field(line, "last_counter")?,
                    dma_buffer_full_last_frontier: field(line, "last_frontier")?,
                    dma_buffer_full_last_admitted: field(line, "last_admitted")?,
                    dma_buffer_full_last_pool_credits: field(line, "last_pool")?,
                    dma_buffer_full_last_queue_credits: field(line, "last_queue")?,
                    dma_buffer_full_last_service_us: field(line, "last_service_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXL ") || line.contains(" ORXL ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    reload_transactions: field(line, "transactions")?,
                    reload_us: field(line, "reload_us")?,
                    reload_max_us: field(line, "reload_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXR ") || line.contains(" ORXR ") {
            let sample = (|| {
                Some(RxReorderEvidence {
                    intervals: 1,
                    starts: field(line, "starts")?,
                    stops: field(line, "stops")?,
                    last_start_tid: field(line, "start_tid")?,
                    last_start_sequence: field(line, "start_seq")?,
                    window: field(line, "window")?,
                    first_samples: field(line, "first_samples")?,
                    first_tid: field(line, "first_tid")?,
                    first_start_sequence: field(line, "first_start")?,
                    first_frame_sequence: field(line, "first_seq")?,
                    first_distance: field(line, "first_distance")?,
                    buffered: field(line, "buffered")?,
                    released: field(line, "released")?,
                    missing: field(line, "missing")?,
                    stale: field(line, "stale")?,
                    gap_expiries: field(line, "expiries")?,
                    occupied: field(line, "occupied")?,
                    maximum_occupied: field(line, "occupied_max")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_reorder.push(sample);
            }
        } else if line.starts_with("ORXF ") || line.contains(" ORXF ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    frontier_zero_services: field(line, "zero")?,
                    frontier_one_services: field(line, "one")?,
                    frontier_two_three_services: field(line, "two_three")?,
                    frontier_four_seven_services: field(line, "four_seven")?,
                    frontier_eight_fifteen_services: field(line, "eight_fifteen")?,
                    frontier_sixteen_thirty_one_services: field(line, "sixteen_thirty_one")?,
                    frontier_thirty_two_plus_services: field(line, "thirty_two_plus")?,
                    rx_irq_posts: field(line, "irq_posts")?,
                    rx_irq_epochs: field(line, "irq_epochs")?,
                    mac_irq_entries: field(line, "irq_entries").unwrap_or(0),
                    rx_irq_coalesced_posts: field(line, "irq_coalesced")?,
                    rx_irq_service_samples: field(line, "irq_samples")?,
                    rx_irq_clock_skew_samples: field(line, "irq_skew")?,
                    rx_irq_to_service_us: field(line, "irq_service_us")?,
                    rx_irq_to_service_max_us: field(line, "irq_service_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_frontier.push(sample);
            }
        } else if line.starts_with("ORXI ") || line.contains(" ORXI ") {
            let sample = (|| {
                Some(MacIrqEvidence {
                    spurious_entries: field(line, "spurious")?,
                    rx_only_entries: field(line, "rx_only")?,
                    rx_mixed_entries: field(line, "rx_mixed")?,
                    tx_only_entries: field(line, "tx_only")?,
                    tx_mixed_entries: field(line, "tx_mixed")?,
                    other_only_entries: field(line, "other_only")?,
                    extra_nonzero_snapshots: field(line, "extra")?,
                    saturated_entries: field(line, "saturated")?,
                    auxiliary_status_or: field(line, "aux_or")?,
                    unknown_status_or: field(line, "unknown_or")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.mac_irq.push(sample);
            }
        } else if line.starts_with("ORXSM ") || line.contains(" ORXSM ") {
            let sample = (|| {
                Some(RxSmpduEvidence {
                    s_mpdu_datagrams: field(line, "s_mpdu")?,
                    not_s_mpdu_datagrams: field(line, "not_s_mpdu")?,
                    unavailable_datagrams: field(line, "unavailable")?,
                    s_mpdu_beacons: field(line, "beacon_s_mpdu")?,
                    not_s_mpdu_beacons: field(line, "beacon_not_s_mpdu")?,
                    unavailable_beacons: field(line, "beacon_unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_s_mpdu.push(sample);
            }
        } else if line.starts_with("ORXAG ") || line.contains(" ORXAG ") {
            let sample = (|| {
                Some(RxAmpduEvidence {
                    ampdu_datagrams: field(line, "ampdu")?,
                    not_ampdu_datagrams: field(line, "not_ampdu")?,
                    hardware_ampdu_datagrams: field(line, "hardware_ampdu")?,
                    hardware_not_ampdu_datagrams: field(line, "hardware_not_ampdu")?,
                    protocol_ampdu_datagrams: field(line, "protocol_ampdu")?,
                    protocol_not_ampdu_datagrams: field(line, "protocol_not_ampdu")?,
                    unavailable_datagrams: field(line, "unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_ampdu.push(sample);
            }
        } else if line.starts_with("ORXM ") || line.contains(" ORXM ") {
            let histogram = core::array::from_fn(|mcs| field(line, &format!("m{mcs}")));
            if include_rx_interval_evidence
                && histogram.iter().all(Option::is_some)
                && let Some(other) = field(line, "other")
            {
                report
                    .rx_mcs_histograms
                    .push((histogram.map(Option::unwrap), other));
            }
        } else if line.starts_with("ORX ") || line.contains(" ORX ") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) =
                (field(line, "d"), field(line, "u"), field(line, "k"))
            {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
                if let Some(address) = field(line, "code") {
                    report.code_addresses.push(address);
                }
                if report.dma_health.len() < report.rx.len()
                    && let (Some(buffer_full), Some(fifo_overflow)) =
                        (field(line, "full"), field(line, "overflow"))
                {
                    report.dma_health.push((buffer_full, fifo_overflow));
                }
            }
        } else if line.contains("OTX ") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) =
                (field(line, "k"), field(line, "w"), field(line, "r"))
            {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
                if let Some(address) = field(line, "a").or_else(|| field(line, "code")) {
                    report.code_addresses.push(address);
                }
            }
        } else if line.starts_with("ORXP ") || line.contains(" ORXP ") {
            if include_rx_interval_evidence && let Some(format) = field(line, "f") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("result=BENCH") && line.contains("stage=udp-rx-direct") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) = (
                field(line, "datagrams"),
                field(line, "elapsed_us"),
                field(line, "throughput_kbps"),
            ) {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
            }
        } else if line.contains("result=BENCH") && has_token(line, "stage=udp-rx") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) = (
                field(line, "datagrams"),
                field(line, "elapsed_us"),
                field(line, "throughput_kbps"),
            ) {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
            }
            if let Some(address) = field(line, "code_address") {
                report.code_addresses.push(address);
            }
        } else if line.contains("result=BENCH") && has_token(line, "stage=udp-rx-path") {
            if include_rx_interval_evidence
                && let (Some(buffer_full), Some(fifo_overflow)) =
                    (field(line, "buffer_full"), field(line, "fifo_overflow"))
            {
                report.dma_health.push((buffer_full, fifo_overflow));
            }
            if include_rx_interval_evidence
                && let (Some(enqueued), Some(dropped)) =
                    (field(line, "enqueued"), field(line, "queue_dropped"))
            {
                report.software_health.push((enqueued, dropped));
            }
            if include_rx_interval_evidence
                && let Some(format) =
                    field(line, "rx_format").filter(|format| *format <= u8::MAX.into())
            {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("result=BENCH") && line.contains("stage=raw-mac-tx") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) = (
                field(line, "throughput_kbps"),
                field(line, "bandwidth_mhz"),
                field(line, "rate_kbps"),
            ) {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
            }
        } else if line.contains("stage=udp-rx-phy") {
            if include_rx_interval_evidence && let Some(format) = field(line, "rx_format") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("stage=rx-runtime-delta") {
            if let (Some(buffer_full), Some(fifo_overflow)) =
                (field(line, "buffer_full"), field(line, "fifo_overflow"))
            {
                report.dma_health.push((buffer_full, fifo_overflow));
            }
        } else if line.contains("stage=tx-runtime")
            && let Some(address) = field(line, "code_address")
        {
            report.code_addresses.push(address);
        }
        if line.contains("result=FAIL")
            && (line.contains("raw-mac")
                || line.contains("embassy-net-radio")
                || line.contains("rx-output-reservation")
                || line.contains("mic-failure"))
        {
            report.failures.push(line.to_owned());
        }
    }
    report
}

fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.trim_end_matches(',').parse::<u64>().ok())?
    })
}

fn optional_sequence_field(line: &str, key: &str) -> Option<Option<u64>> {
    let value = field(line, key)?;
    Some((value != u64::from(u32::MAX)).then_some(value))
}

fn text_field<'line>(line: &'line str, key: &str) -> Option<&'line str> {
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.trim_end_matches(','))
    })
}

fn has_token(line: &str, expected: &str) -> bool {
    line.split_whitespace().any(|token| token == expected)
}

fn is_qualified_rx_sample(sample: ThroughputSample) -> bool {
    sample.elapsed_us >= MIN_QUALIFIED_SAMPLE.as_micros() as u64
        && sample.throughput_kbps > 0
        && sample.datagrams >= 16
}

fn qualified_rx_samples(report: &DeviceReport) -> Vec<ThroughputSample> {
    report
        .rx
        .iter()
        .copied()
        .filter(|sample| is_qualified_rx_sample(*sample))
        .collect()
}

fn qualify_runtime_marker(report: &DeviceReport) -> Result<()> {
    if report.code_addresses.is_empty()
        || report
            .code_addresses
            .iter()
            .any(|address| !(PSRAM_CODE_START..PSRAM_CODE_END).contains(address))
    {
        return Err("missing psram-code runtime marker".into());
    }
    Ok(())
}

#[cfg(test)]
fn qualify_tx_samples(
    report: &DeviceReport,
    required_width: u16,
    minimum_rate: u64,
) -> Result<u64> {
    if report.tx.is_empty()
        || report
            .tx
            .iter()
            .any(|sample| sample.bandwidth_mhz != required_width || sample.rate_kbps < minimum_rate)
    {
        return Err(format!(
            "TX did not remain at {required_width} MHz / at least {minimum_rate} kbit/s"
        )
        .into());
    }
    Ok(report
        .tx
        .iter()
        .map(|sample| sample.throughput_kbps)
        .min()
        .expect("nonempty TX samples"))
}

#[cfg(test)]
fn qualify_ampdu(report: &DeviceReport) -> Result<AmpduEvidence> {
    let ampdu = AmpduEvidence::from_report(report);
    if ampdu.aggregates < MIN_QUALIFIED_AGGREGATES || ampdu.completed == 0 {
        return Err(format!(
            "insufficient A-MPDU evidence: prepared={} completed={} required={MIN_QUALIFIED_AGGREGATES}",
            ampdu.aggregates, ampdu.completed,
        )
        .into());
    }
    if ampdu.maximum > 32 || ampdu.minimum == 0 {
        return Err(format!(
            "invalid A-MPDU size range: min={} max={}",
            ampdu.minimum, ampdu.maximum,
        )
        .into());
    }
    if ampdu.subframes <= ampdu.aggregates {
        return Err("TX never formed a multi-MPDU aggregate".into());
    }
    if ampdu.histogram_total() != ampdu.aggregates {
        return Err(format!(
            "incomplete A-MPDU histogram: buckets={} prepared={}",
            ampdu.histogram_total(),
            ampdu.aggregates,
        )
        .into());
    }
    let build_stop_total = ampdu
        .stop_frame
        .saturating_add(ampdu.stop_capacity)
        .saturating_add(ampdu.stop_empty);
    if build_stop_total != ampdu.aggregates {
        return Err(format!(
            "incomplete A-MPDU build-stop evidence: stops={build_stop_total} prepared={}",
            ampdu.aggregates,
        )
        .into());
    }
    if report.ampdu_timings.is_empty() {
        return Err("missing A-MPDU preparation/publication/exchange timing".into());
    }
    if report.tx_irq_timings.is_empty() {
        return Err("missing TX IRQ-to-service timing".into());
    }
    if report.ampdu_block_acks.is_empty() {
        return Err("missing A-MPDU BlockAck validity evidence".into());
    }
    let classified_block_acks = ampdu
        .full_block_ack
        .saturating_add(ampdu.partial_block_ack)
        .saturating_add(ampdu.empty_block_ack);
    if ampdu.block_ack_samples != ampdu.publications
        || classified_block_acks != ampdu.block_ack_samples
        // Receipt and bitmap coverage are independent axes. A received
        // BlockAck may contain an empty bitmap; a missing BlockAck is also
        // classified as empty because it acknowledges no subframes.
        || ampdu.block_ack_received > ampdu.block_ack_samples
    {
        return Err(format!(
            "inconsistent BlockAck evidence: samples={} publications={} received={} \
             full={} partial={} empty={}",
            ampdu.block_ack_samples,
            ampdu.publications,
            ampdu.block_ack_received,
            ampdu.full_block_ack,
            ampdu.partial_block_ack,
            ampdu.empty_block_ack,
        )
        .into());
    }
    if ampdu.block_ack_success_without != 0 || ampdu.block_ack_nonzero_control != 0 {
        return Err(format!(
            "invalid BlockAck validity/control evidence: success_without={} nonzero_control={}",
            ampdu.block_ack_success_without, ampdu.block_ack_nonzero_control,
        )
        .into());
    }
    if ampdu.tx_irq_epochs != 0 && ampdu.tx_irq_samples.saturating_add(ampdu.tx_irq_skew) == 0 {
        return Err("TX IRQ timing sampled no service edge".into());
    }
    // Publication-to-IRQ timing is available only in the intrusive MAC-IRQ
    // diagnostic image. The correctness image still owns complete aggregate
    // and BlockAck accounting, so zero samples are valid here.
    if ampdu.timeout != 0 || ampdu.collision != 0 {
        return Err(format!(
            "terminal A-MPDU failure: timeout={} collision={}",
            ampdu.timeout, ampdu.collision,
        )
        .into());
    }
    Ok(ampdu)
}

fn observe_rx_report(report: &DeviceReport, expected_format: u8) -> Result<RxQualification> {
    qualify_runtime_marker(report)?;
    if report.dma_health.is_empty() {
        return Err("missing RX DMA-health interval".into());
    }
    if report.software_health.is_empty() {
        return Err("missing RX software-queue health interval".into());
    }
    let rx = qualified_rx_samples(report);
    if rx.is_empty() {
        return Err("missing complete device-side direct-RX sample".into());
    }
    if report.rx_formats.is_empty()
        || report
            .rx_formats
            .iter()
            .any(|format| *format != expected_format)
    {
        return Err(format!("RX did not remain in baseband format {expected_format}").into());
    }
    if report.rx_service.is_empty()
        || report.rx_dispatch.is_empty()
        || report.rx_frontier.is_empty()
    {
        return Err("missing RX pipeline phase telemetry".into());
    }
    if report.rx_sequences.len() != rx.len() {
        return Err(format!(
            "incomplete UDP sequence evidence: records={} qualified_samples={}",
            report.rx_sequences.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_reorder.len() != rx.len() {
        return Err(format!(
            "incomplete RX BlockAck reorder evidence: records={} qualified_samples={}",
            report.rx_reorder.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_s_mpdu.len() != rx.len() {
        return Err(format!(
            "incomplete RX S-MPDU evidence: records={} qualified_samples={}",
            report.rx_s_mpdu.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_ampdu.len() != rx.len() {
        return Err(format!(
            "incomplete RX A-MPDU evidence: records={} qualified_samples={}",
            report.rx_ampdu.len(),
            rx.len(),
        )
        .into());
    }
    if report.mac_irq.is_empty() {
        return Err("missing MAC interrupt classification telemetry".into());
    }
    let mut pipeline = RxPipelineEvidence::default();
    for sample in report
        .rx_service
        .iter()
        .chain(&report.rx_dispatch)
        .chain(&report.rx_frontier)
    {
        pipeline.merge(*sample);
    }
    let mut irq = MacIrqEvidence::default();
    for sample in &report.mac_irq {
        irq.merge(*sample);
    }
    if pipeline.mac_irq_entries != 0 && irq.classified_entries() != pipeline.mac_irq_entries {
        return Err(format!(
            "incomplete MAC interrupt classification: classified={} hard_entries={}",
            irq.classified_entries(),
            pipeline.mac_irq_entries,
        )
        .into());
    }
    if pipeline.frontier_histogram_total() != pipeline.service_calls {
        return Err(format!(
            "incomplete RX frontier histogram: buckets={} services={}",
            pipeline.frontier_histogram_total(),
            pipeline.service_calls,
        )
        .into());
    }
    let mut he_mcs_histogram = [0_u64; 12];
    let mut other_phy_frames = 0_u64;
    for (histogram, other) in &report.rx_mcs_histograms {
        for (total, sample) in he_mcs_histogram.iter_mut().zip(histogram) {
            *total = total.saturating_add(*sample);
        }
        other_phy_frames = other_phy_frames.saturating_add(*other);
    }
    let mut s_mpdu = RxSmpduEvidence::default();
    for sample in &report.rx_s_mpdu {
        s_mpdu.merge(*sample);
    }
    if s_mpdu.observed_datagrams() == 0 {
        return Err("RX S-MPDU evidence did not observe a benchmark datagram".into());
    }
    if s_mpdu.unavailable_datagrams != 0 {
        return Err(format!(
            "RX S-MPDU provenance unavailable for {} benchmark datagrams",
            s_mpdu.unavailable_datagrams,
        )
        .into());
    }
    if s_mpdu.observed_beacons() == 0 {
        return Err("RX S-MPDU evidence did not observe a connected beacon".into());
    }
    if s_mpdu.unavailable_beacons != 0 {
        return Err(format!(
            "RX S-MPDU provenance unavailable for {} connected beacons",
            s_mpdu.unavailable_beacons,
        )
        .into());
    }
    let mut ampdu = RxAmpduEvidence::default();
    for sample in &report.rx_ampdu {
        ampdu.merge(*sample);
    }
    if ampdu.ampdu_datagrams
        != ampdu
            .hardware_ampdu_datagrams
            .saturating_add(ampdu.protocol_ampdu_datagrams)
        || ampdu.not_ampdu_datagrams
            != ampdu
                .hardware_not_ampdu_datagrams
                .saturating_add(ampdu.protocol_not_ampdu_datagrams)
    {
        return Err("RX A-MPDU totals do not match their provenance classes".into());
    }
    if expected_format == 2 {
        if ampdu.hardware_observed_datagrams() == 0 {
            return Err("HT RX did not carry direct HT-SIG A-MPDU evidence".into());
        }
        if ampdu.unavailable_datagrams != 0 {
            return Err(format!(
                "HT RX A-MPDU provenance unavailable for {} benchmark datagrams",
                ampdu.unavailable_datagrams,
            )
            .into());
        }
        if ampdu.ampdu_datagrams == 0 {
            return Err("HT RX did not observe an aggregated benchmark MPDU".into());
        }
        if ampdu.protocol_validated_datagrams() != 0 {
            return Err("HT RX A-MPDU evidence did not remain hardware-sourced".into());
        }
    } else if matches!(expected_format, 4..=7) {
        if ampdu.protocol_ampdu_datagrams == 0 {
            return Err("HE RX did not carry format-validated A-MPDU evidence".into());
        }
        if ampdu.protocol_not_ampdu_datagrams != 0
            || ampdu.hardware_observed_datagrams() != 0
            || ampdu.unavailable_datagrams != 0
        {
            return Err(format!(
                "HE RX A-MPDU provenance was not exclusively format-validated: \
                 protocol_ampdu={} protocol_not_ampdu={} hardware={} unavailable={}",
                ampdu.protocol_ampdu_datagrams,
                ampdu.protocol_not_ampdu_datagrams,
                ampdu.hardware_observed_datagrams(),
                ampdu.unavailable_datagrams,
            )
            .into());
        }
    }
    let mut sequence = UdpSequenceEvidence::default();
    for sample in &report.rx_sequences {
        sequence.merge(*sample);
    }
    let mut order = RxOrderEvidence::default();
    for sample in &report.rx_order {
        order.merge(*sample);
    }
    let mut reorder = RxReorderEvidence::default();
    for sample in &report.rx_reorder {
        reorder.merge(*sample);
    }
    if reorder.window == 0 || reorder.window > 64 || reorder.last_start_tid > 7 {
        return Err(format!(
            "invalid RX BlockAck agreement evidence: tid={} window={}",
            reorder.last_start_tid, reorder.window,
        )
        .into());
    }
    if reorder.occupied > reorder.maximum_occupied || reorder.maximum_occupied >= reorder.window {
        return Err(format!(
            "invalid RX reorder occupancy: current={} maximum={} window={}",
            reorder.occupied, reorder.maximum_occupied, reorder.window,
        )
        .into());
    }
    if reorder.first_samples != 0 {
        let distance = reorder
            .first_frame_sequence
            .wrapping_sub(reorder.first_start_sequence)
            & 0x0fff;
        if reorder.first_tid > 7
            || reorder.first_distance != distance
            // A forward frame outside the initial window is valid: the
            // reorder algorithm advances its window and accounts for the
            // missing prefix. Only the backward half of the 12-bit sequence
            // space is stale. Exact UDP delivery below still rejects any loss
            // inside the measured stream.
            || reorder.first_distance >= 0x0800
        {
            return Err(format!(
                "invalid first RX reorder frame: tid={} start={} sequence={} distance={} window={}",
                reorder.first_tid,
                reorder.first_start_sequence,
                reorder.first_frame_sequence,
                reorder.first_distance,
                reorder.window,
            )
            .into());
        }
    }
    Ok(RxQualification {
        throughput_median_kbps: median(rx.iter().map(|sample| sample.throughput_kbps).collect())
            .expect("nonempty RX samples"),
        sample_count: rx.len(),
        received_datagrams: rx.iter().map(|sample| sample.datagrams).sum(),
        enqueued: report
            .software_health
            .iter()
            .map(|(enqueued, _)| *enqueued)
            .sum(),
        dropped: report
            .software_health
            .iter()
            .map(|(_, dropped)| *dropped)
            .sum(),
        he_mcs_histogram,
        other_phy_frames,
        s_mpdu,
        ampdu,
        pipeline,
        irq,
        task_polls: report.task_polls,
        sequence,
        order,
        reorder,
        buffer_full: report.dma_health.iter().map(|(full, _)| *full).sum(),
        fifo_overflow: report
            .dma_health
            .iter()
            .map(|(_, overflow)| *overflow)
            .sum(),
    })
}

fn rx_policy_failure(report: &DeviceReport, rx: &RxQualification) -> Option<String> {
    if rx.buffer_full != 0 || rx.fifo_overflow != 0 {
        return Some(format!(
            "RX DMA starvation: buffer_full={} fifo_overflow={}",
            rx.buffer_full, rx.fifo_overflow,
        ));
    }
    if rx.dropped != 0 {
        return Some(format!("RX software queue dropped {} frames", rx.dropped));
    }
    if let Some(failure) = report.failures.first() {
        return Some(format!("device reported a data-path failure: {failure}"));
    }
    if rx.irq.saturated_entries != 0 {
        return Some(format!(
            "MAC interrupt acknowledgement loop saturated {} times",
            rx.irq.saturated_entries,
        ));
    }
    if rx.irq.unknown_status_or != 0 {
        return Some(format!(
            "MAC interrupt STATUS contains unqualified bits 0x{:08x}",
            rx.irq.unknown_status_or,
        ));
    }
    None
}

fn assess_rx_report(report: &DeviceReport, expected_format: u8) -> Result<RxAssessment> {
    let rx = observe_rx_report(report, expected_format)?;
    Ok(RxAssessment {
        failure: rx_policy_failure(report, &rx),
        rx,
    })
}

#[cfg(test)]
fn qualify_rx_report(report: &DeviceReport, expected_format: u8) -> Result<RxQualification> {
    let assessment = assess_rx_report(report, expected_format)?;
    if let Some(failure) = assessment.failure {
        return Err(failure.into());
    }
    Ok(assessment.rx)
}

pub(crate) fn assess_rx_log(log: &str, expected_format: u8) -> Result<RxAssessment> {
    assess_rx_report(&parse_device_report(log), expected_format)
}

#[cfg(test)]
fn qualify(
    options: &Options,
    host: HostTransmission,
    report: &DeviceReport,
) -> Result<RxQualification> {
    let rx = qualify_rx_report(report, options.phy.expected_rx_format())?;
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    if host.throughput_bps() < minimum_bps {
        return Err("host failed to offer at least 90% of the requested rate".into());
    }
    if rx.throughput_median_kbps < minimum_bps / 1_000 {
        return Err(format!(
            "device RX {} kbit/s is below the acceptance floor",
            rx.throughput_median_kbps,
        )
        .into());
    }
    validate_exact_rx_delivery(host.datagrams, rx.received_datagrams, rx.sequence, rx.order)?;
    let (required_width, minimum_rate) = options.phy.required_tx();
    qualify_tx_samples(report, required_width, minimum_rate)?;
    qualify_ampdu(report)?;
    Ok(rx)
}

#[cfg(test)]
pub(crate) fn validate_exact_rx_delivery(
    host_datagrams: u64,
    received_datagrams: u64,
    sequence: UdpSequenceEvidence,
    order: RxOrderEvidence,
) -> Result<()> {
    let expected_highest = host_datagrams.checked_sub(1);
    if received_datagrams != host_datagrams {
        return Err(format!(
            "device received {received_datagrams}/{host_datagrams} host UDP datagrams"
        )
        .into());
    }
    if sequence.first != Some(0)
        || sequence.highest != expected_highest
        || sequence.next != Some(host_datagrams)
        || sequence.gap_events != 0
        || sequence.forward_missing != 0
        || sequence.backward != 0
        || sequence.adjacent_duplicates != 0
        || sequence.unsequenced != 0
    {
        let localization = rx_order_localization(sequence, order);
        return Err(format!(
            "device RX sequence defects: first={:?} highest={:?} next={:?} gaps={} \
             missing={} backward={} duplicates={} unsequenced={}; localization={localization}",
            sequence.first,
            sequence.highest,
            sequence.next,
            sequence.gap_events,
            sequence.forward_missing,
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.unsequenced,
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
fn rx_order_localization(sequence: UdpSequenceEvidence, order: RxOrderEvidence) -> &'static str {
    if sequence.backward == 0 {
        return "no recovered late datagram is available for MAC-order correlation";
    }
    if order.intervals == 0 {
        return "RX order telemetry was not enabled";
    }
    if order.backward == 0 {
        return "reordering appeared after the pre-network ConnectedRx observer";
    }
    if order.backward != sequence.backward {
        return "reordering spans more than one observed RX boundary";
    }
    if order.backward_mac_forward == order.backward {
        return "all late UDP datagrams carried forward 802.11 sequence numbers; reordering predates the open driver's per-TID MAC/BlockAck boundary";
    }
    if order.backward_mac_backward != 0 {
        return "at least one late UDP datagram carried a backward 802.11 sequence number; inspect the open driver's per-TID BlockAck reorder path";
    }
    if order.backward_mac_same != 0 {
        return "at least one late UDP datagram shared an MPDU with its predecessor; inspect A-MSDU subframe order";
    }
    "MAC-order evidence is mixed or unavailable"
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some((values.get(middle - 1)? + values.get(middle)?) / 2)
    } else {
        values.get(middle).copied()
    }
}

pub(crate) fn task_poll_markdown(task_polls: TaskPollSet) -> String {
    if !task_polls.is_complete() {
        return String::from(
            "## Embassy task poll residence\n\n\
             Not collected in the ordinary throughput image. Use the explicit \
             `radio-poll-profile` HIL scenario for diagnostic instrumentation.\n\n",
        );
    }
    let mut output = String::from(
        "## Embassy task poll residence\n\n\
         Wall time includes interrupt preemption. Counters are diagnostic HIL instrumentation, \
         not a production scheduling policy.\n\n",
    );
    for (name, evidence) in [
        ("network", task_polls.network),
        ("protocol", task_polls.protocol),
        ("radio", task_polls.radio),
        ("udp_rx", task_polls.udp_rx),
        ("udp_tx", task_polls.udp_tx),
        ("tcp", task_polls.tcp),
    ] {
        let average_us = evidence.poll_us as f64 / evidence.polls.max(1) as f64;
        writeln!(
            output,
            "- `{name}`: {} polls / {} us total / {average_us:.2} us average / {} us boot \
             maximum; >100/500/1000/5000 us: {}/{}/{}/{} ({} interval records)",
            evidence.polls,
            evidence.poll_us,
            evidence.poll_boot_max_us,
            evidence.over_100us,
            evidence.over_500us,
            evidence.over_1000us,
            evidence.over_5000us,
            evidence.intervals,
        )
        .expect("writing task-poll evidence to String cannot fail");
    }
    output.push('\n');
    output
}

fn network_scheduler_markdown(
    evidence: Option<open_esp_radio_hil_protocol::NetworkSchedulerEvidence>,
) -> String {
    let Some(evidence) = evidence else {
        return String::from(
            "## Cooperative network scheduler\n\nNot collected in this image.\n\n",
        );
    };
    format!(
        "## Cooperative network scheduler\n\n\
         - Polls: `{}`\n\
         - Ingress calls/packets: `{}` / `{}`; egress passes/TX tokens: `{}` / `{}`\n\
         - Egress credit blocks: `{}`; ingress/egress budget exhaustion: `{}` / `{}`\n\
         - Start ingress/egress: `{}` / `{}`; exit drained/work/credit: `{}` / `{}` / `{}`\n\n",
        evidence.polls,
        evidence.ingress_calls,
        evidence.ingress_packets,
        evidence.egress_passes,
        evidence.egress_tx_tokens,
        evidence.egress_blocked,
        evidence.ingress_budget_exhausted,
        evidence.egress_budget_exhausted,
        evidence.started_with_ingress,
        evidence.started_with_egress,
        evidence.exit_drained,
        evidence.exit_work_budget,
        evidence.exit_egress_credit,
    )
}

pub(crate) fn udp_sequence_markdown(sequence: UdpSequenceEvidence, host_datagrams: u64) -> String {
    let value = |value: Option<u64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| String::from("not-observed"))
    };
    let tail_missing = sequence
        .highest
        .map(|highest| host_datagrams.saturating_sub(highest.saturating_add(1)))
        .unwrap_or(host_datagrams);
    let unrecovered_forward = sequence.forward_missing.saturating_sub(sequence.backward);
    format!(
        "## UDP sequence evidence\n\n\
         - Interval records: `{}`; first/highest/next sequence: `{}` / `{}` / `{}`; host tail after highest: `{tail_missing}`\n\
         - Forward gap events/missing observations/unrecovered after backward arrivals: `{}` / `{}` / `{unrecovered_forward}`\n\
         - Maximum gap/sequence after it: `{}` / `{}`; first/last sequence after a gap: `{}` / `{}`\n\
         - Backward observations/adjacent duplicates/unsequenced datagrams: `{}` / `{}` / `{}`\n\
         - Maximum application interarrival/sequence: `{} us` / `{}`\n\
         - Forward-missing observations are not reduced by later backward arrivals; the two counters remain separate so reordering is not mislabeled as loss.\n\n",
        sequence.intervals,
        value(sequence.first),
        value(sequence.highest),
        value(sequence.next),
        sequence.gap_events,
        sequence.forward_missing,
        sequence.maximum_gap,
        value(sequence.maximum_gap_at),
        value(sequence.first_gap_at),
        value(sequence.last_gap_at),
        sequence.backward,
        sequence.adjacent_duplicates,
        sequence.unsequenced,
        sequence.maximum_interarrival_us,
        value(sequence.maximum_interarrival_at),
    )
}

pub(crate) fn rx_order_markdown(order: RxOrderEvidence) -> String {
    if order.intervals == 0 {
        return String::from(
            "## RX order correlation\n\n\
             Not collected in the ordinary throughput image. Use the explicit \
             RX-order profile for this traffic scenario to correlate UDP and 802.11 ordering.\n\n",
        );
    }
    let classified = order
        .backward_mac_backward
        .saturating_add(order.backward_mac_same)
        .saturating_add(order.backward_mac_forward)
        .saturating_add(order.backward_mac_other_tid)
        .saturating_add(order.backward_mac_unavailable);
    format!(
        "## RX order correlation\n\n\
         - Observer interval records: `{}`; UDP gap events/forward-missing/backward/adjacent duplicates: `{}` / `{}` / `{}` / `{}`\n\
         - Backward UDP classified by 802.11 sequence: MAC-backward `{}`, same MPDU `{}`, MAC-forward `{}`, different TID `{}`, unavailable `{}`; classified `{classified}/{}`\n\
         - MAC-backward is direct evidence that an MPDU crossed the open driver out of its negotiated per-TID BlockAck order. MAC-forward means the application order had already changed before the peer assigned 802.11 sequence numbers. Same-MPDU records can be distinct A-MSDU subframes.\n\n",
        order.intervals,
        order.gap_events,
        order.forward_missing,
        order.backward,
        order.adjacent_duplicates,
        order.backward_mac_backward,
        order.backward_mac_same,
        order.backward_mac_forward,
        order.backward_mac_other_tid,
        order.backward_mac_unavailable,
        order.backward,
    )
}

pub(crate) fn rx_reorder_markdown(reorder: RxReorderEvidence) -> String {
    format!(
        "## RX BlockAck reorder\n\n\
         - Agreement interval records/starts/stops: `{}` / `{}` / `{}`; last TID/start/window: `{}` / `{}` / `{}`\n\
         - First-frame samples/TID/start/sequence/distance: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Buffered/released/missing/stale/gap expiries: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Occupied at report/maximum: `{}` / `{}` of `{}` negotiated slots\n\n",
        reorder.intervals,
        reorder.starts,
        reorder.stops,
        reorder.last_start_tid,
        reorder.last_start_sequence,
        reorder.window,
        reorder.first_samples,
        reorder.first_tid,
        reorder.first_start_sequence,
        reorder.first_frame_sequence,
        reorder.first_distance,
        reorder.buffered,
        reorder.released,
        reorder.missing,
        reorder.stale,
        reorder.gap_expiries,
        reorder.occupied,
        reorder.maximum_occupied,
        reorder.window,
    )
}

struct BidirectionalPerformanceReport<'a> {
    options: &'a Options,
    host_offer: HostTransmission,
    host_sink: Burst,
    structured: SessionEvidence,
    rx_kbps: u64,
    tx_kbps: u64,
    host_receive_buffer_bytes: usize,
}

fn write_bidirectional_performance_report(
    output: &Path,
    report: BidirectionalPerformanceReport<'_>,
) -> Result<()> {
    let BidirectionalPerformanceReport {
        options,
        host_offer,
        host_sink,
        structured,
        rx_kbps,
        tx_kbps,
        host_receive_buffer_bytes,
    } = report;
    let target_tx_rate = options
        .tx_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} bidirectional performance HIL\n\n\
             - Result: `PASS`\n\
             - Evidence boundary: `transport, external host source/sink, stack watermark; driver observation not collected`\n\
             - Device: `{}`\n\
             - Requested/actual downlink offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Target uplink offered-rate bound: `{target_tx_rate}`\n\
             - Target RX/TX throughput: `{:.3}` / `{:.3} Mbit/s`; combined `{:.3} Mbit/s`\n\
             - Host received target TX: `{:.3} Mbit/s`, `{}` bytes in `{}` datagrams\n\
             - Host target-TX missing/reordered/duplicate datagrams (informational): `{}` / `{}` / `{}`\n\
             - Target transport RX/TX: `{}` / `{}` bytes; `{}` / `{}` datagrams; `{}` us\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
             - Evidence CRC32C: `0x{:08x}`\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.name().to_uppercase(),
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host_offer.throughput_bps() as f64 / 1_000_000.0,
            rx_kbps as f64 / 1_000.0,
            tx_kbps as f64 / 1_000.0,
            rx_kbps.saturating_add(tx_kbps) as f64 / 1_000.0,
            host_sink.throughput_kbps() as f64 / 1_000.0,
            host_sink.bytes,
            host_sink.datagrams,
            host_sink.missing,
            host_sink.reordered,
            host_sink.duplicates,
            structured.transport.rx_bytes,
            structured.transport.tx_bytes,
            structured.transport.rx_units,
            structured.transport.tx_units,
            structured.transport.elapsed_micros,
            structured.stack.cpu0.free_bytes,
            structured.stack.cpu0.capacity_bytes,
            structured.stack.cpu0.minimum_free_bytes,
            structured.stack.cpu1.free_bytes,
            structured.stack.cpu1.capacity_bytes,
            structured.stack.cpu1.minimum_free_bytes,
            structured.finished.evidence_crc32c,
        ),
    )?;
    Ok(())
}

fn write_report(
    output: &Path,
    options: &Options,
    evidence: BidirectionalEvidence,
    require_exact_delivery: bool,
    failure: Option<&str>,
) -> Result<()> {
    let BidirectionalEvidence {
        host_offer: host,
        target_rx: rx,
        target_tx_floor_kbps: tx_floor,
        host_sink: host_tx,
        session: structured,
        ampdu,
        host_receive_buffer_bytes,
        fixture_rx,
        fixture_tx,
        tx_monitor_rx,
        independent_air_rx,
    } = evidence;
    let rx_median = rx.throughput_median_kbps;
    let pipeline = rx.pipeline;
    let irq = rx.irq;
    let average_service_us = pipeline.service_us as f64 / pipeline.admitted_frames.max(1) as f64;
    let average_reload_us = pipeline.reload_us as f64 / pipeline.reload_transactions.max(1) as f64;
    let average_dispatch_us = pipeline.dispatch_us as f64 / pipeline.protocol_frames.max(1) as f64;
    let average_publish_us =
        pipeline.network_publish_us as f64 / pipeline.network_publications.max(1) as f64;
    let average_wait_us =
        pipeline.network_ready_wait_us as f64 / pipeline.network_ready_waits.max(1) as f64;
    let average_irq_service_us =
        pipeline.rx_irq_to_service_us as f64 / pipeline.rx_irq_service_samples.max(1) as f64;
    let average_subframes = ampdu.subframes as f64 / ampdu.aggregates.max(1) as f64;
    let full32_percent = ampdu.full32 as f64 * 100.0 / ampdu.aggregates.max(1) as f64;
    let average_preparation_us = ampdu.preparation_us as f64 / ampdu.aggregates.max(1) as f64;
    let average_publication_us = ampdu.publication_us as f64 / ampdu.publications.max(1) as f64;
    let terminal_exchanges = ampdu
        .completed
        .saturating_add(ampdu.timeout)
        .saturating_add(ampdu.collision);
    let average_exchange_us = ampdu.exchange_us as f64 / terminal_exchanges.max(1) as f64;
    let average_first_exchange_us =
        ampdu.first_exchange_us as f64 / ampdu.first_exchanges.max(1) as f64;
    let average_retry_exchange_us =
        ampdu.retry_exchange_us as f64 / ampdu.retried_exchanges.max(1) as f64;
    let average_tx_irq_service_us =
        ampdu.tx_irq_service_us as f64 / ampdu.tx_irq_samples.max(1) as f64;
    let average_tx_flight_us = ampdu.tx_flight_us as f64 / ampdu.tx_flight_samples.max(1) as f64;
    let task_poll_report = task_poll_markdown(rx.task_polls);
    let network_scheduler_report = network_scheduler_markdown(structured.network_scheduler);
    let udp_sequence_report = udp_sequence_markdown(rx.sequence, host.datagrams);
    let rx_order_report = rx_order_markdown(rx.order);
    let rx_reorder_report = rx_reorder_markdown(rx.reorder);
    let typed_delivery_report = structured
        .rx_delivery
        .map(|delivery| evidence::rx_delivery::markdown(host.datagrams, delivery))
        .unwrap_or_else(|| {
            String::from(
                "## Typed RX delivery frontier\n\nNot collected in this image. Use the explicit RX-delivery profile.\n\n",
            )
        });
    let target_tx_rate = options
        .tx_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    let required_tx_floor = options
        .tx_floor_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("not configured"));
    let host_tx_mbps = host_tx.throughput_kbps() as f64 / 1_000.0;
    let host_tx_bytes = host_tx.bytes;
    let host_tx_datagrams = host_tx.datagrams;
    let host_tx_missing = host_tx.missing;
    let host_tx_reordered = host_tx.reordered;
    let host_tx_duplicates = host_tx.duplicates;
    let host_tx_maximum_interarrival_us = host_tx.maximum_interarrival_us;
    let host_tx_sequence_after_maximum_interarrival = host_tx.sequence_after_maximum_interarrival;
    let structured_report = format!(
        "- Typed session RX/TX: `{}` / `{}` bytes; `{}` / `{}` datagrams; CRC32C `0x{:08x}`\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n",
        structured.transport.rx_bytes,
        structured.transport.tx_bytes,
        structured.transport.rx_units,
        structured.transport.tx_units,
        structured.finished.evidence_crc32c,
        structured.stack.cpu0.free_bytes,
        structured.stack.cpu0.capacity_bytes,
        structured.stack.cpu0.minimum_free_bytes,
        structured.stack.cpu1.free_bytes,
        structured.stack.cpu1.capacity_bytes,
        structured.stack.cpu1.minimum_free_bytes,
    );
    let fixture_report = fixture_rx.as_ref().map_or_else(
        || String::from("- AP-side evidence: `external AP; not observed`\n"),
        |fixture| fixture.markdown(),
    );
    let fixture_tx_report = fixture_tx.as_ref().map_or_else(
        || String::from("- AP wireless ingress evidence: `not observed`\n"),
        |fixture| {
            format!(
                "- Local Linux AP filtered target TX ingress: `{}` packets; channel width: `{}` MHz; TX/RX bitrate: `{}` / `{}`\n",
                fixture.udp_packets,
                fixture.channel_width_mhz,
                fixture.tx_bitrate,
                fixture.rx_bitrate,
            )
        },
    );
    let tx_monitor_report = tx_monitor_rx.as_ref().map_or_else(
        || String::from("## OpenWrt AP TX-monitor frontier\n\nNot collected.\n\n"),
        |air| {
            let target = structured.rx_delivery.map(|delivery| delivery.post_reorder);
            let frontier = target.map_or("target delivery evidence unavailable", |target| {
                if air.unique_units == target.data_units
                    && air.unrecovered
                        == u32::try_from(host.datagrams)
                            .unwrap_or(u32::MAX)
                            .saturating_sub(target.data_units)
                {
                    "at or before the OpenWrt AP TX-monitor tap (tap and target have equal sequence cardinality)"
                } else {
                    "between the OpenWrt AP TX-monitor tap and target post-reorder"
                }
            });
            format!(
                "## OpenWrt AP TX-monitor frontier\n\n\
                 - Classification: `{frontier}`\n\
                 - Capture frames/kernel drops: `{}` / `{}`\n\
                 - UDP publications/unique/duplicates: `{}` / `{}` / `{}`\n\
                 - Gap/missing/late/unrecovered: `{}` / `{}` / `{}` / `{}`\n\
                 - Out-of-range/terminal/retry publications: `{}` / `{}` / `{}`\n\
                 - UDP publications missing MAC metadata: `{}`\n\
                 - First anomaly: `{:?}`\n\
                 - Target post-reorder data units: `{}`\n\n",
                air.captured_frames,
                air.kernel_dropped,
                air.data_units,
                air.unique_units,
                air.duplicates,
                air.gap_events,
                air.forward_missing,
                air.late_recovered,
                air.unrecovered,
                air.out_of_range,
                air.terminal_markers,
                air.mac_retry_publications,
                air.missing_mac_metadata,
                air.first_anomaly,
                target.map_or(0, |target| target.data_units),
            )
        },
    );
    let independent_air_report = match (tx_monitor_rx.as_ref(), independent_air_rx.as_ref()) {
        (Some(ap), Some(observer)) => {
            let ap_with_metadata = ap.mac_units.values().copied().sum::<u32>();
            let observer_with_metadata = observer.mac_units.values().copied().sum::<u32>();
            let ap_known_tid = ap
                .mac_units
                .iter()
                .filter(|(key, _)| key.tid != u8::MAX)
                .map(|(_, count)| count)
                .copied()
                .sum::<u32>();
            let observer_known_tid = observer
                .mac_units
                .iter()
                .filter(|(key, _)| key.tid != u8::MAX)
                .map(|(_, count)| count)
                .copied()
                .sum::<u32>();
            let ap_projected = project_mac_units(&ap.mac_units);
            let observer_projected = project_mac_units(&observer.mac_units);
            let matched = ap_projected
                .iter()
                .map(|(key, count)| count.min(observer_projected.get(key).unwrap_or(&0)))
                .copied()
                .sum::<u32>();
            let not_observed = ap_with_metadata.saturating_sub(matched);
            let observer_extra = observer_with_metadata.saturating_sub(matched);
            format!(
                "## Independent laptop air observer\n\n\
                 - Capture frames/kernel drops: `{}` / `{}`\n\
                 - Logical data MPDUs/retry attempts/missing metadata: `{}` / `{}` / `{}`\n\
                 - AP/observer logical MPDUs with MAC metadata: `{}` / `{}`\n\
                 - AP/observer MPDUs with known QoS TID: `{}` / `{}`\n\
                 - Matched by `(sequence, fragment)`: `{}`\n\
                 - AP MPDUs not observed / observer extras: `{}` / `{}`\n\
                 - Correlation limitation: `ath10k's AP monitor tap omits QoS TID; cross-device matching intentionally projects away TID`\n\
                 - Interpretation: `matched MPDUs prove transmission on air; unmatched MPDUs remain ambiguous because a passive observer may miss frames`\n\n",
                observer.captured_frames,
                observer.kernel_dropped,
                observer.logical_data_units,
                observer.retry_attempts,
                observer.missing_mac_metadata,
                ap_with_metadata,
                observer_with_metadata,
                ap_known_tid,
                observer_known_tid,
                matched,
                not_observed,
                observer_extra,
            )
        }
        (None, Some(_)) => String::from(
            "## Independent laptop air observer\n\nInvalid evidence: AP TX-monitor correlation is absent.\n\n",
        ),
        (_, None) => String::from("## Independent laptop air observer\n\nNot collected.\n\n"),
    };
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure_report = failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} bidirectional HIL\n\n\
             - Result: `{result}`\n\
             - Delivery contract: `{}`\n\
             - Device: `{}`\n\
             - Requested downlink: `{:.3} Mbit/s`\n\
             - Actual host offer: `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             - Target TX payload / offered bound: `{}` bytes / `{target_tx_rate}`\n\
             - Required target-to-host floor: `{required_tx_floor}`\n\
             - Host received target TX: `{host_tx_mbps:.3} Mbit/s`, `{host_tx_bytes}` bytes in `{host_tx_datagrams}` datagrams\n\
             - Host target-TX missing/reordered/duplicate datagrams: `{host_tx_missing}` / `{host_tx_reordered}` / `{host_tx_duplicates}`\n\
             - Host target-TX maximum packet interarrival: `{host_tx_maximum_interarrival_us}` us before sequence `{host_tx_sequence_after_maximum_interarrival:?}`\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             {failure_report}\
             {structured_report}\
             {fixture_report}\
             {fixture_tx_report}\
             - Host pacing maximum lateness/catch-up/deadline resets: `{} us` / `{}` datagrams / `{}`\n\
            - Direct RX median: `{:.3} Mbit/s`\n\
             - Received host UDP datagrams: `{}` / `{}`\n\
            - Concurrent open-radio TX floor: `{:.3} Mbit/s`\n\
             - Combined conservative floor: `{:.3} Mbit/s`\n\n\
             {udp_sequence_report}\
             {rx_order_report}\
             {rx_reorder_report}\
             {typed_delivery_report}\
             {tx_monitor_report}\
             {independent_air_report}\
             ## RX evidence\n\n\
             - Enqueued/software-dropped frames: `{}` / `{}`\n\
             - Sampled HE-SU MCS0..11 frame histogram: `{:?}`; other sampled PHY frames: `{}`\n\
             - Benchmark UDP datagrams marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Connected beacons marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Benchmark UDP datagrams marked A-MPDU / not A-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - A-MPDU provenance hardware true/false, protocol true/false: `{}` / `{}`, `{}` / `{}`\n\
             - Hardware BUFFER_FULL/FIFO_OVERFLOW: `{}` / `{}`\n\
             - DMA service calls/frontier/admitted: `{}` / `{}` / `{}`; max frontier/admitted: `{}` / `{}`\n\
             - Service-observed BUFFER_FULL increments/samples: `{}` / `{}`; between/during: `{}` / `{}` increments across `{}` / `{}` services; last boot service/phase/counter/frontier/admitted/pool/queue/service time: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{} us`\n\
             - Frontier service buckets 0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - RX IRQ posts/wake epochs/hard entries/coalesced/sampled services/clock-skew rejects: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; sampled IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - MAC entry causes spurious / RX-work-only / RX-mixed / TX-only / TX-mixed / auxiliary-or-unknown-only: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; classified `{}` entries; extra snapshots `{}`, loop saturations `{}`, auxiliary STATUS OR `0x{:08x}`, unknown STATUS OR `0x{:08x}`\n\
             - Staged bytes: `{}`; invalid empty/oversize units recycled: `{}` / `{}`; service: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Safe reload transactions: `{}`; `{:.2} us` average, `{}` us boot maximum; `{}` us total\n\
             - Backpressured services: `{}`; pool/queue credit limited: `{}` / `{}`; maximum deferred frames: `{}`; minimum pool/queue credits: `{}` / `{}`\n\
             - Protocol frames/data: `{}` / `{}`; dispatch: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - A-MSDU MPDUs/subframes: `{}` / `{}`; raw unit buckets <=1700 / 1701-3400 / >3400 bytes: `{}` / `{}` / `{}`; boot maximum: `{}` bytes\n\
             - Network publications/bytes: `{}` / `{}`; copy+publish: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network-ready waits: `{}`; `{:.2} us` average, `{}` us boot maximum\n\n\
             {task_poll_report}\
             {network_scheduler_report}\
             ## A-MPDU evidence\n\n\
             - Prepared/completed/publications: `{}` / `{}` / `{}`\n\
             - Subframes: `{}` total, `{:.2}` average, min `{}`, max `{}`\n\
             - Full 32-member aggregates: `{}` (`{:.2}%`)\n\
             - Size buckets 1 / 2-3 / 4-7 / 8-15 / 16-23 / 24-30 / 31 / 32: \
               `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - Build stop at frame limit / capacity limit / empty queue: \
               `{}` / `{}` / `{}`\n\
             - Acknowledged subframes: `{}`; individual fallback retries: `{}`\n\
             - Hardware timeouts/collisions: `{}` / `{}`\n\
             - BlockAck samples/received/full/partial/empty: `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - BlockAck success-without-valid/control-TID/start-outside/max-start-lag: `{}` / `{}` / `{}` / `{}`\n\
             - Preparation: `{:.2} us` average, `{}` us boot maximum\n\
             - Hardware publication programming: `{:.2} us` average, \
               `{}` us boot maximum\n\
             - Aggregate exchange: `{:.2} us` average, `{}` us boot maximum\n\n\
             - First-publication exchange: `{:.2} us` average, `{}` us boot maximum across `{}` exchanges\n\
             - Retried exchange: `{:.2} us` average, `{}` us boot maximum across `{}` exchanges and `{}` publications\n\
             - TX IRQ wake epochs/samples/clock-skew rejects: `{}` / `{}` / `{}`; IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - Sampled publication-to-IRQ flight: `{:.2} us` average, `{}` us boot maximum across `{}` samples\n\n\
             - Standby prepared/published/cancelled: `{}` / `{}` / `{}`\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.name().to_uppercase(),
            if require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            host.bytes,
            host.datagrams,
            options.tx_payload,
            host.maximum_lateness_us(),
            host.maximum_catch_up_datagrams,
            host.deadline_resets,
            rx_median as f64 / 1_000.0,
            rx.received_datagrams,
            host.datagrams,
            tx_floor as f64 / 1_000.0,
            rx_median.saturating_add(tx_floor) as f64 / 1_000.0,
            rx.enqueued,
            rx.dropped,
            rx.he_mcs_histogram,
            rx.other_phy_frames,
            rx.s_mpdu.s_mpdu_datagrams,
            rx.s_mpdu.not_s_mpdu_datagrams,
            rx.s_mpdu.unavailable_datagrams,
            rx.s_mpdu.s_mpdu_beacons,
            rx.s_mpdu.not_s_mpdu_beacons,
            rx.s_mpdu.unavailable_beacons,
            rx.ampdu.ampdu_datagrams,
            rx.ampdu.not_ampdu_datagrams,
            rx.ampdu.unavailable_datagrams,
            rx.ampdu.hardware_ampdu_datagrams,
            rx.ampdu.hardware_not_ampdu_datagrams,
            rx.ampdu.protocol_ampdu_datagrams,
            rx.ampdu.protocol_not_ampdu_datagrams,
            rx.buffer_full,
            rx.fifo_overflow,
            pipeline.service_calls,
            pipeline.frontier_frames,
            pipeline.admitted_frames,
            pipeline.maximum_frontier,
            pipeline.maximum_admitted,
            pipeline.dma_buffer_full_increments,
            pipeline.dma_buffer_full_service_samples,
            pipeline.dma_buffer_full_between_services,
            pipeline.dma_buffer_full_during_services,
            pipeline.dma_buffer_full_between_service_samples,
            pipeline.dma_buffer_full_during_service_samples,
            pipeline.dma_buffer_full_last_service,
            pipeline.dma_buffer_full_last_phase,
            pipeline.dma_buffer_full_last_counter,
            pipeline.dma_buffer_full_last_frontier,
            pipeline.dma_buffer_full_last_admitted,
            pipeline.dma_buffer_full_last_pool_credits,
            pipeline.dma_buffer_full_last_queue_credits,
            pipeline.dma_buffer_full_last_service_us,
            pipeline.frontier_zero_services,
            pipeline.frontier_one_services,
            pipeline.frontier_two_three_services,
            pipeline.frontier_four_seven_services,
            pipeline.frontier_eight_fifteen_services,
            pipeline.frontier_sixteen_thirty_one_services,
            pipeline.frontier_thirty_two_plus_services,
            pipeline.rx_irq_posts,
            pipeline.rx_irq_epochs,
            pipeline.mac_irq_entries,
            pipeline.rx_irq_coalesced_posts,
            pipeline.rx_irq_service_samples,
            pipeline.rx_irq_clock_skew_samples,
            average_irq_service_us,
            pipeline.rx_irq_to_service_max_us,
            irq.spurious_entries,
            irq.rx_only_entries,
            irq.rx_mixed_entries,
            irq.tx_only_entries,
            irq.tx_mixed_entries,
            irq.other_only_entries,
            irq.classified_entries(),
            irq.extra_nonzero_snapshots,
            irq.saturated_entries,
            irq.auxiliary_status_or,
            irq.unknown_status_or,
            pipeline.staged_bytes,
            pipeline.stage_empty_discards,
            pipeline.stage_too_long_discards,
            average_service_us,
            pipeline.service_max_us,
            pipeline.reload_transactions,
            average_reload_us,
            pipeline.reload_max_us,
            pipeline.reload_us,
            pipeline.backpressured_services,
            pipeline.pool_credit_limited_services,
            pipeline.queue_credit_limited_services,
            pipeline.maximum_deferred_frames,
            pipeline.minimum_backpressured_pool_credits,
            pipeline.minimum_backpressured_queue_credits,
            pipeline.protocol_frames,
            pipeline.protocol_data_frames,
            average_dispatch_us,
            pipeline.dispatch_max_us,
            pipeline.protocol_amsdu_mpdus,
            pipeline.protocol_amsdu_subframes,
            pipeline.protocol_units_le_1700,
            pipeline.protocol_units_1701_3400,
            pipeline.protocol_units_over_3400,
            pipeline.protocol_unit_max_bytes,
            pipeline.network_publications,
            pipeline.network_published_bytes,
            average_publish_us,
            pipeline.network_publish_max_us,
            pipeline.network_ready_waits,
            average_wait_us,
            pipeline.network_ready_wait_max_us,
            ampdu.aggregates,
            ampdu.completed,
            ampdu.publications,
            ampdu.subframes,
            average_subframes,
            ampdu.minimum,
            ampdu.maximum,
            ampdu.full32,
            full32_percent,
            ampdu.one,
            ampdu.two_three,
            ampdu.four_seven,
            ampdu.eight_fifteen,
            ampdu.sixteen_twentythree,
            ampdu.twentyfour_thirty,
            ampdu.thirtyone,
            ampdu.full32,
            ampdu.stop_frame,
            ampdu.stop_capacity,
            ampdu.stop_empty,
            ampdu.acknowledged,
            ampdu.individual_retry,
            ampdu.timeout,
            ampdu.collision,
            ampdu.block_ack_samples,
            ampdu.block_ack_received,
            ampdu.full_block_ack,
            ampdu.partial_block_ack,
            ampdu.empty_block_ack,
            ampdu.block_ack_success_without,
            ampdu.block_ack_nonzero_control,
            ampdu.block_ack_start_outside,
            ampdu.block_ack_start_lag_max,
            average_preparation_us,
            ampdu.preparation_max_us,
            average_publication_us,
            ampdu.publication_max_us,
            average_exchange_us,
            ampdu.exchange_max_us,
            average_first_exchange_us,
            ampdu.first_exchange_max_us,
            ampdu.first_exchanges,
            average_retry_exchange_us,
            ampdu.retry_exchange_max_us,
            ampdu.retried_exchanges,
            ampdu.retry_publications,
            ampdu.tx_irq_epochs,
            ampdu.tx_irq_samples,
            ampdu.tx_irq_skew,
            average_tx_irq_service_us,
            ampdu.tx_irq_service_max_us,
            average_tx_flight_us,
            ampdu.tx_flight_max_us,
            ampdu.tx_flight_samples,
            ampdu.standby_prepared,
            ampdu.standby_published,
            ampdu.standby_cancelled,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_device_mac_projection_ignores_missing_tid() {
        let mut units = BTreeMap::new();
        units.insert(
            MacFrameKey {
                tid: u8::MAX,
                sequence: 42,
                fragment: 0,
            },
            2,
        );
        units.insert(
            MacFrameKey {
                tid: 3,
                sequence: 42,
                fragment: 0,
            },
            1,
        );
        units.insert(
            MacFrameKey {
                tid: 0,
                sequence: 43,
                fragment: 1,
            },
            4,
        );

        assert_eq!(project_mac_units(&units).get(&(42, 0)), Some(&3));
        assert_eq!(project_mac_units(&units).get(&(43, 1)), Some(&4));
    }

    #[test]
    fn parses_rates_and_options() {
        let lab = LabConfig::for_test();
        assert_eq!(parse_rate("10M").unwrap(), 10_000_000);
        assert_eq!(parse_rate("2500K").unwrap(), 2_500_000);
        let options = parse_options(
            &[
                "--rate".into(),
                "55M".into(),
                "--phy".into(),
                "he20".into(),
                "--seconds".into(),
                "5".into(),
                "--tx-payload".into(),
                "1300".into(),
                "--tx-rate".into(),
                "50M".into(),
                "--combined-floor".into(),
                "90M".into(),
                "--tx-port".into(),
                "9100".into(),
            ],
            &lab,
        )
        .unwrap();
        assert_eq!(options.phy, Phy::He20);
        assert_eq!(options.duration, Duration::from_secs(5));
        assert_eq!(options.tx_payload, 1_300);
        assert_eq!(options.tx_rate_bps, Some(50_000_000));
        assert_eq!(options.tx_floor_bps, Some(45_000_000));
        assert_eq!(options.combined_floor_bps, Some(90_000_000));
        assert_eq!(options.tx_port, 9_100);

        let explicit_floor = parse_options(
            &[
                "--tx-rate".into(),
                "50M".into(),
                "--tx-floor".into(),
                "40M".into(),
            ],
            &lab,
        )
        .unwrap();
        assert_eq!(explicit_floor.tx_floor_bps, Some(40_000_000));
        assert!(
            parse_options(
                &[
                    "--tx-rate".into(),
                    "40M".into(),
                    "--tx-floor".into(),
                    "41M".into(),
                ],
                &lab
            )
            .is_err()
        );
    }

    #[test]
    fn ht40_tx_accepts_mcs7_long_or_short_gi_but_not_a_lower_vector() {
        let mut report = DeviceReport {
            tx: vec![TxSample {
                throughput_kbps: 80_000,
                bandwidth_mhz: 40,
                rate_kbps: 135_000,
            }],
            ..DeviceReport::default()
        };
        assert_eq!(qualify_tx_samples(&report, 40, 135_000).unwrap(), 80_000);

        report.tx[0].rate_kbps = 150_000;
        assert_eq!(qualify_tx_samples(&report, 40, 135_000).unwrap(), 80_000);

        report.tx[0].rate_kbps = 121_500;
        assert!(qualify_tx_samples(&report, 40, 135_000).is_err());
        report.tx[0].rate_kbps = 135_000;
        report.tx[0].bandwidth_mhz = 20;
        assert!(qualify_tx_samples(&report, 40, 135_000).is_err());
    }

    #[test]
    fn quantifies_delivery_loss_after_positive_block_ack() {
        let ampdu = AmpduEvidence {
            subframes: 31_101,
            acknowledged: 31_073,
            ..AmpduEvidence::default()
        };
        let host = Burst {
            datagrams: 31_005,
            missing: 96,
            ..Burst::default()
        };

        assert_eq!(
            post_block_ack_delivery_loss_lower_bound(ampdu, host, 31_101),
            Some(68),
        );
    }

    #[test]
    fn post_block_ack_comparison_requires_one_subframe_per_typed_tx_unit() {
        let ampdu = AmpduEvidence {
            subframes: 31_100,
            acknowledged: 31_073,
            ..AmpduEvidence::default()
        };
        let host = Burst {
            datagrams: 31_005,
            ..Burst::default()
        };

        assert_eq!(
            post_block_ack_delivery_loss_lower_bound(ampdu, host, 31_101),
            None,
        );
    }

    #[test]
    fn exact_rx_delivery_rejects_loss_and_reordering() {
        let exact = UdpSequenceEvidence {
            intervals: 1,
            first: Some(0),
            highest: Some(3),
            next: Some(4),
            ..UdpSequenceEvidence::default()
        };
        assert!(validate_exact_rx_delivery(4, 4, exact, RxOrderEvidence::default()).is_ok());

        let missing = UdpSequenceEvidence {
            gap_events: 1,
            forward_missing: 1,
            ..exact
        };
        assert!(validate_exact_rx_delivery(4, 3, missing, RxOrderEvidence::default()).is_err());

        let reordered = UdpSequenceEvidence {
            gap_events: 1,
            forward_missing: 1,
            backward: 1,
            ..exact
        };
        assert!(validate_exact_rx_delivery(4, 4, reordered, RxOrderEvidence::default()).is_err());
    }

    #[test]
    fn localizes_recovered_udp_reordering_across_rx_boundaries() {
        let sequence = UdpSequenceEvidence {
            first: Some(0),
            highest: Some(3),
            next: Some(4),
            gap_events: 1,
            forward_missing: 1,
            backward: 1,
            ..UdpSequenceEvidence::default()
        };
        let before_mac = RxOrderEvidence {
            intervals: 1,
            gap_events: 1,
            forward_missing: 1,
            backward: 1,
            backward_mac_forward: 1,
            ..RxOrderEvidence::default()
        };
        let error = validate_exact_rx_delivery(4, 4, sequence, before_mac)
            .unwrap_err()
            .to_string();
        assert!(error.contains("predates the open driver's per-TID MAC/BlockAck boundary"));

        let after_handoff = RxOrderEvidence {
            intervals: 1,
            ..RxOrderEvidence::default()
        };
        let error = validate_exact_rx_delivery(4, 4, sequence, after_handoff)
            .unwrap_err()
            .to_string();
        assert!(error.contains("after the pre-network ConnectedRx observer"));
    }

    #[test]
    fn excludes_readiness_probe_health_from_sustained_rx_evidence() {
        let report = parse_device_report(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=64 datagrams=1 elapsed_us=1 throughput_kbps=512000 code_address=1342257664\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=54 queue_dropped=1 rx_format=4\n\
             ORXQ first=0 highest=0 next=1 gap_events=0 forward_missing=0 maximum_gap=0 maximum_gap_at=4294967295 first_gap_at=4294967295 last_gap_at=4294967295 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=0 maximum_interarrival_at=4294967295\n\
             ORXO gap_events=0 forward_missing=0 backward=0 adjacent_duplicates=0 backward_mac_backward=0 backward_mac_same=0 backward_mac_forward=0 backward_mac_other_tid=0 backward_mac_unavailable=0\n\
             ORXSM s_mpdu=0 not_s_mpdu=1 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=1 beacon_unavailable=0\n\
             ORXAG ampdu=1 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=1 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=3 frontier=37 admitted=37 bytes=60860 back=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 fmax=31 amax=31 service_us=636 service_boot_max_us=503\n\
             ORTP task=network polls=2 poll_us=1626 poll_boot_max_us=1582 over_100us=1 over_500us=1 over_1000us=1 over_5000us=0\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=6000000 datagrams=5000 elapsed_us=5000000 throughput_kbps=9600 code_address=1342257664\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_format=4\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=2 forward_missing=3 maximum_gap=2 maximum_gap_at=100 first_gap_at=100 last_gap_at=3000 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=250 maximum_interarrival_at=100\n\
             ORXO gap_events=2 forward_missing=3 backward=3 adjacent_duplicates=0 backward_mac_backward=3 backward_mac_same=0 backward_mac_forward=0 backward_mac_other_tid=0 backward_mac_unavailable=0\n\
             ORXSM s_mpdu=100 not_s_mpdu=4900 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXL transactions=300 reload_us=1500 reload_boot_max_us=9\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n",
        );

        assert_eq!(report.software_health, [(5_000, 0)]);
        assert_eq!(report.dma_health, [(0, 0)]);
        assert_eq!(report.rx_service.len(), 2);
        assert_eq!(report.rx_service[0].service_calls, 5_000);
        assert_eq!(report.rx_service[1].reload_transactions, 300);
        assert_eq!(report.rx_service[1].reload_us, 1_500);
        assert_eq!(report.rx_service[1].reload_max_us, 9);
        assert_eq!(report.task_polls.network.intervals, 1);
        assert_eq!(report.task_polls.network.polls, 5_100);
        assert_eq!(report.rx_sequences.len(), 1);
        assert_eq!(report.rx_sequences[0].forward_missing, 3);
        assert_eq!(report.rx_order.len(), 1);
        assert_eq!(report.rx_order[0].backward_mac_backward, 3);
        assert_eq!(report.rx_s_mpdu.len(), 1);
        assert_eq!(report.rx_s_mpdu[0].not_s_mpdu_datagrams, 4_900);
        assert_eq!(report.rx_s_mpdu[0].not_s_mpdu_beacons, 50);
        assert_eq!(report.rx_ampdu.len(), 1);
        assert_eq!(report.rx_ampdu[0].protocol_ampdu_datagrams, 5_000);
        assert_eq!(report.rx_ampdu[0].unavailable_datagrams, 0);
    }

    #[test]
    fn qualifies_complete_he20_evidence() {
        let report = parse_device_report(
            "\0OTX b=50000000 d=1 u=5000000 k=80000 e=0 w=20 r=114700 g=1 x=0 l=1 a=1342257664\n\
             OAMP aggregates=120 publications=121 completed=120 subframes=3744 acknowledged=3744 single=2 individual_retry=0 timeout=0 collision=0 min=2 max=32 stop_frame=116 stop_capacity=0 stop_empty=4\n\
             OAMPH one=0 two_three=1 four_seven=1 eight_fifteen=1 sixteen_twentythree=1 twentyfour_thirty=0 thirtyone=0 full32=116\n\
             OAMPT preparation_us=1200 preparation_max_us=14 publication_us=605 publication_max_us=8 exchange_us=24000 exchange_max_us=240 first_exchanges=119 first_exchange_us=23760 first_exchange_max_us=210 retried_exchanges=1 retry_publications=2 retry_exchange_us=240 retry_exchange_max_us=240\n\
             OAMPI tx_irq_epochs=121 tx_irq_samples=2 tx_irq_skew=0 tx_irq_service_us=18 tx_irq_service_max_us=11 tx_flight_samples=2 tx_flight_us=390 tx_flight_max_us=210\n\
             OAMPB samples=121 received=120 success_without=0 nonzero_control=0 start_outside=0 start_lag_max=31 full=120 partial=0 empty=1\n\
             ORX b=6000000 d=5000 u=5000000 k=9600 code=1343154946\n\
             ORXP f=4 r=11 m=11\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=0 forward_missing=0 maximum_gap=0 maximum_gap_at=4294967295 first_gap_at=4294967295 last_gap_at=4294967295 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=100 maximum_interarrival_at=1\n\
             ORXSM s_mpdu=100 not_s_mpdu=4900 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXD frames=5000 data=5000 waits=5000 wait_us=1000 wait_boot_max_us=2 dispatch_us=150000 dispatch_boot_max_us=35 publications=5000 bytes=7570000 publish_us=60000 publish_boot_max_us=15\n\
             ORXR starts=0 stops=0 start_tid=0 start_seq=6 window=8 first_samples=1 first_tid=0 first_start=6 first_seq=6 first_distance=0 buffered=3 released=5000 missing=0 stale=0 expiries=0 occupied=0 occupied_max=7\n\
             ORXF zero=0 one=5000 two_three=0 four_seven=0 eight_fifteen=0 sixteen_thirty_one=0 thirty_two_plus=0 irq_posts=5000 irq_epochs=5000 irq_entries=5000 irq_coalesced=0 irq_samples=5000 irq_skew=0 irq_service_us=25000 irq_service_boot_max_us=8\n\
             ORXI spurious=0 rx_only=5000 rx_mixed=0 tx_only=0 tx_mixed=0 other_only=0 extra=0 saturated=0 aux_or=16777248 unknown_or=0\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=protocol polls=5000 poll_us=180000 poll_boot_max_us=120 over_100us=1 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_rx polls=5000 poll_us=125000 poll_boot_max_us=90 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_tx polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=tcp polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_format=4\n",
        );
        let options = parse_options(&[], &LabConfig::for_test()).unwrap();
        let host = HostTransmission {
            bytes: 6_250_000,
            datagrams: 5_000,
            elapsed: Duration::from_secs(5),
            maximum_lateness: Duration::from_micros(20),
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };
        qualify(&options, host, &report).unwrap();
        assert!(report.code_addresses.contains(&1_343_154_946));
        let ampdu = AmpduEvidence::from_report(&report);
        assert_eq!(ampdu.aggregates, 120);
        assert_eq!(ampdu.histogram_total(), 120);
        assert_eq!(ampdu.full32, 116);
        assert_eq!(ampdu.maximum, 32);
        assert_eq!(ampdu.preparation_us, 1_200);
        assert_eq!(ampdu.first_exchanges, 119);
        assert_eq!(ampdu.first_exchange_us, 23_760);
        assert_eq!(ampdu.retried_exchanges, 1);
        assert_eq!(ampdu.retry_publications, 2);
        assert_eq!(ampdu.retry_exchange_us, 240);
        assert_eq!(ampdu.tx_irq_samples, 2);
        assert_eq!(ampdu.tx_irq_service_us, 18);
        assert_eq!(ampdu.tx_flight_samples, 2);
        assert_eq!(ampdu.tx_flight_us, 390);
        assert_eq!(ampdu.tx_flight_max_us, 210);
    }

    #[test]
    fn parses_current_production_rx_benchmark_evidence() {
        let mut report = parse_device_report(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=6000000 datagrams=5000 elapsed_us=5000000 throughput_kbps=9600 receive_errors=0 terminal=1\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path mpdu=5000 data_success=5000 fcs_error=0 buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_irqs=5000 reload_delays=0 rx_format=4\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=2 forward_missing=4 maximum_gap=3 maximum_gap_at=100 first_gap_at=100 last_gap_at=4000 backward=1 adjacent_duplicates=2 unsequenced=0 maximum_interarrival_us=300 maximum_interarrival_at=4000\n\
             ORXSM s_mpdu=50 not_s_mpdu=4950 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXM m0=0 m1=0 m2=0 m3=0 m4=0 m5=0 m6=0 m7=20 m8=30 m9=4950 m10=0 m11=0 other=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXB increments=2 samples=1 between=1 during=1 between_samples=1 during_samples=1 last_service=6123 last_phase=3 last_counter=17 last_frontier=7 last_admitted=7 last_pool=60 last_queue=61 last_service_us=73\n\
             ORXD frames=5000 data=5000 waits=5000 wait_us=1000 wait_boot_max_us=2 dispatch_us=150000 dispatch_boot_max_us=35 publications=5000 bytes=7570000 publish_us=60000 publish_boot_max_us=15\n\
             ORXR starts=0 stops=0 start_tid=0 start_seq=6 window=8 first_samples=1 first_tid=0 first_start=6 first_seq=6 first_distance=0 buffered=3 released=5000 missing=0 stale=0 expiries=0 occupied=0 occupied_max=7\n\
             ORXF zero=0 one=5000 two_three=0 four_seven=0 eight_fifteen=0 sixteen_thirty_one=0 thirty_two_plus=0 irq_posts=5000 irq_epochs=5000 irq_entries=5000 irq_coalesced=0 irq_samples=5000 irq_skew=0 irq_service_us=25000 irq_service_boot_max_us=8\n\
             ORXI spurious=0 rx_only=5000 rx_mixed=0 tx_only=0 tx_mixed=0 other_only=0 extra=0 saturated=0 aux_or=16777248 unknown_or=0\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=protocol polls=5000 poll_us=180000 poll_boot_max_us=120 over_100us=1 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_rx polls=5000 poll_us=125000 poll_boot_max_us=90 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_tx polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=tcp polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             OTX b=50000000 d=1 u=5000000 k=80000 e=0 w=20 r=114700 g=1 x=0 l=1 a=1342257664\n\
             OAMP aggregates=120 publications=120 completed=120 subframes=3840 acknowledged=3840 single=0 individual_retry=0 timeout=0 collision=0 min=32 max=32 stop_frame=120 stop_capacity=0 stop_empty=0\n\
             OAMPH one=0 two_three=0 four_seven=0 eight_fifteen=0 sixteen_twentythree=0 twentyfour_thirty=0 thirtyone=0 full32=120\n\
             OAMPT preparation_us=1200 preparation_max_us=12 publication_us=600 publication_max_us=6 exchange_us=24000 exchange_max_us=210 first_exchanges=120 first_exchange_us=24000 first_exchange_max_us=210 retried_exchanges=0 retry_publications=0 retry_exchange_us=0 retry_exchange_max_us=0\n\
             OAMPI tx_irq_epochs=120 tx_irq_samples=2 tx_irq_skew=0 tx_irq_service_us=17 tx_irq_service_max_us=10 tx_flight_samples=2 tx_flight_us=388 tx_flight_max_us=204\n\
             OAMPB samples=120 received=120 success_without=0 nonzero_control=0 start_outside=0 start_lag_max=31 full=120 partial=0 empty=0\n",
        );

        assert_eq!(report.rx.len(), 1);
        assert_eq!(report.rx[0].throughput_kbps, 9_600);
        assert_eq!(report.rx_formats, [4]);
        assert_eq!(report.dma_health, [(0, 0)]);
        assert_eq!(report.software_health, [(5_000, 0)]);
        assert_eq!(
            report.rx_mcs_histograms,
            [([0, 0, 0, 0, 0, 0, 0, 20, 30, 4_950, 0, 0], 0)]
        );
        let rx = qualify_rx_report(&report, 4).unwrap();
        assert_eq!(rx.he_mcs_histogram[9], 4_950);
        assert_eq!(rx.s_mpdu.not_s_mpdu_datagrams, 4_950);
        assert_eq!(rx.s_mpdu.s_mpdu_datagrams, 50);
        assert_eq!(rx.s_mpdu.not_s_mpdu_beacons, 50);
        assert_eq!(rx.ampdu.protocol_ampdu_datagrams, 5_000);
        assert_eq!(rx.ampdu.unavailable_datagrams, 0);
        assert_eq!(rx.pipeline.service_calls, 5_000);
        assert_eq!(rx.pipeline.service_us, 100_000);
        assert_eq!(rx.pipeline.dma_buffer_full_increments, 2);
        assert_eq!(rx.pipeline.dma_buffer_full_service_samples, 1);
        assert_eq!(rx.pipeline.dma_buffer_full_between_services, 1);
        assert_eq!(rx.pipeline.dma_buffer_full_during_services, 1);
        assert_eq!(rx.pipeline.dma_buffer_full_between_service_samples, 1);
        assert_eq!(rx.pipeline.dma_buffer_full_during_service_samples, 1);
        assert_eq!(rx.pipeline.dma_buffer_full_last_service, 6_123);
        assert_eq!(rx.pipeline.dma_buffer_full_last_phase, 3);
        assert_eq!(rx.pipeline.dma_buffer_full_last_counter, 17);
        assert_eq!(rx.pipeline.dma_buffer_full_last_frontier, 7);
        assert_eq!(rx.pipeline.dma_buffer_full_last_admitted, 7);
        assert_eq!(rx.pipeline.dma_buffer_full_last_pool_credits, 60);
        assert_eq!(rx.pipeline.dma_buffer_full_last_queue_credits, 61);
        assert_eq!(rx.pipeline.dma_buffer_full_last_service_us, 73);
        assert_eq!(rx.pipeline.network_publish_us, 60_000);
        assert_eq!(rx.pipeline.frontier_one_services, 5_000);
        assert_eq!(rx.pipeline.rx_irq_to_service_us, 25_000);
        assert_eq!(rx.irq.rx_only_entries, 5_000);
        assert_eq!(rx.irq.classified_entries(), 5_000);
        assert_eq!(rx.irq.auxiliary_status_or, 0x0100_0020);
        assert_eq!(rx.irq.unknown_status_or, 0);
        assert_eq!(rx.task_polls.network.polls, 5_100);
        assert_eq!(rx.task_polls.radio.poll_us, 390_000);
        assert_eq!(rx.task_polls.radio.over_100us, 20);
        assert_eq!(rx.sequence.first, Some(0));
        assert_eq!(rx.sequence.highest, Some(4_999));
        assert_eq!(rx.sequence.forward_missing, 4);
        assert_eq!(rx.sequence.maximum_gap_at, Some(100));
        assert_eq!(rx.sequence.backward, 1);
        assert_eq!(rx.sequence.adjacent_duplicates, 2);
        assert_eq!(rx.sequence.maximum_interarrival_us, 300);
        assert_eq!(rx.sequence.maximum_interarrival_at, Some(4_000));
        assert_eq!(rx.reorder.window, 8);
        assert_eq!(rx.reorder.first_start_sequence, 6);
        assert_eq!(rx.reorder.first_frame_sequence, 6);
        assert_eq!(rx.reorder.buffered, 3);
        assert_eq!(rx.reorder.released, 5_000);
        assert_eq!(rx.reorder.maximum_occupied, 7);
        report.rx_reorder[0].first_frame_sequence = 28;
        report.rx_reorder[0].first_distance = 22;
        assert!(qualify_rx_report(&report, 4).is_ok());
        report.rx_reorder[0].first_frame_sequence = 5;
        report.rx_reorder[0].first_distance = 0x0fff;
        assert!(
            qualify_rx_report(&report, 4)
                .unwrap_err()
                .to_string()
                .contains("invalid first RX reorder frame")
        );
        report.rx_reorder[0].first_frame_sequence = 6;
        report.rx_reorder[0].first_distance = 0;
        assert_eq!(report.ampdu.len(), 1);
        assert_eq!(report.ampdu_histograms[0].full32, 120);
        assert_eq!(report.ampdu_timings[0].exchange_max_us, 210);

        report.rx_formats[0] = 2;
        report.rx_ampdu[0] = RxAmpduEvidence {
            ampdu_datagrams: 4_500,
            not_ampdu_datagrams: 500,
            hardware_ampdu_datagrams: 4_500,
            hardware_not_ampdu_datagrams: 500,
            ..RxAmpduEvidence::default()
        };
        assert_eq!(
            qualify_rx_report(&report, 2).unwrap().ampdu.ampdu_datagrams,
            4_500
        );
        report.rx_ampdu[0].unavailable_datagrams = 1;
        assert!(
            qualify_rx_report(&report, 2)
                .unwrap_err()
                .to_string()
                .contains("A-MPDU provenance unavailable")
        );
        report.rx_ampdu[0] = RxAmpduEvidence {
            ampdu_datagrams: 0,
            not_ampdu_datagrams: 5_000,
            hardware_not_ampdu_datagrams: 5_000,
            ..RxAmpduEvidence::default()
        };
        assert!(
            qualify_rx_report(&report, 2)
                .unwrap_err()
                .to_string()
                .contains("did not observe an aggregated benchmark MPDU")
        );
        report.rx_formats[0] = 4;
        report.rx_ampdu[0] = RxAmpduEvidence {
            ampdu_datagrams: 5_000,
            not_ampdu_datagrams: 0,
            protocol_ampdu_datagrams: 5_000,
            ..RxAmpduEvidence::default()
        };

        report.rx_s_mpdu[0].unavailable_datagrams = 1;
        assert!(
            qualify_rx_report(&report, 4)
                .unwrap_err()
                .to_string()
                .contains("S-MPDU provenance unavailable")
        );
        report.rx_s_mpdu[0].unavailable_datagrams = 0;
        report.rx_reorder[0].maximum_occupied = 8;
        assert!(
            qualify_rx_report(&report, 4)
                .unwrap_err()
                .to_string()
                .contains("invalid RX reorder occupancy")
        );
        report.rx_reorder[0].maximum_occupied = 7;
        report.dma_health[0] = (2, 0);
        let assessment = assess_rx_report(&report, 4).unwrap();
        assert_eq!(assessment.rx.buffer_full, 2);
        assert_eq!(assessment.rx.sequence.forward_missing, 4);
        assert_eq!(
            assessment.failure.as_deref(),
            Some("RX DMA starvation: buffer_full=2 fifo_overflow=0")
        );
    }
}
