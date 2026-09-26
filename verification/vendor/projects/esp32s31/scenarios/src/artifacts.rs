//! The pinned ESP32-S31 vendor artifacts of `artifacts.toml`.
//!
//! Every scenario input is authenticated against this manifest, and each
//! fetchable artifact has one cache location that `cargo xtask vendor-fetch`
//! fills. The manifest is the only place a pin changes.
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The tracked manifest, relative to the repository root.
pub const MANIFEST: &str = "verification/vendor/projects/esp32s31/artifacts.toml";
/// Cache of fetched artifacts, relative to the repository root.
pub const CACHE: &str = "target/vendor";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub source: Vec<Source>,
    pub artifact: Vec<Artifact>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// A file at `path` of a git revision.
    Git,
    /// A member `path` of a release asset tarball.
    Release,
    /// A local build output at `path` below the repository root.
    Local,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: SourceKind,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub asset: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub source: String,
    pub path: String,
    pub sha256: String,
}

/// Parse a manifest and check that every artifact names a declared source.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
    if manifest.schema != 1 {
        return Err(format!("unsupported artifact schema {}", manifest.schema));
    }
    for artifact in &manifest.artifact {
        if !manifest.source.iter().any(|s| s.id == artifact.source) {
            return Err(format!(
                "{}: unknown source {}",
                artifact.id, artifact.source
            ));
        }
    }
    Ok(manifest)
}

/// The manifest compiled into this binary.
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        parse(include_str!("../../artifacts.toml")).expect("tracked artifact manifest")
    })
}

fn artifact(id: &str) -> &'static Artifact {
    manifest()
        .artifact
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("artifact {id} is not pinned"))
}

/// Pinned SHA-256 of artifact `id`.
pub fn sha256(id: &str) -> &'static str {
    &artifact(id).sha256
}

/// Location of artifact `id` below the repository `root`: the fetch cache
/// for downloaded artifacts, the build output for local ones.
pub fn path(root: &Path, id: &str) -> PathBuf {
    let artifact = artifact(id);
    let source = manifest()
        .source
        .iter()
        .find(|s| s.id == artifact.source)
        .expect("parsed sources");
    match source.kind {
        SourceKind::Local => root.join(&artifact.path),
        SourceKind::Git | SourceKind::Release => root
            .join(CACHE)
            .join(&source.id)
            .join(source.revision.as_deref().unwrap_or_default())
            .join(&artifact.path),
    }
}

/// Pinned location of `id` in this checkout: the default of every scenario
/// input argument.
pub fn default_path(id: &str) -> PathBuf {
    path(
        &crate::observation::root().expect("scenario package inside the repository"),
        id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_manifest_pins_every_scenario_input_once() {
        let manifest = manifest();
        for id in ["libphy", "librftest", "libpp", "rom", "sdk", "phy-sdk"] {
            assert_eq!(sha256(id).len(), 64, "{id}");
        }
        let mut ids: Vec<_> = manifest.artifact.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), manifest.artifact.len());
        for source in &manifest.source {
            if source.kind != SourceKind::Local {
                assert!(
                    source.repository.is_some() && source.revision.is_some(),
                    "{}",
                    source.id
                );
            }
            if source.kind == SourceKind::Release {
                assert!(
                    source.asset.is_some() && source.sha256.is_some(),
                    "{}",
                    source.id
                );
            }
        }
    }

    #[test]
    fn fetched_artifacts_live_under_their_source_revision() {
        let root = Path::new("/repo");
        assert_eq!(
            path(root, "libphy"),
            root.join("target/vendor/esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0/esp32s31/libphy.a")
        );
        assert!(path(root, "sdk").starts_with(root.join("target/architecture-research")));
    }

    #[test]
    fn unknown_sources_are_rejected() {
        let error = parse(
            "schema = 1\nsource = []\n[[artifact]]\nid = \"a\"\nsource = \"b\"\npath = \"c\"\nsha256 = \"d\"\n",
        )
        .unwrap_err();
        assert!(error.contains("unknown source"));
    }
}
