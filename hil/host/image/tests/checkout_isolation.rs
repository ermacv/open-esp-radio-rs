//! Two checkouts whose sources of one package diverge build one after the
//! other on the same host; each build holds its own code (#793).
//!
//! Cargo names a package of the workspace it builds by its path relative to
//! the workspace root, the same in every checkout, and judges freshness by
//! modification times. A compile cache the checkouts shared would hand the
//! second checkout the first checkout's unit, so each test gives the second
//! checkout a change that predates the first checkout's build:
//!
//! - the image pipeline links with the image linker, a package of the root
//!   workspace built for the host: each checkout's linker adds its own
//!   symbol to the boot-smoke image;
//! - `oer-esp32s31-ieee80211-form`'s placement test, which builds its
//!   example of the root workspace for the chip: each checkout's `oer-memory`
//!   holds its own marker in the library that build compiled.
//!
//! They build firmware, so they are ignored by default; the
//! `image-isolation` check runs them with a disposable `OER_BUILD_ROOT`.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use oer_hil_schema::image::{FeatureDelta, ImageClass};

const TARGET: &str = "riscv32imafc-unknown-none-elf";

/// A detached worktree of this repository at `HEAD`, removed on drop.
struct Worktree {
    repository: PathBuf,
    path: PathBuf,
}

impl Worktree {
    fn add(repository: &Path, path: PathBuf) -> Self {
        oer_process::run(
            oer_process::git::command(repository)
                .args(["worktree", "add", "--quiet", "--detach"])
                .arg(&path)
                .arg("HEAD"),
        )
        .unwrap();
        Self {
            repository: repository.to_owned(),
            path,
        }
    }

    /// Replace `from` with `to` in the file `relative`, then give the file
    /// the modification time `modified`.
    fn edit(&self, relative: &str, from: &str, to: &str, modified: SystemTime) {
        let file = self.path.join(relative);
        let source = std::fs::read_to_string(&file).unwrap();
        assert!(source.contains(from), "{relative} holds {from:?}");
        self.write(relative, &source.replacen(from, to, 1), modified);
    }

    /// Append `text` to the file `relative`, then give the file the
    /// modification time `modified`.
    fn append(&self, relative: &str, text: &str, modified: SystemTime) {
        let source = std::fs::read_to_string(self.path.join(relative)).unwrap();
        self.write(relative, &format!("{source}{text}"), modified);
    }

    fn write(&self, relative: &str, text: &str, modified: SystemTime) {
        let file = self.path.join(relative);
        std::fs::write(&file, text).unwrap();
        std::fs::File::open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = oer_process::run(
            oer_process::git::command(&self.repository)
                .args(["worktree", "remove", "--force"])
                .arg(&self.path),
        );
    }
}

/// Two worktrees of this repository; `edit(tree, marker, modified)` gives
/// each its marker, the second's change predating the first's build.
fn checkouts(
    edit: impl Fn(&Worktree, &str, SystemTime),
) -> (tempfile::TempDir, Worktree, Worktree) {
    assert!(
        std::env::var_os(oer_toolchain::image::BUILD_ROOT_ENV).is_some_and(|root| !root.is_empty()),
        "run with a disposable {}",
        oer_toolchain::image::BUILD_ROOT_ENV
    );
    let repository = oer_process::built_root();
    let trees = tempfile::tempdir().unwrap();
    let first = Worktree::add(&repository, trees.path().join("first"));
    let second = Worktree::add(&repository, trees.path().join("second"));
    let now = SystemTime::now();
    edit(&first, "oer_checkout_isolation_first", now);
    edit(
        &second,
        "oer_checkout_isolation_second",
        now - Duration::from_secs(3600),
    );
    (trees, first, second)
}

fn holds(bytes: &[u8], marker: &str) -> bool {
    bytes
        .windows(marker.len())
        .any(|window| window == marker.as_bytes())
}

/// Assert that `first` holds only the first marker and `second` only the
/// second.
fn each_holds_its_own(first: &[u8], second: &[u8]) {
    assert!(holds(first, "oer_checkout_isolation_first"));
    assert!(!holds(first, "oer_checkout_isolation_second"));
    assert!(
        holds(second, "oer_checkout_isolation_second")
            && !holds(second, "oer_checkout_isolation_first"),
        "the second checkout's build holds the first checkout's code"
    );
}

/// The boot-smoke image's bootstrap ELF built from `root`.
fn bootstrap_elf(root: &Path) -> Vec<u8> {
    let artifacts = oer_hil_image::build(
        root,
        "esp32s31",
        ImageClass::BootSmoke,
        None,
        &FeatureDelta::default(),
    )
    .unwrap();
    std::fs::read(
        artifacts
            .bundle
            .path(oer_image_bundle::files::BOOTSTRAP_ELF),
    )
    .unwrap()
}

#[test]
#[ignore = "builds two firmware images; the image-isolation check runs it"]
fn the_pipeline_links_each_checkout_with_its_own_linker() {
    let arguments = "std::env::args_os().skip(1).collect(),";
    let (_trees, first, second) = checkouts(|tree, marker, modified| {
        tree.edit(
            "tools/image/linker/src/main.rs",
            arguments,
            &format!(
                "std::env::args_os().skip(1)\
                 .chain([std::ffi::OsString::from(\"--defsym={marker}=0\")]).collect(),"
            ),
            modified,
        );
    });
    each_holds_its_own(&bootstrap_elf(&first.path), &bootstrap_elf(&second.path));
}

/// The `oer-memory` libraries the placement test of
/// `oer-esp32s31-ieee80211-form` compiled for the chip when run in the tree
/// at `root`, from its compile cache: the test's own build path.
fn placement_memory(root: &Path) -> Vec<u8> {
    // The test binary builds in the tree's own target directory: one built
    // for another tree would build that tree's example.
    oer_process::run(
        oer_toolchain::cargo_in(root)
            .args(["test", "--quiet", "--offline", "--locked"])
            .args(["-p", "oer-esp32s31-ieee80211-form", "--test", "placement"])
            .env_remove("CARGO_TARGET_DIR"),
    )
    .unwrap();
    let cache = oer_toolchain::image::compile_cache(
        &oer_toolchain::image::host_build_root().unwrap(),
        root,
    )
    .unwrap();
    let deps = cache.path().join(TARGET).join("release/deps");
    let mut libraries = Vec::new();
    for entry in std::fs::read_dir(&deps).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("liboer_memory-") && name.ends_with(".rlib") {
            libraries.extend(std::fs::read(entry.path()).unwrap());
        }
    }
    assert!(
        !libraries.is_empty(),
        "{} holds no oer-memory",
        deps.display()
    );
    libraries
}

#[test]
#[ignore = "runs the placement test in two checkouts; the image-isolation check runs it"]
fn the_placement_test_compiles_from_its_own_checkout() {
    let (_trees, first, second) = checkouts(|tree, marker, modified| {
        tree.append(
            "crates/memory/src/lib.rs",
            &format!(
                "\n/// The checkout this library was compiled from.\n\
                 pub static CHECKOUT_ISOLATION: &str = \"{marker}\";\n"
            ),
            modified,
        );
    });
    each_holds_its_own(
        &placement_memory(&first.path),
        &placement_memory(&second.path),
    );
}
