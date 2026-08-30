//! Generated evidence readiness without regenerating project outputs.

use std::collections::BTreeSet;

use super::{
    executable_step,
    model::{AnalysisSurfaceDetail, Component, LinkedIrProfileDetail, Phase, Readiness},
};
use crate::application::{ProjectContext, ProjectContextRequirement};
use crate::{artifacts::inspect_linked_ir, harnesses, run_spec::InputRole};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    fn component(name: &'static str, collect: impl FnOnce() -> Component) -> Component {
        let started = std::time::Instant::now();
        let component = collect();
        tracing::debug!(
            component = name,
            elapsed_ms = started.elapsed().as_millis(),
            "analysis status component collected"
        );
        component
    }
    Phase::collect(
        "analysis",
        vec![
            component("symbol_inventory", || symbol_inventory(context)),
            component("linked_ir", || linked_ir(context)),
            component("radio_surfaces", || radio_surfaces(context)),
            component("event_replays", || event_replays(context)),
            component("mmio_facts", || mmio_facts(context)),
            component("interface_facts", || interface_facts(context)),
            component("navigation_index", || navigation_index(context)),
        ],
    )
}

fn radio_surfaces(context: &ProjectContext<'_>) -> Component {
    let mut profile_protocols = std::collections::BTreeMap::<String, BTreeSet<String>>::new();
    if let Some(review) = &context.project.review {
        for scope in &review.scopes {
            for profile in &scope.profiles {
                profile_protocols
                    .entry(profile.clone())
                    .or_default()
                    .extend(scope.protocols.iter().cloned());
            }
        }
    }
    if profile_protocols.is_empty() && context.project.analysis_symbol_families.is_empty() {
        return Component::new("radio_surfaces", Readiness::NotConfigured);
    }

    let available_sources = context
        .run_spec
        .into_iter()
        .flat_map(crate::run_spec::RunSpec::inputs)
        .filter(|input| input.path.is_file())
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some(source.as_str().to_owned()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut surfaces = Vec::new();
    for (profile_id, protocols) in profile_protocols {
        let Some(profile) = context
            .project
            .ir_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
        else {
            surfaces.push(AnalysisSurfaceDetail {
                id: format!("profile:{profile_id}"),
                protocols: protocols.into_iter().collect(),
                kind: "review-profile".to_owned(),
                status: "missing-profile".to_owned(),
                profile: Some(profile_id),
                sources: Vec::new(),
                missing_sources: Vec::new(),
                output: None,
                symbol_prefix: None,
                matched_symbols: Vec::new(),
                reason: None,
                error: Some("configured review scope refers to a missing IR profile".to_owned()),
            });
            continue;
        };
        let missing_sources = if context.run_spec.is_none() && profile.sources.is_empty() {
            vec!["<project-source-artifacts>".to_owned()]
        } else {
            profile
                .sources
                .iter()
                .filter(|source| !available_sources.contains(*source))
                .cloned()
                .collect::<Vec<_>>()
        };
        let (status, error) = if !missing_sources.is_empty() || context.run_spec.is_none() {
            ("missing-vendor-artifact", None)
        } else if !profile.output.is_dir() {
            ("missing-profile", None)
        } else {
            match inspect_linked_ir(&profile.output) {
                Ok(summary) if summary.functions != 0 => ("analyzed", None),
                Ok(_) => (
                    "invalid-profile",
                    Some("linked-IR profile contains zero functions".to_owned()),
                ),
                Err(error) => ("invalid-profile", Some(error.to_string())),
            }
        };
        surfaces.push(AnalysisSurfaceDetail {
            id: format!("profile:{}", profile.id),
            protocols: protocols.into_iter().collect(),
            kind: "review-profile".to_owned(),
            status: status.to_owned(),
            profile: Some(profile.id.clone()),
            sources: profile.sources.clone(),
            missing_sources,
            output: Some(profile.output.display().to_string()),
            symbol_prefix: match &profile.roots {
                crate::project_ir::ProjectIrRoots::All => None,
                crate::project_ir::ProjectIrRoots::SymbolPrefix(prefix) => Some(prefix.clone()),
            },
            matched_symbols: Vec::new(),
            reason: None,
            error,
        });
    }

    let inventory = context
        .project
        .symbol_inventory
        .as_ref()
        .filter(|spec| spec.output.is_file())
        .map(|spec| {
            std::fs::read_to_string(&spec.output)
                .map_err(|error| error.to_string())
                .and_then(|input| {
                    crate::artifacts::parse_symbol_inventory(&input)
                        .map_err(|error| error.to_string())
                })
        })
        .transpose();
    let inventory_error = inventory.as_ref().err().cloned();
    let inventory = inventory.ok().flatten();
    for family in &context.project.analysis_symbol_families {
        let source_available = available_sources.contains(&family.source);
        match family.disposition {
            crate::project::AnalysisSymbolFamilyDisposition::Required => {
                let matched_symbols = if source_available {
                    inventory
                        .as_ref()
                        .map(|inventory| {
                            let artifact_sources = inventory
                                .artifacts
                                .iter()
                                .map(|artifact| {
                                    (
                                        artifact.index,
                                        artifact.sources.iter().cloned().collect::<BTreeSet<_>>(),
                                    )
                                })
                                .collect::<std::collections::BTreeMap<_, _>>();
                            let symbols = inventory
                                .symbols
                                .iter()
                                .map(|symbol| (symbol.artifact, symbol.name.clone()))
                                .collect::<Vec<_>>();
                            matching_symbol_identities(
                                &family.source,
                                &family.symbol_prefix,
                                &artifact_sources,
                                &symbols,
                            )
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let profile = family.profile.as_ref().and_then(|expected| {
                    context
                        .project
                        .ir_profiles
                        .iter()
                        .find(|profile| &profile.id == expected)
                });
                let (status, output, error) = if !source_available {
                    ("missing-vendor-artifact", None, None)
                } else if inventory.is_none() {
                    (
                        "unverified-required-family",
                        None,
                        Some(inventory_error.clone().unwrap_or_else(|| {
                            "symbol inventory is unavailable; required family matches are unverified"
                                .to_owned()
                        })),
                    )
                } else if matched_symbols.is_empty() {
                    (
                        "stale-required-family",
                        None,
                        Some(
                            "required public symbol prefix matched zero inventory symbols"
                                .to_owned(),
                        ),
                    )
                } else if let Some(profile) = profile {
                    if !profile.output.is_dir() {
                        (
                            "missing-profile",
                            Some(profile.output.display().to_string()),
                            None,
                        )
                    } else {
                        match inspect_linked_ir(&profile.output) {
                            Ok(summary) if summary.functions != 0 => {
                                ("analyzed", Some(profile.output.display().to_string()), None)
                            }
                            Ok(_) => (
                                "invalid-profile",
                                Some(profile.output.display().to_string()),
                                Some("linked-IR profile contains zero functions".to_owned()),
                            ),
                            Err(error) => (
                                "invalid-profile",
                                Some(profile.output.display().to_string()),
                                Some(error.to_string()),
                            ),
                        }
                    }
                } else {
                    ("missing-profile", None, None)
                };
                surfaces.push(AnalysisSurfaceDetail {
                    id: family.id.clone(),
                    protocols: family.protocols.clone(),
                    kind: "public-symbol-family".to_owned(),
                    status: status.to_owned(),
                    profile: family.profile.clone(),
                    sources: vec![family.source.clone()],
                    missing_sources: (!source_available)
                        .then(|| family.source.clone())
                        .into_iter()
                        .collect(),
                    output,
                    symbol_prefix: Some(family.symbol_prefix.clone()),
                    matched_symbols,
                    reason: None,
                    error,
                });
            }
            crate::project::AnalysisSymbolFamilyDisposition::Excluded => {
                let (status, matched_symbols, error) = if let Some(inventory) = &inventory {
                    let artifact_sources = inventory
                        .artifacts
                        .iter()
                        .map(|artifact| {
                            (
                                artifact.index,
                                artifact.sources.iter().cloned().collect::<BTreeSet<_>>(),
                            )
                        })
                        .collect::<std::collections::BTreeMap<_, _>>();
                    let symbols = inventory
                        .symbols
                        .iter()
                        .map(|symbol| (symbol.artifact, symbol.name.clone()))
                        .collect::<Vec<_>>();
                    let matched = matching_symbol_identities(
                        &family.source,
                        &family.symbol_prefix,
                        &artifact_sources,
                        &symbols,
                    );
                    if exclusion_match_status(true, matched.len()) == "stale-exclusion" {
                        (
                            "stale-exclusion",
                            matched,
                            Some(
                                "excluded public symbol prefix matched zero inventory symbols"
                                    .to_owned(),
                            ),
                        )
                    } else {
                        ("intentionally-excluded", matched, None)
                    }
                } else {
                    (
                        "unverified-exclusion",
                        Vec::new(),
                        Some(inventory_error.clone().unwrap_or_else(|| {
                            "symbol inventory is unavailable; exclusion matches are unverified"
                                .to_owned()
                        })),
                    )
                };
                surfaces.push(AnalysisSurfaceDetail {
                    id: family.id.clone(),
                    protocols: family.protocols.clone(),
                    kind: "public-symbol-family".to_owned(),
                    status: status.to_owned(),
                    profile: None,
                    sources: vec![family.source.clone()],
                    missing_sources: Vec::new(),
                    output: None,
                    symbol_prefix: Some(family.symbol_prefix.clone()),
                    matched_symbols,
                    reason: family.reason.clone(),
                    error,
                });
            }
        }
    }
    surfaces.sort_by(|left, right| left.id.cmp(&right.id));
    let analyzed = surfaces
        .iter()
        .filter(|surface| surface.status == "analyzed")
        .count();
    let excluded = surfaces
        .iter()
        .filter(|surface| surface.status == "intentionally-excluded")
        .count();
    let missing = surfaces
        .iter()
        .filter(|surface| {
            matches!(
                surface.status.as_str(),
                "missing-vendor-artifact"
                    | "missing-profile"
                    | "unverified-exclusion"
                    | "unverified-required-family"
            )
        })
        .count();
    let invalid = surfaces
        .iter()
        .filter(|surface| {
            matches!(
                surface.status.as_str(),
                "invalid-profile" | "stale-exclusion" | "stale-required-family"
            )
        })
        .count();
    let mut component = Component::new(
        "radio_surfaces",
        if invalid != 0 {
            Readiness::Invalid
        } else if missing != 0 {
            Readiness::Incomplete
        } else {
            Readiness::Ready
        },
    )
    .detail("analyzed", analyzed)
    .detail("missing", missing)
    .detail("intentionally_excluded", excluded)
    .detail("surfaces", surfaces);
    if invalid != 0 {
        component = component.diagnostic(format!(
            "{invalid} radio analysis surface declaration(s) are invalid or stale"
        ));
    } else if missing != 0 {
        component = component.diagnostic(format!(
            "{missing} required radio analysis surface(s) lack a vendor artifact, generated profile, or exclusion inventory"
        ));
    }
    component
}

pub(crate) fn matching_symbol_identities(
    source: &str,
    prefix: &str,
    artifact_sources: &std::collections::BTreeMap<usize, BTreeSet<String>>,
    symbols: &[(usize, String)],
) -> Vec<String> {
    let mut matched = symbols
        .iter()
        .filter(|(artifact, name)| {
            artifact_sources
                .get(artifact)
                .is_some_and(|sources| sources.contains(source))
                && name.starts_with(prefix)
        })
        .map(|(_, name)| format!("{source}:{name}"))
        .collect::<Vec<_>>();
    matched.sort();
    matched.dedup();
    matched
}

fn exclusion_match_status(inventory_available: bool, matched: usize) -> &'static str {
    if !inventory_available {
        "unverified-exclusion"
    } else if matched == 0 {
        "stale-exclusion"
    } else {
        "intentionally-excluded"
    }
}

fn event_replays(context: &ProjectContext<'_>) -> Component {
    let Some(functions) = context.project.functions.as_ref() else {
        return Component::new("event_replays", Readiness::NotConfigured);
    };
    if !functions.pack.is_file() {
        return Component::new("event_replays", Readiness::NotConfigured)
            .detail("pack", functions.pack.display().to_string());
    }
    let pack = match crate::function_workspace::FunctionPack::load_reviewed(&functions.pack) {
        Ok(pack) => pack,
        Err(error) => {
            return Component::new("event_replays", Readiness::Invalid)
                .detail("pack", functions.pack.display().to_string())
                .diagnostic(error);
        }
    };
    let replays = pack
        .event_routes
        .iter()
        .filter_map(|route| route.replay().map(|replay| (route.id(), replay)))
        .collect::<Vec<_>>();
    if replays.is_empty() {
        return Component::new("event_replays", Readiness::NotConfigured)
            .detail("pack", functions.pack.display().to_string());
    }
    let mut outputs = Vec::with_capacity(replays.len());
    let mut problems = Vec::new();
    for (route, replay) in &replays {
        if !replay.evidence.is_file() {
            problems.push(format!(
                "event route {route:?} replay evidence has not been generated: {}",
                replay.evidence.display()
            ));
            outputs.push(format!("{route}: missing"));
            continue;
        }
        let result = std::fs::read_to_string(&replay.evidence)
            .map_err(|error| error.to_string())
            .and_then(|input| {
                crate::artifacts::parse_replay_evidence(&input)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(()) => outputs.push(format!("{route}: ready")),
            Err(error) => {
                outputs.push(format!("{route}: invalid"));
                problems.push(format!("event route {route:?}: {error}"));
            }
        }
    }
    let incomplete = !problems.is_empty();
    let mut component = Component::new(
        "event_replays",
        if incomplete {
            Readiness::Incomplete
        } else {
            Readiness::Ready
        },
    )
    .detail("pack", functions.pack.display().to_string())
    .detail("count", replays.len())
    .detail("routes", outputs)
    .detail("validation_depth", "shallow")
    .detail("freshness", "unknown")
    .detail("deep_validation", "project doctor / project check");
    for problem in problems {
        component = component.diagnostic(problem);
    }
    if incomplete {
        component = component.next_step(executable_step(
            context,
            "regenerate incomplete analysis evidence",
            ["project", "analyze"],
            ProjectContextRequirement::Analysis,
        ));
    }
    component
}

fn navigation_index(context: &ProjectContext<'_>) -> Component {
    let Some(spec) = &context.project.navigation_index else {
        return Component::new("navigation_index", Readiness::NotConfigured);
    };
    if !spec.output.is_file() {
        return Component::new("navigation_index", Readiness::Incomplete)
            .detail("path", spec.output.display().to_string())
            .diagnostic("navigation index has not been generated")
            .next_step(executable_step(
                context,
                "generate the navigation index",
                ["project", "analyze"],
                ProjectContextRequirement::Analysis,
            ));
    }
    generated_output("navigation_index", &spec.output)
}

fn symbol_inventory(context: &ProjectContext<'_>) -> Component {
    let Some(spec) = &context.project.symbol_inventory else {
        return Component::new("symbol_inventory", Readiness::NotConfigured);
    };
    if !spec.output.is_file() {
        return Component::new("symbol_inventory", Readiness::Incomplete)
            .detail("path", spec.output.display().to_string())
            .diagnostic("symbol inventory has not been generated");
    }
    generated_output("symbol_inventory", &spec.output)
}

fn linked_ir(context: &ProjectContext<'_>) -> Component {
    if context.project.ir_profiles.is_empty() {
        return Component::new("linked_ir", Readiness::NotConfigured);
    }
    let sources = context
        .run_spec
        .into_iter()
        .flat_map(crate::run_spec::RunSpec::inputs)
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some(source.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut invalid = false;
    let mut incomplete = false;
    let mut profiles = Vec::new();
    for profile in &context.project.ir_profiles {
        let requested = if profile.sources.is_empty() {
            sources.clone()
        } else {
            profile.sources.iter().map(String::as_str).collect()
        };
        let missing = requested.difference(&sources).copied().collect::<Vec<_>>();
        if context.run_spec.is_none() || requested.is_empty() || !missing.is_empty() {
            incomplete = true;
        }
        let contract = harnesses::entry_contract_or_neutral(
            context.target.knowledge_provider.as_deref(),
            &profile.entry_contract,
        );
        if contract.is_err() {
            invalid = true;
        }
        let (output_status, summary, output_error) = if !profile.output.is_dir() {
            incomplete = true;
            ("not-generated", None, None)
        } else {
            match inspect_linked_ir(&profile.output) {
                Ok(summary) => ("ready", Some(summary), None),
                Err(error) => {
                    invalid = true;
                    ("invalid", None, Some(error.to_string()))
                }
            }
        };
        let contract_status = if contract.is_ok() { "ready" } else { "invalid" };
        profiles.push(LinkedIrProfileDetail {
            id: profile.id.clone(),
            sources: requested
                .iter()
                .map(|source| (*source).to_owned())
                .collect(),
            missing_sources: missing.iter().map(|source| (*source).to_owned()).collect(),
            entry_contract: profile.entry_contract.clone(),
            contract_status,
            contract_error: contract.err().map(|error| error.to_string()),
            output: profile.output.display().to_string(),
            output_status,
            output_error,
            functions: summary.as_ref().map_or(0, |summary| summary.functions),
            registers: summary.as_ref().map_or(0, |summary| summary.registers),
            field_candidates: summary
                .as_ref()
                .map_or(0, |summary| summary.field_candidates),
        });
    }
    Component::new(
        "linked_ir",
        if invalid {
            Readiness::Invalid
        } else if incomplete {
            Readiness::Incomplete
        } else {
            Readiness::Ready
        },
    )
    .detail("profiles", profiles)
    .detail("validation_depth", "shallow")
    .detail("freshness", "unknown")
    .detail("deep_validation", "project doctor / project check")
}

fn mmio_facts(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.registers else {
        return Component::new("mmio_facts", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("mmio_facts", Readiness::Incomplete)
            .detail("path", paths.facts.display().to_string())
            .diagnostic("MMIO facts have not been generated");
    }
    generated_output("mmio_facts", &paths.facts)
}

fn interface_facts(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.interfaces else {
        return Component::new("interface_facts", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("interface_facts", Readiness::Incomplete)
            .detail("path", paths.facts.display().to_string())
            .diagnostic("interface facts have not been generated");
    }
    generated_output("interface_facts", &paths.facts)
}

fn generated_output(name: &'static str, path: &std::path::Path) -> Component {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() != 0 => {
            Component::new(name, Readiness::Ready)
                .detail("path", path.display().to_string())
                .detail("bytes", metadata.len())
                .detail("validation_depth", "shallow")
                .detail("freshness", "unknown")
                .detail("deep_validation", "project doctor / project check")
        }
        Ok(_) => Component::new(name, Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic("generated output is not a non-empty regular file"),
        Err(error) => Component::new(name, Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_prefix_must_match_the_declared_source_inventory() {
        let artifacts = std::collections::BTreeMap::from([
            (0, BTreeSet::from(["ble-controller".to_owned()])),
            (1, BTreeSet::from(["ieee802154".to_owned()])),
        ]);
        let symbols = vec![
            (0, "esp_ieee802154_enable".to_owned()),
            (1, "esp_ieee802154_disable".to_owned()),
            (1, "unrelated".to_owned()),
        ];

        assert_eq!(
            matching_symbol_identities("ieee802154", "esp_ieee802154_", &artifacts, &symbols,),
            ["ieee802154:esp_ieee802154_disable"]
        );
        assert!(
            matching_symbol_identities("ieee802154", "stale_public_prefix_", &artifacts, &symbols,)
                .is_empty()
        );
        assert_eq!(exclusion_match_status(true, 0), "stale-exclusion");
        assert_eq!(exclusion_match_status(true, 1), "intentionally-excluded");
        assert_eq!(exclusion_match_status(false, 0), "unverified-exclusion");
    }
}
