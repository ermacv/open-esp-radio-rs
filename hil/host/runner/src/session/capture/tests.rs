use super::*;
use crate::session::{error::ErrorKind, tests::hello};
use std::io;

#[test]
fn measurements_survive_link_failure_and_unwinding_capture() {
    let output = Output::new();
    let recorder = crate::evidence::measurements::Recorder::default();
    let (capture, input) = capture(&output, false);
    let capture = capture.record_into(recorder.capture(Path::new("boot-001")).unwrap());
    activate(&capture, &input);
    input
        .send(Ok(frame(Envelope::new(
            7,
            1,
            5,
            1,
            Event::Evidence(EvidenceRecord::Transport(TransportEvidence {
                rx_bytes: 125,
                tx_bytes: 0,
                rx_units: 2,
                tx_units: 0,
                elapsed_micros: 1_000,
                transport_errors: 0,
            })),
        ))))
        .unwrap();
    input.send(Err(io::ErrorKind::BrokenPipe.into())).unwrap();
    failure(&capture, ErrorKind::Transport);
    drop(capture);
    let values = recorder.snapshot();
    assert_eq!(
        values
            .iter()
            .find(|v| v.name.ends_with("transport.rx.bytes"))
            .unwrap()
            .value,
        125
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(output.0.join("measurements.json")).unwrap()).unwrap();
    assert_eq!(stored["finalized"], false);
    assert!(!stored["failure"].is_null());
    assert_eq!(
        stored["measurements"].as_array().unwrap().len(),
        values.len()
    );
}

#[test]
fn observation_discovers_a_running_boot_without_initializing_or_clearing_results() {
    let output = Output::new();
    let (input, rx) = mpsc::channel();
    let (writes, commands) = mpsc::channel();
    let capture =
        SerialCapture::start_transport_at(&output.0, CaptureOrigin::Attachment, move || {
            Ok(Serial {
                input: rx,
                fail_write: false,
                writes: Some(writes),
            })
        })
        .unwrap();
    let input_guard = input.clone();
    let target = thread::spawn(move || {
        for (offset, expected) in [
            Command::GetCapabilities,
            Command::GetStatus,
            Command::QueryStackUsage,
            Command::QueryLinkHealth,
            Command::QueryLinkHealth,
        ]
        .into_iter()
        .enumerate()
        {
            let request = receive_command(&commands);
            assert_eq!(request.body, expected);
            assert_eq!(request.boot_id, if offset == 0 { 0 } else { 7 });
            assert_eq!(request.session_id, 0);
            request.validate_target(7).unwrap();
            let body = match expected {
                Command::GetCapabilities => hello(7, 87).body,
                Command::GetStatus => Event::OperationStatus(OperationStatus {
                    state: SessionState::Finished,
                    configured_session_id: Some(9),
                    completed_session_id: Some(9),
                }),
                Command::QueryStackUsage => {
                    Event::Rejected(open_esp_radio_hil_protocol::RejectReason::InvalidState)
                }
                Command::QueryLinkHealth => Event::LinkHealth(LinkHealth {
                    rx_frames: 12,
                    rx_cobs_errors: 0,
                    rx_checksum_errors: 2,
                    rx_decode_errors: 0,
                    rx_overflows: 0,
                    tx_frames: 87 + offset as u32,
                    tx_dropped: 0,
                    text_dropped: 0,
                    text_truncated: 0,
                }),
                _ => unreachable!(),
            };
            input
                .send(Ok(frame(Envelope::new(
                    7,
                    87 + offset as u32,
                    0,
                    request.request_id,
                    body,
                ))))
                .unwrap();
        }
    });
    let report = capture.observe(Duration::from_secs(2));
    let report = capture.finish_observation_with(report).unwrap();
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(report["boot_id"], 7);
    assert_eq!(report["operation"]["completed_session_id"], 9);
    assert!(report["stack"].is_null());
    assert_eq!(report["link"]["rx_checksum_errors"], 2);
    target.join().unwrap();
    drop(input_guard);
    assert!(
        fs::read_to_string(output.0.join("protocol.jsonl"))
            .unwrap()
            .contains("target-link-health")
    );
}

