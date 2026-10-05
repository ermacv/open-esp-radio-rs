//! The pinned vendor artifacts of a chip: its `artifacts.toml`, the
//! host-wide store and fetching into it.
//!
//! The chip's tracked `artifacts.toml` is the only pin. Git artifacts come
//! from the upstream repository at the pinned revision, release members from
//! the pinned release asset; every file is verified against its SHA-256
//! before it enters `target/vendor/<source>/<revision>/<path>`. Local build
//! outputs are only verified. Downloads use `curl` and release members `tar`.
//!
//! Pinned artifacts are immutable and verified, so every checkout of the host
//! shares one store ([`store`]): each checkout's `target/vendor` is a link
//! to it, and a new checkout or worktree finds everything another one fetched.
//! A checkout's former `target/vendor` directory is merged into the store.
//!
//! `cargo xtask vendor-fetch` and the vendor checks read the pins here, and
//! so does the HIL stand's pinned ESP-IDF build (`oer-hil-cli`).
//! [`project::Project`] names the fixed files of a chip's verification
//! project (its shards, provenance registry and scenarios).
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub mod project;
use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Directory of every chip's verification project, relative to the root.
const VERIFICATION: &str = "verification";

/// Cache of fetched artifacts, relative to the repository root; a link to
/// the host-wide store.
pub const CACHE: &str = "target/vendor";
/// Overrides the host-wide store of fetched artifacts.
pub const STORE_ENV: &str = "OER_VENDOR_CACHE";

/// The host-wide store of fetched artifacts, which every checkout, source
/// snapshot and image build shares: `$OER_VENDOR_CACHE`, else
/// `open-esp-radio/vendor` in the user's cache directory.
pub fn store() -> Result<PathBuf> {
    oer_durable::xdg::overridable(STORE_ENV, oer_durable::xdg::Base::Cache, "vendor")
}

/// Makes `root`'s `target/vendor` a link to `store`, merging a former
/// directory's artifacts into the store first.
pub fn link_store(root: &Path, store: &Path) -> Result<()> {
    std::fs::create_dir_all(store)?;
    let link = root.join(CACHE);
    match std::fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if std::fs::read_link(&link)? == store {
                return Ok(());
            }
            std::fs::remove_file(&link)?;
        }
        Ok(metadata) if metadata.is_dir() => {
            merge_into(&link, store)?;
            std::fs::remove_dir_all(&link)?;
        }
        Ok(_) => std::fs::remove_file(&link)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(store, &link)?;
    Ok(())
}

/// Moves every entry of `from` that `into` lacks; artifacts are keyed by
/// source and revision and verified on use, so an existing entry wins.
fn merge_into(from: &Path, into: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = into.join(entry.file_name());
        if entry.file_type()?.is_dir() && target.is_dir() {
            merge_into(&entry.path(), &target)?;
        } else if std::fs::symlink_metadata(&target).is_err() {
            std::fs::rename(entry.path(), &target)?;
        }
    }
    Ok(())
}

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

/// The verified file of the fetched artifact `id` of `chip` in the
/// host-wide [`store`]: an error naming `cargo xtask vendor-fetch` when it is
/// missing, and naming the pin when it differs.
pub fn fetched(root: &Path, chip: &str, id: &str) -> Result<PathBuf> {
    let manifest = Manifest::load(root, chip)?;
    let artifact = manifest.artifact(id)?;
    let source = manifest.source_of(artifact);
    if source.kind == SourceKind::Local {
        return Err(format!("`{id}` is a local build, not a fetched artifact").into());
    }
    let path = cache_directory(&store()?, source)?.join(&artifact.path);
    if !path.is_file() {
        return Err(format!(
            "the pinned `{id}` artifact {} is missing: run `cargo xtask vendor-fetch {chip} --artifact {id}`",
            path.display()
        )
        .into());
    }
    if !verified(&path, &artifact.sha256)? {
        return Err(format!(
            "{} differs from its pin {}",
            path.display(),
            artifact.sha256
        )
        .into());
    }
    Ok(path)
}

/// Whether `path` is a file whose SHA-256 is `expected`.
pub fn verified(path: &Path, expected: &str) -> Result<bool> {
    Ok(path.is_file() && oer_durable::sha256_file(path)? == expected)
}

/// A local build's state against its pin.
#[derive(Debug, PartialEq, Eq)]
enum LocalBuild {
    Pinned,
    /// Not built on this host: optional, not an error.
    Absent,
    Differs,
}

