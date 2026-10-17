//! The compiler setup every image build shares: the single owner of the
//! image compiler flags. No `.cargo/config.toml` carries Rust flags for an
//! image; `cargo xtask build firmware`, the HIL image builder and their type
//! checks all apply [`configure`].
//!
//! `RUSTC_BOOTSTRAP=1` is set here, on the image's Cargo command only, and
//! exists for the unstable `-Z` flags alone (`emit-stack-sizes`,
//! `move-size-limit`, `share-generics`) on the exact stable toolchain that
//! `rust-toolchain.toml` pins.
//!
//! Every image links through the image linker (`tools/image/linker`), which
//! refuses an input section named as zero-initialized that carries bytes and
//! otherwise runs the target's `rust-lld` with the same arguments. The linker
//! is built from the sources being built, the checkout or source snapshot at
//! [`ImageCompiler::root`], so an image never links with another tree's
//! linker.
//!
//! Firmware compiles in a [`compile_cache`] of its own tree. Cargo names a
//! path package by its path relative to the workspace root, the same in every
//! checkout, and judges freshness by modification times, so one cache shared
//! by two trees reuses units compiled from the other tree's sources. Units of
//! registry and Git crates are shared between trees by a content-addressed
//! compiler cache such as `sccache` (`build.rustc-wrapper`), which the
//! pipeline neither requires nor configures.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Result, Tool};

/// The input section patterns (`name` or `name.*`, comma separated) the
/// image linker refuses initialized bytes in: the regions the boot zeroes.
/// Unset or empty, it checks none.
pub const ZEROED_INPUTS_ENV: &str = "OER_IMAGE_ZEROED_INPUTS";

/// Names the host build root instead of the user's cache directory.
pub const BUILD_ROOT_ENV: &str = "OER_BUILD_ROOT";

/// The directory every checkout of this host builds images in: each tree's
/// compile cache, the image linkers, the build slots and, for HIL, the
/// source snapshots and their build workspaces.
pub fn host_build_root() -> Result<PathBuf> {
    oer_durable::xdg::overridable(BUILD_ROOT_ENV, oer_durable::xdg::Base::Cache, "build")
        .map_err(|error| error.to_string().into())
}

/// The directory below a build root that holds every tree's compile cache.
pub const COMPILE_CACHES: &str = "firmware";

/// The file in a compile cache that names the tree it belongs to: the bytes
/// of the tree's canonical path.
pub const CACHE_TREE_FILE: &str = "tree";

/// The file in a compile cache that identifies the tree's root directory:
/// its device and inode, so a different tree later moved to the same path
/// is told apart.
pub const CACHE_TREE_ID_FILE: &str = "tree-id";

/// The device and inode of the directory at `path`.
fn directory_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .ok()
        .filter(fs::Metadata::is_dir)
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

/// A tree's firmware compile cache (`CARGO_TARGET_DIR`): every image class,
/// example and chip of the tree, and its vendor comparison probes and
/// chip-target test fixtures, compile in it and nowhere else ([module
/// documentation](self)). Builds of the tree uplift their binaries to the
/// same `<target>/release/<name>`, so each holds [`CompileCache::lock`] from
/// its Cargo run until its outputs are copied out.
#[derive(Clone, Debug)]
pub struct CompileCache {
    path: PathBuf,
    tree: PathBuf,
    id: (u64, u64),
}

impl CompileCache {
    /// The cache's directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The canonical root of the tree it belongs to.
    pub fn tree(&self) -> &Path {
        &self.tree
    }

    /// Take the cache's lock: one build at a time, the next waiting for it.
    /// A lock taken on a cache that was moved aside for removal meanwhile is
    /// let go and taken again on the cache now at the path, and the tree
    /// record is written under the lock, so a cache the lock recreated names
    /// its tree too.
    pub fn lock(&self) -> Result<oer_process::lock::FileLock> {
        use std::os::unix::fs::MetadataExt;
        let path = self.path.join("build.lock");
        loop {
            let lock = oer_process::lock::FileLock::wait(
                &path,
                oer_process::lock::Mode::Exclusive,
                &format!("waiting for another build in {}", self.path.display()),
            )
            .map_err(|error| error.to_string())?;
            let held = lock.file().metadata()?;
            if fs::metadata(&path)
                .is_ok_and(|now| (now.dev(), now.ino()) == (held.dev(), held.ino()))
            {
                self.record()?;
                return Ok(lock);
            }
        }
    }

