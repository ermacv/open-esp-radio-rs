//! Fail-closed qualification of user-visible driver features.
//!
//! Review scopes remain useful publication and navigation surfaces. A feature
//! gate is stricter: every configured analysis closure must be complete and
//! every named proof must establish the requested claim strength.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use open_radio_vendor_semantics::DriverAdapterClaim;
use serde::{Deserialize, Serialize};

use crate::{Result, project::ProjectSpec, verification::FunctionVerificationStatus};

pub(crate) const FEATURE_PACK_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeaturePack {
    schema: u32,
    features: Vec<FeatureSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeatureSpec {
    id: String,
    description: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    requirements: Vec<FeatureRequirement>,
    #[serde(default)]
    effects: Vec<FeatureEffectDisposition>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureEffectDisposition {
    id: String,
    source: String,
    symbol: String,
    disposition: FeatureEffectDispositionKind,
    requirement: Option<String>,
    rationale: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FeatureEffectDispositionKind {
    Verified,
    ExcludedByFeaturePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureRequirement {
    id: String,
    description: String,
    suite: String,
    source: String,
    symbol: String,
    claim: DriverAdapterClaim,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FeatureQualificationStatus {
    Qualified,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeatureQualificationReport {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) required: bool,
    pub(crate) status: FeatureQualificationStatus,
    pub(crate) scopes: Vec<String>,
    pub(crate) requirements: usize,
    pub(crate) scope_effects: usize,
    pub(crate) covered_effects: usize,
    pub(crate) blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StoredSuiteReport {
    id: String,
    sources: Vec<StoredSourceReport>,
}

#[derive(Debug, Deserialize)]
struct StoredSourceReport {
    source: String,
    functions: Vec<StoredFunctionReport>,
}

#[derive(Debug, Deserialize)]
struct StoredFunctionReport {
    vendor_symbol: String,
    status: FunctionVerificationStatus,
    claim: Option<DriverAdapterClaim>,
    disposition_reviewed: bool,
    rust_component: Option<String>,
}

impl FeaturePack {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot read qualification feature pack {}: {error}",
                path.display()
            ))
        })?;
        let pack: Self = toml_edit::de::from_str(&input).map_err(|error| {
            crate::Error::invalid(format!(
                "invalid qualification feature pack {}: {error}",
                path.display()
            ))
        })?;
        pack.validate(path)?;
        Ok(pack)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema != FEATURE_PACK_SCHEMA {
            return Err(crate::Error::invalid(format!(
                "qualification feature pack {} requires schema = {FEATURE_PACK_SCHEMA}",
                path.display()
            )));
        }
        if self.features.is_empty() {
            return Err(crate::Error::invalid(format!(
                "qualification feature pack {} has no features",
                path.display()
            )));
        }
        let mut feature_ids = BTreeSet::new();
        for feature in &self.features {
            validate_id(&feature.id, "feature")?;
            if feature.description.trim().is_empty() {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {:?} has an empty description",
                    feature.id
                )));
            }
            if !feature_ids.insert(&feature.id) {
                return Err(crate::Error::invalid(format!(
                    "duplicate qualification feature {:?}",
                    feature.id
                )));
            }
            let mut scopes = BTreeSet::new();
            for scope in &feature.scopes {
                validate_id(scope, "feature scope")?;
                if !scopes.insert(scope) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats scope {scope:?}",
                        feature.id
                    )));
                }
            }
            let mut requirement_ids = BTreeSet::new();
            let mut requirement_selectors = BTreeSet::new();
            for requirement in &feature.requirements {
                validate_id(&requirement.id, "feature requirement")?;
                if requirement.description.trim().is_empty()
                    || requirement.suite.trim().is_empty()
                    || requirement.source.trim().is_empty()
                    || requirement.symbol.trim().is_empty()
                {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} has an incomplete requirement {:?}",
                        feature.id, requirement.id
                    )));
                }
                if !requirement_ids.insert(requirement.id.as_str()) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats requirement id {:?}",
                        feature.id, requirement.id
                    )));
                }
                if !requirement_selectors.insert((
                    &requirement.suite,
                    &requirement.source,
                    &requirement.symbol,
                )) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats proof selector {}:{}:{}",
                        feature.id, requirement.suite, requirement.source, requirement.symbol
                    )));
                }
            }
            let mut effect_ids = BTreeSet::new();
            let mut effect_selectors = BTreeSet::new();
            for effect in &feature.effects {
                validate_id(&effect.id, "feature effect")?;
                if effect.source.trim().is_empty()
                    || effect.symbol.trim().is_empty()
                    || effect.rationale.trim().is_empty()
                {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} has an incomplete effect disposition {:?}",
                        feature.id, effect.id
                    )));
                }
                if !effect_ids.insert(effect.id.as_str()) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats effect id {:?}",
                        feature.id, effect.id
                    )));
                }
                if !effect_selectors.insert((&effect.source, &effect.symbol)) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats effect {}:{}",
                        feature.id, effect.source, effect.symbol
                    )));
                }
                match effect.disposition {
                    FeatureEffectDispositionKind::Verified => {
                        let requirement = effect.requirement.as_deref().ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "verified feature effect {:?} requires requirement",
                                effect.id
                            ))
                        })?;
                        if !requirement_ids.contains(requirement) {
                            return Err(crate::Error::invalid(format!(
                                "feature effect {:?} refers to unknown requirement {requirement:?}",
                                effect.id
                            )));
                        }
                        let proof = feature
                            .requirements
                            .iter()
                            .find(|candidate| candidate.id == requirement)
                            .expect("validated requirement id exists");
                        if proof.source != effect.source || proof.symbol != effect.symbol {
                            return Err(crate::Error::invalid(format!(
                                "feature effect {:?} is {}:{} but requirement {requirement:?} proves {}:{}",
                                effect.id, effect.source, effect.symbol, proof.source, proof.symbol
                            )));
                        }
                    }
                    FeatureEffectDispositionKind::ExcludedByFeaturePolicy
                        if effect.requirement.is_some() =>
                    {
                        return Err(crate::Error::invalid(format!(
                            "policy-excluded feature effect {:?} cannot name a requirement",
                            effect.id
                        )));
                    }
                    FeatureEffectDispositionKind::ExcludedByFeaturePolicy => {}
                }
            }
        }
        Ok(())
    }

    fn feature(&self, id: &str) -> Option<&FeatureSpec> {
        self.features.iter().find(|feature| feature.id == id)
    }
}

