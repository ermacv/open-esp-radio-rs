//! The board journal, `board.jsonl` in the arbiter directory: what each
//! board of the stand carries and what happened to it — flashed firmware,
//! the host-owned startup artifact (which carries the PHY calibration
//! cache), recoveries, quarantine releases and reset soaks. Boards are named
//! by their MAC.
//!
//! [`Arbiter::record_flash`] is the only writer of a flash: the flash
//! operation (`oer-hil-flash`) calls it between its write and its start,
//! and `cargo hil lease --flashed` for a flash a leased command made. Every
//! other change is recorded by [`Arbiter::record_board`], which refuses a
//! flash. [`Arbiter::latest_flash`] and [`Arbiter::carries`] read it back.

use std::fs;

use oer_hil_board::reset::{RecoveryStep, ResetPath};
use serde::{Deserialize, Serialize};

use crate::{Arbiter, history};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardEvent {
    pub unix: u64,
    pub owner: String,
    /// Repository checkout of the recording process.
    pub checkout: Option<String>,
    /// MAC of the board (its USB serial number); unknown in older records.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(flatten)]
    pub kind: BoardEventKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum BoardEventKind {
    Flashed {
        /// HIL image class or standalone example name.
        image: String,
        application_sha256: String,
        commit: Option<String>,
        dirty: Option<bool>,
        /// Run identity or command that flashed.
        origin: String,
    },
    StartupArtifactUploaded {
        path: String,
        sha256: String,
    },
    StartupArtifactWritten {
        path: String,
        sha256: String,
        disposition: String,
    },
    /// The stand recovered an unreachable board automatically.
    Recovered {
        /// The recovery step that brought it back.
        step: RecoveryStep,
        /// Whether the failure was below the firmware: the port vanished, the
        /// ROM waited for a download, or nothing answered at all.
        hardware: bool,
        /// The ROM's reset line after the step.
        reset_line: Option<String>,
        /// The run or command that recovered it.
        origin: String,
    },
    /// A person returned a quarantined board after a reset or power cycle.
    QuarantineReleased {
        confirmation: crate::maintenance::Confirmation,
        /// What the board answered to the release check.
        check: String,
    },
    /// A reset soak: `cycles` rounds of a reset through each of `paths`,
    /// ending at the first reset after which the board did not boot.
    Soaked {
        paths: Vec<ResetPath>,
        cycles: u32,
        resets: u32,
        /// The reset that failed and what the console showed; `None` when
        /// every reset booted.
        failure: Option<String>,
    },
}

impl std::fmt::Display for BoardEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let who = match &self.checkout {
            Some(checkout) => format!("{} ({checkout})", self.owner),
            None => self.owner.clone(),
        };
        match &self.kind {
            BoardEventKind::Flashed {
                image,
                application_sha256,
                commit,
                dirty,
                origin,
            } => {
                let commit = commit.as_deref().map_or_else(
                    || String::from("unknown commit"),
                    |commit| commit.chars().take(12).collect(),
                );
                let dirty = if *dirty == Some(true) { "+dirty" } else { "" };
                write!(
                    f,
                    "{who} flashed {image} {commit}{dirty} app {} ({origin})",
                    short_hash(application_sha256)
                )
            }
            BoardEventKind::StartupArtifactUploaded { path, sha256 } => write!(
                f,
                "{who} uploaded startup artifact {} from {path}",
                short_hash(sha256)
            ),
            BoardEventKind::StartupArtifactWritten {
                path,
                sha256,
                disposition,
            } => write!(
                f,
                "{who} wrote startup artifact {} ({disposition}) to {path}",
                short_hash(sha256)
            ),
            BoardEventKind::Recovered {
                step,
                hardware,
                reset_line,
                origin,
            } => write!(
                f,
                "{who} recovered it by {step:?} ({} failure) in {origin}: {}",
                if *hardware { "hardware" } else { "firmware" },
                reset_line.as_deref().unwrap_or("no reset line")
            ),
            BoardEventKind::QuarantineReleased {
                confirmation,
                check,
            } => write!(f, "{who} returned it after a {confirmation:?}: {check}"),
            BoardEventKind::Soaked {
                paths,
                cycles,
                resets,
                failure,
            } => write!(
                f,
                "{who} soaked it with {resets} resets in {cycles} cycles via {paths:?}: {}",
                failure.as_deref().unwrap_or("every reset booted")
            ),
        }
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// The newest flash of every board, in order of first appearance, and the
/// newest event of every startup-artifact host file. The artifact is a host
/// file uploaded at each boot, so it belongs to a checkout, not to a board.
pub(crate) fn latest(events: &[BoardEvent]) -> (Vec<&BoardEvent>, Vec<&BoardEvent>) {
    let flashes = newest_per_key(events, |event| match &event.kind {
        BoardEventKind::Flashed { .. } => Some(event.device.as_deref()),
        _ => None,
    });
    let artifacts = newest_per_key(events, |event| event.artifact_path().map(Some));
    (flashes, artifacts)
}

/// The newest event of every key, in order of first appearance; `key`
/// excludes an event by returning `None`.
fn newest_per_key(
    events: &[BoardEvent],
    key: impl Fn(&BoardEvent) -> Option<Option<&str>>,
) -> Vec<&BoardEvent> {
    let mut newest: Vec<&BoardEvent> = Vec::new();
    for event in events {
        let Some(this) = key(event) else { continue };
        match newest.iter_mut().find(|known| key(known) == Some(this)) {
            Some(known) => *known = event,
            None => newest.push(event),
        }
    }
    newest
}

impl BoardEvent {
    fn artifact_path(&self) -> Option<&str> {
        match &self.kind {
            BoardEventKind::StartupArtifactUploaded { path, .. }
            | BoardEventKind::StartupArtifactWritten { path, .. } => Some(path),
            BoardEventKind::Flashed { .. }
            | BoardEventKind::Recovered { .. }
            | BoardEventKind::QuarantineReleased { .. }
            | BoardEventKind::Soaked { .. } => None,
        }
    }
}

/// The stand file's label of a board, its MAC, or `unidentified board`.
pub(crate) fn device_label(
    device: Option<&str>,
    stand: Option<&oer_hil_stand_model::StandFile>,
) -> String {
    match (device, stand) {
        (Some(mac), Some(stand)) => stand.label(mac),
        (Some(mac), None) => mac.to_owned(),
        (None, _) => String::from("unidentified board"),
    }
}

/// What an image written into a board is: the name the journal knows it
/// by (a HIL image class, a catalog image, an example), the SHA-256 of its
/// application, the source it was built from and who wrote it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageIdentity {
    pub name: String,
    pub sha256: String,
    pub commit: Option<String>,
    pub dirty: Option<bool>,
    /// The run or command that wrote it.
    pub origin: String,
}

