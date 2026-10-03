//! Cargo build-script helpers that bind the linker scripts to this layout.
//!
//! The scripts under `platform/esp32s31/linker` read every address, size and
//! header constant of this crate through `--defsym` symbols, so the linked
//! images, the bootstrap and the host auditor share one definition.

extern crate std;

use crate::{memory, stage_two};
use std::{path::Path, println};

fn defsym(bin: &str, name: &str, value: u32) {
    println!("cargo:rustc-link-arg-bin={bin}=--defsym={name}={value}");
}

fn link(bin: &str, linker_dir: &Path, scripts: &[&str], entry: &str) {
    for script in scripts {
        let path = linker_dir.join(script);
        assert!(
            path.is_file(),
            "platform linker script {} is missing; pass the platform `linker` directory",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rustc-link-search={}", linker_dir.display());
    // `--emit-relocs` keeps the link's relocations in the ELF: the stack
    // analysis reads jump tables, function addresses and calls from them
    // instead of reconstructing them from machine code.
    for argument in ["-Trom/esp32s31-eco0.x", entry, "--nmagic", "--emit-relocs"] {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
    for (name, region) in [("SRAM", memory::SRAM), ("PSRAM", memory::PSRAM)] {
        defsym(bin, &std::format!("{name}_ORIGIN"), region.origin);
        defsym(bin, &std::format!("{name}_LENGTH"), region.length);
    }
    defsym(bin, "RUNTIME_PSRAM_ORIGIN", memory::RUNTIME_PSRAM.origin);
    defsym(bin, "RUNTIME_PSRAM_LENGTH", memory::RUNTIME_PSRAM.length);
}

/// Links binary `bin` as a stage-two runtime: code, data and task stacks in
/// PSRAM, interrupt entries, stacks and DMA state in internal SRAM.
pub fn configure_runtime(bin: &str, linker_dir: &Path) {
    link(
        bin,
        linker_dir,
        &[
            "rom/esp32s31-eco0.x",
            "runtime/link.x",
            "runtime/memory.x",
            "runtime/sections.x",
        ],
        "-Truntime/link.x",
    );
    for (name, value) in [
        ("STAGE_TWO_MAGIC", stage_two::MAGIC),
        ("STAGE_TWO_ABI_VERSION", stage_two::ABI_VERSION),
        ("STAGE_TWO_HEADER_BYTES", stage_two::HEADER_BYTES as u32),
        (
            "CPU0_PSRAM_TASK_STACK_BYTES",
            memory::CPU0_PSRAM_TASK_STACK_BYTES,
        ),
        ("IRQ_STACK_BYTES", memory::IRQ_STACK_BYTES),
    ] {
        defsym(bin, name, value);
    }
    println!("cargo:rerun-if-env-changed={LAYOUT_SEED_ENV}");
    let seed = std::env::var(LAYOUT_SEED_ENV).ok();
    let seed = layout_seed(seed.as_deref()).unwrap_or_else(|error| panic!("{error}"));
    for argument in shuffle_arguments(seed) {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
}

/// Environment variable that selects a reproducible alternative placement of
/// ordinary code and read-only data.
///
/// Throughput of code that runs from cached external memory depends on where
/// the linker happens to put each function: an unrelated dependency change
/// re-sorts them and moves the cost per byte by tens of percent. A seed makes
/// the linker shuffle the ordinary `.text.*` and `.rodata.*` input sections,
/// so a performance comparison can measure several placements of the same
/// source instead of one accidental one. The explicitly placed ISR, hot and
/// critical sections keep their names and so their placement. Unset, the
/// linker keeps its natural order.
pub const LAYOUT_SEED_ENV: &str = "OER_LAYOUT_SEED";

/// Parse a layout seed; zero is rejected because the linker reads it as
/// "random", which no build record could reproduce.
pub fn layout_seed(value: Option<&str>) -> Result<Option<u32>, std::string::String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.parse::<u32>() {
        Ok(0) | Err(_) => Err(std::format!(
            "{LAYOUT_SEED_ENV}={value:?} must be a nonzero decimal u32"
        )),
        Ok(seed) => Ok(Some(seed)),
    }
}

/// Linker arguments that shuffle ordinary code and read-only data by `seed`.
pub fn shuffle_arguments(seed: Option<u32>) -> std::vec::Vec<std::string::String> {
    seed.into_iter()
        .flat_map(|seed| {
            [".text.*", ".rodata.*"].map(|glob| std::format!("--shuffle-sections={glob}={seed}"))
        })
        .collect()
}

/// Links binary `bin` as the Flash-resident bootstrap.
pub fn configure_bootstrap(bin: &str, linker_dir: &Path) {
    link(
        bin,
        linker_dir,
        &[
            "rom/esp32s31-eco0.x",
            "bootstrap/link.x",
            "bootstrap/memory.x",
            "bootstrap/sections.x",
            "bootstrap/flash-sections.x",
        ],
        "-Tbootstrap/link.x",
    );
    defsym(
        bin,
        "BOOTSTRAP_FLASH_TEXT_ORIGIN",
        memory::BOOTSTRAP_FLASH_TEXT.origin,
    );
    defsym(
        bin,
        "BOOTSTRAP_FLASH_TEXT_LENGTH",
        memory::BOOTSTRAP_FLASH_TEXT.length,
    );
    defsym(
        bin,
        "SECOND_STAGE_LOADER_START",
        memory::SECOND_STAGE_LOADER_START,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_nonzero_seed_shuffles_and_it_is_reproducible() {
        assert_eq!(layout_seed(None), Ok(None));
        assert!(shuffle_arguments(None).is_empty());
        assert!(layout_seed(Some("0")).is_err());
        assert!(layout_seed(Some("-1")).is_err());
        assert!(layout_seed(Some("seven")).is_err());
        let seed = layout_seed(Some("7")).unwrap();
        assert_eq!(
            shuffle_arguments(seed),
            [
                "--shuffle-sections=.text.*=7",
                "--shuffle-sections=.rodata.*=7"
            ]
        );
    }
}
