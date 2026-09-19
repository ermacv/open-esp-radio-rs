//! Independent Linux SMP/GATT peer with automated, boot-bound HIL comparison.
mod comparison;
use crate::{
    Result,
    execution::context::Context,
    fixture::bluetooth::{
        att,
        model::{Adapter, PeerAddress},
        secure_gatt::Owner,
    },
    session::SerialCapture,
};
use open_esp_radio_hil_protocol::BluetoothSecureGattEvidence as Evidence;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn preflight(adapter: Adapter) -> Result<()> {
    att::preflight(adapter)
}
fn join(a: Result<()>, b: Result<()>) -> Result<()> {
    match (a, b) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Err(a), Err(b)) => Err(format!("{a}; cleanup: {b}").into()),
    }
}
fn sample(capture: &SerialCapture, samples: &mut Vec<Evidence>) -> Result<Evidence> {
    let e = capture.bluetooth_secure_gatt()?;
    samples.push(e);
    if e.application_stopped {
        return Err("secure application stopped".into());
    }
    if !e
        .traffic
        .cpu0_stack
        .is_some_and(|s| s.has_required_headroom())
    {
        return Err("secure GATT stack headroom missing or insufficient".into());
    }
    Ok(e)
}
fn wait(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    predicate: impl Fn(&Evidence) -> bool,
) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let e = sample(capture, samples)?;
        if predicate(&e) {
            return Ok(e);
        }
        if Instant::now() >= deadline {
            return Err(format!("secure GATT transition deadline: {e:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

pub(crate) fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("Bluetooth adapter required")?;
    preflight(adapter)?;
    let mut radio = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut samples = Vec::new();
        let mut exchanges = Vec::new();
        let mut peer = None;
        let probe = (|| -> Result<()> {
            let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
            if !capabilities.features.bluetooth_secure_gatt || capabilities.features.bluetooth_gatt { return Err("exclusive secure GATT image required".into()); }
            let initial = wait(capture,&mut samples,|e|e.traffic.advertising && e.traffic.address.is_some())?;
            if initial.traffic.connections != 0 || initial.bonds_stored != 0 || initial.comparisons != 0 || initial.traffic.value != 0 { return Err("fresh secure application epoch required".into()); }
            let address = PeerAddress(initial.traffic.address.ok_or("DUT address missing")?);
            // Reserve the previously absent device record before raw ATT or discovery.
            peer = Some(Owner::acquire(adapter,address)?);
            let peer = peer.as_mut().unwrap();
            plaintext_denied(&radio,address,&mut exchanges)?;
            let idle = wait(capture,&mut samples,|e| !e.traffic.connected && e.traffic.advertising && e.traffic.disconnections == 1)?;
            if idle.traffic.writes != 0 || idle.traffic.value != 0 || idle.bonds_stored != 0 { return Err("plaintext ATT changed protected state".into()); }
            peer.discover()?;
            pairing(capture,peer,&mut samples,false, &mut exchanges)?;
            peer.disconnect()?;
            let declined = wait(capture,&mut samples,|e| !e.traffic.connected && e.traffic.advertising && e.declined == 1)?;
            if declined.bonds_stored != 0 || declined.accepted != 0 { return Err("declined pairing established application trust".into()); }
            pairing(capture,peer,&mut samples,true, &mut exchanges)?;
            wait(capture,&mut samples,|e|e.bonds_stored == 1 && e.accepted == 1)?;
            protected_exchange(peer,0,0x41,&mut exchanges)?;
            peer.disconnect()?;
            wait(capture,&mut samples,|e|!e.traffic.connected && e.traffic.advertising)?;
            peer.connect()?;
            protected_exchange(peer,0x41,0x62,&mut exchanges)?;
            let resumed = wait(capture,&mut samples,|e|e.bonds_resumed == 1 && e.notifications_queued == 2)?;
            if resumed.bonds_stored != 1 || resumed.comparisons != 2 || resumed.accepted != 1 || resumed.declined != 1 || resumed.traffic.writes != 2 || resumed.traffic.value != 0x62 { return Err("bonded reconnect silently repaired, downgraded or changed epoch".into()); }
            peer.disconnect()?;
            wait(capture,&mut samples,|e|!e.traffic.connected && e.traffic.advertising)?;
            let boot = capture.latest_boot_id().ok_or("secure GATT boot missing")?;
            capture.restart_bluetooth_gatt(boot, resumed.epoch)?;
            let restarted = wait(capture,&mut samples,|e|e.epoch == resumed.epoch + 1 && !e.restarting && e.traffic.advertising)?;
            if capture.latest_boot_id() != Some(boot) || restarted.cold_releases != 1 || !restarted.old_hci_closed || restarted.bonds_stored != 1 {
                return Err("cold restart did not retain the application bond and retire the old HCI epoch".into());
            }
            peer.connect()?;
            protected_exchange(peer,0,0x73,&mut exchanges)?;
            let restored = wait(capture,&mut samples,|e|e.bonds_resumed == 2 && e.notifications_queued == 3)?;
            if restored.comparisons != 2 || restored.accepted != 1 || restored.declined != 1 || restored.bonds_stored != 1 {
                return Err("cold reconnect required re-pairing or changed the retained bond".into());
            }
            peer.disconnect()?;
            wait(capture,&mut samples,|e|!e.traffic.connected && e.traffic.advertising)?;
            Ok(())
        })();
        let cleanup = oer_process::cleanup(|| {
            let bond = peer.as_mut().map_or(Ok(()), Owner::restore);
            let power = radio.restore();
            join(bond,power)
        });
        crate::evidence::run::atomic_json(&output.join("trouble-secure-gatt.json"), &serde_json::json!({
            "schema":3, "pairing":"numeric-comparison-only", "dut_store":"ram", "linux_bond":"temporary",
            "confirmation":"automated-hil-number-comparison", "human_presence_verified":false,
            "samples":samples,"peer_exchanges":exchanges,"passed":probe.is_ok() && cleanup.is_ok(),
            "error":probe.as_ref().err().map(ToString::to_string), "restored":cleanup.is_ok(),
            "cleanup_error":cleanup.as_ref().err().map(ToString::to_string),
            "coordinated_controller_retirement":probe.is_ok(),
        }))?;
        join(probe,cleanup)
    });
    join(result, oer_process::cleanup(|| radio.restore()))
}

