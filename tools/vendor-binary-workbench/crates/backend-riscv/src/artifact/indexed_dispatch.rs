//! Relocation-aware recovery of bounded RISC-V jump tables.

use std::collections::BTreeSet;

use object::elf::R_RISCV_32;

use crate::Result;

use super::function_body::classify_control_flow;
use super::{
    AnalysisInstruction, ArtifactDataObjectDefinition, ArtifactIndexedDispatch,
    ArtifactIndexedDispatchCallee, ArtifactIndexedDispatchEntry, ArtifactSymbolDefinition,
    FunctionControlFlow, FunctionControlFlowKind, RelocationKind, decode_symbol_for_analysis,
};

const ENTRY_WIDTH: u64 = 4;
const MAX_TABLE_ENTRIES: usize = 1_024;
const MAX_CASE_SCAN_BYTES: u64 = 96;

/// Recover selector-to-case facts for one function from bounded read-only
/// data objects.  A candidate is accepted only when it starts with at least
/// two consecutive `R_RISCV_32` entries and every entry targets this exact
/// function.  This intentionally fails closed instead of guessing from an
/// arbitrary array of word-sized constants.
pub fn recover_indexed_dispatches(
    definition: &ArtifactSymbolDefinition,
    data_objects: &[ArtifactDataObjectDefinition],
    code_symbols: &[ArtifactSymbolDefinition],
) -> Result<Vec<ArtifactIndexedDispatch>> {
    // This runs for every artifact-wide function. Keep only addresses and
    // control-flow classes; building the human FunctionBody here used to
    // allocate instruction text, raw strings, relocations and a second CFG
    // for every symbol and pushed the real project beyond 1 GiB.
    let instructions = decode_symbol_for_analysis(definition)?
        .into_iter()
        .map(|instruction| LightweightInstruction {
            address: instruction.address(),
            control_flow: match instruction {
                AnalysisInstruction::Supported(decoded) => {
                    classify_control_flow(decoded.address, decoded.instruction)
                }
                AnalysisInstruction::Unsupported(blocker) => FunctionControlFlow {
                    kind: if blocker.linear_control_flow {
                        FunctionControlFlowKind::Unknown
                    } else {
                        FunctionControlFlowKind::IndirectJump
                    },
                    target: None,
                },
            },
        })
        .collect::<Vec<_>>();
    let indirect_sites = instructions
        .iter()
        .filter(|instruction| {
            instruction.control_flow.kind == FunctionControlFlowKind::IndirectJump
        })
        .filter_map(|instruction| u32::try_from(instruction.address).ok())
        .collect::<Vec<_>>();
    if indirect_sites.len() != 1 {
        return Ok(Vec::new());
    }

    let function_end = definition
        .address
        .checked_add(definition.bytes.len() as u64)
        .ok_or("function address range overflows")?;
    let mut output = Vec::new();
    for object in data_objects.iter().filter(|object| {
        !object.writable
            && object.initialized
            && object.member == definition.member
            && object.size >= ENTRY_WIDTH * 2
    }) {
        // Relocations are sorted by object offset. Walking the required
        // prefix directly avoids constructing thousands of tiny maps for
        // every function containing an indirect jump; that pattern causes
        // allocator fragmentation on artifact-wide profiles.
        let mut relocations = object.relocations.iter().peekable();
        let mut entries = Vec::new();
        for index in 0..MAX_TABLE_ENTRIES {
            let offset = index as u64 * ENTRY_WIDTH;
            while relocations
                .peek()
                .is_some_and(|relocation| relocation.offset < offset)
            {
                relocations.next();
            }
            let Some(relocation) = relocations.next_if(|relocation| {
                relocation.offset == offset && relocation.elf_type == Some(R_RISCV_32)
            }) else {
                break;
            };
            let Some(case_address) = relocation
                .target_address
                .and_then(|address| address.checked_add_signed(relocation.addend))
            else {
                break;
            };
            if !(definition.address..function_end).contains(&case_address) {
                entries.clear();
                break;
            }
            entries.push(ArtifactIndexedDispatchEntry {
                selector: index as u32,
                case_target: relocation.target.clone(),
                case_address: case_address as u32,
                callees: case_callees(case_address, &instructions, definition, code_symbols),
            });
        }
        if entries.len() < 2 {
            continue;
        }
        output.push(ArtifactIndexedDispatch {
            table: object.name.clone(),
            table_address: object.address,
            site: indirect_sites[0],
            stride: ENTRY_WIDTH as u8,
            entries,
        });
    }
    output.sort_by(|left, right| (&left.table, left.site).cmp(&(&right.table, right.site)));
    Ok(output)
}