#[cfg(unix)]
#[test]
fn signal_cancellation_harness() {
    let Ok(signal) = std::env::var("OER_HIL_CAPTURE_TEST_SIGNAL") else {
        return;
    };
    let signal: i32 = signal.parse().unwrap();
    let _signals = oer_process::install_signal_handlers().unwrap();
    let output = Output::new();
    let cleanup = crate::fixture::cleanup::Scope::new(&output.0);
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        // SAFETY: this isolated test process installed the handler above.
        assert_eq!(unsafe { libc::kill(libc::getpid(), signal) }, 0);
    });
    let started = Instant::now();
    let error = capture
        .wait_for_protocol_after(0, Duration::from_secs(60), |_| false)
        .unwrap_err();
    sender.join().unwrap();
    assert!(oer_process::is_cancelled(&*error));
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(capture);
    crate::fixture::cleanup::record("restore after cancellation", || {
        oer_process::check_cancelled()?;
        fs::write(output.0.join("restored"), b"yes")?;
        Ok(())
    });
    let records = cleanup.finish().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].failure.is_none());
    assert!(oer_process::check_cancelled().is_err());
    let lab = crate::lab::config::LabConfig::for_test();
    let context = crate::execution::context::Context::new(&lab, Default::default(), &output.0);
    let next = output.0.join("must-not-reset");
    let error = context
        .capture(&next)
        .err()
        .expect("cancel before opening another boot");
    assert!(oer_process::is_cancelled(&*error));
    assert!(!next.exists());
    assert_eq!(
        fs::read(output.0.join("uart.bin")).unwrap(),
        frame(hello(7, 0))
    );
    let protocol = fs::read_to_string(output.0.join("protocol.jsonl")).unwrap();
    assert!(protocol.lines().any(|line| {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        record["cancelled"] == true
    }));
}

#[cfg(unix)]
#[test]
fn signals_cancel_protocol_wait_and_preserve_partial_capture() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let status = oer_process::owned::Child::spawn(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "session::capture::tests::signal_cancellation_harness",
                ])
                .env("OER_HIL_CAPTURE_TEST_SIGNAL", signal.to_string()),
        )
        .unwrap()
        .wait_timeout(Some(Duration::from_secs(10)))
        .unwrap();
        assert!(status.success());
    }
}

struct Output(PathBuf);