fn pairing(
    capture: &SerialCapture,
    peer: &Owner,
    samples: &mut Vec<Evidence>,
    accept: bool,
    exchanges: &mut Vec<serde_json::Value>,
) -> Result<()> {
    std::thread::scope(|scope| {
        let pair = scope.spawn(|| peer.pair());
        let decision = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut prompt = None;
            loop {
                if prompt.is_none() {
                    prompt = peer.prompt()?;
                }
                let e = sample(capture, samples)?;
                if let (Some(linux), Some(dut)) = (prompt.as_ref(), e.comparison) {
                    let boot = capture.latest_boot_id().ok_or("boot identity missing")?;
                    exchanges.push(serde_json::json!({"boot":boot,"dut_challenge":dut,"linux_number":linux.number,"requested_accept":accept}));
                    let decision = comparison::decision(boot, dut, linux.number, accept)?;
                    // Only this HIL scenario acts as the test operator. The DUT
                    // still requires an explicit, exact boot/request decision.
                    capture.confirm_bluetooth_gatt(boot, decision)?;
                    if accept {
                        prompt.take().unwrap().answer(true)?;
                    } else {
                        // Let the DUT consume its explicit decline first. Its
                        // SMP failure may already cancel the peer's callback;
                        // dropping the prompt rejects any still-pending request.
                        wait(capture, samples, |e| e.declined == 1)?;
                        drop(prompt.take());
                    }
                    return Ok(());
                }
                if pair.is_finished() {
                    return Err("Pair ended without both Numeric Comparison prompts".into());
                }
                if Instant::now() >= deadline {
                    return Err("Numeric Comparison prompts missing".into());
                }
                oer_process::sleep(Duration::from_millis(50))?;
            }
        })();
        let cancel = if decision.is_err() && !pair.is_finished() {
            peer.cancel_pairing()
        } else {
            Ok(())
        };
        let paired = pair.join().map_err(|_| "BlueZ pairing worker panicked")?;
        join(decision, cancel)?;
        match (accept, paired) {
            (true, Ok(())) | (false, Err(_)) => Ok(()),
            (true, Err(e)) => Err(e),
            (false, Ok(())) => Err("rejected pairing unexpectedly succeeded".into()),
        }
    })
}

