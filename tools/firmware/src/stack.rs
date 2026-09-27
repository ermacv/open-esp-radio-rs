use std::path::Path;

use oer_memory_report::{StackBudget, StackReport, analyze_stack};

use crate::Result;

pub fn analyze_elf_stack(elf: &Path, budget: &StackBudget) -> Result<StackReport> {
    Ok(analyze_stack(elf, budget)?)
}