    /// Write the tree record unless it already names this tree.
    fn record(&self) -> Result<()> {
        let recorded = CachedTree::read(&self.path);
        if recorded.tree.as_deref() != Some(self.tree.as_path()) {
            oer_durable::atomic_write(
                &self.path.join(CACHE_TREE_FILE),
                self.tree.as_os_str().as_encoded_bytes(),
            )
            .map_err(|error| format!("recording the tree of {}: {error}", self.path.display()))?;
        }
        if recorded.id != Some(self.id) {
            oer_durable::atomic_write(
                &self.path.join(CACHE_TREE_ID_FILE),
                format!("{} {}", self.id.0, self.id.1).as_bytes(),
            )
            .map_err(|error| format!("recording the tree of {}: {error}", self.path.display()))?;
        }
        Ok(())
    }
}

/// The compile cache below `build_root` of the tree at `root`, named by a
/// digest of the tree's canonical path and recorded with the tree's path and
/// identity. A cache another tree left at that path (one moved away, and
/// another moved in) is moved aside and removed, so the tree starts afresh;
/// it fails while a build holds that cache.
pub fn compile_cache(build_root: &Path, root: &Path) -> Result<CompileCache> {
    let tree = fs::canonicalize(root)
        .map_err(|error| format!("the tree {} has no canonical path: {error}", root.display()))?;
    let id = directory_id(&tree)
        .ok_or_else(|| format!("the tree {} is no directory", tree.display()))?;
    let digest = oer_durable::sha256_bytes(tree.as_os_str().as_encoded_bytes());
    let path = build_root.join(COMPILE_CACHES).join(&digest[..16]);
    let recorded = CachedTree::read(&path);
    if recorded.id.is_some_and(|recorded| recorded != id) && !remove_compile_cache(&path)? {
        return Err(format!(
            "a build holds the compile cache {} of another tree once at {}",
            path.display(),
            tree.display()
        )
        .into());
    }
    fs::create_dir_all(&path)
        .map_err(|error| format!("creating the compile cache {}: {error}", path.display()))?;
    let cache = CompileCache { path, tree, id };
    cache.record()?;
    Ok(cache)
}

/// A compile cache below a build root with the tree its record names.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CachedTree {
    /// The cache's directory.
    pub cache: PathBuf,
    /// The tree its [`CACHE_TREE_FILE`] names; `None` when it has none.
    pub tree: Option<PathBuf>,
    id: Option<(u64, u64)>,
}

impl CachedTree {
    fn read(cache: &Path) -> Self {
        use std::os::unix::ffi::OsStringExt;
        let tree = fs::read(cache.join(CACHE_TREE_FILE))
            .ok()
            .map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes)));
        let id = fs::read_to_string(cache.join(CACHE_TREE_ID_FILE))
            .ok()
            .and_then(|text| {
                let (device, inode) = text.trim().split_once(' ')?;
                Some((device.parse().ok()?, inode.parse().ok()?))
            });
        Self {
            cache: cache.to_owned(),
            tree,
            id,
        }
    }

    /// Whether the tree it was made for is gone: its record is missing, or
    /// no directory with its identity is at its path any more.
    pub fn is_gone(&self) -> bool {
        match (&self.tree, self.id) {
            (Some(tree), Some(id)) => directory_id(tree) != Some(id),
            _ => true,
        }
    }
}

/// Every compile cache below `build_root` with the tree it records.
pub fn compile_caches(build_root: &Path) -> Result<Vec<CachedTree>> {
    let mut caches = Vec::new();
    let Ok(entries) = fs::read_dir(build_root.join(COMPILE_CACHES)) else {
        return Ok(caches);
    };
    for entry in entries {
        let entry = entry?;
        // A removal moves a cache aside under a dot name first.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.path().is_dir() {
            caches.push(CachedTree::read(&entry.path()));
        }
    }
    caches.sort();
    Ok(caches)
}

/// Remove the compile cache `cache` unless a build holds it; returns whether
/// it was removed.
pub fn remove_compile_cache(cache: &Path) -> Result<bool> {
    remove_cache_unless(cache, |_| false)
}

/// Remove the compile cache `cache` unless a build holds it or the tree it
/// records exists again ([`CachedTree::is_gone`]), both decided under its
/// lock; returns whether it was removed.
pub fn remove_orphaned_compile_cache(cache: &Path) -> Result<bool> {
    remove_cache_unless(cache, |recorded| !recorded.is_gone())
}

