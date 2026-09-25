pub fn enable_stack_checks(
    command: &mut std::process::Command,
    budget: &oer_memory_report::StackBudget,
) {
    oer_firmware::stack::enable_stack_checks(command, budget);
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
}
pub fn analyze_elf_stack(
    elf: &std::path::Path,
    budget: &oer_memory_report::StackBudget,
) -> crate::Result<oer_memory_report::StackReport> {
    oer_firmware::stack::analyze_elf_stack(elf, budget)
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error })
}
