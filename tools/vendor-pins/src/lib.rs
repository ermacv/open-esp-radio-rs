//! The pins of a chip's vendor artifacts: the one reader of its tracked
//! `verification/<chip>/artifacts.toml` (sources, artifacts, their SHA-256)
//! and where a pinned artifact lies in a checkout. Fetching and the
//! host-wide store are `oer-vendor-artifacts`.
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Directory of every chip's verification project, relative to the root.
const VERIFICATION: &str = "verification";

/// Cache of fetched artifacts, relative to the repository root; a link to
/// the host-wide store.
pub const CACHE: &str = "target/vendor";

/// The pin of `chip`'s vendor artifacts, relative to the repository root,
/// whether or not the chip has one.
pub fn manifest(chip: &str) -> String {
    format!("{VERIFICATION}/{chip}/artifacts.toml")
}

/// Tracked manifest of `chip`, relative to the repository root: an error
/// that lists the supported chips for an unsupported one, or names the
/// missing manifest of a chip without a vendor verification project.
pub fn manifest_path(root: &Path, chip: &str) -> Result<String> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let manifest = manifest(&profile.id);
    if !root.join(&manifest).is_file() {
        return Err(format!("chip {chip} has no vendor verification project ({manifest})").into());
    }
    Ok(manifest)
}

/// The kind of a pinned source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// A file at `path` of a git revision.
    Git,
    /// A member `path` of a release asset tarball.
    Release,
    /// A local build output at `path` below the repository root.
    Local,
}

/// One pinned source of `artifacts.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: SourceKind,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub asset: Option<String>,
    pub sha256: Option<String>,
}

/// One pinned artifact of `artifacts.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    /// The id of its source.
    pub source: String,
    /// Its path in the source: in the git tree, the release tarball or,
    /// for a local build, below the repository root.
    pub path: String,
    pub sha256: String,
}

/// A chip's `artifacts.toml`: the one reader of the pins.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub source: Vec<Source>,
    pub artifact: Vec<Artifact>,
}

impl Manifest {
    /// Parses a manifest and checks that every artifact names a declared
    /// source.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        if manifest.schema != 1 {
            return Err(format!("unsupported artifact schema {}", manifest.schema).into());
        }
        for artifact in &manifest.artifact {
            if !manifest.source.iter().any(|s| s.id == artifact.source) {
                return Err(format!("{}: unknown source {}", artifact.id, artifact.source).into());
            }
        }
        Ok(manifest)
    }

    /// The tracked manifest of `chip` in the repository at `root`.
    pub fn load(root: &Path, chip: &str) -> Result<Self> {
        let manifest = manifest_path(root, chip)?;
        Self::parse(&std::fs::read_to_string(root.join(&manifest))?)
            .map_err(|error| format!("{manifest}: {error}").into())
    }

    /// The artifact `id`.
    pub fn artifact(&self, id: &str) -> Result<&Artifact> {
        self.artifact
            .iter()
            .find(|artifact| artifact.id == id)
            .ok_or_else(|| format!("artifacts.toml pins no `{id}` artifact").into())
    }

    /// The source `id`.
    pub fn source(&self, id: &str) -> Result<&Source> {
        self.source
            .iter()
            .find(|source| source.id == id)
            .ok_or_else(|| format!("artifacts.toml pins no `{id}` source").into())
    }

    /// Where the fetched `source` lies for the checkout at `root`:
    /// `<root>/target/vendor/<source>/<revision>`.
    pub fn source_directory(&self, root: &Path, source: &Source) -> Result<PathBuf> {
        cache_directory(&root.join(CACHE), source)
    }

    /// The source `artifact` names; [`Manifest::parse`] checked it exists.
    pub fn source_of(&self, artifact: &Artifact) -> &Source {
        self.source
            .iter()
            .find(|source| source.id == artifact.source)
            .expect("parsed manifests name declared sources")
    }

    /// Where `artifact` lies for the checkout at `root`: its build output
    /// for a local source, else `<root>/target/vendor/<source>/<revision>/<path>`.
    pub fn location(&self, root: &Path, artifact: &Artifact) -> Result<PathBuf> {
        let source = self.source_of(artifact);
        Ok(match source.kind {
            SourceKind::Local => root.join(&artifact.path),
            SourceKind::Git | SourceKind::Release => {
                self.source_directory(root, source)?.join(&artifact.path)
            }
        })
    }
}

/// `<base>/<source>/<revision>`, where `base` is a store or a checkout's
/// `target/vendor`.
pub fn cache_directory(base: &Path, source: &Source) -> Result<PathBuf> {
    let revision = source
        .revision
        .as_deref()
        .ok_or_else(|| format!("{} lacks `revision`", source.id))?;
    Ok(base.join(&source.id).join(revision))
}

/// A pinned git source of `chip` with its artifacts' paths and SHA-256.
#[derive(Debug)]
pub struct GitPin {
    pub id: String,
    pub repository: String,
    pub revision: String,
    /// `(path, sha256)` of each artifact, relative to the checkout.
    pub artifacts: Vec<(String, String)>,
}

/// Every pinned git source of `chip`.
pub fn git_pins(root: &Path, chip: &str) -> Result<Vec<GitPin>> {
    let Manifest {
        source, artifact, ..
    } = Manifest::load(root, chip)?;
    let artifacts = artifact;
    source
        .into_iter()
        .filter(|s| s.kind == SourceKind::Git)
        .map(|source| {
            Ok(GitPin {
                artifacts: artifacts
                    .iter()
                    .filter(|a| a.source == source.id)
                    .map(|a| (a.path.clone(), a.sha256.clone()))
                    .collect(),
                repository: source
                    .repository
                    .ok_or_else(|| format!("{} lacks `repository`", source.id))?,
                revision: source
                    .revision
                    .ok_or_else(|| format!("{} lacks `revision`", source.id))?,
                id: source.id,
            })
        })
        .collect()
}
