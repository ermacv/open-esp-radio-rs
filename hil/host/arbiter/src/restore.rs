//! A board's hub port returns to its working state when its lease changes
//! hands: when a lease is granted, for a holder that ended without releasing,
//! and when it is released, before the next holder gets the board. A port
//! that is off is powered; a powered port whose board is not attached (gone,
//! or enumerated without its serial number) is cycled. Each restoration is a
//! recovery in the board journal. The power itself is board I/O
//! (`oer-hil-board`'s `power`).

use std::time::{Duration, Instant};

use oer_hil_board::{power::HubPower, reset::RecoveryStep};
use oer_hil_stand_model::StandFile;

use crate::{BoardEventKind, Claim};

/// How long a board may take to attach after its port is powered.
const ATTACH: Duration = Duration::from_secs(10);

/// The MACs of the boards `claims` reach: each claimed board, or every board
/// of the stand file for the whole stand.
pub(crate) fn claimed_boards(claims: &[Claim], stand: Option<&StandFile>) -> Vec<String> {
    if claims.iter().any(|claim| claim.resource == crate::STAND) {
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
        .map(str::to_owned)
        .collect()
}

/// Restore every board of `macs` that resets by power; a failure is
/// reported, never fatal to the lease change.
pub(crate) fn restore(arbiter: &crate::Arbiter, macs: &[String], origin: &str) {
    if macs.is_empty() {
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
        let Some(port) = stand.hub_port(board).filter(|_| macs.contains(&mac)) else {
            continue;
        };
        if let Err(error) = restore_one(arbiter, &mac, &HubPower::new(port), origin) {
            eprintln!("hil-arbiter: {mac} not restored: {error}");
        }
    }
}

fn restore_one(
    arbiter: &crate::Arbiter,
    mac: &str,
    power: &HubPower,
    origin: &str,
) -> crate::Result<()> {
    if oer_hil_board::ports::is_attached(mac) {
        return Ok(());
    }
    let mut step = if power.is_on()? {
        power.cycle()?;
        RecoveryStep::PowerCycle
    } else {
        power.on()?;
        RecoveryStep::PowerOn
    };
    let mut back = wait_attached(mac);
    if !back && step == RecoveryStep::PowerOn {
        power.cycle()?;
        step = RecoveryStep::PowerCycle;
        back = wait_attached(mac);
    }
    arbiter.record_board(
        Some(mac.to_owned()),
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

fn wait_attached(mac: &str) -> bool {
    let deadline = Instant::now() + ATTACH;
    while Instant::now() < deadline {
        if oer_hil_board::ports::is_attached(mac) {
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
             [[board]]\nid = \"a\"\nusb-serial = \"AA:AA:AA:AA:AA:01\"\nchip = \"esp32c5\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 1 }\nreset = [\"power\"]\n\
             [[board]]\nid = \"b\"\nusb-serial = \"aa:aa:aa:aa:aa:02\"\nchip = \"esp32c5\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 2 }\nreset = [\"jtag\"]\n",
        )
        .unwrap();
        let claims = [
            Claim::exclusive("board:AA:AA:AA:AA:AA:01"),
            Claim::exclusive(crate::AIR),
        ];
        assert_eq!(claimed_boards(&claims, Some(&stand)), ["AA:AA:AA:AA:AA:01"]);
        assert_eq!(
            claimed_boards(&[Claim::exclusive(crate::STAND)], Some(&stand)),
            ["AA:AA:AA:AA:AA:01", "AA:AA:AA:AA:AA:02"]
        );
        assert!(claimed_boards(&[Claim::exclusive(crate::STAND)], None).is_empty());
    }
}
