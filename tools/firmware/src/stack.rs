use std::{env, path::Path, process::Command};

use open_esp_radio_memory_report::{StackBudget, StackReport, analyze_stack};

use crate::Result;

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
}

pub fn analyze_elf_stack(elf: &Path, budget: &StackBudget) -> Result<StackReport> {
    Ok(analyze_stack(elf, budget)?)
}
