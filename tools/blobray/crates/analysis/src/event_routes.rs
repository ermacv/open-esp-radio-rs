//! Borrowed saved-fact preparation for conditional event-route checks.
use crate::navigation::Facts;
use blobray_domain::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueProducer {
    Entry {
        register: u8,
    },
    Load {
        offset: u64,
        width: u8,
        signed: bool,
    },
    CallResult {
        offset: u64,
        register: u8,
    },
}
#[derive(Clone, Debug)]
pub struct RouteValue {
    pub source: FunctionSource,
    pub function: FunctionSelector,
    pub raw: AbstractValue,
    pub locations: Vec<SliceLocation>,
    pub issue: Option<AccessIssue>,
    pub producer: Option<ValueProducer>,
    pub addend: u32,
}
impl RouteValue {
    /// Single physical coordinate from the shared saved access-path interpretation.
    pub fn target(&self) -> Option<AbstractValue> {
        if self.issue.is_some() || self.locations.len() != 1 {
            return None;
        }
        match &self.locations[0].address {
            SliceAddress::Scoped {
                source,
                object,
                address,
            } => Some(AbstractValue::ScopedAddress {
                source: source.clone(),
                object: object.clone(),
                address: *address,
            }),
            SliceAddress::Path { path } if path.path.is_empty() => match &path.root {
                AccessRoot::Section { section, offset } => i64::try_from(*offset)
                    .ok()
                    .and_then(|at| at.checked_add(i64::from(path.offset)))
                    .map(|offset| AbstractValue::Section {
                        section: *section,
                        offset,
                    }),
                AccessRoot::Symbol { symbol, addend } => addend
                    .checked_add(i64::from(path.offset))
                    .map(|addend| AbstractValue::Symbol {
                        symbol: symbol.clone(),
                        addend,
                    }),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.source.allocated_bytes()
            + self.function.object().artifact.allocated_bytes()
            + self.raw.allocated_bytes()
            + (self.locations.capacity() * std::mem::size_of::<SliceLocation>()) as u64
            + self
                .locations
                .iter()
                .map(crate::memory_slice::locations::bytes)
                .sum::<u64>()
    }
    pub fn constant(&self) -> Option<u32> {
        match self.raw {
            AbstractValue::Constant { value } => Some(value),
            _ => None,
        }
    }
}
pub struct Prepared<'a, 'f, 'm> {
    records: &'a [FunctionRecord],
    recipe: &'a FunctionRecipe,
    coverage: FunctionCoverage,
    facts: &'f Facts<'a, 'm>,
    expressions: AdmittedVec<'m, (u64, bool)>,
    instructions: AdmittedVec<'m, u64>,
    edges: AdmittedVec<'m, crate::flow::Arc>,
    memory: &'m WorkingMemory,
}
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
pub fn offset(record: &FunctionRecord) -> Result<u64> {
    match record {
        FunctionRecord::Instruction { offset, .. }
        | FunctionRecord::Transfer { offset, .. }
        | FunctionRecord::Condition { offset, .. }
        | FunctionRecord::MemoryAccess { offset, .. }
        | FunctionRecord::CallInputs { offset, .. } => Ok(*offset),
        _ => Err(invalid(
            "event site must identify a local instruction, call, condition or memory access",
        )),
    }
}
impl<'a, 'f, 'm> Prepared<'a, 'f, 'm> {
    pub fn new(
        records: &'a [FunctionRecord],
        recipe: &'a FunctionRecipe,
        coverage: FunctionCoverage,
        facts: &'f Facts<'a, 'm>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut expressions = AdmittedVec::new(memory);
        let mut instructions = AdmittedVec::new(memory);
        for record in records {
            c.checkpoint(1)?;
            match record {
                FunctionRecord::Expression {
                    id, offset, origin, ..
                } => {
                    if *id as usize != expressions.len() {
                        return Err(invalid("noncanonical saved expression identity"));
                    }
                    expressions.push((*offset, origin.is_none()), c.position())?;
                }
                FunctionRecord::Instruction { offset, .. } => {
                    instructions.push(*offset, c.position())?
                }
                _ => (),
            }
        }
        c.checkpoint(instructions.len() as u64 * (instructions.len().max(1).ilog2() as u64 + 1))?;
        instructions.sort_unstable();
        let mut edges = AdmittedVec::new(memory);
        for record in records {
            c.checkpoint(1)?;
            if let FunctionRecord::Edge {
                from,
                target: Some(to),
                external: false,
                relation,
            } = record
                && *relation != EdgeKind::Call
            {
                c.checkpoint(2 * (instructions.len().max(1).ilog2() as u64 + 1))?;
                if let (Ok(from), Ok(to)) = (
                    instructions.binary_search(from),
                    instructions.binary_search(to),
                ) {
                    edges.push(
                        crate::flow::Arc {
                            from,
                            to,
                            record: 0,
                        },
                        c.position(),
                    )?;
                }
            }
        }
        c.checkpoint(edges.len() as u64 * (edges.len().max(1).ilog2() as u64 + 1))?;
        edges.sort_unstable_by_key(|e| (e.from, e.to));
        Ok(Self {
            records,
            recipe,
            coverage,
            facts,
            expressions,
            instructions,
            edges,
            memory,
        })
    }
    pub fn record(&self, ordinal: u64) -> Result<&FunctionRecord> {
        usize::try_from(ordinal)
            .ok()
            .and_then(|i| self.records.get(i))
            .ok_or_else(|| invalid("event evidence record is absent"))
    }
    pub fn last_writes(
        &self,
        request: &MemorySliceQuery,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(&MemorySliceRecord, &mut dyn RunControl) -> Result<()>,
    ) -> Result<MemorySliceSummary> {
        crate::memory_slice::inspect(
            self.recipe,
            self.coverage,
            self.records,
            request,
            self.memory,
            c,
            emit,
        )
    }
    pub fn argument(&self, call: u64, word: u8, c: &mut dyn RunControl) -> Result<RouteValue> {
        let at = offset(self.record(call)?)?;
        let value = self
            .facts
            .call_argument(at, word, c)?
            .unwrap_or(&AbstractValue::Unknown);
        self.value(value, c)
    }
    pub fn value(&self, value: &AbstractValue, c: &mut dyn RunControl) -> Result<RouteValue> {
        let (locations, issue) = crate::memory_slice::locations::identify(
            value,
            4,
            self.recipe,
            self.facts,
            &[],
            Some(CallAbi::RiscvInteger),
            c,
        )?;
        let mut producer = None;
        let mut addend = 0u32;
        let mut current = value;
        for _ in 0..32 {
            c.checkpoint(1)?;
            let AbstractValue::Expression { id } = current else {
                break;
            };
            let Some((site, true)) = self.expressions.get(*id as usize) else {
                break;
            };
            match self.facts.expression(*id)? {
                Expression::EntryRegister { register } => {
                    producer = Some(ValueProducer::Entry {
                        register: *register,
                    });
                    break;
                }
                Expression::Load { width, signed, .. } => {
                    producer = Some(ValueProducer::Load {
                        offset: *site,
                        width: *width,
                        signed: *signed,
                    });
                    break;
                }
                Expression::CallResult { callsite, register } => {
                    producer = Some(ValueProducer::CallResult {
                        offset: *callsite,
                        register: *register,
                    });
                    break;
                }
                Expression::Integer {
                    op: IntegerOp::Add,
                    left,
                    right: AbstractValue::Constant { value },
                } => {
                    addend = addend.wrapping_add(*value);
                    current = left;
                }
                Expression::Integer {
                    op: IntegerOp::Add,
                    left: AbstractValue::Constant { value },
                    right,
                } => {
                    addend = addend.wrapping_add(*value);
                    current = right;
                }
                Expression::Integer {
                    op: IntegerOp::Sub,
                    left,
                    right: AbstractValue::Constant { value },
                } => {
                    addend = addend.wrapping_sub(*value);
                    current = left;
                }
                _ => break,
            }
        }
        Ok(RouteValue {
            source: self.recipe.source.clone(),
            function: self.recipe.selector.clone(),
            raw: value.clone(),
            locations,
            issue,
            producer,
            addend,
        })
    }
    /// Structural suffix only; a partial CFG cannot establish absence or ordering.
    pub fn path(&self, from: u64, to: u64, c: &mut dyn RunControl) -> Result<RouteCheckStatus> {
        self.path_without(from, to, None, c)
    }
    fn path_without(
        &self,
        from: u64,
        to: u64,
        excluded: Option<u64>,
        c: &mut dyn RunControl,
    ) -> Result<RouteCheckStatus> {
        c.checkpoint(2 * (self.instructions.len().max(1).ilog2() as u64 + 1))?;
        let (Ok(from), Ok(to)) = (
            self.instructions.binary_search(&from),
            self.instructions.binary_search(&to),
        ) else {
            return Ok(RouteCheckStatus::Unresolved);
        };
        let mut filtered = AdmittedVec::new(self.memory);
        let edges = if let Some(excluded) = excluded {
            if self.instructions[from] == excluded || self.instructions[to] == excluded {
                return Ok(RouteCheckStatus::Unresolved);
            }
            for edge in self.edges.iter() {
                c.checkpoint(1)?;
                if self.instructions[edge.from] != excluded
                    && self.instructions[edge.to] != excluded
                {
                    filtered.push(*edge, c.position())?;
                }
            }
            &*filtered
        } else {
            &*self.edges
        };
        let visited = crate::flow::reachable(
            self.instructions.len(),
            edges,
            from,
            u32::MAX,
            self.memory,
            c,
        )?;
        Ok(if !self.coverage.complete() {
            RouteCheckStatus::Unresolved
        } else if visited[to].is_some() {
            RouteCheckStatus::Established
        } else {
            RouteCheckStatus::Mismatch
        })
    }
    pub fn case(
        &self,
        case: &RouteCase,
        producer: ValueProducer,
        selector: u32,
        c: &mut dyn RunControl,
    ) -> Result<RouteCheckStatus> {
        let FunctionRecord::Condition {
            offset: at,
            test,
            left,
            right,
        } = self.record(case.condition.record)?
        else {
            return Err(invalid(
                "event case condition must identify a saved branch predicate",
            ));
        };
        let left = self.value(left, c)?;
        let right = self.value(right, c)?;
        let supplied = |v: &RouteValue| -> Option<u32> {
            if v.producer != Some(producer) {
                return v.constant();
            }
            let selector = match producer {
                ValueProducer::Load { width, signed, .. } if width < 4 => {
                    let shift = 32 - u32::from(width) * 8;
                    if signed {
                        ((selector << shift) as i32 >> shift) as u32
                    } else {
                        selector & (u32::MAX >> shift)
                    }
                }
                _ => selector,
            };
            Some(selector.wrapping_add(v.addend))
        };
        if left.producer != Some(producer) && right.producer != Some(producer) {
            return Ok(RouteCheckStatus::Unresolved);
        }
        let (Some(a), Some(b)) = (supplied(&left), supplied(&right)) else {
            return Ok(RouteCheckStatus::Unresolved);
        };
        let taken = match test {
            BranchTest::Eq => a == b,
            BranchTest::Ne => a != b,
            BranchTest::Lt => (a as i32) < (b as i32),
            BranchTest::Ge => (a as i32) >= (b as i32),
            BranchTest::Ltu => a < b,
            BranchTest::Geu => a >= b,
        };
        if taken != case.taken {
            return Ok(RouteCheckStatus::Mismatch);
        }
        let mut successor = None;
        for fact in self.records {
            c.checkpoint(1)?;
            if let FunctionRecord::Edge {
                from,
                target: Some(target),
                external: false,
                relation,
            } = fact
                && from == at
                && *relation
                    == if case.taken {
                        EdgeKind::Taken
                    } else {
                        EdgeKind::Fallthrough
                    }
                && successor.replace(*target).is_some()
            {
                return Ok(RouteCheckStatus::Unresolved);
            }
        }
        match successor {
            Some(start) => self.path_without(
                start,
                offset(self.record(case.handler.record)?)?,
                Some(*at),
                c,
            ),
            None => Ok(RouteCheckStatus::Unresolved),
        }
    }
}

/// Compare physical/saved pointer coordinates, preserving dynamic-root uncertainty.
pub fn same_pointer(a: &RouteValue, b: &RouteValue, offset: i32) -> RouteCheckStatus {
    if a.issue.is_some() || b.issue.is_some() || a.locations.len() != 1 || b.locations.len() != 1 {
        return RouteCheckStatus::Unresolved;
    }
    if [&a.locations[0].address, &b.locations[0].address]
        .into_iter()
        .any(|address| matches!(address,SliceAddress::Path {path} if !path.path.is_empty()))
    {
        return RouteCheckStatus::Unresolved;
    }
    if matches!(a.locations[0].address, SliceAddress::Stack { .. })
        || matches!(b.locations[0].address, SliceAddress::Stack { .. })
    {
        if a.source != b.source || a.function != b.function {
            return RouteCheckStatus::Unresolved;
        }
    } else if !matches!(
        (&a.locations[0].address, &b.locations[0].address),
        (SliceAddress::Scoped { .. }, SliceAddress::Scoped { .. })
    ) && (a.source != b.source || a.function.object() != b.function.object())
    {
        return RouteCheckStatus::Unresolved;
    }
    let mut expected = b.locations[0].address.clone();
    match &mut expected {
        SliceAddress::Stack { offset: at } => *at = at.wrapping_add(i64::from(offset)),
        SliceAddress::Scoped { address, .. } => *address = address.wrapping_add_signed(offset),
        SliceAddress::Path { path } => match &mut path.root {
            AccessRoot::Section { offset: at, .. } if path.path.is_empty() => {
                let Some(next) = at.checked_add_signed(i64::from(offset)) else {
                    return RouteCheckStatus::Unresolved;
                };
                *at = next;
            }
            AccessRoot::Symbol { addend, .. } if path.path.is_empty() => {
                *addend = addend.wrapping_add(i64::from(offset))
            }
            _ => path.offset = path.offset.wrapping_add(offset),
        },
        SliceAddress::Value { offset: at, .. } => *at = at.wrapping_add(offset),
    }
    if a.locations[0].address == expected {
        RouteCheckStatus::Established
    } else if matches!((&a.locations[0].address,&expected), (SliceAddress::Scoped {source:s,object:o,..},SliceAddress::Scoped {source:t,object:p,..}) if s==t && o==p)
        || matches!(
            (&a.locations[0].address, &expected),
            (SliceAddress::Stack { .. }, SliceAddress::Stack { .. })
        )
    {
        RouteCheckStatus::Mismatch
    } else {
        RouteCheckStatus::Unresolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(entry: u64, offset: i64) -> RouteValue {
        RouteValue {
            source: FunctionSource::Input { input: 0 },
            function: FunctionSelector::Range {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"fixture"),
                    location: ObjectLocation::Standalone,
                },
                section: 1,
                extent: CodeRange {
                    start: entry,
                    length: 4,
                },
            },
            raw: AbstractValue::EntryStack { offset },
            locations: vec![SliceLocation {
                address: SliceAddress::Stack { offset },
                width: 4,
            }],
            issue: None,
            producer: None,
            addend: 0,
        }
    }
    #[test]
    fn offsets_do_not_unify_different_frames_or_dynamic_pointer_loads() {
        let a = value(0, 4);
        assert_eq!(
            same_pointer(&a, &value(0, 0), 4),
            RouteCheckStatus::Established
        );
        assert_eq!(
            same_pointer(&a, &value(4, 0), 4),
            RouteCheckStatus::Unresolved
        );
        assert_eq!(
            same_pointer(&a, &value(0, 0), 0),
            RouteCheckStatus::Mismatch
        );
        let mut dynamic = a.clone();
        dynamic.locations[0].address = SliceAddress::Path {
            path: AccessPath {
                root: AccessRoot::Section {
                    section: 1,
                    offset: 0,
                },
                path: vec![AccessStep::LoadPointer { offset: 0 }],
                offset: 0,
            },
        };
        assert_eq!(
            same_pointer(&dynamic, &dynamic, 0),
            RouteCheckStatus::Unresolved
        );
        let mut other = dynamic.clone();
        other.locations[0].address = SliceAddress::Path {
            path: AccessPath {
                root: AccessRoot::Section {
                    section: 1,
                    offset: 0,
                },
                path: vec![],
                offset: 0,
            },
        };
        let mut first = other.clone();
        first.source = FunctionSource::Input { input: 1 };
        assert_eq!(
            same_pointer(&first, &other, 0),
            RouteCheckStatus::Unresolved
        );
    }
}
