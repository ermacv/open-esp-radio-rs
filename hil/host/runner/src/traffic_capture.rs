//! Shared UART capture and end-to-end readiness probes for traffic HIL cells.

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Capabilities, Command, DecodeCounters, Direction, Envelope, Event, EvidenceRecord, Finished,
    FrameDecoder, FrameEncoder, LinkHealth, NetworkSchedulerEvidence, OperationStatus,
    RadioEvidence, RxDeliveryEvidence, RxRadioEvidence, SessionConfig, SessionLinkRequirements,
    SessionReady, SessionState, StackUsage, StartupArtifactChunk, StartupArtifactStatus,
    StateChange, StationEpochEvidence, StationLifecycleEvent, TimebaseProbeEvidence,
    TimebaseProbeRequest, Transport, TransportEvidence, TxAggregateTimingEvidence, TxRadioEvidence,
    WifiMonitorCaptureRequest, WifiMonitorEvidence, WifiMonitorFrameChunk, WifiMonitorRequest,
    WifiRoleTransitionEvidence, WifiScanEvidence, WifiScanRequest, evidence_crc32c,
};
use zeroize::Zeroizing;

use crate::startup_artifact;
use crate::{Result, lab_config::LabConfig};

const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const DHCP_DISCOVERY_GRACE: Duration = Duration::from_millis(500);
const PROTOCOL_READY_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_ARTIFACT_TIMEOUT: Duration = Duration::from_secs(30);
const SERIAL_OPEN_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const SERIAL_OPEN_BUSY_RETRY: Duration = Duration::from_millis(50);
const PROTOCOL_EVENT_CAPACITY: usize = 16_384;

struct ProtocolEvents {
    messages: Mutex<Vec<Envelope<Event>>>,
    health: Mutex<ProtocolHealth>,
    changed: Condvar,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
struct ProtocolHealth {
    active: bool,
    boot_id: Option<u64>,
    next_sequence: u32,
    counters: DecodeCounters,
    #[serde(skip)]
    decoder_baseline: DecodeCounters,
    failure: Option<String>,
}

impl ProtocolHealth {
    fn observe(&mut self, message: &Envelope<Event>, decoder_counters: DecodeCounters) {
        match self.boot_id {
            None => {
                self.begin_boot(message, decoder_counters);
            }
            Some(boot_id) if boot_id != message.boot_id => {
                self.begin_boot(message, decoder_counters);
            }
            Some(_) if message.message_sequence != self.next_sequence => {
                let expected = self.next_sequence;
                self.next_sequence = message.message_sequence.wrapping_add(1);
                self.fail(format!(
                    "target message sequence discontinuity: expected {expected}, observed {}",
                    message.message_sequence
                ));
            }
            Some(_) => self.next_sequence = self.next_sequence.wrapping_add(1),
        }
    }

    fn begin_boot(&mut self, message: &Envelope<Event>, decoder_counters: DecodeCounters) {
        self.active = true;
        self.boot_id = Some(message.boot_id);
        self.next_sequence = message.message_sequence.wrapping_add(1);
        self.decoder_baseline = decoder_counters;
        self.counters = DecodeCounters::default();
        self.failure = None;
        if message.message_sequence != 0 || !matches!(message.body, Event::Hello(_)) {
            self.fail(format!(
                "boot {} began with {:?} at target message sequence {}, expected Hello at 0",
                message.boot_id, message.body, message.message_sequence
            ));
        }
    }

    fn update_decoder_counters(&mut self, totals: DecodeCounters) {
        self.counters = decode_counter_delta(totals, self.decoder_baseline);
    }

    fn decode_error(&mut self, error: impl std::fmt::Display) {
        if self.active {
            self.fail(format!(
                "HIL wire decode failure after protocol activation: {error}"
            ));
        }
    }

