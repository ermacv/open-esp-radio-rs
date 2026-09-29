//! Fetch the pinned vendor artifacts of a chip into the target cache.
//!
//! The chip's tracked `artifacts.toml` is the only pin. Git artifacts come
//! from the upstream repository at the pinned revision, release members from
//! the pinned release asset; every file is verified against its SHA-256
//! before it enters `target/vendor/<source>/<revision>/<path>`. Local build
//! outputs are only verified. Downloads use `curl` and release members `tar`.
//!
//! Pinned artifacts are immutable and verified, so every checkout of the host
//! shares one store (`$OER_VENDOR_CACHE`, default
//! `~/.cache/open-esp-radio/vendor`): each checkout's `target/vendor` is a link
//! to it, and a new checkout or worktree finds everything another one fetched.
//! A checkout's former `target/vendor` directory is merged into the store.
use crate::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Cache of fetched artifacts, relative to the repository root; a link to
/// the host-wide store.
pub const CACHE: &str = "target/vendor";
/// Overrides the host-wide store of fetched artifacts.
pub const STORE_ENV: &str = "OER_VENDOR_CACHE";

/// The host-wide store of fetched artifacts.
pub fn store() -> Result<PathBuf> {
    match std::env::var_os(STORE_ENV).filter(|value| !value.is_empty()) {
        Some(store) => Ok(PathBuf::from(store)),
        None => Ok(std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or("HOME is required to locate the vendor artifact store")?
            .join("open-esp-radio/vendor")),
    }
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

/// Tracked manifest of `chip`, relative to the repository root.
pub fn manifest_path(root: &Path, chip: &str) -> Result<String> {
    Ok(crate::chips::Chip::new(root, chip)?.artifacts())
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Git,
    Release,
    Local,
}

#[derive(Debug)]
struct Source {
    id: String,
    kind: Kind,
    repository: Option<String>,
    revision: Option<String>,
    asset: Option<String>,
    sha256: Option<String>,
}

#[derive(Debug)]
struct Artifact {
    id: String,
    source: String,
    path: String,
    sha256: String,
}

fn string(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

fn required(table: &toml::Table, key: &str, what: &str) -> Result<String> {
    string(table, key).ok_or_else(|| format!("{what} lacks `{key}`").into())
}

fn parse(text: &str) -> Result<(Vec<Source>, Vec<Artifact>)> {
    let table: toml::Table = toml::from_str(text)?;
    if table.get("schema").and_then(|v| v.as_integer()) != Some(1) {
        return Err("unsupported artifact manifest schema".into());
    }
    let entries = |key: &str| -> Result<Vec<toml::Table>> {
        table
            .get(key)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("manifest lacks `{key}`"))?
            .iter()
            .map(|v| {
                v.as_table()
                    .cloned()
                    .ok_or_else(|| format!("`{key}` entry is not a table").into())
            })
            .collect()
    };
    let mut sources = vec![];
    for entry in entries("source")? {
        let id = required(&entry, "id", "source")?;
        let kind = match required(&entry, "kind", &id)?.as_str() {
            "git" => Kind::Git,
            "release" => Kind::Release,
            "local" => Kind::Local,
            other => return Err(format!("{id}: unknown source kind {other}").into()),
        };
        sources.push(Source {
            repository: string(&entry, "repository"),
            revision: string(&entry, "revision"),
            asset: string(&entry, "asset"),
            sha256: string(&entry, "sha256"),
            id,
            kind,
        });
    }
    let mut artifacts = vec![];
    for entry in entries("artifact")? {
        let id = required(&entry, "id", "artifact")?;
        let artifact = Artifact {
            source: required(&entry, "source", &id)?,
            path: required(&entry, "path", &id)?,
            sha256: required(&entry, "sha256", &id)?,
            id,
        };
        if !sources.iter().any(|s| s.id == artifact.source) {
            return Err(format!("{}: unknown source {}", artifact.id, artifact.source).into());
        }
        artifacts.push(artifact);
    }
    Ok((sources, artifacts))
}

pub(crate) fn sha256(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn verified(path: &Path, expected: &str) -> Result<bool> {
    Ok(path.is_file() && sha256(path)? == expected)
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

fn cache_directory(root: &Path, source: &Source) -> Result<PathBuf> {
    let revision = source
        .revision
        .as_deref()
        .ok_or_else(|| format!("{} lacks `revision`", source.id))?;
    Ok(root.join(CACHE).join(&source.id).join(revision))
}

fn fetch(root: &Path, source: &Source, artifact: &Artifact) -> Result<PathBuf> {
    let directory = cache_directory(root, source)?;
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
        Kind::Git => download(
            &format!(
                "https://raw.githubusercontent.com/{}/{revision}/{}",
                github(repository)?,
                artifact.path
            ),
            &destination,
        )?,
        Kind::Release => {
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
                let actual = sha256(&tarball)?;
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
        Kind::Local => unreachable!("local artifacts are not fetched"),
    }
    let actual = sha256(&destination)?;
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
pub fn pinned(ctx: &Context, chip: &str) -> Result<Vec<Pinned>> {
    let resolved = resolve(ctx, chip)?;
    match resolved.unfetched.first() {
        None => Ok(resolved.pinned),
        Some(id) => Err(format!(
            "{id} is not fetched as pinned; run `cargo xtask vendor-fetch {chip}`"
        )
        .into()),
    }
}

/// The fetched vendor artifacts of `chip` that are missing or differ from
/// their pin; empty when every citation can be checked.
pub fn unfetched(ctx: &Context, chip: &str) -> Result<Vec<String>> {
    Ok(resolve(ctx, chip)?.unfetched)
}

struct Resolved {
    pinned: Vec<Pinned>,
    unfetched: Vec<String>,
}

fn resolve(ctx: &Context, chip: &str) -> Result<Resolved> {
    let manifest = manifest_path(&ctx.root, chip)?;
    resolve_manifest(
        &ctx.root,
        &std::fs::read_to_string(ctx.root.join(manifest))?,
    )
}

fn resolve_manifest(root: &Path, manifest: &str) -> Result<Resolved> {
    let (sources, artifacts) = parse(manifest)?;
    let mut resolved = Resolved {
        pinned: vec![],
        unfetched: vec![],
    };
    for artifact in artifacts {
        let source = sources
            .iter()
            .find(|s| s.id == artifact.source)
            .expect("parsed sources");
        let local = source.kind == Kind::Local;
        let path = if local {
            root.join(&artifact.path)
        } else {
            cache_directory(root, source)?.join(&artifact.path)
        };
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
pub fn git_pins(ctx: &Context, chip: &str) -> Result<Vec<GitPin>> {
    let manifest = manifest_path(&ctx.root, chip)?;
    let (sources, artifacts) = parse(&std::fs::read_to_string(ctx.root.join(manifest))?)?;
    sources
        .into_iter()
        .filter(|s| s.kind == Kind::Git)
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
pub fn run(ctx: &Context, chip: &str) -> Result<()> {
    link_store(&ctx.root, &store()?)?;
    let manifest = manifest_path(&ctx.root, chip)?;
    let (sources, artifacts) = parse(&std::fs::read_to_string(ctx.root.join(manifest))?)?;
    let mut failures = vec![];
    for artifact in &artifacts {
        let source = sources
            .iter()
            .find(|s| s.id == artifact.source)
            .expect("parsed sources");
        let result = if source.kind == Kind::Local {
            let path = ctx.root.join(&artifact.path);
            if verified(&path, &artifact.sha256)? {
                Ok(path)
            } else {
                Err(format!(
                    "local build {} is missing or differs from the pin",
                    path.display()
                )
                .into())
            }
        } else {
            fetch(&ctx.root, source, artifact)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_manifests_parse_with_complete_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for chip in ["esp32s31", "esp32c5"] {
            let text =
                std::fs::read_to_string(root.join(manifest_path(&root, chip).unwrap())).unwrap();
            let (sources, artifacts) = parse(&text).unwrap();
            assert!(artifacts.iter().any(|a| a.id == "libphy"), "{chip}");
            for source in &sources {
                if source.kind != Kind::Local {
                    github(source.repository.as_deref().unwrap()).unwrap();
                    assert!(source.revision.is_some());
                }
            }
        }
    }

    #[test]
    fn artifacts_must_name_declared_sources() {
        let error = parse(
            "schema = 1\nsource = []\n[[artifact]]\nid = \"a\"\nsource = \"b\"\npath = \"c\"\nsha256 = \"d\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown source"));
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
            sha256(&cached).unwrap()
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
