//! C language and standard-library semantic boundaries.
//!
//! This crate owns the exact symbol-to-contract mapping. The generic
//! Blobray and architecture backend only carry or execute a contract after
//! a selected provider composes this add-on.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, ExpressionOperation, ExternalArgumentDirection,
    ExternalArgumentSpec, ExternalReturnModel, ExternalSemanticSpec, FunctionAnalysis, MmioMap,
    SemanticFunctionBodyPolicy, StandardMemoryFunction, SymbolicValue,
};
use open_radio_vendor_backend_riscv::{
    Rv32CallArguments, Rv32IntrinsicResult, StructuralPointerContext,
    artifact::ArtifactSymbolDefinition,
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
const WIDE_BINARY_WORD_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "left_low",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "left_high",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "right_low",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "right_high",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
];
const STRING: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "string",
    c_type: "const char *",
    direction: ExternalArgumentDirection::Input,
};
const LEFT_STRING: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "left",
    c_type: "const char *",
    direction: ExternalArgumentDirection::Input,
};
const RIGHT_STRING: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "right",
    c_type: "const char *",
    direction: ExternalArgumentDirection::Input,
};
const LEFT_BYTES: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "left",
    c_type: "const void *",
    direction: ExternalArgumentDirection::Input,
};
const RIGHT_BYTES: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "right",
    c_type: "const void *",
    direction: ExternalArgumentDirection::Input,
};
const CHAR_DESTINATION: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "destination",
    c_type: "char *",
    direction: ExternalArgumentDirection::Output,
};
const MUTABLE_STRING: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "string",
    c_type: "char *",
    direction: ExternalArgumentDirection::InputOutput,
};
const DELIMITERS: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "delimiters",
    c_type: "const char *",
    direction: ExternalArgumentDirection::Input,
};
const CHARACTER: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "character",
    c_type: "int",
    direction: ExternalArgumentDirection::Input,
};
const ALLOCATION: ExternalArgumentSpec = ExternalArgumentSpec {
    name: "allocation",
    c_type: "void *",
    direction: ExternalArgumentDirection::Input,
};

const ONE_STRING_ARGUMENT: &[ExternalArgumentSpec] = &[STRING];
const TWO_STRING_ARGUMENTS: &[ExternalArgumentSpec] = &[LEFT_STRING, RIGHT_STRING];
const TWO_STRING_LENGTH_ARGUMENTS: &[ExternalArgumentSpec] = &[LEFT_STRING, RIGHT_STRING, LENGTH];
const STRING_LENGTH_ARGUMENTS: &[ExternalArgumentSpec] = &[STRING, LENGTH];
const BYTE_COMPARE_ARGUMENTS: &[ExternalArgumentSpec] = &[LEFT_BYTES, RIGHT_BYTES, LENGTH];
const STRING_COPY_ARGUMENTS: &[ExternalArgumentSpec] = &[CHAR_DESTINATION, STRING];
const STRING_COPY_LENGTH_ARGUMENTS: &[ExternalArgumentSpec] = &[CHAR_DESTINATION, STRING, LENGTH];
const TOKEN_ARGUMENTS: &[ExternalArgumentSpec] = &[MUTABLE_STRING, DELIMITERS];
const CHARACTER_ARGUMENTS: &[ExternalArgumentSpec] = &[CHARACTER];
const ALLOCATE_ARGUMENTS: &[ExternalArgumentSpec] = &[LENGTH];
const DEALLOCATE_ARGUMENTS: &[ExternalArgumentSpec] = &[ALLOCATION];

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

macro_rules! wide_runtime_spec {
    ($name:ident, $id:literal, $symbol:literal, $operation:literal) => {
        static $name: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
            id: $id,
            source: "c-addon",
            c_name: $symbol,
            argument_count: 4,
            body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
            return_model: ExternalReturnModel::SymbolicU64,
            semantic: ExternalSemanticSpec {
                operation: $operation,
                arguments: WIDE_BINARY_WORD_ARGUMENTS,
                return_type: "uint64_t",
                replacement: None,
                event_dispatch: None,
            },
            evidence: "exact compiler runtime symbol and standardized RV32 two-word ABI",
        };
    };
}

