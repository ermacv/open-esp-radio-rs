//! Session-owned reviewed table metadata and bounded current-value indexes.
use crate::execution_interfaces::PreparedTable;
use crate::*;
struct Slot {
    offset: u32,
    value: u32,
}
pub(crate) struct Instance<'m> {
    pub declaration: RuntimeTable,
    pub contract: InterfaceContract,
    pub root: RuntimeRoot,
    pub words: [Option<u32>; 64],
    definition: ArtifactId,
    _capacity: MemoryReservation<'m>,
    slots: AdmittedVec<'m, Slot>,
    initialized: u32,
    conditions_checked: bool,
    pointer_installs: u32,
    writes: u64,
    calls: u64,
    issue: Option<RuntimeTableIssue>,
}
#[derive(Clone, Copy)]
struct Target {
    value: u32,
    table: u16,
    offset: u32,
}
#[derive(Clone, Copy)]
struct Owner {
    start: u32,
    end: u64,
    table: u16,
}
pub(crate) enum Association {
    None,
    Ambiguous(u32),
    Slot { table: u16, offset: u32 },
}
pub(crate) struct Tables<'m> {
    memory: &'m WorkingMemory,
    instances: AdmittedVec<'m, Option<Instance<'m>>>,
    owners: AdmittedVec<'m, Owner>,
    targets: AdmittedVec<'m, Target>,
    dirty: bool,
}
impl<'m> Tables<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            instances: AdmittedVec::new(memory),
            owners: AdmittedVec::new(memory),
            targets: AdmittedVec::new(memory),
            dirty: false,
        }
    }
    pub fn install(
        &mut self,
        p: &PreparedTable<'_>,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<u16> {
        c.checkpoint(self.instances.len() as u64 + 1)?;
        if self
            .instances
            .iter()
            .flatten()
            .any(|i| i.declaration.id == p.declaration.id)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "runtime table id already live; omit it on warm continuation",
            ));
        }
        let capacity = self.memory.reserve(
            p.declaration.payload_bytes() + p.contract.allocated_bytes() + 64,
            c.position(),
        )?;
        let mut slots = AdmittedVec::new(self.memory);
        for s in &p.declaration.slots {
            c.checkpoint(1)?;
            slots.push(
                Slot {
                    offset: s.offset,
                    value: s.target.address(),
                },
                c.position(),
            )?;
        }
        c.checkpoint(slots.len() as u64 * (slots.len().max(1).ilog2() as u64 + 1))?;
        slots.sort_unstable_by_key(|s| s.offset);
        let mut words = [None; 64];
        for (dst, src) in words.iter_mut().zip(&input.arguments) {
            *dst = *src;
        }
        let instance = Instance {
            declaration: p.declaration.clone(),
            contract: p.contract.clone(),
            root: p.root,
            words,
            definition: p.definition.clone(),
            _capacity: capacity,
            slots,
            initialized: 0,
            conditions_checked: false,
            pointer_installs: 0,
            writes: 0,
            calls: 0,
            issue: None,
        };
        let index = if let Some(i) = self.instances.iter().position(Option::is_none) {
            self.instances[i] = Some(instance);
            i
        } else {
            if self.instances.len() == MAX_RUNTIME_TABLES {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "live runtime table capacity exhausted",
                ));
            }
            let i = self.instances.len();
            self.instances.push(Some(instance), c.position())?;
            i
        };
        self.rebuild_owners(c)?;
        Ok(index as u16)
    }
    fn rebuild_owners(&mut self, c: &mut dyn RunControl) -> Result<()> {
        while self.owners.pop().is_some() {}
        for (table, i) in self.instances.iter().enumerate() {
            c.checkpoint(1)?;
            let Some(i) = i else { continue };
            self.owners.push(
                Owner {
                    start: i.declaration.seed.address,
                    end: u64::from(i.declaration.seed.address)
                        + u64::from(i.declaration.seed.length),
                    table: table as u16,
                },
                c.position(),
            )?;
        }
        c.checkpoint(self.owners.len() as u64 * (self.owners.len().max(1).ilog2() as u64 + 1))?;
        self.owners.sort_unstable_by_key(|o| o.start);
        if self
            .owners
            .windows(2)
            .any(|o| o[0].end > u64::from(o[1].start))
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "runtime table ownership overlaps",
            ));
        }
        self.dirty = true;
        Ok(())
    }
    pub fn get(&self, index: u16) -> &Instance<'m> {
        self.instances[usize::from(index)].as_ref().unwrap()
    }
    pub fn live(&self) -> impl Iterator<Item = (u16, &Instance<'m>)> {
        self.instances
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.as_ref().map(|t| (i as u16, t)))
    }
    pub fn begin_phase(&mut self) {
        for i in self.instances.iter_mut().flatten() {
            i.conditions_checked = false;
        }
    }
    pub fn checked(&mut self, index: u16) {
        self.instances[usize::from(index)]
            .as_mut()
            .unwrap()
            .conditions_checked = true;
    }
    pub fn initialized(&mut self, index: u16) {
        self.instances[usize::from(index)]
            .as_mut()
            .unwrap()
            .initialized += 1;
    }
    pub fn pointer_installed(&mut self, index: u16) {
        self.instances[usize::from(index)]
            .as_mut()
            .unwrap()
            .pointer_installs += 1;
    }
    pub fn associated(&mut self, index: u16) -> Result<()> {
        let i = self.instances[usize::from(index)].as_mut().unwrap();
        i.calls = i.calls.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimited,
                "runtime association count exhausted",
            )
        })?;
        Ok(())
    }
    pub fn fail(&mut self, index: Option<u16>, issue: RuntimeTableIssue) {
        for (n, i) in self.instances.iter_mut().enumerate() {
            if index.is_none_or(|wanted| usize::from(wanted) == n)
                && let Some(i) = i
            {
                i.issue = Some(issue);
            }
        }
    }
    pub fn note_write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        site: Option<u32>,
        c: &mut dyn RunControl,
    ) -> Result<Option<(u16, RuntimeTableEvent)>> {
        c.checkpoint(self.owners.len().max(1).ilog2() as u64 + 1)?;
        let at = self.owners.partition_point(|o| o.end <= u64::from(address));
        let Some(owner) = self
            .owners
            .get(at)
            .copied()
            .filter(|o| u64::from(o.start) < u64::from(address) + u64::from(width))
        else {
            return Ok(None);
        };
        if address < owner.start || u64::from(address) + u64::from(width) > owner.end {
            return Err(Error::new(
                ErrorCode::Integrity,
                "table write crosses admitted owner",
            ));
        }
        let i = self.instances[usize::from(owner.table)].as_mut().unwrap();
        let offset = address - owner.start;
        c.checkpoint(i.slots.len().max(1).ilog2() as u64 + 1)?;
        let slot = i
            .slots
            .partition_point(|s| u64::from(s.offset) + 4 <= u64::from(offset));
        if let Some(s) = i
            .slots
            .get_mut(slot)
            .filter(|s| s.offset < offset + u32::from(width))
        {
            let start = (offset - s.offset) as usize;
            let mut bytes = s.value.to_le_bytes();
            bytes[start..start + usize::from(width)]
                .copy_from_slice(&value.to_le_bytes()[..usize::from(width)]);
            let changed = u32::from_le_bytes(bytes);
            self.dirty |= s.value != changed;
            s.value = changed;
        }
        i.writes = i
            .writes
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "table write count exhausted"))?;
        Ok(Some((
            owner.table,
            RuntimeTableEvent::Written {
                site,
                offset,
                width,
                value,
            },
        )))
    }
    pub fn associate(&mut self, value: u32, c: &mut dyn RunControl) -> Result<Association> {
        if self.dirty {
            while self.targets.pop().is_some() {}
            for (table, i) in self.instances.iter().enumerate() {
                let Some(i) = i else { continue };
                for s in &*i.slots {
                    c.checkpoint(1)?;
                    self.targets.push(
                        Target {
                            value: s.value,
                            table: table as u16,
                            offset: s.offset,
                        },
                        c.position(),
                    )?;
                }
            }
            c.checkpoint(
                self.targets.len() as u64 * (self.targets.len().max(1).ilog2() as u64 + 1),
            )?;
            self.targets
                .sort_unstable_by_key(|t| (t.value, t.table, t.offset));
            self.dirty = false;
        }
        c.checkpoint(2 * (self.targets.len().max(1).ilog2() as u64 + 1))?;
        let low = self.targets.partition_point(|t| t.value < value);
        let high = self.targets.partition_point(|t| t.value <= value);
        Ok(match high - low {
            0 => Association::None,
            1 => Association::Slot {
                table: self.targets[low].table,
                offset: self.targets[low].offset,
            },
            n => Association::Ambiguous(n as u32),
        })
    }
    pub fn finish(
        &mut self,
        close_chain: bool,
        output: &mut Vec<RuntimeTableObservation>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        output.clear();
        for (instance, slot) in self.instances.iter_mut().enumerate() {
            let Some(i) = slot else { continue };
            c.checkpoint(1)?;
            let closed = close_chain || i.declaration.lifetime == RegionLifetime::Phase;
            let mut observed = RuntimeTableObservation {
                instance: instance as u16,
                id: i.declaration.id.clone(),
                review: i.declaration.review.clone(),
                definition: i.definition.clone(),
                lifetime: i.declaration.lifetime,
                base: i.declaration.seed.address,
                length: i.declaration.seed.length,
                initialized: i.initialized,
                expected_slots: i.declaration.slots.len() as u32,
                expected_pointers: i.declaration.pointer_cells.len() as u32,
                conditions_checked: i.conditions_checked,
                pointer_installs: i.pointer_installs,
                writes: i.writes,
                calls: i.calls,
                closed,
                issue: i.issue,
                status: ModelStatus::Open,
            };
            observed.status = observed.expected_status();
            output.push(observed);
            if closed {
                *slot = None;
            }
        }
        self.rebuild_owners(c)
    }
}