struct LightweightInstruction {
    address: u64,
    control_flow: FunctionControlFlow,
}

fn case_callees(
    case_address: u64,
    instructions: &[LightweightInstruction],
    definition: &ArtifactSymbolDefinition,
    code_symbols: &[ArtifactSymbolDefinition],
) -> Vec<ArtifactIndexedDispatchCallee> {
    let end = case_address.saturating_add(MAX_CASE_SCAN_BYTES);
    let mut output = BTreeSet::new();
    for instruction in instructions
        .iter()
        .skip_while(|instruction| instruction.address < case_address)
        .take_while(|instruction| instruction.address < end)
    {
        if instruction.control_flow.kind == FunctionControlFlowKind::Call {
            if let Some(relocation) = definition.relocations.iter().find(|relocation| {
                u64::from(relocation.address) == instruction.address
                    && matches!(
                        relocation.kind,
                        RelocationKind::Call | RelocationKind::CallPlt
                    )
            }) {
                output.insert(ArtifactIndexedDispatchCallee {
                    site: instruction.address as u32,
                    symbol: relocation.symbol.clone(),
                    address: instruction
                        .control_flow
                        .target
                        .and_then(|address| u32::try_from(address).ok()),
                });
            } else if let Some(address) = instruction.control_flow.target {
                let symbol = code_symbols.iter().find(|symbol| symbol.address == address);
                output.insert(ArtifactIndexedDispatchCallee {
                    site: instruction.address as u32,
                    symbol: symbol.map_or_else(
                        || format!("sub_{address:08x}"),
                        |symbol| symbol.name.clone(),
                    ),
                    address: u32::try_from(address).ok(),
                });
            }
        }
        if matches!(
            instruction.control_flow.kind,
            FunctionControlFlowKind::Jump
                | FunctionControlFlowKind::IndirectJump
                | FunctionControlFlowKind::Return
                | FunctionControlFlowKind::Trap
        ) {
            break;
        }
    }
    output.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_libpp_link_unit_recovers_pp_task_selector_25() {
        let Some(artifact) = std::env::var_os("OPEN_RADIO_VENDOR_LIBPP_ELF") else {
            return;
        };
        let artifact = std::path::Path::new(&artifact);
        if !artifact.is_file() {
            // The linked oracle is caller-owned and deliberately absent from
            // a clean checkout. CI still exercises the pure schema/executor
            // tests; a developer checkout gets this real-binary regression.
            return;
        }
        let symbols =
            super::super::load_code_symbols(artifact, "", super::super::CodeSymbolSelection::All)
                .unwrap();
        let function = symbols
            .iter()
            .find(|symbol| symbol.name == "ppTask")
            .unwrap();
        let objects = super::super::load_data_objects(artifact).unwrap();
        assert!(
            objects
                .iter()
                .map(|object| object.initializer.len())
                .sum::<usize>()
                < 8 * 1024 * 1024,
            "bounded data objects unexpectedly duplicate section storage"
        );
        let table = objects
            .iter()
            .find(|object| {
                object.name == ".L1019" || object.aliases.iter().any(|alias| alias == ".L1019")
            })
            .unwrap();
        assert!(
            table.size < 0x100,
            "jump-table anchor was not bounded: {:#x}",
            table.size
        );
        assert_eq!(
            table.initializer.len() as u64,
            table.size,
            "bounded anchor retained the remainder of its section"
        );

        let dispatches = recover_indexed_dispatches(function, &objects, &symbols).unwrap();
        let dispatch = dispatches
            .iter()
            .find(|dispatch| dispatch.table == table.name)
            .unwrap();
        let selector = dispatch
            .entries
            .iter()
            .find(|entry| entry.selector == 25)
            .unwrap();
        assert_eq!(selector.case_target, ".L1026");
        assert!(
            selector
                .callees
                .iter()
                .any(|callee| callee.symbol == "wdevProcessRxSucDataAll"),
            "selector 25 callees were {:?}",
            selector.callees
        );
    }
}
