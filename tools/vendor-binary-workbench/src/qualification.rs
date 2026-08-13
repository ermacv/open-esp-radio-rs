//! Fail-closed qualification of user-visible driver features.
//!
//! Review scopes remain useful publication and navigation surfaces. A feature
//! gate is stricter: every configured analysis closure must be complete and
//! every named proof must establish the requested claim strength.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use open_radio_vendor_semantics::DriverAdapterClaim;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    project::ProjectSpec,
    verification::{
        FunctionVerificationStatus,
        dispositions::{Disposition, Manifest},
    },
};

mod hardware;

use hardware::evaluate_hardware;

pub(crate) const FEATURE_PACK_SCHEMA: u32 = 5;

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
    coverage: FeatureCoverage,
    #[serde(default)]
    scopes: Vec<String>,
    phases: Vec<FeaturePhase>,
    #[serde(default)]
    requirements: Vec<FeatureRequirement>,
    #[serde(default)]
    effects: Vec<FeatureEffectDisposition>,
    #[serde(default)]
    dependencies: Vec<FeatureDependency>,
    #[serde(default)]
    hardware: Option<FeatureHardwareSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct FeatureHardwareSpec {
    minimum_successful_runs: usize,
    required_observations: Vec<String>,
    required_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeaturePhase {
    id: String,
    description: String,
    #[serde(default)]
    scopes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FeatureCoverage {
    ReviewScopes,
    BoundedEvidence,
    SelectedEvidence,
    ComposedFeatures,
}

impl FeatureCoverage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ReviewScopes => "review-scopes",
            Self::BoundedEvidence => "bounded-evidence",
            Self::SelectedEvidence => "selected-evidence",
            Self::ComposedFeatures => "composed-features",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureDependency {
    feature: String,
    phase: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureEffectDisposition {
    id: String,
    phase: String,
    vendor: FeatureVendorTransaction,
    disposition: FeatureEffectDispositionKind,
    requirement: Option<String>,
    rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureVendorTransaction {
    source: String,
    symbol: String,
    identity: Option<String>,
    fingerprint: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FeatureEffectDispositionKind {
    Verified,
    ExcludedByFeaturePolicy,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RequiredPolicyExclusion {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) identity: Option<String>,
    pub(crate) fingerprint: String,
}

impl FeatureEffectDispositionKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::ExcludedByFeaturePolicy => "excluded-by-feature-policy",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureRequirement {
    id: String,
    phase: String,
    description: String,
    suite: String,
    source: String,
    symbol: String,
    claim: DriverAdapterClaim,
}

/// Feature-policy references to one executable vendor/Rust binding.
///
/// The same binding may support multiple public features.  `release_required`
/// is derived solely from the transitive closure of `[qualification]
/// required-features`; it is not inferred from whether a baseline happens to
/// exist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingFeatureRequirement {
    pub(crate) feature: String,
    pub(crate) claim: DriverAdapterClaim,
    pub(crate) release_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FeatureQualificationStatus {
    Qualified,
    HardwareQualified,
    Blocked,
}

impl FeatureQualificationStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Qualified => "qualified",
            Self::HardwareQualified => "hardware-qualified",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeatureQualificationReport {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) required: bool,
    pub(crate) status: FeatureQualificationStatus,
    pub(crate) coverage: FeatureCoverage,
    pub(crate) scopes: Vec<String>,
    pub(crate) requirements: usize,
    pub(crate) surface_effects: usize,
    pub(crate) covered_effects: usize,
    pub(crate) phases: Vec<FeaturePhaseReport>,
    pub(crate) transactions: Vec<FeatureTransactionReport>,
    pub(crate) dependencies: Vec<FeatureDependencyReport>,
    pub(crate) hardware: Option<FeatureHardwareReport>,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeatureDependencyReport {
    pub(crate) feature: String,
    pub(crate) phase: Option<String>,
    pub(crate) status: FeatureQualificationStatus,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeatureHardwareReport {
    pub(crate) status: String,
    pub(crate) successful_runs: usize,
    pub(crate) minimum_successful_runs: usize,
    pub(crate) observations: Vec<String>,
    pub(crate) required_observations: Vec<String>,
    pub(crate) artifacts: Vec<String>,
    pub(crate) required_artifacts: Vec<String>,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeaturePhaseReport {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) requirements: usize,
    pub(crate) transactions: usize,
    pub(crate) covered_transactions: usize,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FeatureTransactionReport {
    pub(crate) id: String,
    pub(crate) phase: String,
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) identity: Option<String>,
    pub(crate) fingerprint: String,
    pub(crate) disposition: String,
    pub(crate) requirement: Option<String>,
    pub(crate) rationale: String,
    pub(crate) effects: Vec<crate::review_scopes::ReviewScopeEffect>,
    pub(crate) paths: Vec<Vec<String>>,
    pub(crate) current: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FunctionQualificationEvidence {
    pub(crate) feature: String,
    pub(crate) description: String,
    pub(crate) required: bool,
    pub(crate) status: FeatureQualificationStatus,
    pub(crate) coverage: FeatureCoverage,
    pub(crate) requirements: Vec<FunctionQualificationRequirement>,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FunctionQualificationRequirement {
    pub(crate) id: String,
    pub(crate) suite: String,
    pub(crate) claim: DriverAdapterClaim,
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

    fn dependency_closure<'pack, 'root>(
        &'pack self,
        roots: impl IntoIterator<Item = &'root str>,
    ) -> BTreeSet<&'pack str> {
        fn visit<'pack>(
            pack: &'pack FeaturePack,
            feature: &'pack FeatureSpec,
            selected: &mut BTreeSet<&'pack str>,
        ) {
            if !selected.insert(feature.id.as_str()) {
                return;
            }
            for dependency in &feature.dependencies {
                let target = pack
                    .feature(&dependency.feature)
                    .expect("validated dependency target exists");
                visit(pack, target, selected);
            }
        }

        let mut selected = BTreeSet::new();
        for root in roots {
            let feature = self
                .feature(root)
                .expect("required root existence is validated before closure");
            visit(self, feature, &mut selected);
        }
        selected
    }

    fn required_review_scopes<'root>(
        &self,
        roots: impl IntoIterator<Item = &'root str>,
    ) -> BTreeSet<String> {
        fn visit(
            pack: &FeaturePack,
            feature: &FeatureSpec,
            selected_phase: Option<&str>,
            visited: &mut BTreeSet<(String, Option<String>)>,
            scopes: &mut BTreeSet<String>,
        ) {
            let key = (feature.id.clone(), selected_phase.map(str::to_owned));
            if !visited.insert(key) {
                return;
            }
            match selected_phase {
                Some(phase) => scopes.extend(
                    feature
                        .phases
                        .iter()
                        .find(|candidate| candidate.id == phase)
                        .expect("validated dependency phase exists")
                        .scopes
                        .iter()
                        .cloned(),
                ),
                None => scopes.extend(feature.scopes.iter().cloned()),
            }
            for dependency in &feature.dependencies {
                let target = pack
                    .feature(&dependency.feature)
                    .expect("validated dependency target exists");
                visit(pack, target, dependency.phase.as_deref(), visited, scopes);
            }
        }

        let mut scopes = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for root in roots {
            let feature = self
                .feature(root)
                .expect("required root existence is validated before scope selection");
            visit(self, feature, None, &mut visited, &mut scopes);
        }
        scopes
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
            if feature.phases.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {:?} requires at least one lifecycle phase",
                    feature.id
                )));
            }
            let mut phase_ids = BTreeSet::new();
            let mut phase_scopes = BTreeSet::new();
            for phase in &feature.phases {
                validate_id(&phase.id, "feature phase")?;
                if phase.description.trim().is_empty() || !phase_ids.insert(phase.id.as_str()) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} has an empty or duplicate phase {:?}",
                        feature.id, phase.id
                    )));
                }
                if feature.coverage != FeatureCoverage::ReviewScopes && !phase.scopes.is_empty() {
                    return Err(crate::Error::invalid(format!(
                        "{} feature {:?} phase {:?} cannot select review scopes",
                        feature.coverage.as_str(),
                        feature.id,
                        phase.id
                    )));
                }
                for scope in &phase.scopes {
                    validate_id(scope, "feature phase scope")?;
                    if !feature.scopes.contains(scope) {
                        return Err(crate::Error::invalid(format!(
                            "qualification feature {:?} phase {:?} refers to scope {scope:?} outside the feature boundary",
                            feature.id, phase.id
                        )));
                    }
                    if !phase_scopes.insert(scope.as_str()) {
                        return Err(crate::Error::invalid(format!(
                            "qualification feature {:?} assigns scope {scope:?} to multiple phases",
                            feature.id
                        )));
                    }
                }
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
            match feature.coverage {
                FeatureCoverage::ReviewScopes if feature.scopes.is_empty() => {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} with review-scopes coverage requires at least one scope",
                        feature.id
                    )));
                }
                FeatureCoverage::BoundedEvidence if !feature.scopes.is_empty() => {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} with bounded-evidence coverage must not select review scopes",
                        feature.id
                    )));
                }
                FeatureCoverage::SelectedEvidence if !feature.scopes.is_empty() => {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} with selected-evidence coverage must not select review scopes",
                        feature.id
                    )));
                }
                FeatureCoverage::ComposedFeatures if !feature.scopes.is_empty() => {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} with composed-features coverage must not select review scopes",
                        feature.id
                    )));
                }
                _ => {}
            }
            if feature.coverage == FeatureCoverage::ReviewScopes
                && feature
                    .scopes
                    .iter()
                    .any(|scope| !phase_scopes.contains(scope.as_str()))
            {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {:?} must assign every selected review scope to one phase",
                    feature.id
                )));
            }
            let mut requirement_ids = BTreeSet::new();
            let mut requirement_selectors = BTreeSet::new();
            for requirement in &feature.requirements {
                validate_id(&requirement.id, "feature requirement")?;
                validate_phase(&feature.id, &requirement.phase, &phase_ids, "requirement")?;
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
                validate_phase(&feature.id, &effect.phase, &phase_ids, "effect")?;
                if effect.vendor.source.trim().is_empty()
                    || effect.vendor.symbol.trim().is_empty()
                    || effect.rationale.trim().is_empty()
                {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} has an incomplete effect disposition {:?}",
                        feature.id, effect.id
                    )));
                }
                validate_transaction_fingerprint(&effect.vendor.fingerprint)?;
                if effect.vendor.identity.as_deref().is_some_and(str::is_empty) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} effect {:?} has an empty vendor identity",
                        feature.id, effect.id
                    )));
                }
                if !effect_ids.insert(effect.id.as_str()) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats effect id {:?}",
                        feature.id, effect.id
                    )));
                }
                if !effect_selectors.insert((
                    &effect.vendor.source,
                    &effect.vendor.symbol,
                    &effect.vendor.identity,
                )) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats transaction {}:{}",
                        feature.id, effect.vendor.source, effect.vendor.symbol
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
                    }
                    FeatureEffectDispositionKind::ExcludedByFeaturePolicy
                        if effect.requirement.is_some() =>
                    {
                        return Err(crate::Error::invalid(format!(
                            "policy-excluded feature effect {:?} cannot name a requirement",
                            effect.id
                        )));
                    }
                    FeatureEffectDispositionKind::ExcludedByFeaturePolicy => {
                        if matches!(
                            feature.coverage,
                            FeatureCoverage::BoundedEvidence | FeatureCoverage::SelectedEvidence
                        ) {
                            return Err(crate::Error::invalid(format!(
                                "{} feature {:?} cannot exclude effect {:?}; it must provide verified evidence",
                                feature.coverage.as_str(),
                                feature.id,
                                effect.id
                            )));
                        }
                    }
                }
            }
            if matches!(
                feature.coverage,
                FeatureCoverage::BoundedEvidence | FeatureCoverage::SelectedEvidence
            ) && (feature.requirements.is_empty() || feature.effects.is_empty())
            {
                return Err(crate::Error::invalid(format!(
                    "{} feature {:?} requires explicit proofs and verified effects",
                    feature.coverage.as_str(),
                    feature.id
                )));
            }
            if feature.coverage == FeatureCoverage::ComposedFeatures
                && (!feature.requirements.is_empty() || !feature.effects.is_empty())
            {
                return Err(crate::Error::invalid(format!(
                    "composed-features feature {:?} must obtain assurance through dependencies, not direct requirements or effects",
                    feature.id
                )));
            }
            if feature.coverage == FeatureCoverage::ComposedFeatures
                && feature.dependencies.is_empty()
            {
                return Err(crate::Error::invalid(format!(
                    "composed-features feature {:?} requires at least one feature dependency",
                    feature.id
                )));
            }
            let mut dependencies = BTreeSet::new();
            for dependency in &feature.dependencies {
                validate_id(&dependency.feature, "feature dependency")?;
                if !dependencies.insert((&dependency.feature, &dependency.phase)) {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} repeats dependency {:?}",
                        feature.id, dependency.feature
                    )));
                }
            }
            if feature.coverage == FeatureCoverage::BoundedEvidence
                && feature.requirements.iter().any(|requirement| {
                    requirement.claim == DriverAdapterClaim::WholeFunctionEquivalence
                })
            {
                return Err(crate::Error::invalid(format!(
                    "bounded-evidence feature {:?} cannot claim whole-function equivalence",
                    feature.id
                )));
            }
            if let Some(hardware) = &feature.hardware {
                if hardware.minimum_successful_runs == 0 {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} hardware minimum-successful-runs must be nonzero",
                        feature.id
                    )));
                }
                validate_unique_nonempty(
                    &feature.id,
                    "hardware required observation",
                    &hardware.required_observations,
                )?;
                validate_unique_nonempty(
                    &feature.id,
                    "hardware required artifact",
                    &hardware.required_artifacts,
                )?;
            }
        }
        for feature in &self.features {
            for dependency in &feature.dependencies {
                let target = self.feature(&dependency.feature).ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "qualification feature {:?} depends on unknown feature {:?}",
                        feature.id, dependency.feature
                    ))
                })?;
                if dependency.feature == feature.id {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} cannot depend on itself",
                        feature.id
                    )));
                }
                if let Some(phase) = &dependency.phase
                    && !target.phases.iter().any(|candidate| candidate.id == *phase)
                {
                    return Err(crate::Error::invalid(format!(
                        "qualification feature {:?} depends on unknown phase {:?} of feature {:?}",
                        feature.id, phase, dependency.feature
                    )));
                }
            }
        }
        validate_dependency_cycles(self)?;
        Ok(())
    }

    fn feature(&self, id: &str) -> Option<&FeatureSpec> {
        self.features.iter().find(|feature| feature.id == id)
    }
}

