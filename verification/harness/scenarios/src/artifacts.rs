//! The pinned vendor artifacts of the installed chip's `artifacts.toml`,
//! read through `oer-vendor-artifacts`, the one reader of the pins.
//!
//! Every scenario input is authenticated against this manifest, and each
//! fetchable artifact has one cache location that `cargo verification fetch`
//! fills. The manifest is the only place a pin changes.
use oer_vendor_pins::{Artifact, Manifest};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The manifest compiled into this binary.
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        Manifest::parse(crate::chip().manifest_text).expect("tracked artifact manifest")
    })
}

fn artifact(id: &str) -> &'static Artifact {
    manifest()
        .artifact(id)
        .unwrap_or_else(|_| panic!("artifact {id} is not pinned"))
}

/// Pinned SHA-256 of artifact `id`.
pub fn sha256(id: &str) -> &'static str {
    &artifact(id).sha256
}

/// Location of artifact `id` below the repository `root`: the fetch cache
/// for downloaded artifacts, the build output for local ones.
pub fn path(root: &Path, id: &str) -> PathBuf {
    manifest()
        .location(root, artifact(id))
        .unwrap_or_else(|error| panic!("artifact {id}: {error}"))
}

/// Pinned location of `id` in this checkout: the default of every scenario
/// input argument.
pub fn default_path(id: &str) -> PathBuf {
    path(&oer_process::built_root(), id)
}
