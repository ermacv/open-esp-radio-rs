pub fn configure_image_compiler(
    command: &mut std::process::Command,
    budget: &oer_memory_report::StackBudget,
    target: &str,
) -> crate::Result<()> {
    oer_esp32s31_firmware::compiler::configure_image_compiler(command, budget, target)?;
    command
        .env(
            "OPEN_RADIO_CPU0_STACK_MINIMUM_FREE_BYTES",
            budget.runtime_cpu0_minimum_free_bytes.to_string(),
        )
        .env(
            "OPEN_RADIO_CPU1_STACK_MINIMUM_FREE_BYTES",
            budget.runtime_cpu1_minimum_free_bytes.to_string(),
        )
        .env(
            "OPEN_RADIO_IRQ_STACK_MINIMUM_FREE_BYTES",
            budget.runtime_irq_minimum_free_bytes.to_string(),
        );
    Ok(())
}
pub fn analyze_elf_stack(
    elf: &std::path::Path,
    budget: &oer_memory_report::StackBudget,
) -> crate::Result<oer_memory_report::StackReport> {
    oer_esp32s31_firmware::stack::analyze_elf_stack(elf, budget)
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error })
}

/// Each hart's interrupt-stack bound of the ESP32-S31 runtime ELF at `elf`,
/// as the image build's gate computes it.
pub fn interrupt_stacks(
    root: &std::path::Path,
    elf: &std::path::Path,
) -> crate::Result<oer_esp32s31_firmware::interrupt_stack::InterruptStacks> {
    oer_esp32s31_firmware::interrupt_stack::interrupt_stacks(root, elf)
}
