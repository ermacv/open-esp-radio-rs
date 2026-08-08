//! Final-ELF direct control-flow target audit command.

use super::super::*;
use crate::direct_target_audit::{ForbiddenTargetRange, audit_direct_targets};

pub(super) fn run(arguments: ImageAuditArgs) -> Result<bool> {
    let artifact = arguments.artifact.ok_or("missing --artifact")?;
    let ranges = arguments
        .forbid
        .into_iter()
        .map(|range| ForbiddenTargetRange {
            name: range.name,
            start: range.start,
            end: range.end,
        })
        .collect::<Vec<_>>();
    let audit = audit_direct_targets(&artifact, &ranges)?;
    outputln!(
        "DIRECT-TARGET-AUDIT\tartifact={}\texecutable-sections={}\texecutable-bytes={}\tdecoded-instructions={}\tunsupported-non-control={}\tforbidden-targets={}",
        artifact.display(),
        audit.executable_sections,
        audit.executable_bytes,
        audit.decoded_instructions,
        audit.unsupported_instructions,
        audit.forbidden_targets.len(),
    );
    for finding in &audit.forbidden_targets {
        outputln!(
            "FORBIDDEN-DIRECT-TARGET\trange={}\tsection={}\tsite={:#010x}\ttarget={:#010x}",
            finding.range,
            finding.section,
            finding.site,
            finding.target,
        );
    }
    Ok(audit.forbidden_targets.is_empty())
}
