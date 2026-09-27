//! Compiler configuration shared by every image build.

use std::{env, process::Command};

use oer_memory_report::StackBudget;

use crate::TARGET;

/// Configure `command`, a Cargo build of an image, with the compiler flags
/// every image shares.
pub fn configure_image_compiler(command: &mut Command, budget: &StackBudget) {
    // The pinned project toolchain supports these flags, but they remain
    // unstable. Image construction enables them; the stack-size ELF section
    // is consumed by a safe host-side parser.
    command.env("RUSTC_BOOTSTRAP", "1").env(
        "RUSTFLAGS",
        image_rustflags(env::var("RUSTFLAGS").ok(), budget.max_move_bytes),
    );
    // C and C++ that build scripts compile for the image (through the `cc`
    // and `cmake` crates) emit the same `.stack_sizes` section, so the audit
    // measures their frames as well.
    for variable in c_flag_variables() {
        let flags = with_stack_sizes(env::var(&variable).ok());
        command.env(variable, flags);
    }
}

/// The per-target C and C++ flag variables the `cc` and `cmake` crates read.
fn c_flag_variables() -> [String; 2] {
    let target = TARGET.replace('-', "_");
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
fn image_rustflags(existing: Option<String>, max_move_bytes: u64) -> String {
    let mut rustflags = existing.unwrap_or_default();
    for required in [
        "-Z emit-stack-sizes".to_owned(),
        format!("-Z move-size-limit={max_move_bytes}"),
        "-D large-assignments".to_owned(),
        "-Z share-generics=y".to_owned(),
    ] {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(&required);
    }
    rustflags
}

#[cfg(test)]
mod tests {
    use super::{c_flag_variables, image_rustflags, with_stack_sizes};

    #[test]
    fn c_builds_emit_frame_sizes_for_the_image_target() {
        assert_eq!(
            c_flag_variables(),
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
            image_rustflags(Some("-C debuginfo=1".into()), 4096),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -D large-assignments -Z share-generics=y"
        );
    }
}
