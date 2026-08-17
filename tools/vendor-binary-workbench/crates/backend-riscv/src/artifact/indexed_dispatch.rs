//! Relocation-aware recovery of bounded RISC-V jump tables.

use std::collections::BTreeSet;

use object::elf::R_RISCV_32;
use rv_asm::{Inst, Reg};

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
            decoded: match &instruction {
                AnalysisInstruction::Supported(decoded) => Some(decoded.instruction),
                AnalysisInstruction::Unsupported(_) => None,
            },
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
    if indirect_sites.is_empty() {
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
            let case_address = if let Some(address) = relocation.target_address {
                address.checked_add_signed(relocation.addend)
            } else if object.address.is_some() {
                linked_initializer_word(&object.initializer, offset).map(u64::from)
            } else {
                None
            };
            let Some(case_address) = case_address else {
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
        let matching_sites = indirect_sites
            .iter()
            .copied()
            .filter(|site| {
                object
                    .address
                    .is_some_and(|address| site_references_table(&instructions, *site, address))
            })
            .collect::<Vec<_>>();
        let site = match (indirect_sites.as_slice(), matching_sites.as_slice()) {
            ([site], _) => *site,
            (_, [site]) => *site,
            _ => continue,
        };
        output.push(ArtifactIndexedDispatch {
            table: object.name.clone(),
            table_address: object.address,
            site,
            stride: ENTRY_WIDTH as u8,
            entries,
        });
    }
    output.sort_by(|left, right| (&left.table, left.site).cmp(&(&right.table, right.site)));
    Ok(output)
}

struct LightweightInstruction {
    address: u64,
    decoded: Option<Inst>,
    control_flow: FunctionControlFlow,
}

fn linked_initializer_word(initializer: &[u8], offset: u64) -> Option<u32> {
    let offset = usize::try_from(offset).ok()?;
    let end = offset.checked_add(4)?;
    let bytes: [u8; 4] = initializer.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// Associate one read-only jump table with one of several indirect sites.
/// GCC's RV32 switch lowering materializes the table base with LUI+ADDI in
/// the same short dispatch sequence. Entry relocations and in-function case
/// targets are validated independently; this only selects the owning site.
fn site_references_table(
    instructions: &[LightweightInstruction],
    site: u32,
    table_address: u32,
) -> bool {
    let Some(site_index) = instructions
        .iter()
        .position(|instruction| instruction.address == u64::from(site))
    else {
        return false;
    };
    let mut constants = [None; 32];
    constants[usize::from(Reg::ZERO.0)] = Some(0_u32);
    for instruction in &instructions[site_index.saturating_sub(12)..site_index] {
        match instruction.decoded {
            Some(Inst::Lui { uimm, dest }) => {
                constants[usize::from(dest.0)] = Some(uimm.as_u32());
            }
            Some(Inst::Addi { imm, dest, src1 }) => {
                constants[usize::from(dest.0)] = constants[usize::from(src1.0)]
                    .map(|value| value.wrapping_add(imm.as_i32() as u32));
            }
            _ => {}
        }
        if constants.contains(&Some(table_address)) {
            return true;
        }
    }
    false
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
    fn linked_initializer_supplies_absolute_case_address_without_symbol_value() {
        assert_eq!(
            linked_initializer_word(&[0x9e, 0xfd, 0x04, 0x10], 0),
            Some(0x1004_fd9e)
        );
        assert_eq!(linked_initializer_word(&[0; 3], 0), None);
    }

    #[test]
    fn local_wifi_ap_link_unit_recovers_switches_beside_runtime_callbacks() {
        let Some(artifact) = std::env::var_os("OPEN_RADIO_VENDOR_WIFI_AP_ELF") else {
            return;
        };
        let artifact = std::path::Path::new(&artifact);
        if !artifact.is_file() {
            return;
        }
        let symbols =
            super::super::load_code_symbols(artifact, "", super::super::CodeSymbolSelection::All)
                .unwrap();
        let objects = super::super::load_data_objects(artifact).unwrap();
        for (function_name, table_name, site) in [
            ("rssi_margin", ".L449", 0x1004_fc62),
            ("esf_buf_alloc", ".L46", 0x1006_8c64),
            ("esf_buf_recycle", ".L70", 0x1006_8ede),
        ] {
            let function = symbols
                .iter()
                .find(|symbol| symbol.name == function_name)
                .unwrap();
            let dispatches = recover_indexed_dispatches(function, &objects, &symbols).unwrap();
            assert!(
                dispatches
                    .iter()
                    .any(|dispatch| { dispatch.table == table_name && dispatch.site == site }),
                "{function_name} dispatches were {dispatches:?}"
            );
        }
    }

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
