//! Compiler configuration shared by every image build: the single owner of
//! the ESP32-S31 image compiler flags. No `.cargo/config.toml` carries Rust
//! flags for an image; `cargo xtask build firmware`, the HIL image builder
//! and their type checks all apply [`configure_image_compiler`].
//!
//! `RUSTC_BOOTSTRAP=1` is set here, on the image's Cargo command only, and
//! exists for the unstable `-Z` flags alone (`emit-stack-sizes`,
//! `move-size-limit`, `share-generics`) on the exact stable toolchain that
//! `rust-toolchain.toml` pins.
//!
//! Every image links through the image linker (`tools/image-linker`), which
//! refuses an input section named as zero-initialized that carries bytes and
//! otherwise runs the target's `rust-lld` with the same arguments.

use std::{
    env, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::stack::StackPolicy;

/// Configure `command`, a Cargo build of an image, with the compiler flags
/// every image shares, building the image linker first. The stack policy
/// gives the move limit and the headroom reserves the runtime's stack
/// painting reads at compile time.
pub fn configure_image_compiler(
    command: &mut Command,
    policy: &StackPolicy,
    target: &str,
) -> io::Result<()> {
    let linker = image_linker()?;
    // The pinned stable toolchain supports these flags, but they remain
    // unstable: RUSTC_BOOTSTRAP enables them for this command alone. The
    // stack-size ELF section is consumed by a safe host-side parser.
    command.env("RUSTC_BOOTSTRAP", "1").env(
        "RUSTFLAGS",
        image_rustflags(env::var("RUSTFLAGS").ok(), policy.max_move_bytes, &linker)?,
    );
    for (variable, value) in policy.runtime_environment() {
        command.env(variable, value);
    }
    // C and C++ that build scripts compile for the image (through the `cc`
    // and `cmake` crates) emit the same `.stack_sizes` section, so the stack
    // gate has their frames as well.
    for variable in c_flag_variables(target) {
        let flags = with_stack_sizes(env::var(&variable).ok());
        command.env(variable, flags);
    }
    Ok(())
}

/// Build the image linker for the host and return its path. The build
/// leaves the image's compiler environment behind: it is a host tool.
fn image_linker() -> io::Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_dir = root.join("target/image-linker");
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
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
        .arg(&target_dir)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTC_BOOTSTRAP")
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "building the image linker failed: {status}"
        )));
    }
    Ok(target_dir.join("debug/oer-image-linker"))
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
fn image_rustflags(
    existing: Option<String>,
    max_move_bytes: u64,
    linker: &Path,
) -> io::Result<String> {
    let linker = linker
        .to_str()
        .filter(|path| !path.contains(char::is_whitespace))
        .ok_or_else(|| {
            io::Error::other(format!(
                "the image linker's path must be UTF-8 without spaces: {}",
                linker.display()
            ))
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
    use super::{c_flag_variables, image_rustflags, with_stack_sizes};

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
            image_rustflags(
                Some("-C debuginfo=1".into()),
                4096,
                std::path::Path::new("/repo/target/image-linker/debug/oer-image-linker"),
            )
            .unwrap(),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -D large-assignments -Z share-generics=y \
             -C linker=/repo/target/image-linker/debug/oer-image-linker -C linker-flavor=ld.lld"
        );
        // A path RUSTFLAGS would split is refused.
        assert!(image_rustflags(None, 4096, std::path::Path::new("/a b/linker")).is_err());
    }
}
