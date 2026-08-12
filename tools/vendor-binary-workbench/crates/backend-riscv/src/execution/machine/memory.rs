//! Normal-memory/MMIO access and observable-memory projection.

use std::collections::{BTreeMap, VecDeque};

use super::super::{
    ExecutionEvent, ExecutionTimelineEvent, MemoryChange, MemoryOwner, MemoryRange, MmioValue,
    TableLifecycleEvent, execution_stack_contains,
};
use super::Machine;
use crate::{MmioAccessKind, Result};

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
        if self.svd.intersects_mmio(address, width) {
            let identity = self
                .svd
                .classify_access(address, width, MmioAccessKind::Read)
                .map_err(|error| error.to_string())?;
            let device = self
                .devices
                .iter_mut()
                .find(|device| device.contains_access(address, width));
            let value = if let Some(device) = device {
                device.instance.read(address, width)? & MmioValue::mask(width)
            } else {
                let sequenced = self
                    .mmio_reads
                    .get_mut(&address)
                    .and_then(VecDeque::pop_front);
                match sequenced {
                    Some(value) => value & MmioValue::mask(width),
                    None => self
                        .mmio_read_seeds
                        .get(&address)
                        .copied()
                        .map(|value| value & MmioValue::mask(width))
                        .ok_or_else(|| {
                            let recent_start = self.events.len().saturating_sub(4);
                            let prior_same_address = self
                                .events
                                .iter()
                                .zip(&self.event_producers)
                                .enumerate()
                                .filter_map(|(index, (event, producer))| match event {
                                    ExecutionEvent::Read {
                                        address: previous,
                                        value,
                                        ..
                                    } if *previous == address => Some(format!(
                                        "#{index}:read={value:#010x}@pc={:#010x}:{}",
                                        producer.pc,
                                        producer.symbol.as_deref().unwrap_or("<unknown>"),
                                    )),
                                    ExecutionEvent::Write {
                                        address: previous,
                                        value,
                                        ..
                                    } if *previous == address => Some(format!(
                                        "#{index}:write={value:#010x}@pc={:#010x}:{}",
                                        producer.pc,
                                        producer.symbol.as_deref().unwrap_or("<unknown>"),
                                    )),
                                    _ => None,
                                })
                                .collect::<Vec<_>>();
                            format!(
                                "MMIO read at {address:#010x} has no explicit seed or response \
                                 (pc={:#010x}, step={}, observable-event={}, recent-events={:?}, \
                                 recent-producers={:?}, prior-same-address={prior_same_address:?})",
                                self.pc,
                                self.steps,
                                self.events.len(),
                                &self.events[recent_start..],
                                &self.event_producers[recent_start..],
                            )
                        })?,
                }
            };
            self.record_event(ExecutionEvent::Read {
                width: identity.width,
                address: identity.address,
                region: identity.region,
                register: identity.register,
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
            site: self.pc,
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
            || self.table_layouts.iter().any(|layout| {
                MemoryRange {
                    start: layout.base_address,
                    length: layout.layout_size,
                }
                .contains(address)
            })
    }

    pub(in crate::execution) fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
    ) -> Result<()> {
        let bytes = usize::from(width / 8);
        if self.svd.intersects_mmio(address, width) {
            let identity = self
                .svd
                .classify_access(address, width, MmioAccessKind::Write)
                .map_err(|error| error.to_string())?;
            if let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.contains_access(address, width))
            {
                device.instance.write(address, width, value)?;
            }
            self.record_event(ExecutionEvent::Write {
                width: identity.width,
                address: identity.address,
                region: identity.region,
                register: identity.register,
                value: value & MmioValue::mask(width),
            });
            return Ok(());
        }
        let write_end = address.wrapping_add(u32::from(width / 8));
        if self.word_reservation.is_some_and(|reserved| {
            let reserved_end = reserved.wrapping_add(4);
            address < reserved_end && reserved < write_end
        }) {
            self.word_reservation = None;
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
            site: self.pc,
            width,
            address,
            value: value & MmioValue::mask(width),
        });
        let table_writes = self
            .table_layouts
            .iter()
            .filter(|layout| {
                MemoryRange {
                    start: layout.base_address,
                    length: layout.layout_size,
                }
                .contains_access(address, width)
            })
            .map(|layout| TableLifecycleEvent::SlotWritten {
                layout_id: layout.layout_id.clone(),
                offset: address.wrapping_sub(layout.base_address),
                width,
                value: value & MmioValue::mask(width),
                site: self.pc,
            })
            .collect::<Vec<_>>();
        self.table_lifecycle.extend(table_writes);
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
