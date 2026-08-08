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

pub use decode::{andi_immediate, decode_symbol, relocated_call_is_tail};
pub use inventory::inspect_artifact;
pub use model::{
    ArtifactContainerKind, ArtifactInventory, ArtifactObjectInventory, ArtifactObjectKind,
    ArtifactSymbolBinding, ArtifactSymbolDefinition, ArtifactSymbolDefinitionState,
    ArtifactSymbolFact, ArtifactSymbolKind, ArtifactSymbolScope, ArtifactSymbolTable,
    ArtifactSymbolVisibility, DecodedInstruction, ExecutableSection, MemoryRegion, RelocationKind,
    SymbolRelocation,
};
pub use sections::load_executable_sections;
pub use symbols::{load_all_code_symbols, load_symbols};

#[cfg(test)]
mod tests;
