//! Flat verification policy for release-relevant evidence surfaces.
//!
//! This module deliberately does not model product features, hardware runs or
//! readiness.  It answers one narrower question: are all checked-in vendor
//! comparison surfaces closed by current reviewed evidence?  Repository-level
//! qualification remains the sole owner of product readiness.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use open_radio_vendor_semantics::DriverAdapterClaim;
use serde::{Deserialize, Serialize};

use crate::{
    ProjectSpec, Result,
    verification::{
        FunctionVerificationStatus,
        dispositions::{Disposition, Manifest},
    },
};

const POLICY_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema: u32,
    surfaces: Vec<Surface>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Surface {
    id: String,
    description: String,
    kind: SurfaceKind,
    #[serde(default, rename = "review-scopes")]
    review_scopes: Vec<String>,
    #[serde(default)]
    requirements: Vec<Requirement>,
    #[serde(default)]
    effects: Vec<EffectDecision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SurfaceKind {
    ReviewScope,
    SelectedFunctions,
    BoundedProperty,
}

impl SurfaceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ReviewScope => "review-scope",
            Self::SelectedFunctions => "selected-functions",
            Self::BoundedProperty => "bounded-property",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Requirement {
    id: String,
    description: String,
    suite: String,
    source: String,
    symbol: String,
    claim: DriverAdapterClaim,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectDecision {
    id: String,
    vendor: VendorTransaction,
    disposition: EffectDisposition,
    requirement: Option<String>,
    rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct VendorTransaction {
    source: String,
    symbol: String,
    identity: Option<String>,
    fingerprint: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EffectDisposition {
    Verified,
    ExcludedByPolicy,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RequiredPolicyExclusion {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) identity: Option<String>,
    pub(crate) fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingRequirement {
    pub(crate) surface: String,
    pub(crate) claim: DriverAdapterClaim,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PolicyReport {
    pub(crate) schema: u32,
    pub(crate) surfaces: Vec<SurfaceReport>,
    pub(crate) closed: usize,
    pub(crate) blocked: usize,
    pub(crate) passed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SurfaceReport {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) kind: SurfaceKind,
    pub(crate) review_scopes: Vec<String>,
    pub(crate) requirements: usize,
    pub(crate) effects: usize,
    pub(crate) blockers: Vec<String>,
    pub(crate) closed: bool,
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

impl Policy {
    fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot read verification policy {}: {error}",
                path.display()
            ))
        })?;
        let policy: Self = toml_edit::de::from_str(&input).map_err(|error| {
            crate::Error::invalid(format!(
                "invalid verification policy {}: {error}",
                path.display()
            ))
        })?;
        policy.validate(path)?;
        Ok(policy)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema != POLICY_SCHEMA {
            return Err(crate::Error::invalid(format!(
                "verification policy {} requires schema = {POLICY_SCHEMA}",
                path.display()
            )));
        }
        if self.surfaces.is_empty() {
            return Err(crate::Error::invalid(format!(
                "verification policy {} has no surfaces",
                path.display()
            )));
        }
        let mut surface_ids = BTreeSet::new();
        for surface in &self.surfaces {
            validate_id(&surface.id, "surface")?;
            if surface.description.trim().is_empty() || !surface_ids.insert(&surface.id) {
                return Err(crate::Error::invalid(format!(
                    "verification surface {:?} has an empty description or duplicate id",
                    surface.id
                )));
            }
            if surface.kind == SurfaceKind::ReviewScope && surface.review_scopes.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "review-scope surface {:?} requires review-scopes",
                    surface.id
                )));
            }
            if surface.kind != SurfaceKind::ReviewScope && !surface.review_scopes.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "{} surface {:?} cannot select review scopes",
                    surface.kind.as_str(),
                    surface.id
                )));
            }
            if surface.requirements.is_empty() || surface.effects.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "verification surface {:?} requires explicit requirements and effects",
                    surface.id
                )));
            }
            let mut requirement_ids = BTreeSet::new();
            let mut selectors = BTreeSet::new();
            for requirement in &surface.requirements {
                validate_id(&requirement.id, "requirement")?;
                if requirement.description.trim().is_empty()
                    || requirement.suite.trim().is_empty()
                    || requirement.source.trim().is_empty()
                    || requirement.symbol.trim().is_empty()
                    || !requirement_ids.insert(requirement.id.as_str())
                    || !selectors.insert((
                        requirement.suite.as_str(),
                        requirement.source.as_str(),
                        requirement.symbol.as_str(),
                    ))
                {
                    return Err(crate::Error::invalid(format!(
                        "verification surface {:?} has an incomplete or duplicate requirement {:?}",
                        surface.id, requirement.id
                    )));
                }
                if surface.kind == SurfaceKind::BoundedProperty
                    && requirement.claim == DriverAdapterClaim::WholeFunctionEquivalence
                {
                    return Err(crate::Error::invalid(format!(
                        "bounded-property surface {:?} cannot claim whole-function equivalence",
                        surface.id
                    )));
                }
            }
            let mut effect_ids = BTreeSet::new();
            let mut effect_selectors = BTreeSet::new();
            for effect in &surface.effects {
                validate_id(&effect.id, "effect")?;
                validate_fingerprint(&effect.vendor.fingerprint)?;
                if effect.rationale.trim().is_empty()
                    || effect.vendor.source.trim().is_empty()
                    || effect.vendor.symbol.trim().is_empty()
                    || effect.vendor.identity.as_deref().is_some_and(str::is_empty)
                    || !effect_ids.insert(effect.id.as_str())
                    || !effect_selectors.insert((
                        effect.vendor.source.as_str(),
                        effect.vendor.symbol.as_str(),
                        effect.vendor.identity.as_deref(),
                    ))
                {
                    return Err(crate::Error::invalid(format!(
                        "verification surface {:?} has an incomplete or duplicate effect {:?}",
                        surface.id, effect.id
                    )));
                }
                match effect.disposition {
                    EffectDisposition::Verified => {
                        let requirement = effect.requirement.as_deref().ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "verified effect {:?} requires a requirement",
                                effect.id
                            ))
                        })?;
                        if !requirement_ids.contains(requirement) {
                            return Err(crate::Error::invalid(format!(
                                "effect {:?} refers to unknown requirement {requirement:?}",
                                effect.id
                            )));
                        }
                    }
                    EffectDisposition::ExcludedByPolicy => {
                        if surface.kind != SurfaceKind::ReviewScope {
                            return Err(crate::Error::invalid(format!(
                                "{} surface {:?} cannot exclude effects",
                                surface.kind.as_str(),
                                surface.id
                            )));
                        }
                        if effect.requirement.is_some() {
                            return Err(crate::Error::invalid(format!(
                                "policy-excluded effect {:?} cannot name a requirement",
                                effect.id
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn configured(project: &ProjectSpec) -> Result<Option<Policy>> {
    project
        .verification
        .as_ref()
        .and_then(|verification| verification.policy.as_deref())
        .map(Policy::load)
        .transpose()
}

pub(crate) fn required_scope_policy_exclusions(
    project: &ProjectSpec,
) -> Result<BTreeMap<String, BTreeSet<RequiredPolicyExclusion>>> {
    let Some(policy) = configured(project)? else {
        return Ok(BTreeMap::new());
    };
    let mut exclusions = BTreeMap::<String, BTreeSet<RequiredPolicyExclusion>>::new();
    for surface in policy
        .surfaces
        .iter()
        .filter(|surface| surface.kind == SurfaceKind::ReviewScope)
    {
        for effect in surface
            .effects
            .iter()
            .filter(|effect| effect.disposition == EffectDisposition::ExcludedByPolicy)
        {
            let exclusion = RequiredPolicyExclusion {
                source: effect.vendor.source.clone(),
                symbol: effect.vendor.symbol.clone(),
                identity: effect.vendor.identity.clone(),
                fingerprint: effect.vendor.fingerprint.clone(),
            };
            for scope in &surface.review_scopes {
                exclusions
                    .entry(scope.clone())
                    .or_default()
                    .insert(exclusion.clone());
            }
        }
    }
    Ok(exclusions)
}

pub(crate) fn required_review_scopes(project: &ProjectSpec) -> Result<BTreeSet<String>> {
    Ok(configured(project)?
        .into_iter()
        .flat_map(|policy| policy.surfaces)
        .flat_map(|surface| surface.review_scopes)
        .collect())
}

pub(crate) fn binding_requirements(
    project: &ProjectSpec,
) -> Result<BTreeMap<(String, String, String), Vec<BindingRequirement>>> {
    let Some(policy) = configured(project)? else {
        return Ok(BTreeMap::new());
    };
    let mut requirements = BTreeMap::new();
    for surface in policy.surfaces {
        for requirement in surface.requirements {
            requirements
                .entry((requirement.suite, requirement.source, requirement.symbol))
                .or_insert_with(Vec::new)
                .push(BindingRequirement {
                    surface: surface.id.clone(),
                    claim: requirement.claim,
                });
        }
    }
    for entries in requirements.values_mut() {
        entries.sort_by(|left, right| left.surface.cmp(&right.surface));
    }
    Ok(requirements)
}

pub(crate) fn validate_project(project: &ProjectSpec) -> Result<()> {
    let Some(policy) = configured(project)? else {
        return Ok(());
    };
    let scope_ids: BTreeSet<_> = project
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
    let verification = project
        .verification
        .as_ref()
        .expect("a configured policy belongs to a verification workspace");
    let suite_ids = verification
        .suites
        .iter()
        .map(|suite| suite.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut bounded_requirements = BTreeSet::new();
    for surface in &policy.surfaces {
        for scope in &surface.review_scopes {
            if !scope_ids.contains(scope.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "verification surface {:?} refers to unknown review scope {scope:?}",
                    surface.id
                )));
            }
        }
        for requirement in &surface.requirements {
            if !suite_ids.contains(requirement.suite.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "verification surface {:?} refers to unknown suite {:?}",
                    surface.id, requirement.suite
                )));
            }
            if requirement.claim != DriverAdapterClaim::WholeFunctionEquivalence {
                bounded_requirements.insert((
                    requirement.suite.as_str(),
                    requirement.source.as_str(),
                    requirement.symbol.as_str(),
                ));
            }
        }
    }
    for suite in &verification.suites {
        let Some(manifest) = Manifest::load_all(&suite.dispositions)? else {
            continue;
        };
        for entry in manifest
            .entries()
            .filter(|entry| entry.disposition == Disposition::BoundedFeature)
        {
            if !bounded_requirements.contains(&(
                suite.id.as_str(),
                entry.source.as_str(),
                entry.symbol.as_str(),
            )) {
                return Err(crate::Error::invalid(format!(
                    "bounded comparison {}:{} in suite {:?} has no bounded verification surface",
                    entry.source, entry.symbol, suite.id
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn evaluate(project: &ProjectSpec) -> Result<Option<PolicyReport>> {
    let Some(policy) = configured(project)? else {
        return Ok(None);
    };
    let review = crate::review_scopes::load_for_project(project)?;
    let verification = project
        .verification
        .as_ref()
        .expect("a configured policy belongs to a verification workspace");
    let suite_reports = policy
        .surfaces
        .iter()
        .flat_map(|surface| surface.requirements.iter())
        .map(|requirement| requirement.suite.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|suite| {
            let path = suite_report_path(&verification.report, suite);
            let report = load_suite_report(&path, suite).map_err(|error| error.to_string());
            (suite.to_owned(), (path, report))
        })
        .collect::<BTreeMap<_, _>>();
    let mut surfaces = Vec::new();
    for surface in policy.surfaces {
        let mut blockers = Vec::new();
        let mut transactions = BTreeMap::new();
        for scope_id in &surface.review_scopes {
            let scope = review
                .scopes
                .iter()
                .find(|scope| scope.id == *scope_id)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "verification surface {:?} refers to missing scope {scope_id:?}",
                        surface.id
                    ))
                })?;
            if !scope.analysis_inventory_complete {
                blockers.push(format!("analysis scope {scope_id} is incomplete"));
            }
            for transaction in &scope.transactions {
                if let Some(previous) = transactions.insert(&transaction.identity, transaction)
                    && previous.fingerprint != transaction.fingerprint
                {
                    blockers.push(format!(
                        "transaction {} has conflicting fingerprints across selected scopes",
                        transaction.identity
                    ));
                }
            }
        }
        if surface.kind == SurfaceKind::ReviewScope {
            let mut matched = BTreeSet::new();
            for transaction in transactions.values() {
                let candidates = surface
                    .effects
                    .iter()
                    .filter(|effect| {
                        effect.vendor.source == transaction.source
                            && effect.vendor.symbol == transaction.symbol
                            && effect
                                .vendor
                                .identity
                                .as_deref()
                                .is_none_or(|identity| identity == transaction.identity)
                    })
                    .collect::<Vec<_>>();
                if candidates.len() != 1 {
                    blockers.push(format!(
                        "transaction {} ({}) has {} policy decisions",
                        transaction.id,
                        transaction.identity,
                        candidates.len()
                    ));
                    continue;
                }
                let decision = candidates[0];
                matched.insert(decision.id.as_str());
                if decision.vendor.fingerprint != transaction.fingerprint {
                    blockers.push(format!(
                        "transaction {} changed: reviewed {}, current {}",
                        decision.id, decision.vendor.fingerprint, transaction.fingerprint
                    ));
                }
            }
            for effect in &surface.effects {
                if !matched.contains(effect.id.as_str()) {
                    blockers.push(format!(
                        "policy effect {} ({}:{}) is stale for the selected scopes",
                        effect.id, effect.vendor.source, effect.vendor.symbol
                    ));
                }
            }
        }
        for requirement in &surface.requirements {
            let (path, report) = suite_reports
                .get(&requirement.suite)
                .expect("each required suite was cached");
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
            let blocker = match (report, proof) {
                (Err(error), _) => Some(format!(
                    "requirement {} cannot load suite report {}: {error}",
                    requirement.id,
                    path.display()
                )),
                (_, None) => Some(format!(
                    "requirement {} has no result in suite {} for {}:{}",
                    requirement.id, requirement.suite, requirement.source, requirement.symbol
                )),
                (_, Some(proof)) if !status_satisfies_claim(proof.status, requirement.claim) => {
                    Some(format!(
                        "requirement {} ({}) is {:?}",
                        requirement.id, requirement.description, proof.status
                    ))
                }
                (_, Some(proof)) if proof.claim != Some(requirement.claim) => Some(format!(
                    "requirement {} lacks a {:?} claim",
                    requirement.id, requirement.claim
                )),
                (_, Some(proof))
                    if !proof.disposition_reviewed || proof.rust_component.is_none() =>
                {
                    Some(format!(
                        "requirement {} is not bound to a reviewed production component",
                        requirement.id
                    ))
                }
                (_, Some(_)) => None,
            };
            if let Some(blocker) = blocker {
                blockers.push(blocker);
            }
        }
        blockers.sort();
        blockers.dedup();
        surfaces.push(SurfaceReport {
            id: surface.id,
            description: surface.description,
            kind: surface.kind,
            review_scopes: surface.review_scopes,
            requirements: surface.requirements.len(),
            effects: surface.effects.len(),
            closed: blockers.is_empty(),
            blockers,
        });
    }
    let closed = surfaces.iter().filter(|surface| surface.closed).count();
    let blocked = surfaces.len() - closed;
    Ok(Some(PolicyReport {
        schema: POLICY_SCHEMA,
        surfaces,
        closed,
        blocked,
        passed: blocked == 0,
    }))
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
            "invalid verification {kind} id {value:?}"
        )));
    }
    Ok(())
}

