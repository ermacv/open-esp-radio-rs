//! Boards taken out of service.
//!
//! A board under maintenance serves only the owner who took it out: every
//! other request that claims it, or the whole stand, is refused with the
//! reason instead of queueing. Maintenance never stops a lease already held;
//! it keeps later ones away. The state is `maintenance.json` beside the
//! queue, so every checkout of the host user sees it.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::{Arbiter, state::Claim};

const SCHEMA: u32 = 1;

/// One board out of service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Maintenance {
    /// The board's MAC.
    pub mac: String,
    /// The only owner whose requests may claim the board.
    pub owner: String,
    pub reason: String,
    pub since_unix: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Boards {
    schema: u32,
    boards: Vec<Maintenance>,
}

impl Arbiter {
    fn maintenance_path(&self) -> std::path::PathBuf {
        self.directory().join("maintenance.json")
    }

    /// The boards out of service.
    pub fn maintenance(&self) -> crate::Result<Vec<Maintenance>> {
        let path = self.maintenance_path();
        self.locked(|| read(&path).map(|boards| boards.boards))
    }

    /// Take a board out of service for `entry.owner`, replacing an earlier
    /// entry of the same board.
    pub fn set_maintenance(&self, entry: Maintenance) -> crate::Result<()> {
        let path = self.maintenance_path();
        self.locked(|| {
            let mut boards = read(&path)?;
            boards.boards.retain(|known| known.mac != entry.mac);
            boards.boards.push(entry);
            write(&path, &boards)
        })
    }

    /// Return the board with `mac` to service; whether it was out of service.
    pub fn clear_maintenance(&self, mac: &str) -> crate::Result<bool> {
        let path = self.maintenance_path();
        self.locked(|| {
            let mut boards = read(&path)?;
            let before = boards.boards.len();
            boards.boards.retain(|known| known.mac != mac);
            let cleared = boards.boards.len() != before;
            write(&path, &boards)?;
            Ok(cleared)
        })
    }
}

/// Why `owner` may not claim `claims`: a board, or the whole stand, holding
/// a board under another owner's maintenance.
pub(crate) fn refusal(boards: &[Maintenance], owner: &str, claims: &[Claim]) -> Option<String> {
    let whole = claims.iter().any(|claim| claim.resource == crate::STAND);
    boards
        .iter()
        .filter(|board| board.owner != owner)
        .find(|board| {
            whole
                || claims
                    .iter()
                    .any(|claim| claim.resource == Claim::board(&board.mac).resource)
        })
        .map(|board| {
            format!(
                "board {} is under maintenance by {}: {}; it serves only its maintainer until `cargo hil devices release`",
                board.mac, board.owner, board.reason
            )
        })
}

fn read(path: &std::path::Path) -> crate::Result<Boards> {
    match fs::read(path) {
        Ok(bytes) => {
            let boards: Boards = serde_json::from_slice(&bytes)?;
            if boards.schema != SCHEMA {
                return Err(format!(
                    "{} has schema {}; update this checkout",
                    path.display(),
                    boards.schema
                )
                .into());
            }
            Ok(boards)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Boards {
            schema: SCHEMA,
            boards: Vec::new(),
        }),
        Err(error) => Err(error.into()),
    }
}

fn write(path: &std::path::Path, boards: &Boards) -> crate::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(boards)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(mac: &str, owner: &str) -> Maintenance {
        Maintenance {
            mac: mac.into(),
            owner: owner.into(),
            reason: "USB hangs after a reset".into(),
            since_unix: 1,
        }
    }

    #[test]
    fn a_board_under_maintenance_serves_only_its_maintainer() {
        let boards = [board("AA", "stand")];
        let on = |mac: &str| vec![Claim::board(mac), Claim::shared(crate::AIR)];
        assert!(
            refusal(&boards, "802154", &on("AA"))
                .unwrap()
                .contains("maintenance by stand")
        );
        assert_eq!(refusal(&boards, "stand", &on("AA")), None);
        assert_eq!(refusal(&boards, "802154", &on("BB")), None, "another board");
        assert!(
            refusal(&boards, "802154", &[Claim::stand()]).is_some(),
            "the whole stand"
        );
        assert_eq!(refusal(&[], "802154", &on("AA")), None);
    }

    #[test]
    fn maintenance_is_set_replaced_and_cleared() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        assert!(arbiter.maintenance().unwrap().is_empty());
        arbiter.set_maintenance(board("AA", "stand")).unwrap();
        arbiter.set_maintenance(board("AA", "other")).unwrap();
        assert_eq!(arbiter.maintenance().unwrap(), [board("AA", "other")]);
        assert!(arbiter.clear_maintenance("AA").unwrap());
        assert!(!arbiter.clear_maintenance("AA").unwrap());
    }
}