fn protected_exchange(
    peer: &Owner,
    previous: u8,
    value: u8,
    evidence: &mut Vec<serde_json::Value>,
) -> Result<()> {
    let characteristic = peer.characteristic()?;
    let before = peer.read(&characteristic)?;
    if before != [previous] {
        return Err("protected value did not survive reconnect".into());
    }
    let notify = peer.notify(&characteristic)?;
    peer.write(&characteristic, value)?;
    let notification = notify.receive()?;
    let after = peer.read(&characteristic)?;
    evidence.push(serde_json::json!({"read_before":before,"written":value,"notification":notification,"read_after":after}));
    if notification != [value] || after != [value] {
        return Err("independent protected read/notification mismatch".into());
    }
    Ok(())
}

fn plaintext_denied(
    radio: &att::Owner,
    address: PeerAddress,
    evidence: &mut Vec<serde_json::Value>,
) -> Result<()> {
    let peer = radio.connect(address)?;
    let mut exchange = |request: &[u8]| -> Result<Vec<u8>> {
        peer.send(request)?;
        let response = peer.receive()?;
        evidence.push(serde_json::json!({"plaintext_request":request,"response":response}));
        Ok(response)
    };
    let service = exchange(&[6, 1, 0, 255, 255, 0, 0x28, 0xf0, 0xff])?;
    if service.len() != 5 || service[0] != 7 {
        return Err("secure service discovery failed".into());
    }
    let characteristic = exchange(&[8, service[1], service[2], service[3], service[4], 3, 0x28])?;
    let (value, cccd_start) = discover_value(&characteristic, &service)?;
    let [low, high] = value.to_le_bytes();
    let [dl, dh] = cccd_start.to_le_bytes();
    let descriptors = exchange(&[4, dl, dh, service[3], service[4]])?;
    if descriptors.len() != 6 || descriptors[..2] != [5, 1] || descriptors[4..] != [2, 0x29] {
        return Err("expected one discovered CCCD".into());
    }
    let cccd = u16::from_le_bytes([descriptors[2], descriptors[3]]);
    let end = u16::from_le_bytes([service[3], service[4]]);
    if cccd < cccd_start || cccd > end {
        return Err("CCCD outside service".into());
    }
    for request in [
        vec![0x0a, low, high],
        vec![0x12, low, high, 0x99],
        vec![0x12, descriptors[2], descriptors[3], 1, 0],
    ] {
        require_denied(&request, &exchange(&request)?)?;
    }
    Ok(())
}
fn discover_value(characteristic: &[u8], service: &[u8]) -> Result<(u16, u16)> {
    if characteristic.len() != 9
        || characteristic[..2] != [9, 7]
        || characteristic[4] != 0x1a
        || characteristic[7..] != [0xf1, 0xff]
    {
        return Err("expected read/write/notify value".into());
    }
    let start = u16::from_le_bytes([service[1], service[2]]);
    let end = u16::from_le_bytes([service[3], service[4]]);
    let declaration = u16::from_le_bytes([characteristic[2], characteristic[3]]);
    let value = u16::from_le_bytes([characteristic[5], characteristic[6]]);
    if start == 0 || declaration <= start || value <= declaration || value >= end {
        return Err("invalid secure service handles".into());
    }
    Ok((value, value + 1))
}
fn require_denied(request: &[u8], response: &[u8]) -> Result<()> {
    if response.len() == 5
        && response[0] == 1
        && response[1..4] == request[..3]
        && matches!(response[4], 5 | 8)
    {
        Ok(())
    } else {
        Err(format!("protected plaintext access was not denied: {response:?}").into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denial_requires_exact_operation_handle_and_security_error() {
        assert!(require_denied(&[10, 17, 0], &[1, 10, 17, 0, 5]).is_ok());
        for response in [
            &[11, 0][..],
            &[1, 10, 18, 0, 5],
            &[1, 10, 17, 0, 1],
            &[1, 18, 17, 0, 5],
        ] {
            assert!(require_denied(&[10, 17, 0], response).is_err());
        }
    }
    #[test]
    fn secure_discovery_rejects_plaintext_properties_and_foreign_handles() {
        let service = [7, 8, 0, 11, 0];
        let value = [9, 7, 9, 0, 0x1a, 10, 0, 0xf1, 0xff];
        assert_eq!(discover_value(&value, &service).unwrap(), (10, 11));
        let mut wrong = value;
        wrong[4] = 0x0a;
        assert!(discover_value(&wrong, &service).is_err());
        assert!(discover_value(&value, &[7, 10, 0, 11, 0]).is_err());
    }
}
