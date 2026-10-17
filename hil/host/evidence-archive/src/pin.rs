//! The pin: the one archive version the repository tracks (rule 4). It names the repository and the version and repeats the
//! version's manifest, so rule 2 (every cited run is in the version with the
//! same digest) is checked offline, without access to the archive.

use std::{fmt, fs, path::Path};

use oer_hil_model::{RunRef, Sha256};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{Manifest, Payload, Result, Version, refused};

/// Where the repository tracks its pin, relative to its root.
pub const PIN: &str = "qualification/evidence/archive.toml";

const HEADER: &str = "\
# The HIL evidence archive version the repository tracks (#761 Q7).
# Written by `cargo hil archive publish`. Never edit it by hand.
";

/// A GitHub repository, `owner/name`, that holds the archive's releases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository(String);

impl Repository {
    pub fn parse(text: &str) -> Result<Self> {
        let part = |part: &str| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        match text.split_once('/') {
            Some((owner, name)) if part(owner) && part(name) => Ok(Self(text.to_owned())),
            _ => refused(format!(
                "evidence repository `{text}` is not a GitHub `owner/name`"
            )),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Repository {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Repository {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

/// The pin's file: the manifest's fields beside the names derived from it.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Document {
    repository: Repository,
    version: Version,
    manifest_sha256: Sha256,
    schema: u32,
    archive: Payload,
    runs: Vec<RunRef>,
}

/// A checked pin: its version is the digest of the manifest it repeats.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pin {
    repository: Repository,
    manifest: Manifest,
}

impl Pin {
    pub fn new(repository: Repository, manifest: Manifest) -> Self {
        Self {
            repository,
            manifest,
        }
    }

    /// The pin in `text`; refuses a manifest that does not hash to the
    /// recorded digest or a version not named by it.
    pub fn parse(text: &str) -> Result<Self> {
        let document: Document = toml::from_str(text)
            .map_err(|error| crate::Refused(format!("invalid evidence pin: {error}")))?;
        let manifest = Manifest {
            schema: document.schema,
            archive: document.archive,
            runs: document.runs,
        };
        manifest.validate()?;
        let digest = manifest.digest();
        if digest != document.manifest_sha256 {
            return refused(format!(
                "the evidence pin's manifest hashes to {digest}, not to its manifest-sha256 {}",
                document.manifest_sha256
            ));
        }
        if document.version != manifest.version() {
            return refused(format!(
                "the evidence pin names version {}, not {}, its manifest's",
                document.version,
                manifest.version()
            ));
        }
        Ok(Self::new(document.repository, manifest))
    }

    /// The pin at `path`, or `None` when the repository pins no version.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .map(Some)
                .map_err(|error| crate::within(error, path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{}: {error}", path.display()).into()),
        }
    }

    /// The pin's file text.
    pub fn to_toml(&self) -> String {
        let document = Document {
            repository: self.repository.clone(),
            version: self.manifest.version(),
            manifest_sha256: self.manifest.digest(),
            schema: self.manifest.schema,
            archive: self.manifest.archive,
            runs: self.manifest.runs.clone(),
        };
        format!(
            "{HEADER}\n{}",
            toml::to_string(&document).expect("a pin serializes")
        )
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        oer_durable::atomic_write(path, self.to_toml().as_bytes())?;
        Ok(())
    }

    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn version(&self) -> Version {
        self.manifest.version()
    }
}

/// A cited run that the pinned version does not hold with the cited digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unpinned {
    /// What cites it, such as a file and its entry.
    pub citation: String,
    pub reference: RunRef,
    /// The digest the pinned version holds for the run, when it holds it.
    pub pinned: Option<Sha256>,
    /// The version checked: the pinned one, or a candidate to pin.
    pub version: String,
}

impl fmt::Display for Unpinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.pinned {
            Some(pinned) => write!(
                f,
                "{} cites run {} with SHA-256 {}, but {} holds it with {pinned}",
                self.citation, self.reference.run, self.reference.sha256, self.version
            ),
            None => write!(
                f,
                "{} cites run {}, which {} does not hold",
                self.citation, self.reference.run, self.version
            ),
        }
    }
}

/// Rule 2: the citations, each `(what cites it, reference)`, that `pin`
/// does not hold with the same digest; every citation when nothing is pinned.
pub fn unpinned<'a>(
    pin: Option<&Pin>,
    citations: impl IntoIterator<Item = (String, &'a RunRef)>,
) -> Vec<Unpinned> {
    unpinned_in(
        pin.map(|pin| pin.manifest.runs.as_slice()),
        "the pinned version",
        citations,
    )
}

/// Rule 2 against `held`, the runs a version holds or would hold (`version`
/// names it): the citations it does not hold with the same digest; every
/// citation when there is no version.
pub fn unpinned_in<'a>(
    held: Option<&[RunRef]>,
    version: &str,
    citations: impl IntoIterator<Item = (String, &'a RunRef)>,
) -> Vec<Unpinned> {
    citations
        .into_iter()
        .filter_map(|(citation, reference)| {
            let found = held.and_then(|held| held.iter().find(|held| held.run == reference.run));
            (found != Some(reference)).then(|| Unpinned {
                citation,
                reference: reference.clone(),
                pinned: found.map(|held| held.sha256),
                version: version.to_owned(),
            })
        })
        .collect()
}
