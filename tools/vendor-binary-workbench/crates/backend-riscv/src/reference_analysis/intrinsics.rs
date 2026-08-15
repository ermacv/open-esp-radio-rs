//! Contracts for standard runtime functions whose public ABI is the evidence.

use super::*;
use open_radio_vendor_analysis_model::StandardMemoryFunction;

const MAX_STANDARD_MEMORY_INTRINSIC_BYTES: u32 = 256;

/// Model a standard memory call from its exact public symbol identity and ABI.
///
/// Its implementation body is deliberately irrelevant: libc wrappers and
/// compiler-builtins may use different loops, vector widths or tail calls
/// while retaining the same specified behavior. A non-constant or excessive
/// length remains fail-closed because this reference IR records each byte in
/// order and has a finite expansion budget.
pub(super) fn standard_memory_intrinsic_trace(
    intrinsic: StandardMemoryFunction,
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<std::result::Result<FunctionAnalysis, String>> {
    Some((|| {
        let length = arguments[2]
            .as_constant()
            .ok_or_else(|| format!("{} length is not constant", symbol.name))?;
        if length > MAX_STANDARD_MEMORY_INTRINSIC_BYTES {
            return Err(format!(
                "{} length {length} exceeds the standard intrinsic expansion limit of {MAX_STANDARD_MEMORY_INTRINSIC_BYTES} bytes",
                symbol.name
            ));
        }

        let mut reference_events = Vec::new();
        match intrinsic {
            StandardMemoryFunction::Copy | StandardMemoryFunction::Move => {
                // Read the complete source first. This is also the required
                // overlap-safe observable model for memmove.
                for offset in 0..length {
                    reference_events.push(DraftReferenceEvent::Memory {
                        access: MemoryAccess::Read,
                        width: 8,
                        address: SymbolicValue::input(1).add_constant(offset),
                        region: format!("standard {} source", symbol.name),
                        value: None,
                    });
                }
                for offset in 0..length {
                    reference_events.push(DraftReferenceEvent::Memory {
                        access: MemoryAccess::Write,
                        width: 8,
                        address: SymbolicValue::input(0).add_constant(offset),
                        region: format!("standard {} destination", symbol.name),
                        value: Some(SymbolicValue::memory_read(offset, 8, false)),
                    });
                }
            }
            StandardMemoryFunction::Set => {
                let byte = SymbolicValue::input(1).and(0xff);
                for offset in 0..length {
                    reference_events.push(DraftReferenceEvent::Memory {
                        access: MemoryAccess::Write,
                        width: 8,
                        address: SymbolicValue::input(0).add_constant(offset),
                        region: "standard memset destination".to_owned(),
                        value: Some(byte.clone()),
                    });
                }
            }
        }

        Ok(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events,
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::input(0),
            reference_flow: None,
            unresolved_branch: None,
        })
    })())
}