/// Under the exclusive lock of `cache`, unless `keep` holds for the tree it
/// records: move it aside atomically, so a build that comes later creates a
/// fresh cache instead of entering this one, let the lock go and remove it
/// there. A cache that does not exist is not removed.
fn remove_cache_unless(cache: &Path, keep: impl FnOnce(&CachedTree) -> bool) -> Result<bool> {
    if !cache.is_dir() {
        return Ok(false);
    }
    let Some(held) = oer_process::lock::FileLock::try_acquire(
        &cache.join("build.lock"),
        oer_process::lock::Mode::Exclusive,
    )
    .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    if keep(&CachedTree::read(cache)) {
        return Ok(false);
    }
    let name = cache
        .file_name()
        .ok_or("a compile cache has a name")?
        .to_string_lossy();
    let removing = cache.with_file_name(format!(".removing-{name}-{}", std::process::id()));
    fs::rename(cache, &removing).map_err(|error| {
        format!(
            "moving {} aside to {}: {error}",
            cache.display(),
            removing.display()
        )
    })?;
    drop(held);
    fs::remove_dir_all(&removing)
        .map_err(|error| format!("removing {}: {error}", removing.display()))?;
    Ok(true)
}

/// The directory below a build root that holds every installed image
/// linker, one per identity ([`install_linker`]).
pub const LINKERS: &str = "image-linkers";

/// What an image's compiler setup depends on.
#[derive(Clone, Debug)]
pub struct ImageCompiler<'a> {
    /// The tree being built: its `tools/image/linker` becomes the linker.
    pub root: &'a Path,
    /// The tree's [`compile_cache`]; the linker's host build is below it.
    pub cache: &'a Path,
    /// The build root whose [`LINKERS`] store the installed linker.
    pub build_root: &'a Path,
    /// The image's Rust target triple.
    pub rust_target: &'a str,
    /// The largest move the image allows (`-Z move-size-limit`).
    pub max_move_bytes: u64,
    /// Variables the image's runtime reads at compile time (its stack
    /// policy's reserves).
    pub runtime_environment: Vec<(&'static str, String)>,
}

/// Configure `command`, a Cargo build of an image, with the compiler flags
/// every image shares, building and installing the image linker first.
pub fn configure(command: &mut Command, compiler: &ImageCompiler<'_>) -> Result<()> {
    let linker = install_linker(compiler.root, compiler.cache, compiler.build_root)?;
    // The pinned stable toolchain supports these flags, but they remain
    // unstable: RUSTC_BOOTSTRAP enables them for this command alone. The
    // stack-size ELF section is consumed by a safe host-side parser.
    command.env("RUSTC_BOOTSTRAP", "1").env(
        "RUSTFLAGS",
        rustflags(env::var("RUSTFLAGS").ok(), compiler.max_move_bytes, &linker)?,
    );
    for (variable, value) in &compiler.runtime_environment {
        command.env(variable, value);
    }
    // C and C++ that build scripts compile for the image (through the `cc`
    // and `cmake` crates) emit the same `.stack_sizes` section, so the stack
    // gate has their frames as well.
    for variable in c_flag_variables(compiler.rust_target) {
        let flags = with_stack_sizes(env::var(&variable).ok());
        command.env(variable, flags);
    }
    Ok(())
}

/// Build the image linker of the tree at `root` for the host into
/// `target_dir` and return its path. The build leaves the image's compiler
/// environment behind: it is a host tool.
pub fn build_linker(root: &Path, target_dir: &Path) -> Result<PathBuf> {
    let mut command = crate::command(Tool::Cargo)?;
    command
        .current_dir(root)
        .args([
            "build",
            "--quiet",
            "-p",
            "oer-image-linker",
            "--bin",
            "oer-image-linker",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir);
    for variable in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "RUSTC_BOOTSTRAP",
    ] {
        command.env_remove(variable);
    }
    oer_process::run(&mut command).map_err(|error| {
        format!(
            "building the image linker of {} failed: {error}",
            root.display()
        )
    })?;
    Ok(target_dir.join("debug/oer-image-linker"))
}

