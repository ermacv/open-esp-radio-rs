//! Last aggregate verification-result currency inspection.

use crate::{
    application::status::model::{DetailValue, Readiness},
    application::{ProjectContext, status},
};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let component = status::verification_report_status(context);
    let mut capability = CapabilityReport::new("verification-report", component.status.label());
    if let Some(DetailValue::String(path)) = component.details.get("path") {
        capability = capability.field("path", path.clone());
    }
    if let Some(DetailValue::Bool(fresh)) = component.details.get("fresh") {
        capability = capability.field("fresh", *fresh);
    }
    if let Some(DetailValue::Bool(passed)) = component.details.get("passed") {
        capability = capability.field("passed", *passed);
    }
    if let Some(DetailValue::Unsigned(checked)) = component.details.get("checked_inputs") {
        capability = capability.field("checked-inputs", *checked);
    }
    if let Some(diagnostic) = component.diagnostic.as_deref() {
        capability = capability.field("diagnostic", diagnostic);
    }
    match component.status {
        Readiness::Invalid => report.error(),
        Readiness::Incomplete => report.warning(
            component
                .diagnostic
                .unwrap_or_else(|| "verification report is incomplete".to_owned()),
        ),
        Readiness::Ready | Readiness::NotConfigured => {}
    }
    report.capability(capability);
}
