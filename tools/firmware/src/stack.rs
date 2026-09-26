use std::{env, path::Path, process::Command};

use oer_memory_report::{StackBudget, StackReport, analyze_stack};

use crate::{Result, TARGET};

pub fn enable_stack_checks(command: &mut Command, budget: &StackBudget) {
    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    for required in [
        "-Z emit-stack-sizes".to_owned(),
        format!("-Z move-size-limit={}", budget.max_move_bytes),
        "-D large-assignments".to_owned(),
    ] {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(&required);
    }
    // The pinned project toolchain supports this rustc metadata flag, but it
    // remains unstable. Image construction enables this compiler capability;
    // the resulting ELF section is consumed by a safe host-side parser.
    command
        .env("RUSTC_BOOTSTRAP", "1")
        .env("RUSTFLAGS", rustflags);
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

pub fn analyze_elf_stack(elf: &Path, budget: &StackBudget) -> Result<StackReport> {
    Ok(analyze_stack(elf, budget)?)
}

#[cfg(test)]
mod tests {
    use super::{c_flag_variables, with_stack_sizes};

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
}
