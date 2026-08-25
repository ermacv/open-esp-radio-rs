//! Project view of the editable register model and optional discovery facts.

use std::collections::BTreeSet;

use super::{
    RegisterFacts, RegisterModel, SvdExportSummary, observation_is_reviewed,
    physical_register_is_observed,
};
use crate::{Result, project::RegisterWorkspacePaths};

#[derive(Default)]
pub(crate) struct RegisterIdentityMaps {
    pub(crate) identities: std::collections::BTreeMap<(u64, u32), String>,
    pub(crate) reviewed: std::collections::BTreeMap<(u64, u32), String>,
    pub(crate) annotations: std::collections::BTreeMap<(u64, u32), super::ReviewAnnotation>,
}

pub(crate) fn register_identity_maps(model: &RegisterModel) -> Result<RegisterIdentityMaps> {
    let projections = model.register_projections()?;
    let mut identities = std::collections::BTreeMap::new();
    let mut reviewed = std::collections::BTreeMap::new();
    let mut annotations = std::collections::BTreeMap::new();
    for (key, projection) in projections {
        if let Some(annotation) = projection.review {
            reviewed.insert(key, projection.identity.clone());
            annotations.insert(key, annotation);
        }
        identities.insert(key, projection.identity);
    }
    for assertion in model
        .reviewed_register_facts()
        .iter()
        .filter(|assertion| assertion.kind == "register-identity")
    {
        let open_radio_vendor_contracts::SemanticEntityId::Register { address, width, .. } =
            &assertion.subject
        else {
            continue;
        };
        if let Some(name) = identities.get(&(*address, *width)) {
            reviewed.insert((*address, *width), name.clone());
        }
    }
    Ok(RegisterIdentityMaps {
        identities,
        reviewed,
        annotations,
    })
}

pub(crate) fn reviewed_register_identities(
    model: &RegisterModel,
) -> Result<std::collections::BTreeMap<(u64, u32), String>> {
    Ok(register_identity_maps(model)?.reviewed)
}

/// Load reusable chip register geometry and apply only the sparse facts
/// selected by the project. Project-aware consumers use this function so
/// inspection, validation and publication all see the same effective model.
pub(crate) fn load_effective_register_model(
    paths: &RegisterWorkspacePaths,
) -> Result<RegisterModel> {
    let mut model = RegisterModel::load(&paths.model)?;
    if !paths.review_context.chips.is_empty()
        && !matches!(paths.review_context.chips.as_slice(), [chip] if chip == model.chip())
    {
        return Err(crate::Error::invalid(format!(
            "register model chip {:?} does not match the active project chips {:?}",
            model.chip(),
            paths.review_context.chips
        )));
    }
    if !paths.reviewed_knowledge.is_empty() {
        let knowledge =
            open_radio_vendor_review::ReviewKnowledge::load_all(&paths.reviewed_knowledge)
                .and_then(|knowledge| knowledge.select_for(&paths.review_context))
                .map_err(|error| {
                    crate::Error::invalid(format!(
                        "cannot compose reviewed knowledge over register model: {error}"
                    ))
                })?;
        model.apply_review_knowledge(&knowledge)?;
    }
    Ok(model)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspaceSummary {
    pub(crate) ranges: usize,
    pub(crate) observed: usize,
    pub(crate) reviewed: usize,
    pub(crate) ignored: usize,
    pub(crate) non_operational: usize,
    pub(crate) manual: usize,
    pub(crate) unreviewed: usize,
    pub(crate) fields: usize,
}

/// Project publication ownership for one complete physical MMIO access.
///
/// Discovery deliberately retains observations from platform/system ranges.
/// Those observations remain useful evidence, but only an access wholly
/// contained by a range selected in `[registers].owned-ranges` may contribute
/// register assertions to project-owned reviewed knowledge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegisterPublicationOwnership<'a> {
    Owned(&'a super::FactRange),
    External(&'a super::FactRange),
}

impl RegisterPublicationOwnership<'_> {
    pub(crate) const fn is_owned(self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

/// Classify whether a complete MMIO access is authorized for publication by
/// the project register workspace.
///
/// Missing/stale range configuration and accesses that do not fit wholly in a
/// single discovery range fail closed. A valid external range is not an error:
/// it is retained for inspection without becoming project publication debt.
pub(crate) fn classify_register_publication<'a>(
    facts: &'a RegisterFacts,
    owned_ranges: &[String],
    address: u32,
    width: u8,
) -> Result<RegisterPublicationOwnership<'a>> {
    let available = facts
        .ranges
        .iter()
        .map(|range| range.name.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(missing) = owned_ranges
        .iter()
        .find(|name| !available.contains(name.as_str()))
    {
        return Err(crate::Error::invalid(format!(
            "register owned range {missing:?} is absent from MMIO discovery facts"
        )));
    }
    if !matches!(width, 8 | 16 | 32) {
        return Err(crate::Error::invalid(format!(
            "register publication candidate at {address:#010x} has unsupported width {width}"
        )));
    }
    let end = u64::from(address)
        .checked_add(u64::from(width).div_ceil(8))
        .ok_or_else(|| crate::Error::invalid("register publication candidate address overflow"))?;
    let range = facts
        .ranges
        .iter()
        .find(|range| range.contains(address) && end <= u64::from(range.end))
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "register publication candidate {address:#010x}/{width} does not fit wholly in one MMIO discovery range"
            ))
        })?;
    Ok(if owned_ranges.contains(&range.name) {
        RegisterPublicationOwnership::Owned(range)
    } else {
        RegisterPublicationOwnership::External(range)
    })
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectRegisterWorkspace {
    facts: Option<RegisterFacts>,
    model: Box<RegisterModel>,
    owned_ranges: BTreeSet<String>,
    non_operational_functions: BTreeSet<String>,
}

