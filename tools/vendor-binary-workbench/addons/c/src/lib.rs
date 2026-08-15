//! C language and standard-library semantic boundaries.
//!
//! This crate owns the exact symbol-to-contract mapping. The generic
//! Workbench and architecture backend only carry or execute a contract after
//! a selected provider composes this add-on.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, ExpressionOperation, ExternalArgumentDirection,
    ExternalArgumentSpec, ExternalReturnModel, ExternalSemanticSpec, FunctionAnalysis, MmioMap,
    SemanticFunctionBodyPolicy, StandardMemoryFunction, SymbolicValue,
};
use open_radio_vendor_backend_riscv::{
    StructuralPointerContext, artifact::ArtifactSymbolDefinition,
};

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
const WORD_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "value",
    c_type: "unsigned int",
    direction: ExternalArgumentDirection::Input,
}];

static MEMCPY: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "c.standard.memcpy",
    source: "c-addon",
    c_name: "memcpy",
    argument_count: 3,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
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
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
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
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
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

macro_rules! pure_word_runtime_spec {
    ($name:ident, $id:literal, $symbol:literal, $operation:literal) => {
        static $name: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
            id: $id,
            source: "c-addon",
            c_name: $symbol,
            argument_count: 1,
            body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
            return_model: ExternalReturnModel::Unmodeled,
            semantic: ExternalSemanticSpec {
                operation: $operation,
                arguments: WORD_ARGUMENTS,
                return_type: "unsigned int",
                replacement: None,
                event_dispatch: None,
            },
            evidence: "exact compiler runtime symbol and standardized 32-bit operation contract",
        };
    };
}

pure_word_runtime_spec!(
    CTZSI2,
    "c.runtime.ctzsi2",
    "__ctzsi2",
    "integer.trailing-zeros"
);
pure_word_runtime_spec!(
    CLZSI2,
    "c.runtime.clzsi2",
    "__clzsi2",
    "integer.leading-zeros"
);
pure_word_runtime_spec!(
    POPCOUNTSI2,
    "c.runtime.popcountsi2",
    "__popcountsi2",
    "integer.population-count"
);

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
    match name {
        "__ctzsi2" => Some(&CTZSI2),
        "__clzsi2" => Some(&CLZSI2),
        "__popcountsi2" => Some(&POPCOUNTSI2),
        _ => match standard_memory_function(name)? {
            StandardMemoryFunction::Copy => Some(&MEMCPY),
            StandardMemoryFunction::Move => Some(&MEMMOVE),
            StandardMemoryFunction::Set => Some(&MEMSET),
        },
    }
}

/// Exact symbolic summary for pure compiler-runtime word operations.
pub fn reference_intrinsic_trace(
    symbol: &ArtifactSymbolDefinition,
    _svd: &MmioMap,
    _pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    let operation = match symbol.name.as_str() {
        "__ctzsi2" => ExpressionOperation::CountTrailingZeros,
        "__clzsi2" => ExpressionOperation::CountLeadingZeros,
        "__popcountsi2" => ExpressionOperation::PopulationCount,
        _ => return None,
    };
    Some(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::expression(
            operation,
            SymbolicValue::input(0),
            SymbolicValue::Constant(0),
        ),
        reference_flow: None,
        unresolved_branch: None,
    })
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
        assert_eq!(
            direct_external_semantic_function("__ctzsi2")
                .unwrap()
                .semantic
                .operation,
            "integer.trailing-zeros"
        );
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

    #[test]
    fn pure_runtime_intrinsics_have_exact_symbolic_results() {
        let symbol = ArtifactSymbolDefinition {
            member: None,
            name: "__popcountsi2".to_owned(),
            address: 0x1000,
            bytes: vec![0xff],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let trace = reference_intrinsic_trace(
            &symbol,
            &MmioMap {
                registers: Vec::new(),
                regions: Vec::new(),
            },
            &StructuralPointerContext::default(),
        )
        .unwrap();
        assert_eq!(
            trace.return_value.canonical(),
            "expr:PopulationCount(arg0,const:0x00000000)"
        );
    }
}
