//! Symbol, relocation, memory-byte and instruction queries.

use std::collections::BTreeMap;

use rv_asm::{Inst, Reg, Xlen};

use super::{ExecutableImage, RelocatedCall, UnresolvedRelocation};
use crate::Result;

impl ExecutableImage {
    pub fn symbol_address(&self, name: &str) -> Option<u32> {
        self.symbols_by_name.get(name).copied()
    }

    /// Return the half-open text extent of one linked symbol.
    ///
    /// The blobray uses this only to distinguish calls issued directly by
    /// an architectural root from calls made by its children. Linked ELF
    /// symbols are address ordered, so the next text symbol is the fail-closed
    /// end boundary even when the input symbol table omits explicit sizes.
    pub fn symbol_extent(&self, name: &str) -> Option<std::ops::Range<u32>> {
        let start = self.symbol_address(name)?;
        if let Some(size) = self.symbol_sizes_by_address.get(&start) {
            return start.checked_add(*size).map(|end| start..end);
        }
        let end = self
            .symbols_by_address
            .range((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .next()
            .map(|(address, _)| *address)?;
        Some(start..end)
    }

    pub(in crate::execution) fn symbol_at(&self, address: u32) -> Option<&str> {
        self.symbols_by_address.get(&address).map(String::as_str)
    }

    pub(in crate::execution) fn call_symbol_at(&self, address: u32) -> Option<&str> {
        self.symbol_at(address)
            .map(|symbol| symbol.strip_prefix("__call_").unwrap_or(symbol))
    }

    pub(in crate::execution) fn symbol_containing(&self, address: u32) -> Option<(u32, &str)> {
        let (start, name) = self.symbols_by_address.range(..=address).next_back()?;
        let end = self
            .symbol_sizes_by_address
            .get(start)
            .and_then(|size| start.checked_add(*size))
            .or_else(|| {
                self.symbols_by_address
                    .range((
                        std::ops::Bound::Excluded(*start),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .map(|(next, _)| *next)
            });
        end.map_or(address == *start, |end| address < end)
            .then_some((*start, name.as_str()))
    }

    pub(in crate::execution) fn relocated_call_at(&self, address: u32) -> Option<&RelocatedCall> {
        self.relocated_calls_by_address.get(&address)
    }

    pub(in crate::execution) fn unresolved_relocation_at(
        &self,
        address: u32,
    ) -> Option<(u32, &UnresolvedRelocation)> {
        // RISC-V relocations in allocated sections are at most eight bytes in
        // the supported RV32 oracle format. Searching the preceding seven
        // sites also detects a read/fetch into the middle of a relocated word.
        self.unresolved_relocations_by_address
            .range(address.saturating_sub(7)..=address)
            .rev()
            .find_map(|(start, relocation)| {
                address
                    .checked_sub(*start)
                    .is_some_and(|offset| offset < u32::from(relocation.width))
                    .then_some((*start, relocation))
            })
    }

    pub(in crate::execution) fn unresolved_relocation_error(
        &self,
        address: u32,
        operation: &str,
    ) -> Option<String> {
        self.unresolved_relocation_at(address)
            .map(|(site, relocation)| {
                format!(
                    "{operation} reached unresolved ELF relocation type {} to {} at {site:#010x}",
                    relocation.r_type, relocation.name
                )
            })
    }

    pub fn relocated_calls(&self) -> BTreeMap<u32, (String, Option<u32>)> {
        self.relocated_calls_by_address
            .iter()
            .map(|(address, call)| (*address, (call.name.clone(), call.target)))
            .collect()
    }

    pub(in crate::execution) fn relocated_call_link_register(&self, address: u32) -> Result<Reg> {
        match self.instruction(address.wrapping_add(4))?.0 {
            Inst::Jalr { dest, .. } => Ok(dest),
            instruction => Err(format!(
                "R_RISCV_CALL at {address:#x} is not followed by JALR: {instruction}"
            )
            .into()),
        }
    }

    pub fn location(&self, address: u32) -> String {
        self.symbols_by_address
            .range(..=address)
            .next_back()
            .map_or_else(
                || format!("{address:#010x}"),
                |(start, symbol)| format!("{symbol}+{:#x}", address.wrapping_sub(*start)),
            )
    }

    pub(in crate::execution) fn byte(&self, address: u32) -> Option<u8> {
        if self.unresolved_relocation_at(address).is_some() {
            return None;
        }
        self.segments.iter().find_map(|segment| {
            let offset = address.checked_sub(segment.address)? as usize;
            if offset >= segment.memory_size as usize {
                None
            } else {
                Some(segment.bytes.get(offset).copied().unwrap_or(0))
            }
        })
    }

    /// Returns one byte from the linked ELF load image, including the
    /// zero-filled `p_memsz - p_filesz` tail used for BSS.
    pub fn loaded_byte(&self, address: u32) -> Option<u8> {
        self.byte(address)
    }

    /// Returns one little-endian word only when every byte belongs to the
    /// same immutable ELF load segment.
    ///
    /// Coverage propagation may use linked read-only data as a constant, but
    /// must not make the same assumption for `.data`, BSS, or scenario-owned
    /// RAM. This is what lets a concrete selector resolve a compiler-emitted
    /// jump table without turning mutable function-pointer tables into proof.
    pub(in crate::execution) fn immutable_word(&self, address: u32) -> Option<u32> {
        let segment = self.segments.iter().find(|segment| {
            if segment.writable {
                return false;
            }
            address
                .checked_sub(segment.address)
                .and_then(|offset| offset.checked_add(4))
                .is_some_and(|end| end <= segment.memory_size)
        })?;
        let offset = address.checked_sub(segment.address)? as usize;
        let mut bytes = [0_u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let byte_address = address.checked_add(index as u32)?;
            if self.unresolved_relocation_at(byte_address).is_some() {
                return None;
            }
            *byte = segment.bytes.get(offset + index).copied().unwrap_or(0);
        }
        Some(u32::from_le_bytes(bytes))
    }

    pub(in crate::execution) fn contains_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            address
                .checked_sub(segment.address)
                .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    pub(in crate::execution) fn contains_writable_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            segment.writable
                && address
                    .checked_sub(segment.address)
                    .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    pub(in crate::execution) fn instruction(&self, address: u32) -> Result<(Inst, u32)> {
        if let Some(error) = self.unresolved_relocation_error(address, "instruction fetch") {
            return Err(error.into());
        }
        let low = self
            .byte(address)
            .ok_or_else(|| {
                let symbol = self.symbol_at(address).unwrap_or("<unnamed>");
                let call_symbol = self.call_symbol_at(address).unwrap_or("<unresolved>");
                let trampoline = self.call_trampoline_addresses.contains(&address);
                format!(
                    "instruction fetch outside image at {address:#x} (symbol={symbol}, call-symbol={call_symbol}, call-trampoline={trampoline})"
                )
            })?;
        let width = if Inst::first_byte_is_compressed(low) {
            2
        } else {
            4
        };
        let mut word = [0_u8; 4];
        for (offset, byte) in word.iter_mut().take(width as usize).enumerate() {
            let byte_address = address.wrapping_add(offset as u32);
            if let Some(error) = self.unresolved_relocation_error(byte_address, "instruction fetch")
            {
                return Err(error.into());
            }
            *byte = self
                .byte(byte_address)
                .ok_or_else(|| format!("truncated instruction at {address:#x}"))?;
        }
        let (instruction, _) = Inst::decode(u32::from_le_bytes(word), Xlen::Rv32)
            .map_err(|error| format!("cannot decode instruction at {address:#x}: {error}"))?;
        Ok((instruction, width))
    }
}
