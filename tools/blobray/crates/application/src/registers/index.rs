//! Operation-local declaration indexes. No per-access scan of all knowledge.
use super::*;
type BindingSink<'a> = dyn FnMut(&KnowledgeEntry, u64, u64, &mut dyn RunControl) -> Result<()> + 'a;
struct Entry<'a> {
    entry: &'a KnowledgeEntry,
    start: u64,
    end: u64,
    prefix_end: u64,
}
pub(super) struct Index<'a, 'm> {
    entries: AdmittedVec<'m, Entry<'a>>,
}
fn scope(e: &KnowledgeEntry) -> (&FunctionSource, &ObjectId, bool) {
    (
        &e.proposal.occurrence.source,
        &e.proposal.occurrence.object,
        matches!(e.proposal.claim, KnowledgeClaim::MmioRegion { .. }),
    )
}
fn name(e: &KnowledgeEntry) -> &str {
    match &e.proposal.claim {
        KnowledgeClaim::MmioRegister { register } => &register.name,
        KnowledgeClaim::MmioRegion { region } => &region.name,
        _ => unreachable!("indexed MMIO claim"),
    }
}
impl<'a, 'm> Index<'a, 'm> {
    pub fn new(
        entries: &[&'a KnowledgeEntry],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            entries: AdmittedVec::new(memory),
        };
        for &entry in entries {
            c.checkpoint(1)?;
            let (start, end) = range(&entry.proposal.claim).expect("indexed MMIO claim");
            out.entries.push(
                Entry {
                    entry,
                    start,
                    end,
                    prefix_end: end,
                },
                c.position(),
            )?;
        }
        c.checkpoint(entries.len() as u64 * (entries.len().max(1).ilog2() as u64 + 1))?;
        out.entries.sort_unstable_by(|a, b| {
            scope(a.entry)
                .cmp(&scope(b.entry))
                .then(a.start.cmp(&b.start))
                .then(a.entry.id.cmp(&b.entry.id))
        });
        for i in 1..out.entries.len() {
            c.checkpoint(1)?;
            if scope(out.entries[i].entry) == scope(out.entries[i - 1].entry) {
                out.entries[i].prefix_end = out.entries[i].end.max(out.entries[i - 1].prefix_end);
            }
        }
        Ok(out)
    }
    pub fn bindings(
        &self,
        source: &FunctionSource,
        object: &ObjectId,
        start: u64,
        end: u64,
        c: &mut dyn RunControl,
        emit: &mut BindingSink<'_>,
    ) -> Result<()> {
        for region in [false, true] {
            c.checkpoint(2 * (self.entries.len().max(1).ilog2() as u64 + 1))?;
            let key = (source, object, region);
            let lower = self.entries.partition_point(|e| scope(e.entry) < key);
            let upper = self.entries.partition_point(|e| {
                scope(e.entry) < key || scope(e.entry) == key && e.start < end
            });
            for e in self.entries[lower..upper].iter().rev() {
                c.checkpoint(1)?;
                if e.prefix_end <= start {
                    break;
                }
                if e.end > start {
                    emit(e.entry, e.start, e.end, c)?;
                }
            }
        }
        Ok(())
    }
    pub fn conflicts(
        &self,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(&KnowledgeEntry, &KnowledgeEntry, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let mut names = AdmittedVec::new(memory);
        for (i, right) in self.entries.iter().enumerate() {
            c.checkpoint(1)?;
            names.push(right.entry, c.position())?;
            for left in self.entries[..i].iter().rev() {
                c.checkpoint(1)?;
                if scope(left.entry) != scope(right.entry) || left.prefix_end <= right.start {
                    break;
                }
                if left.end > right.start && incompatible(left.entry, right.entry) {
                    emit(left.entry, right.entry, c)?;
                }
            }
        }
        c.checkpoint(names.len() as u64 * (names.len().max(1).ilog2() as u64 + 1))?;
        names.sort_unstable_by(|a, b| {
            (scope(a), name(a))
                .cmp(&(scope(b), name(b)))
                .then(a.id.cmp(&b.id))
        });
        for (i, &right) in names.iter().enumerate() {
            let (rs, re) = range(&right.proposal.claim).expect("indexed claim");
            for &left in names[..i].iter().rev() {
                c.checkpoint(1)?;
                if (scope(left), name(left)) != (scope(right), name(right)) {
                    break;
                }
                let (ls, le) = range(&left.proposal.claim).expect("indexed claim");
                // Geometric overlaps were emitted by the first index.
                if (le <= rs || re <= ls) && incompatible(left, right) {
                    emit(left, right, c)?;
                }
            }
        }
        Ok(())
    }
}
fn incompatible(a: &KnowledgeEntry, b: &KnowledgeEntry) -> bool {
    matches!(a.state, AssertionState::Proposed | AssertionState::Accepted)
        && matches!(b.state, AssertionState::Proposed | AssertionState::Accepted)
        && blobray_knowledge::conflicts(&a.proposal, &b.proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(address: u32, name: &str, input: u64) -> KnowledgeEntry {
        let artifact = ArtifactId::of_bytes(&address.to_le_bytes());
        let id = artifact.as_str();
        KnowledgeEntry {
            id: id.parse().unwrap(),
            proposed_in: id.parse().unwrap(),
            last_revision: id.parse().unwrap(),
            state: AssertionState::Accepted,
            proposal: KnowledgeProposal {
                subject: name.to_owned().try_into().unwrap(),
                occurrence: KnowledgeOccurrence {
                    revision: ArtifactId::of_bytes(b"revision").as_str().parse().unwrap(),
                    source: FunctionSource::Input { input },
                    object: ObjectId {
                        artifact: ArtifactId::of_bytes(b"object"),
                        location: ObjectLocation::Standalone,
                    },
                    symbol: None,
                },
                claim: KnowledgeClaim::MmioRegister {
                    register: MmioRegister {
                        name: name.into(),
                        address,
                        width: 4,
                        fields: vec![],
                    },
                },
                evidence: vec![EvidenceRef::Document { payload: artifact }],
                note: None,
            },
        }
    }
    #[test]
    fn indexed_bindings_preserve_scope_and_conflicts_ignore_retired_claims() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut entries: Vec<_> = (0..1000)
            .map(|i| entry(4 * i, &format!("R{i}"), 0))
            .collect();
        let mut same = entries[500].clone();
        same.state = AssertionState::Superseded;
        if let KnowledgeClaim::MmioRegister { register } = &mut same.proposal.claim {
            register.name = "OLD".into();
        }
        entries.push(same);
        entries.push(entry(2000, "OTHER_SCOPE", 1));
        // Same name at a disjoint address remains a conflict, independently of geometry.
        entries.push(entry(8000, "R500", 0));
        let refs: Vec<_> = entries.iter().collect();
        let index = Index::new(&refs, &memory, &mut || Ok(())).unwrap();
        let mut matches = Vec::new();
        let occurrence = &entries[500].proposal.occurrence;
        index
            .bindings(
                &occurrence.source,
                &occurrence.object,
                2001,
                2002,
                &mut || Ok(()),
                &mut |e, _, _, _| {
                    matches.push(name(e).to_owned());
                    Ok(())
                },
            )
            .unwrap();
        matches.sort();
        assert_eq!(matches, ["OLD", "R500"]);
        let mut conflicts = 0;
        index
            .conflicts(&memory, &mut || Ok(()), &mut |_, _, _| {
                conflicts += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(conflicts, 1);
        drop(index);
        assert_eq!(memory.used(), 0);
    }
}
