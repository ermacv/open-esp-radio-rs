pub(crate) fn enable_stack_checks(
    command: &mut std::process::Command,
    budget: &open_esp_radio_memory_report::StackBudget,
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
        );
}
pub(crate) fn analyze_elf_stack(
    elf: &std::path::Path,
    budget: &open_esp_radio_memory_report::StackBudget,
) -> crate::Result<open_esp_radio_memory_report::StackReport> {
    oer_firmware::stack::analyze_elf_stack(elf, budget)
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error })
}
