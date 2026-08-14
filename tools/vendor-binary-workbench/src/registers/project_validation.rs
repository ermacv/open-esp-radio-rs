//! Shared validation of project-owned register API, lint and evidence packs.

use crate::{
    MemoryMap, Result,
    project::RegisterWorkspacePaths,
    registers::{PacApiPack, RegisterEvidenceSet, RegisterLintPack, RegisterModel},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterMemoryMapSummary {
    pub(crate) registers: usize,
    pub(crate) mmio_regions: usize,
}

pub(crate) fn validate_register_memory_map(
    paths: &RegisterWorkspacePaths,
    memory_map: Option<&MemoryMap>,
) -> Result<Option<RegisterMemoryMapSummary>> {
    let Some(memory) = memory_map else {
        return Ok(None);
    };
    if !RegisterModel::is_model_file(&paths.model)? {
        return Ok(None);
    }
    let model = RegisterModel::load(&paths.model)?;
    let summary = validate_register_model_memory_map(&model, memory)?;
    let mmio = memory.mmio_ranges()?;
    for name in &paths.owned_ranges {
        if !mmio.iter().any(|(candidate, _, _)| candidate == name) {
            return Err(crate::Error::invalid(format!(
                "register owned range {name:?} is not a project MMIO region"
            )));
        }
    }
    let identities = model.register_identities()?;
    if let Some(((address, width), identity)) = identities.iter().find(|((address, width), _)| {
        let end = address.saturating_add(u64::from(*width).div_ceil(8));
        !mmio.iter().any(|(name, start, range_end)| {
            paths.owned_ranges.contains(name)
                && u64::from(*start) <= *address
                && end <= u64::from(*range_end)
        })
    }) {
        return Err(crate::Error::invalid(format!(
            "register {identity:?} at {address:#010x}/{width} lies outside [registers].owned-ranges"
        )));
    }
    Ok(Some(summary))
}

pub(crate) fn validate_register_model_memory_map(
    model: &RegisterModel,
    memory: &MemoryMap,
) -> Result<RegisterMemoryMapSummary> {
    let mmio = memory.mmio_ranges()?;
    let identities = model.register_identities()?;
    if let Some(((address, width), identity)) = identities.iter().find(|((address, width), _)| {
        let end = address.saturating_add(u64::from(*width).div_ceil(8));
        !mmio.iter().any(|(_, start, range_end)| {
            u64::from(*start) <= *address && end <= u64::from(*range_end)
        })
    }) {
        return Err(crate::Error::invalid(format!(
            "register {identity:?} at {address:#010x}/{width} lies outside project MMIO regions"
        )));
    }
    Ok(RegisterMemoryMapSummary {
        registers: identities.len(),
        mmio_regions: mmio.len(),
    })
}

pub(crate) fn validate_pac_api(paths: &RegisterWorkspacePaths) -> Result<Option<PacApiPack>> {
    let Some(path) = &paths.api_pack else {
        return Ok(None);
    };
    let pack = PacApiPack::load(path)?;
    let model = RegisterModel::load(&paths.model)?;
    let (svd, _) = model.render_svd()?;
    pack.validate_against_svd(&svd)?;
    Ok(Some(pack))
}

pub(crate) fn validate_register_lints(
    paths: &RegisterWorkspacePaths,
) -> Result<Option<RegisterLintPack>> {
    let Some(path) = &paths.lint_pack else {
        return Ok(None);
    };
    let pack = RegisterLintPack::load(path)?;
    let model = RegisterModel::load(&paths.model)?;
    model.validate_lints(&pack)?;
    Ok(Some(pack))
}

pub(crate) fn validate_register_evidence(
    paths: &RegisterWorkspacePaths,
    memory_map: Option<&MemoryMap>,
) -> Result<Option<RegisterEvidenceSet>> {
    if paths.evidence_catalogs.is_empty() {
        return Ok(None);
    }
    let evidence = RegisterEvidenceSet::load_all(&paths.evidence_catalogs)?;
    let model = RegisterModel::load(&paths.model)?;
    evidence.validate_references(
        "register model review",
        model
            .review()
            .iter()
            .flat_map(|annotation| annotation.sources.iter().map(String::as_str)),
    )?;
    if let Some(path) = &paths.api_pack {
        let api = PacApiPack::load(path)?;
        evidence.validate_references("PAC API pack", api.source_ids())?;
    }

    let memory = memory_map
        .ok_or("register evidence ranges require a project or target memory-map")
        .map_err(crate::Error::invalid)?;
    let mmio = memory.mmio_ranges()?;
    if let Some(range) = evidence.ranges.iter().find(|range| {
        !mmio.iter().any(|(_, start, end)| {
            u64::from(*start) <= range.start && range.end_exclusive <= u64::from(*end)
        })
    }) {
        return Err(crate::Error::invalid(format!(
            "register evidence range {:?} lies outside project MMIO regions",
            range.name
        )));
    }
    Ok(Some(evidence))
}
