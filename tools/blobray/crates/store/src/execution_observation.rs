//! Canonical final-memory stream validation; no live-memory access or execution.
use super::*;
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum EvidencePart {
    Events,
    Memory,
    Environment,
}
pub(super) struct MemoryState {
    selection: usize,
    offset: u32,
    compared: [bool; MAX_MEMORY_SELECTIONS],
    pub known: bool,
}
impl MemoryState {
    pub fn new() -> Self {
        Self {
            selection: 0,
            offset: 0,
            compared: [false; MAX_MEMORY_SELECTIONS],
            known: true,
        }
    }
    pub fn begin(&mut self, relation: Option<&ComparisonRelation>, side: bool) {
        *self = Self::new();
        if let Some(r) = relation {
            for p in &r.memory {
                self.compared[if side { p.replacement } else { p.vendor } as usize] = true;
            }
        }
    }
    pub fn chunk(&mut self, input: &Invocation, chunk: &FinalMemoryChunk) -> Result<()> {
        chunk.validate()?;
        let span = input
            .observe_memory
            .get(self.selection)
            .ok_or_else(|| integrity("extra final-memory selection"))?;
        let length = (span.length - self.offset).min(MEMORY_CHUNK_BYTES as u32) as u8;
        if chunk.selection as usize != self.selection
            || chunk.offset != self.offset
            || chunk.length != length
        {
            return Err(integrity("final-memory chunk order or geometry differs"));
        }
        if self.compared[self.selection] {
            self.known &= chunk.complete();
        }
        self.offset += u32::from(length);
        if self.offset == span.length {
            self.selection += 1;
            self.offset = 0;
        }
        Ok(())
    }
    pub fn finish(&self, input: &Invocation, blocked: bool) -> Result<()> {
        if self.offset != 0
            || self.selection
                != if blocked {
                    0
                } else {
                    input.observe_memory.len()
                }
        {
            return Err(integrity("missing final-memory observations"));
        }
        Ok(())
    }
}
pub(super) fn difference_valid(
    result: &CaseComparison,
    case: &ExecutionCase,
    max_events: u32,
) -> bool {
    let Some(relation) = &case.relation else {
        return false;
    };
    match (&result.verdict, &result.difference) {
        (ComparisonVerdict::Match | ComparisonVerdict::Incomplete, None) => true,
        (ComparisonVerdict::Diff, Some(ComparisonDifference::Event { index })) => {
            *index <= max_events
                && (relation.observes_calls()
                    || relation.events.mmio_read
                    || relation.events.mmio_write
                    || relation.events.fence
                    || relation.events.delay)
        }
        (
            ComparisonVerdict::Diff,
            Some(ComparisonDifference::Return {
                word,
                vendor,
                replacement,
            }),
        ) => {
            vendor != replacement
                && match word {
                    0 => relation.returns.low,
                    1 => relation.returns.high,
                    _ => false,
                }
        }
        (
            ComparisonVerdict::Diff,
            Some(ComparisonDifference::Memory {
                pair,
                offset,
                vendor,
                replacement,
            }),
        ) => {
            vendor != replacement
                && relation
                    .memory
                    .get(*pair as usize)
                    .is_some_and(|p| *offset < case.vendor.observe_memory[p.vendor as usize].length)
        }
        (
            ComparisonVerdict::Diff,
            Some(ComparisonDifference::CallTarget {
                index,
                vendor,
                replacement,
            }),
        ) => relation.observes_calls() && *index < max_events && vendor != replacement,
        (
            ComparisonVerdict::Diff,
            Some(ComparisonDifference::CallArgument {
                index,
                word,
                vendor,
                replacement,
            }),
        ) => {
            relation.observes_calls()
                && *index < max_events
                && usize::from(*word) < MAX_EXECUTION_ARGUMENT_WORDS
                && vendor != replacement
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_reordered_and_forged_memory_chunks_are_rejected() {
        let input = Invocation {
            observe_calls: None,
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
            observe_memory: vec![MemorySelection {
                name: "bytes".into(),
                address: 0x3000,
                length: 17,
            }],
        };
        let relation = ComparisonRelation {
            calls: false,
            reviewed_calls: None,
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                mmio_read: false,
                mmio_write: false,
                fence: false,
                delay: false,
            },
            memory: vec![MemoryPair {
                vendor: 0,
                replacement: 0,
            }],
        };
        let first = FinalMemoryChunk {
            selection: 0,
            offset: 0,
            length: 16,
            bytes: [7; 16],
            available: u16::MAX,
            known: u16::MAX,
        };
        let mut s = MemoryState::new();
        s.begin(Some(&relation), false);
        assert!(s.finish(&input, false).is_err());
        for variant in 0..4 {
            let mut bad = first;
            match variant {
                0 => bad.offset = 16,
                1 => bad.selection = 1,
                2 => bad.available = 0,
                _ => bad.known = 0,
            };
            assert_eq!(
                s.chunk(&input, &bad).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        s.chunk(&input, &first).unwrap();
        assert!(s.finish(&input, false).is_err());
        assert!(s.chunk(&input, &first).is_err());
        let last = FinalMemoryChunk {
            selection: 0,
            offset: 16,
            length: 1,
            bytes: [0; 16],
            available: 1,
            known: 0,
        };
        s.chunk(&input, &last).unwrap();
        s.finish(&input, false).unwrap();
        assert!(!s.known);
        let mut excluded = MemoryState::new();
        excluded.chunk(&input, &first).unwrap();
        excluded.chunk(&input, &last).unwrap();
        assert!(excluded.known);
    }
}
