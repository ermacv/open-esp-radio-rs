use super::*;

#[test]
fn readiness_handles_enabled_and_events_on_both_sides_of_status_response() {
    for ordering in ["already", "before", "after", "disabled", "disabled-before"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hostapd");
        let server = UnixDatagram::bind(&path).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let worker = std::thread::spawn(move || {
            let mut bytes = [0; 128];
            let (n, client) = server.recv_from(&mut bytes).unwrap();
            let client = client.as_pathname().unwrap();
            assert_eq!(&bytes[..n], b"ATTACH");
            server.send_to(b"OK\n", client).unwrap();
            assert_eq!(
                server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
                b"STATUS"
            );
            if ordering == "already" {
                server.send_to(b"state=ENABLED\n", client).unwrap();
                return;
            }
            if ordering == "disabled-before" {
                server.send_to(b"<3>AP-DISABLED", client).unwrap();
                return;
            }
            if ordering == "before" {
                server.send_to(b"<3>AP-ENABLED", client).unwrap();
            }
            server.send_to(b"state=HT_SCAN\n", client).unwrap();
            if ordering == "disabled" {
                server.send_to(b"<3>AP-DISABLED", client).unwrap();
                return;
            }
            if ordering == "after" {
                server.send_to(b"<3>AP-ENABLED", client).unwrap();
            }
            assert_eq!(
                server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
                b"STATUS"
            );
            server.send_to(b"state=ENABLED\n", client).unwrap();
        });
        let mut control = Control::connect(&path).unwrap();
        let result = control.wait_enabled();
        assert_eq!(
            result.is_ok(),
            !ordering.starts_with("disabled"),
            "{ordering}: {result:?}"
        );
        worker.join().unwrap();
    }
}

#[test]
fn absent_readiness_expires_without_a_polling_loop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hostapd");
    let _server = UnixDatagram::bind(&path).unwrap();
    let mut control = Control::connect(&path).unwrap();
    control.deadline = Instant::now();
    assert!(
        control
            .wait_enabled()
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );
}