impl Output {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oer-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Serial {
    input: mpsc::Receiver<io::Result<Vec<u8>>>,
    fail_write: bool,
    writes: Option<mpsc::Sender<Vec<u8>>>,
}

impl Read for Serial {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.input.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => {
                let bytes = result?;
                buffer[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(io::ErrorKind::TimedOut.into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(0),
        }
    }
}

impl Write for Serial {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            if let Some(writes) = &self.writes {
                writes
                    .send(bytes.to_vec())
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
            }
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture(
    output: &Output,
    fail_write: bool,
) -> (SerialCapture, mpsc::Sender<io::Result<Vec<u8>>>) {
    let (input, rx) = mpsc::channel();
    let capture = SerialCapture::start_transport(&output.0, move || {
        Ok(Serial {
            input: rx,
            fail_write,
            writes: None,
        })
    })
    .unwrap();
    (capture, input)
}

fn frame(event: Envelope<Event>) -> Vec<u8> {
    FrameEncoder::new().encode(&event).unwrap().to_vec()
}

fn activate(capture: &SerialCapture, input: &mpsc::Sender<io::Result<Vec<u8>>>) {
    input.send(Ok(frame(hello(7, 0)))).unwrap();
    capture
        .wait_for_protocol_after(0, Duration::from_secs(2), |message| {
            matches!(message.body, Event::Hello(_))
        })
        .unwrap()
        .unwrap();
}

fn failure(capture: &SerialCapture, kind: ErrorKind) -> String {
    let start = Instant::now();
    let error = capture
        .wait_for_protocol_after(0, Duration::from_secs(3), |_| false)
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "failure waited for the event deadline"
    );
    assert_eq!(error.downcast_ref::<LinkError>().unwrap().kind, kind);
    error.to_string()
}

#[test]
fn open_failure_wakes_hello_wait_and_survives_finish() {
    let output = Output::new();
    let capture = SerialCapture::start_transport::<io::Cursor<Vec<u8>>>(&output.0, || {
        Err(LinkError::transport("cannot open test device"))
    })
    .unwrap();
    assert_eq!(
        failure(&capture, ErrorKind::Transport),
        "cannot open test device"
    );
    assert_eq!(
        capture.finish().unwrap_err().to_string(),
        "cannot open test device"
    );
    assert!(fs::read(output.0.join("uart.bin")).unwrap().is_empty());
    assert!(
        fs::read_to_string(output.0.join("protocol.jsonl"))
            .unwrap()
            .contains("cannot open test device")
    );
}

#[test]
fn read_failure_preserves_exact_partial_bytes_on_early_return() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    let raw = b"boot\xff\n\x00\x00".to_vec();
    input.send(Ok(raw.clone())).unwrap();
    input
        .send(Err(io::ErrorKind::ConnectionReset.into()))
        .unwrap();
    assert!(failure(&capture, ErrorKind::Transport).contains("serial read failed"));
    // The raw evidence already exists before finish or Drop.
    assert_eq!(fs::read(output.0.join("uart.bin")).unwrap(), raw);
    drop(capture);
    assert_eq!(fs::read(output.0.join("uart.bin")).unwrap(), raw);
    let log = fs::read_to_string(output.0.join("uart.log")).unwrap();
    assert_eq!(log, String::from_utf8_lossy(&raw));
    assert!(!log.contains("serial read failed"));
}

#[test]
fn end_of_stream_is_a_transport_failure() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    drop(input);
    assert!(failure(&capture, ErrorKind::Transport).contains("end of stream"));
}

#[test]
fn worker_panic_wakes_waiters() {
    let output = Output::new();
    let capture = SerialCapture::start_transport::<io::Cursor<Vec<u8>>>(&output.0, || {
        panic!("injected worker panic")
    })
    .unwrap();
    assert_eq!(
        failure(&capture, ErrorKind::Transport),
        "serial worker panicked"
    );
}

#[test]
fn write_failure_reaches_the_command_caller() {
    let output = Output::new();
    let (capture, input) = capture(&output, true);
    activate(&capture, &input);
    let start = Instant::now();
    let error = capture
        .request_capabilities(Duration::from_secs(3))
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(
        error.downcast_ref::<LinkError>().unwrap().kind,
        ErrorKind::Transport
    );
    assert!(error.to_string().contains("serial write failed"));
}

#[test]
fn malformed_active_frame_wakes_waiters_and_preserves_the_frame() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input.send(Ok(vec![0, 0, 255, 1, 0])).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("decode failure"));
    drop(capture);
    assert!(
        fs::read(output.0.join("uart.bin"))
            .unwrap()
            .ends_with(&[0, 0, 255, 1, 0])
    );
}

#[test]
fn reboot_cannot_clear_a_failure_or_satisfy_an_old_operation() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let mut batch = frame(Envelope::new(7, 3, 0, 0, Event::Accepted));
    batch.extend(frame(hello(8, 0)));
    input.send(Ok(batch)).unwrap();
    let cause = failure(&capture, ErrorKind::Protocol);
    assert!(cause.contains("sequence discontinuity"));
    assert_eq!(capture.finish().unwrap_err().to_string(), cause);
}

#[test]
fn unexpected_reboot_fails_the_capture() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input.send(Ok(frame(hello(8, 0)))).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("target rebooted"));
}

