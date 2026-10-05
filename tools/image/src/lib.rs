//! The one image pipeline: `build(ImageSpec) -> ImageBundle`.
//!
//! Every firmware image of the repository, a standalone example or a HIL
//! image class, of a staged or an ESP-IDF-bootloader chip, is built here,
//! with the pipeline of the chip profile's boot kind:
//!
//! - [`staged`]: the ROM loads the platform bootstrap, which stages the
//!   packed runtime into PSRAM (ESP32-S31);
//! - [`esp_idf`]: the chip's ESP-IDF second-stage bootloader, built from the
//!   firmware catalog against the pinned ESP-IDF, loads the application
//!   (ESP32-C5).
//!
//! Both compile with the image compiler of `oer-toolchain` under the
//! image's stack policy, run every gate the policy names (task, interrupt
//! and bootstrap stacks, frame coverage, move size, placement), and encode
//! the complete flash contents at build time with the `espflash` library:
//! the application, the bootloader, the partition table and the OTA
//! selection, at the offsets of the chip's [`FlashMap`]. A flash only writes
//! a bundle's files ([`ImageBundle::segments`]). Each bundle records the
//! repository files it was built from (`source-inputs.json`), the builder's
//! own sources taken from its real dependency closure.

pub mod build_log;
pub mod bundle;
pub mod cargo;
pub mod compare;
pub mod encode;
pub mod esp_idf;
pub mod exclusion;
pub mod interrupt_stack;
pub mod source_inputs;
pub mod stack;
pub mod staged;

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

pub use build_log::{BuildLog, BuildStepFailed};
pub use bundle::{ImageBundle, Segment};
pub use cargo::{LAYOUT_SEED_ENV, Overrides};
pub use interrupt_stack::Required;
pub use oer_chip_profile::{Boot, FlashMap};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The packages of the pipeline itself: every bundle's builder sources
/// start from their closure. The image linker is a root of its own: no
/// package depends on it, every image links with it.
pub const PIPELINE: [&str; 2] = ["oer-image", "oer-image-linker"];

/// An audit of the runtime ELF that the caller adds to the pipeline's own.
pub type Audit = Box<dyn Fn(&oer_elf::Elf<'_>) -> Result<()> + Send + Sync>;

/// What to build.
pub struct ImageSpec {
    /// The tree the image is built from: a checkout or a source snapshot.
    pub root: PathBuf,
    /// The chip id; its profile names the boot kind and the flash map.
    pub chip: String,
    pub application: Application,
    /// The image's stack policy (`stack.toml`), relative to `root`.
    pub stack_policy: PathBuf,
    /// What the interrupt-stack gate requires of the runtime.
    pub interrupts: Required,
    /// The seed the runtime's code and read-only data are shuffled by;
    /// `None` keeps the linker's natural order.
    pub layout_seed: Option<NonZeroU32>,
    /// Local checkouts that replace pinned dependencies; a build with any
    /// resolves through its private lock copy without `--locked`.
    pub overrides: Overrides,
    /// The caller's packages whose code drives the build (a HIL image
    /// builder): their dependency closure joins the source inputs.
    pub builders: Vec<String>,
    /// Further repository files the build reads, relative to `root`.
    pub reads: Vec<PathBuf>,
    /// The bundle's directory.
    pub output: PathBuf,
    /// The compile cache: one build at a time uses it.
    pub cache: PathBuf,
    /// The caller's own audit of the runtime ELF (placement of its state).
    pub audit: Option<Audit>,
}

/// The application crate of an image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Application {
    /// Its Cargo workspace, relative to the root; the workspace's committed
    /// `Cargo.lock` is the build's catalog.
    pub workspace: PathBuf,
    pub package: String,
    pub binary: String,
    pub features: Vec<String>,
    pub default_features: bool,
}

impl Application {
    /// Cargo's feature arguments for this application.
    fn feature_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if !self.default_features {
            arguments.push("--no-default-features".to_owned());
        }
        if !self.features.is_empty() {
            arguments.push("--features".to_owned());
            arguments.push(self.features.join(","));
        }
        arguments
    }
}

/// Build `spec` with the pipeline of its chip's boot kind.
pub fn build(spec: &ImageSpec) -> Result<ImageBundle> {
    let profile = profile(&spec.root, &spec.chip)?;
    match profile.boot {
        Boot::Staged => staged::build(spec, &profile),
        Boot::EspIdfBootloader => esp_idf::build(spec, &profile),
    }
}

