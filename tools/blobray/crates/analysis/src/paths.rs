//! Iterative physical access paths; never acquires sources or chooses bindings.
use blobray_domain::*;
fn integrity(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn leaf(
    v: &AbstractValue,
    recipe: &FunctionRecipe,
) -> std::result::Result<AccessRoot, AccessIssue> {
    Ok(match v {
        AbstractValue::Constant { value } => AccessRoot::Address { address: *value },
        AbstractValue::ImageAddress { address } => AccessRoot::Address { address: *address },
        AbstractValue::Section { section, offset } => AccessRoot::Section {
            section: *section,
            offset: u64::try_from(*offset).map_err(|_| AccessIssue::OffsetOutOfRange)?,
        },
        AbstractValue::Symbol { symbol, addend } if symbol.object == *recipe.selector.object() => {
            AccessRoot::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            }
        }
        AbstractValue::Symbol { .. } | AbstractValue::ScopedAddress { .. } => {
            return Err(AccessIssue::ForeignOccurrence);
        }
        _ => return Err(AccessIssue::UnknownValue),
    })
}
fn finish(
    root: AccessRoot,
    reverse: &[AccessStep],
) -> std::result::Result<AccessPath, AccessIssue> {
    let mut path = Vec::with_capacity(reverse.len());
    let mut offset = 0i64;
    for step in reverse.iter().rev() {
        match step {
            AccessStep::Offset { bytes } => offset += i64::from(*bytes),
            AccessStep::LoadPointer { offset: bytes } => {
                let bytes = i32::try_from(offset + i64::from(*bytes))
                    .map_err(|_| AccessIssue::OffsetOutOfRange)?;
                path.push(AccessStep::LoadPointer { offset: bytes });
                offset = 0;
            }
        }
    }
    Ok(AccessPath {
        root,
        path,
        offset: i32::try_from(offset).map_err(|_| AccessIssue::OffsetOutOfRange)?,
    })
}

fn expression<'a>(id: u32, expressions: &[&'a Expression]) -> Result<&'a Expression> {
    expressions
        .get(id as usize)
        .copied()
        .ok_or_else(|| integrity("interface expression ID is absent"))
}
pub fn address_paths(
    v: &AbstractValue,
    expressions: &[&Expression],
    recipe: &FunctionRecipe,
    c: &mut dyn RunControl,
) -> Result<(Vec<AccessPath>, Option<AccessIssue>)> {
    let mut current = v;
    let mut reverse = Vec::with_capacity(16);
    let mut previous = None;
    loop {
        c.checkpoint(1)?;
        if reverse.len() >= 16 {
            return Ok((vec![], Some(AccessIssue::PathLimit)));
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
                    Expression::EntryRegister { .. }
                    | Expression::Load {
                        address: AbstractValue::EntryStack { .. },
                        width: 4,
                        ..
                    } => Err(AccessIssue::EntryArgument),
                    Expression::Load {
                        address, width: 4, ..
                    } => {
                        reverse.push(AccessStep::LoadPointer { offset: 0 });
                        current = address;
                        continue;
                    }
                    Expression::Load { .. } => Err(AccessIssue::NonPointerLoad),
                    Expression::CallResult { .. } => Err(AccessIssue::UnmodeledCallResult),
                    Expression::Integer { op, left, right } => {
                        let constant = match (op, left, right) {
                            (IntegerOp::And, v, AbstractValue::Constant { value: u32::MAX })
                            | (IntegerOp::And, AbstractValue::Constant { value: u32::MAX }, v) => {
                                Some((v, 0))
                            }
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
                                return Ok((vec![], Some(AccessIssue::OffsetOutOfRange)));
                            };
                            reverse.push(AccessStep::Offset { bytes });
                            current = v;
                            continue;
                        }
                        Err(AccessIssue::UnsupportedExpression)
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
