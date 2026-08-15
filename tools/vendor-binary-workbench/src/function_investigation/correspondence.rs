//! Conservative correspondence between relocatable and linked instructions.
//!
//! This module owns navigation evidence only. It never projects archive
//! offsets arithmetically and never claims semantic equivalence after linker
//! rewriting.

use std::collections::{BTreeMap, BTreeSet};

use crate::artifact;

use super::{OriginInstructionCorrespondence, OriginRelocationDependency};

pub(crate) fn origin_instruction_correspondence(
    origin: &artifact::FunctionBody,
    runtime: &artifact::FunctionBody,
) -> Vec<OriginInstructionCorrespondence> {
    let mut evidence = Vec::new();
    let mut origin_index = 0usize;
    let mut runtime_index = 0usize;
    while origin_index < origin.instructions.len() && runtime_index < runtime.instructions.len() {
        let current_origin = &origin.instructions[origin_index];
        let current_runtime = &runtime.instructions[runtime_index];
        if instruction_shapes_match(current_origin, current_runtime) {
            push_correspondence(
                &mut evidence,
                std::slice::from_ref(current_origin),
                current_runtime,
                "same-shape",
            );
            origin_index += 1;
            runtime_index += 1;
            continue;
        }

        if let Some(next_origin) = origin.instructions.get(origin_index + 1)
            && contracted_relocation_pair_matches(current_origin, next_origin, current_runtime)
        {
            push_correspondence(
                &mut evidence,
                &origin.instructions[origin_index..=origin_index + 1],
                current_runtime,
                "linker-relaxation",
            );
            origin_index += 2;
            runtime_index += 1;
            continue;
        }

        // A low relocation can reuse a base established earlier in the
        // archive while the linked image folds that base to zero. Retain only
        // a relocation-bearing, opcode/destination match.
        if !current_origin.relocations.is_empty()
            && relaxed_single_instruction_matches(current_origin, current_runtime)
        {
            push_correspondence(
                &mut evidence,
                std::slice::from_ref(current_origin),
                current_runtime,
                "linker-relaxation",
            );
            origin_index += 1;
            runtime_index += 1;
            continue;
        }

        // Bound resynchronization to one instruction. Larger gaps are left
        // deliberately unmapped instead of manufacturing correspondence.
        if origin
            .instructions
            .get(origin_index + 1)
            .is_some_and(|next| instruction_shapes_match(next, current_runtime))
        {
            origin_index += 1;
        } else if runtime
            .instructions
            .get(runtime_index + 1)
            .is_some_and(|next| instruction_shapes_match(current_origin, next))
        {
            runtime_index += 1;
        } else {
            origin_index += 1;
            runtime_index += 1;
        }
    }
    evidence
}

pub(super) fn origin_relocation_dependencies(
    body: &artifact::FunctionBody,
) -> Vec<OriginRelocationDependency> {
    #[derive(Default)]
    struct Dependency {
        references: usize,
        instruction_offsets: BTreeSet<u64>,
        kinds: BTreeSet<String>,
    }

    let mut dependencies = BTreeMap::<String, Dependency>::new();
    for instruction in &body.instructions {
        for relocation in &instruction.relocations {
            let dependency = dependencies.entry(relocation.symbol.clone()).or_default();
            dependency.references += 1;
            dependency.instruction_offsets.insert(instruction.offset);
            dependency.kinds.insert(relocation.kind.clone());
        }
    }
    dependencies
        .into_iter()
        .map(|(symbol, dependency)| OriginRelocationDependency {
            symbol,
            references: dependency.references,
            instruction_offsets: dependency.instruction_offsets.into_iter().collect(),
            kinds: dependency.kinds.into_iter().collect(),
        })
        .collect()
}

fn push_correspondence(
    output: &mut Vec<OriginInstructionCorrespondence>,
    origins: &[artifact::FunctionInstruction],
    runtime: &artifact::FunctionInstruction,
    kind: &'static str,
) {
    let relocation_symbols = origins
        .iter()
        .flat_map(|instruction| &instruction.relocations)
        .map(|relocation| relocation.symbol.clone())
        .collect::<BTreeSet<_>>();
    if relocation_symbols.is_empty() {
        return;
    }
    output.push(OriginInstructionCorrespondence {
        origin_offsets: origins
            .iter()
            .map(|instruction| instruction.offset)
            .collect(),
        runtime_address: runtime.address,
        runtime_offset: runtime.offset,
        kind,
        relocation_symbols: relocation_symbols.into_iter().collect(),
        semantic_equivalence_claim: false,
    });
}

fn contracted_relocation_pair_matches(
    first: &artifact::FunctionInstruction,
    second: &artifact::FunctionInstruction,
    runtime: &artifact::FunctionInstruction,
) -> bool {
    let first_mnemonic = instruction_mnemonic(&first.text);
    let second_mnemonic = instruction_mnemonic(&second.text);
    let runtime_mnemonic = instruction_mnemonic(&runtime.text);
    let symbols = first
        .relocations
        .iter()
        .map(|relocation| relocation.symbol.as_str())
        .collect::<BTreeSet<_>>();
    let shared_relocation = second
        .relocations
        .iter()
        .any(|relocation| symbols.contains(relocation.symbol.as_str()));
    let relaxed_call = first_mnemonic == "auipc"
        && second_mnemonic == "jalr"
        && runtime_mnemonic == "jal"
        && first
            .relocations
            .iter()
            .any(|relocation| relocation.kind == "call");
    let relaxed_address = first_mnemonic == "lui"
        && shared_relocation
        && second_mnemonic == runtime_mnemonic
        && destination_register(&second.text) == destination_register(&runtime.text);
    relaxed_call || relaxed_address
}

