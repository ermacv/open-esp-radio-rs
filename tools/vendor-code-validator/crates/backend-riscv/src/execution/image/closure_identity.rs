//! Address-independent identity of one linked code closure.

use std::collections::BTreeMap;

use rv_asm::{Inst, Reg};

use super::ExecutableImage;
use crate::Result;

impl ExecutableImage {
    /// Canonicalize the linked code closure rooted at one exact text symbol.
    ///
    /// The identity is deliberately independent of the surrounding ELF and
    /// of linked addresses. Exact function bytes are retained, except for
    /// direct inter-function call encodings: those are represented as stable
    /// edges to recursively canonicalized local callees. Calls to another
    /// global symbol remain named ABI edges. This makes qualification evidence
    /// local to the selected probe while still binding compiler/internal
    /// helpers which were not inlined.
    ///
    /// Direct `JAL` and adjacent `AUIPC`/`JALR` edges are followed. A symbolic
    /// ELF call relocation is retained by name. Indirect calls remain part of
    /// the caller bytes and therefore still require execution scenarios or an
    /// explicit ABI adapter to qualify their possible targets.
    pub fn code_closure_identity(&self, root_symbol: &str) -> Result<String> {
        const MAX_FUNCTIONS: usize = 4_096;
        const MAX_INSTRUCTIONS: usize = 1_000_000;

        let root = self
            .symbol_address(root_symbol)
            .ok_or_else(|| format!("execution symbol {root_symbol} was not found"))?;
        if !self.symbol_sizes_by_address.contains_key(&root) {
            return Err(
                format!("execution symbol {root_symbol} has no exact linked text size").into(),
            );
        }

        let mut addresses = vec![root];
        let mut indices = BTreeMap::from([(root, 0_usize)]);
        let mut canonical = String::from("riscv32-code-closure-v1\n");
        let mut instruction_count = 0_usize;
        let mut node = 0_usize;

        while node < addresses.len() {
            if addresses.len() > MAX_FUNCTIONS {
                return Err(format!(
                    "code closure for {root_symbol} exceeds {MAX_FUNCTIONS} functions"
                )
                .into());
            }
            let start = addresses[node];
            let size = self.symbol_sizes_by_address[&start];
            let end = start
                .checked_add(size)
                .ok_or("linked text symbol extent overflows RV32 address space")?;
            canonical.push_str(&format!("node {node} size={size}\n"));

            let mut address = start;
            while address < end {
                instruction_count += 1;
                if instruction_count > MAX_INSTRUCTIONS {
                    return Err(format!(
                        "code closure for {root_symbol} exceeds {MAX_INSTRUCTIONS} instructions"
                    )
                    .into());
                }
                let offset = address - start;

                if let Some((relocation_site, relocation)) = self.unresolved_relocation_at(address)
                {
                    if relocation_site < start {
                        return Err(format!(
                            "linked symbol at {start:#x} begins inside unresolved relocation at {relocation_site:#x}"
                        )
                        .into());
                    }
                    let relocation_offset = relocation_site - start;
                    canonical.push_str(&format!(
                        "unresolved-relocation +{relocation_offset:#x} type={} width={} symbol={}\n",
                        relocation.r_type, relocation.width, relocation.name
                    ));
                    let relocation_end =
                        relocation_site
                            .checked_add(u32::from(relocation.width))
                            .ok_or("unresolved relocation extent overflows RV32 address space")?;
                    if relocation_end > end {
                        return Err(format!(
                            "unresolved relocation at {address:#x} crosses linked symbol extent"
                        )
                        .into());
                    }
                    address = relocation_end;
                    continue;
                }

                if let Some(call) = self.relocated_call_at(address) {
                    let target_node = call
                        .target
                        .filter(|target| self.closure_owns(root, *target))
                        .map(|target| closure_node(target, &mut addresses, &mut indices));
                    canonical.push_str(&format!(
                        "reloc-call +{offset:#x} symbol={} target={}\n",
                        call.name,
                        target_node
                            .map_or_else(|| "external".to_owned(), |index| index.to_string())
                    ));
                    let pair_end = address
                        .checked_add(8)
                        .ok_or("R_RISCV_CALL pair overflows RV32 address space")?;
                    if pair_end > end {
                        return Err(format!(
                            "R_RISCV_CALL at {address:#x} exceeds linked symbol extent"
                        )
                        .into());
                    }
                    // Validate the pair even though its address-bearing bytes
                    // are intentionally excluded from the identity.
                    self.relocated_call_link_register(address)?;
                    address = pair_end;
                    continue;
                }

                let Ok((instruction, width)) = self.instruction(address) else {
                    // Rust and vendor linkers may place jump tables or literal
                    // data inside a FUNC-sized range. The exact remainder is
                    // still part of this local implementation identity, but
                    // it cannot safely be interpreted as more call edges.
                    canonical.push_str(&format!("opaque-bytes +{offset:#x} "));
                    for byte_address in address..end {
                        let byte = self.byte(byte_address).ok_or_else(|| {
                            format!("linked symbol byte is absent at {byte_address:#x}")
                        })?;
                        canonical.push_str(&format!("{byte:02x}"));
                    }
                    canonical.push('\n');
                    break;
                };
                let next = address
                    .checked_add(width)
                    .ok_or("instruction extent overflows RV32 address space")?;
                if next > end {
                    return Err(format!(
                        "instruction at {address:#x} crosses linked symbol extent"
                    )
                    .into());
                }

                if let Inst::Jal { offset: jump, dest } = instruction {
                    let target = address.wrapping_add(jump.as_u32());
                    if !(start..end).contains(&target) {
                        write_closure_edge(
                            &mut canonical,
                            "jal",
                            offset,
                            dest,
                            target,
                            root,
                            self,
                            &mut addresses,
                            &mut indices,
                        );
                        address = next;
                        continue;
                    }
                }

                if let Inst::Auipc { uimm, dest: base } = instruction
                    && next < end
                {
                    let (following, following_width) = self.instruction(next)?;
                    if let Inst::Jalr {
                        offset: jump,
                        base: jump_base,
                        dest,
                    } = following
                        && jump_base == base
                    {
                        let pair_end = next
                            .checked_add(following_width)
                            .ok_or("AUIPC/JALR extent overflows RV32 address space")?;
                        if pair_end > end {
                            return Err(format!(
                                "AUIPC/JALR at {address:#x} crosses linked symbol extent"
                            )
                            .into());
                        }
                        let target = address
                            .wrapping_add(uimm.as_u32())
                            .wrapping_add(jump.as_u32())
                            & !1;
                        write_closure_edge(
                            &mut canonical,
                            "auipc-jalr",
                            offset,
                            dest,
                            target,
                            root,
                            self,
                            &mut addresses,
                            &mut indices,
                        );
                        address = pair_end;
                        continue;
                    }
                }

                canonical.push_str(&format!("bytes +{offset:#x} "));
                for byte_address in address..next {
                    let byte = self.byte(byte_address).ok_or_else(|| {
                        format!("linked symbol byte is absent at {byte_address:#x}")
                    })?;
                    canonical.push_str(&format!("{byte:02x}"));
                }
                canonical.push('\n');
                address = next;
            }
            node += 1;
        }

        Ok(canonical)
    }

