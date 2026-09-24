//! Iterative physical access paths; never acquires sources or chooses bindings.
use blobray_domain::*;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum KeyStep {
    Load(i128),
    Index(u8, u32),
    Offset(i128),
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PathKey {
    root: AccessRoot,
    steps: Vec<KeyStep>,
}
impl PathKey {
    pub fn new(root: &AccessRoot, path: &[AccessStep], slot: u32) -> Self {
        let mut root = root.clone();
        let mut offset = match &mut root {
            AccessRoot::Symbol { addend, .. } => {
                let n = *addend;
                *addend = 0;
                i128::from(n)
            }
            AccessRoot::Address { address } => {
                let n = *address;
                *address = 0;
                i128::from(n)
            }
            AccessRoot::Section { offset, .. } => {
                let n = *offset;
                *offset = 0;
                i128::from(n)
            }
            _ => 0,
        };
        let mut steps = Vec::with_capacity(2 * path.len() + 1);
        for step in path {
            match step {
                AccessStep::Offset { bytes } => offset += i128::from(*bytes),
                AccessStep::LoadPointer { offset: bytes } => {
                    steps.push(KeyStep::Load(offset + i128::from(*bytes)));
                    offset = 0;
                }
                AccessStep::Index { word, stride } => {
                    if offset != 0 {
                        steps.push(KeyStep::Offset(offset));
                        offset = 0;
                    }
                    steps.push(KeyStep::Index(*word, *stride));
                }
            }
        }
        steps.push(KeyStep::Offset(offset + i128::from(slot)));
        Self { root, steps }
    }
    pub fn allocated_bytes(&self) -> u64 {
        (self.steps.capacity() * std::mem::size_of::<KeyStep>()) as u64
            + match &self.root {
                AccessRoot::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
                AccessRoot::EntryWord { function, .. } => {
                    function.object().artifact.allocated_bytes()
                }
                _ => 0,
            }
    }
}
fn integrity(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn word(register: u8, abi: Option<CallAbi>) -> std::result::Result<u8, AccessIssue> {
    if abi.is_none() {
        return Err(AccessIssue::AbiRequired);
    }
    if (10..=17).contains(&register) {
        Ok(register - 10)
    } else {
        Err(AccessIssue::UnsupportedArgument)
    }
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
            step @ AccessStep::Index { .. } => {
                if offset != 0 {
                    path.push(AccessStep::Offset {
                        bytes: i32::try_from(offset).map_err(|_| AccessIssue::OffsetOutOfRange)?,
                    });
                    offset = 0;
                }
                path.push(*step);
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
        .then(|| word(register, abi).ok().map(|arg| (arg, stride)))
        .flatten())
}
pub fn address_paths(
    v: &AbstractValue,
    expressions: &[&Expression],
    recipe: &FunctionRecipe,
    abi: Option<CallAbi>,
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
                    Expression::EntryRegister { register } => match word(*register, abi) {
                        Ok(word) => Ok(AccessRoot::EntryWord {
                            function: recipe.selector.clone(),
                            word,
                        }),
                        Err(issue) => Err(issue),
                    },
                    Expression::Load {
                        address: AbstractValue::EntryStack { offset },
                        width: 4,
                        ..
                    } => {
                        if abi.is_none() {
                            Err(AccessIssue::AbiRequired)
                        } else if *offset >= 0 && *offset % 4 == 0 && *offset / 4 < 56 {
                            Ok(AccessRoot::EntryWord {
                                function: recipe.selector.clone(),
                                word: 8 + (*offset / 4) as u8,
                            })
                        } else {
                            Err(AccessIssue::UnsupportedArgument)
                        }
                    }
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
                        if *op == IntegerOp::Add {
                            if let Some((word, stride)) = index_operand(right, expressions, abi)? {
                                reverse.push(AccessStep::Index { word, stride });
                                current = left;
                                continue;
                            }
                            if let Some((word, stride)) = index_operand(left, expressions, abi)? {
                                reverse.push(AccessStep::Index { word, stride });
                                current = right;
                                continue;
                            }
                        }
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
