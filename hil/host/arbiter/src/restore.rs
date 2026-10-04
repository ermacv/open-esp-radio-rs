//! A board's hub port returns to its working state when its lease changes
//! hands: when a lease is granted, for a holder that ended without releasing,
//! and when it is released, before the next holder gets the board. A port
//! that is off is powered; a powered port whose board is not attached (gone,
//! or enumerated without its serial number) is cycled. Each restoration is a
//! recovery in the board journal.

use std::time::{Duration, Instant};

use crate::{BoardEventKind, Claim, Device, RecoveryStep};

/// How long a board may take to attach after its port is powered.
const ATTACH: Duration = Duration::from_secs(10);

/// The MACs of the boards `claims` reach: each claimed board, or every
/// registered board for the whole stand.
pub(crate) fn claimed_boards(claims: &[Claim], devices: &[Device]) -> Vec<String> {
    if claims.iter().any(|claim| claim.resource == crate::STAND) {
        return devices.iter().map(|device| device.mac.clone()).collect();
    }
    claims
        .iter()
        .filter_map(|claim| claim.resource.strip_prefix("board:"))
        .map(str::to_owned)
        .collect()
}

/// Restore every board of `macs` that has a registered hub port; a failure
/// is reported, never fatal to the lease change.
pub(crate) fn restore(arbiter: &crate::Arbiter, macs: &[String], origin: &str) {
    if macs.is_empty() {
        return;
    }
    let devices = match arbiter.devices() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("hil-arbiter: board restoration skipped: {error}");
            return;
        }
    };
    for device in devices.iter().filter(|device| macs.contains(&device.mac)) {
        if let Err(error) = restore_one(arbiter, device, origin) {
            eprintln!("hil-arbiter: {} not restored: {error}", device.mac);
        }
    }
}

fn restore_one(arbiter: &crate::Arbiter, device: &Device, origin: &str) -> crate::Result<()> {
    let Some(power) = device.power.as_ref() else {
        return Ok(());
    };
    if attached(&device.mac) {
        return Ok(());
    }
    let mut step = if power.is_on()? {
        power.cycle()?;
        RecoveryStep::PowerCycle
    } else {
        power.switch_on()?;
        RecoveryStep::PowerOn
    };
    let mut back = wait_attached(&device.mac);
    if !back && step == RecoveryStep::PowerOn {
        power.cycle()?;
        step = RecoveryStep::PowerCycle;
        back = wait_attached(&device.mac);
    }
    arbiter.record_board(
        Some(device.mac.clone()),
        BoardEventKind::Recovered {
            step,
            hardware: true,
            reset_line: None,
            origin: origin.to_owned(),
        },
    )?;
    if back {
        eprintln!("hil-arbiter: {} restored ({step:?})", device.mac);
        Ok(())
    } else {
        Err(format!("not attached {}s after {step:?}", ATTACH.as_secs()).into())
    }
}

fn attached(mac: &str) -> bool {
    crate::attached_ports()
        .iter()
        .any(|port| port.mac.as_deref() == Some(mac))
}

fn wait_attached(mac: &str) -> bool {
    let deadline = Instant::now() + ATTACH;
    while Instant::now() < deadline {
        if attached(mac) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_restores_its_claimed_boards_or_every_board_of_the_stand() {
        let device = |mac: &str| Device {
            mac: mac.to_owned(),
            name: mac.to_lowercase(),
            chip: "esp32s31".into(),
            power: None,
        };
        let devices = [device("AA"), device("BB")];
        let claims = [Claim::exclusive("board:AA"), Claim::exclusive(crate::AIR)];
        assert_eq!(claimed_boards(&claims, &devices), ["AA"]);
        assert_eq!(
            claimed_boards(&[Claim::exclusive(crate::STAND)], &devices),
            ["AA", "BB"]
        );
    }
}
