use crate::Result;
use std::{collections::BTreeMap, env, ffi::OsString, fs, path::Path, process::Command};
pub const TARGET: &str = "riscv32imafc-unknown-none-elf";
pub const BOOTSTRAP_BIN: &str = "oer-esp32s31-bootstrap";
fn program_from_env(variable: &str, fallback: &str) -> OsString {
    env::var_os(variable).unwrap_or_else(|| fallback.into())
}
pub fn audit_runtime(elf: &Path, binary: &Path, psram_task_stack: bool) -> Result<String> {
    let output = Command::new(program_from_env("LLVM_NM", "llvm-nm"))
        .args(["--defined-only", "--numeric-sort"])
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err("llvm-nm failed while auditing runtime placement".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut symbols = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(address) = words.next() else {
            continue;
        };
        let Some(_kind) = words.next() else { continue };
        let Some(name) = words.next() else { continue };
        if let Ok(address) = u64::from_str_radix(address, 16) {
            symbols.insert(name.to_owned(), address);
        }
    }
    let symbol = |name: &str| -> Result<u64> {
        symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("runtime ELF lacks `{name}`").into())
    };

    let image_start = symbol("__runtime_image_start")?;
    let payload_end = symbol("__runtime_payload_end")?;
    let text_start = symbol("__runtime_text_start")?;
    let text_end = symbol("__runtime_text_end")?;
    let entry = symbol("_runtime_start")?;
    let data_start = symbol("__runtime_data_start")?;
    let bss_end = symbol("__runtime_data_bss_end")?;
    let isr_start = symbol("__runtime_isr_start")?;
    let isr_end = symbol("__runtime_isr_end")?;
    let critical_start = symbol("__runtime_critical_data_start")?;
    let critical_bss_end = symbol("__runtime_critical_bss_end")?;
    let dma_start = symbol("__runtime_dma_data_start")?;
    let dma_end = symbol("__runtime_dma_bss_end")?;
    let stack_bottom = symbol("_stack_end")?;
    let stack_top = symbol("_stack_start")?;
    let binary_bytes = fs::metadata(binary)?.len();
    let in_sram = |start: u64, end: u64| start >= 0x2f00_0000 && end >= start && end <= 0x2f07_afc0;
    let stack_placement_valid = if psram_task_stack {
        let cpu0_irq_bottom = symbol("__runtime_cpu0_irq_stack_bottom")?;
        let cpu0_irq_top = symbol("__runtime_cpu0_irq_stack_top")?;
        let cpu1_irq_bottom = symbol("__runtime_cpu1_irq_stack_bottom")?;
        let cpu1_irq_top = symbol("__runtime_cpu1_irq_stack_top")?;
        let trap_entry = symbol("_start_trap")?;
        let irq_entry_first = symbol("_runtime_psram_irq_entry_1")?;
        let irq_entry_last = symbol("_runtime_psram_irq_entry_47")?;
        let mtvt_source = symbol("_runtime_psram_mtvt_source")?;
        let cpu0_mtvt = symbol("_mtvt_table")?;
        let cpu1_mtvt = symbol("_mtvt_table2")?;
        let all_irq_entries_in_sram = (1..=47).all(|number| {
            symbols
                .get(&format!("_runtime_psram_irq_entry_{number}"))
                .is_some_and(|entry| in_sram(*entry, *entry + 4))
        });
        stack_bottom >= 0x5000_0000
            && stack_top <= 0x5100_0000
            && stack_top.saturating_sub(stack_bottom) == 0x3_0000
            && in_sram(cpu0_irq_bottom, cpu0_irq_top)
            && in_sram(cpu1_irq_bottom, cpu1_irq_top)
            && cpu0_irq_top.saturating_sub(cpu0_irq_bottom) == 0x8000
            && cpu1_irq_top.saturating_sub(cpu1_irq_bottom) == 0x8000
            && in_sram(trap_entry, trap_entry + 4)
            && in_sram(irq_entry_first, irq_entry_first + 4)
            && in_sram(irq_entry_last, irq_entry_last + 4)
            && in_sram(mtvt_source, mtvt_source + 48 * 4)
            && in_sram(cpu0_mtvt, cpu0_mtvt + 48 * 4)
            && in_sram(cpu1_mtvt, cpu1_mtvt + 48 * 4)
            && all_irq_entries_in_sram
    } else {
        stack_top == 0x2f07_afc0 && stack_top.saturating_sub(stack_bottom) >= 0x1_0000
    };
    if image_start != 0x5001_0000
        || payload_end <= image_start
        || payload_end - image_start != binary_bytes
        || entry < text_start
        || entry >= text_end
        || data_start < 0x5000_0000
        || bss_end > 0x5100_0000
        || !in_sram(isr_start, isr_end)
        || !in_sram(critical_start, critical_bss_end)
        || !in_sram(dma_start, dma_end)
        || !stack_placement_valid
    {
        return Err("runtime ELF violates the PSRAM/PSRAM placement contract".into());
    }
    if psram_task_stack {
        audit_psram_stack_entry_instructions(elf)?;
    }

    Ok(format!(
        "profile={}\n\
         image={image_start:#010x}..{payload_end:#010x}\n\
         text={text_start:#010x}..{text_end:#010x}\n\
         data_start={data_start:#010x}\n\
         bss_end={bss_end:#010x}\n\
         isr={isr_start:#010x}..{isr_end:#010x}\n\
         critical={critical_start:#010x}..{critical_bss_end:#010x}\n\
         dma={dma_start:#010x}..{dma_end:#010x}\n\
         stack={stack_bottom:#010x}..{stack_top:#010x}\n\
         result=PASS\n",
        if psram_task_stack {
            "psram-code-psram-data-psram-stack"
        } else {
            "psram-code-psram-data"
        }
    ))
}