impl Arbiter {
    pub fn board_events(&self) -> crate::Result<Vec<BoardEvent>> {
        history::read_lines(&self.board_path())
    }

    /// The newest journaled flash of the board with `mac`.
    pub fn latest_flash(&self, mac: &str) -> crate::Result<Option<BoardEvent>> {
        Ok(self.board_events()?.into_iter().rev().find(|event| {
            event.device.as_deref() == Some(mac)
                && matches!(event.kind, BoardEventKind::Flashed { .. })
        }))
    }

    /// Whether the newest journaled flash of the board with `mac` is the
    /// image `name` with the application digest `sha256`.
    pub fn carries(&self, mac: &str, name: &str, sha256: &str) -> crate::Result<bool> {
        Ok(carried(self.latest_flash(mac)?.as_ref(), name, sha256))
    }

    /// Journal that `owner` wrote `image` into the board with `mac`: the
    /// only writer of a flash.
    pub fn record_flash(
        &self,
        owner: String,
        mac: &str,
        image: &ImageIdentity,
    ) -> crate::Result<()> {
        if image.sha256.len() != 64 || !image.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("`{}` is not a SHA-256", image.sha256).into());
        }
        self.append(
            owner,
            Some(oer_hil_stand_model::mac::normalize(mac)?),
            BoardEventKind::Flashed {
                image: image.name.clone(),
                application_sha256: image.sha256.to_ascii_lowercase(),
                commit: image.commit.clone(),
                dirty: image.dirty,
                origin: image.origin.clone(),
            },
        )
    }

    /// Journal a change of the board with MAC `device` (the device under
    /// test when unknown), attributed to the current owner. A flash is
    /// journaled by [`Self::record_flash`] only.
    pub fn record_board(&self, device: Option<String>, kind: BoardEventKind) -> crate::Result<()> {
        // A journal entry is not a lease: it names an unregistered owner as such.
        let owner =
            crate::grant::owner_from_environment().unwrap_or_else(|_| String::from("unregistered"));
        self.record_board_by(owner, device, kind)
    }

    /// [`Self::record_board`] attributed to `owner`.
    pub fn record_board_by(
        &self,
        owner: String,
        device: Option<String>,
        kind: BoardEventKind,
    ) -> crate::Result<()> {
        if matches!(kind, BoardEventKind::Flashed { .. }) {
            return Err("a flash is journaled by the flash operation (record_flash) only".into());
        }
        self.append(owner, device, kind)
    }

    fn append(
        &self,
        owner: String,
        device: Option<String>,
        kind: BoardEventKind,
    ) -> crate::Result<()> {
        let event = BoardEvent {
            unix: oer_durable::unix_seconds(),
            owner,
            checkout: std::env::current_dir()
                .ok()
                .map(|directory| directory.display().to_string()),
            device,
            kind,
        };
        let path = self.board_path();
        self.locked(|| {
            history::append_line(&path, &event)?;
            if fs::metadata(&path)?.len() > 1 << 20 {
                history::retain_newest::<BoardEvent>(&path)?;
            }
            Ok(())
        })
    }
}

