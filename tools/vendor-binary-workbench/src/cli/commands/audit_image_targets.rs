//! Final-ELF direct control-flow target audit command.

use super::super::*;
use crate::direct_target_audit::{ForbiddenTargetRange, audit_direct_targets};

fn parse_range(value: &str) -> Result<ForbiddenTargetRange> {
    let (name, bounds) = value
        .split_once('=')
        .ok_or("--forbid requires NAME=START..END")?;
    let (start, end) = bounds
        .split_once("..")
        .ok_or("--forbid requires NAME=START..END")?;
    if name.is_empty() {
        return Err("forbidden range name cannot be empty".into());
    }
    let start = parse_u32(start).ok_or("invalid forbidden range start")?;
    let end = parse_u32(end).ok_or("invalid forbidden range end")?;
    if start >= end {
        return Err("forbidden range must be non-empty and ordered".into());
    }
    Ok(ForbiddenTargetRange {
        name: name.to_owned(),
        start,
        end,
    })
}

pub(super) fn run(arguments: ImageAuditArgs) -> Result<bool> {
    let artifact = arguments.artifact.ok_or("missing --artifact")?;
    let ranges = arguments
        .forbid
        .iter()
        .map(|range| parse_range(range))
        .collect::<Result<Vec<_>>>()?;
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