fn validate_fingerprint(value: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(crate::Error::invalid(format!(
            "transaction fingerprint {value:?} must use sha256:<hex>"
        )));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(crate::Error::invalid(format!(
            "invalid transaction fingerprint {value:?}"
        )));
    }
    Ok(())
}

fn status_satisfies_claim(status: FunctionVerificationStatus, claim: DriverAdapterClaim) -> bool {
    match claim {
        DriverAdapterClaim::WholeFunctionEquivalence => status == FunctionVerificationStatus::Match,
        DriverAdapterClaim::ReviewedDomainEquivalence
        | DriverAdapterClaim::ReviewedRefinement
        | DriverAdapterClaim::ReviewedProjection
        | DriverAdapterClaim::RustConformance => {
            matches!(status, FunctionVerificationStatus::BoundedMatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_function_claim_rejects_bounded_match() {
        assert!(!status_satisfies_claim(
            FunctionVerificationStatus::BoundedMatch,
            DriverAdapterClaim::WholeFunctionEquivalence
        ));
    }

    #[test]
    fn bounded_claim_accepts_only_bounded_match() {
        assert!(status_satisfies_claim(
            FunctionVerificationStatus::BoundedMatch,
            DriverAdapterClaim::ReviewedRefinement
        ));
        assert!(!status_satisfies_claim(
            FunctionVerificationStatus::Match,
            DriverAdapterClaim::ReviewedRefinement
        ));
    }
}
