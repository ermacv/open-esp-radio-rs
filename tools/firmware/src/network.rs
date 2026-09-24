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

const CONFIG: &str = "crates/network/dependencies/xarxa-patched.toml";
const UPSTREAM: &str = "git+https://github.com/embassy-rs/xarxa?rev=14c369bbcbe8ee7167488ac9c9e18be059d83555#14c369bbcbe8ee7167488ac9c9e18be059d83555";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Integration {
    #[default]
    UpstreamXarxa,
    PatchedXarxa,
    UpstreamSmoltcp,
    OwnedXarxa,
}
impl Integration {
    pub const fn id(self) -> &'static str {
        match self {
            Self::UpstreamXarxa => "upstream-xarxa",
            Self::PatchedXarxa => "patched-xarxa",
            Self::UpstreamSmoltcp => "upstream-smoltcp",
            Self::OwnedXarxa => "owned-xarxa",
        }
    }
    pub const fn feature(self) -> &'static str {
        match self {
            Self::UpstreamXarxa | Self::PatchedXarxa => "upstream-network",
            Self::UpstreamSmoltcp => "embassy-network",
            Self::OwnedXarxa => "owned-network",
        }
    }

    /// Resolve legacy feature selection without hiding the effective stack in artifacts.
    pub fn for_example(explicit: Option<Self>, features: &[String]) -> Result<Self> {
        let mut selected = explicit;
        for feature in features {
            let candidate = match feature.as_str() {
                "upstream-network" => Self::UpstreamXarxa,
                "embassy-network" => Self::UpstreamSmoltcp,
                "owned-network" => Self::OwnedXarxa,
                _ => continue,
            };
            if let Some(current) = selected {
                if current.feature() != candidate.feature() {
                    return Err("select exactly one network implementation; --network conflicts with the network Cargo feature".into());
                }
            } else {
                selected = Some(candidate);
            }
        }
        Ok(selected.unwrap_or_default())
    }

    pub fn configure(self, command: &mut Command, root: &Path) {
        if self == Self::PatchedXarxa {
            command.arg("--config").arg(root.join(CONFIG));
        }
    }
}
impl FromStr for Integration {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "upstream-xarxa" | "upstream" => Ok(Self::UpstreamXarxa),
            "patched-xarxa" | "udp-backpressure" => Ok(Self::PatchedXarxa),
            "upstream-smoltcp" => Ok(Self::UpstreamSmoltcp),
            "owned-xarxa" => Ok(Self::OwnedXarxa),
            _ => Err(format!(
                "unknown network integration `{value}` (expected upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa)"
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

    /// Check that the network selection changed only its expected pins.
    pub fn validate(&self, root: &Path, integration: Integration) -> Result<()> {
        let config: toml::Value = toml::from_str(&fs::read_to_string(root.join(CONFIG))?)?;
        let spec = &config["patch"]["https://github.com/embassy-rs/xarxa"]["xarxa"];
        let git = spec["git"]
            .as_str()
            .ok_or("missing patched Xarxa repository")?;
        let rev = spec["rev"]
            .as_str()
            .ok_or("missing patched Xarxa revision")?;
        if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Xarxa must be pinned to a full revision".into());
        }
        validate_identities(
            identities(&fs::read(&self.committed)?)?,
            identities(&fs::read(&self.path)?)?,
            integration,
            &format!("git+{git}?rev={rev}#{rev}"),
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
fn validate_identities(
    mut expected: BTreeSet<Identity>,
    actual: BTreeSet<Identity>,
    integration: Integration,
    patched: &str,
) -> Result<()> {
    if integration == Integration::PatchedXarxa {
        let entry = expected
            .iter()
            .find(|(n, _, s)| n == "xarxa" && s.as_deref() == Some(UPSTREAM))
            .cloned()
            .ok_or("patched-xarxa requires the original upstream Xarxa composition")?;
        expected.remove(&entry);
        expected.insert((entry.0, entry.1, Some(patched.to_owned())));
    }
    if expected != actual {
        return Err(format!(
            "network selection changed unexpected dependency pins: removed {:?}; added {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