wide_runtime_spec!(
    DIVDI3,
    "c.runtime.divdi3",
    "__divdi3",
    "integer.divide-signed-64"
);
wide_runtime_spec!(
    MODDI3,
    "c.runtime.moddi3",
    "__moddi3",
    "integer.remainder-signed-64"
);
wide_runtime_spec!(
    UDIVDI3,
    "c.runtime.udivdi3",
    "__udivdi3",
    "integer.divide-unsigned-64"
);
wide_runtime_spec!(
    UMODDI3,
    "c.runtime.umoddi3",
    "__umoddi3",
    "integer.remainder-unsigned-64"
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

macro_rules! opaque_c_function {
    (
        $name:ident,
        $id:literal,
        $symbol:literal,
        $operation:literal,
        $arguments:ident,
        $return_type:literal,
        $return_model:expr
    ) => {
        static $name: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
            id: $id,
            source: "c-addon",
            c_name: $symbol,
            argument_count: $arguments.len() as u8,
            body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
            return_model: $return_model,
            semantic: ExternalSemanticSpec {
                operation: $operation,
                arguments: $arguments,
                return_type: $return_type,
                replacement: None,
                event_dispatch: None,
            },
            evidence: "exact public symbol identity and standardized C function contract",
        };
    };
}

opaque_c_function!(
    MALLOC,
    "c.standard.malloc",
    "malloc",
    "memory.allocate",
    ALLOCATE_ARGUMENTS,
    "void *",
    ExternalReturnModel::Allocated { size_argument: 0 }
);
opaque_c_function!(
    FREE,
    "c.standard.free",
    "free",
    "memory.free",
    DEALLOCATE_ARGUMENTS,
    "void",
    ExternalReturnModel::Void
);
opaque_c_function!(
    MEMCMP,
    "c.standard.memcmp",
    "memcmp",
    "memory.compare",
    BYTE_COMPARE_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    STRLEN,
    "c.standard.strlen",
    "strlen",
    "string.length",
    ONE_STRING_ARGUMENT,
    "size_t",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    STRNLEN,
    "c.standard.strnlen",
    "strnlen",
    "string.bounded-length",
    STRING_LENGTH_ARGUMENTS,
    "size_t",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    STRCMP,
    "c.standard.strcmp",
    "strcmp",
    "string.compare",
    TWO_STRING_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    STRNCMP,
    "c.standard.strncmp",
    "strncmp",
    "string.bounded-compare",
    TWO_STRING_LENGTH_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    STRCPY,
    "c.standard.strcpy",
    "strcpy",
    "string.copy",
    STRING_COPY_ARGUMENTS,
    "char *",
    ExternalReturnModel::Unmodeled
);
opaque_c_function!(
    STRNCPY,
    "c.standard.strncpy",
    "strncpy",
    "string.bounded-copy",
    STRING_COPY_LENGTH_ARGUMENTS,
    "char *",
    ExternalReturnModel::Unmodeled
);
opaque_c_function!(
    STRTOK,
    "c.standard.strtok",
    "strtok",
    "string.tokenize",
    TOKEN_ARGUMENTS,
    "char *",
    ExternalReturnModel::Unmodeled
);
opaque_c_function!(
    PUTS,
    "c.standard.puts",
    "puts",
    "diagnostic.puts",
    ONE_STRING_ARGUMENT,
    "int",
    ExternalReturnModel::SymbolicU32
);
opaque_c_function!(
    PUTCHAR,
    "c.standard.putchar",
    "putchar",
    "diagnostic.putchar",
    CHARACTER_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32
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
        "__divdi3" => Some(&DIVDI3),
        "__moddi3" => Some(&MODDI3),
        "__udivdi3" => Some(&UDIVDI3),
        "__umoddi3" => Some(&UMODDI3),
        "memcmp" => Some(&MEMCMP),
        "strlen" => Some(&STRLEN),
        "strnlen" => Some(&STRNLEN),
        "strcmp" => Some(&STRCMP),
        "strncmp" => Some(&STRNCMP),
        "strcpy" => Some(&STRCPY),
        "strncpy" => Some(&STRNCPY),
        "strtok" => Some(&STRTOK),
        "puts" => Some(&PUTS),
        "putchar" => Some(&PUTCHAR),
        "malloc" => Some(&MALLOC),
        "free" => Some(&FREE),
        _ => match standard_memory_function(name)? {
            StandardMemoryFunction::Copy => Some(&MEMCPY),
            StandardMemoryFunction::Move => Some(&MEMMOVE),
            StandardMemoryFunction::Set => Some(&MEMSET),
        },
    }
}

