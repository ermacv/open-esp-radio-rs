//! Cargo build-script helpers that bind the linker scripts to this layout.
//!
//! The scripts under `platform/esp32s31/linker` read every address, size and
//! header constant of this crate through `--defsym` symbols, so the linked
//! images, the bootstrap and the host auditor share one definition.

extern crate std;

use crate::{
    memory::{self, CodePlacement, DataPlacement, RuntimeProfile},
    stage_two,
};
use std::{path::Path, println};

fn defsym(bin: &str, name: &str, value: u32) {
    println!("cargo:rustc-link-arg-bin={bin}=--defsym={name}={value}");
}

fn link(bin: &str, linker_dir: &Path, scripts: &[&str], entry: &str) {
    for script in scripts {
        println!(
            "cargo:rerun-if-changed={}",
            linker_dir.join(script).display()
        );
    }
    println!("cargo:rustc-link-search={}", linker_dir.display());
    for argument in ["-Trom/esp32s31-eco0.x", entry, "--nmagic"] {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
    for (name, region) in [("SRAM", memory::SRAM), ("PSRAM", memory::PSRAM)] {
        defsym(bin, &std::format!("{name}_ORIGIN"), region.origin);
        defsym(bin, &std::format!("{name}_LENGTH"), region.length);
    }
    defsym(bin, "RUNTIME_PSRAM_ORIGIN", memory::RUNTIME_PSRAM.origin);
    defsym(bin, "RUNTIME_PSRAM_LENGTH", memory::RUNTIME_PSRAM.length);
    defsym(
        bin,
        "RUNTIME_FLASH_CODE_ORIGIN",
        memory::RUNTIME_FLASH_CODE.origin,
    );
}

/// Links binary `bin` as a stage-two runtime placed by `profile`.
pub fn configure_runtime(bin: &str, linker_dir: &Path, profile: RuntimeProfile) {
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
    let code = profile.code_region();
    let data = profile.data_region();
    for (name, value) in [
        (
            "RUNTIME_CODE_IN_PSRAM",
            u32::from(profile.code() == CodePlacement::Psram),
        ),
        ("RUNTIME_CODE_ORIGIN", code.origin),
        ("RUNTIME_CODE_LENGTH", code.length),
        (
            "RUNTIME_DATA_IN_PSRAM",
            u32::from(profile.data() == DataPlacement::Psram),
        ),
        ("RUNTIME_DATA_ORIGIN", data.origin),
        ("RUNTIME_DATA_LENGTH", data.length),
        ("PSRAM_TASK_STACKS", u32::from(profile.psram_task_stack())),
        ("STAGE_TWO_MAGIC", stage_two::MAGIC),
        ("STAGE_TWO_ABI_VERSION", stage_two::ABI_VERSION),
        ("STAGE_TWO_HEADER_BYTES", stage_two::HEADER_BYTES as u32),
        (
            "CPU0_PSRAM_TASK_STACK_BYTES",
            memory::CPU0_PSRAM_TASK_STACK_BYTES,
        ),
        ("IRQ_STACK_BYTES", memory::IRQ_STACK_BYTES),
        (
            "MIN_SRAM_THREAD_STACK_BYTES",
            memory::MIN_SRAM_THREAD_STACK_BYTES,
        ),
    ] {
        defsym(bin, name, value);
    }
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
            "bootstrap/psram-sections.x",
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
}
