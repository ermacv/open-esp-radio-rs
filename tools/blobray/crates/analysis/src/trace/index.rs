//! Borrowed per-function indexes, shared by every invocation of that function.
use super::*;
pub(super) struct Instruction<'a> {
    pub record: u64,
    pub offset: u64,
    pub decoded: &'a DecodedOp,
    pub memory: Option<(
        u64,
        MemoryKind,
        u8,
        &'a AbstractValue,
        Option<&'a AbstractValue>,
    )>,
    pub condition: Option<(BranchTest, &'a AbstractValue, &'a AbstractValue)>,
    pub inputs: Option<&'a [AbstractValue]>,
    pub returns: Option<(&'a AbstractValue, &'a AbstractValue)>,
    pub gap: Option<SemanticGapReason>,
    pub fence: Option<(u8, u8, u8)>,
}
pub(super) struct ExpressionRef<'a> {
    pub offset: u64,
    pub expression: &'a Expression,
    pub composed: bool,
}
pub(super) struct Index<'a, 'm> {
    pub function: &'a TraceFunction<'a>,
    pub instructions: AdmittedVec<'m, Instruction<'a>>,
    pub expressions: AdmittedVec<'m, ExpressionRef<'a>>,
}
impl<'a, 'm> Index<'a, 'm> {
    pub fn new(
        function: &'a TraceFunction<'a>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            function,
            instructions: AdmittedVec::new(memory),
            expressions: AdmittedVec::new(memory),
        };
        // Reuse common validation of flat DAG IDs and physical facts.
        let _validated = crate::navigation::Facts::new(function.records, memory, c)?;
        for (record, r) in function.records.iter().enumerate() {
            c.checkpoint(1)?;
            match r {
                FunctionRecord::Instruction {
                    offset,
                    decoded,
                    bytes,
                } => {
                    if decoded.length == 0 || usize::from(decoded.length) != bytes.len() {
                        return Err(integrity("invalid trace instruction length"));
                    }
                    out.instructions.push(
                        Instruction {
                            record: record as u64,
                            offset: *offset,
                            decoded,
                            memory: None,
                            condition: None,
                            inputs: None,
                            returns: None,
                            gap: None,
                            fence: None,
                        },
                        c.position(),
                    )?;
                }
                FunctionRecord::Expression {
                    offset,
                    expression,
                    origin,
                    ..
                } => out.expressions.push(
                    ExpressionRef {
                        offset: *offset,
                        expression,
                        composed: origin.is_some(),
                    },
                    c.position(),
                )?,
                _ => (),
            }
        }
        c.checkpoint(
            out.instructions.len() as u64 * (out.instructions.len().max(1).ilog2() as u64 + 1),
        )?;
        out.instructions.sort_unstable_by_key(|i| i.offset);
        let extent = function.manifest.recipe.extent;
        let end = extent
            .start
            .checked_add(extent.length)
            .ok_or_else(|| integrity("trace extent overflow"))?;
        if out.instructions.len() as u64 != function.manifest.instructions {
            return Err(integrity("trace instruction count differs from manifest"));
        }
        let mut previous_end = extent.start;
        for instruction in &*out.instructions {
            c.checkpoint(1)?;
            let instruction_end = instruction
                .offset
                .checked_add(u64::from(instruction.decoded.length))
                .ok_or_else(|| integrity("trace instruction offset overflow"))?;
            if instruction.offset < previous_end || instruction_end > end {
                return Err(integrity(
                    "trace instruction overlaps or escapes its extent",
                ));
            }
            previous_end = instruction_end;
        }
        for (ordinal, r) in function.records.iter().enumerate() {
            c.checkpoint(1)?;
            let offset = match r {
                FunctionRecord::MemoryAccess { offset, .. }
                | FunctionRecord::Condition { offset, .. }
                | FunctionRecord::CallInputs { offset, .. }
                | FunctionRecord::ReturnValue { offset, .. }
                | FunctionRecord::SemanticGap { offset, .. }
                | FunctionRecord::Fence { offset, .. } => *offset,
                _ => continue,
            };
            let i = out
                .lookup(offset, c)?
                .ok_or_else(|| integrity("semantic trace fact has no instruction"))?;
            let at = &mut out.instructions[i];
            let duplicate = match r {
                FunctionRecord::MemoryAccess {
                    access,
                    width,
                    address,
                    value,
                    ..
                } => at
                    .memory
                    .replace((ordinal as u64, *access, *width, address, value.as_ref()))
                    .is_some(),
                FunctionRecord::Condition {
                    test, left, right, ..
                } => at.condition.replace((*test, left, right)).is_some(),
                FunctionRecord::CallInputs { registers, .. } => {
                    at.inputs.replace(registers).is_some()
                }
                FunctionRecord::ReturnValue { low, high, .. } => {
                    at.returns.replace((low, high)).is_some()
                }
                // A widening and an unsupported instruction can both report a gap.
                FunctionRecord::SemanticGap { reason, .. } => {
                    if at.gap.is_none() || *reason != SemanticGapReason::OpaqueCall {
                        at.gap = Some(*reason);
                    }
                    false
                }
                FunctionRecord::Fence {
                    fm,
                    predecessor,
                    successor,
                    ..
                } => at.fence.replace((*fm, *predecessor, *successor)).is_some(),
                _ => unreachable!(),
            };
            if duplicate {
                return Err(integrity("duplicate static trace fact"));
            }
        }
        Ok(out)
    }
    pub fn lookup(&self, offset: u64, c: &mut dyn RunControl) -> Result<Option<usize>> {
        c.checkpoint(self.instructions.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .instructions
            .binary_search_by_key(&offset, |i| i.offset)
            .ok())
    }
}
