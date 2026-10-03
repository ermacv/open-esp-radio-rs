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

/// A way the stand resets a board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResetPath {
    /// RTS on the chip's own USB Serial/JTAG port.
    Rts,
    /// The CPU reset through the chip's JTAG.
    Jtag,
    /// EN through the board's registered UART bridge.
    En,
    /// A power cycle of the board's registered hub port.
    Power,
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

/// An automatic recovery step of an unreachable board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryStep {
    /// A pulse on RTS of the chip's own USB Serial/JTAG port, for a board
    /// without an EN path.
    RtsReset,
    /// A pulse on EN through the board's registered reset path.
    EnReset,
    /// Its switchable hub port was powered off and on.
    PowerCycle,
    /// Its switchable hub port was off and was powered on.
    PowerOn,
    /// A system reset through the chip's builtin USB-JTAG (OpenOCD `reset
    /// run`), which clears low-power state an RTS reset leaves.
    JtagReset,
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
            control: None,
            unknown: Default::default(),
        }];
        assert_eq!(
            device_label(Some("38:44:BE:AA:25:64"), &devices),
            "38:44:BE:AA:25:64 (esp32c5)"
        );
        assert_eq!(device_label(Some("AA"), &devices), "AA");
        assert_eq!(device_label(None, &devices), "unidentified board");
    }
}
