use super::*;

fn observation(target: Result<MultiClientTarget>) -> MultiClientObservation {
    MultiClientObservation {
        direction: Direction::Tx,
        duration: Duration::from_secs(12),
        peers: [
            Ipv4Endpoint {
                address: [10, 43, 0, 2],
                port: 9002,
            },
            Ipv4Endpoint {
                address: [10, 43, 0, 3],
                port: 9003,
            },
        ],
        host_tx: [None, None],
        host_rx: [
            Some(vec![Burst {
                bytes: 1472,
                datagrams: 1,
                started_at_zero: true,
                ..Burst::default()
            }]),
            Some(vec![]),
        ],
        target,
    }
}

#[test]
fn missing_terminal_evidence_preserves_each_hosts_delivery() {
    let output = tempfile::tempdir().unwrap();
    let error = observation(Err("terminal evidence timed out".into()))
        .evaluate(output.path(), &Criteria::default())
        .err()
        .expect("qualification must fail");
    assert!(error.to_string().contains("terminal evidence timed out"));
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(output.path().join("delivery-progress.json")).unwrap())
            .unwrap();
    assert_eq!(saved["host_rx"][0][0]["bytes"], 1472);
    assert_eq!(saved["host_rx"][1], serde_json::json!([]));
    assert!(saved["target_flows"].is_null());
    assert_eq!(saved["duration_micros"], 12_000_000);
}

#[test]
fn failed_per_peer_rate_gate_preserves_complete_raw_evidence() {
    let output = tempfile::tempdir().unwrap();
    let flows = std::array::from_fn(|index| {
        Some(FlowTransportEvidence {
            flow_id: index as u8,
            rx_bytes: 0,
            rx_units: 0,
            tx_bytes: 1472,
            tx_units: 1,
            elapsed_micros: 12_000_000,
            transport_errors: 0,
        })
    });
    let mut observed = observation(Ok(MultiClientTarget {
        elapsed_micros: 12_000_000,
        flows,
    }));
    observed.host_rx[1] = observed.host_rx[0].clone();
    let criteria = Criteria {
        exact_delivery: false,
        minimum_bps_per_flow: Some(10_000),
        ..Criteria::default()
    };
    let error = observed
        .evaluate(output.path(), &criteria)
        .err()
        .expect("qualification must fail");
    assert!(error.to_string().contains("below required 10000"));
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(output.path().join("delivery-progress.json")).unwrap())
            .unwrap();
    assert_eq!(saved["target_flows"][1]["tx_bytes"], 1472);
    assert_eq!(saved["host_rx"][1][0]["bytes"], 1472);
    assert!(saved["target_error"].is_null());
}
