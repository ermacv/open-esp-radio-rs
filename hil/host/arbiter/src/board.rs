//! Journal of changes to the board's state: flashed firmware and the
//! host-owned startup artifact (which carries the PHY calibration cache).
//! The arbiter only reports these changes; it never restores a state.

use serde::{Deserialize, Serialize};

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
    let mut flashes: Vec<&BoardEvent> = Vec::new();
    let mut artifacts: Vec<&BoardEvent> = Vec::new();
    for event in events {
        let (list, same): (&mut Vec<&BoardEvent>, fn(&BoardEvent, &BoardEvent) -> bool) =
            match &event.kind {
                BoardEventKind::Flashed { .. } => (&mut flashes, |a, b| a.device == b.device),
                BoardEventKind::StartupArtifactUploaded { .. }
                | BoardEventKind::StartupArtifactWritten { .. } => (&mut artifacts, |a, b| {
                    a.artifact_path() == b.artifact_path()
                }),
            };
        match list.iter_mut().find(|known| same(known, event)) {
            Some(known) => *known = event,
            None => list.push(event),
        }
    }
    (flashes, artifacts)
}

impl BoardEvent {
    fn artifact_path(&self) -> Option<&str> {
        match &self.kind {
            BoardEventKind::StartupArtifactUploaded { path, .. }
            | BoardEventKind::StartupArtifactWritten { path, .. } => Some(path),
            BoardEventKind::Flashed { .. } => None,
        }
    }
}

/// The registered label of a board, its MAC, or `unidentified board`.
pub(crate) fn device_label(device: Option<&str>, devices: &[crate::Device]) -> String {
    match device {
        Some(mac) => devices
            .iter()
            .find(|registered| registered.mac == mac)
            .map_or_else(|| mac.to_owned(), crate::Device::label),
        None => String::from("unidentified board"),
    }
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
        let devices = [crate::Device {
            mac: "38:44:BE:AA:25:64".into(),
            chip: Some("esp32c5".into()),
            name: None,
        }];
        assert_eq!(
            device_label(Some("38:44:BE:AA:25:64"), &devices),
            "38:44:BE:AA:25:64 (esp32c5)"
        );
        assert_eq!(device_label(Some("AA"), &devices), "AA");
        assert_eq!(device_label(None, &devices), "unidentified board");
    }
}