fn local_build(path: &Path, expected: &str) -> Result<LocalBuild> {
    Ok(if !path.is_file() {
        LocalBuild::Absent
    } else if verified(path, expected)? {
        LocalBuild::Pinned
    } else {
        LocalBuild::Differs
    })
}

/// `https://github.com/<owner>/<repo>` as `<owner>/<repo>`.
fn github(repository: &str) -> Result<&str> {
    repository
        .strip_prefix("https://github.com/")
        .map(|r| r.trim_end_matches('/'))
        .ok_or_else(|| format!("unsupported repository {repository}").into())
}

fn download(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = destination.with_extension("partial");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(url)
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("download failed: {url}").into());
    }
    std::fs::rename(&partial, destination)?;
    Ok(())
}

/// `<base>/<source>/<revision>`, where `base` is a store or a checkout's
/// `target/vendor`.
fn cache_directory(base: &Path, source: &Source) -> Result<PathBuf> {
    let revision = source
        .revision
        .as_deref()
        .ok_or_else(|| format!("{} lacks `revision`", source.id))?;
    Ok(base.join(&source.id).join(revision))
}

fn fetch(root: &Path, source: &Source, artifact: &Artifact) -> Result<PathBuf> {
    let directory = cache_directory(&root.join(CACHE), source)?;
    let destination = directory.join(&artifact.path);
    if verified(&destination, &artifact.sha256)? {
        return Ok(destination);
    }
    let repository = source
        .repository
        .as_deref()
        .ok_or_else(|| format!("{} lacks `repository`", source.id))?;
    let revision = source.revision.as_deref().unwrap_or_default();
    match source.kind {
        SourceKind::Git => download(
            &format!(
                "https://raw.githubusercontent.com/{}/{revision}/{}",
                github(repository)?,
                artifact.path
            ),
            &destination,
        )?,
        SourceKind::Release => {
            let asset = source
                .asset
                .as_deref()
                .ok_or_else(|| format!("{} lacks `asset`", source.id))?;
            let tarball = directory.join(asset);
            let expected = source
                .sha256
                .as_deref()
                .ok_or_else(|| format!("{} lacks `sha256`", source.id))?;
            if !verified(&tarball, expected)? {
                download(
                    &format!("{repository}/releases/download/{revision}/{asset}"),
                    &tarball,
                )?;
                let actual = oer_durable::sha256_file(&tarball)?;
                if actual != expected {
                    return Err(format!("{asset}: sha256 {actual}, pinned {expected}").into());
                }
            }
            let status = Command::new("tar")
                .arg("-xzf")
                .arg(&tarball)
                .arg("-C")
                .arg(&directory)
                .arg(&artifact.path)
                .status()?;
            if !status.success() {
                return Err(format!("{asset} lacks member {}", artifact.path).into());
            }
        }
        SourceKind::Local => unreachable!("local artifacts are not fetched"),
    }
    let actual = oer_durable::sha256_file(&destination)?;
    if actual != artifact.sha256 {
        return Err(format!(
            "{}: fetched sha256 {actual}, pinned {}",
            artifact.id, artifact.sha256
        )
        .into());
    }
    Ok(destination)
}

/// One pinned artifact at its cache or local-build path.
#[derive(Debug)]
pub struct Pinned {
    pub id: String,
    /// The artifact's source id.
    pub source: String,
    pub path: PathBuf,
    /// Built locally rather than fetched from a vendor source.
    pub local: bool,
}

/// Every pinned artifact of `chip` at its verified path; fails when one is
/// missing or differs, naming `cargo xtask vendor-fetch` for fetched ones.
/// Local builds are skipped when absent: they are not vendor sources.
pub fn pinned(root: &Path, chip: &str) -> Result<Vec<Pinned>> {
    let resolved = resolve(root, chip)?;
    match resolved.unfetched.first() {
        None => Ok(resolved.pinned),
        Some(id) => Err(format!(
            "{id} is not fetched as pinned; run `cargo xtask vendor-fetch {chip}`"
        )
        .into()),
    }
}

/// Fetches every pinned vendor artifact of `chip` into the shared store,
/// leaving local builds alone; fails when one cannot be fetched as pinned.
pub fn fetch_vendor_sources(root: &Path, chip: &str) -> Result<()> {
    link_store(root, &store()?)?;
    let manifest = Manifest::load(root, chip)?;
    let mut failures = vec![];
    for artifact in &manifest.artifact {
        let source = manifest.source_of(artifact);
        if source.kind == SourceKind::Local {
            continue;
        }
        if let Err(error) = fetch(root, source, artifact) {
            failures.push(format!("{}: {error}", artifact.id));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "vendor artifacts of {chip} could not be fetched: {}",
            failures.join("; ")
        )
        .into())
    }
}