    fn fail(&mut self, failure: String) {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
    }
}

fn decode_counter_delta(totals: DecodeCounters, baseline: DecodeCounters) -> DecodeCounters {
    DecodeCounters {
        frames: totals.frames.saturating_sub(baseline.frames),
        cobs_errors: totals.cobs_errors.saturating_sub(baseline.cobs_errors),
        too_short: totals.too_short.saturating_sub(baseline.too_short),
        header_errors: totals.header_errors.saturating_sub(baseline.header_errors),
        framing_version_errors: totals
            .framing_version_errors
            .saturating_sub(baseline.framing_version_errors),
        message_kind_errors: totals
            .message_kind_errors
            .saturating_sub(baseline.message_kind_errors),
        protocol_version_errors: totals
            .protocol_version_errors
            .saturating_sub(baseline.protocol_version_errors),
        payload_length_errors: totals
            .payload_length_errors
            .saturating_sub(baseline.payload_length_errors),
        checksum_errors: totals
            .checksum_errors
            .saturating_sub(baseline.checksum_errors),
        deserialize_errors: totals
            .deserialize_errors
            .saturating_sub(baseline.deserialize_errors),
        overflows: totals.overflows.saturating_sub(baseline.overflows),
    }
}

/// Concurrent UART transcript retained across traffic setup and measurement.
pub(crate) struct SerialCapture {
    stop: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
    protocol: Arc<ProtocolEvents>,
    outbound: mpsc::Sender<Zeroizing<Vec<u8>>>,
    next_host_sequence: AtomicU32,
    next_session_id: AtomicU64,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpRxReady {
    pub(crate) address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpTxReady {
    pub(crate) address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TcpReady {
    pub(crate) address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionHandle {
    session_id: u64,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StationEpochHandle {
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WifiCommandHandle {
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionEvidence {
    pub(crate) transport: TransportEvidence,
    pub(crate) radio: Option<RadioEvidence>,
    pub(crate) tx_timing: Option<TxAggregateTimingEvidence>,
    pub(crate) rx_delivery: Option<RxDeliveryEvidence>,
    pub(crate) network_scheduler: Option<NetworkSchedulerEvidence>,
    pub(crate) stack: StackUsage,
    pub(crate) finished: Finished,
}

impl SessionEvidence {
    pub(crate) fn require_rx_radio_health(self, expected_format: u8) -> Result<RxRadioEvidence> {
        let rx = self
            .radio
            .and_then(|evidence| evidence.rx)
            .ok_or("session did not publish typed RX radio evidence")?;
        if rx.phy_format != expected_format {
            return Err(format!(
                "RX did not remain in baseband format {expected_format}: observed {}",
                rx.phy_format
            )
            .into());
        }
        if rx.dma_buffer_full != 0
            || rx.dma_fifo_overflow != 0
            || rx.network_dropped != 0
            || rx.irq_drain_saturated != 0
            || rx.unknown_irq_status != 0
        {
            return Err(format!("typed RX radio health failed: {rx:?}").into());
        }
        let s_mpdu_datagrams = rx
            .s_mpdu_datagrams
            .saturating_add(rx.not_s_mpdu_datagrams)
            .saturating_add(rx.s_mpdu_unavailable_datagrams);
        let s_mpdu_beacons = rx
            .s_mpdu_beacons
            .saturating_add(rx.not_s_mpdu_beacons)
            .saturating_add(rx.s_mpdu_unavailable_beacons);
        if s_mpdu_datagrams == 0
            || rx.s_mpdu_unavailable_datagrams != 0
            || s_mpdu_beacons == 0
            || rx.s_mpdu_unavailable_beacons != 0
        {
            return Err(format!("incomplete typed RX S-MPDU provenance: {rx:?}").into());
        }
        if rx.ampdu_datagrams
            != rx
                .hardware_ampdu_datagrams
                .saturating_add(rx.protocol_ampdu_datagrams)
            || rx.not_ampdu_datagrams
                != rx
                    .hardware_not_ampdu_datagrams
                    .saturating_add(rx.protocol_not_ampdu_datagrams)
        {
            return Err(format!("inconsistent typed RX A-MPDU provenance: {rx:?}").into());
        }
        if expected_format == 2 {
            if rx.hardware_ampdu_datagrams == 0
                || rx.ampdu_datagrams == 0
                || rx.protocol_ampdu_datagrams != 0
                || rx.protocol_not_ampdu_datagrams != 0
                || rx.ampdu_unavailable_datagrams != 0
            {
                return Err(format!("invalid typed HT A-MPDU provenance: {rx:?}").into());
            }
        } else if matches!(expected_format, 4..=7)
            && (rx.protocol_ampdu_datagrams == 0
                || rx.protocol_not_ampdu_datagrams != 0
                || rx.hardware_ampdu_datagrams != 0
                || rx.hardware_not_ampdu_datagrams != 0
                || rx.ampdu_unavailable_datagrams != 0)
        {
            return Err(format!("invalid typed HE A-MPDU provenance: {rx:?}").into());
        }
        if rx.reorder_window == 0
            || rx.reorder_window > 64
            || rx.reorder_tid > 7
            || rx.reorder_current_occupied > rx.reorder_maximum_occupied
            || rx.reorder_maximum_occupied >= u32::from(rx.reorder_window)
        {
            return Err(format!("invalid typed RX reorder agreement: {rx:?}").into());
        }
        if rx.reorder_first_samples != 0 {
            let distance = rx
                .reorder_first_sequence
                .wrapping_sub(rx.reorder_first_start)
                & 0x0fff;
            if rx.reorder_first_tid > 7
                || rx.reorder_first_distance != distance
                || rx.reorder_first_distance >= 0x0800
            {
                return Err(format!("invalid typed first RX reorder frame: {rx:?}").into());
            }
        }
        if rx.rx_frontier_histogram_samples != rx.rx_service_calls
            || (rx.mac_irq_entries != 0 && rx.mac_irq_classified_entries != rx.mac_irq_entries)
        {
            return Err(format!("incomplete typed RX accounting: {rx:?}").into());
        }
        Ok(rx)
    }

    pub(crate) fn require_rx_radio(
        self,
        expected_format: u8,
        expected_datagrams: u64,
    ) -> Result<RxRadioEvidence> {
        let rx = self.require_rx_radio_health(expected_format)?;
        let expected_datagrams = u32::try_from(expected_datagrams).map_err(
            |_| "RX qualification sent more datagrams than typed evidence can represent",
        )?;
        if rx.sequence_first != Some(0)
            || rx.sequence_highest != expected_datagrams.checked_sub(1)
            || rx.sequence_gap_events != 0
            || rx.sequence_forward_missing != 0
            || rx.sequence_backward != 0
            || rx.sequence_duplicates != 0
            || rx.sequence_unsequenced != 0
        {
            return Err(format!("typed RX sequence evidence is not exact: {rx:?}").into());
        }
        Ok(rx)
    }

    pub(crate) fn require_tx_radio(
        self,
        required_width: u16,
        minimum_rate_kbps: u64,
        minimum_aggregates: u32,
    ) -> Result<(TxRadioEvidence, TxAggregateTimingEvidence)> {
        let tx = self
            .radio
            .and_then(|evidence| evidence.tx)
            .ok_or("session did not publish typed TX radio evidence")?;
        if tx.bandwidth_mhz != required_width
            || u64::from(tx.aggregate_rate_kbps) < minimum_rate_kbps
        {
            return Err(format!(
                "TX did not remain at {required_width} MHz / at least {minimum_rate_kbps} kbit/s: {tx:?}"
            )
            .into());
        }
        if tx.aggregates_prepared < minimum_aggregates || tx.aggregates_completed == 0 {
            return Err(format!("insufficient typed A-MPDU evidence: {tx:?}").into());
        }
        if tx.minimum_subframes == 0
            || tx.maximum_subframes > 32
            || tx.subframes_prepared <= tx.aggregates_prepared
        {
            return Err(format!("invalid typed A-MPDU size evidence: {tx:?}").into());
        }
        let histogram_total = tx
            .prepared_histogram
            .iter()
            .copied()
            .fold(0_u32, u32::saturating_add);
        let stop_total = tx
            .stopped_at_frame_limit
            .saturating_add(tx.stopped_at_capacity_limit)
            .saturating_add(tx.stopped_on_empty_queue);
        if histogram_total != tx.aggregates_prepared || stop_total != tx.aggregates_prepared {
            return Err(format!("incomplete typed A-MPDU accounting: {tx:?}").into());
        }
        let classified = tx
            .full_block_ack
            .saturating_add(tx.partial_block_ack)
            .saturating_add(tx.empty_block_ack);
        if tx.block_ack_samples != tx.aggregate_publications
            || classified != tx.block_ack_samples
            // Receipt and bitmap coverage are independent axes. A received
            // BlockAck may contain an empty bitmap; a missing BlockAck is also
            // classified as empty because it acknowledges no subframes.
            || tx.block_ack_received > tx.block_ack_samples
            || tx.success_without_block_ack != 0
            || tx.nonzero_block_ack_control != 0
        {
            return Err(format!("inconsistent typed BlockAck evidence: {tx:?}").into());
        }
        if tx.tx_irq_epochs != 0
            && tx
                .tx_irq_service_samples
                .saturating_add(tx.tx_irq_clock_skew_samples)
                == 0
        {
            return Err("typed TX IRQ evidence contains no service edge".into());
        }
        if tx.tx_publication_to_irq_samples == 0 {
            return Err("typed TX evidence contains no publication-to-IRQ flight".into());
        }
        if tx.hardware_timeouts != 0 || tx.collisions != 0 {
            return Err(format!("terminal typed A-MPDU failure: {tx:?}").into());
        }
        let timing = self
            .tx_timing
            .ok_or("session did not publish typed aggregate-TX timing evidence")?;
        validate_tx_timing(tx, timing)?;
        Ok((tx, timing))
    }
}

fn validate_tx_timing(tx: TxRadioEvidence, timing: TxAggregateTimingEvidence) -> Result<()> {
    for (name, samples, total, maximum) in [
        (
            "preparation",
            tx.aggregates_prepared,
            timing.preparation_micros,
            timing.preparation_max_micros,
        ),
        (
            "publication",
            tx.aggregate_publications,
            timing.publication_micros,
            timing.publication_max_micros,
        ),
        (
            "exchange",
            tx.aggregates_completed,
            timing.exchange_micros,
            timing.exchange_max_micros,
        ),
        (
            "first exchange",
            timing.first_exchanges,
            timing.first_exchange_micros,
            timing.first_exchange_max_micros,
        ),
        (
            "retried exchange",
            timing.retried_exchanges,
            timing.retry_exchange_micros,
            timing.retry_exchange_max_micros,
        ),
        (
            "IRQ-to-service",
            timing.tx_irq_service_samples,
            timing.tx_irq_service_micros,
            timing.tx_irq_service_max_micros,
        ),
        (
            "publication-to-IRQ",
            timing.tx_publication_to_irq_samples,
            timing.tx_publication_to_irq_micros,
            timing.tx_publication_to_irq_max_micros,
        ),
    ] {
        if samples != 0 && (total == 0 || maximum == 0 || maximum > total) {
            return Err(format!(
                "inconsistent typed {name} timing: samples={samples} total={total} max={maximum}"
            )
            .into());
        }
        if samples == 0 && (total != 0 || maximum != 0) {
            return Err(format!(
                "typed {name} timing has values without samples: total={total} max={maximum}"
            )
            .into());
        }
    }
    if timing
        .first_exchanges
        .saturating_add(timing.retried_exchanges)
        != tx.aggregates_completed
        || timing.tx_irq_epochs != tx.tx_irq_epochs
        || timing.tx_irq_service_samples != tx.tx_irq_service_samples
        || timing.tx_irq_clock_skew_samples != tx.tx_irq_clock_skew_samples
        || timing.tx_publication_to_irq_samples != tx.tx_publication_to_irq_samples
        || timing.standby_prepared
            != timing
                .standby_published
                .saturating_add(timing.standby_cancelled)
    {
        return Err(format!(
            "typed aggregate-TX timing does not match radio ownership: radio={tx:?} timing={timing:?}"
        )
        .into());
    }
    Ok(())
}

fn validate_stack_usage(usage: StackUsage) -> Result<()> {
    for (name, watermark) in [("cpu0", usage.cpu0), ("cpu1", usage.cpu1)] {
        if watermark.capacity_bytes == 0
            || watermark.free_bytes > watermark.capacity_bytes
            || watermark.used_bytes > watermark.capacity_bytes
            || watermark.free_bytes + watermark.used_bytes != watermark.capacity_bytes
            || watermark.minimum_free_bytes == 0
            || watermark.minimum_free_bytes > watermark.capacity_bytes
        {
            return Err(format!("device reported inconsistent {name} stack watermark").into());
        }
        if watermark.free_bytes < watermark.minimum_free_bytes {
            return Err(format!(
                "{name} stack headroom is below policy: free={} capacity={} required={} bytes",
                watermark.free_bytes, watermark.capacity_bytes, watermark.minimum_free_bytes
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) struct MonitorCaptureEvidence {
    pub(crate) chunks: Vec<WifiMonitorFrameChunk>,
    pub(crate) summary: WifiMonitorEvidence,
}

impl SerialCapture {
    /// Open the diagnostics owner before resetting the USB-Serial/JTAG target.
    ///
    /// Traffic qualification needs the DHCP and UDP-ready records from the
    /// current boot. Resetting through a second process after opening this
    /// handle is impossible because `serialport` owns the device exclusively.
    pub(crate) fn start_with_reset(port: &Path) -> Self {
        Self::start_inner(port, true)
    }

    /// Boot-smoke deliberately runs before the full radio/protocol runtime.
    /// Its only contract is that the relocated Embassy timer executes once.
    pub(crate) fn wait_for_boot_smoke(&self, timeout: Duration) -> Result<()> {
        const PASS: &[u8] = b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS";
        const PANIC: &[u8] = b"OPEN_RADIO_HIL runtime=PANIC";
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let bytes = self
                .bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bytes
                .windows(PANIC.len())
                .any(|candidate| candidate == PANIC)
            {
                return Err("boot-smoke runtime panicked".into());
            }
            if bytes.windows(PASS.len()).any(|candidate| candidate == PASS) {
                return Ok(());
            }
            drop(bytes);
            thread::sleep(Duration::from_millis(10));
        }
        Err("boot-smoke timer completion was not observed".into())
    }

    fn start_inner(port: &Path, reset_target: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let protocol = Arc::new(ProtocolEvents {
            messages: Mutex::new(Vec::new()),
            health: Mutex::new(ProtocolHealth::default()),
            changed: Condvar::new(),
        });
        let (outbound, outbound_rx) = mpsc::channel::<Zeroizing<Vec<u8>>>();
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let worker_protocol = Arc::clone(&protocol);
        let port = port.to_owned();
        let worker = thread::spawn(move || {
            let mut serial = match open_serial_after_busy_release(&port) {
                Ok(serial) => serial,
                Err(error) => {
                    append(
                        &worker_bytes,
                        format!("serial capture failed for {}: {error}\n", port.display())
                            .as_bytes(),
                    );
                    return;
                }
            };
            if reset_target {
                let _ = serial.clear(serialport::ClearBuffer::Input);
            }
            if reset_target && let Err(error) = reset_usb_serial_jtag(&mut *serial) {
                append(
                    &worker_bytes,
                    format!(
                        "serial target reset failed for {}: {error}\n",
                        port.display()
                    )
                    .as_bytes(),
                );
                return;
            }
            let mut decoder = FrameDecoder::new();
            let mut buffer = [0_u8; 2_048];
            while !worker_stop.load(Ordering::Acquire) {
                while let Ok(frame) = outbound_rx.try_recv() {
                    if let Err(error) = serial.write_all(frame.as_slice()) {
                        append(
                            &worker_bytes,
                            format!("\nserial write failed: {error}\n").as_bytes(),
                        );
                        return;
                    }
                }
                match serial.read(&mut buffer) {
                    Ok(length) => {
                        append(&worker_bytes, &buffer[..length]);
                        let decoder_counters_before_read = decoder.counters();
                        decoder.feed::<Event>(&buffer[..length], |message| match message {
                            Ok(message) => {
                                worker_protocol
                                    .health
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .observe(&message, decoder_counters_before_read);
                                let mut messages = worker_protocol
                                    .messages
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let capacity_exhausted = messages.len() == PROTOCOL_EVENT_CAPACITY;
                                if !capacity_exhausted {
                                    messages.push(message);
                                }
                                drop(messages);
                                if capacity_exhausted {
                                    worker_protocol
                                        .health
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .fail(format!(
                                            "host protocol event capacity {PROTOCOL_EVENT_CAPACITY} exhausted"
                                        ));
                                }
                                worker_protocol.changed.notify_all();
                            }
                            Err(error) => worker_protocol
                                .health
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .decode_error(error),
                        });
                        worker_protocol
                            .health
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .update_decoder_counters(decoder.counters());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(error) => {
                        append(
                            &worker_bytes,
                            format!("\nserial read failed: {error}\n").as_bytes(),
                        );
                        break;
                    }
                }
            }
        });
        Self {
            stop,
            bytes,
            protocol,
            outbound,
            next_host_sequence: AtomicU32::new(1),
            next_session_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    /// Performs one typed host-to-target round trip and returns the current
    /// image capabilities.
    pub(crate) fn request_capabilities(&self, timeout: Duration) -> Result<Capabilities> {
        let _hello = self
            .wait_for_protocol_after(0, timeout, |message| {
                matches!(message.body, Event::Hello(_))
            })
            .ok_or_else(|| {
                self.protocol_failure_or("device did not publish a HIL protocol hello")
            })?;
        let response = self.send_command(0, Command::GetCapabilities, timeout)?;
        match response.body {
            Event::Hello(capabilities) => Ok(capabilities),
            Event::Rejected(reason) => {
                Err(format!("device rejected HIL capability request: {reason:?}").into())
            }
            _ => Err("device returned an invalid HIL capability response".into()),
        }
    }

    /// Establishes the typed link and provisions this boot from host-owned
    /// local configuration. The passphrase is never echoed by the target or
    /// appended to the UART capture.
    pub(crate) fn prepare_protocol(&self, lab: &LabConfig) -> Result<Capabilities> {
        let capabilities = self.request_capabilities(PROTOCOL_READY_TIMEOUT)?;
        let artifact_path = lab.device.startup_artifact.as_deref();
        if artifact_path.is_some() && !capabilities.features.startup_artifact {
            return Err("firmware does not support a host-owned startup artifact".into());
        }
        if !capabilities.features.data_plane_placement {
            return Err("firmware does not support explicit data-plane placement".into());
        }
        let artifact_event_start = self.protocol_event_count();
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path
            && let Some(bytes) = startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.runtime_initialization {
            self.initialize(lab, PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path
        {
            let status = self.wait_for_startup_artifact_status_after(
                artifact_event_start,
                STARTUP_ARTIFACT_TIMEOUT,
            )?;
            let bytes = self
                .wait_for_startup_artifact_after(artifact_event_start, STARTUP_ARTIFACT_TIMEOUT)?;
            if usize::from(status.total_length) != bytes.len() {
                return Err(format!(
                    "startup artifact status length {} does not match {} returned bytes",
                    status.total_length,
                    bytes.len()
                )
                .into());
            }
            startup_artifact::persist_atomically(path, &bytes)?;
            eprintln!(
                "startup_artifact={} disposition={:?} bytes={} initialization_elapsed_us={}",
                path.display(),
                status.disposition,
                bytes.len(),
                status.initialization_elapsed_micros,
            );
        }
        Ok(capabilities)
    }

    fn wait_for_startup_artifact_status_after(
        &self,
        start: usize,
        timeout: Duration,
    ) -> Result<StartupArtifactStatus> {
        let event = self
            .wait_for_protocol_after(start, timeout, |message| {
                matches!(&message.body, Event::StartupArtifactReady(_))
            })
            .ok_or("device did not report startup artifact initialization status")?;
        match event.body {
            Event::StartupArtifactReady(status) => Ok(status),
            _ => unreachable!("startup artifact status predicate accepted only status events"),
        }
    }

    fn upload_startup_artifact(&self, bytes: &[u8], timeout: Duration) -> Result<()> {
        for chunk in startup_artifact::chunks(bytes)? {
            match self
                .send_command(0, Command::UploadStartupArtifact(chunk), timeout)?
                .body
            {
                Event::Accepted => {}
                Event::Rejected(reason) => {
                    return Err(format!("device rejected HIL startup artifact: {reason:?}").into());
                }
                _ => return Err("device returned an invalid startup artifact response".into()),
            }
        }
        Ok(())
    }

    fn wait_for_startup_artifact_after(&self, start: usize, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut cursor = start;
        let mut assembler = startup_artifact::Assembler::new();
        loop {
            let chunk = self
                .wait_for_startup_artifact_chunk(&mut cursor, deadline)
                .ok_or("device did not return its startup artifact")?;
            if let Some(bytes) = assembler.push(&chunk)? {
                return Ok(bytes);
            }
        }
    }

    fn wait_for_startup_artifact_chunk(
        &self,
        cursor: &mut usize,
        deadline: Instant,
    ) -> Option<StartupArtifactChunk> {
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some((relative, chunk)) = messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find_map(|(relative, message)| match &message.body {
                    Event::StartupArtifact(chunk) => Some((relative, chunk.clone())),
                    _ => None,
                })
            {
                *cursor += relative + 1;
                return Some(chunk);
            }
            *cursor = messages.len();
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return None;
            }
        }
    }

    fn initialize(&self, lab: &LabConfig, timeout: Duration) -> Result<()> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(
            0,
            Command::Initialize(
                open_esp_radio_hil_protocol::InitializationConfiguration::new(
                    lab.station.ipv4(),
                    lab.data_plane(),
                ),
            ),
            timeout,
        )?;
        let request_id = response.request_id;
        match response.body {
            Event::Initialized => return Ok(()),
            Event::Accepted
            | Event::State(StateChange {
                current: SessionState::Idle,
                ..
            }) => {}
            Event::Rejected(reason) => {
                return Err(format!("device rejected HIL initialization: {reason:?}").into());
            }
            _ => return Err("device returned an invalid initialization response".into()),
        }
        self.wait_for_protocol_after(first_event, timeout, |message| {
            message.request_id == request_id && matches!(message.body, Event::Initialized)
        })
        .ok_or_else(|| "device did not complete role-neutral initialization".into())
        .map(|_| ())
    }

    pub(crate) fn prepare_station(
        &self,
        lab: &LabConfig,
        timeout: Duration,
    ) -> Result<Capabilities> {
        let capabilities = self.prepare_protocol(lab)?;
        let lifecycle_cursor = self.station_lifecycle_cursor();
        let handle = self.request_station_start(lab)?;
        self.wait_wifi_role_transition(handle, timeout)?;
        self.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
        Ok(capabilities)
    }

    pub(crate) fn query_operation_status(&self, timeout: Duration) -> Result<OperationStatus> {
        match self.send_command(0, Command::GetStatus, timeout)?.body {
            Event::OperationStatus(status) => Ok(status),
            Event::Rejected(reason) => {
                Err(format!("device rejected operation-status query: {reason:?}").into())
            }
            _ => Err("device returned an invalid operation-status response".into()),
        }
    }

    fn send_command(
        &self,
        session_id: u64,
        body: Command,
        timeout: Duration,
    ) -> Result<Envelope<Event>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before command")?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let command = Envelope::new(boot_id, request_id, session_id, request_id, body);
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL command: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| "serial worker stopped before HIL command")?;
        self.wait_for_protocol_after(event_count, timeout, |message| {
            message.boot_id == boot_id && message.request_id == request_id
        })
        .ok_or_else(|| self.protocol_failure_or("device did not answer HIL command"))
    }

    fn expect_accepted(
        &self,
        session_id: u64,
        command: Command,
        operation: &str,
        expected_state: SessionState,
    ) -> Result<()> {
        let response = self
            .send_command(session_id, command, PROTOCOL_READY_TIMEOUT)
            .map_err(|error| format!("session {operation} command failed: {error}"))?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::State(StateChange { current, .. }) if current == expected_state => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected session {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid session {operation} response").into()),
        }
    }

    pub(crate) fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let first_event = self.protocol_event_count();
        let direction = config.direction;
        let link_requirements = config.link_requirements;
        self.expect_accepted(
            session_id,
            Command::Configure(config),
            "configuration",
            SessionState::Configured,
        )?;
        self.expect_accepted(session_id, Command::Arm, "arm", SessionState::Armed)?;
        self.expect_accepted(session_id, Command::Start, "start", SessionState::Running)?;
        let expected_directions: &[Direction] = match direction {
            Direction::Rx => &[Direction::Rx],
            Direction::Tx => &[Direction::Tx],
            Direction::Bidirectional => &[Direction::Rx, Direction::Tx],
        };
        for expected in expected_directions {
            self.wait_for_protocol_after(first_event, PROTOCOL_READY_TIMEOUT, |message| {
                message.session_id == session_id
                    && matches!(
                        message.body,
                        Event::SessionReady(reported)
                            if session_ready_covers(
                                direction,
                                reported,
                                *expected,
                                link_requirements,
                            )
                    )
            })
            .ok_or_else(|| {
                format!(
                    "device did not publish {expected:?} data-plane readiness for session \
                     {session_id}; required TX BlockAck TID: {:?}",
                    link_requirements.tx_block_ack_tid,
                )
            })?;
        }
        Ok(SessionHandle {
            session_id,
            first_event,
        })
    }

    pub(crate) fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        let deadline = Instant::now() + timeout;
        let evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Transport(_)))
            })
            .ok_or("device did not publish structured session evidence")?;
        let transport = match evidence.body {
            Event::Evidence(EvidenceRecord::Transport(transport)) => transport,
            _ => unreachable!("session evidence predicate accepted only transport evidence"),
        };
        let link_evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Link(_)))
            })
            .ok_or("device did not publish structured protocol-link evidence")?;
        let link = match link_evidence.body {
            Event::Evidence(EvidenceRecord::Link(link)) => link,
            _ => unreachable!("link evidence predicate accepted only link evidence"),
        };
        if link.rx_cobs_errors != 0
            || link.rx_checksum_errors != 0
            || link.rx_decode_errors != 0
            || link.rx_overflows != 0
            || link.tx_dropped != 0
        {
            return Err(format!("device protocol link is unhealthy: {link:?}").into());
        }
        let stack_evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Stack(_)))
            })
            .ok_or("device did not publish structured stack evidence")?;
        let stack = match stack_evidence.body {
            Event::Evidence(EvidenceRecord::Stack(stack)) => stack,
            _ => unreachable!("stack evidence predicate accepted only stack evidence"),
        };
        validate_stack_usage(stack)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let finished = self
            .wait_for_protocol_after(session.first_event, remaining, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Finished(_))
            })
            .ok_or("device did not finish the structured HIL session")?;
        let finished = match finished.body {
            Event::Finished(finished) => finished,
            _ => unreachable!("session completion predicate accepted only Finished"),
        };
        let radio = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Radio(_)))
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::Radio(radio)) => radio,
                _ => unreachable!("radio predicate accepted only radio evidence"),
            });
        let tx_timing = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::TxAggregateTiming(_))
                    )
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::TxAggregateTiming(timing)) => timing,
                _ => unreachable!("TX timing predicate accepted only aggregate timing evidence"),
            });
        let rx_delivery = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::RxDelivery(_)))
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::RxDelivery(delivery)) => delivery,
                _ => unreachable!("RX delivery predicate accepted only delivery evidence"),
            });
        let network_scheduler = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::NetworkScheduler(_))
                    )
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::NetworkScheduler(evidence)) => evidence,
                _ => unreachable!("scheduler predicate accepted only scheduler evidence"),
            });
        let expected_records = 3
            + u16::from(radio.is_some())
            + u16::from(tx_timing.is_some())
            + u16::from(rx_delivery.is_some())
            + u16::from(network_scheduler.is_some());
        if finished.summary.evidence_records != expected_records {
            return Err(format!(
                "device reported {} evidence records but published {expected_records} typed records",
                finished.summary.evidence_records
            )
            .into());
        }
        let mut records = Vec::with_capacity(usize::from(finished.summary.evidence_records));
        records.push(EvidenceRecord::Transport(transport));
        if let Some(radio) = radio {
            records.push(EvidenceRecord::Radio(radio));
        }
        if let Some(timing) = tx_timing {
            records.push(EvidenceRecord::TxAggregateTiming(timing));
        }
        if let Some(delivery) = rx_delivery {
            records.push(EvidenceRecord::RxDelivery(delivery));
        }
        if let Some(scheduler) = network_scheduler {
            records.push(EvidenceRecord::NetworkScheduler(scheduler));
        }
        records.push(EvidenceRecord::Link(link));
        records.push(EvidenceRecord::Stack(stack));
        let checksum = evidence_crc32c(&records)
            .map_err(|error| format!("cannot checksum structured HIL evidence: {error}"))?;
        if checksum != finished.evidence_crc32c {
            return Err(format!(
                "structured HIL evidence checksum mismatch: host={checksum:#010x} device={:#010x}",
                finished.evidence_crc32c
            )
            .into());
        }
        Ok(SessionEvidence {
            transport,
            radio,
            tx_timing,
            rx_delivery,
            network_scheduler,
            stack,
            finished,
        })
    }

    pub(crate) fn acknowledge_session(&self, session: SessionHandle) -> Result<()> {
        self.expect_accepted(
            session.session_id,
            Command::AcknowledgeResult,
            "acknowledgement",
            SessionState::Idle,
        )
    }

    pub(crate) fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(0, Command::CycleStationEpoch, PROTOCOL_READY_TIMEOUT)?;
        match response.body {
            Event::Accepted => Ok(StationEpochHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected station epoch cycle: {reason:?}").into())
            }
            _ => Err("device returned an invalid station epoch cycle response".into()),
        }
    }

    pub(crate) fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages
            .get(handle.first_event..)
            .unwrap_or_default()
            .iter()
            .find_map(|message| {
                if message.request_id != handle.request_id {
                    return None;
                }
                match message.body {
                    Event::StationEpochCompleted(evidence) => Some(evidence),
                    _ => None,
                }
            })
    }

    fn request_wifi_command(&self, command: Command, operation: &str) -> Result<WifiCommandHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(0, command, PROTOCOL_READY_TIMEOUT)?;
        match response.body {
            Event::Accepted => Ok(WifiCommandHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid {operation} response").into()),
        }
    }

    pub(crate) fn request_station_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopStation, "station stop")
    }

    pub(crate) fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage> {
        let response = self.send_command(0, Command::QueryStackUsage, timeout)?;
        match response.body {
            Event::StackUsage(usage) => {
                validate_stack_usage(usage)?;
                Ok(usage)
            }
            Event::Rejected(reason) => {
                Err(format!("device rejected stack-usage query: {reason:?}").into())
            }
            _ => Err("device returned an invalid stack-usage response".into()),
        }
    }

    pub(crate) fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        let response = self.send_command(0, Command::QueryLinkHealth, timeout)?;
        match response.body {
            Event::LinkHealth(health) => Ok(health),
            Event::Rejected(reason) => {
                Err(format!("device rejected link-health query: {reason:?}").into())
            }
            _ => Err("device returned an invalid link-health response".into()),
        }
    }

    pub(crate) fn probe_timebase(
        &self,
        request: TimebaseProbeRequest,
        timeout: Duration,
    ) -> Result<TimebaseProbeEvidence> {
        let response = self.send_command(0, Command::ProbeTimebase(request), timeout)?;
        match response.body {
            Event::TimebaseProbeCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected timebase probe: {reason:?}").into())
            }
            _ => Err("device returned an invalid timebase-probe response".into()),
        }
    }

    pub(crate) fn request_station_start(&self, lab: &LabConfig) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            Command::StartStation(lab.station.protocol_credentials()?),
            "station start",
        )
    }

    pub(crate) fn request_wifi_scan(&self, request: WifiScanRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::ScanWifi(request), "standalone Wi-Fi scan")
    }

    pub(crate) fn request_monitor_start(
        &self,
        request: WifiMonitorRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartMonitor(request), "monitor start")
    }

    pub(crate) fn request_monitor_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopMonitor, "monitor stop")
    }

    pub(crate) fn request_access_point_start(
        &self,
        request: open_esp_radio_hil_protocol::WifiAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartAccessPoint(request), "access-point start")
    }

    pub(crate) fn request_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopAccessPoint, "access-point stop")
    }

    pub(crate) fn request_monitor_capture(
        &self,
        request: WifiMonitorCaptureRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::CaptureMonitor(request), "finite monitor capture")
    }

    pub(crate) fn wait_wifi_role_transition(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiRoleTransitioned(_))
            })
            .ok_or("device did not complete the Wi-Fi role transition")?;
        match event.body {
            Event::WifiRoleTransitioned(evidence) => Ok(evidence),
            _ => unreachable!("role-transition predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_wifi_scan(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiScanEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiScanCompleted(_))
            })
            .ok_or("device did not complete the standalone Wi-Fi scan")?;
        match event.body {
            Event::WifiScanCompleted(evidence) => Ok(evidence),
            _ => unreachable!("scan predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_monitor_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStarted(_))
            })
            .ok_or("device did not complete monitor start")?;
        match event.body {
            Event::WifiMonitorStarted(evidence) => Ok(evidence),
            _ => unreachable!("monitor-start predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_access_point_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStarted(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the access-point start")?;
        match event.body {
            Event::WifiAccessPointStarted(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point start failed: {failure:?}").into())
            }
            _ => unreachable!("AP-start predicate accepted only terminal AP events"),
        }
    }

    pub(crate) fn wait_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<open_esp_radio_hil_protocol::WifiAccessPointEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStopped(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the access-point stop")?;
        match event.body {
            Event::WifiAccessPointStopped(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point stop failed: {failure:?}").into())
            }
            _ => unreachable!("AP-stop predicate accepted only terminal AP events"),
        }
    }

    pub(crate) fn wait_monitor_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiMonitorEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStopped(_))
            })
            .ok_or("device did not complete monitor stop")?;
        match event.body {
            Event::WifiMonitorStopped(evidence) => Ok(evidence),
            _ => unreachable!("monitor-stop predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_monitor_capture(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence> {
        let completion = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorCaptureCompleted(_))
            })
            .ok_or("device did not complete finite monitor capture")?;
        let summary = match completion.body {
            Event::WifiMonitorCaptureCompleted(evidence) => evidence,
            _ => unreachable!("capture predicate accepted only terminal capture evidence"),
        };
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let chunks = messages
            .get(handle.first_event..)
            .unwrap_or_default()
            .iter()
            .filter_map(|message| {
                if message.request_id != handle.request_id {
                    return None;
                }
                match &message.body {
                    Event::WifiMonitorFrame(chunk) => Some(chunk.clone()),
                    _ => None,
                }
            })
            .collect();
        Ok(MonitorCaptureEvidence { chunks, summary })
    }

    pub(crate) fn wait_for_connected_station(&self, timeout: Duration) -> Result<u32> {
        self.wait_for_connected_station_after(0, timeout)
    }

    /// Wait for a connected edge published after a caller-owned event cursor.
    ///
    /// Reusing the first connected event of the current boot is incorrect for
    /// role roundtrips: the next AP epoch would then start while the preceding
    /// station restart was still scanning. Callers that initiate a new station
    /// epoch must snapshot [`Self::station_lifecycle_cursor`] before the start
    /// command and use this method.
    pub(crate) fn wait_for_connected_station_after(
        &self,
        first_event: usize,
        timeout: Duration,
    ) -> Result<u32> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let event = self
            .wait_for_protocol_after(first_event, timeout, |message| {
                message.boot_id == boot_id
                    && matches!(
                        message.body,
                        Event::StationLifecycle(StationLifecycleEvent::Connected { .. })
                    )
            })
            .ok_or("device did not publish connected station readiness")?;
        match event.body {
            Event::StationLifecycle(StationLifecycleEvent::Connected { generation }) => {
                Ok(generation)
            }
            _ => unreachable!("connected predicate accepted only a connected lifecycle event"),
        }
    }

    /// Cursor for reliable unsolicited station lifecycle events.
    pub(crate) fn station_lifecycle_cursor(&self) -> usize {
        self.protocol_event_count()
    }

    /// Wait for the next station lifecycle event and advance past it.
    pub(crate) fn wait_station_lifecycle_event(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<StationLifecycleEvent> {
        self.wait_station_lifecycle_event_optional(cursor, timeout)?
            .ok_or_else(|| "device did not publish the next station lifecycle event".into())
    }

    pub(crate) fn wait_station_lifecycle_event_optional(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<Option<StationLifecycleEvent>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(event) = next_station_lifecycle_event(&messages, cursor, boot_id) {
                return Ok(Some(event));
            }
            *cursor = messages.len();
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return Ok(None);
            }
        }
    }

    fn latest_boot_id(&self) -> Option<u64> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        latest_boot_id_in(&messages)
    }

    pub(crate) fn observed_protocol_ipv4(&self) -> Option<Ipv4Addr> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let boot_id = latest_boot_id_in(&messages)?;
        messages
            .iter()
            .rev()
            .find_map(|message| match message.body {
                Event::NetworkReady(network) if message.boot_id == boot_id => {
                    Some(Ipv4Addr::from(network.address))
                }
                _ => None,
            })
    }

    pub(crate) fn beacon_loss_count(&self) -> usize {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        beacon_loss_count_in(&messages)
    }

    pub(crate) fn require_no_beacon_loss(&self) -> Result<()> {
        let count = self.beacon_loss_count();
        if count == 0 {
            Ok(())
        } else {
            Err(format!("observed {count} typed station beacon-loss event(s)").into())
        }
    }

    fn observed_udp_service(&self, direction: Direction, port: u16) -> bool {
        self.observed_service(Transport::Udp, direction, port)
    }

    fn observed_service(&self, transport: Transport, direction: Direction, port: u16) -> bool {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(boot_id) = latest_boot_id_in(&messages) else {
            return false;
        };
        messages.iter().any(|message| match message.body {
            Event::ServiceReady(service) => {
                message.boot_id == boot_id
                    && service.transport == transport
                    && service.direction == direction
                    && service.local_port == port
            }
            _ => false,
        })
    }

    fn protocol_event_count(&self) -> usize {
        self.protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn protocol_failure_or(&self, fallback: &str) -> Box<dyn std::error::Error> {
        self.protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .clone()
            .unwrap_or_else(|| fallback.to_owned())
            .into()
    }

    fn wait_for_protocol_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Option<Envelope<Event>> {
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(message) = messages
                .get(start..)
                .unwrap_or_default()
                .iter()
                .find(|message| predicate(message))
            {
                return Some(message.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return None;
            }
        }
    }

    /// Stops capture and persists both the byte-exact diagnostic transcript
    /// and the decoded target event stream. Host commands are deliberately not
    /// logged because station/AP commands can contain credentials.
    pub(crate) fn finish_to(mut self, output: &Path) -> Result<String> {
        let protocol_active = self
            .protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active;
        let target_health = protocol_active
            .then(|| {
                self.query_link_health(PROTOCOL_READY_TIMEOUT)
                    .map_err(|error| error.to_string())
            })
            .transpose();
        self.stop_and_join();
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let uart = String::from_utf8_lossy(&bytes).into_owned();
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let health = self
            .protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        fs::create_dir_all(output)?;
        fs::write(output.join("uart.log"), &uart)?;
        let mut protocol_log = Vec::new();
        for message in messages.iter() {
            serde_json::to_writer(
                &mut protocol_log,
                &serde_json::json!({"record": "target-event", "envelope": message}),
            )?;
            protocol_log.push(b'\n');
        }
        serde_json::to_writer(
            &mut protocol_log,
            &serde_json::json!({"record": "link-health", "health": &health}),
        )?;
        protocol_log.push(b'\n');
        serde_json::to_writer(
            &mut protocol_log,
            &serde_json::json!({"record": "target-link-health", "health": &target_health}),
        )?;
        protocol_log.push(b'\n');
        fs::write(output.join("protocol.jsonl"), protocol_log)?;
        if !decode_counters_are_clean(health.counters) {
            return Err(format!(
                "host reported unhealthy HIL frame decoding: {:?}",
                health.counters
            )
            .into());
        }
        if let Some(failure) = health.failure {
            return Err(format!("HIL protocol health failed: {failure}").into());
        }
        if protocol_active {
            let target_health = target_health
                .map_err(|error| format!("target link-health query failed: {error}"))?
                .ok_or("target link-health query returned no result")?;
            validate_target_link_health(target_health)?;
            if target_health.text_dropped != 0 || target_health.text_truncated != 0 {
                eprintln!(
                    "diagnostic_text_loss=dropped:{} truncated:{}",
                    target_health.text_dropped, target_health.text_truncated,
                );
            }
        }
        Ok(uart)
    }

    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn latest_boot_id_in(messages: &[Envelope<Event>]) -> Option<u64> {
    messages
        .iter()
        .rev()
        .find_map(|message| matches!(message.body, Event::Hello(_)).then_some(message.boot_id))
}

fn beacon_loss_count_in(messages: &[Envelope<Event>]) -> usize {
    let Some(boot_id) = latest_boot_id_in(messages) else {
        return 0;
    };
    messages
        .iter()
        .filter(|message| {
            message.boot_id == boot_id
                && matches!(
                    message.body,
                    Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                        reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
                        ..
                    })
                )
        })
        .count()
}

