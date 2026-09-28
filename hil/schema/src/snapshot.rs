//! The source-snapshot manifest: which files a run's firmware was built from.
//!
//! The runner writes `manifest.json` beside the snapshot's `sources.tar`;
//! the qualification evaluator reads it back and recomputes each source's
//! identity from its serialized form. Both use these types, so the field
//! order and the fields themselves are one contract. Unknown fields are
//! refused: a field this build does not know would change an identity it
//! cannot recompute.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The manifest schema this build writes and reads.
pub const MANIFEST_SCHEMA: u16 = 1;

/// Every source of one snapshot: the repository first, then each local
/// dependency override.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u16,
    pub sources: Vec<SourceInput>,
}

/// One checkout's archived files. Its identity is the SHA-256 of its JSON
/// serialization in this field order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub name: String,
    pub commit: String,
    pub dirty: bool,
    pub files: Vec<FileInput>,
    /// The untracked files among `files`, and why each was archived.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untracked: Vec<UntrackedInput>,
}

/// One archived file, by its path relative to its checkout.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileInput {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    /// `0o644` or `0o755`.
    pub mode: u32,
}

/// An untracked file a snapshot archived.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UntrackedInput {
    pub path: PathBuf,
    pub by: UntrackedReason,
}

/// Why a snapshot archived an untracked file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UntrackedReason {
    /// Named with `--source-include`.
    SourceInclude,
    /// Inside an image package, with `--include-untracked`.
    ImagePackage,
    /// Inside the HIL host packages or scenarios, with `--include-untracked`.
    HilHost,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(untracked: Vec<UntrackedInput>) -> SourceInput {
        SourceInput {
            name: String::from("repository"),
            commit: String::from("abc"),
            dirty: true,
            files: vec![FileInput {
                path: PathBuf::from("new.rs"),
                size_bytes: 3,
                sha256: String::from("00"),
                mode: 0o644,
            }],
            untracked,
        }
    }

    #[test]
    fn a_source_without_untracked_files_keeps_its_earlier_serialization() {
        let json = serde_json::to_string(&source(Vec::new())).unwrap();
        assert!(!json.contains("untracked"));
        let read: SourceInput = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), json);
    }

    #[test]
    fn untracked_files_round_trip_to_the_same_bytes() {
        let written = source(vec![UntrackedInput {
            path: PathBuf::from("new.rs"),
            by: UntrackedReason::ImagePackage,
        }]);
        let json = serde_json::to_string(&written).unwrap();
        assert!(json.contains(r#""by":"image-package""#));
        let read: SourceInput = serde_json::from_str(&json).unwrap();
        assert_eq!(read, written);
        assert_eq!(serde_json::to_string(&read).unwrap(), json);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let mut json = serde_json::to_value(source(Vec::new())).unwrap();
        json["later"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SourceInput>(json).is_err());
    }
}