/// Build the image linker of the tree at `root` in its compile cache `cache`
/// and install it below `build_root`'s [`LINKERS`] under its identity
/// ([`linker_identity`]); returns the installed path.
///
/// The path is part of every image's Rust flags, and so of every compiled
/// unit's identity: trees whose linker sources agree share one path, so a
/// content-addressed compiler cache serves their units, and a changed linker
/// gets a new path, so every unit it would link is rebuilt.
pub fn install_linker(root: &Path, cache: &Path, build_root: &Path) -> Result<PathBuf> {
    let target = cache.join("image-linker");
    let built = build_linker(root, &target)?;
    let dep_info = built.with_extension("d");
    let inputs = linker_inputs(root, &target, &dep_info)?;
    // A source written after the build may not be in the binary: its
    // identity would name sources it was not built from. Only the files the
    // dep-info names: Cargo judges their freshness by time, while a
    // manifest, lock or configuration it compares by content may be newer
    // than a build that is up to date.
    let built_at = modified(&built)?;
    let sources = dep_info_paths(&fs::read_to_string(&dep_info).map_err(|error| {
        format!(
            "the image linker's dep-info {} is unreadable: {error}",
            dep_info.display()
        )
    })?);
    if let Some(path) = sources
        .iter()
        .find(|path| modified(path).is_ok_and(|at| at > built_at))
    {
        return Err(format!(
            "{} changed after the image linker {} was built; build again",
            path.display(),
            built.display()
        )
        .into());
    }
    let directory = build_root.join(LINKERS).join(identity_of(&inputs)?);
    let installed = directory.join("oer-image-linker");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    // A hard link publishes the linker whole and opens no file for writing:
    // a writable descriptor, inherited by a child another thread forks
    // meanwhile, makes running the linker fail with "text file busy" until
    // that child executes. Cargo replaces its output by a new file, never in
    // place, so the link keeps this build's bytes. Another thread or process
    // installing the same identity meanwhile linked the same linker.
    match fs::hard_link(&built, &installed) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_into_place(&built, &installed)?;
        }
        Err(error) => {
            return Err(format!(
                "installing {} as {}: {error}",
                built.display(),
                installed.display()
            )
            .into());
        }
    }
    // Its modification time is its last use, which `cargo hil sweep` ages:
    // renewed at every installation, since a new link or copy carries the
    // time Cargo built the linker, which may be days ago. A running linker
    // cannot be opened for writing; its owner may set the time through a
    // read-only handle.
    fs::File::open(&installed)
        .and_then(|file| file.set_modified(std::time::SystemTime::now()))
        .map_err(|error| format!("renewing {}: {error}", installed.display()))?;
    Ok(installed)
}

/// Copy `built` to a file of its own beside `installed`, then rename it into
/// place unless another installation got there first.
fn copy_into_place(built: &Path, installed: &Path) -> Result<()> {
    let directory = installed
        .parent()
        .ok_or("an installed linker has a directory")?;
    let staged = tempfile::Builder::new()
        .prefix(".oer-image-linker.")
        .tempfile_in(directory)
        .map_err(|error| format!("staging a linker in {}: {error}", directory.display()))?;
    // `fs::copy` keeps the executable bit.
    fs::copy(built, staged.path()).map_err(|error| {
        format!(
            "copying {} to {}: {error}",
            built.display(),
            staged.path().display()
        )
    })?;
    match staged.persist_noclobber(installed) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("installing {}: {}", installed.display(), error.error).into()),
    }
}

fn modified(path: &Path) -> Result<std::time::SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("reading the time of {}: {error}", path.display()).into())
}

/// The identity of an image linker built into `target` from the tree at
/// `root`, from its dep-info at `dep_info` ([`linker_inputs`]).
pub fn linker_identity(root: &Path, target: &Path, dep_info: &Path) -> Result<String> {
    identity_of(&linker_inputs(root, target, dep_info)?)
}

/// A digest of `inputs` (each a file and the name it is listed under) and of
/// the compiler that built them (`rustc -vV`).
fn identity_of(inputs: &[(PathBuf, String)]) -> Result<String> {
    let compiler = oer_process::capture(crate::command(Tool::Rustc)?.arg("-vV"))
        .map_err(|error| format!("reading the compiler's version: {error}"))?;
    let mut listing = format!(
        "rustc -vV\t{}\n",
        oer_durable::sha256_bytes(&compiler.stdout)
    );
    for (path, named) in inputs {
        listing.push_str(&format!("{named}\t{}\n", oer_durable::sha256_file(path)?));
    }
    Ok(oer_durable::sha256_bytes(listing.as_bytes())[..16].to_owned())
}

/// The inputs of an image linker built into `target`, each with the name its
/// identity lists it under: the files its dep-info at `dep_info` names, the
/// manifest of each package of the tree at `root` they lie in, and the tree's
/// workspace manifest, lock, Cargo configuration and toolchain pin. A file
/// inside the tree or generated inside `target` is named by its path
/// relative to it, so two checkouts of the same sources agree.
pub fn linker_inputs(
    root: &Path,
    target: &Path,
    dep_info: &Path,
) -> Result<Vec<(PathBuf, String)>> {
    let canonical = |path: &Path| {
        fs::canonicalize(path)
            .map_err(|error| format!("{} has no canonical path: {error}", path.display()))
    };
    let root = canonical(root)?;
    let target = canonical(target)?;
    let text = fs::read_to_string(dep_info).map_err(|error| {
        format!(
            "the image linker's dep-info {} is unreadable: {error}",
            dep_info.display()
        )
    })?;
    let mut inputs = std::collections::BTreeSet::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config.toml",
    ] {
        inputs.insert(root.join(name));
    }
    for path in dep_info_paths(&text) {
        let path = fs::canonicalize(&path).unwrap_or(path);
        if path.starts_with(&root) && !path.starts_with(&target) {
            let manifest = path
                .ancestors()
                .skip(1)
                .take_while(|directory| directory.starts_with(&root))
                .map(|directory| directory.join("Cargo.toml"))
                .find(|manifest| manifest.is_file());
            inputs.extend(manifest);
        }
        inputs.insert(path);
    }
    Ok(inputs
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| {
            let named = match (path.strip_prefix(&target), path.strip_prefix(&root)) {
                (Ok(generated), _) => Path::new("<target>").join(generated),
                (_, Ok(inside)) => inside.to_owned(),
                _ => path.clone(),
            };
            let named = named.display().to_string();
            (path, named)
        })
        .collect())
}