fn relaxed_single_instruction_matches(
    origin: &artifact::FunctionInstruction,
    runtime: &artifact::FunctionInstruction,
) -> bool {
    instruction_mnemonic(&origin.text) == instruction_mnemonic(&runtime.text)
        && destination_register(&origin.text) == destination_register(&runtime.text)
}

fn instruction_shapes_match(
    left: &artifact::FunctionInstruction,
    right: &artifact::FunctionInstruction,
) -> bool {
    let left_mnemonic = canonical_mnemonic(instruction_mnemonic(&left.text));
    let right_mnemonic = canonical_mnemonic(instruction_mnemonic(&right.text));
    left_mnemonic == right_mnemonic
        && instruction_registers(&left.text) == instruction_registers(&right.text)
}

fn instruction_mnemonic(text: &str) -> &str {
    text.split_ascii_whitespace().next().unwrap_or("")
}

fn canonical_mnemonic(mnemonic: &str) -> &str {
    match mnemonic {
        "mv" => "addi",
        other => other,
    }
}

fn destination_register(text: &str) -> Option<&str> {
    instruction_registers(text).into_iter().next()
}

fn instruction_registers(text: &str) -> Vec<&str> {
    text.split(|character: char| !(character.is_ascii_alphanumeric()))
        .filter(|token| is_riscv_register(token))
        .collect()
}

fn is_riscv_register(token: &str) -> bool {
    matches!(
        token,
        "zero"
            | "ra"
            | "sp"
            | "gp"
            | "tp"
            | "t0"
            | "t1"
            | "t2"
            | "t3"
            | "t4"
            | "t5"
            | "t6"
            | "s0"
            | "s1"
            | "s2"
            | "s3"
            | "s4"
            | "s5"
            | "s6"
            | "s7"
            | "s8"
            | "s9"
            | "s10"
            | "s11"
            | "a0"
            | "a1"
            | "a2"
            | "a3"
            | "a4"
            | "a5"
            | "a6"
            | "a7"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instruction(
        offset: u64,
        address: u64,
        text: &str,
        relocation: Option<(&str, &str)>,
    ) -> artifact::FunctionInstruction {
        artifact::FunctionInstruction {
            offset,
            address,
            width: 4,
            raw: "00000000".to_owned(),
            text: text.to_owned(),
            supported: true,
            blocker_class: None,
            control_flow: artifact::FunctionControlFlow {
                kind: artifact::FunctionControlFlowKind::Linear,
                target: None,
            },
            relocations: relocation
                .map(|(kind, symbol)| artifact::FunctionInstructionRelocation {
                    kind: kind.to_owned(),
                    symbol: symbol.to_owned(),
                    addend: 0,
                })
                .into_iter()
                .collect(),
        }
    }

    fn body(
        artifact_name: &str,
        address: u64,
        instructions: Vec<artifact::FunctionInstruction>,
    ) -> artifact::FunctionBody {
        artifact::FunctionBody {
            artifact: artifact_name.to_owned(),
            member: None,
            symbol: "root".to_owned(),
            address,
            size: instructions
                .iter()
                .map(|instruction| usize::from(instruction.width))
                .sum(),
            addresses_resolved: address != 0,
            accounted_bytes: instructions
                .iter()
                .map(|instruction| usize::from(instruction.width))
                .sum(),
            instructions,
            basic_blocks: Vec::new(),
            loops: Vec::new(),
            labels: Vec::new(),
        }
    }

    #[test]
    fn dependencies_group_relocations_and_relaxation_is_not_a_semantic_claim() {
        let origin = body(
            "fixture.a",
            0,
            vec![
                instruction(0, 0, "lui a0, 0", Some(("hi20", "g_state"))),
                instruction(4, 4, "lw a0, 0(a0)", Some(("lo12-i", "g_state"))),
            ],
        );
        let runtime = body(
            "fixture.elf",
            0x1000,
            vec![instruction(0, 0x1000, "lw a0, 0(zero)", None)],
        );

        let dependencies = origin_relocation_dependencies(&origin);
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].symbol, "g_state");
        assert_eq!(dependencies[0].references, 2);
        assert_eq!(dependencies[0].instruction_offsets, [0, 4]);
        assert_eq!(dependencies[0].kinds, ["hi20", "lo12-i"]);

        let correspondence = origin_instruction_correspondence(&origin, &runtime);
        assert_eq!(correspondence.len(), 1);
        assert_eq!(correspondence[0].origin_offsets, [0, 4]);
        assert_eq!(correspondence[0].runtime_address, 0x1000);
        assert_eq!(correspondence[0].kind, "linker-relaxation");
        assert_eq!(correspondence[0].relocation_symbols, ["g_state"]);
        assert!(!correspondence[0].semantic_equivalence_claim);
    }
}
