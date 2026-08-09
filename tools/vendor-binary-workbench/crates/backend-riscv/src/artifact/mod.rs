//! Structural ELF/archive loading and instruction decoding.
//!
//! This module deliberately does not invoke binutils. Symbol boundaries and
//! instruction bytes come from the binary containers themselves.

mod decode;
mod inventory;
mod model;
mod relocations;
mod sections;
mod symbols;

pub use decode::{
    FloatingDataInstruction, FloatingDataOperation, FloatingMemoryAccess,
    FloatingMemoryInstruction, andi_immediate, decode_floating_data_instruction,
    decode_floating_memory_instruction, decode_symbol, decode_symbol_for_analysis,
    reachable_unsupported_instructions, relocated_call_is_tail, unsupported_instruction_mnemonic,
};
pub use inventory::inspect_artifact;
pub use model::{
    AnalysisInstruction, ArtifactCodeRange, ArtifactCodeRecoveryBlocker,
    ArtifactCodeSectionCoverage, ArtifactContainerKind, ArtifactDirectControlFlowEvidence,
    ArtifactDirectControlFlowKind, ArtifactFunctionBoundaryCandidate, ArtifactInventory,
    ArtifactObjectInventory, ArtifactObjectKind, ArtifactSymbolBinding, ArtifactSymbolDefinition,
    ArtifactSymbolDefinitionState, ArtifactSymbolFact, ArtifactSymbolKind, ArtifactSymbolScope,
    ArtifactSymbolTable, ArtifactSymbolVisibility, CodeSymbolSelection, DecodedInstruction,
    ExecutableSection, MemoryRegion, RelocationKind, ReviewedCodeRange, SymbolRelocation,
    UnsupportedInstruction, UnsupportedInstructionClass,
};
pub use sections::load_executable_sections;
pub use symbols::{load_code_symbols, load_reviewed_code_ranges};

#[cfg(test)]
mod tests;
