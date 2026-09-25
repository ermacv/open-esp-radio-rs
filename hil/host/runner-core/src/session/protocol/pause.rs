//! Collect detail already received before a correlated pause completion.
use oer_hil_protocol::{Envelope, Event, PhyTxWaitEvidence};

#[derive(serde::Serialize)]
pub struct Report {
    pub evidence: oer_hil_protocol::StationPauseEvidence,
    pub rx_gain: Option<oer_hil_protocol::PhyRxGainEvidence>,
    pub tx_waits: Option<PhyTxWaitEvidence>,
    pub temperature: Option<oer_hil_protocol::TemperatureEvidence>,
    pub rfpll: Option<oer_hil_protocol::RfpllEvidence>,
    pub timer: Option<oer_hil_protocol::TimerWindowEvidence>,
    pub service: Option<oer_hil_protocol::StationTrackingServiceEvidence>,
}

pub(super) fn rfpll(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<oer_hil_protocol::RfpllEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationRfpllObserved(value) => Some(*value),
        _ => None,
    })
}

pub(super) fn temperature(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<oer_hil_protocol::TemperatureEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationTemperatureObserved(value) => Some(*value),
        _ => None,
    })
}

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
) -> crate::Result<Option<oer_hil_protocol::TimerWindowEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationTimerObserved(timer) => Some(*timer),
        _ => None,
    })
}

pub(super) fn service(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<oer_hil_protocol::StationTrackingServiceEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationTrackingService(service) => Some(*service),
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

pub(super) fn rx_gain(
    messages: &[Envelope<Event>],
    completion: &Envelope<Event>,
) -> crate::Result<Option<oer_hil_protocol::PhyRxGainEvidence>> {
    detail(messages, completion, |event| match event {
        Event::StationPhyRxGain(value) => Some(*value),
        _ => None,
    })
}
