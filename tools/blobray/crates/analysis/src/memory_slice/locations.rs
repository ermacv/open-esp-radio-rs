//! Canonical local locations and conservative, explicit alias boundaries.
use crate::navigation::Facts;
use blobray_domain::*;
pub(crate) fn bytes(location: &SliceLocation) -> u64 {
    match &location.address {
        SliceAddress::Stack { .. } | SliceAddress::Value { .. } => 0,
        SliceAddress::Scoped { source, object, .. } => {
            source.allocated_bytes() + object.artifact.allocated_bytes()
        }
        SliceAddress::Path { path } => {
            (path.path.capacity() * std::mem::size_of::<AccessStep>()) as u64
                + match &path.root {
                    AccessRoot::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
                    AccessRoot::EntryWord { function, .. } => {
                        function.object().artifact.allocated_bytes()
                    }
                    _ => 0,
                }
        }
    }
}
fn scoped(recipe: &FunctionRecipe, address: u32) -> SliceAddress {
    SliceAddress::Scoped {
        source: recipe.source.clone(),
        object: recipe.selector.object().clone(),
        address,
    }
}
fn canonical(
    mut path: AccessPath,
    recipe: &FunctionRecipe,
    facts: &Facts<'_, '_>,
    c: &mut dyn RunControl,
) -> Result<SliceAddress> {
    if let AccessRoot::Symbol { symbol, addend } = &path.root
        && let Some(target) = facts.reference(symbol, c)?
        && target.definition == SymbolDefinition::Section
    {
        let at = i128::from(target.offset) + i128::from(*addend);
        if let Ok(at) = u32::try_from(at) {
            path.root = if recipe.address_space == CodeAddressSpace::Image {
                AccessRoot::Address { address: at }
            } else if let Some(section) = target.section {
                AccessRoot::Section {
                    section,
                    offset: u64::from(at),
                }
            } else {
                path.root
            };
        }
    }
    if path.path.is_empty() {
        match &mut path.root {
            AccessRoot::Address { address } => {
                return Ok(scoped(recipe, address.wrapping_add_signed(path.offset)));
            }
            AccessRoot::Section { offset, .. } => {
                if let Some(at) = offset.checked_add_signed(i64::from(path.offset)) {
                    *offset = at;
                    path.offset = 0;
                }
            }
            AccessRoot::Symbol { addend, .. } => {
                if let Some(at) = addend.checked_add(i64::from(path.offset)) {
                    *addend = at;
                    path.offset = 0;
                }
            }
            _ => (),
        }
    }
    Ok(SliceAddress::Path { path })
}
pub(crate) fn identify(
    value: &AbstractValue,
    width: u8,
    recipe: &FunctionRecipe,
    facts: &Facts<'_, '_>,
    stable: &[bool],
    abi: Option<CallAbi>,
    c: &mut dyn RunControl,
) -> Result<(Vec<SliceLocation>, Option<AccessIssue>)> {
    let mut out = Vec::new();
    let mut issue = None;
    let mut one = |value: &AbstractValue, c: &mut dyn RunControl| -> Result<()> {
        c.checkpoint(1)?;
        match value {
            AbstractValue::EntryStack { offset } => out.push(SliceLocation {
                address: SliceAddress::Stack { offset: *offset },
                width,
            }),
            AbstractValue::ScopedAddress {
                source,
                object,
                address,
            } => out.push(SliceLocation {
                address: SliceAddress::Scoped {
                    source: source.clone(),
                    object: object.clone(),
                    address: *address,
                },
                width,
            }),
            _ => {
                if let Some(address) = scalar(value, facts, stable, c)? {
                    out.push(SliceLocation { address, width });
                    return Ok(());
                }
                let (paths, problem) = facts.paths(value, recipe, abi, c)?;
                issue = issue.or(problem);
                for path in paths {
                    c.checkpoint(1)?;
                    out.push(SliceLocation {
                        address: canonical(path, recipe, facts, c)?,
                        width,
                    });
                }
            }
        }
        Ok(())
    };
    if let AbstractValue::Alternatives { values } = value {
        for value in values.values() {
            one(&value.as_value(), c)?;
        }
    } else {
        one(value, c)?;
    }
    out.sort_unstable();
    out.dedup();
    Ok((out, issue))
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Relation {
    Exact,
    Disjoint,
    Partial,
    Covering,
    Dynamic,
    MayAlias,
}
fn range(a: u32, aw: u8, b: u32, bw: u8) -> Relation {
    if a == b && aw == bw {
        Relation::Exact
    } else if u64::from(b.wrapping_sub(a)) + u64::from(bw) <= u64::from(aw) {
        Relation::Covering
    } else if a.wrapping_sub(b) < u32::from(bw) || b.wrapping_sub(a) < u32::from(aw) {
        Relation::Partial
    } else {
        Relation::Disjoint
    }
}
pub(super) fn relation(a: &SliceLocation, b: &SliceLocation) -> Relation {
    match (&a.address, &b.address) {
        (
            SliceAddress::Value {
                expression: a_id,
                offset: x,
            },
            SliceAddress::Value {
                expression: b_id,
                offset: y,
            },
        ) if a_id == b_id => range(*x as u32, a.width, *y as u32, b.width),
        (SliceAddress::Stack { offset: x }, SliceAddress::Stack { offset: y }) => {
            range(*x as u32, a.width, *y as u32, b.width)
        }
        (
            SliceAddress::Scoped {
                source: s,
                object: o,
                address: x,
            },
            SliceAddress::Scoped {
                source: t,
                object: p,
                address: y,
            },
        ) if s == t && o == p => range(*x, a.width, *y, b.width),
        (SliceAddress::Path { path: x }, SliceAddress::Path { path: y }) => {
            if !x.path.is_empty() || !y.path.is_empty() {
                return if x == y {
                    Relation::Dynamic
                } else {
                    Relation::MayAlias
                };
            }
            let pair = match (&x.root, &y.root) {
                (
                    AccessRoot::EntryWord {
                        function: f,
                        word: a,
                    },
                    AccessRoot::EntryWord {
                        function: g,
                        word: b,
                    },
                ) if f == g && a == b => {
                    // Stack argument cells are mutable; a saved load value has its own Value identity.
                    if *a >= 8 {
                        return Relation::Dynamic;
                    }
                    Some((0, 0))
                }
                (
                    AccessRoot::Section {
                        section: s,
                        offset: a,
                    },
                    AccessRoot::Section {
                        section: t,
                        offset: b,
                    },
                ) if s == t => Some((*a as u32, *b as u32)),
                (
                    AccessRoot::Symbol {
                        symbol: s,
                        addend: a,
                    },
                    AccessRoot::Symbol {
                        symbol: t,
                        addend: b,
                    },
                ) if s == t => Some((*a as u32, *b as u32)),
                (AccessRoot::Address { address: a }, AccessRoot::Address { address: b }) => {
                    Some((*a, *b))
                }
                _ => None,
            };
            pair.map_or(Relation::MayAlias, |(a_offset, b_offset)| {
                range(
                    a_offset.wrapping_add_signed(x.offset),
                    a.width,
                    b_offset.wrapping_add_signed(y.offset),
                    b.width,
                )
            })
        }
        _ => Relation::MayAlias,
    }
}

fn scalar(
    value: &AbstractValue,
    facts: &Facts<'_, '_>,
    stable: &[bool],
    c: &mut dyn RunControl,
) -> Result<Option<SliceAddress>> {
    let AbstractValue::Expression { id: original } = value else {
        return Ok(None);
    };
    let mut id = *original;
    let mut offset = 0i32;
    for _ in 0..16 {
        c.checkpoint(1)?;
        let expression = facts.expression(id)?;
        match expression {
            Expression::EntryRegister { .. } => return Ok(None),
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: next },
                right: AbstractValue::Constant { value },
            }
            | Expression::Integer {
                op: IntegerOp::Add,
                right: AbstractValue::Expression { id: next },
                left: AbstractValue::Constant { value },
            } => {
                id = *next;
                offset = offset.wrapping_add(*value as i32);
            }
            Expression::Integer {
                op: IntegerOp::Sub,
                left: AbstractValue::Expression { id: next },
                right: AbstractValue::Constant { value },
            } => {
                id = *next;
                offset = offset.wrapping_sub(*value as i32);
            }
            _ => {
                return Ok(stable.get(id as usize).copied().unwrap_or(false).then_some(
                    SliceAddress::Value {
                        expression: id,
                        offset,
                    },
                ));
            }
        }
    }
    Ok(stable
        .get(*original as usize)
        .copied()
        .unwrap_or(false)
        .then_some(SliceAddress::Value {
            expression: *original,
            offset: 0,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_frame_spans_handle_partial_overlap_and_rv32_wrap() {
        let loc = |offset, width| SliceLocation {
            address: SliceAddress::Stack { offset },
            width,
        };
        assert_eq!(relation(&loc(-4, 4), &loc(0, 4)), Relation::Disjoint);
        assert_eq!(relation(&loc(-2, 4), &loc(0, 4)), Relation::Partial);
        assert_eq!(relation(&loc(0, 4), &loc(1i64 << 32, 4)), Relation::Exact);
        assert_eq!(relation(&loc(0, 4), &loc(2, 2)), Relation::Covering);
    }
}