impl ProjectRegisterWorkspace {
    pub(crate) fn load(paths: &RegisterWorkspacePaths) -> Result<Self> {
        if !RegisterModel::is_model_file(&paths.model)? {
            return Err(crate::Error::invalid(format!(
                "register workspace {} is not a register-model-v3 manifest",
                paths.model.display()
            )));
        }
        let facts = paths
            .facts
            .is_file()
            .then(|| RegisterFacts::load(&paths.facts))
            .transpose()?;
        Ok(Self {
            facts,
            model: Box::new(load_effective_register_model(paths)?),
            owned_ranges: paths.owned_ranges.iter().cloned().collect(),
            non_operational_functions: paths.non_operational_functions.iter().cloned().collect(),
        })
    }

    pub(crate) fn required_facts(&self) -> Result<&RegisterFacts> {
        self.facts
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("register research requires MMIO discovery facts"))
    }

    pub(crate) fn model(&self) -> &RegisterModel {
        &self.model
    }

    pub(crate) fn summary(&self) -> Result<RegisterWorkspaceSummary> {
        let identities = self.model.register_identities()?;
        let reviewed_identities = reviewed_register_identities(&self.model)?;
        let observations = self.facts.as_ref().map_or_else(
            || Ok(ObservationKeys::default()),
            |facts| observation_keys(facts, &self.owned_ranges, &self.non_operational_functions),
        )?;
        let fact_keys = observations.all();
        let model_keys = identities.keys().copied().collect::<BTreeSet<_>>();
        if let Some(facts) = &self.facts
            && let Some((address, width)) = model_keys.iter().find(|(address, width)| {
                let Ok(address) = u32::try_from(*address) else {
                    return true;
                };
                let byte_width = u64::from(*width).div_ceil(8);
                let end = u64::from(address).saturating_add(byte_width);
                !facts.ranges.iter().any(|range| {
                    self.owned_ranges.contains(&range.name)
                        && range.contains(address)
                        && end <= u64::from(range.end)
                })
            })
        {
            return Err(crate::Error::invalid(format!(
                "register model entry at {address:#010x}/{width} lies outside [registers].owned-ranges"
            )));
        }
        Ok(RegisterWorkspaceSummary {
            ranges: self.facts.as_ref().map_or(0, |facts| facts.ranges.len()),
            observed: fact_keys.len(),
            reviewed: observations
                .owned
                .iter()
                .filter(|observation| observation_is_reviewed(&reviewed_identities, observation))
                .count(),
            ignored: observations.outside_scope.len(),
            non_operational: observations
                .non_operational
                .iter()
                .filter(|observation| !observation_is_reviewed(&reviewed_identities, observation))
                .count(),
            manual: model_keys
                .iter()
                .filter(|register| !physical_register_is_observed(register, &fact_keys))
                .count(),
            unreviewed: observations
                .owned
                .iter()
                .filter(|observation| !observation_is_reviewed(&reviewed_identities, observation))
                .filter(|key| !observations.non_operational.contains(key))
                .count(),
            fields: self.model.render_svd()?.1.fields,
        })
    }

    pub(crate) fn unreviewed_in_mmio_scope(&self, scope: &BTreeSet<(u32, u8)>) -> Result<usize> {
        Ok(self.unreviewed_mmio_in_scope(scope)?.len())
    }

    pub(crate) fn unreviewed_mmio_in_scope(
        &self,
        scope: &BTreeSet<(u32, u8)>,
    ) -> Result<BTreeSet<(u32, u8)>> {
        if scope.is_empty() {
            return Ok(BTreeSet::new());
        }
        let reviewed_identities = reviewed_register_identities(&self.model)?;
        let facts = self.facts.as_ref().ok_or_else(|| {
            crate::Error::invalid(
                "release-scoped register validation requires MMIO discovery facts",
            )
        })?;
        validate_non_operational_functions(facts, &self.non_operational_functions)?;
        let mut unreviewed = BTreeSet::new();
        for &(address, width) in scope {
            let byte_width = u64::from(width).div_ceil(8);
            let end = u64::from(address).saturating_add(byte_width);
            let Some(range) = facts
                .ranges
                .iter()
                .find(|range| range.contains(address) && end <= u64::from(range.end))
            else {
                return Err(crate::Error::invalid(format!(
                    "publication scope MMIO {address:#010x}/{width} lies outside discovery ranges"
                )));
            };
            if !self.owned_ranges.contains(&range.name) {
                continue;
            }
            let key = (u64::from(address), u32::from(width));
            if observation_is_reviewed(&reviewed_identities, &key) {
                continue;
            }
            let non_operational = facts
                .registers
                .iter()
                .find(|fact| fact.address == address && fact.width == width)
                .is_some_and(|fact| fact_is_non_operational(fact, &self.non_operational_functions));
            if !non_operational {
                unreviewed.insert((address, width));
            }
        }
        Ok(unreviewed)
    }

    pub(crate) fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        Ok(self.model.render_svd()?)
    }

    pub(crate) const fn format_label(&self) -> &'static str {
        "register-model-v3"
    }
}