/// Whether `latest` is a flash of `name` with `sha256`.
fn carried(latest: Option<&BoardEvent>, name: &str, sha256: &str) -> bool {
    matches!(
        latest.map(|event| &event.kind),
        Some(BoardEventKind::Flashed { image, application_sha256, .. })
            if image == name && application_sha256.eq_ignore_ascii_case(sha256)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_flat_and_render_their_origin() {
        let event = BoardEvent {
            unix: 5,
            owner: "wifi".into(),
            checkout: Some("/src/wifi".into()),
            device: Some("30:ED:A0:F3:F6:D0".into()),
            kind: BoardEventKind::Flashed {
                image: "correctness".into(),
                application_sha256: "0123456789abcdef".into(),
                commit: Some("fedcba9876543210".into()),
                dirty: Some(true),
                origin: "run r1".into(),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "flashed");
        assert_eq!(serde_json::from_value::<BoardEvent>(json).unwrap(), event);
        assert_eq!(
            event.to_string(),
            "wifi (/src/wifi) flashed correctness fedcba987654+dirty app 0123456789ab (run r1)"
        );
        let artifact = BoardEvent {
            kind: BoardEventKind::StartupArtifactWritten {
                path: "/a".into(),
                sha256: "aa".into(),
                disposition: "Created".into(),
            },
            ..event.clone()
        };
        let peer = BoardEvent {
            device: Some("38:44:BE:AA:25:64".into()),
            unix: 6,
            ..event.clone()
        };
        let newer = BoardEvent {
            unix: 7,
            ..event.clone()
        };
        let other_checkout = BoardEvent {
            kind: BoardEventKind::StartupArtifactUploaded {
                path: "/b".into(),
                sha256: "bb".into(),
            },
            ..event.clone()
        };
        let events = [
            event.clone(),
            artifact.clone(),
            peer.clone(),
            other_checkout.clone(),
            newer.clone(),
        ];
        let (flashes, artifacts) = latest(&events);
        assert_eq!(flashes, [&newer, &peer]);
        assert_eq!(artifacts, [&artifact, &other_checkout]);
        let stand = oer_hil_stand_model::StandFile::parse(
            "schema = 1\n[stand]\nid = \"t\"\nair = \"exclusive\"\n\
             [[hub]]\nid = \"h\"\nusb2 = \"1-1\"\n\
             [[board]]\nid = \"c5-a\"\nusb-serial = \"38:44:BE:AA:25:64\"\nchip = \"esp32c5\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 1 }\nreset = [\"jtag\"]\n",
        )
        .unwrap();
        assert_eq!(
            device_label(Some("38:44:BE:AA:25:64"), Some(&stand)),
            "c5-a (esp32c5)"
        );
        assert_eq!(device_label(Some("AA"), Some(&stand)), "AA");
        assert_eq!(device_label(None, Some(&stand)), "unidentified board");
    }

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    #[test]
    fn a_flash_is_journaled_by_record_flash_alone_and_read_back_by_carries() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        let mac = "38:44:BE:AA:25:64";
        let image = ImageIdentity {
            name: "ieee802154-peer".into(),
            sha256: sha('a'),
            commit: Some("c0ffee".into()),
            dirty: Some(false),
            origin: "test".into(),
        };
        assert!(!arbiter.carries(mac, "ieee802154-peer", &sha('a')).unwrap());
        arbiter
            .record_flash("peer".into(), "38:44:be:aa:25:64", &image)
            .unwrap();
        assert!(arbiter.carries(mac, "ieee802154-peer", &sha('a')).unwrap());
        assert!(
            !arbiter.carries(mac, "ieee802154-peer", &sha('b')).unwrap(),
            "a stale build"
        );
        assert!(!arbiter.carries(mac, "openthread-peer", &sha('a')).unwrap());
        assert!(
            !arbiter
                .carries("AA:AA:AA:AA:AA:AA", "ieee802154-peer", &sha('a'))
                .unwrap()
        );
        // Every other writer is refused a flash.
        let flashed = BoardEventKind::Flashed {
            image: "x".into(),
            application_sha256: sha('c'),
            commit: None,
            dirty: None,
            origin: String::new(),
        };
        let error = arbiter
            .record_board_by("x".into(), Some(mac.into()), flashed)
            .unwrap_err()
            .to_string();
        assert!(error.contains("record_flash"), "{error}");
        // A digest that is none is refused.
        let mut bad = image.clone();
        bad.sha256 = "zz".into();
        assert!(arbiter.record_flash("peer".into(), mac, &bad).is_err());
        let latest = arbiter.latest_flash(mac).unwrap().unwrap();
        assert_eq!(latest.owner, "peer");
        assert_eq!(latest.device.as_deref(), Some(mac));
    }
}
