//! Exhaust one Host RX credit, observe supervision, drain, and reuse the handle.
use crate::{
    Result,
    execution::context::Context,
    fixture::bluetooth::{att, model::PeerAddress},
    session::SerialCapture,
};
use open_esp_radio_hil_protocol::{
    BluetoothPeripheralEvidence as Evidence, BluetoothPeripheralOperation as Op,
    BluetoothPeripheralResult as Outcome, BluetoothPeripheralTermination,
    bluetooth_backpressure_packet,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(output: &Path, context: &Context<'_>, active_maintenance: bool) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut samples = Vec::new();
        let probe = exercise(capture, &owner, &mut samples, active_maintenance);
        let cleanup = oer_process::cleanup(|| -> Result<()> {
            capture.bluetooth_peripheral(Op::HoldAclCredit { hold: false })?;
            owner.restore()?;
            wait(capture, Duration::from_secs(6), &mut samples, |e| {
                let b = e.acl_backpressure.ok_or("missing backpressure evidence")?;
                Ok(!b.held
                    && b.received == b.returned
                    && e.peripheral_disconnections >= e.connection_complete_events)
            })?;
            let disabled = capture.bluetooth_peripheral(Op::AclBackpressure { enabled: false })?;
            if disabled.result != (Outcome::AclBackpressureConfigured { enabled: false }) {
                return Err("Host mode restoration rejected".into());
            }
            samples.push(disabled);
            let retired = capture.bluetooth_peripheral(Op::Retire)?;
            if !retired.is_retired(Op::Retire) {
                return Err("cold retirement incomplete".into());
            }
            samples.push(retired);
            Ok(())
        });
        crate::evidence::run::atomic_json(
            &output.join("acl-backpressure.json"),
            &serde_json::json!({
                "schema":1, "samples":samples, "active_maintenance":active_maintenance,
                "passed":probe.is_ok() && cleanup.is_ok(),
                "error":probe.as_ref().err().map(ToString::to_string), "restored":cleanup.is_ok(),
                "cleanup_error":cleanup.as_ref().err().map(ToString::to_string)
            }),
        )?;
        join(probe, cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    join(result, restore)
}
fn join(a: Result<()>, b: Result<()>) -> Result<()> {
    match (a, b) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Ok(())) | (Ok(()), Err(a)) => Err(a),
        (Err(a), Err(b)) => Err(format!("{a}; cleanup: {b}").into()),
    }
}
fn start(
    capture: &SerialCapture,
    owner: &att::Owner,
    samples: &mut Vec<Evidence>,
    connection: u32,
) -> Result<att::Att> {
    let op = Op::StartAdvertising {
        termination: BluetoothPeripheralTermination::PeerReset,
        hold_millis: 0,
    };
    let started = capture.bluetooth_peripheral(op)?;
    let peer = owner.connect(PeerAddress(
        started.started_address(op).ok_or("advertising failed")?,
    ))?;
    samples.push(started);
    peer.send(&[2, 247, 0])?;
    if peer.receive()? != [3, 247, 0] {
        return Err("fresh connection MTU exchange failed".into());
    }
    wait(capture, Duration::from_secs(2), samples, |e| {
        let b = e.acl_backpressure.ok_or("missing backpressure evidence")?;
        Ok(e.connection_complete_events == connection
            && b.mtu_exchanged
            && b.sent == connection
            && b.completed == b.sent
            && b.received == b.returned)
    })?;
    Ok(peer)
}
fn exercise(
    capture: &SerialCapture,
    owner: &att::Owner,
    samples: &mut Vec<Evidence>,
    active_maintenance: bool,
) -> Result<()> {
    if !capture
        .request_capabilities(Duration::from_secs(10))?
        .features
        .bluetooth_dtm
    {
        return Err("Bluetooth diagnostic image required".into());
    }
    let configured = capture.bluetooth_peripheral(Op::AclBackpressure { enabled: true })?;
    if configured.result != (Outcome::AclBackpressureConfigured { enabled: true }) {
        return Err("Host mode rejected".into());
    }
    samples.push(configured);
    let peer = start(capture, owner, samples, 1)?;
    if active_maintenance {
        let baseline = samples.last().ok_or("missing live ACL evidence")?.clone();
        wait(capture, Duration::from_secs(4), samples, |e| {
            maintenance_before_hold(&baseline, e)
        })?;
    }
    let armed = capture.bluetooth_peripheral(Op::HoldAclCredit { hold: true })?;
    if armed.result != (Outcome::AclCreditHoldConfigured { hold: true }) {
        return Err("credit hold rejected".into());
    }
    samples.push(armed);
    peer.send(&bluetooth_backpressure_packet()[4..])?;
    let held = wait(capture, Duration::from_secs(2), samples, |e| {
        Ok(e.acl_backpressure.is_some_and(|b| b.held))
    })?;
    let b = held.acl_backpressure.unwrap();
    if !(100..=8000).contains(&b.supervision_timeout_millis)
        || b.returned.checked_add(1) != Some(b.received)
    {
        return Err("unsupported supervision interval or inexact held credit".into());
    }
    let disconnected = wait(
        capture,
        Duration::from_millis(u64::from(b.supervision_timeout_millis) + 1500),
        samples,
        |e| Ok(e.disconnection_complete_events == 1),
    )?;
    validate_timeout(&disconnected)?;
    // Until this point the peer socket stays live: no Reset, rfkill, disconnect
    // or Test End is used to generate the target's supervision expiration.
    drop(peer);
    let released = capture.bluetooth_peripheral(Op::HoldAclCredit { hold: false })?;
    samples.push(released);
    wait(capture, Duration::from_secs(3), samples, |e| {
        let b = e.acl_backpressure.ok_or("missing backpressure evidence")?;
        Ok(e.peripheral_disconnections == 1 && !b.held && b.received == b.returned)
    })?;
    // Same boot, HCI session and handle: only fresh advertising, no Host Reset.
    let peer = start(capture, owner, samples, 2)?;
    let last = samples.last().unwrap();
    if last.target_reset_commands != 0 || last.disconnection_complete_events != 1 {
        return Err("recovery changed the HCI lifecycle".into());
    }
    drop(peer);
    Ok(())
}
fn validate_timeout(e: &Evidence) -> Result<()> {
    validate_health(e)?;
    let b = e.acl_backpressure.ok_or("missing backpressure evidence")?;
    let elapsed = b
        .disconnected_at_millis
        .checked_sub(b.held_at_millis)
        .ok_or("invalid event timing")?;
    if !b.enabled
        || !b.held
        || b.armed
        || !b.disconnected_while_held
        || b.held_at_millis == 0
        || b.returned.checked_add(1) != Some(b.received)
        || b.faults != 0
        || !(100..=8000).contains(&b.supervision_timeout_millis)
        || e.last_disconnect_reason != Some(0x08)
        || e.disconnection_complete_events != 1
        || e.target_reset_commands != 0
        || e.target_disconnect_commands != 0
        || elapsed.saturating_add(250) < b.supervision_timeout_millis
        || elapsed > b.supervision_timeout_millis + 1000
    {
        return Err("supervision event was not delivered within bounds while retaining exactly one Host credit".into());
    }
    Ok(())
}
fn validate_health(e: &Evidence) -> Result<()> {
    if e.terminal
        || e.saturated
        || e.host_event_faults != 0
        || e.host_acl_faults != 0
        || e.acl_backpressure.is_none_or(|b| b.faults != 0)
    {
        return Err("Controller/Host invariant failed".into());
    }
    Ok(())
}

