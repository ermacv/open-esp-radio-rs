//! A board's hub port returns to its working state when its lease changes
//! hands: when a lease is granted, for a holder that ended without releasing,
//! and when it is released, before the next holder gets the board. A port
//! that is off is powered; a powered port whose board is not attached (gone,
//! or enumerated without its serial number) is cycled. Each restoration is a
//! recovery in the board journal. Only a board whose device access the
//! lease holds is touched.

use std::time::{Duration, Instant};

use oer_device_lock::{DeviceAccess, DeviceId};
use oer_devices::reset::RecoveryStep;
use oer_stand_file::StandFile;
use oer_stand_power::HubPower;

use oer_stand_claims::Claim;

use oer_stand_journal::BoardEventKind;

/// How long a board may take to attach after its port is powered.
const ATTACH: Duration = Duration::from_secs(10);

/// The MACs of the boards `claims` reach: each claimed board, or every board
/// of the stand file for the whole stand.
pub(crate) fn claimed_boards(claims: &[Claim], stand: Option<&StandFile>) -> Vec<DeviceId> {
    if claims
        .iter()
        .any(|claim| claim.resource == oer_stand_claims::STAND)
    {
        return stand
            .map(|stand| {
                stand
                    .board
                    .iter()
                    .filter_map(|board| board.mac().ok())
                    .collect()
            })
            .unwrap_or_default();
    }
    claims
        .iter()
        .filter_map(|claim| claim.resource.strip_prefix("board:"))
        .filter_map(|mac| DeviceId::parse(mac).ok())
        .collect()
}

/// Restore every board of `devices` (the lease's accesses) that resets by
/// power; a failure is reported, never fatal to the lease change.
pub(crate) fn restore(arbiter: &crate::Arbiter, devices: &[DeviceAccess], origin: &str) {
    if devices.is_empty() {
        return;
    }
    let stand = match arbiter.stand() {
        Ok(stand) => stand,
        Err(error) => {
            eprintln!("hil-arbiter: board restoration skipped: {error}");
            return;
        }
    };
    for board in &stand.board {
        let Ok(mac) = board.mac() else { continue };
        let Some(access) = devices.iter().find(|device| *device.id() == mac) else {
            continue;
        };
        let Some(port) = stand.hub_port(board) else {
            continue;
        };
        if let Err(error) = restore_one(arbiter, access, &HubPower::new(port), origin) {
            eprintln!("hil-arbiter: {mac} not restored: {error}");
        }
    }
}

fn restore_one(
    arbiter: &crate::Arbiter,
    access: &DeviceAccess,
    power: &HubPower,
    origin: &str,
) -> crate::Result<()> {
    let _operation = access.operation()?;
    let mac = access.id();
    if oer_devices::discovery::is_attached(mac) {
        return Ok(());
    }
    let mut step = if power.is_on(_operation.lifetime())? {
        power.cycle(_operation.lifetime())?;
        RecoveryStep::PowerCycle
    } else {
        power.on(_operation.lifetime())?;
        RecoveryStep::PowerOn
    };
    let mut back = wait_attached(mac);
    if !back && step == RecoveryStep::PowerOn {
        power.cycle(_operation.lifetime())?;
        step = RecoveryStep::PowerCycle;
        back = wait_attached(mac);
    }
    arbiter.journal().record(
        Some(mac.to_string()),
        BoardEventKind::Recovered {
            step,
            hardware: true,
            reset_line: None,
            origin: origin.to_owned(),
        },
    )?;
    if back {
        eprintln!("hil-arbiter: {mac} restored ({step:?})");
        Ok(())
    } else {
        Err(format!("not attached {}s after {step:?}", ATTACH.as_secs()).into())
    }
}

fn wait_attached(mac: &DeviceId) -> bool {
    let deadline = Instant::now() + ATTACH;
    while Instant::now() < deadline {
        if oer_devices::discovery::is_attached(mac) {
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
        let stand = StandFile::parse(
            "schema = 1\n[stand]\nid = \"t\"\nair = \"exclusive\"\n\
             [[hub]]\nid = \"h\"\nusb2 = \"1-1\"\nswitchable = [1, 2]\n\
             [[board]]\nid = \"a\"\nusb-serial = \"AA:AA:AA:AA:AA:01\"\nchip = \"chip-b\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 1 }\nreset = [\"power\"]\n\
             [[board]]\nid = \"b\"\nusb-serial = \"aa:aa:aa:aa:aa:02\"\nchip = \"chip-b\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 2 }\nreset = [\"jtag\"]\n",
        )
        .unwrap();
        let claims = [
            Claim::exclusive("board:AA:AA:AA:AA:AA:01"),
            Claim::exclusive(oer_stand_claims::AIR),
        ];
        assert_eq!(claimed_boards(&claims, Some(&stand)), ["AA:AA:AA:AA:AA:01"]);
        assert_eq!(
            claimed_boards(&[Claim::exclusive(oer_stand_claims::STAND)], Some(&stand)),
            ["AA:AA:AA:AA:AA:01", "AA:AA:AA:AA:AA:02"]
        );
        assert!(claimed_boards(&[Claim::exclusive(oer_stand_claims::STAND)], None).is_empty());
    }
}
