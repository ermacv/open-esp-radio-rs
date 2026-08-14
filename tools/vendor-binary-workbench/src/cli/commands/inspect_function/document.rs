//! Stable compact JSON projection for the default function investigation.
//!
//! The investigator keeps complete function bodies because the human renderer,
//! TUI and focused queries need them.  Serializing those bodies by default,
//! however, made the ordinary machine report little better than an embedded
//! `objdump`.  This projection retains identity, accounting and proof evidence;
//! `inspect function --full` remains the explicitly lossless document.

use serde::Serialize;

use crate::{
    artifact::FunctionBody,
    function_investigation::{
        CfgPathEvidence, FunctionInvestigationReport, InvestigationLedgerEntry,
        OriginFunctionEvidence, OriginInstructionCorrespondence, OriginRelocationDependency,
        ReplacementEvidence, ReviewedPathEvidence, ReviewedPreconditionEvidence,
        SemanticFunctionEvidence,
    },
};

#[derive(Serialize)]
pub(super) struct CompactFunctionInvestigation<'a> {
    schema_version: u32,
    command: &'static str,
    source: &'a str,
    symbol: &'a str,
    runtime: FunctionBodySummary<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<OriginFunctionSummary<'a>>,
    semantics: &'a [SemanticFunctionEvidence],
    reviewed_preconditions: &'a [ReviewedPreconditionEvidence],
    reviewed_paths: &'a [ReviewedPathEvidence],
    #[serde(skip_serializing_if = "Option::is_none")]
    cfg_path: Option<&'a CfgPathEvidence>,
    proof_ledger: &'a [InvestigationLedgerEntry],
    replacements: &'a [ReplacementEvidence],
}

impl<'a> CompactFunctionInvestigation<'a> {
    pub(super) fn from_report(report: &'a FunctionInvestigationReport) -> Self {
        Self {
            schema_version: report.schema_version,
            command: report.command,
            source: &report.source,
            symbol: &report.symbol,
            runtime: FunctionBodySummary::from_body(&report.runtime),
            origin: report
                .origin
                .as_ref()
                .map(OriginFunctionSummary::from_origin),
            semantics: &report.semantics,
            reviewed_preconditions: &report.reviewed_preconditions,
            reviewed_paths: &report.reviewed_paths,
            cfg_path: report.cfg_path.as_ref(),
            proof_ledger: &report.proof_ledger,
            replacements: &report.replacements,
        }
    }
}

#[derive(Serialize)]
struct FunctionBodySummary<'a> {
    artifact: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    member: Option<&'a str>,
    symbol: &'a str,
    address: u64,
    size: usize,
    addresses_resolved: bool,
    accounted_bytes: usize,
    instructions: usize,
    basic_blocks: usize,
    reachable_blocks: usize,
    labels: usize,
    relocations: usize,
    unsupported_instructions: usize,
}

impl<'a> FunctionBodySummary<'a> {
    fn from_body(body: &'a FunctionBody) -> Self {
        Self {
            artifact: &body.artifact,
            member: body.member.as_deref(),
            symbol: &body.symbol,
            address: body.address,
            size: body.size,
            addresses_resolved: body.addresses_resolved,
            accounted_bytes: body.accounted_bytes,
            instructions: body.instructions.len(),
            basic_blocks: body.basic_blocks.len(),
            reachable_blocks: body
                .basic_blocks
                .iter()
                .filter(|block| block.reachable)
                .count(),
            labels: body.labels.len(),
            relocations: body
                .instructions
                .iter()
                .map(|instruction| instruction.relocations.len())
                .sum(),
            unsupported_instructions: body
                .instructions
                .iter()
                .filter(|instruction| !instruction.supported)
                .count(),
        }
    }
}

#[derive(Serialize)]
struct OriginFunctionSummary<'a> {
    association: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_report: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_address: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_member: Option<&'a str>,
    relocation_dependencies: &'a [OriginRelocationDependency],
    instruction_correspondence: &'a [OriginInstructionCorrespondence],
    body: FunctionBodySummary<'a>,
}

impl<'a> OriginFunctionSummary<'a> {
    fn from_origin(origin: &'a OriginFunctionEvidence) -> Self {
        Self {
            association: origin.association,
            inventory_report: origin.inventory_report.as_deref(),
            linked_address: origin.linked_address,
            linked_member: origin.linked_member.as_deref(),
            relocation_dependencies: &origin.relocation_dependencies,
            instruction_correspondence: &origin.instruction_correspondence,
            body: FunctionBodySummary::from_body(&origin.body),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{
        FunctionBasicBlock, FunctionBlockSuccessor, FunctionControlFlow, FunctionControlFlowKind,
        FunctionInstruction, FunctionInstructionRelocation, FunctionLabel,
    };

    fn body() -> FunctionBody {
        FunctionBody {
            artifact: "fixture.elf".to_owned(),
            member: None,
            symbol: "entry".to_owned(),
            address: 0x1000,
            size: 4,
            addresses_resolved: true,
            accounted_bytes: 4,
            instructions: vec![FunctionInstruction {
                offset: 0,
                address: 0x1000,
                width: 4,
                raw: "00000013".to_owned(),
                text: "addi zero, zero, 0".to_owned(),
                supported: true,
                blocker_class: None,
                control_flow: FunctionControlFlow {
                    kind: FunctionControlFlowKind::Linear,
                    target: None,
                },
                relocations: vec![FunctionInstructionRelocation {
                    kind: "call".to_owned(),
                    symbol: "dependency".to_owned(),
                    addend: 0,
                }],
            }],
            basic_blocks: vec![FunctionBasicBlock {
                id: 0,
                start_offset: 0,
                end_offset: 4,
                reachable: true,
                successors: vec![FunctionBlockSuccessor {
                    kind: "fallthrough".to_owned(),
                    block: None,
                    target: None,
                }],
            }],
            labels: vec![FunctionLabel {
                offset: 0,
                name: "entry".to_owned(),
                kind: "function".to_owned(),
            }],
        }
    }

    #[test]
    fn compact_document_counts_the_body_without_serializing_instructions() {
        let report = FunctionInvestigationReport {
            schema_version: 12,
            command: "inspect function",
            source: "fixture".to_owned(),
            symbol: "entry".to_owned(),
            runtime: body(),
            origin: None,
            semantics: Vec::new(),
            reviewed_preconditions: Vec::new(),
            reviewed_paths: Vec::new(),
            cfg_path: None,
            proof_ledger: Vec::new(),
            replacements: Vec::new(),
        };
        let value = serde_json::to_value(CompactFunctionInvestigation::from_report(&report))
            .expect("serialize compact investigation");
        assert_eq!(value["runtime"]["instructions"], 1);
        assert_eq!(value["runtime"]["relocations"], 1);
        assert!(value["runtime"].get("basic_blocks").is_some());
        assert!(value["runtime"].get("labels").is_some());
        assert!(value["runtime"].get("raw").is_none());
    }
}