fn audit_psram_stack_entry_instructions(elf: &Path) -> Result<()> {
    let mut names = vec!["_start_trap".to_owned()];
    names.extend((1..=47).map(|number| format!("_runtime_psram_irq_entry_{number}")));
    let output = Command::new(program_from_env("LLVM_OBJDUMP", "llvm-objdump"))
        .arg("-d")
        .arg(format!("--disassemble-symbols={}", names.join(",")))
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err("llvm-objdump failed while auditing PSRAM stack entries".into());
    }
    let text = String::from_utf8(output.stdout)?;
    for name in names {
        let marker = format!("<{name}>:");
        let tail = text
            .split_once(&marker)
            .map(|(_, tail)| tail)
            .ok_or_else(|| format!("runtime disassembly lacks `{name}`"))?;
        let instruction = tail
            .lines()
            .find_map(|line| line.trim().split_once(':').map(|(_, body)| body.trim()))
            .filter(|body| !body.is_empty())
            .ok_or_else(|| format!("runtime disassembly has no instruction for `{name}`"))?;
        if !instruction.contains("csrrw") || !instruction.contains("sp, mscratch, sp") {
            return Err(format!(
                "`{name}` touches the interrupted stack before swapping to SRAM: `{instruction}`"
            )
            .into());
        }
    }
    Ok(())
}

pub fn audit_application_image(path: &Path) -> Result<()> {
    const APP_DESC_OFFSET: usize = 0x20;
    const APP_DESC_MMU_PAGE_LOG2_OFFSET: usize = 180;
    let bytes = fs::read(path)?;
    let end = APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET + 1;
    if bytes.len() < end
        || bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4] != 0xabcd_5432_u32.to_le_bytes()
        || bytes[APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET] != 16
    {
        return Err("ESP application image has an invalid app descriptor or MMU page size".into());
    }
    Ok(())
}

/// Configure the common bootstrap build; callers own process execution and provenance.
pub fn bootstrap_command(command: &mut Command, root: &Path, runtime: &Path, target_dir: &Path) {
    command
        .args(["build", "--manifest-path"])
        .arg(root.join("platform/esp32s31/Cargo.toml"))
        .args(["-p", BOOTSTRAP_BIN, "--release", "--target", TARGET])
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("PSRAM_RUNTIME_BIN", runtime);
}

/// Encode an application image with the board's bootloader and partition contract.
pub fn save_image_command(command: &mut Command, root: &Path, bootstrap: &Path, output: &Path) {
    encode_image(command, root, bootstrap, output, false);
}

/// Produce a container for extracting the ROM-readable DIO bootloader.
/// The application is independently encoded as QIO; ROM and ESP-IDF have
/// different responsibilities for enabling the Flash data lines.
pub fn save_rom_image_command(command: &mut Command, root: &Path, bootstrap: &Path, output: &Path) {
    encode_image(command, root, bootstrap, output, true);
}

fn encode_image(command: &mut Command, root: &Path, bootstrap: &Path, output: &Path, rom: bool) {
    command
        .args([
            "save-image",
            "--chip",
            "esp32s31",
            "--flash-mode",
            if rom { "dio" } else { "qio" },
            "--flash-freq",
            "80mhz",
            "--flash-size",
            "16mb",
            "--mmu-page-size",
            "65536",
            "--partition-table",
        ])
        .arg(root.join("platform/esp32s31/partitions/applications.csv"))
        .args(["--target-app-partition", "ota_0"]);
    if rom {
        command.args(["--merge", "--skip-padding"]);
    }
    command.arg(bootstrap).arg(output);
}