/// Type-check the runtime of `spec` against the committed pins with the
/// image build's target, features and compiler configuration, without code
/// generation, in its compile cache. Lints that need monomorphization
/// (`large_assignments`) and the link-time gates still need [`build`].
pub fn type_check(spec: &ImageSpec) -> Result<()> {
    let profile = profile(&spec.root, &spec.chip)?;
    let policy = stack::StackPolicy::load(&spec.root.join(&spec.stack_policy))?;
    let lock = exclusion::BuildLock::prepare(
        &spec.root.join(&spec.application.workspace),
        &spec.cache.join("check-lock"),
    )?;
    let mut command = runtime_command(spec, &profile, "check", &policy)?;
    command.env("CARGO_TARGET_DIR", spec.cache.join("runtime"));
    lock.configure(&mut command);
    let manifest = spec
        .root
        .join(&spec.application.workspace)
        .join("Cargo.toml");
    cargo::ensure_fetched(&spec.root, &manifest, |command| lock.configure(command))?;
    // Through the shared runner, so a caller's output policy (xtask logs
    // child output by default) applies to the type check too.
    oer_process::run(&mut command).map_err(|error| {
        format!(
            "the {} runtime does not type-check: {error}",
            spec.application.package
        )
    })?;
    lock.validate()
}

/// The profile of `chip` at `root`, which must name its flash map.
pub fn profile(root: &Path, chip: &str) -> Result<oer_chip_profile::Profile> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    if profile.flash.is_none() {
        return Err(
            format!("platform/{chip}/chip.toml names no [flash] map for its images").into(),
        );
    }
    Ok(profile)
}

/// The Cargo `subcommand` (`build` or `check`) of `spec`'s runtime with the
/// image compiler of `policy`: the application package for the chip's
/// target, its features alone, `--locked` unless a dependency is
/// overridden.
fn runtime_command(
    spec: &ImageSpec,
    profile: &oer_chip_profile::Profile,
    subcommand: &str,
    policy: &stack::StackPolicy,
) -> Result<std::process::Command> {
    let application = &spec.application;
    let mut command = cargo::command();
    command
        .current_dir(&spec.root)
        .arg(subcommand)
        .arg("--manifest-path")
        .arg(spec.root.join(&application.workspace).join("Cargo.toml"))
        .args(["-p", &application.package, "--bin", &application.binary])
        .args(["--release", "--target", &profile.rust_target])
        .args(application.feature_arguments())
        .env("CARGO_INCREMENTAL", "0");
    if spec.overrides.is_empty() {
        command.arg("--locked");
    }
    spec.overrides.apply(&mut command);
    if let Some(seed) = spec.layout_seed {
        command.env(LAYOUT_SEED_ENV, seed.to_string());
    }
    oer_toolchain::image::configure(
        &mut command,
        &policy.image_compiler(
            &spec.root,
            &spec.cache.join("image-linker"),
            &profile.rust_target,
        ),
    )?;
    Ok(command)
}

/// Names the host build root instead of the user's cache directory.
pub const BUILD_ROOT_ENV: &str = "OER_BUILD_ROOT";

/// The directory every checkout of this host builds images in: its build
/// slots ([`exclusion::slot`]), and for HIL the source snapshots, their
/// build workspaces and compile caches. One per host, so each agent's build
/// of the same sources reuses the same compiled units, and its size is
/// bounded by the slots instead of growing with the number of checkouts.
pub fn host_build_root() -> Result<PathBuf> {
    // Unit tests build into the workspace's disposable target directory,
    // never into the host's shared build root.
    #[cfg(test)]
    if std::env::var_os(BUILD_ROOT_ENV).is_none_or(|root| root.is_empty()) {
        return Ok(oer_process::built_root().join("target/test-build-root"));
    }
    oer_durable::xdg::overridable(BUILD_ROOT_ENV, oer_durable::xdg::Base::Cache, "build")
}

/// Copy `source` to `target` unless `target` already holds the same bytes, so
/// that an unchanged file keeps the timestamp Cargo compares.
fn replace_if_changed(source: &Path, target: &Path) -> Result<()> {
    let bytes = std::fs::read(source)?;
    if std::fs::read(target).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let parent = target.parent().ok_or("target has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut staged, &bytes)?;
    staged.persist(target)?;
    Ok(())
}

/// Copy the compiled `source` out of the compile cache, which a later build
/// may overwrite, to `output/name`.
fn snapshot(source: &Path, output: &Path, name: &str) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(format!("the build left no {name}: {}", source.display()).into());
    }
    let destination = output.join(name);
    std::fs::copy(source, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests;