#[derive(Debug, Default)]
struct ObservationKeys {
    owned: BTreeSet<(u64, u32)>,
    outside_scope: BTreeSet<(u64, u32)>,
    non_operational: BTreeSet<(u64, u32)>,
}

impl ObservationKeys {
    fn all(&self) -> BTreeSet<(u64, u32)> {
        self.owned.union(&self.outside_scope).copied().collect()
    }
}

fn observation_keys(
    facts: &RegisterFacts,
    owned_ranges: &BTreeSet<String>,
    non_operational_functions: &BTreeSet<String>,
) -> Result<ObservationKeys> {
    let available = facts
        .ranges
        .iter()
        .map(|range| range.name.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(missing) = owned_ranges
        .iter()
        .find(|name| !available.contains(name.as_str()))
    {
        return Err(crate::Error::invalid(format!(
            "register owned range {missing:?} is absent from MMIO discovery facts"
        )));
    }
    validate_non_operational_functions(facts, non_operational_functions)?;
    let mut observations = ObservationKeys::default();
    for fact in &facts.registers {
        let key = (u64::from(fact.address), u32::from(fact.width));
        let range = facts
            .ranges
            .iter()
            .find(|range| range.contains(fact.address))
            .expect("validated register facts have exactly one owning range");
        if owned_ranges.contains(&range.name) {
            observations.owned.insert(key);
            if fact_is_non_operational(fact, non_operational_functions) {
                observations.non_operational.insert(key);
            }
        } else {
            observations.outside_scope.insert(key);
        }
    }
    Ok(observations)
}

pub(crate) fn fact_is_non_operational(
    fact: &super::RegisterFact,
    non_operational_functions: &BTreeSet<String>,
) -> bool {
    let mut functions = fact.read_functions.iter().chain(&fact.write_functions);
    let Some(first) = functions.next() else {
        return false;
    };
    non_operational_functions.contains(first)
        && functions.all(|function| non_operational_functions.contains(function))
}

pub(crate) fn validate_non_operational_functions(
    facts: &RegisterFacts,
    non_operational_functions: &BTreeSet<String>,
) -> Result<()> {
    let observed = facts
        .registers
        .iter()
        .flat_map(|fact| fact.read_functions.iter().chain(&fact.write_functions))
        .collect::<BTreeSet<_>>();
    if let Some(function) = non_operational_functions
        .iter()
        .find(|function| !observed.contains(function))
    {
        return Err(crate::Error::invalid(format!(
            "register review non-operational function {function:?} is absent from MMIO discovery facts"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{FactRange, RegisterFact};

    fn fact(address: u32) -> RegisterFact {
        RegisterFact {
            address,
            width: 32,
            catalog_name: "UNMAPPED".to_owned(),
            reads: 1,
            writes: 0,
            read_functions: BTreeSet::new(),
            write_functions: BTreeSet::new(),
            read_sites: BTreeSet::new(),
            write_sites: BTreeSet::new(),
            write_patterns: Vec::new(),
            candidate_masks: Vec::new(),
        }
    }

    #[test]
    fn publication_scope_keeps_external_observations_visible_but_non_blocking() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                FactRange {
                    name: "radio".to_owned(),
                    start: 0x1000,
                    end: 0x2000,
                },
                FactRange {
                    name: "system".to_owned(),
                    start: 0x3000,
                    end: 0x4000,
                },
            ],
            registers: vec![fact(0x1010), fact(0x3010)],
        };
        let observations =
            observation_keys(&facts, &["radio".to_owned()].into(), &BTreeSet::new()).unwrap();
        assert_eq!(observations.owned, [(0x1010, 32)].into());
        assert_eq!(observations.outside_scope, [(0x3010, 32)].into());
    }

    #[test]
    fn publication_ownership_distinguishes_radio_and_platform_ranges() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                FactRange {
                    name: "coex-hw-timer".to_owned(),
                    start: 0x2010_f400,
                    end: 0x2010_f450,
                },
                FactRange {
                    name: "modem-lpcon-platform-high".to_owned(),
                    start: 0x2010_f450,
                    end: 0x2010_f800,
                },
                FactRange {
                    name: "lp-peripheral".to_owned(),
                    start: 0x2080_0000,
                    end: 0x2090_0000,
                },
            ],
            registers: Vec::new(),
        };
        let owned = vec!["coex-hw-timer".to_owned()];

        let owned_control = classify_register_publication(&facts, &owned, 0x2010_f420, 32).unwrap();
        let external_coex = classify_register_publication(&facts, &owned, 0x2010_f4a0, 32).unwrap();
        let external_tsens =
            classify_register_publication(&facts, &owned, 0x2081_8000, 32).unwrap();

        assert_eq!(
            owned_control,
            RegisterPublicationOwnership::Owned(&facts.ranges[0])
        );
        assert_eq!(
            external_coex,
            RegisterPublicationOwnership::External(&facts.ranges[1])
        );
        assert_eq!(
            external_tsens,
            RegisterPublicationOwnership::External(&facts.ranges[2])
        );
    }

    #[test]
    fn publication_ownership_fails_closed_on_crossing_or_stale_ranges() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![FactRange {
                name: "coex-hw-timer".to_owned(),
                start: 0x2010_f400,
                end: 0x2010_f450,
            }],
            registers: Vec::new(),
        };

        let crossing =
            classify_register_publication(&facts, &["coex-hw-timer".to_owned()], 0x2010_f44f, 32)
                .unwrap_err();
        assert!(crossing.to_string().contains("does not fit wholly"));

        let stale = classify_register_publication(
            &facts,
            &["renamed-owned-range".to_owned()],
            0x2010_f420,
            32,
        )
        .unwrap_err();
        assert!(
            stale
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }

    #[test]
    fn publication_scope_rejects_a_range_missing_from_generated_facts() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: Vec::new(),
        };
        let error =
            observation_keys(&facts, &["missing".to_owned()].into(), &BTreeSet::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }

    #[test]
    fn non_operational_policy_excludes_only_exclusive_observations() {
        let mut diagnostic = fact(0x1010);
        diagnostic.read_functions = ["archive:dump".to_owned()].into();
        let mut mixed = fact(0x1020);
        mixed.read_functions = ["archive:dump".to_owned(), "rom:enable".to_owned()].into();
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![diagnostic, mixed],
        };
        let observations = observation_keys(
            &facts,
            &["radio".to_owned()].into(),
            &["archive:dump".to_owned()].into(),
        )
        .unwrap();
        assert_eq!(observations.non_operational, [(0x1010, 32)].into());
        assert!(observations.owned.contains(&(0x1020, 32)));
        assert!(!observations.non_operational.contains(&(0x1020, 32)));
    }

    #[test]
    fn non_operational_policy_rejects_stale_function_names() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![fact(0x1010)],
        };
        let error = observation_keys(
            &facts,
            &["radio".to_owned()].into(),
            &["archive:missing".to_owned()].into(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }

    #[test]
    fn generated_observations_and_sparse_identity_merge_without_inferred_hardware_semantics() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-register-identity-integration-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let model = directory.join("device.toml");
        let fragment = directory.join("radio.toml");
        let facts = directory.join("mmio.json");
        let reviewed = directory.join("reviewed.toml");
        std::fs::write(
            &model,
            r#"schema = 3
chip = "fixture-chip"
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
            r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000
"#,
        )
        .unwrap();
        std::fs::write(
            &facts,
            r#"{
  "schema_version": 5,
  "command": "mmio discover",
  "analysis_mode": "best-effort",
  "access_count_mode": "maximum-per-path",
  "completeness_claim": false,
  "code_selection": {"symbols":"all","symbol_prefix":""},
  "ranges": [{"name":"radio","start":"0x1000","end_exclusive":"0x2000"}],
  "artifacts": [],
  "registers": [{
    "address":"0x1010",
    "width":32,
    "name":"UNMAPPED",
    "reads":1,
    "writes":2,
    "read_functions":["vendor:read_status"],
    "write_functions":["vendor:ack_status"],
    "read_sites":[{"function":"vendor:read_status","pc":"0x00000100"}],
    "write_sites":[{"function":"vendor:ack_status","pc":"0x00000104"}],
    "write_patterns":[{
      "occurrences":2,
      "modified_mask":"0x1",
      "candidate_bit_ranges":"0",
      "preserved_mask":"0xfffffffe",
      "inverted_mask":"0x0",
      "forced_zero_mask":"0x0",
      "forced_one_mask":"0x1",
      "read_derived_mask":"0x0",
      "dynamic_mask":"0x0",
      "functions":["vendor:ack_status"]
    }]
  }],
  "diagnostics": []
}"#,
        )
        .unwrap();
        std::fs::write(
            &reviewed,
            r#"schema = 2
id = "fixture.register-review"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
chips = ["fixture-chip"]

[[assertions]]
id = "fixture.status.identity"
subject = "register:fixture-chip/cpu/0x1010/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "region-and-name"
"#,
        )
        .unwrap();
        let mut paths = RegisterWorkspacePaths {
            facts,
            model,
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
            evidence_catalogs: Vec::new(),
            reviewed_knowledge: vec![reviewed],
            review_context: open_radio_vendor_contracts::ApplicabilityContext {
                chips: vec!["fixture-chip".to_owned()],
                ..open_radio_vendor_contracts::ApplicabilityContext::default()
            },
        };

        paths.review_context.chips = vec!["other-chip".to_owned()];
        let error = ProjectRegisterWorkspace::load(&paths).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match the active project chips")
        );
        paths.review_context.chips = vec!["fixture-chip".to_owned()];
        let workspace = ProjectRegisterWorkspace::load(&paths).unwrap();
        let loaded_facts = workspace.required_facts().unwrap();
        assert!(std::ptr::eq(
            loaded_facts,
            workspace.required_facts().unwrap()
        ));
        assert_eq!(loaded_facts.registers.len(), 1);
        assert_eq!(workspace.model().address_space(), "cpu");
        let without_facts = ProjectRegisterWorkspace {
            facts: None,
            model: workspace.model.clone(),
            owned_ranges: workspace.owned_ranges.clone(),
            non_operational_functions: workspace.non_operational_functions.clone(),
        };
        assert!(
            without_facts
                .required_facts()
                .unwrap_err()
                .to_string()
                .contains("requires MMIO discovery facts")
        );
        let summary = workspace.summary().unwrap();
        let (svd, export) = workspace.render_svd().unwrap();
        let identity = &workspace.model().reviewed_register_facts()[0];
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(summary.observed, 1);
        assert_eq!(summary.reviewed, 1);
        assert_eq!(summary.unreviewed, 0);
        assert_eq!(summary.manual, 0);
        assert_eq!(export.registers, 1);
        assert!(svd.contains("<name>EVENT_STATUS</name>"));
        assert!(!svd.contains("<access>"));
        assert!(!svd.contains("modifiedWriteValues"));
        assert_eq!(identity.id, "fixture.status.identity");
        assert_eq!(identity.metadata.applies_to.chips, ["fixture-chip"]);
    }
}
