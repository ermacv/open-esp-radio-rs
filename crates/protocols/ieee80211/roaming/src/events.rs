//! Event logging belongs to the ESS/IBSS; report dialogs belong to an associated
//! peer. Retire dialogs at a BSS transition and retain the journal in that ESS.
mod journal;
mod requester;
mod station;
pub use crate::reports::{NetworkIdentity, ReportError, ReportWindowEvent, RequestReplacement};
pub use journal::*;
pub use requester::*;
pub use station::*;

use oer_ieee80211_mac::roaming::*;
/// Apply every recognized condition; the standard requires ignoring unknown
/// request subelements. Frequent-transition counters are handled separately.
fn matches(request: EventRequest<'_>, report: EventReport<'_>) -> Result<bool, ReportError> {
    if request.kind != report.kind {
        return Ok(false);
    }
    for condition in request.typed_conditions()? {
        let allowed = match (report.data, condition) {
            (EventData::Transition(v), EventCondition::TransitionTarget(target)) => {
                v.target == target
            }
            (EventData::Transition(v), EventCondition::TransitionSource(source)) => {
                v.source == source
            }
            (EventData::Transition(v), EventCondition::MinimumTransitionTime(time)) => {
                v.time_tu >= time
            }
            (EventData::Transition(v), EventCondition::TransitionOutcome(outcome)) => {
                outcome.allows(v.result == EventResultCode::SUCCESS)
            }
            (EventData::Rsna(v), EventCondition::RsnaTarget(target)) => v.target == target,
            (EventData::Rsna(v), EventCondition::RsnaAkm(akm)) => v.authentication == akm,
            (EventData::Rsna(v), EventCondition::RsnaEap(eap)) => v.eap == eap,
            (EventData::Rsna(v), EventCondition::RsnaOutcome(outcome)) => {
                outcome.allows(v.result == EventResultCode::SUCCESS)
            }
            (EventData::PeerLink(v), EventCondition::PeerAddress(peer)) => v.peer == peer,
            (EventData::PeerLink(v), EventCondition::PeerChannel(channel)) => {
                channel.matches(v.operating_class, v.channel)
            }
            _ => true,
        };
        if !allowed {
            return Ok(false);
        }
    }
    Ok(true)
}
#[cfg(test)]
mod tests;
