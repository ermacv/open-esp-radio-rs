//! Collect detail already received before a correlated pause completion.
use open_esp_radio_hil_protocol::{Envelope, Event, PhyTxWaitEvidence};

pub(super) fn tx_waits(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<PhyTxWaitEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationPhyTxWaits(waits) => Some(*waits),
        _ => None,
    })
}

pub(super) fn timer(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<open_esp_radio_hil_protocol::TimerWindowEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationTimerObserved(timer) => Some(*timer),
        _ => None,
    })
}

fn detail<T>(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
    select: impl Fn(&Event) -> Option<T>,
) -> crate::Result<Option<T>> {
    let boundary = messages
        .iter()
        .position(|message| message == completion)
        .ok_or("pause completion absent from retained event stream")?;
    let mut detail = None;
    for message in &messages[..boundary] {
        if message.boot_id != completion.boot_id
            || message.session_id != completion.session_id
            || message.request_id != completion.request_id
        {
            continue;
        }
        if let Some(value) = select(&message.body)
            && detail.replace(value).is_some()
        {
            return Err("duplicate diagnostic detail for pause request".into());
        }
    }
    Ok(detail)
}

#[cfg(test)]
mod tests;
