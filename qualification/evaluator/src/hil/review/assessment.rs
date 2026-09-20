//! A review is applicable only while every property, build and resolution binding holds.
use super::*;

pub(super) fn assess(
    root: &Path,
    review: &Document,
    binding: &PropertyBinding,
    requirement: &HilRequirement,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) -> Result<&'static str> {
    if review.property_sha256 != binding.sha256
        && review.property_sha256 != binding.legacy_sha256
        && review.property_sha256 != binding.previous_sha256
    {
        return Ok("property-changed");
    }
    if !binding.unmapped_capabilities.is_empty() {
        return Ok("implementation-owners-not-mapped");
    }
    if !binding.required_inputs.iter().all(|path| {
        (!review.dependency_roots.is_empty() && binding.implicit_build_inputs.contains(path))
            || review
                .inputs
                .iter()
                .any(|i| &i.path == path && i.kind != InputKind::Evidence)
    }) {
        return Ok("owner-bindings-incomplete");
    }
    for input in &review.inputs {
        if input.kind == InputKind::Evidence {
            continue;
        }
        if !root.join(&input.path).try_exists()? {
            return Ok("current-input-missing");
        }
        regular(root, &input.path)?;
        if input_hash(&fs::read(root.join(&input.path))?, &input.kind)? != input.sha256 {
            return Ok("current-input-changed");
        }
    }
    let Some((source_scenario, source)) = find(index, &review.source.id) else {
        return Ok("source-observation-missing");
    };
    let destination = if let Some(path) = &review.destination.build_record {
        crate::hil::build_record::BuildEvidence::load(root, path, &review.destination.id)?
    } else {
        let Some((_, observation)) = find(index, &review.destination.id) else {
            return Ok("destination-observation-missing");
        };
        let Some(subject) = crate::hil::build_record::BuildEvidence::observation(observation)
        else {
            return Ok("destination-source-binding-not-established");
        };
        subject
    };
    let Some(source_build) = crate::hil::build_record::BuildEvidence::observation(source) else {
        return Ok("source-contract-changed-or-unavailable");
    };
    if source_scenario != review.scenario {
        return Ok("source-property-mismatch");
    }
    if !passed(source, requirement, catalog) {
        return Ok("source-obligation-not-passed");
    }
    if !image_matches(source, &review.source)
        || !subject_matches(&destination.subject, &review.destination)
    {
        return Ok("build-binding-mismatch");
    }
    if !procedure_matches(source, &review.scenario, &review.source.image, catalog)? {
        return Ok("source-procedure-mismatch");
    }
    if !original_control_passed(source, requirement, index, catalog) {
        return Ok("source-control-not-passed");
    }
    let sensitive = binding.image_sensitive;
    if (review.kind == Kind::IdenticalImage || sensitive)
        && review.source.application_sha256 != review.destination.application_sha256
    {
        return Ok("identical-image-required");
    }
    if !sources::matches(
        root,
        &destination,
        &review.destination.image,
        &review.inputs,
    )? {
        return Ok("destination-source-binding-not-established");
    }
    if review.kind == Kind::UnchangedFunctionalContract
        && !sources::matches(root, &source_build, &review.source.image, &review.inputs)?
    {
        return Ok("source-contract-changed-or-unavailable");
    }
    if !sources::same_dependencies(
        &source_build,
        &review.source.image,
        &destination,
        &review.destination.image,
        &review.dependency_roots,
        root,
        &binding
            .required_inputs
            .iter()
            .filter(|p| !binding.implicit_build_inputs.contains(p))
            .cloned()
            .collect::<Vec<_>>(),
    )? {
        return Ok("external-composition-changed-or-unavailable");
    }
    for disposition in &review.failures {
        let Some((scenario, failure)) = find(index, &disposition.observation) else {
            return Ok("disposed-observation-missing");
        };
        if scenario != review.scenario || !failed(failure, requirement, catalog) {
            return Ok("disposition-does-not-name-a-property-failure");
        }
        if let Some(id) = &disposition.resolving_observation {
            let Some((scenario, resolved)) = find(index, id) else {
                return Ok("resolving-observation-missing");
            };
            if scenario != review.scenario
                || resolved.started_unix_millis <= failure.started_unix_millis
                || !passed(resolved, requirement, catalog)
                || !image_matches(resolved, &review.destination)
                || !original_control_passed(resolved, requirement, index, catalog)
            {
                return Ok("failure-resolution-not-established");
            }
        }
    }
    Ok("applied")
}

