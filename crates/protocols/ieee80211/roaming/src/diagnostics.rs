//! WNM diagnostics coordinate existing association/authentication owners and
//! retain reports. They never perform a second association or EAP exchange.
mod requester;
mod station;
pub use crate::reports::{NetworkIdentity, ReportError, ReportWindowEvent, RequestReplacement};
use oer_ieee80211_mac::roaming::*;
pub use requester::*;
pub use station::*;
fn validate_result(
    request: DiagnosticRequest<'_>,
    report: DiagnosticReport<'_>,
) -> Result<(), ReportError> {
    report.validate()?;
    if report.token != request.token {
        return Err(ReportError::UnexpectedToken(report.token));
    }
    if report.kind != request.kind {
        return Err(ReportError::UnexpectedType(report.kind.0));
    }
    if !report.status.is_known() {
        return Err(ReportError::UnsupportedStatus(report.status.0));
    }
    if report.status == DiagnosticReportStatus::SUCCESSFUL {
        if report.kind == DiagnosticType::CONFIGURATION
            && report
                .information
                .unique(diagnostic_information_id::PROFILE_ID)?
                .is_none()
        {
            return Err(ReportError::InvalidResult);
        }
        if report.kind.active() {
            let target = report
                .information
                .unique(diagnostic_information_id::AP_DESCRIPTOR)?
                .ok_or(ReportError::InvalidResult)?;
            if Some(DiagnosticAp::parse(target)?) != request.target()?
                || report
                    .information
                    .unique(diagnostic_information_id::STATUS_CODE)?
                    .is_none()
            {
                return Err(ReportError::InvalidResult);
            }
            if report.kind == DiagnosticType::IEEE8021X {
                for id in [
                    diagnostic_information_id::EAP_METHOD,
                    diagnostic_information_id::CREDENTIAL_TYPES,
                ] {
                    if report.information.unique(id)?.is_none() {
                        return Err(ReportError::InvalidResult);
                    }
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
