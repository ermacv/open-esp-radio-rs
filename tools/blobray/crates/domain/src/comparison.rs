//! Explicit selected observation relations; no inferred ABI or layout equivalence.
use crate::*;
pub const MAX_MEMORY_SELECTIONS: usize = 128;
pub const MAX_OBSERVED_MEMORY_BYTES: u32 = 1024 * 1024;
pub const MEMORY_CHUNK_BYTES: usize = 16;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySelection {
    pub name: String,
    pub address: u32,
    pub length: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnWords {
    pub low: bool,
    pub high: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventChannels {
    pub timeline: TimelineCapture,
    pub mmio_read: bool,
    pub mmio_write: bool,
    pub fence: bool,
    pub delay: bool,
}
impl EventChannels {
    pub fn any(self) -> bool {
        self.mmio_read || self.mmio_write || self.fence || self.delay || self.timeline.any()
    }
    pub fn selects(self, event: &ExecutionEvent) -> bool {
        if let Some(transaction) = event.normal_memory() {
            return self.timeline.selects(&transaction);
        }
        match event {
            ExecutionEvent::Branch { .. } => self.timeline.branches,
            ExecutionEvent::Read { .. } => self.mmio_read,
            ExecutionEvent::Write { .. } => self.mmio_write,
            ExecutionEvent::Fence { .. } => self.fence,
            ExecutionEvent::DelayMicros { .. } => self.delay,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPair {
    pub vendor: u16,
    pub replacement: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonRelation {
    pub effects: Option<EffectReview>,
    pub projection: Option<ProjectionReview>,
    pub returns: ReturnWords,
    pub events: EventChannels,
    pub memory: Vec<MemoryPair>,
    pub calls: bool,
    pub reviewed_calls: Option<ReviewedCalls>,
}
impl ComparisonRelation {
    pub fn observes_calls(&self) -> bool {
        self.calls || self.reviewed_calls.is_some()
    }
    pub fn validate(&self, vendor: &Invocation, replacement: &Invocation) -> Result<()> {
        let bad = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid selected comparison relation",
            )
        };
        if self.memory.len() > MAX_MEMORY_SELECTIONS
            || (!self.returns.low
                && !self.returns.high
                && !self.events.mmio_read
                && !self.events.mmio_write
                && !self.events.fence
                && !self.events.delay
                && !self.events.timeline.any()
                && self.memory.is_empty()
                && !self.observes_calls()
                && self.projection.is_none())
        {
            return Err(bad());
        }
        if self.effects.is_some()
            && !(self.events.mmio_read
                && self.events.mmio_write
                && self.events.fence
                && self.events.delay)
        {
            return Err(bad());
        }
        if !vendor.observe_timeline.contains(self.events.timeline)
            || !replacement.observe_timeline.contains(self.events.timeline)
        {
            return Err(bad());
        }
        if (self.returns.low || self.returns.high)
            && (!matches!(vendor.goal, ExecutionGoal::Return)
                || !matches!(replacement.goal, ExecutionGoal::Return))
        {
            return Err(bad());
        }
        if self.calls
            && (vendor.observe_calls.is_none() || vendor.observe_calls != replacement.observe_calls)
        {
            return Err(bad());
        }
        if let Some(r) = &self.reviewed_calls {
            if self.calls
                || vendor.observe_calls.is_none()
                || replacement.observe_calls.is_none()
                || r.pairs.is_empty()
                || r.pairs.len() > MAX_CALL_PAIRS
                || r.pairs
                    .iter()
                    .enumerate()
                    .any(|(i, p)| r.pairs[..i].contains(p))
            {
                return Err(bad());
            }
            if r.unlisted == UnlistedCalls::Exact
                && vendor.observe_calls != replacement.observe_calls
            {
                return Err(bad());
            }
        }
        for (i, pair) in self.memory.iter().enumerate() {
            let (Some(a), Some(b)) = (
                vendor.observe_memory.get(pair.vendor as usize),
                replacement.observe_memory.get(pair.replacement as usize),
            ) else {
                return Err(bad());
            };
            if a.length != b.length
                || self.memory[..i]
                    .iter()
                    .any(|p| p.vendor == pair.vendor || p.replacement == pair.replacement)
            {
                return Err(bad());
            }
        }
        Ok(())
    }
}
impl Invocation {
    pub fn validate_memory_selection(&self) -> Result<()> {
        let bad = || Error::new(ErrorCode::InvalidRequest, "invalid final-memory selection");
        if self.observe_memory.len() > MAX_MEMORY_SELECTIONS {
            return Err(bad());
        }
        let mut total = 0u64;
        for (i, s) in self.observe_memory.iter().enumerate() {
            total += u64::from(s.length);
            if s.name.trim().is_empty()
                || s.name.len() > 128
                || !s.name.is_ascii()
                || s.length == 0
                || u64::from(s.address) + u64::from(s.length) >= u64::from(u32::MAX - 1)
                || total > u64::from(MAX_OBSERVED_MEMORY_BYTES)
                || self.observe_memory[..i].iter().any(|p| {
                    p.name == s.name
                        || (u64::from(s.address) < u64::from(p.address) + u64::from(p.length)
                            && u64::from(p.address) < u64::from(s.address) + u64::from(s.length))
                })
            {
                return Err(bad());
            }
        }
        Ok(())
    }
    pub fn memory_chunks(&self) -> usize {
        self.observe_memory
            .iter()
            .map(|s| s.length.div_ceil(MEMORY_CHUNK_BYTES as u32) as usize)
            .sum()
    }
}
/// Every selected byte is represented, even unchanged, unknown or unavailable bytes.
/// Bits outside `length` and bytes whose known bit is clear are canonical zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalMemoryChunk {
    pub selection: u16,
    pub offset: u32,
    pub length: u8,
    pub bytes: [u8; MEMORY_CHUNK_BYTES],
    pub available: u16,
    pub known: u16,
}
impl FinalMemoryChunk {
    pub fn mask(&self) -> Option<u16> {
        match self.length {
            1..=15 => Some((1u16 << self.length) - 1),
            16 => Some(u16::MAX),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error::new(ErrorCode::Integrity, "invalid final-memory chunk");
        let mask = self.mask().ok_or_else(invalid)?;
        if self.available & !mask != 0
            || self.known & !self.available != 0
            || self
                .bytes
                .iter()
                .enumerate()
                .any(|(i, b)| (self.known & (1 << i)) == 0 && *b != 0)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.mask() == Some(self.known)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ComparisonDifference {
    EffectViolation {
        violation: EffectViolation,
    },
    ProjectedMemory {
        field: u16,
        offset: u32,
        vendor: u8,
        replacement: u8,
    },
    CallTarget {
        index: u32,
        vendor: u32,
        replacement: u32,
    },
    CallArgument {
        index: u32,
        word: u16,
        vendor: u32,
        replacement: u32,
    },
    Event {
        index: u32,
    },
    Return {
        word: u8,
        vendor: u32,
        replacement: u32,
    },
    Memory {
        pair: u16,
        offset: u32,
        vendor: u8,
        replacement: u8,
    },
}