fn next_station_lifecycle_event(
    messages: &[Envelope<Event>],
    cursor: &mut usize,
    boot_id: u64,
) -> Option<StationLifecycleEvent> {
    let (relative, event) = messages
        .get(*cursor..)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .find_map(|(relative, message)| {
            if message.boot_id != boot_id {
                return None;
            }
            match &message.body {
                Event::StationLifecycle(event) => Some((relative, *event)),
                _ => None,
            }
        })?;
    *cursor += relative + 1;
    Some(event)
}

fn decode_counters_are_clean(counters: DecodeCounters) -> bool {
    counters.cobs_errors == 0
        && counters.too_short == 0
        && counters.header_errors == 0
        && counters.framing_version_errors == 0
        && counters.message_kind_errors == 0
        && counters.protocol_version_errors == 0
        && counters.payload_length_errors == 0
        && counters.checksum_errors == 0
        && counters.deserialize_errors == 0
        && counters.overflows == 0
}

fn validate_target_link_health(health: LinkHealth) -> Result<()> {
    if health.rx_cobs_errors != 0
        || health.rx_checksum_errors != 0
        || health.rx_decode_errors != 0
        || health.rx_overflows != 0
        || health.tx_dropped != 0
    {
        return Err(
            format!("target reported unhealthy serialized console transport: {health:?}").into(),
        );
    }
    Ok(())
}

