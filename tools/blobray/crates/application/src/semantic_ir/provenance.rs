//! Bounded visited set and worklist for saved analysis provenance, never call discovery.
use super::*;
struct Entry<'m> {
    id: FunctionAnalysisId,
    selected: bool,
    visited: bool,
    _capacity: MemoryReservation<'m>,
}
pub(super) struct Closure<'m> {
    memory: &'m WorkingMemory,
    entries: AdmittedVec<'m, Entry<'m>>,
    slots: AdmittedVec<'m, usize>,
    pending: AdmittedVec<'m, usize>,
}
fn hash(id: &FunctionAnalysisId) -> usize {
    // IDs are validated hexadecimal SHA-256 values; collisions still compare full IDs.
    id.as_str().bytes().take(16).fold(0usize, |h, b| {
        h.wrapping_mul(31).wrapping_add(usize::from(b))
    })
}
impl<'m> Closure<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            entries: AdmittedVec::new(memory),
            slots: AdmittedVec::new(memory),
            pending: AdmittedVec::new(memory),
        }
    }
    fn grow(&mut self, c: &mut dyn RunControl) -> Result<()> {
        let size = self
            .slots
            .len()
            .checked_mul(2)
            .ok_or_else(|| invalid("IR provenance index overflow"))?
            .max(64);
        let mut slots = AdmittedVec::new(self.memory);
        for _ in 0..size {
            c.checkpoint(1)?;
            slots.push(usize::MAX, c.position())?;
        }
        for (i, entry) in self.entries.iter().enumerate() {
            let mut slot = hash(&entry.id) & (size - 1);
            while slots[slot] != usize::MAX {
                c.checkpoint(1)?;
                slot = (slot + 1) & (size - 1);
            }
            slots[slot] = i;
        }
        self.slots = slots;
        Ok(())
    }
    fn entry(&mut self, id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<usize> {
        if self.slots.is_empty() {
            self.grow(c)?;
        }
        loop {
            let mut slot = hash(id) & (self.slots.len() - 1);
            while self.slots[slot] != usize::MAX {
                c.checkpoint(1)?;
                let index = self.slots[slot];
                if self.entries[index].id == *id {
                    return Ok(index);
                }
                slot = (slot + 1) & (self.slots.len() - 1);
            }
            c.checkpoint(1)?;
            if self.entries.len() >= self.slots.len() / 2 {
                self.grow(c)?;
                continue;
            }
            let capacity = self.memory.reserve(id.allocated_bytes(), c.position())?;
            let index = self.entries.len();
            self.entries.push(
                Entry {
                    id: id.clone(),
                    selected: false,
                    visited: false,
                    _capacity: capacity,
                },
                c.position(),
            )?;
            self.pending.push(index, c.position())?;
            self.slots[slot] = index;
            return Ok(index);
        }
    }
    pub fn selected(&mut self, id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<()> {
        let i = self.entry(id, c)?;
        self.entries[i].selected = true;
        Ok(())
    }
    pub fn require(&mut self, id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<()> {
        self.entry(id, c)?;
        Ok(())
    }
    pub fn observe(&mut self, source: &dyn ByteSource, c: &mut dyn RunControl) -> Result<()> {
        blobray_store::visit_jsonl(source, c, |record: FunctionRecord, c| {
            match record {
                FunctionRecord::CalleeEffect { analysis, .. }
                | FunctionRecord::Expression {
                    origin: Some(analysis),
                    ..
                }
                | FunctionRecord::CallResolution {
                    analysis: Some(analysis),
                    ..
                } => self.require(&analysis, c)?,
                _ => (),
            }
            Ok(())
        })
    }
    pub fn finish(
        &mut self,
        reader: &mut blobray_store::AnalysisReader<'_>,
        revision: &RevisionId,
        output: &mut Output,
        c: &mut dyn RunControl,
    ) -> Result<u64> {
        let mut count = 0;
        while let Some(i) = self.pending.pop() {
            c.checkpoint(1)?;
            if self.entries[i].selected || self.entries[i].visited {
                continue;
            }
            self.entries[i].visited = true;
            let id = self.entries[i].id.clone();
            let lease = reader.analysis(&id, c)?;
            if lease.manifest.recipe.revision != *revision {
                return Err(invalid("IR evidence belongs to another source revision"));
            }
            self.observe(&lease.records, c)?;
            output.emit(
                &SemanticIrRecord::Function {
                    function: NavigationFunction {
                        analysis: id,
                        location: FunctionLocation {
                            source: lease.manifest.recipe.source.clone(),
                            selector: lease.manifest.recipe.selector.clone(),
                        },
                    },
                    manifest: Box::new(lease.manifest),
                    name: None,
                    profiles: vec![],
                    roots: vec![],
                    provenance_only: true,
                },
                c,
            )?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_dependencies_do_not_requeue_or_grow_and_failed_capacity_releases() {
        let memory = WorkingMemory::new(256 * 1024).unwrap();
        let mut closure = Closure::new(&memory);
        let ids: Vec<FunctionAnalysisId> = (0u32..500)
            .map(|i| {
                ArtifactId::of_bytes(&i.to_le_bytes())
                    .as_str()
                    .parse()
                    .unwrap()
            })
            .collect();
        for id in &ids {
            closure.require(id, &mut || Ok(())).unwrap();
        }
        let used = memory.used();
        for _ in 0..3 {
            for id in &ids {
                closure.selected(id, &mut || Ok(())).unwrap();
            }
        }
        assert_eq!(closure.pending.len(), ids.len());
        assert_eq!(memory.used(), used);
        assert!(closure.entries.iter().all(|e| e.selected));
        drop(closure);
        assert_eq!(memory.used(), 0);
        let small = WorkingMemory::new(1024).unwrap();
        let mut closure = Closure::new(&small);
        assert!(
            ids.iter()
                .any(|id| closure.require(id, &mut || Ok(())).is_err())
        );
        drop(closure);
        assert_eq!(small.used(), 0);
    }
}
