//! Validate the already captured, correlated AP-stop accounting transaction.
use super::*;
use oer_hil_protocol::wifi::WifiAirtimePeerEvidence;

impl SerialCapture {
    /// Call after the stop completion. All accounting records must precede it;
    /// missing evidence is an error, not a reason to wait for a guessed delay.
    pub fn require_access_point_airtime(&self, handle: WifiCommandHandle) -> Result<()> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.check()?;
        validate(
            state.messages[handle.first_event..]
                .iter()
                .filter(|message| {
                    message.request_id == handle.request_id && message.session_id == 0
                }),
        )
    }
}

fn validate<'a>(messages: impl Iterator<Item = &'a Received>) -> Result<()> {
    let mut peers: Vec<WifiAirtimePeerEvidence> = Vec::new();
    let mut report = None;
    let mut stopped = false;
    for message in messages {
        if let Some(oer_hil_protocol::wifi::AirtimePeer(peer)) = message.decode() {
            if report.is_some() || stopped || peers.iter().any(|old| old.peer == peer.peer) {
                return Err("AP airtime has duplicate or out-of-order peer evidence".into());
            }
            let count = peer
                .settlements
                .checked_add(peer.cancellations)
                .and_then(|n| n.checked_add(u64::from(peer.outstanding)));
            let budget = peer
                .settled_grants_micros
                .checked_add(peer.cancelled_grants_micros)
                .and_then(|n| n.checked_add(peer.outstanding_micros));
            if count != Some(peer.grants)
                || budget != Some(peer.granted_micros)
                || peer.outstanding != 0
                || peer.outstanding_micros != 0
            {
                return Err(
                    format!("AP airtime reservations did not reconcile at stop: {peer:?}").into(),
                );
            }
            peers.push(peer);
        } else if let Some(oer_hil_protocol::wifi::AirtimeReport(value)) = message.decode() {
            if report.is_some()
                || stopped
                || usize::from(value.peer_records) != peers.len()
                || value.dropped_events != 0
                || value.saturated
            {
                return Err(format!("AP airtime report is incomplete: {value:?}").into());
            }
            report = Some(value);
        } else if let Some(oer_hil_protocol::wifi::AccessPointStopped(_)) = message.decode() {
            stopped = true;
        }
    }
    if report.is_none() || !stopped {
        return Err("AP stop is missing correlated airtime completeness evidence".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
