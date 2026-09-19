//! Independent Linux ATT peer for the shared plaintext Trouble application.
use crate::{
    Result,
    execution::context::Context,
    fixture::bluetooth::{att, model::PeerAddress},
    session::SerialCapture,
};
use open_esp_radio_hil_protocol::BluetoothGattEvidence as Evidence;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(serde::Serialize)]
struct Exchange {
    connection: u32,
    request: Vec<u8>,
    response: Vec<u8>,
}

pub(crate) fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut samples = Vec::new();
        let mut exchanges = Vec::new();
        let probe = exercise(capture, &owner, &mut samples, &mut exchanges);
        let cleanup = oer_process::cleanup(|| -> Result<()> {
            owner.restore()?;
            wait(capture, &mut samples, |e| !e.connected)?;
            Ok(())
        });
        crate::evidence::run::atomic_json(&output.join("trouble-gatt.json"), &serde_json::json!({
            "schema": 1, "security": "plaintext", "connections": 3,
            "samples": samples, "exchanges": exchanges,
            "passed": probe.is_ok() && cleanup.is_ok(),
            "error": probe.as_ref().err().map(ToString::to_string),
            "restored": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
        }))?;
        join(probe, cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    join(result, restore)
}

fn join(first: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (first, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}").into()),
    }
}

