//! Resolve only selected physical coordinates; retain uncertainty and saved references.
use super::*;
#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum Coordinate<'a> {
    Image(u64),
    Section(u32, u64),
    Symbol(&'a SymbolId),
}
#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct Key<'a> {
    source: &'a FunctionSource,
    object: &'a ObjectId,
    coordinate: Coordinate<'a>,
}
struct Entry<'a> {
    key: Key<'a>,
    node: usize,
}
fn append(
    index: &[Entry<'_>],
    key: Key<'_>,
    selected: &mut AdmittedVec<'_, usize>,
    c: &mut dyn RunControl,
) -> Result<()> {
    c.checkpoint(index.len().max(1).ilog2() as u64 + 1)?;
    let start = index.partition_point(|x| x.key < key);
    for entry in index[start..].iter().take_while(|x| x.key == key) {
        c.checkpoint(1)?;
        selected.push(entry.node, c.position())?;
    }
    Ok(())
}
fn candidates(
    index: &[Entry<'_>],
    caller: &Node<'_>,
    value: &AbstractValue,
    selected: &mut AdmittedVec<'_, usize>,
    c: &mut dyn RunControl,
) -> Result<()> {
    let mut source = &caller.function.location.source;
    let mut object = caller.function.location.selector.object();
    let coordinate = match value {
        AbstractValue::ImageAddress { address } | AbstractValue::Constant { value: address }
            if caller.space == CodeAddressSpace::Image =>
        {
            Coordinate::Image(u64::from(*address))
        }
        AbstractValue::ScopedAddress {
            source: s,
            object: o,
            address,
        } => {
            source = s;
            object = o;
            Coordinate::Image(u64::from(*address))
        }
        AbstractValue::Section { section, offset } if caller.space == CodeAddressSpace::Section => {
            let Ok(offset) = u64::try_from(*offset) else {
                return Ok(());
            };
            Coordinate::Section(*section, offset)
        }
        AbstractValue::Symbol { symbol, addend: 0 } => {
            object = &symbol.object;
            Coordinate::Symbol(symbol)
        }
        _ => return Ok(()),
    };
    append(
        index,
        Key {
            source,
            object,
            coordinate,
        },
        selected,
        c,
    )
}
pub(super) fn emit(
    nodes: &[Node<'_>],
    pending: &[Pending<'_>],
    focus: Option<&FunctionLocation>,
    direction: CallDirection,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let mut index = AdmittedVec::new(memory);
    for (node, descriptor) in nodes.iter().enumerate() {
        c.checkpoint(1)?;
        let source = &descriptor.function.location.source;
        let object = descriptor.function.location.selector.object();
        let coordinate = if descriptor.space == CodeAddressSpace::Image {
            Coordinate::Image(descriptor.extent.start)
        } else {
            Coordinate::Section(descriptor.section, descriptor.extent.start)
        };
        index.push(
            Entry {
                key: Key {
                    source,
                    object,
                    coordinate,
                },
                node,
            },
            c.position(),
        )?;
        if !descriptor.user_extent
            && let Some(symbol) = descriptor.function.location.selector.symbol()
        {
            index.push(
                Entry {
                    key: Key {
                        source,
                        object,
                        coordinate: Coordinate::Symbol(symbol),
                    },
                    node,
                },
                c.position(),
            )?;
        }
    }
    sort_charge(index.len(), c)?;
    index.sort_unstable_by(|a, b| a.key.cmp(&b.key).then(a.node.cmp(&b.node)));
    for pending in pending {
        c.checkpoint(1)?;
        let caller = &nodes[pending.caller];
        if direction == CallDirection::Callees
            && focus.is_some_and(|f| f != &caller.function.location)
        {
            continue;
        }
        let call = &pending.call;
        let mut selected = AdmittedVec::new(memory);
        if let AbstractValue::Alternatives { values } = &call.target {
            for value in values.values() {
                c.checkpoint(1)?;
                candidates(&index, caller, &value.as_value(), &mut selected, c)?;
            }
        } else {
            candidates(&index, caller, &call.target, &mut selected, c)?;
        }
        let mut outside = false;
        if let Some(id) = &call.saved_resolution {
            c.checkpoint(nodes.len().max(1).ilog2() as u64 + 1)?;
            match nodes.binary_search_by(|n| n.function.analysis.cmp(id)) {
                Ok(i) => selected.push(i, c.position())?,
                Err(_) => outside = true,
            }
        }
        sort_charge(selected.len(), c)?;
        selected.sort_unstable();
        let focus_match = focus.map(|f| match direction {
            CallDirection::Callees => &caller.function.location == f,
            CallDirection::Callers => selected.iter().any(|i| &nodes[*i].function.location == f),
        });
        // Unclassified transfers are visible but never labeled as confirmed callers of the focus.
        if direction == CallDirection::Callers
            && focus_match == Some(false)
            && !selected.is_empty()
            && !outside
        {
            continue;
        }
        let mut previous = None;
        let mut count = 0;
        let mut bytes = 0u64;
        for &i in selected.iter() {
            if previous == Some(i) {
                continue;
            }
            previous = Some(i);
            count += 1;
            bytes = bytes
                .checked_add(
                    std::mem::size_of::<NavigationFunction>() as u64
                        + nodes[i].function.allocated_bytes(),
                )
                .ok_or_else(|| invalid("navigation candidate capacity overflow"))?;
        }
        let _capacity = memory.reserve(bytes, c.position())?;
        let mut owned = Vec::new();
        owned.try_reserve_exact(count).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "navigation candidate allocation refused",
            )
        })?;
        previous = None;
        for &i in selected.iter() {
            c.checkpoint(1)?;
            if previous != Some(i) {
                owned.push(nodes[i].function.clone());
            }
            previous = Some(i);
        }
        let issue = if outside {
            Some(NavigationIssue::OutsideSelection)
        } else if count == 0 {
            Some(NavigationIssue::UnresolvedTarget)
        } else if count > 1 || matches!(call.target, AbstractValue::Alternatives { .. }) {
            Some(NavigationIssue::AmbiguousTarget)
        } else {
            None
        };
        emit(
            &NavigationRecord::Call {
                caller: caller.function.clone(),
                record: call.record,
                offset: call.offset,
                call: call.call,
                target: call.target.clone(),
                saved_resolution: call.saved_resolution.clone(),
                candidates: owned,
                focus_match,
                issue,
            },
            c,
        )?;
    }
    Ok(())
}
