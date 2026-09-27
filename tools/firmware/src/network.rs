//! Reproducible network implementations shared by firmware builders.
use crate::Result;
use fs2::FileExt;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

/// The network implementation a firmware links. Owned Xarxa/Embassy is the
/// only one: the maintained owner-transfer forks declared in the manifests.
/// The type remains so that artifacts keep recording which stack they carry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Integration {
    #[default]
    OwnedXarxa,
}
impl Integration {
    pub const fn id(self) -> &'static str {
        match self {
            Self::OwnedXarxa => "owned-xarxa",
        }
    }
    pub const fn feature(self) -> &'static str {
        match self {
            Self::OwnedXarxa => "owned-network",
        }
    }

    /// Resolve the example's selection; the Cargo feature and `--network`
    /// can name only the owned stack.
    pub fn for_example(explicit: Option<Self>, features: &[String]) -> Result<Self> {
        for feature in features {
            if feature.ends_with("-network") && feature != Self::OwnedXarxa.feature() {
                return Err(format!(
                    "network feature `{feature}` was removed; owned-network is the only network implementation"
                )
                .into());
            }
        }
        Ok(explicit.unwrap_or_default())
    }

    /// No source override: the owned stack's pins live in the manifests.
    pub fn configure(self, _command: &mut Command, _root: &Path) {}
}
impl FromStr for Integration {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "owned-xarxa" => Ok(Self::OwnedXarxa),
            "upstream-xarxa" | "upstream" | "patched-xarxa" | "udp-backpressure"
            | "upstream-smoltcp" => Err(format!(
                "network integration `{value}` was removed; owned-xarxa is the only network implementation"
            )),
            _ => Err(format!(
                "unknown network integration `{value}` (expected owned-xarxa)"
            )),
        }
    }
}

/// A private copy of one workspace's committed `Cargo.lock` for one build.
///
/// Cargo resolves through `resolver.lockfile-path`, so a patched or locally
/// overridden resolution is written only to this copy. The committed catalog
/// is never modified, concurrent builds never observe a temporary resolution,
/// and the copy is the build's effective lockfile. One build owns the copy's
/// directory at a time.
pub struct BuildLock {
    committed: PathBuf,
    path: PathBuf,
    _lease: Lease,
}

// Closing one descriptor does not release flock while a forked pre-exec
// child still holds the shared open file description. Release ownership at
// the owner's boundary, including failures after acquisition.
struct Lease(fs::File);

impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.0) {
            eprintln!("release build lockfile ownership: {error}");
        }
    }
}

impl BuildLock {
    /// Copy `workspace/Cargo.lock` to `directory/Cargo.lock` for one build.
    pub fn prepare(workspace: &Path, directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let lease = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("build.lease"))?;
        lease
            .try_lock_exclusive()
            .map_err(|e| format!("another build owns {}: {e}", directory.display()))?;
        let committed = workspace.join("Cargo.lock");
        let path = directory.join("Cargo.lock");
        fs::copy(&committed, &path)?;
        Ok(Self {
            committed,
            path,
            _lease: Lease(lease),
        })
    }

    /// The effective lockfile of this build.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve `command` through this copy instead of the committed catalog.
    pub fn configure(&self, command: &mut Command) {
        let path = toml::Value::String(self.path.display().to_string());
        command
            .arg("--config")
            .arg(format!("resolver.lockfile-path={path}"));
    }

    /// Check that the build resolved exactly the committed pins.
    pub fn validate(&self, _root: &Path, _integration: Integration) -> Result<()> {
        validate_identities(
            identities(&fs::read(&self.committed)?)?,
            identities(&fs::read(&self.path)?)?,
        )
    }
}

type Identity = (String, String, Option<String>);
fn identities(bytes: &[u8]) -> Result<BTreeSet<Identity>> {
    let lock: toml::Value = toml::from_str(std::str::from_utf8(bytes)?)?;
    lock["package"]
        .as_array()
        .ok_or("Cargo.lock has no package catalog")?
        .iter()
        .map(|p| {
            Ok((
                p["name"].as_str().ok_or("package name missing")?.to_owned(),
                p["version"]
                    .as_str()
                    .ok_or("package version missing")?
                    .to_owned(),
                p.get("source")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect()
}
fn validate_identities(expected: BTreeSet<Identity>, actual: BTreeSet<Identity>) -> Result<()> {
    if expected != actual {
        return Err(format!(
            "the build changed dependency pins: removed {:?}; added {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
