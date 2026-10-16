//! Publication and download of versions as releases, through a transport.
//!
//! A version is published at most once and never changed or deleted (rules
//! 3 and 5): publishing creates a draft, reads its assets back and verifies
//! them, and only then publishes. A version that already exists is
//! accepted only when it holds exactly `manifest.json` and
//! `evidence.tar.gz` and they are the version's bytes; a release with a
//! missing or another asset is refused.

use std::path::{Path, PathBuf};

use crate::{
    Copy, Manifest, Pin, Result, Version,
    copy::unpack,
    manifest::{ARCHIVE, MANIFEST},
    refused,
};

/// Whether a release is visible to readers yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Draft,
    Published,
}

/// A store of releases by tag, such as one GitHub repository's. Each call
/// may reach the network.
pub trait Releases {
    /// Where the releases are, as a person names it to delete a draft by
    /// hand: the `owner/name` of a GitHub repository.
    fn location(&self) -> String;
    /// The release tagged `version`, if any.
    fn state(&self, version: &Version) -> Result<Option<State>>;
    /// The names of the assets of release `version`.
    fn assets(&self, version: &Version) -> Result<Vec<String>>;
    /// Create the draft release `version` with `assets`.
    fn create_draft(&self, version: &Version, assets: &[PathBuf]) -> Result<()>;
    /// Download the asset `name` of release `version` into `directory`.
    fn download(&self, version: &Version, name: &str, directory: &Path) -> Result<()>;
    /// Make the draft `version` visible.
    fn publish(&self, version: &Version) -> Result<()>;
}

/// What publishing did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Published {
    /// The version is new.
    Created,
    /// A draft of the same bytes, left by an interrupted publication after
    /// its upload, is now published. A draft whose upload was interrupted
    /// lacks an asset and is refused: nothing reads a draft, so a person
    /// deletes it and publishes again.
    Completed,
    /// The version was already published with the same bytes.
    Existing,
}

/// Publish the packed version in `directory` (its `manifest.json` and
/// `evidence.tar.gz`) after verifying it locally.
pub fn publish(releases: &dyn Releases, directory: &Path) -> Result<(Manifest, Published)> {
    let manifest = Manifest::parse(&std::fs::read(directory.join(MANIFEST))?)?;
    let archive = directory.join(ARCHIVE);
    // Every staging goes next to the version, never in a temporary file
    // system, and the local check's copy is gone before the read-back.
    let staging = crate::copy::beside(directory)?;
    {
        let check = tempfile::tempdir_in(&staging)?;
        unpack(&archive, &manifest, &check.path().join("copy"))?;
    }
    let version = manifest.version();
    let published = match releases.state(&version)? {
        Some(State::Published) => {
            complete(releases, &version, State::Published)?;
            read_back(releases, &manifest, &staging)?;
            Published::Existing
        }
        Some(State::Draft) => {
            complete(releases, &version, State::Draft)?;
            read_back(releases, &manifest, &staging)?;
            releases.publish(&version)?;
            Published::Completed
        }
        None => {
            releases.create_draft(&version, &[directory.join(MANIFEST), archive])?;
            read_back(releases, &manifest, &staging)?;
            releases.publish(&version)?;
            Published::Created
        }
    };
    Ok((manifest, published))
}

/// Download the pinned version and verify it into `out`, which must not
/// exist.
pub fn fetch(releases: &dyn Releases, pin: &Pin, out: &Path) -> Result<Copy> {
    // Before any download: a copy never lands on another one.
    if crate::copy::taken(out) {
        return refused(format!("{} already exists", out.display()));
    }
    let version = pin.version();
    match releases.state(&version)? {
        Some(State::Published) => {}
        Some(State::Draft) => {
            return refused(format!(
                "version {version} is a draft in {}: it was never published",
                pin.repository()
            ));
        }
        None => return refused(format!("{} has no version {version}", pin.repository())),
    }
    // The download stays next to `out`, never in a temporary file system.
    let download = tempfile::tempdir_in(crate::copy::beside(out)?)?;
    downloaded_manifest(releases, pin.manifest(), download.path())?;
    releases.download(&version, ARCHIVE, download.path())?;
    unpack(&download.path().join(ARCHIVE), pin.manifest(), out)
}

/// Whether release `version` holds exactly the two assets of a version.
fn complete(releases: &dyn Releases, version: &Version, state: State) -> Result<()> {
    let mut assets = releases.assets(version)?;
    assets.sort();
    if assets == [ARCHIVE, MANIFEST] {
        return Ok(());
    }
    match state {
        State::Draft => refused(format!(
            "draft {version} holds the assets {assets:?}, not exactly {MANIFEST} and {ARCHIVE}: \
             {} Nothing reads a draft: delete it by hand \
             (`gh release delete {version} --repo {}`), then publish again",
            if [ARCHIVE, MANIFEST]
                .iter()
                .all(|name| assets.iter().any(|asset| asset == name))
            {
                "it holds an asset no version has."
            } else {
                "its upload was interrupted."
            },
            releases.location()
        )),
        State::Published => refused(format!(
            "published version {version} holds the assets {assets:?}, not exactly {MANIFEST} \
             and {ARCHIVE}; a published version is never changed: ask the owner"
        )),
    }
}

/// Download the version's assets into `staging` and verify them against
/// `manifest`.
fn read_back(releases: &dyn Releases, manifest: &Manifest, staging: &Path) -> Result<()> {
    let download = tempfile::tempdir_in(staging)?;
    downloaded_manifest(releases, manifest, download.path())?;
    releases.download(&manifest.version(), ARCHIVE, download.path())?;
    unpack(
        &download.path().join(ARCHIVE),
        manifest,
        &download.path().join("copy"),
    )
    .map_err(|error| {
        crate::within(
            error,
            format_args!(
                "release {} does not hold the packed payload",
                manifest.version()
            ),
        )
    })?;
    Ok(())
}

/// Download the version's manifest into `directory` and refuse other bytes
/// than `expected`'s.
fn downloaded_manifest(
    releases: &dyn Releases,
    expected: &Manifest,
    directory: &Path,
) -> Result<()> {
    let version = expected.version();
    releases.download(&version, MANIFEST, directory)?;
    if std::fs::read(directory.join(MANIFEST))? != expected.bytes() {
        return refused(format!(
            "release {version} holds another manifest than the version's"
        ));
    }
    Ok(())
}