/// The fetched vendor artifacts of `chip` that are missing or differ from
/// their pin; empty when every citation can be checked.
pub fn unfetched(root: &Path, chip: &str) -> Result<Vec<String>> {
    Ok(resolve(root, chip)?.unfetched)
}

struct Resolved {
    pinned: Vec<Pinned>,
    unfetched: Vec<String>,
}

fn resolve(root: &Path, chip: &str) -> Result<Resolved> {
    resolve_manifest(root, &Manifest::load(root, chip)?)
}

fn resolve_manifest(root: &Path, manifest: &Manifest) -> Result<Resolved> {
    let mut resolved = Resolved {
        pinned: vec![],
        unfetched: vec![],
    };
    for artifact in manifest.artifact.iter().cloned() {
        let local = manifest.source_of(&artifact).kind == SourceKind::Local;
        let path = manifest.location(root, &artifact)?;
        if !verified(&path, &artifact.sha256)? {
            if !local {
                resolved.unfetched.push(artifact.id);
            }
            continue;
        }
        resolved.pinned.push(Pinned {
            id: artifact.id,
            source: artifact.source,
            path,
            local,
        });
    }
    Ok(resolved)
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

/// Fetch and verify every artifact of `chip`; report local builds that are
/// missing or differ. Fails when any artifact is not available as pinned.
pub fn run(root: &Path, chip: &str, only: &[String]) -> Result<()> {
    link_store(root, &store()?)?;
    let manifest = Manifest::load(root, chip)?;
    let artifacts = selected(manifest.artifact.clone(), only)?;
    let mut failures = vec![];
    for artifact in &artifacts {
        let source = manifest.source_of(artifact);
        let result = if source.kind == SourceKind::Local {
            let path = root.join(&artifact.path);
            match local_build(&path, &artifact.sha256)? {
                LocalBuild::Pinned => Ok(path),
                // Local builds are optional: a host without one skips it.
                LocalBuild::Absent => {
                    println!(
                        "{:<16} skipped: local build {} absent",
                        artifact.id,
                        path.display()
                    );
                    continue;
                }
                LocalBuild::Differs => {
                    Err(format!("local build {} differs from the pin", path.display()).into())
                }
            }
        } else {
            fetch(root, source, artifact)
        };
        match result {
            Ok(path) => println!("{:<16} {}", artifact.id, path.display()),
            Err(error) => {
                println!("{:<16} ERROR {error}", artifact.id);
                failures.push(artifact.id.clone());
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("unavailable artifacts: {}", failures.join(", ")).into())
    }
}

/// The artifacts `only` names, or all when it names none; an id the
/// manifest lacks is an error.
fn selected(artifacts: Vec<Artifact>, only: &[String]) -> Result<Vec<Artifact>> {
    if let Some(unknown) = only
        .iter()
        .find(|id| !artifacts.iter().any(|artifact| &artifact.id == *id))
    {
        return Err(format!("no artifact `{unknown}` in artifacts.toml").into());
    }
    Ok(artifacts
        .into_iter()
        .filter(|artifact| only.is_empty() || only.contains(&artifact.id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_local_build_is_optional_and_a_changed_one_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sdk.elf");
        let pin = "0".repeat(64);
        assert_eq!(local_build(&path, &pin).unwrap(), LocalBuild::Absent);
        std::fs::write(&path, b"built").unwrap();
        assert_eq!(local_build(&path, &pin).unwrap(), LocalBuild::Differs);
        let actual = oer_durable::sha256_file(&path).unwrap();
        assert_eq!(local_build(&path, &actual).unwrap(), LocalBuild::Pinned);
    }

    #[test]
    fn an_artifact_selection_keeps_only_the_named_artifacts() {
        let artifact = |id: &str| Artifact {
            id: id.to_owned(),
            source: "s".to_owned(),
            path: "p".to_owned(),
            sha256: "h".to_owned(),
        };
        let all = || vec![artifact("rom"), artifact("phy")];
        assert_eq!(selected(all(), &[]).unwrap().len(), 2);
        let rom = selected(all(), &["rom".to_owned()]).unwrap();
        assert_eq!(
            rom.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["rom"]
        );
        assert!(selected(all(), &["missing".to_owned()]).is_err());
    }

    #[test]
    fn tracked_manifests_parse_with_complete_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for chip in ["esp32s31", "esp32c5"] {
            let manifest = Manifest::load(&root, chip).unwrap();
            assert!(manifest.artifact("libphy").is_ok(), "{chip}");
            for source in &manifest.source {
                if source.kind != SourceKind::Local {
                    github(source.repository.as_deref().unwrap()).unwrap();
                    assert!(source.revision.is_some());
                }
            }
        }
    }

    #[test]
    fn the_rom_pin_resolves_into_the_vendor_store() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        match fetched(&root, "esp32s31", "rom") {
            Ok(path) => assert!(path.ends_with("esp32s31_rev0_rom.elf")),
            // A host without the vendor store names the fetch command.
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("vendor-fetch esp32s31 --artifact rom"),
                "{error}"
            ),
        }
        let error = fetched(&root, "esp32s31", "absent").unwrap_err();
        assert!(error.to_string().contains("pins no `absent` artifact"));
    }

    #[test]
    fn artifacts_must_name_declared_sources() {
        let error = Manifest::parse(
            "schema = 1\nsource = []\n[[artifact]]\nid = \"a\"\nsource = \"b\"\npath = \"c\"\nsha256 = \"d\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown source"));
        assert!(Manifest::parse("schema = 2\nsource = []\nartifact = []\n").is_err());
        assert!(Manifest::parse("schema = 1\nsource = []\nartifact = []\nextra = 1\n").is_err());
    }

    #[test]
    fn unfetched_names_missing_vendor_artifacts_in_the_source_keyed_cache() {
        let root = tempfile::tempdir().unwrap();
        let cached = root.path().join(CACHE).join("vendor/0123/lib.a");
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"pinned").unwrap();
        let manifest = format!(
            "schema = 1\n\
             [[source]]\nid = \"vendor\"\nkind = \"git\"\n\
             repository = \"https://github.com/o/r\"\nrevision = \"0123\"\n\
             [[source]]\nid = \"build\"\nkind = \"local\"\n\
             [[artifact]]\nid = \"fetched\"\nsource = \"vendor\"\npath = \"lib.a\"\nsha256 = \"{}\"\n\
             [[artifact]]\nid = \"missing\"\nsource = \"vendor\"\npath = \"other.a\"\nsha256 = \"00\"\n\
             [[artifact]]\nid = \"unbuilt\"\nsource = \"build\"\npath = \"out.elf\"\nsha256 = \"00\"\n",
            oer_durable::sha256_file(&cached).unwrap()
        );
        let manifest = Manifest::parse(&manifest).unwrap();
        let unbuilt = manifest.artifact("unbuilt").unwrap();
        assert_eq!(
            manifest.location(root.path(), unbuilt).unwrap(),
            root.path().join("out.elf")
        );
        let resolved = resolve_manifest(root.path(), &manifest).unwrap();
        assert_eq!(resolved.unfetched, ["missing"]);
        let pinned: Vec<_> = resolved.pinned.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pinned, ["fetched"]);
    }
    #[test]
    fn a_checkout_links_the_shared_store_and_merges_its_former_cache() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let store = dir.path().join("store");
        std::fs::create_dir_all(checkout.join(CACHE).join("idf/abc")).unwrap();
        std::fs::write(checkout.join(CACHE).join("idf/abc/lib.a"), b"lib").unwrap();
        std::fs::create_dir_all(store.join("idf/abc")).unwrap();
        std::fs::write(store.join("idf/abc/rom.elf"), b"rom").unwrap();
        link_store(&checkout, &store).unwrap();
        assert_eq!(std::fs::read_link(checkout.join(CACHE)).unwrap(), store);
        assert_eq!(
            std::fs::read(checkout.join(CACHE).join("idf/abc/lib.a")).unwrap(),
            b"lib"
        );
        assert_eq!(
            std::fs::read(checkout.join(CACHE).join("idf/abc/rom.elf")).unwrap(),
            b"rom"
        );
        // A second checkout finds what the first fetched, and relinking is idempotent.
        let other = dir.path().join("other");
        link_store(&other, &store).unwrap();
        link_store(&other, &store).unwrap();
        assert_eq!(
            std::fs::read(other.join(CACHE).join("idf/abc/lib.a")).unwrap(),
            b"lib"
        );
    }
}