fn maintenance_before_hold(baseline: &Evidence, e: &Evidence) -> Result<bool> {
    validate_health(e)?;
    if e.connection_complete_events != baseline.connection_complete_events
        || e.disconnection_complete_events != baseline.disconnection_complete_events
        || e.target_reset_commands != baseline.target_reset_commands
        || e.target_disconnect_commands != baseline.target_disconnect_commands
    {
        return Err("connection changed before backpressure maintenance proof".into());
    }
    if e.phy_peripheral_maintenance <= baseline.phy_peripheral_maintenance {
        return Ok(false);
    }
    if super::active_maintenance_restoration_pending(e) {
        return Ok(false);
    }
    super::require_active_maintenance(baseline.phy_peripheral_maintenance, e)?;
    let m = e
        .phy_maintenance
        .as_ref()
        .ok_or("maintenance timing missing")?;
    if !matches!((m.run_at_micros, m.restoration_deadline_micros),
        (Some(run), Some(deadline)) if m.admitted_at_micros <= run && run < deadline)
    {
        return Err("maintenance RUN did not precede its restoration deadline".into());
    }
    Ok(true)
}
fn wait(
    capture: &SerialCapture,
    timeout: Duration,
    samples: &mut Vec<Evidence>,
    predicate: impl Fn(&Evidence) -> Result<bool>,
) -> Result<Evidence> {
    let deadline = Instant::now() + timeout;
    loop {
        let e = capture.bluetooth_peripheral(Op::Snapshot)?;
        samples.push(e.clone());
        validate_health(&e)?;
        if predicate(&e)? {
            return Ok(e);
        }
        if Instant::now() >= deadline {
            return Err("backpressure observation deadline exceeded".into());
        }
        oer_process::sleep(Duration::from_millis(20))?;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_requires_event_before_credit_return_and_original_timer_bounds() {
        let mut e = super::super::peripheral_tests::evidence();
        e.disconnection_complete_events = 1;
        e.last_disconnect_reason = Some(0x08);
        e.acl_backpressure = Some(
            open_esp_radio_hil_protocol::BluetoothAclBackpressureEvidence {
                enabled: true,
                held: true,
                received: 2,
                returned: 1,
                held_at_millis: 1000,
                disconnected_at_millis: 3000,
                disconnected_while_held: true,
                supervision_timeout_millis: 2000,
                ..Default::default()
            },
        );
        validate_timeout(&e).unwrap();
        for case in 0..13 {
            let mut bad = e.clone();
            let b = bad.acl_backpressure.as_mut().unwrap();
            match case {
                0 => b.held = false,
                1 => b.returned = 2,
                2 => b.disconnected_while_held = false,
                3 => b.disconnected_at_millis = 5000,
                4 => b.disconnected_at_millis = 1100,
                5 => bad.last_disconnect_reason = Some(0x13),
                6 => bad.target_reset_commands = 1,
                7 => b.returned = u32::MAX,
                8 => b.supervision_timeout_millis = u32::MAX,
                9 => bad.host_acl_faults = 1,
                10 => bad.host_event_faults = 1,
                11 => bad.terminal = true,
                _ => bad.saturated = true,
            }
            assert!(validate_timeout(&bad).is_err());
        }
    }

    #[test]
    fn maintenance_requires_new_guarded_restoration_on_the_original_connection() {
        let mut baseline = super::super::peripheral_tests::evidence();
        baseline.connection_complete_events = 1;
        baseline.acl_backpressure = Some(Default::default());
        baseline.phy_peripheral_maintenance = 2;
        assert!(!maintenance_before_hold(&baseline, &baseline).unwrap());
        let mut restored = baseline.clone();
        restored.phy_peripheral_maintenance = 3;
        restored.phy_maintenance = Some(
            open_esp_radio_hil_protocol::BluetoothPhyMaintenanceEvidence {
                restored: 3,
                restoration_deadline_micros: Some(200),
                run_at_micros: Some(190),
                ..Default::default()
            },
        );
        assert!(maintenance_before_hold(&baseline, &restored).unwrap());
        let mut pending = restored.clone();
        let m = pending.phy_maintenance.as_mut().unwrap();
        m.restored = 2;
        m.physical_finished_at_micros = Some(180);
        m.run_at_micros = None;
        assert!(!maintenance_before_hold(&baseline, &pending).unwrap());
        pending.phy_maintenance.as_mut().unwrap().invalid = true;
        assert!(maintenance_before_hold(&baseline, &pending).is_err());
        for case in 0..10 {
            let mut bad = restored.clone();
            match case {
                0 => bad.connection_complete_events += 1,
                1 => bad.disconnection_complete_events += 1,
                2 => bad.target_reset_commands += 1,
                3 => bad.target_disconnect_commands += 1,
                4 => bad.phy_maintenance.as_mut().unwrap().invalid = true,
                5 => bad.phy_maintenance.as_mut().unwrap().run_at_micros = None,
                6 => bad.host_acl_faults = 1,
                7 => bad.phy_maintenance.as_mut().unwrap().run_at_micros = Some(200),
                8 => {
                    bad.phy_maintenance
                        .as_mut()
                        .unwrap()
                        .restoration_deadline_micros = None
                }
                _ => bad.phy_maintenance.as_mut().unwrap().admitted_at_micros = 191,
            }
            assert!(maintenance_before_hold(&baseline, &bad).is_err());
        }
    }
}