fn image_matches(observation: &ScenarioEvidence, reference: &ObservationRef) -> bool {
    observation
        .subject
        .as_ref()
        .is_some_and(|subject| subject_matches(subject, reference))
}
fn subject_matches(subject: &subject::ObservationSubject, reference: &ObservationRef) -> bool {
    subject
        .firmware
        .iter()
        .filter(|f| f.image.as_deref() == Some(&reference.image))
        .count()
        == 1
        && subject.firmware.iter().any(|f| {
            f.image.as_deref() == Some(&reference.image)
                && f.application
                    .as_ref()
                    .is_some_and(|a| a.sha256 == reference.application_sha256)
        })
}

fn find<'a>(index: &'a HilEvidenceIndex, id: &str) -> Option<(&'a str, &'a ScenarioEvidence)> {
    index.scenarios.iter().find_map(|(scenario, observations)| {
        observations
            .iter()
            .find(|o| o.observation_id(scenario).as_deref() == Some(id))
            .map(|o| (scenario.as_str(), o))
    })
}

fn passed(
    observation: &ScenarioEvidence,
    requirement: &HilRequirement,
    catalog: &ScenarioCatalog,
) -> bool {
    observation.outcome == Outcome::Passed
        && observation.repetitions >= usize::from(requirement.minimum_repetitions)
        && observation
            .repetition_outcomes
            .iter()
            .all(|o| *o == Outcome::Passed)
        && observation.measurements.len() == observation.repetitions
        && requirement.checks.iter().all(|name| {
            catalog
                .checks
                .get(&requirement.scenario)
                .and_then(|c| c.get(name))
                .is_some_and(|c| {
                    observation
                        .measurements
                        .iter()
                        .all(|m| checks::assess(name, c, m) == checks::Assessment::Passed)
                })
        })
}

pub(super) fn failed(
    observation: &ScenarioEvidence,
    requirement: &HilRequirement,
    catalog: &ScenarioCatalog,
) -> bool {
    observation.outcome == Outcome::Failed
        || observation.repetition_outcomes.contains(&Outcome::Failed)
        || requirement.checks.iter().any(|name| {
            catalog
                .checks
                .get(&requirement.scenario)
                .and_then(|c| c.get(name))
                .is_some_and(|c| {
                    observation
                        .measurements
                        .iter()
                        .any(|m| checks::assess(name, c, m) == checks::Assessment::Failed)
                })
        })
}

fn procedure_matches(
    observation: &ScenarioEvidence,
    scenario: &str,
    image: &str,
    catalog: &ScenarioCatalog,
) -> Result<bool> {
    let (Some(run), Some(procedure)) = (
        &observation.run_directory,
        observation
            .subject
            .as_ref()
            .and_then(|s| s.procedure.as_ref()),
    ) else {
        return Ok(false);
    };
    let document: serde_json::Value = read_json(&run.join(&procedure.path))?;
    Ok(
        document.get("id").and_then(serde_json::Value::as_str) == Some(scenario)
            && document.get("image").and_then(serde_json::Value::as_str) == Some(image)
            && catalog.definitions.get(scenario).is_none_or(|current| {
                crate::hil::procedure::normalize(current)
                    == crate::hil::procedure::normalize(&document)
            }),
    )
}

fn original_control_passed(
    observation: &ScenarioEvidence,
    requirement: &HilRequirement,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) -> bool {
    catalog.control_for(&requirement.scenario).is_none_or(|id| {
        let control = HilRequirement {
            scenario: id.into(),
            checks: Vec::new(),
            minimum_repetitions: requirement.minimum_repetitions,
        };
        index.scenarios.get(id).is_some_and(|records| {
            records.iter().any(|r| {
                r.run_id == observation.run_id
                    && r.repetitions == observation.repetitions
                    && passed(r, &control, catalog)
            })
        })
    })
}
