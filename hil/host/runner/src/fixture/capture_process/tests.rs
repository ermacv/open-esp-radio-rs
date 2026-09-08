use super::*;

#[test]
fn stop_is_explicit_and_ready_does_not_require_packets() {
    let directory = tempfile::tempdir().unwrap();
    let stopped = directory.path().join("stopped");
    let script = format!(
        "printf 'capture-open\\n' >&2; read command; test \"$command\" = stop; touch {}; printf 'capture-closed\\n' >&2",
        quote(stopped.to_str().unwrap())
    );
    let mut command = Command::new("sh");
    command.args(["-c", &script]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(10)).unwrap();
    assert!(!stopped.exists());
    let output = capture.finish().unwrap();
    assert!(output.status.success());
    assert!(stopped.exists());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("capture-closed")
    );
}

#[test]
fn exit_without_a_ready_event_cannot_start_a_session() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'permission denied\\n' >&2; exit 1"]);
    assert!(Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).is_err());
}

#[test]
fn a_single_unterminated_diagnostic_line_is_bounded() {
    let (sender, receiver) = mpsc::channel();
    let error = read_diagnostics(std::io::repeat(b'x'), "capture-open", &sender).unwrap_err();
    assert!(error.to_string().contains("exceeded 1 MiB"));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn premature_exit_preserves_the_capture_failure() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "exec 0<&-; printf 'capture-open\\nwrite failed: no space\\n' >&2; exit 4",
    ]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).unwrap();
    let error = capture.finish().unwrap_err();
    assert!(error.to_string().contains("capture exited before Stop"));
    assert!(error.to_string().contains("write failed: no space"));
}

#[test]
fn controlled_child_is_reaped_after_stop() {
    let program = "sh -c 'trap \"exit 0\" INT TERM; printf \"capture-open\\n\" >&2; read ignored'";
    // A FIFO holds the test child without a timer or busy loop.
    let directory = tempfile::tempdir().unwrap();
    let fifo = directory.path().join("wait");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let program = format!("{program} <> {}", quote(fifo.to_str().unwrap()));
    let mut command = Command::new("sh");
    command.args(["-c", &controlled(&program, ":")]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).unwrap();
    assert!(capture.finish().unwrap().status.success());
}

#[test]
#[ignore = "requires installed dumpcap and local packet-capture permissions"]
fn installed_dumpcap_acknowledges_ready_and_reports_explicit_stop() {
    let capture = dumpcap(
        "lo",
        Some("udp and port 9"),
        128,
        std::path::Path::new("/dev/null"),
        Duration::from_secs(1),
    )
    .unwrap();
    let output = capture.finish().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Packets captured:"));
}
