//! Canonical symbolic input/read expressions, with explicit capacity and no solver claims.
use super::*;
use std::hash::{DefaultHasher, Hash, Hasher};
pub struct Canonical<'m> {
    memory: &'m WorkingMemory,
    nodes: AdmittedVec<'m, TraceExpression>,
    slots: AdmittedVec<'m, usize>,
}
fn hash(expression: &TraceExpression) -> usize {
    // Bucket placement is private; identities follow insertion order and compare full keys.
    let mut hash = DefaultHasher::new();
    expression.hash(&mut hash);
    hash.finish() as usize
}
impl<'m> Canonical<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            nodes: AdmittedVec::new(memory),
            slots: AdmittedVec::new(memory),
        }
    }
    fn grow(&mut self, c: &mut dyn RunControl) -> Result<()> {
        let size = self
            .slots
            .len()
            .checked_mul(2)
            .ok_or_else(|| integrity("trace expression index overflow"))?
            .max(64);
        let mut slots = AdmittedVec::new(self.memory);
        for _ in 0..size {
            c.checkpoint(1)?;
            slots.push(usize::MAX, c.position())?;
        }
        for (id, node) in self.nodes.iter().enumerate() {
            let mut slot = hash(node) & (size - 1);
            while slots[slot] != usize::MAX {
                c.checkpoint(1)?;
                slot = (slot + 1) & (size - 1);
            }
            slots[slot] = id;
        }
        self.slots = slots;
        Ok(())
    }
    pub(super) fn intern(
        &mut self,
        expression: TraceExpression,
        c: &mut dyn RunControl,
        emit: &mut Emitter<'_>,
    ) -> Result<TraceValue> {
        if self.slots.is_empty() {
            self.grow(c)?;
        }
        loop {
            let mut slot = hash(&expression) & (self.slots.len() - 1);
            while self.slots[slot] != usize::MAX {
                c.checkpoint(1)?;
                let id = self.slots[slot];
                if self.nodes[id] == expression {
                    return Ok(TraceValue::Expression { id: id as u32 });
                }
                slot = (slot + 1) & (self.slots.len() - 1);
            }
            c.checkpoint(1)?;
            if self.nodes.len() >= self.slots.len() / 2 {
                self.grow(c)?;
                continue;
            }
            let id = u32::try_from(self.nodes.len())
                .map_err(|_| integrity("trace expression IDs exhausted"))?;
            self.nodes.push(expression, c.position())?;
            self.slots[slot] = id as usize;
            emit(&TraceRecord::Expression { id, expression }, c)?;
            return Ok(TraceValue::Expression { id });
        }
    }
    fn binary(
        &mut self,
        op: IntegerOp,
        left: TraceValue,
        right: TraceValue,
        c: &mut dyn RunControl,
        emit: &mut Emitter<'_>,
    ) -> Result<TraceValue> {
        if let (TraceValue::Constant { value: a }, TraceValue::Constant { value: b }) =
            (left, right)
        {
            return Ok(constant(crate::values::fold_integer(op, a, b)));
        }
        if left == right {
            match op {
                IntegerOp::Sub | IntegerOp::Xor | IntegerOp::Lt | IntegerOp::Ltu => {
                    return Ok(constant(0));
                }
                IntegerOp::And | IntegerOp::Or => return Ok(left),
                _ => (),
            }
        }
        let (left, right) = if matches!(
            op,
            IntegerOp::Add | IntegerOp::And | IntegerOp::Or | IntegerOp::Xor | IntegerOp::Mul
        ) && right < left
        {
            (right, left)
        } else {
            (left, right)
        };
        self.intern(TraceExpression::Integer { op, left, right }, c, emit)
    }
}
pub(super) fn constant(value: u32) -> TraceValue {
    TraceValue::Constant { value }
}
pub(super) fn number(value: Option<TraceValue>) -> Option<u32> {
    if let Some(TraceValue::Constant { value }) = value {
        Some(value)
    } else {
        None
    }
}
pub(super) struct Eval<'m> {
    memo: AdmittedVec<'m, Option<Option<TraceValue>>>,
    pending: AdmittedVec<'m, (u32, bool)>,
    /// Original instruction index to selected observed read identity.
    pub reads: AdmittedVec<'m, Option<(u64, u8, u32)>>,
    pub returns: AdmittedVec<'m, Option<(Option<TraceValue>, Option<TraceValue>)>>,
}
impl<'m> Eval<'m> {
    pub fn new(
        index: &Index<'_, '_>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            memo: AdmittedVec::new(memory),
            pending: AdmittedVec::new(memory),
            reads: AdmittedVec::new(memory),
            returns: AdmittedVec::new(memory),
        };
        for _ in &*index.expressions {
            c.checkpoint(1)?;
            out.memo.push(None, c.position())?;
        }
        for _ in &*index.instructions {
            c.checkpoint(1)?;
            out.reads.push(None, c.position())?;
            out.returns.push(None, c.position())?;
        }
        Ok(out)
    }
    fn leaf(
        &self,
        value: &AbstractValue,
        args: &[Option<TraceValue>; 32],
    ) -> Result<Option<TraceValue>> {
        Ok(match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                Some(constant(*value))
            }
            AbstractValue::EntryStack { offset } => {
                number(args[2]).map(|n| constant(n.wrapping_add(*offset as u32)))
            }
            AbstractValue::Expression { id } => self
                .memo
                .get(*id as usize)
                .ok_or_else(|| integrity("unknown trace expression ID"))?
                .unwrap_or(None),
            _ => None,
        })
    }
    pub fn value(
        &mut self,
        value: &AbstractValue,
        index: &Index<'_, '_>,
        args: &[Option<TraceValue>; 32],
        canonical: &mut Canonical<'_>,
        c: &mut dyn RunControl,
        emit: &mut Emitter<'_>,
    ) -> Result<Option<TraceValue>> {
        if let AbstractValue::Expression { id } = value {
            self.pending.push((*id, false), c.position())?;
            while let Some((id, ready)) = self.pending.pop() {
                c.checkpoint(1)?;
                if self
                    .memo
                    .get(id as usize)
                    .ok_or_else(|| integrity("unknown trace expression ID"))?
                    .is_some()
                {
                    continue;
                }
                let node = &index.expressions[id as usize];
                if node.composed {
                    self.memo[id as usize] = Some(None);
                    continue;
                }
                if !ready {
                    self.pending.push((id, true), c.position())?;
                    let mut push = |v: &AbstractValue| -> Result<()> {
                        if let AbstractValue::Expression { id: child } = v {
                            if *child >= id {
                                return Err(integrity("noncausal trace expression"));
                            }
                            self.pending.push((*child, false), c.position())?;
                        }
                        Ok(())
                    };
                    match node.expression {
                        Expression::Integer { left, right, .. } => {
                            push(left)?;
                            push(right)?;
                        }
                        Expression::Load { address, .. } => push(address)?,
                        _ => (),
                    }
                    continue;
                }
                let result = match node.expression {
                    Expression::EntryRegister { register } => *args
                        .get(*register as usize)
                        .ok_or_else(|| integrity("invalid entry register"))?,
                    Expression::Integer { op, left, right } => {
                        match (self.leaf(left, args)?, self.leaf(right, args)?) {
                            (Some(a), Some(b)) => Some(canonical.binary(*op, a, b, c, emit)?),
                            _ => None,
                        }
                    }
                    Expression::Load {
                        address,
                        width,
                        signed,
                    } => {
                        if let Some(i) = index.lookup(node.offset, c)?
                            && let Some((event, actual_width, actual_address)) = self.reads[i]
                            && *width == actual_width
                            && number(self.leaf(address, args)?) == Some(actual_address)
                        {
                            Some(canonical.intern(
                                TraceExpression::Read {
                                    event,
                                    width: *width,
                                    signed: *signed,
                                },
                                c,
                                emit,
                            )?)
                        } else {
                            None
                        }
                    }
                    Expression::CallResult { callsite, register } => {
                        if let Some(i) = index.lookup(*callsite, c)?
                            && let Some((low, high)) = self.returns[i]
                        {
                            match register {
                                10 => low,
                                11 => high,
                                _ => None,
                            }
                        } else {
                            None
                        }
                    }
                };
                self.memo[id as usize] = Some(result);
            }
        }
        self.leaf(value, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_growth_deduplicates_and_releases_all_admitted_capacity() {
        let memory = WorkingMemory::new(128 * 1024).unwrap();
        let mut canonical = Canonical::new(&memory);
        let mut emitted = 0;
        let mut sink = |_: &TraceRecord, _: &mut dyn RunControl| {
            emitted += 1;
            Ok(())
        };
        for repeat in 0..3 {
            for event in 0..500 {
                assert_eq!(
                    canonical
                        .intern(
                            TraceExpression::Read {
                                event,
                                width: 4,
                                signed: false
                            },
                            &mut || Ok(()),
                            &mut sink
                        )
                        .unwrap(),
                    TraceValue::Expression { id: event as u32 }
                );
            }
            assert_eq!(canonical.nodes.len(), 500, "repeat {repeat}");
        }
        assert_eq!(emitted, 500);
        drop(canonical);
        assert_eq!(memory.used(), 0);
        let small = WorkingMemory::new(1).unwrap();
        let mut canonical = Canonical::new(&small);
        assert_eq!(
            canonical
                .intern(
                    TraceExpression::EntryRegister { register: 10 },
                    &mut || Ok(()),
                    &mut |_, _| Ok(())
                )
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        drop(canonical);
        assert_eq!(small.used(), 0);
    }
}
