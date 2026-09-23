//! Capacity ownership for retained function records. Shared targets are charged
//! per reference conservatively; this measures admitted capacity, not RSS.
use crate::*;
use std::mem::size_of;

impl AbstractValue {
    pub fn allocated_bytes(&self) -> u64 {
        match self {
            Self::ScopedAddress { source, object, .. } => {
                source.allocated_bytes() + object.artifact.allocated_bytes()
            }
            Self::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
            _ => 0,
        }
    }
}
impl FunctionSource {
    pub fn allocated_bytes(&self) -> u64 {
        match self {
            Self::Image { image } => image.allocated_bytes(),
            Self::Input { .. } => 0,
        }
    }
}
impl Expression {
    fn allocated_bytes(&self) -> u64 {
        match self {
            Self::Integer { left, right, .. } => left.allocated_bytes() + right.allocated_bytes(),
            Self::Load { address, .. } => address.allocated_bytes(),
            _ => 0,
        }
    }
}
impl FunctionRecipe {
    pub fn allocated_bytes(&self) -> u64 {
        self.decoder.capacity() as u64
            + self.semantics.as_ref().map_or(0, |s| s.capacity() as u64)
            + self.project.allocated_bytes()
            + self.revision.allocated_bytes()
            + self.source.allocated_bytes()
            + self.symbol.object.artifact.allocated_bytes()
            + self.payload.allocated_bytes()
            + self.research.as_ref().map_or(0, |r| {
                r.publication.allocated_bytes()
                    + r.knowledge
                        .as_ref()
                        .map_or(0, KnowledgeRevisionId::allocated_bytes)
                    + (r.companions.capacity() * size_of::<PublicationId>()) as u64
                    + r.companions
                        .iter()
                        .map(PublicationId::allocated_bytes)
                        .sum::<u64>()
            })
    }
}
impl ReferenceTarget {
    fn retained_bytes(&self) -> u64 {
        (size_of::<Self>() + 2 * size_of::<usize>() + self.name.capacity()) as u64
            + self.symbol.object.artifact.allocated_bytes()
    }
}
impl FunctionRecord {
    /// Variable allocation capacities, including boxes and conservatively charged Arcs.
    /// The inline enum is accounted by the owning container.
    pub fn allocated_bytes(&self) -> u64 {
        let value =
            |v: &Option<AbstractValue>| v.as_ref().map_or(0, AbstractValue::allocated_bytes);
        match self {
            Self::MmioRange {
                assertion, region, ..
            } => assertion.allocated_bytes() + region.name.capacity() as u64,
            Self::Condition { left, right, .. }
            | Self::ReturnValue {
                low: left,
                high: right,
                ..
            } => left.allocated_bytes() + right.allocated_bytes(),
            Self::Expression {
                origin, expression, ..
            } => {
                origin
                    .as_ref()
                    .map_or(0, FunctionAnalysisId::allocated_bytes)
                    + expression.allocated_bytes()
            }
            Self::CallInputs { registers, .. } => {
                (registers.capacity() * size_of::<AbstractValue>()) as u64
                    + registers
                        .iter()
                        .map(AbstractValue::allocated_bytes)
                        .sum::<u64>()
            }
            Self::Mmio {
                assertion,
                register,
                ..
            } => {
                assertion.allocated_bytes()
                    + register.name.capacity() as u64
                    + (register.fields.capacity() * size_of::<MmioField>()) as u64
                    + register
                        .fields
                        .iter()
                        .map(|f| f.name.capacity() as u64)
                        .sum::<u64>()
            }
            Self::CalleeEffect {
                analysis,
                address,
                value: v,
                ..
            } => analysis.allocated_bytes() + address.allocated_bytes() + value(v),
            Self::CallResolution {
                analysis, reason, ..
            } => {
                analysis
                    .as_ref()
                    .map_or(0, FunctionAnalysisId::allocated_bytes)
                    + reason.as_ref().map_or(0, |s| s.capacity() as u64)
            }
            Self::Transfer { target, .. } | Self::Value { value: target, .. } => {
                target.allocated_bytes()
            }
            Self::MemoryAccess {
                address, value: v, ..
            } => address.allocated_bytes() + value(v),
            Self::Instruction { bytes, decoded, .. } => {
                (bytes.capacity() + decoded.text.capacity()) as u64
            }
            Self::Reference { raw, target, .. } => {
                size_of::<FunctionRelocation>() as u64
                    + raw.target.retained_bytes()
                    + target.retained_bytes()
            }
            Self::SemanticGap { .. }
            | Self::Block { .. }
            | Self::Edge { .. }
            | Self::Gap { .. } => 0,
        }
    }
}
/// Each incoming record must already be covered by the producer's transient
/// workspace. Admission transfers it into retained ownership before the next record.
pub struct RecordBuffer<'a> {
    records: AdmittedVec<'a, FunctionRecord>,
    payloads: AdmittedVec<'a, MemoryReservation<'a>>,
    memory: &'a WorkingMemory,
}
impl<'a> RecordBuffer<'a> {
    pub fn new(memory: &'a WorkingMemory) -> Self {
        Self {
            records: AdmittedVec::new(memory),
            payloads: AdmittedVec::new(memory),
            memory,
        }
    }
    pub fn push(&mut self, record: FunctionRecord, position: RunPosition) -> Result<()> {
        let payload = self.memory.reserve(record.allocated_bytes(), position)?;
        self.payloads.push(payload, position)?;
        self.records.push(record, position)
    }
}
impl std::ops::Deref for RecordBuffer<'_> {
    type Target = [FunctionRecord];
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_payloads_consume_capacity_and_release_on_failure() {
        let memory = WorkingMemory::new(24 * 1024).unwrap();
        let record = FunctionRecord::Instruction {
            offset: 0,
            bytes: vec![1, 0],
            decoded: DecodedOp {
                length: 2,
                text: "x".repeat(8192),
                flow: InstructionFlow::Next,
            },
        };
        let mut records = RecordBuffer::new(&memory);
        records
            .push(record.clone(), RunPosition::default())
            .unwrap();
        let after_first = memory.used();
        assert!(after_first >= 8192);
        records
            .push(record.clone(), RunPosition::default())
            .unwrap();
        assert!(memory.used() >= after_first + 8192);
        assert_eq!(
            records
                .push(record, RunPosition::default())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(records.len(), 2);
        drop(records);
        assert_eq!(memory.used(), 0);
    }
}
