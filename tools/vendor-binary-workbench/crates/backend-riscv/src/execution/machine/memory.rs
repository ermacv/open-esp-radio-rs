//! Normal-memory/MMIO access and observable-memory projection.

use std::collections::{BTreeMap, VecDeque};

use super::super::{
    ExecutionEvent, ExecutionTimelineEvent, MemoryChange, MemoryOwner, MmioValue,
    execution_stack_contains,
};
use super::Machine;
use crate::Result;

impl Machine<'_> {
    pub(in crate::execution) fn normal_byte(&self, address: u32) -> Result<u8> {
        if let Some(value) = self.overlay.get(&address).copied() {
            return Ok(value);
        }
        if let Some(error) = self.image.unresolved_relocation_error(address, "RAM read") {
            return Err(error.into());
        }
        if self.memory_ownership.iter().any(|ownership| {
            ownership.range.contains(address) && ownership.owner.may_change_outside_cpu()
        }) {
            return Err(format!(
                "read from externally mutable RAM at {address:#010x} without a call-entry seed"
            )
            .into());
        }
        if execution_stack_contains(address)
            && let Some(value) = self.private_stack_fill
        {
            return Ok(value);
        }
        self.image.byte(address).ok_or_else(|| {
            format!(
                "read from poison/unmapped memory at {address:#010x} from pc={:#010x}",
                self.pc
            )
            .into()
        })
    }

    pub(in crate::execution) fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        if self.svd.contains_mmio(address) {
            let sequenced = self
                .mmio_reads
                .get_mut(&address)
                .and_then(VecDeque::pop_front);
            let value = match sequenced {
                Some(value) => value & MmioValue::mask(width),
                None => self
                    .mmio_read_seeds
                    .get(&address)
                    .copied()
                    .map(|value| value & MmioValue::mask(width))
                    .ok_or_else(|| {
                        format!("MMIO read at {address:#010x} has no explicit seed or response")
                    })?,
            };
            self.record_event(ExecutionEvent::Read {
                width,
                address,
                register: self.svd.register_name(address),
                value,
            });
            return Ok(value);
        }
        let bytes = usize::from(width / 8);
        let mut value = 0_u32;
        for offset in 0..bytes {
            value |=
                u32::from(self.normal_byte(address.wrapping_add(offset as u32))?) << (offset * 8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamRead {
            width,
            address,
            value,
        });
        Ok(value)
    }

    pub(in crate::execution) fn normal_address_is_valid(&self, address: u32) -> bool {
        self.image.contains_memory(address)
            || execution_stack_contains(address)
            || self.initial_overlay.contains_key(&address)
            || self
                .observed_memory
                .iter()
                .any(|range| range.contains(address))
            || self.memory_aliases.iter().any(|alias| {
                address
                    .checked_sub(alias.start)
                    .is_some_and(|offset| offset < alias.length)
            })
    }

    pub(in crate::execution) fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
    ) -> Result<()> {
        let bytes = usize::from(width / 8);
        if self.svd.contains_mmio(address) {
            self.record_event(ExecutionEvent::Write {
                width,
                address,
                register: self.svd.register_name(address),
                value: value & MmioValue::mask(width),
            });
            return Ok(());
        }
        for offset in 0..bytes {
            let byte_address = address.wrapping_add(offset as u32);
            if self.memory_ownership.iter().any(|ownership| {
                ownership.range.contains(byte_address) && ownership.owner == MemoryOwner::Immutable
            }) {
                return Err(format!(
                    "write to ownership-declared immutable RAM at {byte_address:#010x}"
                )
                .into());
            }
            if self.image.contains_memory(byte_address)
                && !self.image.contains_writable_memory(byte_address)
            {
                return Err(
                    format!("write to read-only ELF memory at {byte_address:#010x}").into(),
                );
            }
            if !self.normal_address_is_valid(byte_address) {
                return Err(format!("write to undeclared memory at {byte_address:#010x}").into());
            }
            self.overlay
                .insert(byte_address, (value >> (offset * 8)) as u8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamWrite {
            width,
            address,
            value: value & MmioValue::mask(width),
        });
        Ok(())
    }

    pub(in crate::execution) fn memory_changes(&self) -> Result<Vec<MemoryChange>> {
        let mut observed_addresses: BTreeMap<u32, u32> = self
            .observed_memory
            .iter()
            .flat_map(|range| {
                (0..range.length).map(move |offset| {
                    let address = range.start.wrapping_add(offset);
                    (address, address)
                })
            })
            .collect();
        for alias in &self.memory_aliases {
            for offset in 0..alias.length {
                observed_addresses.insert(
                    alias.comparison_start.wrapping_add(offset),
                    alias.start.wrapping_add(offset),
                );
            }
        }
        let mut changes = Vec::new();
        for (comparison_address, address) in observed_addresses {
            let before = self
                .initial_overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| {
                    format!("observed memory at {address:#010x} has no explicit initial value")
                })?;
            let after = self
                .overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| format!("observed memory at {address:#010x} remains poison"))?;
            if before != after {
                changes.push(MemoryChange {
                    address: comparison_address,
                    before,
                    after,
                });
            }
        }
        Ok(changes)
    }

    pub(in crate::execution) fn persistent_memory(&self) -> BTreeMap<u32, u8> {
        self.overlay
            .iter()
            .filter_map(|(address, value)| {
                let explicitly_persistent = self
                    .persistent_memory
                    .iter()
                    .any(|range| range.contains(*address));
                ((!execution_stack_contains(*address)
                    && self.image.contains_writable_memory(*address))
                    || explicitly_persistent)
                    .then_some((*address, *value))
            })
            .collect()
    }
}
