//! Stable function-review facts projected from linked-IR artifacts.

mod parse;
mod validate;

use std::path::PathBuf;

use crate::Result;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInputFact {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionContextFieldFact {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum FunctionMemoryObjectFact {
    Argument {
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    Dereferenced {
        pointer: Box<FunctionMemoryObjectFact>,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
    Indexed {
        object: Box<FunctionMemoryObjectFact>,
        argument: u8,
        stride: i64,
    },
    Allocation {
        call_token: u32,
    },
    ZeroedAllocation {
        call_token: u32,
    },
    OpaqueExternalObject {
        call_token: u32,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionMemoryFieldFact {
    pub(crate) object: FunctionMemoryObjectFact,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionCallFact {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) site: Option<u32>,
    pub(crate) arguments: Vec<String>,
    pub(crate) guard_paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionEventDispatchFact {
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) receiver: Option<String>,
    pub(crate) interface_complete: bool,
    pub(crate) bindings: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ScenarioArgumentFact {
    pub(crate) index: u8,
    pub(crate) value: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ScenarioMmioReadFact {
    pub(crate) address: u32,
    pub(crate) mask: u32,
    pub(crate) expected: u32,
    pub(crate) values: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ScenarioSuggestionVariantFact {
    pub(crate) name: String,
    pub(crate) arguments: Vec<ScenarioArgumentFact>,
    pub(crate) mmio_reads: Vec<ScenarioMmioReadFact>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ScenarioSuggestionFact {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) evidence: String,
    pub(crate) variants: Vec<ScenarioSuggestionVariantFact>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionDecodeBlockerFact {
    pub(crate) address: u64,
    pub(crate) width: u8,
    pub(crate) raw: u32,
    pub(crate) class: String,
    pub(crate) linear_control_flow: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionFact {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) selection: String,
    pub(crate) body_complete: bool,
    pub(crate) call_targets_complete: bool,
    pub(crate) transitive_effects_complete: bool,
    pub(crate) executable_complete: bool,
    pub(crate) transitive_effects_materialized: bool,
    pub(crate) call_graph_closed: bool,
    pub(crate) context_projection_materialized: bool,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) decode_blockers: Vec<FunctionDecodeBlockerFact>,
    pub(crate) direct_calls: usize,
    pub(crate) calls: Vec<FunctionCallFact>,
    pub(crate) mmio_addresses: Vec<u32>,
    pub(crate) context_fields: Vec<FunctionContextFieldFact>,
    pub(crate) memory_fields: Vec<FunctionMemoryFieldFact>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) trampoline_calls: usize,
    pub(crate) event_dispatches: Vec<FunctionEventDispatchFact>,
    pub(crate) scenario_suggestions: Vec<ScenarioSuggestionFact>,
    pub(crate) pseudo: String,
}

impl FunctionFact {
    pub(crate) fn is_root(&self) -> bool {
        self.selection == "symbol-prefix-root"
    }

    pub(crate) fn review_complete(&self) -> bool {
        self.body_complete
            && self.call_targets_complete
            && self.transitive_effects_complete
            && self.executable_complete
            && self.transitive_effects_materialized
            && self.call_graph_closed
            && self.context_projection_materialized
            && self.context_projection_complete
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionFacts {
    pub(crate) inputs: Vec<FunctionInputFact>,
    pub(crate) functions: Vec<FunctionFact>,
}

pub(crate) fn function_fact_from_stored(
    profile: &str,
    function: crate::artifacts::StoredFunction,
) -> FunctionFact {
    parse::parse_function(profile, function)
}

impl FunctionFacts {
    #[tracing::instrument(name = "load_function_facts", skip_all, fields(reports = reports.len()))]
    pub(crate) fn load(reports: &[(String, PathBuf)]) -> Result<Self> {
        let mut inputs = Vec::new();
        let mut functions = Vec::new();
        for (profile, path) in reports {
            let document = crate::artifacts::load_linked_ir_functions(path)?;
            let (report_inputs, report_functions) = parse::parse_document(profile, document)?;
            inputs.extend(report_inputs);
            functions.extend(report_functions);
        }
        inputs.sort();
        functions.sort_by(|left, right| {
            (&left.profile, &left.identity).cmp(&(&right.profile, &right.identity))
        });
        validate::validate(&inputs, &functions)?;
        Ok(Self { inputs, functions })
    }

    #[tracing::instrument(
        name = "load_function_fact_summaries",
        skip_all,
        fields(reports = reports.len())
    )]
    pub(crate) fn load_summary(reports: &[(String, PathBuf)]) -> Result<Self> {
        let mut inputs = Vec::new();
        let mut functions = Vec::new();
        for (profile, path) in reports {
            let projection =
                crate::artifacts::LinkedIrReader::open(path)?.read_review_projection()?;
            let (report_inputs, report_functions) =
                parse::parse_review_projection(profile, projection)?;
            inputs.extend(report_inputs);
            functions.extend(report_functions);
        }
        inputs.sort();
        functions.sort_by(|left, right| {
            (&left.profile, &left.identity).cmp(&(&right.profile, &right.identity))
        });
        validate::validate(&inputs, &functions)?;
        Ok(Self { inputs, functions })
    }

    pub(crate) fn function(
        &self,
        profile: &str,
        source: &str,
        identity: &str,
    ) -> Option<&FunctionFact> {
        self.functions.iter().find(|function| {
            function.profile == profile
                && function.source == source
                && function.identity == identity
        })
    }

    pub(crate) fn root_functions(&self) -> impl Iterator<Item = &FunctionFact> {
        self.functions.iter().filter(|function| function.is_root())
    }
}
