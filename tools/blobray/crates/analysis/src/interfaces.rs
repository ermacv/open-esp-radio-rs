//! Bounded discovery from saved local facts. No source reads, publication or model execution.
use blobray_domain::*;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum KeyStep {
    Load(i128),
    Index(u8, u32),
    Offset(i128),
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PathKey {
    root: InterfaceRoot,
    steps: Vec<KeyStep>,
}
impl PathKey {
    pub fn new(root: &InterfaceRoot, path: &[InterfaceStep], slot: u32) -> Self {
        let mut root = root.clone();
        let mut offset = match &mut root {
            InterfaceRoot::Symbol { addend, .. } => {
                let n = *addend;
                *addend = 0;
                i128::from(n)
            }
            InterfaceRoot::Address { address } => {
                let n = *address;
                *address = 0;
                i128::from(n)
            }
            InterfaceRoot::Section { offset, .. } => {
                let n = *offset;
                *offset = 0;
                i128::from(n)
            }
            _ => 0,
        };
        let mut steps = Vec::with_capacity(2 * path.len() + 1);
        for step in path {
            match step {
                InterfaceStep::Offset { bytes } => offset += i128::from(*bytes),
                InterfaceStep::LoadPointer { offset: bytes } => {
                    steps.push(KeyStep::Load(offset + i128::from(*bytes)));
                    offset = 0;
                }
                InterfaceStep::Index { argument, stride } => {
                    if offset != 0 {
                        steps.push(KeyStep::Offset(offset));
                        offset = 0;
                    }
                    steps.push(KeyStep::Index(*argument, *stride));
                }
            }
        }
        steps.push(KeyStep::Offset(offset + i128::from(slot)));
        Self { root, steps }
    }
    pub fn allocated_bytes(&self) -> u64 {
        (self.steps.capacity() * std::mem::size_of::<KeyStep>()) as u64
            + match &self.root {
                InterfaceRoot::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
                InterfaceRoot::FunctionArgument { function, .. } => {
                    function.object().artifact.allocated_bytes()
                }
                _ => 0,
            }
    }
}
fn integrity(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn argument(register: u8, abi: Option<CallAbi>) -> std::result::Result<u8, InterfaceIssue> {
    if abi.is_none() {
        return Err(InterfaceIssue::AbiRequired);
    }
    if (10..=17).contains(&register) {
        Ok(register - 10)
    } else {
        Err(InterfaceIssue::UnsupportedArgument)
    }
}
fn leaf(
    v: &AbstractValue,
    recipe: &FunctionRecipe,
) -> std::result::Result<InterfaceRoot, InterfaceIssue> {
    Ok(match v {
        AbstractValue::Constant { value } => InterfaceRoot::Address { address: *value },
        AbstractValue::ImageAddress { address } => InterfaceRoot::Address { address: *address },
        AbstractValue::Section { section, offset } => InterfaceRoot::Section {
            section: *section,
            offset: u64::try_from(*offset).map_err(|_| InterfaceIssue::OffsetOutOfRange)?,
        },
        AbstractValue::Symbol { symbol, addend } if symbol.object == *recipe.selector.object() => {
            InterfaceRoot::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            }
        }
        AbstractValue::Symbol { .. } | AbstractValue::ScopedAddress { .. } => {
            return Err(InterfaceIssue::ForeignOccurrence);
        }
        _ => return Err(InterfaceIssue::UnknownValue),
    })
}
fn finish(
    root: InterfaceRoot,
    reverse: &[InterfaceStep],
) -> std::result::Result<InterfaceAccessPath, InterfaceIssue> {
    let mut path = Vec::with_capacity(reverse.len());
    let mut offset = 0i64;
    for step in reverse.iter().rev() {
        match step {
            InterfaceStep::Offset { bytes } => offset += i64::from(*bytes),
            InterfaceStep::LoadPointer { offset: bytes } => {
                let bytes = i32::try_from(offset + i64::from(*bytes))
                    .map_err(|_| InterfaceIssue::OffsetOutOfRange)?;
                path.push(InterfaceStep::LoadPointer { offset: bytes });
                offset = 0;
            }
            step @ InterfaceStep::Index { .. } => {
                if offset != 0 {
                    path.push(InterfaceStep::Offset {
                        bytes: i32::try_from(offset)
                            .map_err(|_| InterfaceIssue::OffsetOutOfRange)?,
                    });
                    offset = 0;
                }
                path.push(*step);
            }
        }
    }
    if offset != 0 {
        return Err(InterfaceIssue::NonzeroCallDisplacement);
    }
    let Some(InterfaceStep::LoadPointer { offset }) = path.pop() else {
        return Err(InterfaceIssue::NoPointerPath);
    };
    let slot = u32::try_from(offset).map_err(|_| InterfaceIssue::OffsetOutOfRange)?;
    if !slot.is_multiple_of(4) {
        return Err(InterfaceIssue::NonPointerLoad);
    }
    Ok(InterfaceAccessPath { root, path, slot })
}
fn expression<'a>(id: u32, expressions: &[&'a Expression]) -> Result<&'a Expression> {
    expressions
        .get(id as usize)
        .copied()
        .ok_or_else(|| integrity("interface expression ID is absent"))
}
fn index_operand(
    v: &AbstractValue,
    expressions: &[&Expression],
    abi: Option<CallAbi>,
) -> Result<Option<(u8, u32)>> {
    let AbstractValue::Expression { id } = v else {
        return Ok(None);
    };
    let (register, stride) = match expression(*id, expressions)? {
        Expression::EntryRegister { .. } => return Ok(None),
        Expression::Integer {
            op: IntegerOp::Mul | IntegerOp::Shl,
            left: AbstractValue::Expression { id: arg },
            right: AbstractValue::Constant { value },
            ..
        } if arg < id => {
            let Expression::EntryRegister { register } = expression(*arg, expressions)? else {
                return Ok(None);
            };
            let stride = if matches!(
                expression(*id, expressions)?,
                Expression::Integer {
                    op: IntegerOp::Shl,
                    ..
                }
            ) {
                1u32 << (*value & 31)
            } else {
                *value
            };
            (*register, stride)
        }
        _ => return Ok(None),
    };
    Ok((stride != 0)
        .then(|| argument(register, abi).ok().map(|arg| (arg, stride)))
        .flatten())
}
fn paths(
    v: &AbstractValue,
    expressions: &[&Expression],
    recipe: &FunctionRecipe,
    abi: Option<CallAbi>,
    c: &mut dyn RunControl,
) -> Result<(Vec<InterfaceAccessPath>, Option<InterfaceIssue>)> {
    let mut current = v;
    let mut reverse = Vec::with_capacity(16);
    let mut previous = None;
    loop {
        c.checkpoint(1)?;
        if reverse.len() >= 16 {
            return Ok((vec![], Some(InterfaceIssue::PathLimit)));
        }
        let root = match current {
            AbstractValue::Expression { id } => {
                if previous.is_some_and(|old| *id >= old) {
                    return Err(integrity(
                        "interface path contains a forward or cyclic expression",
                    ));
                }
                previous = Some(*id);
                match expression(*id, expressions)? {
                    Expression::EntryRegister { register } => match argument(*register, abi) {
                        Ok(argument) => Ok(InterfaceRoot::FunctionArgument {
                            function: recipe.selector.clone(),
                            argument,
                        }),
                        Err(issue) => Err(issue),
                    },
                    Expression::Load {
                        address, width: 4, ..
                    } => {
                        reverse.push(InterfaceStep::LoadPointer { offset: 0 });
                        current = address;
                        continue;
                    }
                    Expression::Load { .. } => Err(InterfaceIssue::NonPointerLoad),
                    Expression::CallResult { .. } => Err(InterfaceIssue::UnmodeledCallResult),
                    Expression::Integer { op, left, right } => {
                        if *op == IntegerOp::Add {
                            if let Some((argument, stride)) =
                                index_operand(right, expressions, abi)?
                            {
                                reverse.push(InterfaceStep::Index { argument, stride });
                                current = left;
                                continue;
                            }
                            if let Some((argument, stride)) = index_operand(left, expressions, abi)?
                            {
                                reverse.push(InterfaceStep::Index { argument, stride });
                                current = right;
                                continue;
                            }
                        }
                        let constant = match (op, left, right) {
                            (IntegerOp::Add, v, AbstractValue::Constant { value }) => {
                                Some((v, i64::from(*value as i32)))
                            }
                            (IntegerOp::Add, AbstractValue::Constant { value }, v) => {
                                Some((v, i64::from(*value as i32)))
                            }
                            (IntegerOp::Sub, v, AbstractValue::Constant { value }) => {
                                Some((v, -i64::from(*value as i32)))
                            }
                            _ => None,
                        };
                        if let Some((v, bytes)) = constant {
                            let Ok(bytes) = i32::try_from(bytes) else {
                                return Ok((vec![], Some(InterfaceIssue::OffsetOutOfRange)));
                            };
                            reverse.push(InterfaceStep::Offset { bytes });
                            current = v;
                            continue;
                        }
                        Err(InterfaceIssue::UnsupportedExpression)
                    }
                }
            }
            AbstractValue::Alternatives { values } => {
                let mut found = Vec::with_capacity(values.values().len());
                for v in values.values() {
                    c.checkpoint(1)?;
                    match leaf(&v.as_value(), recipe).and_then(|root| finish(root, &reverse)) {
                        Ok(path) => found.push(path),
                        Err(issue) => return Ok((vec![], Some(issue))),
                    }
                }
                return Ok((found, None));
            }
            v => leaf(v, recipe),
        };
        return Ok(match root.and_then(|root| finish(root, &reverse)) {
            Ok(path) => (vec![path], None),
            Err(issue) => (vec![], Some(issue)),
        });
    }
}
/// Emits local indirect-transfer observations; known target values do not invent load provenance.
pub fn discover(
    records: &[FunctionRecord],
    recipe: &FunctionRecipe,
    abi: Option<CallAbi>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(InterfaceObservation, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    c.phase(RunPhase::IndexResearch)?;
    let _construction = memory.reserve(65536, c.position())?;
    let mut expressions = AdmittedVec::new(memory);
    let mut instructions = AdmittedVec::new(memory);
    let mut inputs = AdmittedVec::new(memory);
    let mut transfers = AdmittedVec::new(memory);
    for (record, r) in records.iter().enumerate() {
        c.checkpoint(1)?;
        match r {
            FunctionRecord::Expression { id, expression, .. } => {
                if *id as usize != expressions.len() {
                    return Err(integrity("noncanonical interface expression records"));
                }
                let earlier = |v: &AbstractValue| !matches!(v,AbstractValue::Expression {id:reference} if reference>=id);
                let valid = match expression {
                    Expression::Integer { left, right, .. } => earlier(left) && earlier(right),
                    Expression::Load { address, .. } => earlier(address),
                    _ => true,
                };
                if !valid {
                    return Err(integrity(
                        "interface expression has a forward or cyclic dependency",
                    ));
                }
                expressions.push(expression, c.position())?;
            }
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => instructions.push((*offset, record as u64, decoded.flow), c.position())?,
            FunctionRecord::CallInputs { offset, registers } => {
                inputs.push((*offset, registers), c.position())?
            }
            FunctionRecord::Transfer { offset, target, .. } => {
                transfers.push((*offset, record as u64, target), c.position())?
            }
            _ => (),
        }
    }
    c.checkpoint(
        (instructions.len() + inputs.len() + transfers.len()) as u64
            * (records.len().max(1).ilog2() as u64 + 1),
    )?;
    instructions.sort_unstable_by_key(|x| x.0);
    inputs.sort_unstable_by_key(|x| x.0);
    transfers.sort_unstable_by_key(|x| x.0);
    if instructions.windows(2).any(|w| w[0].0 == w[1].0)
        || inputs.windows(2).any(|w| w[0].0 == w[1].0)
        || transfers.windows(2).any(|w| w[0].0 == w[1].0)
    {
        return Err(integrity(
            "duplicate interface instruction or call-input records",
        ));
    }
    let mut ordinal = 0;
    for &(offset, instruction_record, flow) in instructions.iter() {
        c.checkpoint(1)?;
        let InstructionFlow::Indirect {
            base,
            offset: displacement,
            link,
        } = flow
        else {
            continue;
        };
        c.checkpoint((inputs.len() + transfers.len()).max(1).ilog2() as u64 + 2)?;
        let input = inputs
            .binary_search_by_key(&offset, |x| x.0)
            .ok()
            .and_then(|i| inputs[i].1.get(base as usize));
        let transfer = transfers
            .binary_search_by_key(&offset, |x| x.0)
            .ok()
            .map(|i| transfers[i]);
        if !link && input.is_none() && transfer.is_none() {
            continue;
        }
        let record = transfer.map_or(instruction_record, |r| r.1);
        let target = transfer.map(|r| r.2.clone());
        let (paths, issue) = if displacement != 0 {
            (vec![], Some(InterfaceIssue::NonzeroCallDisplacement))
        } else if let Some(v) = input {
            paths(v, &expressions, recipe, abi, c)?
        } else {
            (vec![], Some(InterfaceIssue::MissingCallInputs))
        };
        emit(
            InterfaceObservation {
                ordinal,
                record,
                offset,
                paths,
                target,
                pointer: None,
                issue,
                bindings: vec![],
            },
            c,
        )?;
        ordinal += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe() -> FunctionRecipe {
        let payload = ArtifactId::of_bytes(b"interface fixture");
        FunctionRecipe {
            research: None,
            abi: RiscvAbi::Ilp32,
            schema: FUNCTION_SCHEMA,
            policy: FUNCTION_POLICY,
            decoder: "fixture".into(),
            semantics: Some("fixture".into()),
            project: payload.as_str().parse().unwrap(),
            revision: payload.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            address_space: CodeAddressSpace::Section,
            selector: FunctionSelector::Range {
                object: ObjectId {
                    artifact: payload.clone(),
                    location: ObjectLocation::Standalone,
                },
                section: 1,
                extent: CodeRange {
                    start: 0,
                    length: 16,
                },
            },
            payload,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 16,
            },
            user_extent: true,
        }
    }
    fn loaded(address: AbstractValue) -> Expression {
        Expression::Load {
            address,
            width: 4,
            signed: false,
        }
    }
    #[test]
    fn exact_argument_address_and_indexed_paths_retain_every_step() {
        let recipe = recipe();
        let expressions = [
            Expression::EntryRegister { register: 10 },
            loaded(AbstractValue::Expression { id: 0 }),
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: 1 },
                right: AbstractValue::Constant { value: 8 },
            },
            loaded(AbstractValue::Expression { id: 2 }),
        ];
        let refs: Vec<_> = expressions.iter().collect();
        let (found, issue) = paths(
            &AbstractValue::Expression { id: 3 },
            &refs,
            &recipe,
            Some(CallAbi::RiscvInteger),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(issue, None);
        assert_eq!(
            found,
            [InterfaceAccessPath {
                root: InterfaceRoot::FunctionArgument {
                    function: recipe.selector.clone(),
                    argument: 0
                },
                path: vec![InterfaceStep::LoadPointer { offset: 0 }],
                slot: 8
            }]
        );
        assert_eq!(
            paths(
                &AbstractValue::Expression { id: 3 },
                &refs,
                &recipe,
                None,
                &mut || Ok(())
            )
            .unwrap()
            .1,
            Some(InterfaceIssue::AbiRequired)
        );
        let indexed = [
            Expression::EntryRegister { register: 11 },
            Expression::Integer {
                op: IntegerOp::Shl,
                left: AbstractValue::Expression { id: 0 },
                // RV32 masks the shift count to five bits.
                right: AbstractValue::Constant { value: 34 },
            },
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Constant { value: 0x1000 },
                right: AbstractValue::Expression { id: 1 },
            },
            loaded(AbstractValue::Expression { id: 2 }),
        ];
        let refs: Vec<_> = indexed.iter().collect();
        let (found, issue) = paths(
            &AbstractValue::Expression { id: 3 },
            &refs,
            &recipe,
            Some(CallAbi::RiscvInteger),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(issue, None);
        assert_eq!(
            found,
            [InterfaceAccessPath {
                root: InterfaceRoot::Address { address: 0x1000 },
                path: vec![InterfaceStep::Index {
                    argument: 1,
                    stride: 4
                }],
                slot: 0
            }]
        );
        let alternatives = AbstractValue::Alternatives {
            values: ValueAlternatives::new(vec![
                ValueAlternative::ImageAddress { address: 0x1000 },
                ValueAlternative::ImageAddress { address: 0x2000 },
            ])
            .unwrap(),
        };
        let expression = loaded(alternatives);
        let (found, issue) = paths(
            &AbstractValue::Expression { id: 0 },
            &[&expression],
            &recipe,
            None,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(issue, None);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].root, InterfaceRoot::Address { address: 0x2000 });
    }
    fn records(expressions: Vec<Expression>, target: u32) -> Vec<FunctionRecord> {
        let mut out: Vec<_> = expressions
            .into_iter()
            .enumerate()
            .map(|(id, expression)| FunctionRecord::Expression {
                id: id as u32,
                offset: 0,
                origin: None,
                expression,
            })
            .collect();
        out.push(FunctionRecord::Instruction {
            offset: 12,
            bytes: vec![0; 4],
            decoded: DecodedOp {
                length: 4,
                text: "fixture".into(),
                flow: InstructionFlow::Indirect {
                    base: 5,
                    offset: 0,
                    link: true,
                },
            },
        });
        let mut registers = vec![AbstractValue::Unknown; 32];
        registers[5] = AbstractValue::Expression { id: target };
        out.push(FunctionRecord::CallInputs {
            offset: 12,
            registers,
        });
        out
    }
    #[test]
    fn object_calls_need_no_image_transfer_and_invalid_graphs_or_budgets_emit_no_success() {
        let recipe = recipe();
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let good = records(vec![loaded(AbstractValue::Constant { value: 0x1000 })], 0);
        let mut output = vec![];
        discover(
            &good,
            &recipe,
            None,
            &memory,
            &mut || Ok(()),
            &mut |r, _| {
                output.push(r);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].record, 1);
        assert_eq!(output[0].target, None);
        assert_eq!(output[0].paths[0].slot, 0);
        assert_eq!(memory.observation().reserved_bytes, 0);
        let bad = records(vec![loaded(AbstractValue::Expression { id: 0 })], 0);
        let mut count = 0;
        assert_eq!(
            discover(&bad, &recipe, None, &memory, &mut || Ok(()), &mut |_, _| {
                count += 1;
                Ok(())
            })
            .unwrap_err()
            .code,
            ErrorCode::Integrity
        );
        assert_eq!(count, 0);
        let mut left = 3u64;
        assert_eq!(
            discover(
                &good,
                &recipe,
                None,
                &memory,
                &mut || {
                    left = left.saturating_sub(1);
                    if left == 0 {
                        Err(Error::new(ErrorCode::Cancelled, "fixture"))
                    } else {
                        Ok(())
                    }
                },
                &mut |_, _| {
                    count += 1;
                    Ok(())
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::Cancelled
        );
        assert_eq!(count, 0);
        assert_eq!(memory.observation().reserved_bytes, 0);
        let tiny = WorkingMemory::new(16).unwrap();
        assert_eq!(
            discover(&good, &recipe, None, &tiny, &mut || Ok(()), &mut |_, _| Ok(
                ()
            ))
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimited
        );
    }
    #[test]
    fn canonical_path_keys_merge_static_offsets_without_equating_distinct_dereferences() {
        let a = PathKey::new(
            &InterfaceRoot::Address { address: 0x1000 },
            &[
                InterfaceStep::Offset { bytes: 4 },
                InterfaceStep::LoadPointer { offset: 4 },
            ],
            8,
        );
        let b = PathKey::new(
            &InterfaceRoot::Address { address: 0x1008 },
            &[InterfaceStep::LoadPointer { offset: 0 }],
            8,
        );
        assert_eq!(a, b);
        assert_ne!(
            a,
            PathKey::new(&InterfaceRoot::Address { address: 0x1008 }, &[], 8)
        );
    }
}