    fn closure_owns(&self, root: u32, address: u32) -> bool {
        address == root
            || (self.symbol_sizes_by_address.contains_key(&address)
                && self.local_text_symbols.contains(&address))
    }
}

fn closure_node(
    address: u32,
    addresses: &mut Vec<u32>,
    indices: &mut BTreeMap<u32, usize>,
) -> usize {
    if let Some(index) = indices.get(&address) {
        return *index;
    }
    let index = addresses.len();
    addresses.push(address);
    indices.insert(address, index);
    index
}

#[allow(
    clippy::too_many_arguments,
    reason = "closure edge rendering keeps traversal state explicit and architecture-local"
)]
fn write_closure_edge(
    canonical: &mut String,
    encoding: &str,
    offset: u32,
    destination: Reg,
    target: u32,
    root: u32,
    image: &ExecutableImage,
    addresses: &mut Vec<u32>,
    indices: &mut BTreeMap<u32, usize>,
) {
    if image.closure_owns(root, target) {
        let target_node = closure_node(target, addresses, indices);
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} target={target_node}\n",
            destination.0
        ));
    } else if let Some(symbol) = image.symbol_at(target) {
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} external-symbol={symbol}\n",
            destination.0
        ));
    } else {
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} external-address={target:#010x}\n",
            destination.0
        ));
    }
}
