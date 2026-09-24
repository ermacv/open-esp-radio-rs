//! Local field/selector/dataflow checks run while one fact owner is borrowed.
use super::*;
fn target<'m>(
    site: SavedSite,
    value: RouteValue,
    callback: FunctionAnalysisId,
    out: &mut AdmittedVec<'m, Owned<'m, Target>>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    let capacity = memory.reserve(
        site.analysis.allocated_bytes() + value.allocated_bytes() + callback.allocated_bytes(),
        c.position(),
    )?;
    out.push(
        Owned {
            value: Target {
                site,
                value,
                callback,
            },
            _capacity: capacity,
        },
        c.position(),
    )
}
fn load<'a>(
    prepared: &'a Prepared<'_, '_, '_>,
    site: &SavedSite,
    width: u8,
) -> Result<(u64, &'a AbstractValue)> {
    match prepared.record(site.record)? {
        FunctionRecord::MemoryAccess {
            offset,
            access: MemoryKind::Load,
            width: w,
            address,
            ..
        } if *w == width => Ok((*offset, address)),
        _ => Err(invalid(
            "route field must name an exact local load of the declared width",
        )),
    }
}
fn dataflow(value: &RouteValue, producer: ValueProducer) -> RouteCheckStatus {
    if value.producer == Some(producer) && value.addend == 0 {
        RouteCheckStatus::Established
    } else if value.constant().is_some() {
        RouteCheckStatus::Mismatch
    } else {
        RouteCheckStatus::Unresolved
    }
}
fn load_case(
    prepared: &Prepared<'_, '_, '_>,
    case: &RouteCase,
    load_offset: u64,
    width: u8,
    selector: u32,
    c: &mut dyn RunControl,
) -> Result<RouteCheckStatus> {
    let FunctionRecord::Condition { left, right, .. } = prepared.record(case.condition.record)?
    else {
        return Err(invalid("case site is not a branch condition"));
    };
    for v in [left, right] {
        let v = prepared.value(v, c)?;
        if let Some(
            p @ ValueProducer::Load {
                offset, width: w, ..
            },
        ) = v.producer
            && offset == load_offset
            && w == width
        {
            return prepared.case(case, p, selector, c);
        }
    }
    Ok(RouteCheckStatus::Unresolved)
}
pub(super) fn inspect<'m>(
    route: &ReviewedEventRoute,
    id: &FunctionAnalysisId,
    prepared: &Prepared<'_, '_, '_>,
    rows: &mut Rows<'m>,
    targets: &mut AdmittedVec<'m, Owned<'m, Target>>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    match &route.route {
        EventRouteMechanism::SelectorDelivery(r) if *id == r.delivery.call.caller => {
            let (at, address) = load(prepared, &r.selector_load, r.selector_width)?;
            let address = prepared.value(address, c)?;
            let output = prepared.argument(r.delivery.call.record, r.delivery.word, c)?;
            rows.fact(
                &r.selector_load,
                prepared.record(r.selector_load.record)?,
                memory,
                c,
            )?;
            rows.fact(
                &r.case.condition,
                prepared.record(r.case.condition.record)?,
                memory,
                c,
            )?;
            rows.check(
                "delivered-selector-field",
                same_pointer(&address, &output, r.selector_offset),
                vec![site(&r.delivery.call), r.selector_load.clone()],
                memory,
                c,
            )?;
            rows.check(
                "receive-before-selector-load",
                prepared.path(offset(prepared.record(r.delivery.call.record)?)?, at, c)?,
                vec![site(&r.delivery.call), r.selector_load.clone()],
                memory,
                c,
            )?;
            rows.check(
                "selector-load-before-condition",
                prepared.path(at, offset(prepared.record(r.case.condition.record)?)?, c)?,
                vec![r.selector_load.clone(), r.case.condition.clone()],
                memory,
                c,
            )?;
            rows.check(
                "selector-handler-case",
                load_case(prepared, &r.case, at, r.selector_width, r.selector, c)?,
                vec![
                    r.selector_load.clone(),
                    r.case.condition.clone(),
                    site(&r.case.handler),
                ],
                memory,
                c,
            )?;
        }
        EventRouteMechanism::StaticCallback(r) => {
            if *id == r.registration.call.caller {
                target(
                    site(&r.registration.call),
                    prepared.argument(
                        r.registration.call.record,
                        r.registration.callback_word,
                        c,
                    )?,
                    r.callback.clone(),
                    targets,
                    memory,
                    c,
                )?;
            }
            if *id == r.receive.call.caller {
                let invoke = prepared.argument(r.invoke.call.record, r.invoke.word, c)?;
                let receive = offset(prepared.record(r.receive.call.record)?)?;
                let invoked = offset(prepared.record(r.invoke.call.record)?)?;
                rows.check(
                    "receive-before-invoke",
                    prepared.path(receive, invoked, c)?,
                    vec![site(&r.receive.call), site(&r.invoke.call)],
                    memory,
                    c,
                )?;
                let status = match &r.receive.output {
                    EventReceiveOutput::Return => dataflow(
                        &invoke,
                        ValueProducer::CallResult {
                            offset: receive,
                            register: 10,
                        },
                    ),
                    EventReceiveOutput::Argument { word, load: site } => {
                        let (at, address) = load(prepared, site, 4)?;
                        rows.fact(site, prepared.record(site.record)?, memory, c)?;
                        let address = prepared.value(address, c)?;
                        let output = prepared.argument(r.receive.call.record, *word, c)?;
                        rows.check(
                            "received-object-load",
                            same_pointer(&address, &output, 0),
                            vec![super::site(&r.receive.call), site.clone()],
                            memory,
                            c,
                        )?;
                        rows.check(
                            "receive-before-object-load",
                            prepared.path(receive, at, c)?,
                            vec![super::site(&r.receive.call), site.clone()],
                            memory,
                            c,
                        )?;
                        rows.check(
                            "object-load-before-invoke",
                            prepared.path(at, invoked, c)?,
                            vec![site.clone(), super::site(&r.invoke.call)],
                            memory,
                            c,
                        )?;
                        // Signedness is immaterial for a full pointer word.
                        if invoke.addend == 0
                            && matches!(invoke.producer,Some(ValueProducer::Load {offset,width:4,..}) if offset==at)
                        {
                            RouteCheckStatus::Established
                        } else if invoke.constant().is_some() {
                            RouteCheckStatus::Mismatch
                        } else {
                            RouteCheckStatus::Unresolved
                        }
                    }
                };
                rows.check(
                    "received-object-invocation",
                    status,
                    vec![site(&r.receive.call), site(&r.invoke.call)],
                    memory,
                    c,
                )?;
            }
        }
        EventRouteMechanism::BrokerSubscription(r) => {
            if *id == r.subscribe.caller {
                let store = prepared.record(r.callback_store.record)?;
                let FunctionRecord::MemoryAccess {
                    access: MemoryKind::Store,
                    width: 4,
                    address,
                    value: Some(value),
                    ..
                } = store
                else {
                    return Err(invalid(
                        "broker callback field must name an exact pointer-sized store",
                    ));
                };
                rows.fact(&r.callback_store, store, memory, c)?;
                let address = prepared.value(address, c)?;
                let object = prepared.argument(r.subscribe.record, r.subscriber_word, c)?;
                rows.check(
                    "subscriber-callback-field",
                    same_pointer(&address, &object, r.callback_offset),
                    vec![r.callback_store.clone(), site(&r.subscribe)],
                    memory,
                    c,
                )?;
                target(
                    r.callback_store.clone(),
                    prepared.value(value, c)?,
                    r.callback.clone(),
                    targets,
                    memory,
                    c,
                )?;
                let mut status = RouteCheckStatus::Mismatch;
                let slice = MemorySliceQuery {
                    analysis: id.clone(),
                    anchor: r.subscribe.record,
                    abi: Some(CallAbi::RiscvInteger),
                    locations: vec![MemorySliceSelection::Access {
                        record: r.callback_store.record,
                    }],
                };
                if address.issue.is_none() && !address.locations.is_empty() {
                    prepared.last_writes(&slice, c, &mut |record, _| {
                        match record {
                            MemorySliceRecord::Location { issues, .. } if !issues.is_empty() => {
                                status = RouteCheckStatus::Unresolved
                            }
                            MemorySliceRecord::Definition { record, class, .. }
                                if *record == r.callback_store.record =>
                            {
                                status = if *class == DefinitionClass::Must {
                                    RouteCheckStatus::Established
                                } else {
                                    RouteCheckStatus::Unresolved
                                };
                            }
                            _ => (),
                        }
                        Ok(())
                    })?;
                } else {
                    status = RouteCheckStatus::Unresolved;
                }
                rows.check(
                    "callback-store-reaches-subscription",
                    status,
                    vec![r.callback_store.clone(), site(&r.subscribe)],
                    memory,
                    c,
                )?;
            }
            if *id == r.callback {
                rows.fact(
                    &r.case.condition,
                    prepared.record(r.case.condition.record)?,
                    memory,
                    c,
                )?;
                let status = if r.callback_selector_word < 8 {
                    prepared.case(
                        &r.case,
                        ValueProducer::Entry {
                            register: 10 + r.callback_selector_word,
                        },
                        r.selector,
                        c,
                    )?
                } else {
                    RouteCheckStatus::Unresolved
                };
                rows.check(
                    "broker-selector-handler-case",
                    status,
                    vec![r.case.condition.clone(), site(&r.case.handler)],
                    memory,
                    c,
                )?;
            }
        }
        _ => (),
    }
    Ok(())
}
