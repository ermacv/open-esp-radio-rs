//! Bounded discovery from saved local facts. No source reads, publication or model execution.
#[cfg(test)]
use crate::paths::PathKey;
use crate::paths::address_paths;
use blobray_domain::*;
fn integrity(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn paths(
    v: &AbstractValue,
    expressions: &[&Expression],
    recipe: &FunctionRecipe,
    abi: Option<CallAbi>,
    c: &mut dyn RunControl,
) -> Result<(Vec<InterfaceAccessPath>, Option<AccessIssue>)> {
    let (values, issue) = address_paths(v, expressions, recipe, abi, c)?;
    if issue.is_some() {
        return Ok((vec![], issue));
    }
    let mut paths = Vec::with_capacity(values.len());
    for mut value in values {
        if value.offset != 0 {
            return Ok((vec![], Some(AccessIssue::NonzeroCallDisplacement)));
        }
        let Some(AccessStep::LoadPointer { offset }) = value.path.pop() else {
            return Ok((vec![], Some(AccessIssue::NoPointerPath)));
        };
        let Ok(slot) = u32::try_from(offset) else {
            return Ok((vec![], Some(AccessIssue::OffsetOutOfRange)));
        };
        if !slot.is_multiple_of(4) {
            return Ok((vec![], Some(AccessIssue::NonPointerLoad)));
        }
        paths.push(InterfaceAccessPath {
            root: value.root,
            path: value.path,
            slot,
        });
    }
    Ok((paths, None))
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
    let _construction = memory.reserve(CONTROL_MESSAGE_BYTES as u64, c.position())?;
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
            (vec![], Some(AccessIssue::NonzeroCallDisplacement))
        } else if let Some(v) = input {
            paths(v, &expressions, recipe, abi, c)?
        } else {
            (vec![], Some(AccessIssue::MissingCallInputs))
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
                root: AccessRoot::EntryWord {
                    function: recipe.selector.clone(),
                    word: 0
                },
                path: vec![AccessStep::LoadPointer { offset: 0 }],
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
            Some(AccessIssue::AbiRequired)
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
                root: AccessRoot::Address { address: 0x1000 },
                path: vec![AccessStep::Index { word: 1, stride: 4 }],
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
        assert_eq!(found[1].root, AccessRoot::Address { address: 0x2000 });
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
            &AccessRoot::Address { address: 0x1000 },
            &[
                AccessStep::Offset { bytes: 4 },
                AccessStep::LoadPointer { offset: 4 },
            ],
            8,
        );
        let b = PathKey::new(
            &AccessRoot::Address { address: 0x1008 },
            &[AccessStep::LoadPointer { offset: 0 }],
            8,
        );
        assert_eq!(a, b);
        assert_ne!(
            a,
            PathKey::new(&AccessRoot::Address { address: 0x1008 }, &[], 8)
        );
    }
}
