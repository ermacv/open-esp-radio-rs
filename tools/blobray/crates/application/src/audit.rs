//! Ephemeral final-image audit: owned read, no project writer or durable result.
use crate::*;
use std::io::Read;
pub(crate) fn audit_targets(
    artifact: &OriginPath,
    ranges: &[ForbiddenTargetRange],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut impl FnMut(&TargetAuditRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<(ArtifactId, TargetAuditSummary)> {
    if ranges.is_empty() || ranges.len() > 64 {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "audit requires 1..64 forbidden ranges",
        ));
    }
    for range in ranges {
        range.validate()?;
    }
    control.set_position(RunPosition {
        phase: RunPhase::Elf,
        ..Default::default()
    });
    control.checkpoint(0)?;
    let file = std::fs::File::open(artifact.to_path()?).map_err(storage_io)?;
    let metadata = file.metadata().map_err(storage_io)?;
    if !metadata.is_file() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "audit input is not a regular file",
        ));
    }
    let mut bytes = memory.bytes(
        usize::try_from(metadata.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "ELF exceeds address space"))?,
        control.position(),
    )?;
    let mut file = file;
    for chunk in bytes.chunks_mut(WORK_BLOCK) {
        control.bytes(chunk.len())?;
        file.read_exact(chunk).map_err(storage_io)?;
    }
    let after = file.metadata().map_err(storage_io)?;
    if after.len() != metadata.len() || after.modified().ok() != metadata.modified().ok() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "audit input changed during capture",
        ));
    }
    let id = ArtifactId::of_bytes_controlled(&bytes, control)?;
    let mut position = RunPosition {
        phase: RunPhase::AnalyzeFunction,
        ..Default::default()
    };
    position.artifact(&id);
    control.set_position(position);
    let mut total = TargetAuditSummary::default();
    blobray_artifacts::executable_sections(&bytes, memory, control, &mut |section, c| {
        let part = blobray_analysis::audit::audit_section(section, decoder, ranges, c, emit)?;
        total.sections += part.sections;
        total.executable_bytes += part.executable_bytes;
        total.embedded_data_bytes += part.embedded_data_bytes;
        total.instructions += part.instructions;
        total.unsupported_non_control += part.unsupported_non_control;
        total.unresolved_indirect += part.unresolved_indirect;
        total.coverage_gaps += part.coverage_gaps;
        total.forbidden_targets += part.forbidden_targets;
        Ok(())
    })?;
    Ok((id, total))
}
