//! Lossless, on-demand inspection of one function body.
//!
//! Structural analysis is intentionally allowed to stop at an unsupported
//! operation.  Human investigation is not: this view accounts for every byte
//! in the selected symbol and builds a conservative CFG from the complete,
//! loss-tolerant decode stream.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

use object::{FileKind, Object, ObjectSymbol, SymbolKind, read::archive::ArchiveFile};
use rv_asm::{Inst, Reg};

use crate::{Error, Result};

use super::{
    AnalysisInstruction, ArtifactSymbolDefinition, CodeSymbolSelection, RelocationKind,
    decode_symbol_for_analysis, load_code_symbols, unsupported_instruction_mnemonic,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionBody {
    pub artifact: String,
    pub member: Option<String>,
    pub symbol: String,
    pub address: u64,
    pub size: usize,
    pub addresses_resolved: bool,
    pub accounted_bytes: usize,
    pub instructions: Vec<FunctionInstruction>,
    pub basic_blocks: Vec<FunctionBasicBlock>,
    /// Lossless structural loop regions recovered from the conservative CFG.
    ///
    /// These facts describe graph shape only. A counted-loop candidate is
    /// emitted only for one exact affine induction pattern and remains a
    /// presentation aid rather than an execution or termination proof.
    pub loops: Vec<FunctionLoop>,
    pub labels: Vec<FunctionLabel>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionInstruction {
    pub offset: u64,
    pub address: u64,
    pub width: u8,
    pub raw: String,
    pub text: String,
    pub supported: bool,
    pub blocker_class: Option<String>,
    pub control_flow: FunctionControlFlow,
    pub relocations: Vec<FunctionInstructionRelocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionInstructionRelocation {
    pub kind: String,
    pub symbol: String,
    pub addend: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionControlFlow {
    pub kind: FunctionControlFlowKind,
    pub target: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionControlFlowKind {
    Linear,
    Branch,
    Jump,
    Call,
    IndirectCall,
    IndirectJump,
    Return,
    Trap,
    Unknown,
}

impl FunctionControlFlowKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Branch => "branch",
            Self::Jump => "jump",
            Self::Call => "call",
            Self::IndirectCall => "indirect-call",
            Self::IndirectJump => "indirect-jump",
            Self::Return => "return",
            Self::Trap => "trap",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionBasicBlock {
    pub id: usize,
    pub start_offset: u64,
    pub end_offset: u64,
    pub reachable: bool,
    pub successors: Vec<FunctionBlockSuccessor>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionBlockSuccessor {
    pub kind: String,
    pub block: Option<usize>,
    pub target: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionLoop {
    pub id: usize,
    pub kind: FunctionLoopKind,
    pub header_block: Option<usize>,
    pub latch_blocks: Vec<usize>,
    pub body_blocks: Vec<usize>,
    pub exit_blocks: Vec<usize>,
    pub parent: Option<usize>,
    pub depth: usize,
    pub counted: Option<FunctionCountedLoop>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionLoopKind {
    Natural,
    Irreducible,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionCountedLoop {
    pub induction_register: String,
    pub initial: u32,
    pub step: i32,
    pub bound: u32,
    pub comparison: &'static str,
    pub trip_count: u32,
    /// This is deliberately false. Structural affine recovery makes the loop
    /// readable but does not prove that the path is executable or terminating.
    pub execution_proof: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FunctionLabel {
    pub offset: u64,
    pub name: String,
    pub kind: String,
}

/// Inspect one exact symbol without invoking external disassembly tools.
pub fn inspect_function_body(
    artifact: &Path,
    member: Option<&str>,
    symbol: &str,
) -> Result<FunctionBody> {
    inspect_function_body_at(artifact, member, symbol, None)
}

/// Inspect one symbol at an exact linked address when duplicate symbol names
/// exist in the same image.
pub fn inspect_function_body_at(
    artifact: &Path,
    member: Option<&str>,
    symbol: &str,
    address: Option<u64>,
) -> Result<FunctionBody> {
    let mut candidates = load_code_symbols(artifact, symbol, CodeSymbolSelection::All)?
        .into_iter()
        .filter(|candidate| {
            candidate.name == symbol
                && member.is_none_or(|m| candidate.member.as_deref() == Some(m))
                && address.is_none_or(|address| candidate.address == address)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(Error::Message(format!(
            "function {symbol:?}{} was not found in {}",
            member
                .map(|member| format!(" in member {member:?}"))
                .unwrap_or_default(),
            artifact.display()
        )));
    }
    if candidates.len() != 1 {
        let choices = candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{}@{:#x}",
                    candidate.member.as_deref().unwrap_or("<linked-image>"),
                    candidate.address
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::Message(format!(
            "function {symbol:?} is ambiguous in {} ({choices}); select --member or SYMBOL@0xADDRESS",
            artifact.display()
        )));
    }
    let definition = candidates.pop().expect("one candidate");
    let labels = load_labels(artifact, &definition)?;
    build_body(artifact, definition, labels)
}

/// Decode a definition already owned by an analysis catalog without reopening
/// its container. Local labels are intentionally absent; instruction,
/// relocation, byte-accounting and CFG facts remain complete.
pub fn inspect_function_definition(definition: &ArtifactSymbolDefinition) -> Result<FunctionBody> {
    build_body(
        Path::new("<analysis-catalog>"),
        definition.clone(),
        Vec::new(),
    )
}

/// Map exact instruction addresses to conservative basic-block identifiers
/// without requiring a second artifact lookup.
pub fn basic_block_ids_for_sites(
    definition: &ArtifactSymbolDefinition,
    sites: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, usize>> {
    if sites.is_empty() {
        return Ok(BTreeMap::new());
    }
    let body = build_body(Path::new("<analysis>"), definition.clone(), Vec::new())?;
    let mut output = BTreeMap::new();
    for site in sites {
        let Some(offset) = u64::from(*site).checked_sub(definition.address) else {
            continue;
        };
        if let Some(block) = body
            .basic_blocks
            .iter()
            .find(|block| offset >= block.start_offset && offset < block.end_offset)
        {
            output.insert(*site, block.id);
        }
    }
    Ok(output)
}

pub(super) fn build_body(
    artifact: &Path,
    definition: ArtifactSymbolDefinition,
    labels: Vec<FunctionLabel>,
) -> Result<FunctionBody> {
    let decoded = decode_symbol_for_analysis(&definition)?;
    let mut instructions = Vec::with_capacity(decoded.len());
    let mut accounted_bytes = 0usize;
    for item in &decoded {
        let address = item.address();
        let width = item.width();
        let offset = address
            .checked_sub(definition.address)
            .ok_or_else(|| Error::Message("decoded instruction precedes its symbol".to_owned()))?;
        let start = usize::try_from(offset)
            .map_err(|_| Error::Message("instruction offset overflow".to_owned()))?;
        let end = start + usize::from(width);
        let bytes = definition
            .bytes
            .get(start..end)
            .ok_or_else(|| Error::Message("decoded instruction exceeds its symbol".to_owned()))?;
        let raw_value = bytes.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | (u32::from(*byte) << (index * 8))
        });
        let (text, supported, blocker_class, control_flow) = match item {
            AnalysisInstruction::Supported(decoded) => (
                decoded.instruction.to_string(),
                true,
                None,
                classify_control_flow(decoded.address, decoded.instruction),
            ),
            AnalysisInstruction::Unsupported(blocker) => (
                unsupported_instruction_mnemonic(blocker.width, blocker.raw).to_owned(),
                false,
                Some(blocker.class.as_str().to_owned()),
                FunctionControlFlow {
                    kind: if blocker.linear_control_flow {
                        FunctionControlFlowKind::Unknown
                    } else {
                        FunctionControlFlowKind::IndirectJump
                    },
                    target: None,
                },
            ),
        };
        let relocations = definition
            .relocations
            .iter()
            .filter(|relocation| {
                u64::from(relocation.address) >= address
                    && u64::from(relocation.address) < address + u64::from(width)
            })
            .map(|relocation| FunctionInstructionRelocation {
                kind: relocation_kind_label(relocation.kind).to_owned(),
                symbol: relocation.symbol.clone(),
                addend: relocation.addend,
            })
            .collect();
        instructions.push(FunctionInstruction {
            offset,
            address,
            width,
            raw: if width == 2 {
                format!("0x{raw_value:04x}")
            } else {
                format!("0x{raw_value:08x}")
            },
            text,
            supported,
            blocker_class,
            control_flow,
            relocations,
        });
        accounted_bytes += usize::from(width);
    }
    let basic_blocks = build_cfg(&instructions, definition.address, definition.bytes.len());
    let loops = recover_loops(&basic_blocks, &decoded, definition.address);
    Ok(FunctionBody {
        artifact: artifact.display().to_string(),
        member: definition.member,
        symbol: definition.name,
        address: definition.address,
        size: definition.bytes.len(),
        addresses_resolved: definition.addresses_resolved,
        accounted_bytes,
        instructions,
        basic_blocks,
        loops,
        labels,
    })
}

pub(super) fn classify_control_flow(address: u64, instruction: Inst) -> FunctionControlFlow {
    match instruction {
        Inst::Jal { offset, dest } => FunctionControlFlow {
            kind: if dest == Reg::ZERO {
                FunctionControlFlowKind::Jump
            } else {
                FunctionControlFlowKind::Call
            },
            target: Some((address as u32).wrapping_add(offset.as_i32() as u32) as u64),
        },
        Inst::Jalr { dest, base, offset } => FunctionControlFlow {
            kind: if dest == Reg::ZERO && base == Reg::RA && offset.as_i32() == 0 {
                FunctionControlFlowKind::Return
            } else if dest == Reg::ZERO {
                FunctionControlFlowKind::IndirectJump
            } else {
                FunctionControlFlowKind::IndirectCall
            },
            target: None,
        },
        Inst::Beq { offset, .. }
        | Inst::Bne { offset, .. }
        | Inst::Blt { offset, .. }
        | Inst::Bge { offset, .. }
        | Inst::Bltu { offset, .. }
        | Inst::Bgeu { offset, .. } => FunctionControlFlow {
            kind: FunctionControlFlowKind::Branch,
            target: Some((address as u32).wrapping_add(offset.as_i32() as u32) as u64),
        },
        Inst::Ebreak => FunctionControlFlow {
            kind: FunctionControlFlowKind::Trap,
            target: None,
        },
        _ => FunctionControlFlow {
            kind: FunctionControlFlowKind::Linear,
            target: None,
        },
    }
}

fn build_cfg(
    instructions: &[FunctionInstruction],
    base: u64,
    size: usize,
) -> Vec<FunctionBasicBlock> {
    if instructions.is_empty() {
        return Vec::new();
    }
    let end_address = base + size as u64;
    let index_by_address = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address, index))
        .collect::<BTreeMap<_, _>>();
    let successors = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| {
            instruction_successors(index, instruction, instructions, &index_by_address)
        })
        .collect::<Vec<_>>();
    let mut leaders = BTreeSet::from([0usize]);
    for (index, instruction) in instructions.iter().enumerate() {
        match instruction.control_flow.kind {
            FunctionControlFlowKind::Branch | FunctionControlFlowKind::Jump => {
                if let Some(target) = instruction
                    .control_flow
                    .target
                    .and_then(|target| index_by_address.get(&target).copied())
                {
                    leaders.insert(target);
                }
                if index + 1 < instructions.len() {
                    leaders.insert(index + 1);
                }
            }
            FunctionControlFlowKind::Return
            | FunctionControlFlowKind::Trap
            | FunctionControlFlowKind::IndirectJump => {
                // Bytes following a terminator remain visible as a separate,
                // initially unreachable block.
                if index + 1 < instructions.len() {
                    leaders.insert(index + 1);
                }
            }
            FunctionControlFlowKind::Linear
            | FunctionControlFlowKind::Call
            | FunctionControlFlowKind::IndirectCall
            | FunctionControlFlowKind::Unknown => {}
        }
    }
    let starts = leaders.into_iter().collect::<Vec<_>>();
    let mut block_by_instruction = vec![0usize; instructions.len()];
    for (block, start) in starts.iter().copied().enumerate() {
        let end = starts.get(block + 1).copied().unwrap_or(instructions.len());
        for slot in &mut block_by_instruction[start..end] {
            *slot = block;
        }
    }
    let mut reachable_instructions = BTreeSet::new();
    let mut pending = VecDeque::from([0usize]);
    while let Some(index) = pending.pop_front() {
        if !reachable_instructions.insert(index) {
            continue;
        }
        for (_, successor) in &successors[index] {
            if let Some(successor) = successor {
                pending.push_back(*successor);
            }
        }
    }
    starts
        .iter()
        .copied()
        .enumerate()
        .map(|(block, start)| {
            let end = starts.get(block + 1).copied().unwrap_or(instructions.len());
            let last = end - 1;
            FunctionBasicBlock {
                id: block,
                start_offset: instructions[start].offset,
                end_offset: instructions[last].offset + u64::from(instructions[last].width),
                reachable: (start..end).any(|index| reachable_instructions.contains(&index)),
                successors: successors[last]
                    .iter()
                    .map(|(kind, successor)| FunctionBlockSuccessor {
                        kind: (*kind).to_owned(),
                        block: successor.map(|index| block_by_instruction[index]),
                        target: successor
                            .map(|index| instructions[index].address)
                            .or_else(|| {
                                instructions[last]
                                    .control_flow
                                    .target
                                    .filter(|target| *target < base || *target >= end_address)
                            }),
                    })
                    .collect(),
            }
        })
        .collect()
}

fn instruction_successors(
    index: usize,
    instruction: &FunctionInstruction,
    instructions: &[FunctionInstruction],
    index_by_address: &BTreeMap<u64, usize>,
) -> Vec<(&'static str, Option<usize>)> {
    let fallthrough = (index + 1 < instructions.len()).then_some(index + 1);
    let target = instruction
        .control_flow
        .target
        .and_then(|target| index_by_address.get(&target).copied());
    match instruction.control_flow.kind {
        FunctionControlFlowKind::Branch => vec![("branch", target), ("fallthrough", fallthrough)],
        FunctionControlFlowKind::Jump => vec![("jump", target)],
        FunctionControlFlowKind::Call | FunctionControlFlowKind::IndirectCall => {
            vec![("return-site", fallthrough)]
        }
        FunctionControlFlowKind::Return
        | FunctionControlFlowKind::Trap
        | FunctionControlFlowKind::IndirectJump => Vec::new(),
        FunctionControlFlowKind::Linear | FunctionControlFlowKind::Unknown => fallthrough
            .into_iter()
            .map(|next| ("fallthrough", Some(next)))
            .collect(),
    }
}

fn recover_loops(
    blocks: &[FunctionBasicBlock],
    instructions: &[AnalysisInstruction],
    base: u64,
) -> Vec<FunctionLoop> {
    if blocks.is_empty() {
        return Vec::new();
    }
    let reachable = blocks
        .iter()
        .filter(|block| block.reachable)
        .map(|block| block.id)
        .collect::<BTreeSet<_>>();
    let mut predecessors = vec![BTreeSet::new(); blocks.len()];
    for block in blocks.iter().filter(|block| block.reachable) {
        for successor in block
            .successors
            .iter()
            .filter_map(|successor| successor.block)
        {
            if reachable.contains(&successor) {
                predecessors[successor].insert(block.id);
            }
        }
    }

    let mut dominators = vec![BTreeSet::new(); blocks.len()];
    for block in &reachable {
        dominators[*block] = if *block == 0 {
            BTreeSet::from([0])
        } else {
            reachable.clone()
        };
    }
    loop {
        let mut changed = false;
        for block in reachable.iter().copied().filter(|block| *block != 0) {
            let mut incoming = predecessors[block].iter().copied();
            let mut next = incoming
                .next()
                .map(|predecessor| dominators[predecessor].clone())
                .unwrap_or_default();
            for predecessor in incoming {
                next = next
                    .intersection(&dominators[predecessor])
                    .copied()
                    .collect();
            }
            next.insert(block);
            if next != dominators[block] {
                dominators[block] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut latches_by_header = BTreeMap::<usize, BTreeSet<usize>>::new();
    for block in blocks.iter().filter(|block| block.reachable) {
        for successor in block
            .successors
            .iter()
            .filter_map(|successor| successor.block)
        {
            if dominators[block.id].contains(&successor) {
                latches_by_header
                    .entry(successor)
                    .or_default()
                    .insert(block.id);
            }
        }
    }

    let mut regions = Vec::new();
    for (header, latches) in latches_by_header {
        let mut body = BTreeSet::from([header]);
        let mut pending = VecDeque::new();
        for latch in &latches {
            if body.insert(*latch) {
                pending.push_back(*latch);
            }
        }
        while let Some(block) = pending.pop_front() {
            for predecessor in &predecessors[block] {
                if body.insert(*predecessor) && *predecessor != header {
                    pending.push_back(*predecessor);
                }
            }
        }
        let exits = loop_exit_blocks(blocks, &body);
        regions.push(FunctionLoop {
            id: 0,
            kind: FunctionLoopKind::Natural,
            header_block: Some(header),
            latch_blocks: latches.into_iter().collect(),
            body_blocks: body.into_iter().collect(),
            exit_blocks: exits,
            parent: None,
            depth: 0,
            counted: None,
        });
    }

    for component in strongly_connected_components(blocks, &reachable) {
        let cyclic = component.len() > 1
            || component.iter().any(|block| {
                blocks[*block]
                    .successors
                    .iter()
                    .any(|successor| successor.block == Some(*block))
            });
        if !cyclic {
            continue;
        }
        let entries = component
            .iter()
            .filter(|block| {
                predecessors[**block]
                    .iter()
                    .any(|predecessor| !component.contains(predecessor))
            })
            .count();
        if entries <= 1 {
            continue;
        }
        regions.push(FunctionLoop {
            id: 0,
            kind: FunctionLoopKind::Irreducible,
            header_block: None,
            latch_blocks: Vec::new(),
            body_blocks: component.iter().copied().collect(),
            exit_blocks: loop_exit_blocks(blocks, &component),
            parent: None,
            depth: 0,
            counted: None,
        });
    }

    regions.sort_by_key(|region| {
        (
            region.body_blocks.first().copied().unwrap_or(usize::MAX),
            std::cmp::Reverse(region.body_blocks.len()),
            matches!(region.kind, FunctionLoopKind::Irreducible),
        )
    });
    for (id, region) in regions.iter_mut().enumerate() {
        region.id = id;
    }
    for child in 0..regions.len() {
        let child_body = regions[child]
            .body_blocks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let parent = regions
            .iter()
            .filter(|candidate| {
                candidate.id != child && candidate.body_blocks.len() > child_body.len()
            })
            .filter(|candidate| {
                child_body
                    .iter()
                    .all(|block| candidate.body_blocks.contains(block))
            })
            .min_by_key(|candidate| candidate.body_blocks.len())
            .map(|candidate| candidate.id);
        regions[child].parent = parent;
    }
    for index in 0..regions.len() {
        let mut depth = 0;
        let mut parent = regions[index].parent;
        while let Some(parent_id) = parent {
            depth += 1;
            parent = regions[parent_id].parent;
        }
        regions[index].depth = depth;
    }
    for region in &mut regions {
        if region.kind == FunctionLoopKind::Natural {
            region.counted = recover_counted_loop(region, blocks, instructions, base);
        }
    }
    regions
}

fn loop_exit_blocks(blocks: &[FunctionBasicBlock], body: &BTreeSet<usize>) -> Vec<usize> {
    body.iter()
        .flat_map(|block| {
            blocks[*block]
                .successors
                .iter()
                .filter_map(|successor| successor.block)
        })
        .filter(|successor| !body.contains(successor))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn strongly_connected_components(
    blocks: &[FunctionBasicBlock],
    reachable: &BTreeSet<usize>,
) -> Vec<BTreeSet<usize>> {
    fn visit(
        block: usize,
        blocks: &[FunctionBasicBlock],
        reachable: &BTreeSet<usize>,
        visited: &mut BTreeSet<usize>,
        order: &mut Vec<usize>,
    ) {
        if !visited.insert(block) {
            return;
        }
        for successor in blocks[block]
            .successors
            .iter()
            .filter_map(|successor| successor.block)
            .filter(|successor| reachable.contains(successor))
        {
            visit(successor, blocks, reachable, visited, order);
        }
        order.push(block);
    }

    let mut order = Vec::new();
    let mut visited = BTreeSet::new();
    for block in reachable {
        visit(*block, blocks, reachable, &mut visited, &mut order);
    }
    let mut reverse = vec![Vec::new(); blocks.len()];
    for block in reachable {
        for successor in blocks[*block]
            .successors
            .iter()
            .filter_map(|successor| successor.block)
            .filter(|successor| reachable.contains(successor))
        {
            reverse[successor].push(*block);
        }
    }
    fn collect(
        block: usize,
        reverse: &[Vec<usize>],
        visited: &mut BTreeSet<usize>,
        component: &mut BTreeSet<usize>,
    ) {
        if !visited.insert(block) {
            return;
        }
        component.insert(block);
        for predecessor in &reverse[block] {
            collect(*predecessor, reverse, visited, component);
        }
    }
    visited.clear();
    let mut output = Vec::new();
    while let Some(block) = order.pop() {
        if visited.contains(&block) {
            continue;
        }
        let mut component = BTreeSet::new();
        collect(block, &reverse, &mut visited, &mut component);
        output.push(component);
    }
    output
}

fn recover_counted_loop(
    region: &FunctionLoop,
    blocks: &[FunctionBasicBlock],
    instructions: &[AnalysisInstruction],
    base: u64,
) -> Option<FunctionCountedLoop> {
    let header = region.header_block?;
    let header_address = base.checked_add(blocks[header].start_offset)?;
    let latch = match region.latch_blocks.as_slice() {
        [latch] => *latch,
        _ => return None,
    };
    let terminator_address = base.checked_add(blocks[latch].end_offset)?;
    let terminator = instructions
        .iter()
        .rev()
        .find(|instruction| instruction.address() < terminator_address)?;
    let AnalysisInstruction::Supported(decoded) = terminator else {
        return None;
    };
    let (left, right, comparison) = match decoded.instruction {
        Inst::Bne { offset, src1, src2 }
            if (decoded.address as u32).wrapping_add(offset.as_i32() as u32)
                == header_address as u32 =>
        {
            (src1, src2, "not-equal")
        }
        _ => return None,
    };
    let body_ranges = region
        .body_blocks
        .iter()
        .map(|block| {
            (
                base + blocks[*block].start_offset,
                base + blocks[*block].end_offset,
            )
        })
        .collect::<Vec<_>>();
    let in_body = |address: u64| {
        body_ranges
            .iter()
            .any(|(start, end)| address >= *start && address < *end)
    };
    for (induction, bound_register) in [(left, right), (right, left)] {
        let steps = instructions
            .iter()
            .filter(|instruction| in_body(instruction.address()))
            .filter_map(|instruction| match instruction {
                AnalysisInstruction::Supported(decoded) => match decoded.instruction {
                    Inst::Addi { imm, dest, src1 } if dest == induction && src1 == induction => {
                        Some(imm.as_i32())
                    }
                    _ => None,
                },
                AnalysisInstruction::Unsupported(_) => None,
            })
            .collect::<Vec<_>>();
        let step = match steps.as_slice() {
            [step] if *step != 0 => *step,
            _ => continue,
        };
        let mut induction_writes = 0;
        for instruction in instructions
            .iter()
            .filter(|instruction| in_body(instruction.address()))
        {
            if integer_destination(*instruction)? == Some(induction) {
                induction_writes += 1;
            }
        }
        if induction_writes != 1 {
            continue;
        }
        let initial = closest_constant_assignment_before(instructions, header_address, induction);
        let bound =
            closest_constant_assignment_before(instructions, decoded.address, bound_register);
        let (Some(initial), Some(bound)) = (initial, bound) else {
            continue;
        };
        let distance = i64::from(bound).checked_sub(i64::from(initial))?;
        let step64 = i64::from(step);
        if distance == 0 || distance.signum() != step64.signum() || distance % step64 != 0 {
            continue;
        }
        let trip_count = u32::try_from(distance / step64).ok()?;
        if trip_count == 0 {
            continue;
        }
        return Some(FunctionCountedLoop {
            induction_register: induction.to_string(),
            initial,
            step,
            bound,
            comparison,
            trip_count,
            execution_proof: false,
        });
    }
    None
}

fn closest_constant_assignment_before(
    instructions: &[AnalysisInstruction],
    before: u64,
    register: Reg,
) -> Option<u32> {
    for instruction in instructions
        .iter()
        .rev()
        .filter(|instruction| instruction.address() < before)
    {
        if integer_destination(*instruction)? == Some(register) {
            return constant_assignment(*instruction, register);
        }
    }
    None
}

fn constant_assignment(instruction: AnalysisInstruction, register: Reg) -> Option<u32> {
    match instruction {
        AnalysisInstruction::Supported(decoded) => match decoded.instruction {
            Inst::Addi { imm, dest, src1 } if dest == register && src1 == Reg::ZERO => {
                Some(imm.as_u32())
            }
            _ => None,
        },
        AnalysisInstruction::Unsupported(_) => None,
    }
}

/// `None` means the decoder introduced a supported instruction whose integer
/// destination semantics this structural pass has not reviewed. The inner
/// option distinguishes a known instruction without an integer destination.
fn integer_destination(instruction: AnalysisInstruction) -> Option<Option<Reg>> {
    let AnalysisInstruction::Supported(decoded) = instruction else {
        let AnalysisInstruction::Unsupported(blocker) = instruction else {
            unreachable!()
        };
        return Some(blocker.integer_destination.map(Reg));
    };
    match decoded.instruction {
        Inst::Lui { dest, .. }
        | Inst::Auipc { dest, .. }
        | Inst::Jal { dest, .. }
        | Inst::Jalr { dest, .. }
        | Inst::Lb { dest, .. }
        | Inst::Lbu { dest, .. }
        | Inst::Lh { dest, .. }
        | Inst::Lhu { dest, .. }
        | Inst::Lw { dest, .. }
        | Inst::Lwu { dest, .. }
        | Inst::Ld { dest, .. }
        | Inst::Addi { dest, .. }
        | Inst::AddiW { dest, .. }
        | Inst::Slti { dest, .. }
        | Inst::Sltiu { dest, .. }
        | Inst::Xori { dest, .. }
        | Inst::Ori { dest, .. }
        | Inst::Andi { dest, .. }
        | Inst::Slli { dest, .. }
        | Inst::SlliW { dest, .. }
        | Inst::Srli { dest, .. }
        | Inst::SrliW { dest, .. }
        | Inst::Srai { dest, .. }
        | Inst::SraiW { dest, .. }
        | Inst::Add { dest, .. }
        | Inst::AddW { dest, .. }
        | Inst::Sub { dest, .. }
        | Inst::SubW { dest, .. }
        | Inst::Sll { dest, .. }
        | Inst::SllW { dest, .. }
        | Inst::Slt { dest, .. }
        | Inst::Sltu { dest, .. }
        | Inst::Xor { dest, .. }
        | Inst::Srl { dest, .. }
        | Inst::SrlW { dest, .. }
        | Inst::Sra { dest, .. }
        | Inst::SraW { dest, .. }
        | Inst::Or { dest, .. }
        | Inst::And { dest, .. }
        | Inst::Mul { dest, .. }
        | Inst::MulW { dest, .. }
        | Inst::Mulh { dest, .. }
        | Inst::Mulhsu { dest, .. }
        | Inst::Mulhu { dest, .. }
        | Inst::Div { dest, .. }
        | Inst::DivW { dest, .. }
        | Inst::Divu { dest, .. }
        | Inst::DivuW { dest, .. }
        | Inst::Rem { dest, .. }
        | Inst::RemW { dest, .. }
        | Inst::Remu { dest, .. }
        | Inst::RemuW { dest, .. }
        | Inst::LrW { dest, .. }
        | Inst::ScW { dest, .. }
        | Inst::AmoW { dest, .. } => Some(Some(dest)),
        Inst::Beq { .. }
        | Inst::Bne { .. }
        | Inst::Blt { .. }
        | Inst::Bge { .. }
        | Inst::Bltu { .. }
        | Inst::Bgeu { .. }
        | Inst::Sb { .. }
        | Inst::Sh { .. }
        | Inst::Sw { .. }
        | Inst::Sd { .. }
        | Inst::Fence { .. }
        | Inst::Ecall
        | Inst::Ebreak => Some(None),
        _ => None,
    }
}

fn load_labels(
    artifact: &Path,
    definition: &ArtifactSymbolDefinition,
) -> Result<Vec<FunctionLabel>> {
    let data = crate::read_artifact(artifact)?;
    match FileKind::parse(data.as_slice())? {
        FileKind::Archive => {
            let archive = ArchiveFile::parse(data.as_slice())?;
            for member in archive.members() {
                let member = member?;
                if Some(member.name()) == definition.member.as_deref().map(str::as_bytes) {
                    return collect_labels(member.data(data.as_slice())?, definition);
                }
            }
            Ok(Vec::new())
        }
        FileKind::Elf32 => collect_labels(&data, definition),
        kind => Err(format!("unsupported artifact kind: {kind:?}").into()),
    }
}

fn collect_labels(
    data: &[u8],
    definition: &ArtifactSymbolDefinition,
) -> Result<Vec<FunctionLabel>> {
    let file = object::File::parse(data)?;
    let selected = file
        .symbols()
        .find(|symbol| {
            symbol.name().ok() == Some(definition.name.as_str())
                && symbol.address() == definition.address
                && symbol.size() as usize == definition.bytes.len()
        })
        .ok_or_else(|| {
            Error::Message(format!("cannot recover section for {:?}", definition.name))
        })?;
    let section = selected.section_index();
    let end = definition.address + definition.bytes.len() as u64;
    let mut labels = file
        .symbols()
        .filter(|symbol| {
            symbol.is_definition()
                && symbol.section_index() == section
                && symbol.address() >= definition.address
                && symbol.address() < end
        })
        .filter_map(|symbol| {
            let name = symbol.name().ok()?.to_owned();
            (!name.is_empty()).then(|| FunctionLabel {
                offset: symbol.address() - definition.address,
                name,
                kind: match symbol.kind() {
                    SymbolKind::Text => "text",
                    SymbolKind::Label => "label",
                    SymbolKind::Data => "data",
                    _ => "unknown",
                }
                .to_owned(),
            })
        })
        .collect::<Vec<_>>();
    labels.sort_by(|left, right| (left.offset, &left.name).cmp(&(right.offset, &right.name)));
    labels.dedup();
    Ok(labels)
}

const fn relocation_kind_label(kind: RelocationKind) -> &'static str {
    match kind {
        RelocationKind::GotHi20 => "got-hi20",
        RelocationKind::Hi20 => "hi20",
        RelocationKind::Lo12I => "lo12-i",
        RelocationKind::Lo12S => "lo12-s",
        RelocationKind::PcRelHi20 => "pcrel-hi20",
        RelocationKind::PcRelLo12I => "pcrel-lo12-i",
        RelocationKind::PcRelLo12S => "pcrel-lo12-s",
        RelocationKind::GotPcRelLo12I => "got-pcrel-lo12-i",
        RelocationKind::Call => "call",
        RelocationKind::CallPlt => "call-plt",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn definition(words: &[u32]) -> ArtifactSymbolDefinition {
        ArtifactSymbolDefinition {
            member: None,
            name: "focused".to_owned(),
            address: 0x1000,
            bytes: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
            addresses_resolved: true,
            memory_regions: Arc::default(),
            relocations: Vec::new(),
        }
    }

    #[test]
    fn lossless_body_keeps_all_bytes_and_builds_real_basic_blocks() {
        // nop; beq zero,zero,+8; nop; ret
        let body = build_body(
            Path::new("focused.elf"),
            definition(&[0x0000_0013, 0x0000_0463, 0x0000_0013, 0x0000_8067]),
            Vec::new(),
        )
        .expect("function body");

        assert_eq!(body.accounted_bytes, 16);
        assert_eq!(body.instructions.len(), 4);
        assert_eq!(body.basic_blocks.len(), 3);
        assert!(body.basic_blocks.iter().all(|block| block.reachable));
        assert_eq!(body.basic_blocks[0].start_offset, 0);
        assert_eq!(body.basic_blocks[0].end_offset, 8);
    }

    #[test]
    fn unsupported_instruction_is_evidence_instead_of_an_early_error() {
        // flw f0, 0(zero); ret
        let body = build_body(
            Path::new("focused.elf"),
            definition(&[0x0000_2007, 0x0000_8067]),
            Vec::new(),
        )
        .expect("function body");

        assert_eq!(body.accounted_bytes, 8);
        assert!(!body.instructions[0].supported);
        assert_eq!(
            body.instructions[0].blocker_class.as_deref(),
            Some("floating-point")
        );
        assert_eq!(body.instructions[1].text, "ret");
    }

    fn supported(address: u64, instruction: Inst) -> AnalysisInstruction {
        AnalysisInstruction::Supported(super::super::DecodedInstruction {
            address,
            width: 4,
            instruction,
        })
    }

    fn block(
        id: usize,
        start_offset: u64,
        end_offset: u64,
        successors: &[usize],
    ) -> FunctionBasicBlock {
        FunctionBasicBlock {
            id,
            start_offset,
            end_offset,
            reachable: true,
            successors: successors
                .iter()
                .map(|target| FunctionBlockSuccessor {
                    kind: "test".to_owned(),
                    block: Some(*target),
                    target: Some(0x1000 + *target as u64 * 4),
                })
                .collect(),
        }
    }

    #[test]
    fn nested_natural_loops_retain_structure_and_affine_count_candidates() {
        let base = 0x1000;
        let blocks = vec![
            block(0, 0, 8, &[1]),
            block(1, 8, 12, &[2]),
            block(2, 12, 24, &[2, 3]),
            block(3, 24, 36, &[1, 4]),
            block(4, 36, 40, &[]),
        ];
        let instructions = vec![
            supported(
                base,
                Inst::Addi {
                    imm: 0_u32.into(),
                    dest: Reg::S11,
                    src1: Reg::ZERO,
                },
            ),
            supported(
                base + 4,
                Inst::Addi {
                    imm: 30_u32.into(),
                    dest: Reg::A1,
                    src1: Reg::ZERO,
                },
            ),
            supported(
                base + 8,
                Inst::Addi {
                    imm: 0_u32.into(),
                    dest: Reg::S4,
                    src1: Reg::ZERO,
                },
            ),
            supported(
                base + 12,
                Inst::Addi {
                    imm: 1_u32.into(),
                    dest: Reg::S4,
                    src1: Reg::S4,
                },
            ),
            supported(
                base + 16,
                Inst::Addi {
                    imm: 10_u32.into(),
                    dest: Reg::A2,
                    src1: Reg::ZERO,
                },
            ),
            supported(
                base + 20,
                Inst::Bne {
                    offset: (-8_i32 as u32).into(),
                    src1: Reg::S4,
                    src2: Reg::A2,
                },
            ),
            supported(
                base + 24,
                Inst::Addi {
                    imm: 10_u32.into(),
                    dest: Reg::S11,
                    src1: Reg::S11,
                },
            ),
            supported(
                base + 28,
                Inst::Addi {
                    imm: 30_u32.into(),
                    dest: Reg::A1,
                    src1: Reg::ZERO,
                },
            ),
            supported(
                base + 32,
                Inst::Bne {
                    offset: (-24_i32 as u32).into(),
                    src1: Reg::S11,
                    src2: Reg::A1,
                },
            ),
            supported(
                base + 36,
                Inst::Jalr {
                    offset: 0_u32.into(),
                    base: Reg::RA,
                    dest: Reg::ZERO,
                },
            ),
        ];

        let loops = recover_loops(&blocks, &instructions, base);
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].body_blocks, vec![1, 2, 3]);
        assert_eq!(loops[0].counted.as_ref().unwrap().trip_count, 3);
        assert_eq!(loops[1].body_blocks, vec![2]);
        assert_eq!(loops[1].parent, Some(0));
        assert_eq!(loops[1].depth, 1);
        assert_eq!(loops[1].counted.as_ref().unwrap().trip_count, 10);
        assert!(loops.iter().all(|region| {
            region
                .counted
                .as_ref()
                .is_none_or(|counted| !counted.execution_proof)
        }));
    }

    #[test]
    fn multiple_entry_cycle_is_reported_as_irreducible() {
        let blocks = vec![
            block(0, 0, 4, &[1, 2]),
            block(1, 4, 8, &[2]),
            block(2, 8, 12, &[1, 3]),
            block(3, 12, 16, &[]),
        ];
        let loops = recover_loops(&blocks, &[], 0x1000);
        assert!(loops.iter().any(|region| {
            region.kind == FunctionLoopKind::Irreducible
                && region.body_blocks == vec![1, 2]
                && region.header_block.is_none()
        }));
    }
}
