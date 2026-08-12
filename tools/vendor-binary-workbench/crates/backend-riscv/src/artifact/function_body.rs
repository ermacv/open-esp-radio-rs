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
}