#[test]
fn optional_wait_reports_absence_only_while_the_link_is_healthy() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    assert!(
        capture
            .wait_for_protocol_after(0, Duration::ZERO, |_| true)
            .unwrap()
            .is_none()
    );
    activate(&capture, &input);
    assert!(
        capture
            .wait_for_protocol_after(0, Duration::ZERO, |event| matches!(
                event.body,
                Event::Hello(_)
            ))
            .unwrap()
            .is_some()
    );
    assert!(
        capture
            .wait_for_protocol_after(1, Duration::from_millis(5), |_| true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn lifecycle_cursor_advances_and_does_not_repeat_events() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let mut cursor = capture.station_lifecycle_cursor();
    input
        .send(Ok(frame(Envelope::new(
            7,
            1,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Connected { generation: 4 }),
        ))))
        .unwrap();
    assert_eq!(
        capture
            .wait_station_lifecycle_event(&mut cursor, Duration::from_secs(2))
            .unwrap(),
        StationLifecycleEvent::Connected { generation: 4 }
    );
    assert_eq!(
        capture
            .wait_station_lifecycle_event_optional(&mut cursor, Duration::ZERO)
            .unwrap(),
        None
    );
}

#[test]
fn incompatible_version_is_reported_before_hello_timeout() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    let mut message = hello(7, 0);
    message.protocol_version += 1;
    input.send(Ok(frame(message))).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("unsupported HIL protocol version"));
}

#[test]
fn receive_overflow_without_a_completed_frame_wakes_waiters() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let mut bytes = vec![0, 0];
    bytes.extend(vec![1; 1800]);
    input.send(Ok(bytes)).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("overflows: 1"));
}

#[test]
fn target_session_failure_does_not_turn_into_an_evidence_timeout() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input
        .send(Ok(frame(Envelope::new(
            7,
            1,
            9,
            0,
            Event::Failed(open_esp_radio_hil_protocol::FailureCode::Network),
        ))))
        .unwrap();
    let session = SessionHandle {
        session_id: 9,
        first_event: 1,
        flow_ids: [Some(0), None],
    };
    let error = capture
        .wait_for_session(session, Duration::from_secs(3))
        .unwrap_err();
    assert_eq!(error.to_string(), "target session 9 failed: Network");
    assert_eq!(
        crate::execution::classify(&*error).kind,
        crate::evidence::run::FailureKind::Scenario
    );
}

#[test]
fn monitor_failure_is_correlated_and_terminal() {
    use open_esp_radio_hil_protocol::{
        WifiRole, WifiRoleFailureEvidence, WifiRoleFailureReason, WifiRoleOperation,
    };
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input
        .send(Ok(frame(Envelope::new(
            7,
            1,
            0,
            22,
            Event::WifiRoleFailed(WifiRoleFailureEvidence {
                role: WifiRole::Monitor,
                operation: WifiRoleOperation::Start,
                reason: WifiRoleFailureReason::HardwareFault,
            }),
        ))))
        .unwrap();
    let handle = WifiCommandHandle {
        request_id: 22,
        first_event: 1,
    };
    let error = capture
        .wait_monitor_start(handle, Duration::from_secs(3))
        .unwrap_err();
    assert!(error.to_string().contains("HardwareFault"));
}

#[test]
fn finalization_failure_keeps_the_primary_cause_and_both_messages() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    drop(input);
    failure(&capture, ErrorKind::Transport);
    let error = capture
        .finish_with::<()>(Err("scenario criterion failed".into()))
        .unwrap_err();
    assert!(error.to_string().starts_with("scenario criterion failed;"));
    assert!(error.to_string().contains("end of stream"));
    assert_eq!(
        crate::execution::classify(&*error).kind,
        crate::evidence::run::FailureKind::Scenario
    );
    let records = fs::read_to_string(output.0.join("protocol.jsonl")).unwrap();
    assert!(records.contains("end of stream"));
}

