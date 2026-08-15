//! C language and standard-library semantic boundaries.
//!
//! This crate owns the exact symbol-to-contract mapping. The generic
//! Workbench and architecture backend only carry or execute a contract after
//! a selected provider composes this add-on.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, ExternalArgumentDirection, ExternalArgumentSpec,
    ExternalReturnModel, ExternalSemanticSpec, StandardMemoryFunction,
};
use open_radio_vendor_backend_riscv::artifact::ArtifactSymbolDefinition;

const DESTINATION: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "destination",
    c_type: "void *",
    direction: ExternalArgumentDirection::Output,
};
const SOURCE: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "source",
    c_type: "const void *",
    direction: ExternalArgumentDirection::Input,
};
const BYTE: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "byte",
    c_type: "int",
    direction: ExternalArgumentDirection::Input,
};
const LENGTH: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "length",
    c_type: "size_t",
    direction: ExternalArgumentDirection::Input,
};

const COPY_ARGUMENTS: &[ExternalArgumentSpec] = &[DESTINATION, SOURCE, LENGTH];
const SET_ARGUMENTS: &[ExternalArgumentSpec] = &[DESTINATION, BYTE, LENGTH];

static MEMCPY: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "c.standard.memcpy",
    source: "c-addon",
    c_name: "memcpy",
    argument_count: 3,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "memory.copy",
        arguments: COPY_ARGUMENTS,
        return_type: "void *",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact public symbol identity and standardized C function contract",
};

static MEMMOVE: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "c.standard.memmove",
    source: "c-addon",
    c_name: "memmove",
    argument_count: 3,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "memory.move",
        arguments: COPY_ARGUMENTS,
        return_type: "void *",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact public symbol identity and standardized C function contract",
};

static MEMSET: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "c.standard.memset",
    source: "c-addon",
    c_name: "memset",
    argument_count: 3,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "memory.set",
        arguments: SET_ARGUMENTS,
        return_type: "void *",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact public symbol identity and standardized C function contract",
};

/// Return the standardized memory contract selected by one exact C symbol.
pub fn standard_memory_function(name: &str) -> Option<StandardMemoryFunction> {
    match name {
        "memcpy" | "__builtin_memcpy" => Some(StandardMemoryFunction::Copy),
        "memmove" | "__builtin_memmove" => Some(StandardMemoryFunction::Move),
        "memset" | "__builtin_memset" => Some(StandardMemoryFunction::Set),
        _ => None,
    }
}

/// Semantic ABI for a linked definition. Its bytes are deliberately ignored:
/// the implementation is not the analysis target once the public C contract
/// has been selected.
pub fn direct_semantic_function(
    symbol: &ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    direct_external_semantic_function(&symbol.name)
}

/// Semantic ABI for a relocation whose implementation is outside the loaded
/// artifact catalog.
pub fn direct_external_semantic_function(
    name: &str,
) -> Option<&'static DirectSemanticFunctionSpec> {
    match standard_memory_function(name)? {
        StandardMemoryFunction::Copy => Some(&MEMCPY),
        StandardMemoryFunction::Move => Some(&MEMMOVE),
        StandardMemoryFunction::Set => Some(&MEMSET),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_standard_names_select_c_contracts() {
        assert_eq!(
            standard_memory_function("memcpy"),
            Some(StandardMemoryFunction::Copy)
        );
        assert_eq!(
            standard_memory_function("__builtin_memset"),
            Some(StandardMemoryFunction::Set)
        );
        assert_eq!(standard_memory_function("vendor_memcpy"), None);
        assert_eq!(standard_memory_function("memcpy_fast"), None);
    }

    #[test]
    fn body_bytes_do_not_select_or_change_the_standard_contract() {
        let symbol = |bytes| ArtifactSymbolDefinition {
            member: None,
            name: "memcpy".to_owned(),
            address: 0x1000,
            bytes,
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let first = direct_semantic_function(&symbol(vec![0xff])).unwrap();
        let second = direct_semantic_function(&symbol(vec![0x73, 0, 0x10, 0])).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.semantic.operation, "memory.copy");
    }
}
