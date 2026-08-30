//! Fail-closed structural ordering witnesses for focused flow investigation.
//!
//! A witness proves only that every conservative CFG path which reaches the
//! later instruction executes the earlier instruction first. It deliberately
//! does not claim that either instruction is executable for a concrete input.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::{FunctionBody, FunctionControlFlowKind};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MustExecuteBeforeWitness {
    pub(crate) earlier_site: u32,
    pub(crate) later_site: u32,
    pub(crate) earlier_block: usize,
    pub(crate) later_block: usize,
    pub(crate) proof: &'static str,
    pub(crate) path_feasibility_claim: bool,
}

/// Prove conditional ordering over the complete conservative function CFG.
///
/// `None` is returned for incomplete decode, unresolved intra-function control
/// flow, malformed CFGs, non-instruction sites, unreachable sites, or when a
/// bypass path exists. Calls are allowed: failure to return only removes a path
/// and therefore cannot invalidate an ordering claim about paths which do
/// reach `later_site`.
pub(crate) fn must_execute_before(
    body: &FunctionBody,
    earlier_site: u32,
    later_site: u32,
) -> Option<MustExecuteBeforeWitness> {
    if !body.addresses_resolved
        || body.accounted_bytes != body.size
        || body.instructions.iter().any(|item| !item.supported)
    {
        return None;
    }
    let earlier_address = u64::from(earlier_site);
    let later_address = u64::from(later_site);
    if !body
        .instructions
        .iter()
        .any(|item| item.address == earlier_address)
        || !body
            .instructions
            .iter()
            .any(|item| item.address == later_address)
    {
        return None;
    }

    let blocks = body
        .basic_blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<BTreeMap<_, _>>();
    if blocks.len() != body.basic_blocks.len() {
        return None;
    }
    let entry = block_for_address(body, body.address)?;
    let earlier_block = block_for_address(body, earlier_address)?;
    let later_block = block_for_address(body, later_address)?;
    let mut successors = BTreeMap::<usize, BTreeSet<usize>>::new();
    let mut predecessors = BTreeMap::<usize, BTreeSet<usize>>::new();
    for block in blocks.values() {
        successors.entry(block.id).or_default();
        predecessors.entry(block.id).or_default();
        for successor in &block.successors {
            let target = successor.block?;
            if !blocks.contains_key(&target) {
                return None;
            }
            successors.entry(block.id).or_default().insert(target);
            predecessors.entry(target).or_default().insert(block.id);
        }
    }

    let mut reachable = BTreeSet::from([entry]);
    let mut pending = VecDeque::from([entry]);
    while let Some(block) = pending.pop_front() {
        for successor in successors.get(&block).into_iter().flatten() {
            if reachable.insert(*successor) {
                pending.push_back(*successor);
            }
        }
    }
    if !reachable.contains(&earlier_block) || !reachable.contains(&later_block) {
        return None;
    }
    if body.instructions.iter().any(|instruction| {
        matches!(
            instruction.control_flow.kind,
            FunctionControlFlowKind::IndirectJump | FunctionControlFlowKind::Unknown
        ) && block_for_address(body, instruction.address)
            .is_some_and(|block| reachable.contains(&block))
    }) {
        return None;
    }
    if earlier_block == later_block {
        if earlier_address < later_address {
            return Some(MustExecuteBeforeWitness {
                earlier_site,
                later_site,
                earlier_block,
                later_block,
                proof: "same-basic-block-sequence",
                path_feasibility_claim: false,
            });
        }
        return None;
    }

    let mut dominators = reachable
        .iter()
        .map(|block| {
            let initial = if *block == entry {
                BTreeSet::from([entry])
            } else {
                reachable.clone()
            };
            (*block, initial)
        })
        .collect::<BTreeMap<_, _>>();
    loop {
        let mut changed = false;
        for block in reachable.iter().copied().filter(|block| *block != entry) {
            let incoming = predecessors
                .get(&block)?
                .iter()
                .filter(|candidate| reachable.contains(candidate))
                .copied()
                .collect::<Vec<_>>();
            if incoming.is_empty() {
                return None;
            }
            let mut next = dominators.get(&incoming[0])?.clone();
            for predecessor in &incoming[1..] {
                next = next
                    .intersection(dominators.get(predecessor)?)
                    .copied()
                    .collect();
            }
            next.insert(block);
            if dominators.get(&block) != Some(&next) {
                dominators.insert(block, next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    dominators
        .get(&later_block)?
        .contains(&earlier_block)
        .then_some(MustExecuteBeforeWitness {
            earlier_site,
            later_site,
            earlier_block,
            later_block,
            proof: "dominating-basic-block",
            path_feasibility_claim: false,
        })
}

fn block_for_address(body: &FunctionBody, address: u64) -> Option<usize> {
    let offset = address.checked_sub(body.address)?;
    body.basic_blocks
        .iter()
        .find(|block| offset >= block.start_offset && offset < block.end_offset)
        .map(|block| block.id)
}

#[cfg(test)]
mod tests {
    use crate::artifact::{FunctionBasicBlock, FunctionBlockSuccessor};
    use crate::{
        FunctionControlFlow, FunctionInstruction, FunctionInstructionRelocation, FunctionLabel,
    };

    use super::*;

    fn instruction(address: u64, kind: FunctionControlFlowKind) -> FunctionInstruction {
        FunctionInstruction {
            offset: address - 0x1000,
            address,
            width: 4,
            raw: "0x00000013".to_owned(),
            text: "fixture".to_owned(),
            supported: true,
            blocker_class: None,
            control_flow: FunctionControlFlow { kind, target: None },
            relocations: Vec::<FunctionInstructionRelocation>::new(),
        }
    }

    fn successor(block: usize) -> FunctionBlockSuccessor {
        FunctionBlockSuccessor {
            kind: "fixture".to_owned(),
            block: Some(block),
            target: None,
        }
    }

    fn body(blocks: Vec<FunctionBasicBlock>) -> FunctionBody {
        FunctionBody {
            artifact: "fixture".to_owned(),
            member: None,
            symbol: "fixture".to_owned(),
            address: 0x1000,
            size: 16,
            addresses_resolved: true,
            accounted_bytes: 16,
            instructions: (0..4)
                .map(|index| instruction(0x1000 + index * 4, FunctionControlFlowKind::Linear))
                .collect(),
            basic_blocks: blocks,
            loops: Vec::new(),
            labels: Vec::<FunctionLabel>::new(),
        }
    }

    #[test]
    fn same_block_order_is_a_conditional_witness() {
        let body = body(vec![FunctionBasicBlock {
            id: 0,
            start_offset: 0,
            end_offset: 16,
            reachable: true,
            successors: Vec::new(),
        }]);
        let witness = must_execute_before(&body, 0x1004, 0x1008).unwrap();
        assert_eq!(witness.proof, "same-basic-block-sequence");
        assert!(!witness.path_feasibility_claim);
        assert!(must_execute_before(&body, 0x1008, 0x1004).is_none());
    }

    #[test]
    fn domination_accepts_join_only_when_every_path_contains_earlier_block() {
        let dominated = body(vec![
            FunctionBasicBlock {
                id: 0,
                start_offset: 0,
                end_offset: 4,
                reachable: true,
                successors: vec![successor(1)],
            },
            FunctionBasicBlock {
                id: 1,
                start_offset: 4,
                end_offset: 8,
                reachable: true,
                successors: vec![successor(2)],
            },
            FunctionBasicBlock {
                id: 2,
                start_offset: 8,
                end_offset: 16,
                reachable: true,
                successors: Vec::new(),
            },
        ]);
        assert_eq!(
            must_execute_before(&dominated, 0x1004, 0x1008)
                .unwrap()
                .proof,
            "dominating-basic-block"
        );

        let bypass = body(vec![
            FunctionBasicBlock {
                id: 0,
                start_offset: 0,
                end_offset: 4,
                reachable: true,
                successors: vec![successor(1), successor(2)],
            },
            FunctionBasicBlock {
                id: 1,
                start_offset: 4,
                end_offset: 8,
                reachable: true,
                successors: vec![successor(2)],
            },
            FunctionBasicBlock {
                id: 2,
                start_offset: 8,
                end_offset: 16,
                reachable: true,
                successors: Vec::new(),
            },
        ]);
        assert!(must_execute_before(&bypass, 0x1004, 0x1008).is_none());
    }

    #[test]
    fn unresolved_indirect_jump_fails_closed() {
        let mut body = body(vec![FunctionBasicBlock {
            id: 0,
            start_offset: 0,
            end_offset: 16,
            reachable: true,
            successors: Vec::new(),
        }]);
        body.instructions[0].control_flow.kind = FunctionControlFlowKind::IndirectJump;
        assert!(must_execute_before(&body, 0x1004, 0x1008).is_none());
    }

    #[test]
    fn unresolved_runtime_addresses_fail_closed() {
        let mut body = body(vec![FunctionBasicBlock {
            id: 0,
            start_offset: 0,
            end_offset: 16,
            reachable: true,
            successors: Vec::new(),
        }]);
        body.addresses_resolved = false;
        assert!(must_execute_before(&body, 0x1004, 0x1008).is_none());
    }
}