/// The paths a Makefile-style dep-info file (Cargo's `.d`) lists, a space
/// inside a path escaped.
pub fn dep_info_paths(dep_info: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in dep_info.lines() {
        let Some((_, list)) = line.split_once(": ") else {
            continue;
        };
        let mut current = String::new();
        let mut characters = list.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '\\' if characters.peek() == Some(&' ') => {
                    current.push(' ');
                    characters.next();
                }
                ' ' => {
                    if !current.is_empty() {
                        paths.push(PathBuf::from(std::mem::take(&mut current)));
                    }
                }
                character => current.push(character),
            }
        }
        if !current.is_empty() {
            paths.push(PathBuf::from(current));
        }
    }
    paths
}

/// The per-target C and C++ flag variables the `cc` and `cmake` crates read.
fn c_flag_variables(target: &str) -> [String; 2] {
    let target = target.replace('-', "_");
    [format!("CFLAGS_{target}"), format!("CXXFLAGS_{target}")]
}

/// Existing C flags with Clang's frame-size section appended once.
fn with_stack_sizes(existing: Option<String>) -> String {
    const STACK_SIZES: &str = "-fstack-size-section";
    match existing {
        Some(flags) if flags.split_whitespace().any(|flag| flag == STACK_SIZES) => flags,
        Some(flags) if !flags.trim().is_empty() => format!("{flags} {STACK_SIZES}"),
        _ => STACK_SIZES.to_owned(),
    }
}