fn open_serial_after_busy_release(
    port: &Path,
) -> serialport::Result<Box<dyn serialport::SerialPort>> {
    let deadline = Instant::now() + SERIAL_OPEN_BUSY_TIMEOUT;
    loop {
        match serialport::new(port.to_string_lossy(), 115_200)
            .timeout(Duration::from_millis(20))
            .open()
        {
            Ok(serial) => return Ok(serial),
            Err(error)
                if error.kind == serialport::ErrorKind::NoDevice
                    && port.exists()
                    && Instant::now() < deadline =>
            {
                thread::sleep(SERIAL_OPEN_BUSY_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
}

fn session_ready_covers(
    configured: Direction,
    reported: SessionReady,
    expected: Direction,
    requirements: SessionLinkRequirements,
) -> bool {
    let direction_covers = reported.direction == expected
        || (configured == Direction::Bidirectional
            && reported.direction == Direction::Bidirectional);
    let requirements_met =
        expected != Direction::Tx || reported.tx_block_ack_tid == requirements.tx_block_ack_tid;
    direction_covers && requirements_met
}

/// Wait for a runtime-configured TCP receive service and its current IPv4
/// address. Unlike UDP, readiness does not inject a probe connection: the
/// target begins listening only after the session `Start` transition, and the
/// measured host connection is the sole stream owned by that session.
pub(crate) fn await_tcp_ready(
    capture: &SerialCapture,
    lab: &LabConfig,
    address_hint: Ipv4Addr,
    port: u16,
    direction: Direction,
    timeout: Duration,
) -> Result<TcpReady> {
    let capabilities = capture.prepare_station(lab, timeout)?;
    let direction_supported = match direction {
        Direction::Rx => capabilities.features.rx,
        Direction::Tx => capabilities.features.tx,
        Direction::Bidirectional => capabilities.features.bidirectional,
    };
    if !capabilities.features.tcp || !direction_supported {
        return Err(format!("firmware does not advertise TCP {direction:?} capability").into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err("TCP RX requires runtime sessions and structured evidence".into());
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let address = capture.observed_protocol_ipv4();
        if capture.observed_service(Transport::Tcp, direction, port)
            && let Some(address) = address
        {
            return Ok(TcpReady { address });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device {address_hint}:{port} did not publish TCP {direction:?} readiness within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

/// Provisions the station and returns only the typed `NetworkReady` address.
pub(crate) fn await_network_ready(
    capture: &SerialCapture,
    lab: &LabConfig,
    timeout: Duration,
) -> Result<Ipv4Addr> {
    capture.prepare_station(lab, timeout)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(address) = capture.observed_protocol_ipv4() {
            return Ok(address);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish typed network readiness within {} seconds",
        timeout.as_secs()
    )
    .into())
}

/// Reset an ESP USB-Serial/JTAG target without giving up the capture handle.
///
/// This is the `espflash` `reset_after_flash` USB-Serial/JTAG sequence. DTR is
/// kept high at the board pin, while the RTS transition issues the chip reset.
fn reset_usb_serial_jtag(serial: &mut dyn serialport::SerialPort) -> serialport::Result<()> {
    thread::sleep(Duration::from_millis(100));
    serial.write_data_terminal_ready(false)?;
    thread::sleep(Duration::from_millis(100));
    serial.write_request_to_send(true)?;
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(true)?;
    thread::sleep(Duration::from_millis(100));
    serial.write_request_to_send(false)
}

impl Drop for SerialCapture {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn append(bytes: &Mutex<Vec<u8>>, chunk: &[u8]) {
    bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend_from_slice(chunk);
}

/// Wait until the target owns its IPv4 address and UDP RX service.
///
/// The qualification image requires a typed service-ready edge emitted only
/// after the target consumes a negative warm-up datagram. `Arm`/`Start` then
/// provide measured synchronization.
pub(crate) fn await_udp_rx_ready(
    capture: &SerialCapture,
    lab: &LabConfig,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let capabilities = capture.prepare_station(lab, timeout)?;
    if !capabilities.features.udp || !capabilities.features.rx {
        return Err("firmware does not advertise UDP RX capability".into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err(
            "qualification firmware requires runtime sessions and structured evidence".into(),
        );
    }
    probe_udp_rx_ready(capture, address_hint, port, timeout)
}

/// Prove the already-running UDP RX service from its current network peer.
///
/// Unlike [`await_udp_rx_ready`], this does not prepare or assume the station
/// role. AP qualification calls it only after the controlled client has joined,
/// so the unmeasured datagram also establishes the AP-side neighbor/data path
/// before sequence zero is admitted to a measured session.
pub(crate) fn probe_udp_rx_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    probe_udp_rx_ready_via(capture, address_hint, None, port, timeout)
}

/// Prove UDP RX readiness while an explicit routed peer owns the host path.
///
/// `address_hint` remains the target identity published in typed evidence;
/// `traffic_address` is only the address through which the probe reaches it.
/// This distinction is required when a controlled OpenWrt Wi-Fi station
/// forwards traffic from the wired HIL generator to the DUT AP.
pub(crate) fn probe_udp_rx_ready_via(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    traffic_address: Option<Ipv4Addr>,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let mut address = address_hint;
    let mut connected_address = traffic_address.unwrap_or(address);
    if !connected_address.is_unspecified() {
        socket.connect(SocketAddrV4::new(connected_address, port))?;
    }
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut packet = [0x5a; RX_PROBE_PAYLOAD];
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(discovered) = capture.observed_protocol_ipv4()
            && discovered != address
        {
            address = discovered;
            if traffic_address.is_none() {
                connected_address = address;
                socket.connect(SocketAddrV4::new(connected_address, port))?;
            }
        }
        let rx_service_ready = capture.observed_udp_service(Direction::Rx, port);
        let tx_service_ready = capture.observed_udp_service(Direction::Tx, 4_324);
        if !rx_service_ready || !tx_service_ready || capture.observed_protocol_ipv4().is_none() {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let boot_id = capture
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let event_start = capture.protocol_event_count();
        packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
        socket.send(&packet)?;
        if capture
            .wait_for_protocol_after(event_start, RX_PROBE_RESPONSE_TIMEOUT, |message| {
                message.boot_id == boot_id
                    && matches!(
                        message.body,
                        Event::ServiceReady(service)
                            if service.transport == Transport::Udp
                                && service.direction == Direction::Rx
                                && service.local_port == port
                    )
            })
            .is_some()
        {
            return Ok(UdpRxReady { address });
        }
    }

    Err(format!(
        "device {address}:{port} did not confirm end-to-end UDP RX within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

pub(crate) fn await_udp_tx_ready(
    capture: &SerialCapture,
    lab: &LabConfig,
    address_hint: Ipv4Addr,
    timeout: Duration,
) -> Result<UdpTxReady> {
    let capabilities = capture.prepare_station(lab, timeout)?;
    if !capabilities.features.udp || !capabilities.features.tx {
        return Err("firmware does not advertise UDP TX capability".into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err(
            "qualification firmware requires runtime sessions and structured evidence".into(),
        );
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.observed_udp_service(Direction::Tx, 4_324) {
            let discovery_deadline = Instant::now() + DHCP_DISCOVERY_GRACE;
            while Instant::now() < discovery_deadline {
                if let Some(address) = capture.observed_protocol_ipv4() {
                    return Ok(UdpTxReady { address });
                }
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(UdpTxReady {
                address: address_hint,
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish typed UDP TX readiness within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

#[cfg(test)]
mod tests {
    use open_esp_radio_hil_protocol::{
        Capabilities, DecodeCounters, Direction, Envelope, Event, FeatureCapabilities, Finished,
        RadioEvidence, ResultSummary, RxRadioEvidence, SessionLinkRequirements, SessionReady,
        StackUsage, StackWatermark, StationLifecycleEvent, TransportEvidence,
    };

    use super::{
        ProtocolHealth, SessionEvidence, beacon_loss_count_in, next_station_lifecycle_event,
        session_ready_covers, validate_stack_usage,
    };

    fn hello(boot_id: u64, message_sequence: u32) -> Envelope<Event> {
        Envelope::new(
            boot_id,
            message_sequence,
            0,
            0,
            Event::Hello(Capabilities {
                features: FeatureCapabilities {
                    udp: true,
                    tcp: true,
                    rx: true,
                    tx: true,
                    bidirectional: true,
                    runtime_initialization: true,
                    runtime_configuration: true,
                    structured_evidence: true,
                    startup_artifact: true,
                    station_epoch_control: true,
                    wifi_role_control: true,
                    wifi_access_point: true,
                    wifi_monitor_capture: true,
                    station_lifecycle_events: true,
                    rx_delivery_evidence: true,
                    task_poll_evidence: false,
                    psram_task_stack: false,
                    network_scheduler_evidence: false,
                    data_plane_placement: true,
                    timebase_probe: true,
                },
                maximum_payload_bytes: 1,
                maximum_wire_frame_bytes: 1,
            }),
        )
    }

    fn session_with_rx(rx: RxRadioEvidence) -> SessionEvidence {
        SessionEvidence {
            transport: TransportEvidence {
                rx_bytes: 0,
                tx_bytes: 0,
                rx_units: 0,
                tx_units: 0,
                elapsed_micros: 1,
                transport_errors: 0,
            },
            radio: Some(RadioEvidence {
                rx: Some(rx),
                tx: None,
            }),
            tx_timing: None,
            rx_delivery: None,
            network_scheduler: None,
            stack: StackUsage {
                cpu0: StackWatermark {
                    capacity_bytes: 1,
                    free_bytes: 1,
                    used_bytes: 0,
                    minimum_free_bytes: 1,
                },
                cpu1: StackWatermark {
                    capacity_bytes: 1,
                    free_bytes: 1,
                    used_bytes: 0,
                    minimum_free_bytes: 1,
                },
            },
            finished: Finished {
                summary: ResultSummary {
                    passed: true,
                    evidence_records: 4,
                },
                evidence_crc32c: 0,
            },
        }
    }

    fn healthy_he_rx() -> RxRadioEvidence {
        RxRadioEvidence {
            phy_format: 4,
            sequence_first: Some(0),
            sequence_highest: Some(99),
            not_s_mpdu_datagrams: 100,
            not_s_mpdu_beacons: 1,
            ampdu_datagrams: 100,
            protocol_ampdu_datagrams: 100,
            reorder_tid: 0,
            reorder_window: 64,
            reorder_first_samples: 1,
            reorder_first_tid: 0,
            reorder_first_start: 7,
            reorder_first_sequence: 9,
            reorder_first_distance: 2,
            reorder_maximum_occupied: 8,
            rx_service_calls: 10,
            rx_frontier_histogram_samples: 10,
            mac_irq_entries: 10,
            mac_irq_classified_entries: 10,
            ..RxRadioEvidence::default()
        }
    }

    #[test]
    fn typed_rx_radio_enforces_order_and_provenance_without_text() {
        assert!(
            session_with_rx(healthy_he_rx())
                .require_rx_radio(4, 100)
                .is_ok()
        );

        let mut reordered = healthy_he_rx();
        reordered.sequence_backward = 1;
        assert!(
            session_with_rx(reordered)
                .require_rx_radio_health(4)
                .is_ok(),
            "performance evidence keeps radio-health guarantees without claiming exact delivery",
        );
        assert!(session_with_rx(reordered).require_rx_radio(4, 100).is_err());

        let mut wrong_provenance = healthy_he_rx();
        wrong_provenance.protocol_ampdu_datagrams = 0;
        wrong_provenance.hardware_ampdu_datagrams = 100;
        assert!(
            session_with_rx(wrong_provenance)
                .require_rx_radio_health(4)
                .is_err()
        );
    }

    #[test]
    fn runtime_stack_policy_rejects_low_headroom_on_either_core() {
        let watermark = StackWatermark {
            capacity_bytes: 16_000,
            free_bytes: 4_000,
            used_bytes: 12_000,
            minimum_free_bytes: 4_000,
        };
        assert!(
            validate_stack_usage(StackUsage {
                cpu0: watermark,
                cpu1: watermark,
            })
            .is_ok()
        );
        let insufficient = StackWatermark {
            minimum_free_bytes: 4_001,
            ..watermark
        };
        assert!(
            validate_stack_usage(StackUsage {
                cpu0: watermark,
                cpu1: insufficient,
            })
            .is_err()
        );
    }

    #[test]
    fn bidirectional_readiness_covers_both_owned_data_planes() {
        assert!(session_ready_covers(
            Direction::Bidirectional,
            SessionReady {
                direction: Direction::Bidirectional,
                tx_block_ack_tid: Some(0),
            },
            Direction::Rx,
            SessionLinkRequirements::tx_block_ack(0),
        ));
        assert!(session_ready_covers(
            Direction::Bidirectional,
            SessionReady {
                direction: Direction::Bidirectional,
                tx_block_ack_tid: Some(0),
            },
            Direction::Tx,
            SessionLinkRequirements::tx_block_ack(0),
        ));
        assert!(session_ready_covers(
            Direction::Bidirectional,
            SessionReady {
                direction: Direction::Rx,
                tx_block_ack_tid: None,
            },
            Direction::Rx,
            SessionLinkRequirements::tx_block_ack(0),
        ));
        assert!(!session_ready_covers(
            Direction::Rx,
            SessionReady {
                direction: Direction::Bidirectional,
                tx_block_ack_tid: None,
            },
            Direction::Rx,
            SessionLinkRequirements::NONE,
        ));
        assert!(!session_ready_covers(
            Direction::Tx,
            SessionReady {
                direction: Direction::Tx,
                tx_block_ack_tid: None,
            },
            Direction::Tx,
            SessionLinkRequirements::tx_block_ack(0),
        ));
    }

    #[test]
    fn target_sequence_discontinuity_is_a_fatal_protocol_error() {
        let mut health = ProtocolHealth::default();
        health.observe(&hello(7, 0), DecodeCounters::default());
        health.observe(
            &Envelope::new(7, 2, 0, 0, Event::Accepted),
            DecodeCounters::default(),
        );
        assert!(
            health
                .failure
                .as_deref()
                .is_some_and(|failure| { failure.contains("expected 1, observed 2") })
        );
    }

    #[test]
    fn a_new_boot_must_restart_its_target_sequence() {
        let mut health = ProtocolHealth::default();
        health.observe(&hello(7, 0), DecodeCounters::default());
        health.observe(
            &Envelope::new(8, 3, 0, 0, Event::Accepted),
            DecodeCounters::default(),
        );
        assert!(
            health
                .failure
                .as_deref()
                .is_some_and(|failure| { failure.contains("boot 8") })
        );
    }

    #[test]
    fn a_valid_new_boot_clears_previous_boot_failure() {
        let mut health = ProtocolHealth::default();
        health.observe(&hello(7, 0), DecodeCounters::default());
        health.observe(
            &Envelope::new(7, 2, 0, 0, Event::Accepted),
            DecodeCounters::default(),
        );
        assert!(health.failure.is_some());
        health.observe(&hello(8, 0), DecodeCounters::default());
        assert_eq!(health.boot_id, Some(8));
        assert_eq!(health.failure, None);
    }

    #[test]
    fn lifecycle_cursor_ignores_events_from_the_previous_boot() {
        let messages = [
            Envelope::new(
                7,
                1,
                0,
                0,
                Event::StationLifecycle(StationLifecycleEvent::RetryExhausted {
                    generation: 1,
                    attempts: 3,
                    stage: open_esp_radio_hil_protocol::StationFailureStage::CandidateSelection,
                    reason: open_esp_radio_hil_protocol::StationAttemptFailureReason::NoCandidate,
                }),
            ),
            Envelope::new(
                8,
                1,
                0,
                0,
                Event::StationLifecycle(StationLifecycleEvent::Connected { generation: 0 }),
            ),
        ];
        let mut cursor = 0;
        assert_eq!(
            next_station_lifecycle_event(&messages, &mut cursor, 8),
            Some(StationLifecycleEvent::Connected { generation: 0 })
        );
        assert_eq!(cursor, 2);
    }

    #[test]
    fn beacon_loss_qualification_ignores_previous_boots() {
        let mut messages = vec![
            hello(7, 0),
            Envelope::new(
                7,
                1,
                0,
                0,
                Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                    generation: 0,
                    reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
                }),
            ),
            hello(8, 0),
            Envelope::new(
                8,
                1,
                0,
                0,
                Event::StationLifecycle(StationLifecycleEvent::Connected { generation: 0 }),
            ),
        ];
        assert_eq!(beacon_loss_count_in(&messages), 0);

        messages.push(Envelope::new(
            8,
            2,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                generation: 0,
                reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
            }),
        ));
        assert_eq!(beacon_loss_count_in(&messages), 1);
    }
}
