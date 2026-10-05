//! The staged image's placement contract: where the runtime's code, data,
//! interrupt entries and stacks must lie, and the application descriptor of
//! the encoded image.

use crate::Result;
use oer_esp32s31_platform_layout::memory as layout;
use std::{fs, path::Path};

/// Audit the runtime ELF `elf` and its flattened `binary` against the
/// platform's placement; returns the placement report.
pub fn audit_runtime(elf: &Path, binary: &Path) -> Result<String> {
    let bytes = fs::read(elf)?;
    let elf = oer_elf::Elf::executable(&bytes)?;
    let symbols = elf.addresses();
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
    let in_sram = |start: u64, end: u64| layout::SRAM.contains_range(start, end);
    let psram_start = u64::from(layout::PSRAM.origin);
    let psram_end = u64::from(layout::PSRAM.end());
    let irq_stack_bytes = u64::from(layout::IRQ_STACK_BYTES);
    let stack_placement_valid = {
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
                .get(format!("_runtime_psram_irq_entry_{number}").as_str())
                .is_some_and(|entry| in_sram(*entry, *entry + 4))
        });
        stack_bottom >= psram_start
            && stack_top <= psram_end
            && stack_top.saturating_sub(stack_bottom)
                == u64::from(layout::CPU0_PSRAM_TASK_STACK_BYTES)
            && in_sram(cpu0_irq_bottom, cpu0_irq_top)
            && in_sram(cpu1_irq_bottom, cpu1_irq_top)
            && cpu0_irq_top.saturating_sub(cpu0_irq_bottom) == irq_stack_bytes
            && cpu1_irq_top.saturating_sub(cpu1_irq_bottom) == irq_stack_bytes
            && in_sram(trap_entry, trap_entry + 4)
            && in_sram(irq_entry_first, irq_entry_first + 4)
            && in_sram(irq_entry_last, irq_entry_last + 4)
            && in_sram(mtvt_source, mtvt_source + 48 * 4)
            && in_sram(cpu0_mtvt, cpu0_mtvt + 48 * 4)
            && in_sram(cpu1_mtvt, cpu1_mtvt + 48 * 4)
            && all_irq_entries_in_sram
    };
    if image_start != u64::from(layout::RUNTIME_PSRAM.origin)
        || payload_end <= image_start
        || payload_end - image_start != binary_bytes
        || entry < text_start
        || entry >= text_end
        || data_start < psram_start
        || bss_end > psram_end
        || !in_sram(isr_start, isr_end)
        || !in_sram(critical_start, critical_bss_end)
        || !in_sram(dma_start, dma_end)
        || !stack_placement_valid
    {
        return Err("runtime ELF violates the PSRAM/PSRAM placement contract".into());
    }
    audit_psram_stack_entry_instructions(&elf)?;

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
        "psram-code-psram-data-psram-stack"
    ))
}

/// Each PSRAM trap and interrupt entry's first instruction swaps `sp` with
/// `mscratch` (`csrrw sp, mscratch, sp`) before anything touches the
/// interrupted stack.
fn audit_psram_stack_entry_instructions(elf: &oer_elf::Elf<'_>) -> Result<()> {
    use oer_riscv_decode::{CsrOp, Extension, Extensions, Instruction, Operand};
    const SP: u8 = 2;
    const MSCRATCH: u16 = 0x340;
    let mut names = vec!["_start_trap".to_owned()];
    names.extend((1..=47).map(|number| format!("_runtime_psram_irq_entry_{number}")));
    for name in names {
        let address = elf
            .address(&name)
            .ok_or_else(|| format!("runtime ELF lacks `{name}`"))?;
        let section = elf
            .sections()
            .find(|section| section.executable && section.contains(address))
            .ok_or_else(|| format!("`{name}` is not in executable code"))?;
        let at = usize::try_from(address - section.address)?;
        let instruction = section
            .data
            .get(at..)
            .and_then(|bytes| oer_riscv_decode::decode(bytes, Extensions::ALL))
            .map(|(instruction, _)| instruction)
            .ok_or_else(|| format!("runtime code has no instruction for `{name}`"))?;
        let swaps = matches!(
            instruction,
            Instruction::Extension(Extension::Csr {
                op: CsrOp::Write,
                dest: SP,
                source: Operand::Register(SP),
                csr: MSCRATCH,
            })
        );
        if !swaps {
            return Err(format!(
                "`{name}` touches the interrupted stack before swapping to SRAM: `{instruction}`"
            )
            .into());
        }
    }
    Ok(())
}

/// Audit an encoded application image: its ESP-IDF app descriptor and the
/// 64-KiB MMU page size, warning when it nearly fills `capacity`, the bytes
/// of its partition.
pub fn audit_application_image(bytes: &[u8], capacity: u32) -> Result<()> {
    const APP_DESC_OFFSET: usize = 0x20;
    const APP_DESC_MMU_PAGE_LOG2_OFFSET: usize = 180;
    let end = APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET + 1;
    if bytes.len() < end
        || bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4] != 0xabcd_5432_u32.to_le_bytes()
        || bytes[APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET] != 16
    {
        return Err("ESP application image has an invalid app descriptor or MMU page size".into());
    }
    if bytes.len() > capacity as usize {
        return Err(format!(
            "the application image ({} bytes) exceeds its partition ({capacity} bytes)",
            bytes.len()
        )
        .into());
    }
    if let Some(warning) =
        crate::encode::partition_budget_warning(bytes.len() as u64, u64::from(capacity))
    {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