/// Existing Rust flags with the image flags appended.
///
/// Without shared generics, fat LTO keeps one copy of a generic instance per
/// crate that instantiates it: the Wi-Fi and Bluetooth compositions both
/// instantiate the same PHY registration and radio guard, which would
/// otherwise be emitted twice.
///
/// The image linker replaces the target's `rust-lld` and keeps its flavor.
/// `RUSTFLAGS` separates flags at spaces, so the linker's path must have
/// none.
fn rustflags(existing: Option<String>, max_move_bytes: u64, linker: &Path) -> Result<String> {
    let linker = linker
        .to_str()
        .filter(|path| !path.contains(char::is_whitespace))
        .ok_or_else(|| {
            format!(
                "the image linker's path must be UTF-8 without spaces: {}",
                linker.display()
            )
        })?;
    let mut rustflags = existing.unwrap_or_default();
    for required in [
        "-Z emit-stack-sizes".to_owned(),
        format!("-Z move-size-limit={max_move_bytes}"),
        "-D large-assignments".to_owned(),
        "-Z share-generics=y".to_owned(),
        format!("-C linker={linker}"),
        "-C linker-flavor=ld.lld".to_owned(),
    ] {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(&required);
    }
    Ok(rustflags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_builds_emit_frame_sizes_for_the_image_target() {
        assert_eq!(
            c_flag_variables("riscv32imafc-unknown-none-elf"),
            [
                "CFLAGS_riscv32imafc_unknown_none_elf".to_owned(),
                "CXXFLAGS_riscv32imafc_unknown_none_elf".to_owned(),
            ]
        );
        assert_eq!(with_stack_sizes(None), "-fstack-size-section");
        assert_eq!(
            with_stack_sizes(Some("-O2".into())),
            "-O2 -fstack-size-section"
        );
        assert_eq!(
            with_stack_sizes(Some("-fstack-size-section".into())),
            "-fstack-size-section"
        );
    }

    #[test]
    fn images_share_generic_instances_after_the_callers_flags() {
        assert_eq!(
            rustflags(
                Some("-C debuginfo=1".into()),
                4096,
                Path::new("/repo/target/image-linker/debug/oer-image-linker"),
            )
            .unwrap(),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -D large-assignments -Z share-generics=y \
             -C linker=/repo/target/image-linker/debug/oer-image-linker -C linker-flavor=ld.lld"
        );
        // A path RUSTFLAGS would split is refused.
        assert!(rustflags(None, 4096, Path::new("/a b/linker")).is_err());
    }

    /// A tree whose `oer-image-linker` is a stand-in that announces its tree.
    fn tree_with_linker(marker: &str) -> tempfile::TempDir {
        let tree = tempfile::tempdir().unwrap();
        let package = tree.path().join("linker");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(
            tree.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"linker\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        std::fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"oer-image-linker\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            package.join("src/main.rs"),
            format!("fn main() {{ print!(\"{marker}\"); }}\n"),
        )
        .unwrap();
        tree
    }

    #[test]
    fn the_linker_is_built_from_the_given_tree() {
        let first = tree_with_linker("first");
        let second = tree_with_linker("second");
        let target = tempfile::tempdir().unwrap();
        for (tree, marker) in [(&first, "first"), (&second, "second")] {
            let linker = build_linker(tree.path(), &target.path().join(marker)).unwrap();
            let output = oer_process::command(&linker).output().unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), marker);
        }
    }

    fn run(program: &Path) -> String {
        let output = oer_process::command(program).output().unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    /// Two trees whose linkers agree install one linker; a changed linker
    /// installs a new one, so the units its path names are rebuilt.
    #[test]
    fn the_installed_linker_is_named_by_its_inputs() {
        let build_root = tempfile::tempdir().unwrap();
        let install = |tree: &tempfile::TempDir| {
            let cache = compile_cache(build_root.path(), tree.path()).unwrap();
            install_linker(tree.path(), cache.path(), build_root.path()).unwrap()
        };
        let first = tree_with_linker("first");
        let same = tree_with_linker("first");
        let other = tree_with_linker("second");
        let installed = install(&first);
        assert!(installed.starts_with(build_root.path().join(LINKERS)));
        assert_eq!(run(&installed), "first");
        assert_eq!(install(&same), installed);
        let changed = install(&other);
        assert_ne!(changed, installed);
        assert_eq!(run(&changed), "second");
        // Editing a tree's linker moves that tree to a new linker as well.
        std::fs::write(
            first.path().join("linker/src/main.rs"),
            "fn main() { print!(\"edited\"); }\n",
        )
        .unwrap();
        let edited = install(&first);
        assert_ne!(edited, installed);
        assert_eq!(run(&edited), "edited");
        assert_eq!(run(&installed), "first");
    }

    /// An installation marks the linker used now, however long ago Cargo
    /// built it, so `cargo hil sweep` does not take it from a build.
    #[test]
    fn an_installation_renews_the_linkers_last_use() {
        let build_root = tempfile::tempdir().unwrap();
        let tree = tree_with_linker("old");
        let cache = compile_cache(build_root.path(), tree.path()).unwrap();
        let built = build_linker(tree.path(), &cache.path().join("image-linker")).unwrap();
        // A build days ago, of sources older still.
        let days_ago = |days: u64| {
            std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600)
        };
        for (path, days) in [(tree.path().join("linker/src/main.rs"), 11), (built, 10)] {
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(days_ago(days))
                .unwrap();
        }
        let installed = install_linker(tree.path(), cache.path(), build_root.path()).unwrap();
        let used = modified(&installed).unwrap();
        assert!(
            used.elapsed()
                .is_ok_and(|age| age < std::time::Duration::from_secs(60)),
            "{used:?}"
        );
    }

    /// Threads installing one identity at once all get the whole linker.
    #[test]
    fn concurrent_installs_of_one_linker_agree() {
        let build_root = tempfile::tempdir().unwrap();
        let trees: Vec<_> = (0..4).map(|_| tree_with_linker("same")).collect();
        let installed: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = trees
                .iter()
                .map(|tree| {
                    let build_root = build_root.path();
                    scope.spawn(move || {
                        let cache = compile_cache(build_root, tree.path()).unwrap();
                        install_linker(tree.path(), cache.path(), build_root).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        assert!(installed.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(run(&installed[0]), "same");
        // Every tree's own build is whole too.
        for tree in &trees {
            let cache = compile_cache(build_root.path(), tree.path()).unwrap();
            assert_eq!(
                run(&cache.path().join("image-linker/debug/oer-image-linker")),
                "same"
            );
        }
    }

    /// A linker input written after the linker was built refuses its
    /// installation: its identity would name sources it was not built from.
    #[test]
    fn an_input_newer_than_the_built_linker_refuses_its_installation() {
        let build_root = tempfile::tempdir().unwrap();
        let tree = tree_with_linker("first");
        let cache = compile_cache(build_root.path(), tree.path()).unwrap();
        install_linker(tree.path(), cache.path(), build_root.path()).unwrap();
        // A manifest or lock Cargo compares by content may be newer than an
        // up-to-date build, as after a rebase.
        for file in ["Cargo.toml", "Cargo.lock"] {
            std::fs::File::open(tree.path().join(file))
                .unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
                .unwrap();
        }
        install_linker(tree.path(), cache.path(), build_root.path()).unwrap();
        // A source dated after any build Cargo can make of it now.
        std::fs::File::open(tree.path().join("linker/src/main.rs"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let error = install_linker(tree.path(), cache.path(), build_root.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("changed after the image linker"), "{error}");
    }

    /// A tree with an `app` binary that prints the `MARKER` of its path
    /// package `marker`: the same relative layout in every tree.
    fn tree_with_marker(marker: &str) -> tempfile::TempDir {
        let tree = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = tree.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"app\", \"marker\"]\nresolver = \"3\"\n",
        );
        write(
            "marker/Cargo.toml",
            "[package]\nname = \"marker\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write(
            "marker/src/lib.rs",
            &format!("pub const MARKER: &str = \"{marker}\";\n"),
        );
        write(
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nmarker = { path = \"../marker\" }\n",
        );
        write(
            "app/src/main.rs",
            "fn main() { print!(\"{}\", marker::MARKER); }\n",
        );
        tree
    }

    /// Build `tree`'s `app` into `cache` and return what it prints.
    fn built_marker(tree: &Path, cache: &Path) -> String {
        let mut command = crate::command(Tool::Cargo).unwrap();
        command
            .current_dir(tree)
            .args(["build", "--quiet", "--offline", "-p", "app"])
            .arg("--manifest-path")
            .arg(tree.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(cache)
            .env_remove("CARGO_TARGET_DIR");
        oer_process::run(&mut command).unwrap();
        run(&cache.join("debug/app"))
    }

    fn older(path: &Path) {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
    }

    /// The defect a per-tree cache prevents: a second tree whose diverging
    /// source is older than the first tree's build reuses the first tree's
    /// unit in a shared cache, and compiles its own in its own cache.
    #[test]
    fn each_tree_compiles_its_own_sources_in_its_own_cache() {
        let build_root = tempfile::tempdir().unwrap();
        let first = tree_with_marker("first");
        let second = tree_with_marker("second");
        // The second tree's change predates the first tree's build.
        older(&second.path().join("marker/src/lib.rs"));
        let first_cache = compile_cache(build_root.path(), first.path()).unwrap();
        let second_cache = compile_cache(build_root.path(), second.path()).unwrap();
        assert_ne!(first_cache.path(), second_cache.path());
        assert_eq!(built_marker(first.path(), first_cache.path()), "first");
        assert_eq!(built_marker(second.path(), second_cache.path()), "second");
        // One cache for both trees serves the first tree's unit to the second.
        let shared = tempfile::tempdir().unwrap();
        assert_eq!(built_marker(first.path(), shared.path()), "first");
        assert_eq!(built_marker(second.path(), shared.path()), "first");
    }

    /// One tree moved away and another moved to its path: the second finds
    /// the first's cache by its path, tells it apart by the root's identity
    /// and starts afresh instead of reusing its units.
    #[test]
    fn a_different_tree_at_the_same_path_starts_a_fresh_cache() {
        let build_root = tempfile::tempdir().unwrap();
        let place = tempfile::tempdir().unwrap();
        let path = place.path().join("worktree");
        let first = tree_with_marker("first");
        let second = tree_with_marker("second");
        older(&second.path().join("marker/src/lib.rs"));
        // `mv` keeps modification times, as `git worktree move` does.
        std::fs::rename(first.path(), &path).unwrap();
        let cache = compile_cache(build_root.path(), &path).unwrap();
        assert_eq!(built_marker(&path, cache.path()), "first");
        let moved_away = place.path().join("elsewhere");
        std::fs::rename(&path, &moved_away).unwrap();
        std::fs::rename(second.path(), &path).unwrap();
        // The first tree's cache, at the path's name, records another tree.
        let recorded = compile_caches(build_root.path()).unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].is_gone());
        let again = compile_cache(build_root.path(), &path).unwrap();
        assert_eq!(again.path(), cache.path());
        assert!(!compile_caches(build_root.path()).unwrap()[0].is_gone());
        assert_eq!(built_marker(&path, again.path()), "second");
        // A held cache of the other tree is never replaced under its build.
        let held = again.lock().unwrap();
        std::fs::rename(&path, place.path().join("third")).unwrap();
        std::fs::rename(&moved_away, &path).unwrap();
        let error = compile_cache(build_root.path(), &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("a build holds"), "{error}");
        drop(held);
        // Neither renamed tempdir is removed by its guard any more.
        let _ = std::fs::remove_dir_all(place.path());
    }

    #[test]
    fn a_tree_has_one_cache_that_names_it() {
        let build_root = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let link = build_root.path().join("link");
        std::os::unix::fs::symlink(tree.path(), &link).unwrap();
        let cache = compile_cache(build_root.path(), tree.path()).unwrap();
        // Every spelling of the tree's path names its one cache.
        assert_eq!(
            compile_cache(build_root.path(), &link).unwrap().path(),
            cache.path()
        );
        let other_cache = compile_cache(build_root.path(), other.path()).unwrap();
        let canonical = |path: &Path| Some(std::fs::canonicalize(path).unwrap());
        let recorded: Vec<_> = compile_caches(build_root.path())
            .unwrap()
            .into_iter()
            .map(|recorded| {
                (
                    recorded.cache.clone(),
                    recorded.tree.clone(),
                    recorded.is_gone(),
                )
            })
            .collect();
        assert_eq!(recorded, {
            let mut expected = vec![
                (cache.path().to_owned(), canonical(tree.path()), false),
                (
                    other_cache.path().to_owned(),
                    canonical(other.path()),
                    false,
                ),
            ];
            expected.sort();
            expected
        });
        // A build holding the cache keeps it.
        let held = cache.lock().unwrap();
        assert!(!remove_compile_cache(cache.path()).unwrap());
        assert!(cache.path().is_dir());
        drop(held);
        // An orphan's removal keeps the cache of a tree that exists.
        assert!(!remove_orphaned_compile_cache(cache.path()).unwrap());
        assert!(cache.path().is_dir());
        assert!(remove_compile_cache(cache.path()).unwrap());
        assert!(!cache.path().exists());
        assert!(!remove_compile_cache(cache.path()).unwrap());
        // Nothing of it is left aside, and the next build starts afresh.
        assert_eq!(
            compile_caches(build_root.path()).unwrap().len(),
            1,
            "only the other tree's cache"
        );
        let again = compile_cache(build_root.path(), tree.path()).unwrap();
        assert!(again.path().join(CACHE_TREE_FILE).is_file());
        drop(again.lock().unwrap());
        let gone = tempfile::tempdir().unwrap();
        let orphan = compile_cache(build_root.path(), gone.path()).unwrap();
        drop(gone);
        assert!(remove_orphaned_compile_cache(orphan.path()).unwrap());
        assert!(!orphan.path().exists());
        assert!(
            std::fs::read_dir(build_root.path().join(COMPILE_CACHES))
                .unwrap()
                .flatten()
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".removing-"))
        );
    }

    /// A build waiting for a cache that is moved aside for removal takes the
    /// lock of the cache recreated at its path, and records its tree there,
    /// so the next sweep keeps it.
    #[test]
    fn a_lock_taken_on_a_cache_moved_aside_is_taken_again() {
        let build_root = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let cache = compile_cache(build_root.path(), tree.path()).unwrap();
        let held = cache.lock().unwrap();
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| cache.lock().unwrap());
            // Let the waiter block on the old lock, then remove the cache as
            // a sweep does: move it aside, let its lock go, delete it.
            std::thread::sleep(std::time::Duration::from_millis(300));
            let aside = cache.path().with_file_name(".removing-test");
            std::fs::rename(cache.path(), &aside).unwrap();
            drop(held);
            std::fs::remove_dir_all(&aside).unwrap();
            let lock = waiting.join().unwrap();
            use std::os::unix::fs::MetadataExt;
            let (now, locked) = (
                std::fs::metadata(cache.path().join("build.lock")).unwrap(),
                lock.file().metadata().unwrap(),
            );
            assert_eq!((now.dev(), now.ino()), (locked.dev(), locked.ino()));
            let recorded = compile_caches(build_root.path()).unwrap();
            assert_eq!(recorded.len(), 1);
            assert!(!recorded[0].is_gone());
        });
    }

    /// A tree whose path is not UTF-8 is recorded and found as itself.
    #[test]
    fn a_tree_outside_utf8_is_recorded() {
        use std::os::unix::ffi::OsStrExt;
        let build_root = tempfile::tempdir().unwrap();
        let place = tempfile::tempdir().unwrap();
        let tree = place.path().join(std::ffi::OsStr::from_bytes(b"tree-\xff"));
        std::fs::create_dir(&tree).unwrap();
        let cache = compile_cache(build_root.path(), &tree).unwrap();
        let recorded = compile_caches(build_root.path()).unwrap();
        assert_eq!(recorded[0].tree.as_deref(), Some(cache.tree()));
        assert!(!recorded[0].is_gone());
        assert!(!remove_orphaned_compile_cache(cache.path()).unwrap());
    }

    #[test]
    fn dep_info_paths_keep_escaped_spaces() {
        assert_eq!(
            dep_info_paths("/t/app: /a/b.rs /c\\ d/e.rs\n\n/a/b.rs:\n"),
            [PathBuf::from("/a/b.rs"), PathBuf::from("/c d/e.rs")]
        );
    }
}