/// Exact value semantics for pure compiler-runtime boundaries. The caller's
/// linked stub address is intentionally irrelevant: the origin relocation
/// supplies the public symbol identity.
pub fn direct_external_intrinsic(
    name: &str,
    arguments: &Rv32CallArguments,
) -> Option<Rv32IntrinsicResult> {
    let operation = match name {
        "__ctzsi2" => ExpressionOperation::CountTrailingZeros,
        "__clzsi2" => ExpressionOperation::CountLeadingZeros,
        "__popcountsi2" => ExpressionOperation::PopulationCount,
        _ => return None,
    };
    Some((
        SymbolicValue::expression(operation, arguments[0].clone(), SymbolicValue::Constant(0)),
        None,
    ))
}

/// Exact symbolic summary for pure compiler-runtime word operations.
pub fn reference_intrinsic_trace(
    symbol: &ArtifactSymbolDefinition,
    _svd: &MmioMap,
    _pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    let arguments = core::array::from_fn(|index| SymbolicValue::input(index as u8));
    let (return_value, high) = direct_external_intrinsic(&symbol.name, &arguments)?;
    debug_assert!(high.is_none());
    Some(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value,
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
        assert_eq!(
            direct_external_semantic_function("strncmp")
                .unwrap()
                .semantic
                .arguments
                .len(),
            3
        );
        assert_eq!(
            direct_external_semantic_function("strlen")
                .unwrap()
                .return_model,
            ExternalReturnModel::SymbolicU32
        );
        let umoddi3 = direct_external_semantic_function("__umoddi3").unwrap();
        assert_eq!(umoddi3.return_model, ExternalReturnModel::SymbolicU64);
        assert_eq!(umoddi3.argument_count, 4);
        assert_eq!(
            umoddi3.body_policy,
            SemanticFunctionBodyPolicy::OpaqueBoundary
        );
        assert_eq!(umoddi3.semantic.operation, "integer.remainder-unsigned-64");
        assert!(direct_external_semantic_function("vendor___umoddi3").is_none());
        assert_eq!(
            direct_external_semantic_function("malloc")
                .unwrap()
                .return_model,
            ExternalReturnModel::Allocated { size_argument: 0 }
        );
        assert_eq!(
            direct_external_semantic_function("free")
                .unwrap()
                .return_model,
            ExternalReturnModel::Void
        );
        assert!(direct_external_semantic_function("vendor_malloc").is_none());
        assert!(direct_external_semantic_function("sprintf").is_none());
        assert!(direct_external_semantic_function("pp_printf").is_none());
        let arguments = core::array::from_fn(|index| SymbolicValue::input(index as u8));
        assert_eq!(
            direct_external_intrinsic("__ctzsi2", &arguments),
            Some((
                SymbolicValue::expression(
                    ExpressionOperation::CountTrailingZeros,
                    SymbolicValue::input(0),
                    SymbolicValue::Constant(0),
                ),
                None,
            ))
        );
        assert!(direct_external_intrinsic("vendor_ctz", &arguments).is_none());
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
