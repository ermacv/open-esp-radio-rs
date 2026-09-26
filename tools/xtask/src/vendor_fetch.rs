//! Fetch the pinned vendor artifacts of a chip into the target cache.
//!
//! The chip's tracked `artifacts.toml` is the only pin. Git artifacts come
//! from the upstream repository at the pinned revision, release members from
//! the pinned release asset; every file is verified against its SHA-256
//! before it enters `target/vendor/<source>/<revision>/<path>`. Local build
//! outputs are only verified. Downloads use `curl` and release members `tar`.
use crate::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Cache of fetched artifacts, relative to the repository root.
pub const CACHE: &str = "target/vendor";

/// Tracked manifest of `chip`, relative to the repository root.
pub fn manifest_path(chip: &str) -> Result<&'static str> {
    match chip {
        "esp32s31" => Ok("verification/vendor/projects/esp32s31/artifacts.toml"),
        other => Err(format!("no vendor artifacts pinned for chip {other}").into()),
    }
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

fn sha256(path: &Path) -> Result<String> {
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

/// Fetch and verify every artifact of `chip`; report local builds that are
/// missing or differ. Fails when any artifact is not available as pinned.
pub fn run(ctx: &Context, chip: &str) -> Result<()> {
    let manifest = manifest_path(chip)?;
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
        let text = std::fs::read_to_string(root.join(manifest_path("esp32s31").unwrap())).unwrap();
        let (sources, artifacts) = parse(&text).unwrap();
        assert!(artifacts.iter().any(|a| a.id == "libphy"));
        for source in &sources {
            if source.kind != Kind::Local {
                github(source.repository.as_deref().unwrap()).unwrap();
                assert!(source.revision.is_some());
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
}
