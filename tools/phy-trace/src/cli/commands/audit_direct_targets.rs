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

pub(super) fn run(filtered: Vec<String>) -> Result<bool> {
    let mut artifact = None;
    let mut ranges = Vec::new();
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--forbid" => ranges.push(parse_range(&take_value(&mut arguments, "--forbid")?)?),
            _ => return Err(format!("unknown audit-direct-targets option: {argument}").into()),
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let audit = audit_direct_targets(&artifact, &ranges)?;
    println!(
        "DIRECT-TARGET-AUDIT\tartifact={}\texecutable-sections={}\texecutable-bytes={}\tdecoded-instructions={}\tunsupported-non-control={}\tforbidden-targets={}",
        artifact.display(),
        audit.executable_sections,
        audit.executable_bytes,
        audit.decoded_instructions,
        audit.unsupported_instructions,
        audit.forbidden_targets.len(),
    );
    for finding in &audit.forbidden_targets {
        println!(
            "FORBIDDEN-DIRECT-TARGET\trange={}\tsection={}\tsite={:#010x}\ttarget={:#010x}",
            finding.range, finding.section, finding.site, finding.target,
        );
    }
    Ok(audit.forbidden_targets.is_empty())
}
