use super::*;

/// Replays scripted peer output; every sent line is recorded.
struct ScriptedLink {
    output: VecDeque<String>,
    sent: Vec<String>,
}

impl ScriptedLink {
    fn new(output: &[&str]) -> Self {
        Self {
            output: output.iter().map(|line| format!("{line}\n")).collect(),
            sent: Vec::new(),
        }
    }
}

impl PeerLink for ScriptedLink {
    fn send(&mut self, line: &str) -> Result<()> {
        self.sent.push(line.to_owned());
        Ok(())
    }

    fn receive(&mut self, _deadline: Instant) -> Result<Option<String>> {
        Ok(self.output.pop_front())
    }
}

#[test]
fn protocol_lines_parse_and_other_output_is_ignored() {
    assert_eq!(parse_line("ESP-ROM:esp32c5\n"), None);
    assert_eq!(
        parse_line("@READY protocol=1 target=esp32c5\r\n"),
        Some(Line::Ready { protocol: 1 })
    );
    assert_eq!(parse_line("@OK TX"), Some(Line::Ok("TX".into())));
    assert_eq!(
        parse_line("@ERR CFG ESP_ERR_INVALID_ARG"),
        Some(Line::Err {
            command: "CFG".into(),
            reason: "ESP_ERR_INVALID_ARG".into()
        })
    );
    assert_eq!(
        parse_line("@RX 41880045 rssi=-47 lqi=255 pending=1 ch=15"),
        Some(Line::Event(PeerEvent::Received(PeerFrame {
            bytes: vec![0x41, 0x88, 0x00, 0x45],
            rssi_dbm: -47,
            lqi: 255,
            pending: true,
            channel: 15,
        })))
    );
    assert_eq!(
        parse_line("@TXDONE ack=-"),
        Some(Line::Event(PeerEvent::Transmitted {
            acknowledgement: None
        }))
    );
    assert_eq!(
        parse_line("@TXDONE ack=120007 pending=0 rssi=-40 lqi=200"),
        Some(Line::Event(PeerEvent::Transmitted {
            acknowledgement: Some(PeerAck {
                bytes: vec![0x12, 0x00, 0x07],
                pending: false,
                rssi_dbm: -40,
                lqi: 200,
            })
        }))
    );
    assert_eq!(
        parse_line("@TXFAIL 3"),
        Some(Line::Event(PeerEvent::TransmitFailed(3)))
    );
    assert_eq!(
        parse_line("@ED -88"),
        Some(Line::Event(PeerEvent::EnergyDetected(-88)))
    );
    for malformed in [
        "@RX 4 rssi=0 lqi=0 pending=0 ch=11",
        "@RX 41 rssi=x lqi=0 pending=0 ch=11",
        "@TXDONE",
        "@UNKNOWN",
    ] {
        assert_eq!(parse_line(malformed), None, "{malformed}");
    }
}

#[test]
fn the_configuration_renders_the_documented_command() {
    let config = PeerConfig {
        channel: 15,
        pan_id: 0x4f45,
        short_address: 0x0002,
        extended_address: [1, 2, 3, 4, 5, 6, 7, 8],
        promiscuous: false,
        power_dbm: -3,
    };
    assert_eq!(cfg_line(&config), "CFG 15 4f45 0002 0102030405060708 0 -3");
}

#[test]
fn commands_wait_for_their_answer_and_keep_interleaved_events() {
    let link = ScriptedLink::new(&[
        "boot noise",
        "@READY protocol=1 target=esp32c5",
        "@RX 4188 rssi=-50 lqi=100 pending=0 ch=15",
        "@OK TX",
        "@TXDONE ack=-",
    ]);
    let mut peer = Peer::start(link).unwrap();
    peer.transmit(false, &[0x41, 0x88]).unwrap();
    assert_eq!(peer.link.sent, ["TX 0 4188"]);
    assert!(matches!(
        peer.next_event(Duration::ZERO).unwrap(),
        Some(PeerEvent::Received(_))
    ));
    assert_eq!(
        peer.next_event(Duration::ZERO).unwrap(),
        Some(PeerEvent::Transmitted {
            acknowledgement: None
        })
    );
    assert_eq!(peer.next_event(Duration::ZERO).unwrap(), None);
}

#[test]
fn a_rejected_or_silent_command_fails() {
    let link = ScriptedLink::new(&["@READY protocol=1", "@ERR RX ESP_FAIL"]);
    let mut peer = Peer::start(link).unwrap();
    assert!(peer.receive().is_err());
    assert!(peer.sleep().is_err());
}

#[test]
fn a_peer_of_another_protocol_is_refused() {
    assert!(Peer::start(ScriptedLink::new(&["@READY protocol=2"])).is_err());
    assert!(Peer::start(ScriptedLink::new(&["no ready line"])).is_err());
}
