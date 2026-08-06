//! Reviewed generic memory and arithmetic intrinsics.

use super::body_identity::*;
use super::*;

pub(super) const MAX_REVIEWED_MEMORY_INTRINSIC_BYTES: u32 = 256;
pub(super) const ROM_DIVDI3_SIZE: usize = 926;
pub(super) const ROM_DIVDI3_ADDRESS: u32 = crate::wide_signed_divide_target_address();

pub(super) fn exact_standard_memory_intrinsic(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    matches!(
        (symbol.name.as_str(), symbol.address, symbol.bytes.len()),
        ("memcpy", 0x2f80_d260, 224) | ("memset", 0x2f82_20c6, 168)
    )
}

pub(super) fn exact_wide_signed_divide(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "__divdi3",
            address: u64::from(ROM_DIVDI3_ADDRESS),
            size: ROM_DIVDI3_SIZE,
        },
    )
}

pub(crate) fn wide_signed_divide_intrinsic(
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    exact_wide_signed_divide(symbol).then(|| {
        SymbolicValue::wide_signed_divide_words(
            arguments[0].clone(),
            arguments[1].clone(),
            arguments[2].clone(),
            arguments[3].clone(),
        )
    })
}

pub(crate) fn standard_memory_intrinsic_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<std::result::Result<FunctionAnalysis, String>> {
    if !exact_standard_memory_intrinsic(symbol) {
        return None;
    }
    Some((|| {
        let length = arguments[2]
            .as_constant()
            .ok_or_else(|| format!("{} length is not constant", symbol.name))?;
        if length > MAX_REVIEWED_MEMORY_INTRINSIC_BYTES {
            return Err(format!(
                "{} length {length} exceeds the reviewed summary limit of {MAX_REVIEWED_MEMORY_INTRINSIC_BYTES} bytes",
                symbol.name
            ));
        }

        let mut reference_events = Vec::new();
        if symbol.name == "memcpy" {
            for offset in 0..length {
                reference_events.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Read,
                    width: 8,
                    address: SymbolicValue::input(1).add_constant(offset),
                    region: "standard memcpy source".to_owned(),
                    value: None,
                });
            }
            for offset in 0..length {
                reference_events.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: 8,
                    address: SymbolicValue::input(0).add_constant(offset),
                    region: "standard memcpy destination".to_owned(),
                    value: Some(SymbolicValue::memory_read(offset, 8, false)),
                });
            }
        } else {
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
        Ok(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
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