pub(crate) fn required_scope_policy_exclusions(
    project: &ProjectSpec,
) -> Result<BTreeMap<String, BTreeSet<RequiredPolicyExclusion>>> {
    let Some(workspace) = project.qualification.as_ref() else {
        return Ok(BTreeMap::new());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    let selected = pack.dependency_closure(workspace.required_features.iter().map(String::as_str));
    let mut exclusions = BTreeMap::<String, BTreeSet<RequiredPolicyExclusion>>::new();
    for feature in pack
        .features
        .iter()
        .filter(|feature| selected.contains(feature.id.as_str()))
        .filter(|feature| feature.coverage == FeatureCoverage::ReviewScopes)
    {
        for effect in feature.effects.iter().filter(|effect| {
            effect.disposition == FeatureEffectDispositionKind::ExcludedByFeaturePolicy
        }) {
            let exclusion = RequiredPolicyExclusion {
                source: effect.vendor.source.clone(),
                symbol: effect.vendor.symbol.clone(),
                identity: effect.vendor.identity.clone(),
                fingerprint: effect.vendor.fingerprint.clone(),
            };
            for scope in &feature.scopes {
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
    let Some(workspace) = &project.qualification else {
        return Ok(BTreeSet::new());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    for id in &workspace.required_features {
        if pack.feature(id).is_none() {
            return Err(crate::Error::invalid(format!(
                "unknown required feature {id:?}"
            )));
        }
    }
    Ok(pack.required_review_scopes(workspace.required_features.iter().map(String::as_str)))
}

pub(crate) fn binding_feature_requirements(
    project: &ProjectSpec,
) -> Result<BTreeMap<(String, String, String), Vec<BindingFeatureRequirement>>> {
    let Some(workspace) = &project.qualification else {
        return Ok(BTreeMap::new());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    let selected = pack.dependency_closure(workspace.required_features.iter().map(String::as_str));
    let mut requirements = BTreeMap::<
        (String, String, String),
        Vec<BindingFeatureRequirement>,
    >::new();
    for feature in &pack.features {
        let release_required = selected.contains(feature.id.as_str());
        for requirement in &feature.requirements {
            requirements
                .entry((
                    requirement.suite.clone(),
                    requirement.source.clone(),
                    requirement.symbol.clone(),
                ))
                .or_default()
                .push(BindingFeatureRequirement {
                    feature: feature.id.clone(),
                    claim: requirement.claim,
                    release_required,
                });
        }
    }
    for policies in requirements.values_mut() {
        policies.sort_by(|left, right| left.feature.cmp(&right.feature));
    }
    Ok(requirements)
}

fn validate_dependency_cycles(pack: &FeaturePack) -> Result<()> {
    fn visit<'a>(
        pack: &'a FeaturePack,
        feature: &'a FeatureSpec,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if visited.contains(feature.id.as_str()) {
            return Ok(());
        }
        if !visiting.insert(feature.id.as_str()) {
            let chain = visiting.iter().copied().collect::<Vec<_>>().join(" -> ");
            return Err(crate::Error::invalid(format!(
                "qualification feature dependency cycle contains {} -> {}",
                chain, feature.id
            )));
        }
        for dependency in &feature.dependencies {
            let target = pack
                .feature(&dependency.feature)
                .expect("dependency targets were validated");
            visit(pack, target, visiting, visited)?;
        }
        visiting.remove(feature.id.as_str());
        visited.insert(feature.id.as_str());
        Ok(())
    }

    let mut visited = BTreeSet::new();
    for feature in &pack.features {
        visit(pack, feature, &mut BTreeSet::new(), &mut visited)?;
    }
    Ok(())
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
        pack.feature(id)
            .ok_or_else(|| crate::Error::invalid(format!("unknown required feature {id:?}")))?;
    }
    let mut bounded_requirements = BTreeSet::new();
    for feature in &pack.features {
        for requirement in &feature.requirements {
            if requirement.claim != DriverAdapterClaim::WholeFunctionEquivalence {
                bounded_requirements.insert((
                    requirement.suite.as_str(),
                    requirement.source.as_str(),
                    requirement.symbol.as_str(),
                ));
            }
        }
    }
    for feature in &pack.features {
        for scope in &feature.scopes {
            if !scope_ids.contains(scope.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {:?} refers to unknown review scope {scope:?}",
                    feature.id
                )));
            }
        }
        for requirement in &feature.requirements {
            if !suite_ids.contains(requirement.suite.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "qualification feature {:?} refers to unknown verification suite {:?}",
                    feature.id, requirement.suite
                )));
            }
        }
    }
    if let Some(verification) = &project.verification {
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
                        "bounded-feature {}:{} in verification suite {:?} has no bounded feature requirement",
                        entry.source, entry.symbol, suite.id
                    )));
                }
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
    // A suite can prove requirements for many features.  Reading and parsing
    // its complete report once per requirement made read-only status scale as
    // O(requirements × suite-size), even though every lookup addresses the
    // same immutable document.
    let suite_reports = pack
        .features
        .iter()
        .flat_map(|feature| feature.requirements.iter())
        .map(|requirement| requirement.suite.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|suite| {
            let path = suite_report_path(&verification.report, suite);
            let report = load_suite_report(&path, suite).map_err(|error| error.to_string());
            (suite.to_owned(), (path, report))
        })
        .collect::<BTreeMap<_, _>>();
    let required = workspace.required_features.iter().collect::<BTreeSet<_>>();
    let mut reports = pack
        .features
        .iter()
        .map(|feature| {
            let mut blockers = Vec::new();
            let mut phase_blockers = feature
                .phases
                .iter()
                .map(|phase| (phase.id.clone(), Vec::<String>::new()))
                .collect::<std::collections::BTreeMap<_, _>>();
            let mut scope_transactions = std::collections::BTreeMap::<
                String,
                (crate::review_scopes::ReviewScopeTransaction, String),
            >::new();
            for scope_id in &feature.scopes {
                let scope_phase = feature
                    .phases
                    .iter()
                    .find(|phase| phase.scopes.contains(scope_id))
                    .map(|phase| phase.id.clone())
                    .unwrap_or_else(|| "<unassigned>".to_owned());
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
                    let blocker = format!(
                        "analysis scope {scope_id} is incomplete (decode={}, direct={}, call-graph={}, reference={}, unresolved-calls={})",
                        scope.decode_blockers,
                        scope.direct_blockers,
                        scope.call_graph_blockers,
                        scope.reference_blockers,
                        scope.unresolved_calls,
                    );
                    blockers.push(blocker.clone());
                    phase_blockers
                        .get_mut(&scope_phase)
                        .expect("validated phase owns every selected scope")
                        .push(blocker);
                }
                for transaction in &scope.transactions {
                    let key = transaction.identity.clone();
                    if let Some((previous, _)) = scope_transactions.get(&key)
                        && previous.fingerprint != transaction.fingerprint
                    {
                        blockers.push(format!(
                            "transaction {key} has conflicting fingerprints across selected scopes"
                        ));
                    }
                    scope_transactions
                        .entry(key)
                        .or_insert_with(|| (transaction.clone(), scope_phase.clone()));
                }
            }
            let effect_coverage = match feature.coverage {
                FeatureCoverage::ReviewScopes => {
                    effect_coverage(feature, &scope_transactions)
                }
                FeatureCoverage::BoundedEvidence => {
                    selected_effect_coverage(feature)
                }
                FeatureCoverage::SelectedEvidence => selected_effect_coverage(feature),
                FeatureCoverage::ComposedFeatures => empty_effect_coverage(),
            };
            for (phase, blocker) in &effect_coverage.blockers {
                blockers.push(blocker.clone());
                if let Some(entries) = phase_blockers.get_mut(phase) {
                    entries.push(blocker.clone());
                }
            }
            for requirement in &feature.requirements {
                let (path, report) = suite_reports
                    .get(&requirement.suite)
                    .expect("every requirement suite was cached");
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
                let blocker = match (&report, proof) {
                    (Err(error), _) => Some(format!(
                        "requirement {} cannot load suite {} report {}: {error}",
                        requirement.id, requirement.suite, path.display(),
                    )),
                    (_, None) => Some(format!(
                        "requirement {} has no result in suite {} for {}:{}",
                        requirement.id,
                        requirement.suite,
                        requirement.source,
                        requirement.symbol
                    )),
                    (_, Some(proof)) if !status_satisfies_claim(proof.status, requirement.claim) => {
                        Some(format!(
                            "requirement {} ({}) is {:?} in suite {} for {}:{}",
                            requirement.id,
                            requirement.description,
                            proof.status,
                            requirement.suite,
                            requirement.source,
                            requirement.symbol
                        ))
                    }
                    (_, Some(proof)) if proof.claim != Some(requirement.claim) => {
                        Some(format!(
                            "requirement {} lacks a {:?} claim in suite {} for {}:{}",
                            requirement.id, requirement.claim, requirement.suite, requirement.source, requirement.symbol
                        ))
                    }
                    (_, Some(proof))
                        if !proof.disposition_reviewed || proof.rust_component.is_none() =>
                    {
                        Some(format!(
                            "requirement {} is not bound to a reviewed production component in suite {} for {}:{}",
                            requirement.id, requirement.suite, requirement.source, requirement.symbol
                        ))
                    }
                    (_, Some(_)) => None,
                };
                if let Some(blocker) = blocker {
                    blockers.push(blocker.clone());
                    phase_blockers
                        .get_mut(&requirement.phase)
                        .expect("validated phase exists")
                        .push(blocker);
                }
            }
            blockers.sort();
            blockers.dedup();
            let phases = feature
                .phases
                .iter()
                .map(|phase| {
                    let transactions = effect_coverage
                        .transactions
                        .iter()
                        .filter(|transaction| transaction.phase == phase.id)
                        .count();
                    let covered_transactions = effect_coverage
                        .transactions
                        .iter()
                        .filter(|transaction| transaction.phase == phase.id && transaction.current)
                        .count();
                    let mut blockers = phase_blockers.remove(&phase.id).unwrap_or_default();
                    blockers.sort();
                    blockers.dedup();
                    FeaturePhaseReport {
                        id: phase.id.clone(),
                        description: phase.description.clone(),
                        scopes: phase.scopes.clone(),
                        requirements: feature
                            .requirements
                            .iter()
                            .filter(|requirement| requirement.phase == phase.id)
                            .count(),
                        transactions,
                        covered_transactions,
                        blockers,
                    }
                })
                .collect();
            let hardware = feature.hardware.as_ref().map(|spec| {
                evaluate_hardware(
                    &feature.id,
                    spec,
                    workspace.hardware_evidence.as_deref(),
                )
            });
            let status = if !blockers.is_empty() {
                FeatureQualificationStatus::Blocked
            } else if hardware
                .as_ref()
                .is_some_and(|hardware| hardware.blockers.is_empty())
            {
                FeatureQualificationStatus::HardwareQualified
            } else {
                FeatureQualificationStatus::Qualified
            };
            Ok(FeatureQualificationReport {
                id: feature.id.clone(),
                description: feature.description.clone(),
                required: required.contains(&feature.id),
                status,
                coverage: feature.coverage,
                scopes: feature.scopes.clone(),
                requirements: feature.requirements.len(),
                surface_effects: effect_coverage.surface,
                covered_effects: effect_coverage.covered,
                phases,
                transactions: effect_coverage.transactions,
                dependencies: Vec::new(),
                hardware,
                blockers,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    apply_feature_dependencies(&pack, &mut reports)?;
    Ok(reports)
}

fn apply_feature_dependencies(
    pack: &FeaturePack,
    reports: &mut [FeatureQualificationReport],
) -> Result<()> {
    fn resolve(
        feature_id: &str,
        pack: &FeaturePack,
        reports: &mut [FeatureQualificationReport],
        resolved: &mut BTreeSet<String>,
    ) -> Result<()> {
        if resolved.contains(feature_id) {
            return Ok(());
        }
        let feature = pack
            .feature(feature_id)
            .expect("reports originate from the validated feature pack");
        for dependency in &feature.dependencies {
            resolve(&dependency.feature, pack, reports, resolved)?;
        }

        let mut dependency_reports = Vec::new();
        let mut dependency_blockers = Vec::new();
        for dependency in &feature.dependencies {
            let target = reports
                .iter()
                .find(|report| report.id == dependency.feature)
                .expect("validated dependency has a qualification report");
            let mut blockers = if let Some(phase) = &dependency.phase {
                target
                    .phases
                    .iter()
                    .find(|candidate| candidate.id == *phase)
                    .expect("validated dependency phase has a report")
                    .blockers
                    .clone()
            } else {
                target.blockers.clone()
            };
            for nested in &target.dependencies {
                if nested.status == FeatureQualificationStatus::Blocked {
                    blockers.push(format!(
                        "dependency {}{} is blocked",
                        nested.feature,
                        nested
                            .phase
                            .as_ref()
                            .map_or_else(String::new, |phase| format!(" phase {phase}"))
                    ));
                }
            }
            blockers.sort();
            blockers.dedup();
            let status = if blockers.is_empty() {
                if dependency.phase.is_none() {
                    target.status
                } else if target.status == FeatureQualificationStatus::HardwareQualified {
                    FeatureQualificationStatus::HardwareQualified
                } else {
                    FeatureQualificationStatus::Qualified
                }
            } else {
                FeatureQualificationStatus::Blocked
            };
            if status == FeatureQualificationStatus::Blocked {
                let selector = dependency.phase.as_ref().map_or_else(
                    || dependency.feature.clone(),
                    |phase| format!("{} phase {phase}", dependency.feature),
                );
                dependency_blockers.push(format!(
                    "feature dependency {selector} is blocked ({} blocker(s)); inspect it with `project feature {}`{}",
                    blockers.len(),
                    dependency.feature,
                    dependency.phase.as_ref().map_or_else(String::new, |phase| format!(" --phase {phase}")),
                ));
            }
            dependency_reports.push(FeatureDependencyReport {
                feature: dependency.feature.clone(),
                phase: dependency.phase.clone(),
                status,
                blockers,
            });
        }

        let report = reports
            .iter_mut()
            .find(|report| report.id == feature_id)
            .expect("feature has a qualification report");
        report.dependencies = dependency_reports;
        report.blockers.extend(dependency_blockers);
        report.blockers.sort();
        report.blockers.dedup();
        if !report.blockers.is_empty() {
            report.status = FeatureQualificationStatus::Blocked;
        }
        resolved.insert(feature_id.to_owned());
        Ok(())
    }

    let mut resolved = BTreeSet::new();
    for feature in &pack.features {
        resolve(&feature.id, pack, reports, &mut resolved)?;
    }
    Ok(())
}

pub(crate) fn evidence_for_function(
    project: &ProjectSpec,
    source: &str,
    symbol: &str,
) -> Result<Vec<FunctionQualificationEvidence>> {
    let Some(workspace) = &project.qualification else {
        return Ok(Vec::new());
    };
    let pack = FeaturePack::load(&workspace.pack)?;
    let reports = evaluate(project)?
        .into_iter()
        .map(|report| (report.id.clone(), report))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut evidence = Vec::new();
    for feature in pack.features {
        let requirements = feature
            .requirements
            .iter()
            .filter(|requirement| requirement.source == source && requirement.symbol == symbol)
            .map(|requirement| FunctionQualificationRequirement {
                id: requirement.id.clone(),
                suite: requirement.suite.clone(),
                claim: requirement.claim,
            })
            .collect::<Vec<_>>();
        if requirements.is_empty() {
            continue;
        }
        let report = reports.get(&feature.id).ok_or_else(|| {
            crate::Error::invalid(format!(
                "qualification report is missing feature {:?}",
                feature.id
            ))
        })?;
        evidence.push(FunctionQualificationEvidence {
            feature: feature.id,
            description: feature.description,
            required: report.required,
            status: report.status,
            coverage: feature.coverage,
            requirements,
            blockers: report.blockers.clone(),
        });
    }
    Ok(evidence)
}

struct EffectCoverage {
    surface: usize,
    covered: usize,
    blockers: Vec<(String, String)>,
    transactions: Vec<FeatureTransactionReport>,
}

fn empty_effect_coverage() -> EffectCoverage {
    EffectCoverage {
        surface: 0,
        covered: 0,
        blockers: Vec::new(),
        transactions: Vec::new(),
    }
}

fn effect_coverage(
    feature: &FeatureSpec,
    scope_transactions: &std::collections::BTreeMap<
        String,
        (crate::review_scopes::ReviewScopeTransaction, String),
    >,
) -> EffectCoverage {
    let mut blockers = Vec::new();
    let mut reports = Vec::new();
    let mut matched = BTreeSet::new();
    let mut covered = 0;
    for (transaction, discovered_phase) in scope_transactions.values() {
        let candidates = feature
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
        let Some(effect) = (candidates.len() == 1).then(|| candidates[0]) else {
            let message = if candidates.is_empty() {
                format!(
                    "new transaction {} ({}) has no reviewed disposition; fingerprint={}",
                    transaction.id, transaction.identity, transaction.fingerprint
                )
            } else {
                format!(
                    "transaction {} ({}) matches multiple reviewed dispositions",
                    transaction.id, transaction.identity
                )
            };
            blockers.push((discovered_phase.clone(), message));
            reports.push(FeatureTransactionReport {
                id: transaction.id.clone(),
                phase: discovered_phase.clone(),
                source: transaction.source.clone(),
                symbol: transaction.symbol.clone(),
                identity: Some(transaction.identity.clone()),
                fingerprint: transaction.fingerprint.clone(),
                disposition: "missing".to_owned(),
                requirement: None,
                rationale: String::new(),
                effects: transaction.effects.clone(),
                paths: transaction.paths.clone(),
                current: false,
            });
            continue;
        };
        matched.insert(effect.id.as_str());
        let current = effect.vendor.fingerprint == transaction.fingerprint;
        if current {
            covered += 1;
        } else {
            blockers.push((
                effect.phase.clone(),
                format!(
                    "transaction {} changed: reviewed {}, current {}",
                    effect.id, effect.vendor.fingerprint, transaction.fingerprint
                ),
            ));
        }
        reports.push(transaction_report(effect, transaction, current));
    }
    for effect in &feature.effects {
        if !matched.contains(effect.id.as_str()) {
            blockers.push((
                effect.phase.clone(),
                format!(
                    "feature transaction {} ({}:{}) is stale for the selected scopes",
                    effect.id, effect.vendor.source, effect.vendor.symbol
                ),
            ));
        }
    }
    reports.sort_by(|left, right| (&left.phase, &left.id).cmp(&(&right.phase, &right.id)));
    EffectCoverage {
        surface: scope_transactions.len(),
        covered,
        blockers,
        transactions: reports,
    }
}

fn selected_effect_coverage(feature: &FeatureSpec) -> EffectCoverage {
    EffectCoverage {
        surface: feature.effects.len(),
        covered: feature.effects.len(),
        blockers: Vec::new(),
        transactions: feature
            .effects
            .iter()
            .map(|effect| FeatureTransactionReport {
                id: effect.id.clone(),
                phase: effect.phase.clone(),
                source: effect.vendor.source.clone(),
                symbol: effect.vendor.symbol.clone(),
                identity: effect.vendor.identity.clone(),
                fingerprint: effect.vendor.fingerprint.clone(),
                disposition: effect.disposition.as_str().to_owned(),
                requirement: effect.requirement.clone(),
                rationale: effect.rationale.clone(),
                effects: Vec::new(),
                paths: Vec::new(),
                current: true,
            })
            .collect(),
    }
}

fn transaction_report(
    effect: &FeatureEffectDisposition,
    transaction: &crate::review_scopes::ReviewScopeTransaction,
    current: bool,
) -> FeatureTransactionReport {
    FeatureTransactionReport {
        id: effect.id.clone(),
        phase: effect.phase.clone(),
        source: transaction.source.clone(),
        symbol: transaction.symbol.clone(),
        identity: Some(transaction.identity.clone()),
        fingerprint: transaction.fingerprint.clone(),
        disposition: effect.disposition.as_str().to_owned(),
        requirement: effect.requirement.clone(),
        rationale: effect.rationale.clone(),
        effects: transaction.effects.clone(),
        paths: transaction.paths.clone(),
        current,
    }
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

fn validate_phase(feature: &str, phase: &str, phases: &BTreeSet<&str>, kind: &str) -> Result<()> {
    if !phases.contains(phase) {
        return Err(crate::Error::invalid(format!(
            "qualification feature {feature:?} {kind} refers to unknown phase {phase:?}"
        )));
    }
    Ok(())
}

fn validate_transaction_fingerprint(value: &str) -> Result<()> {
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

fn validate_unique_nonempty(feature: &str, kind: &str, values: &[String]) -> Result<()> {
    if values.is_empty() {
        return Err(crate::Error::invalid(format!(
            "qualification feature {feature:?} requires at least one {kind}"
        )));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() || !unique.insert(value) {
            return Err(crate::Error::invalid(format!(
                "qualification feature {feature:?} has an empty or duplicate {kind} {value:?}"
            )));
        }
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
    fn whole_function_claim_rejects_a_reviewed_projection_status() {
        assert!(!status_satisfies_claim(
            FunctionVerificationStatus::ImplementedUnqualified,
            DriverAdapterClaim::WholeFunctionEquivalence,
        ));
    }

    #[test]
    fn reviewed_projection_accepts_only_completed_evidence() {
        assert!(status_satisfies_claim(
            FunctionVerificationStatus::BoundedMatch,
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
            coverage: FeatureCoverage::ReviewScopes,
            scopes: vec!["connected".to_owned()],
            phases: vec![FeaturePhase {
                id: "policy".to_owned(),
                description: "fixture policy".to_owned(),
                scopes: vec!["connected".to_owned()],
            }],
            requirements: Vec::new(),
            effects,
            dependencies: Vec::new(),
            hardware: None,
        }
    }

    fn excluded(source: &str, symbol: &str) -> FeatureEffectDisposition {
        FeatureEffectDisposition {
            id: symbol.replace('_', "-"),
            phase: "policy".to_owned(),
            vendor: FeatureVendorTransaction {
                source: source.to_owned(),
                symbol: symbol.to_owned(),
                identity: None,
                fingerprint: format!("sha256:{}", "0".repeat(64)),
            },
            disposition: FeatureEffectDispositionKind::ExcludedByFeaturePolicy,
            requirement: None,
            rationale: "explicit feature policy".to_owned(),
        }
    }

    fn bounded_feature() -> FeatureSpec {
        FeatureSpec {
            id: "key-role".to_owned(),
            description: "bounded property fixture".to_owned(),
            coverage: FeatureCoverage::BoundedEvidence,
            scopes: Vec::new(),
            phases: vec![FeaturePhase {
                id: "install".to_owned(),
                description: "fixture install".to_owned(),
                scopes: Vec::new(),
            }],
            requirements: vec![FeatureRequirement {
                id: "role-proof".to_owned(),
                phase: "install".to_owned(),
                description: "executed role proof".to_owned(),
                suite: "keys".to_owned(),
                source: "wifi".to_owned(),
                symbol: "insert_key".to_owned(),
                claim: DriverAdapterClaim::RustConformance,
            }],
            effects: vec![FeatureEffectDisposition {
                id: "connection-context".to_owned(),
                phase: "install".to_owned(),
                vendor: FeatureVendorTransaction {
                    source: "wifi".to_owned(),
                    symbol: "insert_key".to_owned(),
                    identity: None,
                    fingerprint: format!("sha256:{}", "1".repeat(64)),
                },
                disposition: FeatureEffectDispositionKind::Verified,
                requirement: Some("role-proof".to_owned()),
                rationale: "the exact property is replayed".to_owned(),
            }],
            dependencies: Vec::new(),
            hardware: None,
        }
    }

    #[test]
    fn bounded_evidence_is_an_explicit_property_not_an_incomplete_review_scope() {
        let pack = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![bounded_feature()],
        };

        pack.validate(Path::new("features.toml")).unwrap();
    }

    #[test]
    fn selected_evidence_accepts_a_whole_function_leaf() {
        let mut feature = bounded_feature();
        feature.coverage = FeatureCoverage::SelectedEvidence;
        feature.requirements[0].claim = DriverAdapterClaim::WholeFunctionEquivalence;
        let pack = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![feature],
        };

        pack.validate(Path::new("features.toml")).unwrap();
    }

    #[test]
    fn composed_features_require_valid_acyclic_dependencies() {
        let leaf = feature_with_effects(vec![excluded("wifi", "leaf")]);
        let composed = FeatureSpec {
            id: "runtime".to_owned(),
            description: "composed runtime fixture".to_owned(),
            coverage: FeatureCoverage::ComposedFeatures,
            scopes: Vec::new(),
            phases: vec![FeaturePhase {
                id: "prerequisites".to_owned(),
                description: "fixture dependencies".to_owned(),
                scopes: Vec::new(),
            }],
            requirements: Vec::new(),
            effects: Vec::new(),
            dependencies: vec![FeatureDependency {
                feature: leaf.id.clone(),
                phase: Some("policy".to_owned()),
            }],
            hardware: None,
        };
        FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![leaf.clone(), composed.clone()],
        }
        .validate(Path::new("features.toml"))
        .unwrap();

        let mut cyclic_leaf = leaf;
        cyclic_leaf.dependencies.push(FeatureDependency {
            feature: composed.id.clone(),
            phase: None,
        });
        let error = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![cyclic_leaf, composed],
        }
        .validate(Path::new("features.toml"))
        .unwrap_err();
        assert!(error.to_string().contains("dependency cycle"));
    }

    #[test]
    fn required_feature_dependency_closure_selects_bounded_leaves() {
        let leaf = bounded_feature();
        let composed = FeatureSpec {
            id: "radio".to_owned(),
            description: "top-level radio contract".to_owned(),
            coverage: FeatureCoverage::ComposedFeatures,
            scopes: Vec::new(),
            phases: vec![FeaturePhase {
                id: "static".to_owned(),
                description: "static contract".to_owned(),
                scopes: Vec::new(),
            }],
            requirements: Vec::new(),
            effects: Vec::new(),
            dependencies: vec![FeatureDependency {
                feature: leaf.id.clone(),
                phase: None,
            }],
            hardware: None,
        };
        let pack = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![leaf, composed],
        };
        pack.validate(Path::new("features.toml")).unwrap();

        assert_eq!(
            pack.dependency_closure(["radio"]),
            BTreeSet::from(["key-role", "radio"])
        );
    }

    #[test]
    fn required_publication_scopes_follow_selected_dependency_phases() {
        let mut leaf = feature_with_effects(vec![excluded("wifi", "leaf")]);
        leaf.scopes.push("power-save".to_owned());
        leaf.phases.push(FeaturePhase {
            id: "power-save".to_owned(),
            description: "excluded power-save policy".to_owned(),
            scopes: vec!["power-save".to_owned()],
        });
        let composed = FeatureSpec {
            id: "radio".to_owned(),
            description: "phase-selected radio contract".to_owned(),
            coverage: FeatureCoverage::ComposedFeatures,
            scopes: Vec::new(),
            phases: vec![FeaturePhase {
                id: "static".to_owned(),
                description: "static contract".to_owned(),
                scopes: Vec::new(),
            }],
            requirements: Vec::new(),
            effects: Vec::new(),
            dependencies: vec![FeatureDependency {
                feature: leaf.id.clone(),
                phase: Some("policy".to_owned()),
            }],
            hardware: None,
        };
        let pack = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![leaf, composed],
        };
        pack.validate(Path::new("features.toml")).unwrap();

        assert_eq!(
            pack.required_review_scopes(["radio"]),
            BTreeSet::from(["connected".to_owned()])
        );
        assert_eq!(
            pack.required_review_scopes(["sta"]),
            BTreeSet::from(["connected".to_owned(), "power-save".to_owned()])
        );
    }

    #[test]
    fn composed_feature_reports_dependency_without_copying_leaf_blockers() {
        let leaf = feature_with_effects(vec![excluded("wifi", "leaf")]);
        let composed = FeatureSpec {
            id: "runtime".to_owned(),
            description: "composed runtime fixture".to_owned(),
            coverage: FeatureCoverage::ComposedFeatures,
            scopes: Vec::new(),
            phases: vec![FeaturePhase {
                id: "prerequisites".to_owned(),
                description: "fixture dependencies".to_owned(),
                scopes: Vec::new(),
            }],
            requirements: Vec::new(),
            effects: Vec::new(),
            dependencies: vec![FeatureDependency {
                feature: leaf.id.clone(),
                phase: Some("policy".to_owned()),
            }],
            hardware: None,
        };
        let pack = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![leaf, composed],
        };
        let mut reports = vec![
            FeatureQualificationReport {
                id: "sta".to_owned(),
                description: "leaf".to_owned(),
                required: true,
                status: FeatureQualificationStatus::Blocked,
                coverage: FeatureCoverage::ReviewScopes,
                scopes: vec!["connected".to_owned()],
                requirements: 0,
                surface_effects: 1,
                covered_effects: 0,
                phases: vec![FeaturePhaseReport {
                    id: "policy".to_owned(),
                    description: "policy".to_owned(),
                    scopes: vec!["connected".to_owned()],
                    requirements: 0,
                    transactions: 1,
                    covered_transactions: 0,
                    blockers: vec!["first".to_owned(), "second".to_owned()],
                }],
                transactions: Vec::new(),
                dependencies: Vec::new(),
                hardware: None,
                blockers: vec!["first".to_owned(), "second".to_owned()],
            },
            FeatureQualificationReport {
                id: "runtime".to_owned(),
                description: "runtime".to_owned(),
                required: true,
                status: FeatureQualificationStatus::Qualified,
                coverage: FeatureCoverage::ComposedFeatures,
                scopes: Vec::new(),
                requirements: 0,
                surface_effects: 0,
                covered_effects: 0,
                phases: vec![FeaturePhaseReport {
                    id: "prerequisites".to_owned(),
                    description: "prerequisites".to_owned(),
                    scopes: Vec::new(),
                    requirements: 0,
                    transactions: 0,
                    covered_transactions: 0,
                    blockers: Vec::new(),
                }],
                transactions: Vec::new(),
                dependencies: Vec::new(),
                hardware: None,
                blockers: Vec::new(),
            },
        ];

        apply_feature_dependencies(&pack, &mut reports).unwrap();

        assert_eq!(reports[1].status, FeatureQualificationStatus::Blocked);
        assert_eq!(reports[1].dependencies[0].blockers.len(), 2);
        assert_eq!(reports[1].blockers.len(), 1);
    }

    #[test]
    fn bounded_evidence_rejects_a_review_scope_or_policy_exclusion() {
        let mut with_scope = bounded_feature();
        with_scope.scopes.push("whole-driver".to_owned());
        let error = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![with_scope],
        }
        .validate(Path::new("features.toml"))
        .unwrap_err();
        assert!(error.to_string().contains("must not select review scopes"));

        let mut excluded_effect = bounded_feature();
        excluded_effect.effects[0].disposition =
            FeatureEffectDispositionKind::ExcludedByFeaturePolicy;
        excluded_effect.effects[0].requirement = None;
        let error = FeaturePack {
            schema: FEATURE_PACK_SCHEMA,
            features: vec![excluded_effect],
        }
        .validate(Path::new("features.toml"))
        .unwrap_err();
        assert!(error.to_string().contains("cannot exclude effect"));
    }

    #[test]
    fn missing_vendor_transaction_is_a_qualification_blocker() {
        let scope_effects =
            transaction_map(&["hal_set_sta_beacon_filter", "hal_disable_sta_beacon_filter"]);
        let feature = feature_with_effects(vec![excluded("wifi", "hal_disable_sta_beacon_filter")]);

        let coverage = effect_coverage(&feature, &scope_effects);

        assert_eq!(coverage.covered, 1);
        assert_eq!(coverage.blockers.len(), 1);
        assert!(
            coverage.blockers[0]
                .1
                .contains("wifi:hal_set_sta_beacon_filter")
        );
        assert!(coverage.blockers[0].1.contains("no reviewed disposition"));
    }

    #[test]
    fn stale_vendor_transaction_disposition_is_a_qualification_blocker() {
        let scope_effects = transaction_map(&["hal_disable_sta_beacon_filter"]);
        let feature = feature_with_effects(vec![
            excluded("wifi", "hal_disable_sta_beacon_filter"),
            excluded("wifi", "removed_transaction"),
        ]);

        let coverage = effect_coverage(&feature, &scope_effects);

        assert_eq!(coverage.covered, 1);
        assert_eq!(coverage.blockers.len(), 1);
        assert!(coverage.blockers[0].1.contains("wifi:removed_transaction"));
        assert!(coverage.blockers[0].1.contains("stale"));
    }

    #[test]
    fn changed_transaction_fingerprint_requires_new_review() {
        let scope_effects = transaction_map(&["hal_disable_sta_beacon_filter"]);
        let mut disposition = excluded("wifi", "hal_disable_sta_beacon_filter");
        disposition.vendor.fingerprint = format!("sha256:{}", "1".repeat(64));

        let coverage = effect_coverage(&feature_with_effects(vec![disposition]), &scope_effects);

        assert_eq!(coverage.covered, 0);
        assert_eq!(coverage.blockers.len(), 1);
        assert!(coverage.blockers[0].1.contains("changed"));
    }

    #[test]
    fn hardware_evidence_requires_current_artifacts_and_all_observations() {
        let directory = std::env::temp_dir().join(format!(
            "vendor-workbench-hardware-evidence-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let artifact = directory.join("firmware.bin");
        std::fs::write(&artifact, b"firmware-v1").unwrap();
        let digest = crate::artifact_path_sha256(&artifact).unwrap();
        let evidence = directory.join("evidence.json");
        std::fs::write(
            &evidence,
            serde_json::json!({
                "schema": 1,
                "command": "project hardware evidence",
                "features": [{
                    "id": "wifi-ap",
                    "passed": true,
                    "successful_runs": 20,
                    "observations": ["beacon", "association"],
                    "artifacts": [{
                        "id": "firmware",
                        "path": "firmware.bin",
                        "sha256": digest,
                    }],
                }],
            })
            .to_string(),
        )
        .unwrap();
        let spec = FeatureHardwareSpec {
            minimum_successful_runs: 20,
            required_observations: vec!["beacon".to_owned(), "association".to_owned()],
            required_artifacts: vec!["firmware".to_owned()],
        };

        let current = evaluate_hardware("wifi-ap", &spec, Some(&evidence));
        assert_eq!(current.status, "passed");
        assert!(current.blockers.is_empty());

        std::fs::write(&artifact, b"firmware-v2").unwrap();
        let stale = evaluate_hardware("wifi-ap", &spec, Some(&evidence));
        assert_eq!(stale.status, "stale");
        assert!(stale.blockers[0].contains("firmware"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn transaction_map(
        symbols: &[&str],
    ) -> std::collections::BTreeMap<String, (crate::review_scopes::ReviewScopeTransaction, String)>
    {
        symbols
            .iter()
            .map(|symbol| {
                let identity = format!("wifi::{symbol}");
                (
                    identity.clone(),
                    (
                        crate::review_scopes::ReviewScopeTransaction {
                            id: format!("wifi:{symbol}"),
                            identity,
                            source: "wifi".to_owned(),
                            symbol: (*symbol).to_owned(),
                            fingerprint: format!("sha256:{}", "0".repeat(64)),
                            paths: Vec::new(),
                            effects: Vec::new(),
                        },
                        "policy".to_owned(),
                    ),
                )
            })
            .collect()
    }
}