fn result_events(rx_frames: u32) -> Vec<Event> {
    use open_esp_radio_hil_protocol::{ResultSummary, StackWatermark};
    let transport = TransportEvidence {
        rx_bytes: 8,
        tx_bytes: 0,
        rx_units: 2,
        tx_units: 0,
        elapsed_micros: 100,
        transport_errors: 0,
    };
    let watermark = StackWatermark {
        capacity_bytes: 100,
        free_bytes: 90,
        used_bytes: 10,
        minimum_free_bytes: 20,
    };
    let records = [
        EvidenceRecord::Transport(transport),
        EvidenceRecord::FlowTransport(FlowTransportEvidence::from_session_total(0, transport)),
        EvidenceRecord::Link(LinkHealth {
            rx_frames,
            rx_cobs_errors: 0,
            rx_checksum_errors: 0,
            rx_decode_errors: 0,
            rx_overflows: 0,
            tx_frames: 5,
            tx_dropped: 0,
            text_dropped: 0,
            text_truncated: 0,
        }),
        EvidenceRecord::Stack(StackUsage {
            cpu0: watermark,
            cpu1: watermark,
        }),
    ];
    let finished = Finished {
        summary: ResultSummary {
            passed: true,
            evidence_records: 4,
        },
        evidence_crc32c: evidence_crc32c(&records).unwrap(),
    };
    records
        .into_iter()
        .map(Event::Evidence)
        .chain([Event::Finished(finished)])
        .collect()
}

fn publish_result(
    input: &mpsc::Sender<io::Result<Vec<u8>>>,
    sequence: u32,
    request: u32,
    rx_frames: u32,
) {
    for (offset, event) in result_events(rx_frames).into_iter().enumerate() {
        input
            .send(Ok(frame(Envelope::new(
                7,
                sequence + offset as u32,
                9,
                request,
                event,
            ))))
            .unwrap();
    }
}

fn receive_command(writes: &mpsc::Receiver<Vec<u8>>) -> Envelope<Command> {
    let bytes = writes.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut command = None;
    FrameDecoder::new().feed::<Command>(&bytes, |decoded| command = Some(decoded.unwrap()));
    command.unwrap()
}

fn replay_before_acknowledgement(changed: bool) {
    let output = Output::new();
    let (input, rx) = mpsc::channel();
    let (writes, commands) = mpsc::channel();
    let capture = SerialCapture::start_transport(&output.0, move || {
        Ok(Serial {
            input: rx,
            fail_write: false,
            writes: Some(writes),
        })
    })
    .unwrap();
    activate(&capture, &input);
    publish_result(&input, 1, 0, 5);
    let session = SessionHandle {
        session_id: 9,
        first_event: 1,
        flow_ids: [Some(0), None],
    };
    capture
        .wait_for_session(session, Duration::from_secs(2))
        .unwrap();
    let input_guard = input.clone();
    let target = thread::spawn(move || {
        let replay = receive_command(&commands);
        assert!(matches!(replay.body, Command::ReplayResult));
        assert_eq!(replay.session_id, 9);
        // Model live link counters changing between the first result and its
        // replay: the historical target bug changed both evidence and CRC.
        publish_result(&input, 6, replay.request_id, if changed { 6 } else { 5 });
        if !changed {
            let ack = receive_command(&commands);
            assert!(matches!(ack.body, Command::AcknowledgeResult));
            input
                .send(Ok(frame(Envelope::new(
                    7,
                    11,
                    9,
                    ack.request_id,
                    Event::State(StateChange {
                        previous: SessionState::Finished,
                        current: SessionState::Idle,
                    }),
                ))))
                .unwrap();
        }
    });
    let result = capture.acknowledge_session(session);
    if changed {
        let error = result.unwrap_err();
        assert!(error.to_string().contains("changed the retained result"));
        assert_eq!(
            crate::execution::classify(&*error).kind,
            crate::evidence::run::FailureKind::Infrastructure
        );
    } else {
        result.unwrap();
    }
    target.join().unwrap();
    drop(capture);
    drop(input_guard);
}

#[test]
fn acknowledgement_requires_an_identical_replay() {
    replay_before_acknowledgement(false);
}

#[test]
fn changed_replay_is_rejected_before_result_removal() {
    replay_before_acknowledgement(true);
}
