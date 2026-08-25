//! Deterministic, read-only planning for splitting a mixed register model.
//!
//! The planner never promotes a software access pattern to hardware truth. It
//! classifies existing provenance, proves which sparse assertions are already
//! duplicated in the base model, and reports semantic properties that still
//! need an explicit reviewed assertion before they can be removed from the
//! base. Applying the plan remains a review step.

use std::collections::{BTreeMap, BTreeSet};

use open_radio_vendor_contracts::{FactAccuracy, FactCompleteness, FactProvenance};
use open_radio_vendor_review::{
    Applicability, AssertionValue, EffectiveAssertion, EvidenceReference, FactClassification,
    ReviewKnowledge,
};
use serde::Serialize;
use svd_rs::{
    Access, Device, MaybeArray, ModifiedWriteValues, RegisterCluster, RegisterProperties,
};

use super::{
    Error, RegisterEditOutcome, RegisterModel, RegisterSubject, Result, apply_register_assertion,
    edit_register, merge_properties,
};

pub const REGISTER_MIGRATION_PLAN_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterMigrationPlan {
    pub schema: u32,
    pub address_space: String,
    pub review_fingerprint: String,
    pub overlay_changes_effective_output: bool,
    pub summary: RegisterMigrationSummary,
    pub base_entities: Vec<RegisterMigrationBaseEntity>,
    pub assertions: Vec<RegisterMigrationAssertion>,
    pub diagnostics: Vec<RegisterMigrationDiagnostic>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterMigrationSummary {
    pub imported_base: usize,
    pub generated_base_candidates: usize,
    pub base_review_required: usize,
    pub embedded_reviewed_facts: usize,
    pub sparse_reviewed_facts: usize,
    pub ignored_non_register_facts: usize,
    pub targeted_diagnostics_requiring_review: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterMigrationBaseDisposition {
    ImportedBase,
    GeneratedBaseCandidate,
    ReviewRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterMigrationBaseEntity {
    pub entity: String,
    pub disposition: RegisterMigrationBaseDisposition,
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<FactProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<FactAccuracy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completeness: Option<FactCompleteness>,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterMigrationAssertionDisposition {
    /// The reviewed assertion already equals the base property. It may be
    /// extracted only by replacing/removing the base property and proving the
    /// recomposed effective SVD is unchanged.
    EmbeddedReviewedFact,
    /// The sparse assertion currently changes or materializes the base model.
    SparseReviewedFact,
    /// The fact is outside the model's address space or register vocabulary.
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterMigrationAssertion {
    pub pack: String,
    pub id: String,
    pub subject: String,
    pub kind: String,
    pub value: AssertionValue,
    pub disposition: RegisterMigrationAssertionDisposition,
    pub classification: FactClassification,
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterMigrationDiagnostic {
    pub subject: String,
    pub kind: String,
    pub current: String,
    pub reason: String,
}

impl RegisterModel {
    /// Build a deterministic migration plan without changing files or this
    /// model. The complete overlay is first applied to a clone, so invalid or
    /// conflicting review knowledge fails closed before any item is reported
    /// as extractable.
    pub fn plan_review_migration(
        &self,
        knowledge: &ReviewKnowledge,
    ) -> Result<RegisterMigrationPlan> {
        let base_svd = self.render_svd()?.0;
        let mut effective = self.clone();
        effective.apply_review_knowledge(knowledge)?;
        let effective_svd = effective.render_svd()?.0;

        let mut summary = RegisterMigrationSummary::default();
        let mut base_entities = self
            .review
            .iter()
            .map(|annotation| {
                let (disposition, reason) = classify_base_annotation(annotation);
                match disposition {
                    RegisterMigrationBaseDisposition::ImportedBase => summary.imported_base += 1,
                    RegisterMigrationBaseDisposition::GeneratedBaseCandidate => {
                        summary.generated_base_candidates += 1;
                    }
                    RegisterMigrationBaseDisposition::ReviewRequired => {
                        summary.base_review_required += 1;
                    }
                }
                RegisterMigrationBaseEntity {
                    entity: annotation.entity.clone(),
                    disposition,
                    sources: annotation.sources.clone(),
                    provenance: annotation.provenance,
                    accuracy: annotation.accuracy,
                    completeness: annotation.completeness,
                    reason,
                }
            })
            .collect::<Vec<_>>();
        let annotated = base_entities
            .iter()
            .map(|entity| entity.entity.clone())
            .collect::<BTreeSet<_>>();
        let mut known_entities = BTreeSet::new();
        for peripheral in &self.device.peripherals {
            super::collect_review_entities(peripheral, &mut known_entities);
        }
        for entity in known_entities {
            if annotated.contains(entity.as_str()) {
                continue;
            }
            summary.base_review_required += 1;
            base_entities.push(RegisterMigrationBaseEntity {
                entity,
                disposition: RegisterMigrationBaseDisposition::ReviewRequired,
                sources: Vec::new(),
                provenance: None,
                accuracy: None,
                completeness: None,
                reason: "base entity has no provenance annotation and cannot be assigned to generated or imported ownership"
                    .to_owned(),
            });
        }
        base_entities.sort_by(|left, right| left.entity.cmp(&right.entity));

        let mut assertions = Vec::with_capacity(knowledge.assertions().len());
        for assertion in knowledge.assertions().values() {
            let (disposition, reason) = self.classify_sparse_assertion(assertion)?;
            match disposition {
                RegisterMigrationAssertionDisposition::EmbeddedReviewedFact => {
                    summary.embedded_reviewed_facts += 1;
                }
                RegisterMigrationAssertionDisposition::SparseReviewedFact => {
                    summary.sparse_reviewed_facts += 1;
                }
                RegisterMigrationAssertionDisposition::Ignored => {
                    summary.ignored_non_register_facts += 1;
                }
            }
            assertions.push(RegisterMigrationAssertion {
                pack: assertion.pack.clone(),
                id: assertion.id.clone(),
                subject: assertion.subject.clone(),
                kind: assertion.kind.clone(),
                value: assertion.value.clone(),
                disposition,
                classification: assertion.classification,
                applies_to: assertion.applies_to.clone(),
                evidence: assertion.evidence.clone(),
                note: assertion.note.clone(),
                reason,
            });
        }

        let diagnostics = self.semantic_migration_diagnostics(knowledge)?;
        summary.targeted_diagnostics_requiring_review = diagnostics.len();
        Ok(RegisterMigrationPlan {
            schema: REGISTER_MIGRATION_PLAN_SCHEMA,
            address_space: self.address_space.clone(),
            review_fingerprint: knowledge.semantic_fingerprint(),
            overlay_changes_effective_output: base_svd != effective_svd,
            summary,
            base_entities,
            assertions,
            diagnostics,
        })
    }

    fn classify_sparse_assertion(
        &self,
        assertion: &EffectiveAssertion,
    ) -> Result<(RegisterMigrationAssertionDisposition, String)> {
        if !super::is_register_assertion(&assertion.kind) {
            return Ok((
                RegisterMigrationAssertionDisposition::Ignored,
                "non-register reviewed knowledge remains outside the register migration".to_owned(),
            ));
        }
        let subject = super::parse_assertion_subject(assertion)?;
        if subject.address_space != self.address_space {
            return Ok((
                RegisterMigrationAssertionDisposition::Ignored,
                format!(
                    "assertion belongs to address space {:?}, not {:?}",
                    subject.address_space, self.address_space
                ),
            ));
        }
        if assertion.kind == "register-declaration" {
            return Ok((
                RegisterMigrationAssertionDisposition::SparseReviewedFact,
                "reviewed declaration materializes geometry absent from the base; keep it sparse"
                    .to_owned(),
            ));
        }

        let mut staged = self.clone();
        let mut changed = false;
        let outcome = edit_register(
            &mut staged.device,
            subject.address,
            subject.width,
            &mut |register| {
                changed = apply_register_assertion(register, &subject, assertion)?;
                Ok(())
            },
        )?;
        match outcome {
            RegisterEditOutcome::Edited if changed => Ok((
                RegisterMigrationAssertionDisposition::SparseReviewedFact,
                "assertion changes the reusable base and is already correctly separated"
                    .to_owned(),
            )),
            RegisterEditOutcome::Edited => Ok((
                RegisterMigrationAssertionDisposition::EmbeddedReviewedFact,
                "base duplicates this reviewed fact; extract only with an unchanged effective-output check"
                    .to_owned(),
            )),
            RegisterEditOutcome::ArrayInstance => Err(Error::message(format!(
                "reviewed assertion {:?} targets one expanded array instance; migration cannot guess array-wide ownership",
                assertion.id
            ))),
            RegisterEditOutcome::NotFound => Err(Error::message(format!(
                "reviewed assertion {:?} targets absent register {:#010x}/{}",
                assertion.id, subject.address, subject.width
            ))),
        }
    }

    fn semantic_migration_diagnostics(
        &self,
        knowledge: &ReviewKnowledge,
    ) -> Result<Vec<RegisterMigrationDiagnostic>> {
        let coverage = knowledge
            .assertions()
            .values()
            .filter(|assertion| super::is_register_assertion(&assertion.kind))
            .map(|assertion| (assertion.subject.as_str(), assertion.kind.as_str()))
            .collect::<BTreeSet<_>>();
        let identities = self.register_identities()?;
        let mut subjects = BTreeMap::<String, RegisterSubject>::new();
        for assertion in knowledge.assertions().values() {
            if !super::is_register_assertion(&assertion.kind) {
                continue;
            }
            let subject = super::parse_assertion_subject(assertion)?;
            if subject.address_space == self.address_space
                && subject.field.is_none()
                && assertion.kind != "register-declaration"
            {
                subjects.insert(assertion.subject.clone(), subject);
            }
        }

        let mut diagnostics = Vec::new();
        for (subject_text, subject) in subjects {
            let Some(_identity) = identities.get(&(subject.address, subject.width)) else {
                continue;
            };
            let Some(view) = inspect_register(&self.device, subject.address, subject.width)? else {
                continue;
            };
            push_uncovered(
                &mut diagnostics,
                &coverage,
                &subject_text,
                "register-name",
                view.name,
            );
            if let Some(description) = view.description {
                push_uncovered(
                    &mut diagnostics,
                    &coverage,
                    &subject_text,
                    "register-description",
                    description,
                );
            }
            if let Some(access) = view.access {
                push_uncovered(
                    &mut diagnostics,
                    &coverage,
                    &subject_text,
                    "register-access",
                    access.as_str().to_owned(),
                );
            }
            if let Some(semantics) = view.modified_write_values {
                push_uncovered(
                    &mut diagnostics,
                    &coverage,
                    &subject_text,
                    "hardware-write-semantics",
                    semantics.as_str().to_owned(),
                );
            }
        }
        diagnostics
            .sort_by(|left, right| (&left.subject, &left.kind).cmp(&(&right.subject, &right.kind)));
        diagnostics
            .dedup_by(|left, right| left.subject == right.subject && left.kind == right.kind);
        Ok(diagnostics)
    }
}

fn classify_base_annotation(
    annotation: &super::ReviewAnnotation,
) -> (RegisterMigrationBaseDisposition, String) {
    match (annotation.provenance, annotation.accuracy) {
        (Some(FactProvenance::Imported), Some(FactAccuracy::Exact)) => (
            RegisterMigrationBaseDisposition::ImportedBase,
            "exact imported origin may remain reproducible base data; completeness still bounds which properties it proves"
                .to_owned(),
        ),
        (Some(FactProvenance::Observed), Some(FactAccuracy::Exact)) => (
            RegisterMigrationBaseDisposition::GeneratedBaseCandidate,
            "exact observation is a generated-base candidate for geometry only; it does not prove names, access or write semantics"
                .to_owned(),
        ),
        _ => (
            RegisterMigrationBaseDisposition::ReviewRequired,
            "derived, hinted, reviewed, approximate or unclassified base ownership cannot be migrated automatically"
                .to_owned(),
        ),
    }
}

fn push_uncovered(
    output: &mut Vec<RegisterMigrationDiagnostic>,
    coverage: &BTreeSet<(&str, &str)>,
    subject: &str,
    kind: &str,
    current: String,
) {
    if !coverage.contains(&(subject, kind)) {
        output.push(RegisterMigrationDiagnostic {
            subject: subject.to_owned(),
            kind: kind.to_owned(),
            current,
            reason: "base property is not backed by an exact sparse assertion; retain it until evidence is reviewed, and never infer it from software reads/writes"
                .to_owned(),
        });
    }
}

#[derive(Clone, Debug)]
struct RegisterView {
    name: String,
    description: Option<String>,
    access: Option<Access>,
    modified_write_values: Option<ModifiedWriteValues>,
}

fn inspect_register(device: &Device, address: u64, width: u32) -> Result<Option<RegisterView>> {
    for peripheral in &device.peripherals {
        let MaybeArray::Single(peripheral) = peripheral else {
            continue;
        };
        let inherited = merge_properties(
            device.default_register_properties,
            peripheral.default_register_properties,
        );
        if let Some(view) = inspect_children(
            peripheral.base_address,
            0,
            inherited,
            peripheral.registers.as_deref().unwrap_or_default(),
            address,
            width,
        )? {
            return Ok(Some(view));
        }
    }
    Ok(None)
}

fn inspect_children(
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    children: &[RegisterCluster],
    address: u64,
    width: u32,
) -> Result<Option<RegisterView>> {
    for child in children {
        match child {
            RegisterCluster::Register(MaybeArray::Single(register)) => {
                let properties = merge_properties(inherited, register.properties);
                if super::physical_match(
                    peripheral_base,
                    parent_offset + u64::from(register.address_offset),
                    properties,
                    address,
                    width,
                ) {
                    return Ok(Some(RegisterView {
                        name: register.name.clone(),
                        description: register.description.clone(),
                        access: properties.access,
                        modified_write_values: register.modified_write_values,
                    }));
                }
            }
            RegisterCluster::Register(MaybeArray::Array(register, dim)) => {
                let properties = merge_properties(inherited, register.properties);
                if (0..dim.dim).any(|index| {
                    super::physical_match(
                        peripheral_base,
                        parent_offset
                            + u64::from(register.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        address,
                        width,
                    )
                }) {
                    return Err(Error::message(
                        "migration inspection cannot target one expanded register-array instance",
                    ));
                }
            }
            RegisterCluster::Cluster(MaybeArray::Single(cluster)) => {
                if let Some(view) = inspect_children(
                    peripheral_base,
                    parent_offset + u64::from(cluster.address_offset),
                    merge_properties(inherited, cluster.default_register_properties),
                    &cluster.children,
                    address,
                    width,
                )? {
                    return Ok(Some(view));
                }
            }
            RegisterCluster::Cluster(MaybeArray::Array(cluster, dim)) => {
                let properties = merge_properties(inherited, cluster.default_register_properties);
                if (0..dim.dim).any(|index| {
                    super::contains_register(
                        peripheral_base,
                        parent_offset
                            + u64::from(cluster.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        &cluster.children,
                        address,
                        width,
                    )
                }) {
                    return Err(Error::message(
                        "migration inspection cannot target one expanded cluster-array instance",
                    ));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use open_radio_vendor_review::ReviewPack;

    use super::*;

    fn fixture(name: &str) -> (PathBuf, RegisterModel, ReviewKnowledge) {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-register-migration-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(directory.join("peripherals")).unwrap();
        fs::write(
            directory.join("device.toml"),
            r#"schema = 2
address-space = "cpu"
fragments = ["peripherals/radio.toml"]

[device]
name = "FIXTURE"
version = "1"
description = "fixture"
address-unit-bits = 8
width = 32
svd-schema = "1.3"
svd-schema-location = "CMSIS-SVD.xsd"
"#,
        )
        .unwrap();
        fs::write(
            directory.join("peripherals/radio.toml"),
            r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000

[[peripherals.registers]]
[peripherals.registers.register]
name = "EVENT_STATUS"
description = "Unreviewed access-class description"
addressOffset = 0x10
size = 32
access = "read-only"

[[review]]
entity = "RADIO.EVENT_STATUS"
sources = ["VENDOR_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "partial"
"#,
        )
        .unwrap();
        let pack = ReviewPack::from_toml(
            r#"schema = 1
id = "fixture.registers"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
chips = ["fixture"]

[[assertions]]
id = "fixture.event-status.name"
subject = "mmio:cpu:0x1010/32"
kind = "register-name"
value = "EVENT_STATUS"
[[assertions.evidence]]
source = "VENDOR_HEADER"
locator = "EVENT_STATUS_REG"

[[assertions]]
id = "fixture.event-status.write-semantics"
subject = "mmio:cpu:0x1010/32"
kind = "hardware-write-semantics"
value = "unknown"
[[assertions.evidence]]
source = "VENDOR_ACCESSOR"
locator = "clear_events"
"#,
        )
        .unwrap();
        let model = RegisterModel::load(&directory.join("device.toml")).unwrap();
        let knowledge = ReviewKnowledge::merge([pack]).unwrap();
        (directory, model, knowledge)
    }

    #[test]
    fn plan_separates_extractable_duplicates_from_unreviewed_semantics() {
        let (directory, model, knowledge) = fixture("classification");
        let plan = model.plan_review_migration(&knowledge).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert!(!plan.overlay_changes_effective_output);
        assert_eq!(plan.summary.imported_base, 1);
        assert_eq!(plan.summary.embedded_reviewed_facts, 2);
        assert_eq!(plan.summary.sparse_reviewed_facts, 0);
        assert_eq!(plan.summary.targeted_diagnostics_requiring_review, 2);
        assert!(plan.assertions.iter().all(|assertion| {
            assertion.disposition == RegisterMigrationAssertionDisposition::EmbeddedReviewedFact
        }));
        assert!(plan.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "register-access" && diagnostic.current == "read-only"
        }));
        assert!(
            plan.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "register-description")
        );
        assert!(
            plan.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.kind != "hardware-write-semantics")
        );
        let name = plan
            .assertions
            .iter()
            .find(|assertion| assertion.kind == "register-name")
            .unwrap();
        assert_eq!(name.classification.provenance, FactProvenance::Reviewed);
        assert_eq!(name.applies_to.chips, ["fixture"]);
        assert_eq!(name.evidence[0].source, "VENDOR_HEADER");
    }

    #[test]
    fn plan_is_deterministic_and_read_only() {
        let (directory, model, knowledge) = fixture("deterministic");
        let before = model.render_svd().unwrap().0;
        let first = model.plan_review_migration(&knowledge).unwrap();
        let second = model.plan_review_migration(&knowledge).unwrap();
        let first = toml_edit::ser::to_string_pretty(&first).unwrap();
        let second = toml_edit::ser::to_string_pretty(&second).unwrap();
        let after = model.render_svd().unwrap().0;
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(first, second);
        assert_eq!(before, after);
    }

    #[test]
    fn extracting_one_name_to_an_overlay_preserves_the_effective_svd() {
        let (directory, original, knowledge) = fixture("extract-name");
        let original_svd = original.render_svd().unwrap().0;
        let fragment = directory.join("peripherals/radio.toml");
        let input = fs::read_to_string(&fragment).unwrap();
        fs::write(&fragment, input.replace("EVENT_STATUS", "WORD_010_BASE")).unwrap();

        let migrated = RegisterModel::load(&directory.join("device.toml")).unwrap();
        let plan = migrated.plan_review_migration(&knowledge).unwrap();
        let mut effective = migrated;
        effective.apply_review_knowledge(&knowledge).unwrap();
        let migrated_svd = effective.render_svd().unwrap().0;
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(original_svd, migrated_svd);
        assert!(plan.overlay_changes_effective_output);
        let name = plan
            .assertions
            .iter()
            .find(|assertion| assertion.kind == "register-name")
            .unwrap();
        assert_eq!(
            name.disposition,
            RegisterMigrationAssertionDisposition::SparseReviewedFact
        );
        assert!(plan.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "register-access" && diagnostic.current == "read-only"
        }));
    }
}
