//! Shared validation of project-owned register API, lint and evidence packs.

use crate::{
    MemoryMap, Result,
    project::RegisterWorkspacePaths,
    registers::{
        PacApiPack, RegisterEvidenceSet, RegisterLintPack, RegisterModel,
        load_effective_register_model,
    },
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
    let model = load_effective_register_model(paths)?;
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
    let model = load_effective_register_model(paths)?;
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
    let model = load_effective_register_model(paths)?;
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
    let model = load_effective_register_model(paths)?;
    evidence.validate_references(
        "register model review",
        model
            .review()
            .iter()
            .flat_map(|annotation| annotation.sources.iter().map(String::as_str))
            .chain(
                model
                    .reviewed_register_identities()
                    .iter()
                    .flat_map(|identity| {
                        identity
                            .assertion
                            .evidence
                            .iter()
                            .map(|reference| reference.source.as_str())
                    }),
            ),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn retained_identity_evidence_is_validated_for_existing_and_absent_registers() {
        for (case, address, register) in [
            (
                "existing",
                0x1000_u64,
                r#"
[[peripherals.registers]]
[peripherals.registers.register]
name = "STATUS"
addressOffset = 0
size = 32
access = "read-write"
"#,
            ),
            ("absent", 0x1004_u64, ""),
        ] {
            assert_identity_evidence_is_required(case, address, register);
        }
    }

    fn assert_identity_evidence_is_required(case: &str, address: u64, register: &str) {
        let directory = std::env::temp_dir().join(format!(
            "blobray-register-identity-evidence-{case}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let model = directory.join("device.toml");
        let fragment = directory.join("radio.toml");
        let reviewed = directory.join("reviewed.toml");
        let catalog = directory.join("evidence.toml");
        let memory_map = directory.join("memory-map.toml");
        std::fs::write(
            &model,
            r#"schema = 2
address-space = "cpu"
fragments = ["radio.toml"]

[device]
name = "fixture"
version = "1"
description = "fixture"
address-unit-bits = 8
width = 32
"#,
        )
        .unwrap();
        std::fs::write(
            &fragment,
            format!(
                r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000
{register}"#
            ),
        )
        .unwrap();
        std::fs::write(
            &reviewed,
            format!(
                r#"schema = 1
id = "fixture.identity"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[[assertions]]
id = "fixture.identity.{case}"
subject = "mmio:cpu:{address:#x}/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "IDENTITY_SOURCE"
locator = "manual"
"#
            ),
        )
        .unwrap();
        std::fs::write(&catalog, "schema = 1\n").unwrap();
        std::fs::write(
            &memory_map,
            r#"schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[regions]]
name = "radio"
address-space = "cpu"
kind = "mmio"
start = 0x1000
end-exclusive = 0x2000
permissions = "rw"
"#,
        )
        .unwrap();
        let paths = register_paths(&model, &reviewed, &catalog);
        let memory = MemoryMap::load(&memory_map).unwrap();

        let error = validate_register_evidence(&paths, Some(&memory)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("undefined evidence source \"IDENTITY_SOURCE\""),
            "{case}: {error}"
        );

        std::fs::write(
            &catalog,
            r#"schema = 1

[[sources]]
id = "IDENTITY_SOURCE"
description = "manually reviewed register identity"
"#,
        )
        .unwrap();
        validate_register_evidence(&paths, Some(&memory)).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn register_paths(model: &Path, reviewed: &Path, catalog: &Path) -> RegisterWorkspacePaths {
        RegisterWorkspacePaths {
            facts: model.with_file_name("unused-mmio.json"),
            model: model.to_owned(),
            owned_ranges: vec!["radio".to_owned()],
            non_operational_functions: Vec::new(),
            review_output: None,
            review_ir_reports: Vec::new(),
            svd_output: None,
            pac_raw: None,
            bindings: None,
            api_pack: None,
            api_output: None,
            lint_pack: None,
            evidence_catalogs: vec![catalog.to_owned()],
            reviewed_knowledge: vec![reviewed.to_owned()],
            review_context: open_radio_vendor_review::ApplicabilityContext::default(),
        }
    }
}
