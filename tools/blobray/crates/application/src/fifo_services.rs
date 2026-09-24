//! Admitted FIFO rings and selected bindings owned by an execution session.
use crate::*;
pub(crate) struct Instance<'m> {
    pub declaration: FifoService,
    definition: ArtifactId,
    _payload: MemoryReservation<'m>,
    ring: ScratchBytes<'m>,
    head: u32,
    pub depth: u32,
    operations: u64,
    issue: Option<FifoIssue>,
}
impl Instance<'_> {
    pub fn front(&self) -> Option<u32> {
        if self.depth == 0 {
            None
        } else {
            let o = self.head as usize * 4;
            Some(u32::from_le_bytes(self.ring[o..o + 4].try_into().unwrap()))
        }
    }
}
pub(crate) struct Services<'m> {
    memory: &'m WorkingMemory,
    instances: AdmittedVec<'m, Option<Instance<'m>>>,
    index: AdmittedVec<'m, (u32, u16, u16)>,
}
impl<'m> Services<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            instances: AdmittedVec::new(memory),
            index: AdmittedVec::new(memory),
        }
    }
    pub fn get(&self, instance: u16) -> &Instance<'m> {
        self.instances[instance as usize].as_ref().unwrap()
    }
    pub fn binding(&self, instance: u16, binding: u16) -> &FifoBinding {
        &self.get(instance).declaration.bindings[binding as usize]
    }
    pub fn bindings(&self) -> impl Iterator<Item = (u16, u16, &FifoBinding)> {
        self.instances
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i, v)))
            .flat_map(|(i, v)| {
                v.declaration
                    .bindings
                    .iter()
                    .enumerate()
                    .map(move |(b, v)| (i as u16, b as u16, v))
            })
    }
    pub fn find(&self, target: u32, c: &mut dyn RunControl) -> Result<Option<(u16, u16)>> {
        c.checkpoint(self.index.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .index
            .binary_search_by_key(&target, |i| i.0)
            .ok()
            .map(|i| (self.index[i].1, self.index[i].2)))
    }
    pub fn by_id(&self, id: &str, c: &mut dyn RunControl) -> Result<Option<u16>> {
        for (i, v) in self.instances.iter().enumerate() {
            c.checkpoint(1)?;
            if v.as_ref().is_some_and(|v| v.declaration.id == id) {
                return Ok(Some(i as u16));
            }
        }
        Ok(None)
    }
    pub fn install(&mut self, declarations: &[FifoService], c: &mut dyn RunControl) -> Result<()> {
        for d in declarations {
            d.validate()?;
            c.checkpoint(self.instances.len() as u64 + 1)?;
            if self
                .instances
                .iter()
                .flatten()
                .any(|v| v.declaration.id == d.id || v.declaration.handle == d.handle)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "FIFO id/handle already live",
                ));
            }
            let capacity = self.memory.reserve(d.payload_bytes() + 64, c.position())?;
            let mut ring = self.memory.bytes(d.capacity as usize * 4, c.position())?;
            for (i, value) in d.items.iter().enumerate() {
                c.checkpoint(1)?;
                ring[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            let value = Instance {
                declaration: d.clone(),
                definition: d.identity(c)?,
                _payload: capacity,
                ring,
                head: 0,
                depth: d.items.len() as u32,
                operations: 0,
                issue: None,
            };
            if let Some(i) = self.instances.iter().position(Option::is_none) {
                self.instances[i] = Some(value);
            } else {
                if self.instances.len() == MAX_FIFO_SERVICES {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "live FIFO capacity exhausted",
                    ));
                }
                self.instances.push(Some(value), c.position())?;
            }
        }
        self.rebuild(c)
    }
    fn rebuild(&mut self, c: &mut dyn RunControl) -> Result<()> {
        while self.index.pop().is_some() {}
        for (i, v) in self.instances.iter().enumerate() {
            let Some(v) = v else { continue };
            for (b, binding) in v.declaration.bindings.iter().enumerate() {
                c.checkpoint(1)?;
                self.index
                    .push((binding.call.address, i as u16, b as u16), c.position())?;
            }
        }
        c.checkpoint(self.index.len() as u64 * (self.index.len().max(1).ilog2() as u64 + 1))?;
        self.index.sort_unstable();
        for pair in self.index.windows(2) {
            c.checkpoint(1)?;
            if pair[0].0 == pair[1].0 {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "FIFO target has multiple bindings",
                ));
            }
        }
        Ok(())
    }
    pub fn fail(&mut self, instance: u16, issue: FifoIssue) -> CallDispatch {
        self.instances[instance as usize].as_mut().unwrap().issue = Some(issue);
        CallDispatch::FifoService { instance, issue }
    }
    pub fn commit(&mut self, instance: u16, transition: FifoTransition) -> Result<u32> {
        let v = self.instances[instance as usize].as_mut().unwrap();
        let count = v.operations.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimited,
                "FIFO operation counter exhausted",
            )
        })?;
        match transition {
            FifoTransition::Enqueued { value, .. } => {
                if v.depth == v.declaration.capacity {
                    return Err(Error::new(ErrorCode::Integrity, "enqueue into full FIFO"));
                }
                let at = ((v.head + v.depth) % v.declaration.capacity) as usize * 4;
                v.ring[at..at + 4].copy_from_slice(&value.to_le_bytes());
                v.depth += 1;
            }
            FifoTransition::Dequeued { value } => {
                if v.front() != Some(value) {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "dequeue differs from FIFO front",
                    ));
                }
                v.head = (v.head + 1) % v.declaration.capacity;
                v.depth -= 1;
            }
            _ => {}
        }
        v.operations = count;
        Ok(v.depth)
    }
    pub fn finish(
        &mut self,
        close: bool,
        output: &mut Vec<FifoObservation>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        output.clear();
        for (instance, slot) in self.instances.iter_mut().enumerate() {
            let Some(v) = slot else { continue };
            c.checkpoint(1)?;
            let closed = close || v.declaration.lifetime == RegionLifetime::Phase;
            let mut o = FifoObservation {
                instance: instance as u16,
                id: v.declaration.id.clone(),
                definition: v.definition.clone(),
                lifetime: v.declaration.lifetime,
                operations: v.operations,
                depth: v.depth,
                closed,
                issue: v.issue,
                status: ModelStatus::Open,
            };
            o.status = o.expected_status();
            output.push(o);
            if closed {
                *slot = None;
            }
        }
        self.rebuild(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rings_release_at_closure_reuse_slots_and_fail_with_typed_capacity() {
        let mut d = FifoService {
            id: "queue".into(),
            applicability: "fixture".into(),
            lifetime: RegionLifetime::Phase,
            handle: 1,
            item_width: 4,
            capacity: MAX_FIFO_ITEMS,
            items: vec![7],
            bindings: vec![FifoBinding {
                table: "callbacks".into(),
                slot: 4,
                call: CallBinding {
                    address: 0x2000,
                    boundary: CallBoundary::Unmapped,
                    allow_tail: false,
                },
                argument_words: 1,
                handle_word: 0,
                operation: FifoOperation::Length,
            }],
        };
        let memory = WorkingMemory::new(400_000).unwrap();
        let mut services = Services::new(&memory);
        let c = &mut || Ok(());
        let mut observations = Vec::with_capacity(MAX_FIFO_SERVICES);
        for _ in 0..130 {
            services.install(std::slice::from_ref(&d), c).unwrap();
            assert_eq!(services.by_id("queue", c).unwrap(), Some(0));
            assert_eq!(services.get(0).front(), Some(7));
            services
                .commit(0, FifoTransition::Dequeued { value: 7 })
                .unwrap();
            services
                .commit(
                    0,
                    FifoTransition::Enqueued {
                        value: 9,
                        woke: true,
                    },
                )
                .unwrap();
            assert_eq!(services.get(0).front(), Some(9));
            services.finish(false, &mut observations, c).unwrap();
            assert_eq!(observations[0].depth, 1);
            assert!(memory.used() < 4096);
        }
        drop(services);
        assert_eq!(memory.used(), 0);
        let small = WorkingMemory::new(4096).unwrap();
        let mut services = Services::new(&small);
        assert_eq!(
            services
                .install(std::slice::from_ref(&d), c)
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(small.used(), 0);
        d.capacity = 2;
        services.install(std::slice::from_ref(&d), c).unwrap();
        drop(services);
        assert_eq!(small.used(), 0);
    }
}