fn wait(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    predicate: impl Fn(&Evidence) -> bool,
) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let evidence = capture.bluetooth_gatt()?;
        samples.push(evidence);
        if !evidence
            .cpu0_stack
            .is_some_and(|stack| stack.has_required_headroom())
        {
            return Err("missing or insufficient GATT task-stack headroom".into());
        }
        if predicate(&evidence) {
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            return Err(format!("GATT transition deadline: {evidence:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

fn exchange(
    peer: &att::Att,
    connection: u32,
    request: &[u8],
    exchanges: &mut Vec<Exchange>,
) -> Result<Vec<u8>> {
    peer.send(request)?;
    let response = peer.receive()?;
    exchanges.push(Exchange {
        connection,
        request: request.to_vec(),
        response: response.clone(),
    });
    Ok(response)
}

fn exercise(
    capture: &SerialCapture,
    owner: &att::Owner,
    samples: &mut Vec<Evidence>,
    exchanges: &mut Vec<Exchange>,
) -> Result<()> {
    if !capture
        .request_capabilities(Duration::from_secs(10))?
        .features
        .bluetooth_gatt
    {
        return Err("exclusive Trouble GATT image required".into());
    }
    let initial = wait(capture, samples, |e| e.address.is_some() && e.advertising)?;
    if initial.connections != 0
        || initial.disconnections != 0
        || initial.writes != 0
        || initial.value != 0
    {
        return Err("GATT requires a fresh application epoch".into());
    }
    let address = initial.address.ok_or("missing Controller address")?;
    let mut previous = 0;
    for (index, written) in [0x31, 0x52, 0x73].into_iter().enumerate() {
        let connection = index as u32 + 1;
        let peer = owner.connect(PeerAddress(address))?;
        let live = wait(capture, samples, |e| {
            e.connected && e.connections == connection
        })?;
        if live.address != Some(address) || live.disconnections != connection - 1 {
            return Err("GATT connection changed its application epoch".into());
        }
        let mtu = exchange(&peer, connection, &[2, 23, 0], exchanges)?;
        if mtu.len() != 3 || mtu[0] != 3 || !(23..=517).contains(&word(&mtu[1..])) {
            return Err(format!("invalid ATT MTU response: {mtu:?}").into());
        }
        // Discover the application's UUIDs over ATT; do not import a target handle.
        let service = service(&exchange(
            &peer,
            connection,
            &[6, 1, 0, 255, 255, 0, 0x28, 0xf0, 0xff],
            exchanges,
        )?)?;
        let [start_low, start_high] = service.0.to_le_bytes();
        let [end_low, end_high] = service.1.to_le_bytes();
        let handle = characteristic(
            &exchange(
                &peer,
                connection,
                &[8, start_low, start_high, end_low, end_high, 3, 0x28],
                exchanges,
            )?,
            service,
        )?;
        let [low, high] = handle.to_le_bytes();
        if exchange(&peer, connection, &[0x0a, low, high], exchanges)? != [0x0b, previous] {
            return Err("GATT value did not survive reconnect".into());
        }
        if exchange(&peer, connection, &[0x12, low, high, written], exchanges)? != [0x13] {
            return Err("ATT write was not acknowledged".into());
        }
        if exchange(&peer, connection, &[0x0a, low, high], exchanges)? != [0x0b, written] {
            return Err("ATT read did not return the written value".into());
        }
        let progressed = wait(capture, samples, |e| {
            e.writes >= connection && e.reads >= 2 * connection
        })?;
        validate_progress(&progressed, connection, written, address)?;
        drop(peer);
        let idle = wait(capture, samples, |e| {
            !e.connected && e.advertising && e.disconnections == connection
        })?;
        if idle.last_disconnect_reason != Some(0x13)
            || idle.connections != connection
            || idle.advertising_starts != connection + 1
            || idle.address != Some(address)
        {
            return Err(format!("expected graceful same-epoch re-advertising: {idle:?}").into());
        }
        previous = written;
    }
    Ok(())
}

fn validate_progress(e: &Evidence, connection: u32, value: u8, address: [u8; 6]) -> Result<()> {
    if !e.connected
        || e.advertising
        || e.connections != connection
        || e.disconnections != connection - 1
        || e.writes != connection
        || e.reads < 2 * connection
        || e.value != value
        || e.address != Some(address)
    {
        return Err(format!("ATT peer and application observations disagree: {e:?}").into());
    }
    Ok(())
}

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn service(response: &[u8]) -> Result<(u16, u16)> {
    if response.len() != 5 || response[0] != 7 {
        return Err("expected exactly one ATT service range".into());
    }
    let range = (word(&response[1..]), word(&response[3..]));
    if range.0 == 0 || range.0 >= range.1 {
        return Err("invalid ATT service range".into());
    }
    Ok(range)
}
fn characteristic(response: &[u8], service: (u16, u16)) -> Result<u16> {
    if response.len() != 9
        || response[..2] != [9, 7]
        || response[4] != 0x0a
        || word(&response[7..]) != 0xfff1
    {
        return Err("expected one read/write application characteristic".into());
    }
    let declaration = word(&response[2..]);
    let value = word(&response[5..]);
    if declaration <= service.0 || declaration >= value || value > service.1 {
        return Err("characteristic handle outside discovered service".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_rejects_truncation_duplicates_and_foreign_handles() {
        assert_eq!(service(&[7, 8, 0, 10, 0]).unwrap(), (8, 10));
        for response in [
            &[][..],
            &[7, 8, 0],
            &[7, 0, 0, 2, 0],
            &[7, 8, 0, 7, 0],
            &[7, 8, 0, 10, 0, 11, 0, 12, 0],
        ] {
            assert!(service(response).is_err());
        }
        let valid = [9, 7, 9, 0, 0x0a, 10, 0, 0xf1, 0xff];
        assert_eq!(characteristic(&valid, (8, 10)).unwrap(), 10);
        for length in 0..valid.len() {
            assert!(characteristic(&valid[..length], (8, 10)).is_err());
        }
        assert!(characteristic(&valid, (10, 15)).is_err());
        assert!(characteristic(&valid, (8, 9)).is_err());
        let mut wrong = valid;
        wrong[7] = 0xf2;
        assert!(characteristic(&wrong, (8, 10)).is_err());
    }
    #[test]
    fn peer_success_cannot_hide_application_restart_or_missing_delivery() {
        let good = Evidence {
            address: Some([1; 6]),
            connected: true,
            connections: 2,
            disconnections: 1,
            reads: 4,
            writes: 2,
            value: 0x52,
            ..Evidence::default()
        };
        assert!(validate_progress(&good, 2, 0x52, [1; 6]).is_ok());
        for bad in [
            Evidence {
                connections: 1,
                ..good
            },
            Evidence { reads: 3, ..good },
            Evidence { writes: 1, ..good },
            Evidence { value: 0, ..good },
            Evidence {
                connected: false,
                ..good
            },
            Evidence {
                address: Some([2; 6]),
                ..good
            },
        ] {
            assert!(validate_progress(&bad, 2, 0x52, [1; 6]).is_err());
        }
    }
}
