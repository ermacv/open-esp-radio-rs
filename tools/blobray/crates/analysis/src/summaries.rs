//! Bounded composition of saved local facts. No source discovery or publication.
use super::*;

/// A selected, acyclic callee. Application resolves exact image-qualified identity.
pub struct Callee<'a> {
    pub offset: u64,
    pub analysis: &'a FunctionAnalysisId,
    pub source: &'a FunctionSource,
    pub object: &'a ObjectId,
    pub records: &'a [FunctionRecord],
}
fn substitute(v: &AbstractValue, map: &[AbstractValue]) -> Result<AbstractValue> {
    match v {
        AbstractValue::Expression { id } => map.get(*id as usize).cloned().ok_or_else(|| {
            Error::new(
                ErrorCode::Integrity,
                "expression references an absent or forward node",
            )
        }),
        v => Ok(v.clone()),
    }
}
fn in_frame(
    v: &AbstractValue,
    map: &[AbstractValue],
    callee: &Callee<'_>,
) -> Result<AbstractValue> {
    match v {
        AbstractValue::ImageAddress { address } => Ok(AbstractValue::ScopedAddress {
            source: callee.source.clone(),
            object: callee.object.clone(),
            address: *address,
        }),
        AbstractValue::Alternatives { values } => {
            let mut imported = Vec::with_capacity(values.values().len());
            for v in values.values() {
                // A callee-local stack possibility cannot be silently removed.
                if matches!(v, ValueAlternative::EntryStack { .. }) {
                    return Ok(AbstractValue::Unknown);
                }
                imported.push(match v {
                    ValueAlternative::ImageAddress { address } => ValueAlternative::ScopedAddress {
                        source: callee.source.clone(),
                        object: callee.object.clone(),
                        address: *address,
                    },
                    v => v.clone(),
                });
            }
            imported.sort_unstable();
            imported.dedup();
            if imported.len() == 1 {
                Ok(imported[0].as_value())
            } else {
                Ok(AbstractValue::Alternatives {
                    values: ValueAlternatives::new(imported)?,
                })
            }
        }
        // Stack storage belongs to the callee frame; do not alias it with its caller.
        AbstractValue::EntryStack { .. } => Ok(AbstractValue::Unknown),
        _ => substitute(v, map),
    }
}
fn import_expression(
    expr: &Expression,
    map: &[AbstractValue],
    callee: &Callee<'_>,
) -> Result<Expression> {
    Ok(match expr {
        Expression::Integer { op, left, right } => Expression::Integer {
            op: *op,
            left: in_frame(left, map, callee)?,
            right: in_frame(right, map, callee)?,
        },
        Expression::Load {
            address,
            width,
            signed,
        } => Expression::Load {
            address: in_frame(address, map, callee)?,
            width: *width,
            signed: *signed,
        },
        e => e.clone(),
    })
}
fn rewrite(expr: &Expression, map: &[AbstractValue]) -> Result<Expression> {
    Ok(match expr {
        Expression::Integer { op, left, right } => Expression::Integer {
            op: *op,
            left: substitute(left, map)?,
            right: substitute(right, map)?,
        },
        Expression::Load {
            address,
            width,
            signed,
        } => Expression::Load {
            address: substitute(address, map)?,
            width: *width,
            signed: *signed,
        },
        e => e.clone(),
    })
}
fn push_expression(
    out: &mut RecordBuffer<'_>,
    position: RunPosition,
    next: &mut u32,
    offset: u64,
    origin: Option<FunctionAnalysisId>,
    expression: Expression,
) -> Result<AbstractValue> {
    if let Expression::Integer { op, left, right } = &expression {
        if let (AbstractValue::Constant { value: a }, AbstractValue::Constant { value: b }) =
            (left, right)
        {
            return Ok(AbstractValue::Constant {
                value: values::fold_integer(*op, *a, *b),
            });
        }
        if matches!(
            (op, right),
            (
                IntegerOp::Add | IntegerOp::Or | IntegerOp::Xor,
                AbstractValue::Constant { value: 0 }
            ) | (IntegerOp::And, AbstractValue::Constant { value: u32::MAX })
        ) {
            return Ok(left.clone());
        }
    }
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "expression ID overflow"))?;
    out.push(
        FunctionRecord::Expression {
            id,
            offset,
            origin,
            expression,
        },
        position,
    )?;
    Ok(AbstractValue::Expression { id })
}
/// Input/output capacity is owned by the application; every loop consumes its control.
/// Callee effects are may-effects, never a claim of unconditional execution/order.
pub struct Composed<'a> {
    pub records: RecordBuffer<'a>,
}
pub fn compose<'a>(
    records: &[FunctionRecord],
    callees: &[Callee<'_>],
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Composed<'a>> {
    control.phase(RunPhase::ComposeResearch)?;
    // Admit cloned mapping/return values and image-to-callee identity growth.
    // Bounded alternatives do not turn composition into recursive object graphs.
    let largest_callee = callees.iter().map(|c| c.records.len()).max().unwrap_or(0);
    let largest_args = records
        .iter()
        .filter_map(|r| match r {
            FunctionRecord::CallInputs { registers, .. } => Some(registers.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let value_slots = records
        .len()
        .checked_add(largest_callee)
        .and_then(|n| n.checked_add(largest_args))
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "summary workspace overflow"))?;
    let mut temporary_bytes = 512u64;
    let scope_bytes = callees
        .iter()
        .map(|c| c.source.allocated_bytes() + c.object.artifact.allocated_bytes())
        .max()
        .unwrap_or(0);
    let mut value_bytes = 0u64;
    for record in records.iter().chain(callees.iter().flat_map(|c| c.records)) {
        control.checkpoint(1)?;
        let mut account = |v: &AbstractValue| {
            let growth = match v {
                AbstractValue::ImageAddress { .. } => scope_bytes,
                AbstractValue::Alternatives { values } => {
                    values
                        .values()
                        .iter()
                        .filter(|v| matches!(v, ValueAlternative::ImageAddress { .. }))
                        .count() as u64
                        * scope_bytes
                }
                _ => 0,
            };
            value_bytes = value_bytes.max(v.allocated_bytes() + growth);
        };
        match record {
            FunctionRecord::Expression { expression, .. } => match expression {
                Expression::Integer { left, right, .. } => {
                    account(left);
                    account(right);
                }
                Expression::Load { address, .. } => account(address),
                _ => (),
            },
            FunctionRecord::Condition { left, right, .. }
            | FunctionRecord::ReturnValue {
                low: left,
                high: right,
                ..
            } => {
                account(left);
                account(right);
            }
            FunctionRecord::CallInputs { registers, .. } => {
                for v in registers {
                    account(v);
                }
            }
            FunctionRecord::MemoryAccess { address, value, .. }
            | FunctionRecord::CalleeEffect { address, value, .. } => {
                account(address);
                if let Some(v) = value {
                    account(v);
                }
            }
            FunctionRecord::Value { value, .. }
            | FunctionRecord::Transfer { target: value, .. } => account(value),
            _ => (),
        }
        let values = match record {
            FunctionRecord::CallInputs { registers, .. } => registers.len() as u64,
            _ => 0,
        };
        let bytes = record
            .allocated_bytes()
            .checked_add(
                values
                    .checked_mul(128)
                    .and_then(|n| n.checked_add(512))
                    .ok_or_else(|| {
                        Error::new(ErrorCode::ResourceLimited, "record workspace overflow")
                    })?,
            )
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "record workspace overflow"))?;
        temporary_bytes = temporary_bytes.max(bytes);
    }
    temporary_bytes =
        temporary_bytes.max(32 * (std::mem::size_of::<AbstractValue>() as u64 + value_bytes));
    let workspace = value_slots
        .checked_mul(std::mem::size_of::<AbstractValue>() + value_bytes as usize)
        .and_then(|n| {
            n.checked_add(records.len().checked_mul(
                std::mem::size_of::<(u64, AbstractValue, AbstractValue)>()
                    + 2 * value_bytes as usize,
            )?)
        })
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "summary workspace overflow"))?;
    let workspace = (workspace as u64)
        .checked_add(temporary_bytes)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "summary workspace overflow"))?;
    let _workspace = memory.reserve(workspace, control.position())?;
    let mut out = RecordBuffer::new(memory);
    let mut mapping = reserve_vec(records.len())?;
    let mut next = 0;
    let mut returns: Vec<(u64, AbstractValue, AbstractValue)> = reserve_vec(records.len())?;
    // Local expressions are topological, including arguments to earlier calls.
    for record in records {
        control.checkpoint(1)?;
        if let FunctionRecord::Expression {
            id,
            offset,
            origin,
            expression,
        } = record
        {
            if *id as usize != mapping.len() {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "noncanonical expression IDs",
                ));
            }
            let value = if let Expression::CallResult { callsite, register } = expression {
                if !returns.iter().any(|r| r.0 == *callsite) {
                    let callee = callees.iter().find(|c| c.offset == *callsite);
                    let args = records.iter().find_map(|r| {
                        if let FunctionRecord::CallInputs { offset, registers } = r {
                            (*offset == *callsite).then_some(registers)
                        } else {
                            None
                        }
                    });
                    if let (Some(callee), Some(args)) = (callee, args) {
                        let args: Vec<_> = args
                            .iter()
                            .map(|v| substitute(v, &mapping))
                            .collect::<Result<_>>()?;
                        let mut imported = reserve_vec(callee.records.len())?;
                        let mut low = None;
                        let mut high = None;
                        let mut complete = true;
                        for r in callee.records {
                            control.checkpoint(1)?;
                            match r {
                                FunctionRecord::Expression {
                                    id,
                                    offset,
                                    origin,
                                    expression,
                                } => {
                                    if *id as usize != imported.len() {
                                        return Err(Error::new(
                                            ErrorCode::Integrity,
                                            "invalid callee DAG",
                                        ));
                                    }
                                    let v = match expression {
                                        Expression::EntryRegister { register } => args
                                            .get(*register as usize)
                                            .cloned()
                                            .unwrap_or(AbstractValue::Unknown),
                                        _ => push_expression(
                                            &mut out,
                                            control.position(),
                                            &mut next,
                                            *offset,
                                            Some(
                                                origin.as_ref().unwrap_or(callee.analysis).clone(),
                                            ),
                                            import_expression(expression, &imported, callee)?,
                                        )?,
                                    };
                                    imported.push(v);
                                }
                                FunctionRecord::ReturnValue {
                                    low: a, high: b, ..
                                } => {
                                    for (slot, v) in [(&mut low, a), (&mut high, b)] {
                                        let v = in_frame(v, &imported, callee)?;
                                        *slot = Some(match slot.as_ref() {
                                            None => v,
                                            Some(old) if old == &v => v,
                                            _ => AbstractValue::Unknown,
                                        });
                                    }
                                }
                                FunctionRecord::MemoryAccess {
                                    offset,
                                    access,
                                    width,
                                    address,
                                    value,
                                    ..
                                } => {
                                    out.push(
                                        FunctionRecord::CalleeEffect {
                                            callsite: *callsite,
                                            analysis: callee.analysis.clone(),
                                            offset: *offset,
                                            access: *access,
                                            width: *width,
                                            address: in_frame(address, &imported, callee)?,
                                            value: value
                                                .as_ref()
                                                .map(|v| in_frame(v, &imported, callee))
                                                .transpose()?,
                                        },
                                        control.position(),
                                    )?;
                                }
                                FunctionRecord::CalleeEffect {
                                    analysis,
                                    offset,
                                    access,
                                    width,
                                    address,
                                    value,
                                    ..
                                } => {
                                    out.push(
                                        FunctionRecord::CalleeEffect {
                                            callsite: *callsite,
                                            analysis: analysis.clone(),
                                            offset: *offset,
                                            access: *access,
                                            width: *width,
                                            address: in_frame(address, &imported, callee)?,
                                            value: value
                                                .as_ref()
                                                .map(|v| in_frame(v, &imported, callee))
                                                .transpose()?,
                                        },
                                        control.position(),
                                    )?;
                                }
                                FunctionRecord::SemanticGap { .. } | FunctionRecord::Gap { .. } => {
                                    complete = false
                                }
                                _ => (),
                            }
                        }
                        // Missing/partial return behavior cannot supply a value.
                        returns.push((
                            *callsite,
                            if complete {
                                low.unwrap_or(AbstractValue::Unknown)
                            } else {
                                AbstractValue::Unknown
                            },
                            if complete {
                                high.unwrap_or(AbstractValue::Unknown)
                            } else {
                                AbstractValue::Unknown
                            },
                        ));
                    } else {
                        returns.push((*callsite, AbstractValue::Unknown, AbstractValue::Unknown));
                    }
                }
                let r = returns.iter().find(|r| r.0 == *callsite).unwrap();
                if *register == 10 {
                    r.1.clone()
                } else {
                    r.2.clone()
                }
            } else {
                push_expression(
                    &mut out,
                    control.position(),
                    &mut next,
                    *offset,
                    origin.clone(),
                    rewrite(expression, &mapping)?,
                )?
            };
            mapping.push(value);
        }
    }
    for record in records {
        control.checkpoint(1)?;
        let r = match record {
            FunctionRecord::Expression { .. } => continue,
            FunctionRecord::Condition {
                offset,
                test,
                left,
                right,
            } => FunctionRecord::Condition {
                offset: *offset,
                test: *test,
                left: substitute(left, &mapping)?,
                right: substitute(right, &mapping)?,
            },
            FunctionRecord::Value {
                offset,
                register,
                value,
                relocation,
            } => FunctionRecord::Value {
                offset: *offset,
                register: *register,
                value: substitute(value, &mapping)?,
                relocation: *relocation,
            },
            FunctionRecord::MemoryAccess {
                offset,
                access,
                width,
                address,
                value,
                relocation,
            } => FunctionRecord::MemoryAccess {
                offset: *offset,
                access: *access,
                width: *width,
                address: substitute(address, &mapping)?,
                value: value
                    .as_ref()
                    .map(|v| substitute(v, &mapping))
                    .transpose()?,
                relocation: *relocation,
            },
            FunctionRecord::ReturnValue { offset, low, high } => FunctionRecord::ReturnValue {
                offset: *offset,
                low: substitute(low, &mapping)?,
                high: substitute(high, &mapping)?,
            },
            FunctionRecord::CallInputs { offset, registers } => FunctionRecord::CallInputs {
                offset: *offset,
                registers: registers
                    .iter()
                    .map(|v| substitute(v, &mapping))
                    .collect::<Result<_>>()?,
            },
            FunctionRecord::SemanticGap {
                offset,
                reason: SemanticGapReason::OpaqueCall,
            } if callees.iter().any(|c| {
                c.offset == *offset
                    && c.records
                        .iter()
                        .any(|r| matches!(r, FunctionRecord::ReturnValue { .. }))
                    && !c.records.iter().any(|r| {
                        matches!(
                            r,
                            FunctionRecord::Gap { .. } | FunctionRecord::SemanticGap { .. }
                        )
                    })
            }) =>
            {
                continue;
            }
            r => r.clone(),
        };
        out.push(r, control.position())?;
    }
    Ok(Composed { records: out })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn expr(id: u32, expression: Expression) -> FunctionRecord {
        FunctionRecord::Expression {
            id,
            offset: u64::from(id) * 4,
            origin: None,
            expression,
        }
    }
    #[test]
    fn callee_alternatives_keep_every_source_and_cannot_alias_caller_stack() {
        let artifact = ArtifactId::of_bytes(b"callee");
        let id: FunctionAnalysisId = artifact.as_str().parse().unwrap();
        let source = FunctionSource::Image {
            image: artifact.as_str().parse().unwrap(),
        };
        let object = ObjectId {
            artifact,
            location: ObjectLocation::Standalone,
        };
        let callee = Callee {
            offset: 4,
            analysis: &id,
            source: &source,
            object: &object,
            records: &[],
        };
        let value = AbstractValue::Alternatives {
            values: ValueAlternatives::new(vec![
                ValueAlternative::ImageAddress { address: 16 },
                ValueAlternative::ImageAddress { address: 32 },
            ])
            .unwrap(),
        };
        let imported = in_frame(&value, &[], &callee).unwrap();
        let expected = AbstractValue::Alternatives {
            values: ValueAlternatives::new(
                [16, 32]
                    .into_iter()
                    .map(|address| ValueAlternative::ScopedAddress {
                        source: source.clone(),
                        object: object.clone(),
                        address,
                    })
                    .collect(),
            )
            .unwrap(),
        };
        assert_eq!(imported, expected);
        let stack = AbstractValue::Alternatives {
            values: ValueAlternatives::new(vec![
                ValueAlternative::ImageAddress { address: 16 },
                ValueAlternative::EntryStack { offset: 0 },
            ])
            .unwrap(),
        };
        assert_eq!(
            in_frame(&stack, &[], &callee).unwrap(),
            AbstractValue::Unknown
        );
    }
    #[test]
    fn arguments_returns_and_may_writes_keep_callee_provenance_and_budget() {
        let x = AbstractValue::Expression { id: 0 };
        let mut args = vec![AbstractValue::Unknown; 32];
        args[10] = x.clone();
        let root = vec![
            expr(0, Expression::EntryRegister { register: 10 }),
            expr(
                1,
                Expression::CallResult {
                    callsite: 4,
                    register: 10,
                },
            ),
            FunctionRecord::CallInputs {
                offset: 4,
                registers: args,
            },
            FunctionRecord::ReturnValue {
                offset: 8,
                low: AbstractValue::Expression { id: 1 },
                high: AbstractValue::Unknown,
            },
            FunctionRecord::SemanticGap {
                offset: 4,
                reason: SemanticGapReason::OpaqueCall,
            },
        ];
        let callee = vec![
            expr(0, Expression::EntryRegister { register: 10 }),
            expr(
                1,
                Expression::Integer {
                    op: IntegerOp::Add,
                    left: x,
                    right: AbstractValue::Constant { value: 7 },
                },
            ),
            FunctionRecord::MemoryAccess {
                offset: 4,
                access: MemoryKind::Store,
                width: 4,
                address: AbstractValue::Constant { value: 0x60000000 },
                value: Some(AbstractValue::Expression { id: 1 }),
                relocation: None,
            },
            FunctionRecord::ReturnValue {
                offset: 8,
                low: AbstractValue::Expression { id: 1 },
                high: AbstractValue::Unknown,
            },
        ];
        let artifact = ArtifactId::of_bytes(b"callee");
        let id: FunctionAnalysisId = artifact.as_str().parse().unwrap();
        let source = FunctionSource::Input { input: 0 };
        let object = ObjectId {
            artifact,
            location: ObjectLocation::Standalone,
        };
        let selected = [Callee {
            offset: 4,
            analysis: &id,
            source: &source,
            object: &object,
            records: &callee,
        }];
        let small = WorkingMemory::new(1).unwrap();
        assert_eq!(
            compose(&root, &selected, &small, &mut || Ok(()))
                .err()
                .unwrap()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(small.used(), 0);
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let result = compose(&root, &selected, &memory, &mut || Ok(())).unwrap();
        assert!(result.records.iter().any(|r| matches!(r, FunctionRecord::CalleeEffect { analysis, value: Some(AbstractValue::Expression { .. }), .. } if analysis == &id)));
        assert!(result.records.iter().any(|r| matches!(r, FunctionRecord::Expression { origin: Some(origin), expression: Expression::Integer { op: IntegerOp::Add, .. }, .. } if origin == &id)));
        assert!(!result.records.iter().any(|r| matches!(
            r,
            FunctionRecord::SemanticGap { .. }
                | FunctionRecord::Expression {
                    expression: Expression::CallResult { .. },
                    ..
                }
        )));
        assert!(memory.used() > 0);
        drop(result);
        assert_eq!(memory.used(), 0);
        let result = compose(&root, &[], &memory, &mut || Ok(())).unwrap();
        assert!(result.records.iter().any(|r| matches!(
            r,
            FunctionRecord::SemanticGap {
                reason: SemanticGapReason::OpaqueCall,
                ..
            }
        )));
    }
    #[test]
    fn invalid_dag_and_cancellation_fail_without_a_summary() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let bad = vec![expr(
            0,
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: 0 },
                right: AbstractValue::Constant { value: 1 },
            },
        )];
        assert_eq!(
            compose(&bad, &[], &memory, &mut || Ok(()))
                .err()
                .unwrap()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(memory.used(), 0);
        assert_eq!(
            compose(&bad, &[], &memory, &mut || Err(Error::new(
                ErrorCode::Cancelled,
                "stop"
            )))
            .err()
            .unwrap()
            .code,
            ErrorCode::Cancelled
        );
        assert_eq!(memory.used(), 0);
    }
    #[test]
    fn constant_callee_return_resolves_a_caller_memory_address() {
        let artifact = ArtifactId::of_bytes(b"address-provider");
        let id: FunctionAnalysisId = artifact.as_str().parse().unwrap();
        let source = FunctionSource::Input { input: 1 };
        let object = ObjectId {
            artifact,
            location: ObjectLocation::Standalone,
        };
        let root = vec![
            expr(
                0,
                Expression::CallResult {
                    callsite: 4,
                    register: 10,
                },
            ),
            expr(
                1,
                Expression::Integer {
                    op: IntegerOp::Add,
                    left: AbstractValue::Expression { id: 0 },
                    right: AbstractValue::Constant { value: 4 },
                },
            ),
            FunctionRecord::CallInputs {
                offset: 4,
                registers: vec![AbstractValue::Unknown; 32],
            },
            FunctionRecord::MemoryAccess {
                offset: 12,
                access: MemoryKind::Store,
                width: 4,
                address: AbstractValue::Expression { id: 1 },
                value: Some(AbstractValue::Constant { value: 7 }),
                relocation: None,
            },
        ];
        let callee = [FunctionRecord::ReturnValue {
            offset: 0,
            low: AbstractValue::Constant { value: 0x60000000 },
            high: AbstractValue::Unknown,
        }];
        let selected = [Callee {
            offset: 4,
            analysis: &id,
            source: &source,
            object: &object,
            records: &callee,
        }];
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let result = compose(&root, &selected, &memory, &mut || Ok(())).unwrap();
        assert!(result.records.iter().any(|r| matches!(
            r,
            FunctionRecord::MemoryAccess {
                address: AbstractValue::Constant { value: 0x60000004 },
                ..
            }
        )));
    }
}
