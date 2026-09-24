//! Finite declaration shape, independent of physical evidence acquisition.
use super::*;

pub fn validate(route: &ReviewedEventRoute) -> Result<()> {
    for text in [&route.execution_context, &route.applicability] {
        if text.trim().is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
            return Err(invalid(
                "event route needs bounded execution context and applicability",
            ));
        }
    }
    path(&route.upstream)?;
    path(&route.terminal)?;
    if route
        .upstream
        .last()
        .is_some_and(|h| &h.callee != route.root())
        || route
            .terminal
            .first()
            .is_some_and(|h| &h.caller != route.endpoint())
    {
        return Err(invalid(
            "event route path endpoint differs from its participant",
        ));
    }
    match &route.route {
        EventRouteMechanism::SelectorDelivery(r) => {
            words(&[r.dispatch.word, r.delivery.word])?;
            if !matches!(r.selector_width, 1 | 2 | 4)
                || u64::from(r.selector) >= 1u64 << (r.selector_width * 8)
                || r.selector_load.analysis != r.delivery.call.caller
                || r.case.condition.analysis != r.delivery.call.caller
                || r.case.handler.caller != r.delivery.call.caller
            {
                return Err(invalid(
                    "selector load, condition and handler must belong to the delivery caller with a valid selector width",
                ));
            }
        }
        EventRouteMechanism::StaticCallback(r) => {
            if r.dispatches.is_empty() || r.dispatches.len() > 16 {
                return Err(invalid("callback route needs 1..16 exact dispatches"));
            }
            for (i, d) in r.dispatches.iter().enumerate() {
                distinct(&[d.object_word, d.queue_word])?;
                if &d.call.caller != route.root()
                    || r.dispatches[..i].iter().any(|old| {
                        old.call.caller == d.call.caller && old.call.record == d.call.record
                    })
                {
                    return Err(invalid(
                        "dispatches must be distinct sites in one selected caller",
                    ));
                }
            }
            distinct(&[r.registration.object_word, r.registration.callback_word])?;
            words(&[r.receive.queue_word, r.invoke.word])?;
            if r.receive.call.caller != r.invoke.call.caller {
                return Err(invalid(
                    "receive and invoke need one exact consumer analysis",
                ));
            }
            if let EventReceiveOutput::Argument { word, load } = &r.receive.output {
                distinct(&[r.receive.queue_word, *word])?;
                if load.analysis != r.receive.call.caller {
                    return Err(invalid("receive output load belongs to another analysis"));
                }
            }
        }
        EventRouteMechanism::BrokerSubscription(r) => {
            distinct(&[r.object_word, r.selector_word, r.payload_word])?;
            distinct(&[r.domain.object_word, r.domain.selector_word])?;
            distinct(&[r.domain_word, r.subscriber_word])?;
            words(&[r.callback_selector_word])?;
            if r.callback_store.analysis != r.subscribe.caller
                || r.case.condition.analysis != r.callback
                || r.case.handler.caller != r.callback
            {
                return Err(invalid(
                    "broker callback store or selector case belongs to another participant",
                ));
            }
        }
    }
    Ok(())
}
fn words(words: &[u8]) -> Result<()> {
    if words.iter().any(|w| *w >= 64) {
        Err(invalid("route ABI word must be below 64"))
    } else {
        Ok(())
    }
}
fn distinct(values: &[u8]) -> Result<()> {
    words(values)?;
    if values
        .iter()
        .enumerate()
        .any(|(i, w)| values[..i].contains(w))
    {
        Err(invalid("different route roles need distinct ABI words"))
    } else {
        Ok(())
    }
}
fn path(hops: &[FlowHop]) -> Result<()> {
    if hops.len() > 16 {
        return Err(invalid("event route path exceeds 16 hops"));
    }
    for (i, h) in hops.iter().enumerate() {
        if i > 0 && hops[i - 1].callee != h.caller
            || hops[..=i].iter().any(|p| p.caller == h.callee)
        {
            return Err(invalid("event route paths must be contiguous and acyclic"));
        }
    }
    Ok(())
}