pub(crate) fn validate_project(project: &ProjectSpec) -> Result<()> {
    let Some(workspace) = &project.qualification else {
        return Ok(());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    let scope_ids: BTreeSet<&str> = project
        .review
        .as_ref()
        .map(|review| {
            review
                .scopes
                .iter()
                .map(|scope| scope.id.as_str())
                .collect()
        })
        .unwrap_or_default();
    let suite_ids: BTreeSet<&str> = project
        .verification
        .as_ref()
        .map(|verification| {
            verification
                .suites
                .iter()
                .map(|suite| suite.id.as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut releases = BTreeSet::new();
    for id in &workspace.required_features {
        if !releases.insert(id) {
            return Err(crate::Error::invalid(format!(
                "duplicate required feature {id:?}"
            )));
        }
        let feature = pack
            .feature(id)
            .ok_or_else(|| crate::Error::invalid(format!("unknown required feature {id:?}")))?;
        for scope in &feature.scopes {
            if !scope_ids.contains(scope.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {id:?} refers to unknown review scope {scope:?}"
                )));
            }
        }
        for requirement in &feature.requirements {
            if !suite_ids.contains(requirement.suite.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {id:?} refers to unknown verification suite {:?}",
                    requirement.suite
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn evaluate(project: &ProjectSpec) -> Result<Vec<FeatureQualificationReport>> {
    let Some(workspace) = &project.qualification else {
        return Ok(Vec::new());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    let review = crate::review_scopes::load_for_project(project)?;
    let verification = project
        .verification
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[qualification] requires [verification]"))?;
    let required = workspace.required_features.iter().collect::<BTreeSet<_>>();
    pack.features
        .iter()
        .map(|feature| {
            let mut blockers = Vec::new();
            let mut scope_effects = BTreeSet::new();
            for scope_id in &feature.scopes {
                let scope = review
                    .scopes
                    .iter()
                    .find(|scope| scope.id == *scope_id)
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "qualification feature {:?} refers to missing scope {scope_id:?}",
                            feature.id
                        ))
                    })?;
                if !scope.analysis_inventory_complete {
                    blockers.push(format!(
                        "analysis scope {scope_id} is incomplete (decode={}, direct={}, call-graph={}, reference={}, unresolved-calls={})",
                        scope.decode_blockers,
                        scope.direct_blockers,
                        scope.call_graph_blockers,
                        scope.reference_blockers,
                        scope.unresolved_calls,
                    ));
                }
                scope_effects.extend(scope.replacement_function_keys.iter().cloned());
            }
            let (covered_effects, effect_blockers) = effect_coverage(feature, &scope_effects);
            blockers.extend(effect_blockers);
            for requirement in &feature.requirements {
                let path = suite_report_path(&verification.report, &requirement.suite);
                let report = load_suite_report(&path, &requirement.suite);
                let proof = report.as_ref().ok().and_then(|report| {
                    report
                        .sources
                        .iter()
                        .find(|source| source.source == requirement.source)
                        .and_then(|source| {
                            source
                                .functions
                                .iter()
                                .find(|function| function.vendor_symbol == requirement.symbol)
                        })
                });
                match proof {
                    _ if report.is_err() => blockers.push(format!(
                        "requirement {} cannot load suite {} report {}: {}",
                        requirement.id,
                        requirement.suite,
                        path.display(),
                        report.unwrap_err(),
                    )),
                    None => blockers.push(format!(
                        "requirement {} has no result in suite {} for {}:{}",
                        requirement.id,
                        requirement.suite,
                        requirement.source,
                        requirement.symbol
                    )),
                    Some(proof) if !status_satisfies_claim(proof.status, requirement.claim) => {
                        blockers.push(format!(
                            "requirement {} ({}) is {:?} in suite {} for {}:{}",
                            requirement.id,
                            requirement.description,
                            proof.status,
                            requirement.suite,
                            requirement.source,
                            requirement.symbol
                        ));
                    }
                    Some(proof) if proof.claim != Some(requirement.claim) => {
                        blockers.push(format!(
                            "requirement {} lacks a {:?} claim in suite {} for {}:{}",
                            requirement.id, requirement.claim, requirement.suite, requirement.source, requirement.symbol
                        ));
                    }
                    Some(proof)
                        if !proof.disposition_reviewed || proof.rust_component.is_none() =>
                    {
                        blockers.push(format!(
                            "requirement {} is not bound to a reviewed production component in suite {} for {}:{}",
                            requirement.id, requirement.suite, requirement.source, requirement.symbol
                        ));
                    }
                    Some(_) => {}
                }
            }
            blockers.sort();
            Ok(FeatureQualificationReport {
                id: feature.id.clone(),
                description: feature.description.clone(),
                required: required.contains(&feature.id),
                status: if blockers.is_empty() {
                    FeatureQualificationStatus::Qualified
                } else {
                    FeatureQualificationStatus::Blocked
                },
                scopes: feature.scopes.clone(),
                requirements: feature.requirements.len(),
                scope_effects: scope_effects.len(),
                covered_effects,
                blockers,
            })
        })
        .collect()
}

fn effect_coverage(
    feature: &FeatureSpec,
    scope_effects: &BTreeSet<String>,
) -> (usize, Vec<String>) {
    let dispositions = feature
        .effects
        .iter()
        .map(|effect| (format!("{}:{}", effect.source, effect.symbol), effect))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut blockers = Vec::new();
    for effect in scope_effects {
        if !dispositions.contains_key(effect) {
            blockers.push(format!(
                "scope effect {effect} has no verified or policy-excluded disposition"
            ));
        }
    }
    for effect in dispositions.keys() {
        if !scope_effects.contains(effect) {
            blockers.push(format!(
                "feature effect disposition {effect} is stale for the selected scopes"
            ));
        }
    }
    let covered = scope_effects
        .iter()
        .filter(|effect| dispositions.contains_key(*effect))
        .count();
    (covered, blockers)
}

pub(crate) fn suite_report_path(aggregate_report: &Path, suite: &str) -> PathBuf {
    aggregate_report
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("verification-suites")
        .join(format!("{suite}.json"))
}

fn load_suite_report(path: &Path, expected_id: &str) -> Result<StoredSuiteReport> {
    let input = fs::read_to_string(path).map_err(|error| {
        crate::Error::invalid(format!("cannot read {}: {error}", path.display()))
    })?;
    let report: StoredSuiteReport = serde_json::from_str(&input)?;
    if report.id != expected_id {
        return Err(crate::Error::invalid(format!(
            "suite report {} contains id {:?}, expected {expected_id:?}",
            path.display(),
            report.id
        )));
    }
    Ok(report)
}

fn validate_id(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(crate::Error::invalid(format!(
            "invalid {kind} id {value:?}"
        )));
    }
    Ok(())
}

fn status_satisfies_claim(status: FunctionVerificationStatus, claim: DriverAdapterClaim) -> bool {
    match claim {
        DriverAdapterClaim::WholeFunctionEquivalence => status == FunctionVerificationStatus::Match,
        DriverAdapterClaim::ReviewedProjection | DriverAdapterClaim::RustConformance => matches!(
            status,
            FunctionVerificationStatus::Match | FunctionVerificationStatus::ImplementedUnqualified
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_function_claim_rejects_a_reviewed_projection_status() {
        assert!(!status_satisfies_claim(
            FunctionVerificationStatus::ImplementedUnqualified,
            DriverAdapterClaim::WholeFunctionEquivalence,
        ));
    }

    #[test]
    fn reviewed_projection_accepts_only_completed_evidence() {
        assert!(status_satisfies_claim(
            FunctionVerificationStatus::ImplementedUnqualified,
            DriverAdapterClaim::ReviewedProjection,
        ));
        assert!(!status_satisfies_claim(
            FunctionVerificationStatus::Incomplete,
            DriverAdapterClaim::ReviewedProjection,
        ));
    }

    fn feature_with_effects(effects: Vec<FeatureEffectDisposition>) -> FeatureSpec {
        FeatureSpec {
            id: "sta".to_owned(),
            description: "fixture".to_owned(),
            scopes: vec!["connected".to_owned()],
            requirements: Vec::new(),
            effects,
        }
    }

    fn excluded(source: &str, symbol: &str) -> FeatureEffectDisposition {
        FeatureEffectDisposition {
            id: symbol.replace('_', "-"),
            source: source.to_owned(),
            symbol: symbol.to_owned(),
            disposition: FeatureEffectDispositionKind::ExcludedByFeaturePolicy,
            requirement: None,
            rationale: "explicit feature policy".to_owned(),
        }
    }

    #[test]
    fn missing_vendor_transaction_is_a_qualification_blocker() {
        let scope_effects = [
            "wifi:hal_set_sta_beacon_filter".to_owned(),
            "wifi:hal_disable_sta_beacon_filter".to_owned(),
        ]
        .into_iter()
        .collect();
        let feature = feature_with_effects(vec![excluded("wifi", "hal_disable_sta_beacon_filter")]);

        let (covered, blockers) = effect_coverage(&feature, &scope_effects);

        assert_eq!(covered, 1);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("wifi:hal_set_sta_beacon_filter"));
        assert!(blockers[0].contains("no verified or policy-excluded disposition"));
    }

    #[test]
    fn stale_vendor_transaction_disposition_is_a_qualification_blocker() {
        let scope_effects = ["wifi:hal_disable_sta_beacon_filter".to_owned()]
            .into_iter()
            .collect();
        let feature = feature_with_effects(vec![
            excluded("wifi", "hal_disable_sta_beacon_filter"),
            excluded("wifi", "removed_transaction"),
        ]);

        let (covered, blockers) = effect_coverage(&feature, &scope_effects);

        assert_eq!(covered, 1);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("wifi:removed_transaction"));
        assert!(blockers[0].contains("stale"));
    }
}
