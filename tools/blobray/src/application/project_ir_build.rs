//! Project-level generation of reproducible linked-IR reports.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    MmioMap, Result, TargetSpec,
    linked_ir_export::{self, ProjectIrDocuments},
    project::ProjectSpec,
    project_ir::ProjectIrProfile,
    run_spec::{InputRole, RunSpec},
};

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectIrBuildRequest {
    pub(crate) profiles: BTreeSet<String>,
    pub(crate) check: bool,
    pub(crate) jobs: usize,
    pub(crate) refresh_review_scopes: bool,
}

struct BuiltProfileSummary<'a> {
    profile: &'a ProjectIrProfile,
    sources: usize,
    functions: usize,
    decode_blockers: usize,
    registers: usize,
    field_candidates: usize,
    documents: usize,
}

struct ResolvedInputs {
    artifacts: Vec<(String, PathBuf)>,
    inventories: Vec<(String, PathBuf)>,
    companions: Vec<PathBuf>,
}

#[derive(Serialize)]
pub(crate) struct ProfileDocument<'a> {
    pub(crate) id: &'a str,
    pub(crate) status: &'static str,
    pub(crate) sources: usize,
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
    pub(crate) bundle: &'a Path,
}

#[derive(Serialize)]
pub(crate) struct BuildDocument<'a> {
    pub(crate) schema: u32,
    pub(crate) command: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) status: &'static str,
    pub(crate) profiles: Vec<ProfileDocument<'a>>,
    pub(crate) documents: usize,
}

pub(crate) fn build_project_ir<'a>(
    request: ProjectIrBuildRequest,
    project_manifest: &Path,
    project: &'a ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<BuildDocument<'a>> {
    // Check mode must reproduce evidence without creating, migrating or
    // updating persistent query-cache state. It intentionally performs
    // uncached function analysis; write mode keeps the normal fact-store path.
    let mut function_fact_store = (!request.check)
        .then(|| crate::application::query_store::QueryStore::open(project_manifest))
        .transpose()?;
    build_project_ir_impl(
        request,
        project,
        run_spec,
        svd,
        target,
        function_fact_store.as_mut(),
    )
}

/// Build project IR while borrowing the caller's lifetime-owned query store.
///
/// Project-wide orchestration already holds the store's exclusive lock. This
/// entry point reuses that writer for function facts instead of trying to open
/// a second `QueryStore` for the same database. Check mode never consults the
/// supplied store, matching [`build_project_ir`]'s read-only behavior.
pub(crate) fn build_project_ir_with_store<'a>(
    request: ProjectIrBuildRequest,
    project: &'a ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
    function_fact_store: &mut crate::application::query_store::QueryStore,
) -> Result<BuildDocument<'a>> {
    let function_fact_store = (!request.check).then_some(function_fact_store);
    build_project_ir_impl(request, project, run_spec, svd, target, function_fact_store)
}

fn build_project_ir_impl<'a>(
    request: ProjectIrBuildRequest,
    project: &'a ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
    mut function_fact_store: Option<&mut crate::application::query_store::QueryStore>,
) -> Result<BuildDocument<'a>> {
    let selected = select_profiles(&project.ir_profiles, &request.profiles)?;
    // Resolve and validate every selected input before loading catalogs or
    // beginning expensive analysis. A missing generated ELF must name its
    // profile, role and path instead of surfacing later as an anonymous
    // `ENOENT` from the object reader.
    let resolved = selected
        .iter()
        .map(|profile| {
            let inputs = resolve_inputs(profile, run_spec)?;
            validate_inputs(profile, &inputs)?;
            Ok(inputs)
        })
        .collect::<Result<Vec<_>>>()?;
    let effective_code = crate::analysis::EffectiveCodeCatalog::load(project)?;
    let interfaces = linked_ir_export::load_project_interfaces(project, target)?;
    let interface_origins = linked_ir_export::load_project_interface_origins(project)?;
    let mut built = Vec::with_capacity(selected.len());
    let mut stale = Vec::new();
    for (profile, inputs) in selected.into_iter().zip(resolved) {
        let documents =
            linked_ir_export::generate_project_profile(linked_ir_export::ProjectProfileRequest {
                inputs: inputs.artifacts,
                inventories: inputs.inventories,
                companions: inputs.companions,
                profile,
                svd,
                target,
                effective_code: &effective_code,
                interfaces: interfaces.as_ref(),
                interface_origins: &interface_origins,
                jobs: request.jobs,
                function_fact_store: function_fact_store
                    .as_deref_mut()
                    .map(|store| store as &mut dyn crate::analysis::FunctionFactStore),
            })?;
        let ProjectIrDocuments {
            bundle,
            sources,
            functions,
            decode_blockers,
            registers,
            field_candidates,
        } = documents;
        tracing::debug!(
            profile = profile.id,
            bundle_bytes = bundle.bytes(),
            "staged linked-IR bundle"
        );
        if request.check {
            stale.extend(bundle.compare(&profile.output)?);
            drop(bundle);
        } else {
            bundle.publish(&profile.output)?;
        }
        built.push(BuiltProfileSummary {
            profile,
            sources,
            functions,
            decode_blockers,
            registers,
            field_candidates,
            documents: 8,
        });
        // A profile creates millions of short-lived analysis objects. Drop its
        // staging state first, then return free allocator pages to the OS before
        // entering the next profile. This does not change analysis semantics;
        // it only prevents sequential profiles from accumulating allocator
        // high-water memory.
        crate::resource_usage::release_unused_memory("linked-ir profile");
    }
    if !stale.is_empty() {
        return Err(crate::Error::invalid(format!(
            "generated project IR differs or is missing: {}; rerun ir build without --check",
            stale
                .iter()
                .map(|path: &PathBuf| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let status = if request.check { "verified" } else { "written" };
    let mut document_count = built.iter().map(|built| built.documents).sum::<usize>();
    if let (true, Some(workspace)) = (request.refresh_review_scopes, project.review.as_ref()) {
        let document = crate::review_scopes::build_document(project)?;
        super::generated_file::write_or_check_json(
            &workspace.output,
            &document,
            request.check,
            "review scope report",
            true,
        )?;
        document_count += 1;
    }
    Ok(BuildDocument {
        schema: 1,
        command: "ir build",
        mode: if request.check { "check" } else { "write" },
        status,
        profiles: built
            .iter()
            .map(|built| ProfileDocument {
                id: &built.profile.id,
                status,
                sources: built.sources,
                functions: built.functions,
                decode_blockers: built.decode_blockers,
                registers: built.registers,
                field_candidates: built.field_candidates,
                bundle: &built.profile.output,
            })
            .collect(),
        documents: document_count,
    })
}

fn validate_inputs(profile: &ProjectIrProfile, inputs: &ResolvedInputs) -> Result<()> {
    for (source, path) in &inputs.artifacts {
        validate_input_file(profile, format!("source-artifact:{source}"), path)?;
    }
    for (source, path) in &inputs.inventories {
        validate_input_file(profile, format!("source-inventory:{source}"), path)?;
    }
    for path in &inputs.companions {
        validate_input_file(profile, "companion".to_owned(), path)?;
    }
    Ok(())
}

fn validate_input_file(profile: &ProjectIrProfile, role: String, path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(crate::error::BlobrayError::ProjectIrInput {
            profile: profile.id.clone(),
            role,
            path: path.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configured path is not a regular file",
            ),
        }),
        Err(source) => Err(crate::error::BlobrayError::ProjectIrInput {
            profile: profile.id.clone(),
            role,
            path: path.to_owned(),
            source,
        }),
    }
}

fn select_profiles<'a>(
    profiles: &'a [ProjectIrProfile],
    selected: &BTreeSet<String>,
) -> Result<Vec<&'a ProjectIrProfile>> {
    if profiles.is_empty() {
        return Err(crate::Error::invalid(
            "project has no [[analysis.ir]] profiles",
        ));
    }
    if selected.is_empty() {
        return Ok(profiles.iter().collect());
    }
    let available = profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(unknown) = selected
        .iter()
        .find(|profile| !available.contains(profile.as_str()))
    {
        return Err(crate::Error::invalid(format!(
            "unknown project IR profile {unknown:?}"
        )));
    }
    Ok(profiles
        .iter()
        .filter(|profile| selected.contains(&profile.id))
        .collect())
}

fn resolve_inputs(profile: &ProjectIrProfile, run_spec: &RunSpec) -> Result<ResolvedInputs> {
    let bound = run_spec
        .inputs()
        .iter()
        .filter_map(|input| {
            let InputRole::SourceArtifact(source) = &input.role else {
                return None;
            };
            Some((source.as_str(), &input.path))
        })
        .fold(
            std::collections::BTreeMap::<_, Vec<_>>::new(),
            |mut bound, (source, path)| {
                bound.entry(source).or_default().push(path);
                bound
            },
        );
    let artifacts = if profile.sources.is_empty() {
        run_spec
            .inputs()
            .iter()
            .filter_map(|input| {
                let InputRole::SourceArtifact(source) = &input.role else {
                    return None;
                };
                Some((source.to_string(), input.path.clone()))
            })
            .collect::<Vec<_>>()
    } else {
        profile
            .sources
            .iter()
            .flat_map(|source| match bound.get(source.as_str()) {
                Some(paths) => paths
                    .iter()
                    .map(|path| Ok((source.clone(), (*path).clone())))
                    .collect::<Vec<_>>(),
                None => vec![Err(crate::Error::invalid(format!(
                    "IR profile {:?} requests missing run-spec role source-artifact:{source}",
                    profile.id
                )))],
            })
            .collect::<Result<Vec<_>>>()?
    };
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(format!(
            "IR profile {:?} has no source-artifact bindings in the run spec",
            profile.id
        )));
    }

    let mut companions = BTreeSet::new();
    if artifacts.len() == 1 {
        for input in run_spec.inputs() {
            if input.role == InputRole::Companion
                || matches!(
                    &input.role,
                    InputRole::SourceCompanion(source) if source.as_str() == artifacts[0].0
                )
            {
                companions.insert(input.path.clone());
            }
        }
    } else if run_spec
        .inputs()
        .iter()
        .any(|input| input.role == InputRole::Companion)
    {
        return Err(crate::Error::invalid(format!(
            "IR profile {:?} selects multiple sources but the run spec has a global companion",
            profile.id
        )));
    }
    Ok(ResolvedInputs {
        artifacts,
        inventories: run_spec
            .inputs()
            .iter()
            .filter_map(|input| match &input.role {
                InputRole::SourceInventory(source)
                    if profile.sources.is_empty()
                        || profile
                            .sources
                            .iter()
                            .any(|candidate| candidate == source.as_str()) =>
                {
                    Some((source.to_string(), input.path.clone()))
                }
                _ => None,
            })
            .collect(),
        companions: companions.into_iter().collect(),
    })
}

/// Exact caller-owned files that can affect one linked-IR profile. This is
/// shared with the project-analysis cache so changing one link unit cannot
/// invalidate every unrelated profile.
pub(crate) fn profile_input_paths(
    profile: &ProjectIrProfile,
    run_spec: &RunSpec,
) -> Result<Vec<PathBuf>> {
    let inputs = resolve_inputs(profile, run_spec)?;
    let mut paths = inputs
        .artifacts
        .into_iter()
        .map(|(_, path)| path)
        .chain(inputs.inventories.into_iter().map(|(_, path)| path))
        .chain(inputs.companions)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> ProjectIrProfile {
        ProjectIrProfile {
            id: id.to_owned(),
            sources: Vec::new(),
            roots: crate::project_ir::ProjectIrRoots::All,
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: PathBuf::from(format!("{id}.json")),
        }
    }

    #[test]
    fn options_select_profiles_and_check_mode() {
        let options = ProjectIrBuildRequest {
            profiles: ["phy".to_owned()].into(),
            check: true,
            jobs: 2,
            refresh_review_scopes: true,
        };
        assert_eq!(options.profiles, ["phy".to_owned()].into());
        assert!(options.check);
        assert_eq!(options.jobs, 2);
        let profiles = [profile("all"), profile("phy")];
        assert_eq!(
            select_profiles(&profiles, &options.profiles)
                .unwrap()
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            ["phy"]
        );
    }

    #[test]
    fn unknown_profile_is_rejected() {
        let profiles = [profile("all")];
        let error = select_profiles(&profiles, &["missing".to_owned()].into()).unwrap_err();
        assert!(error.to_string().contains("unknown project IR profile"));
    }

    #[test]
    fn source_selection_preserves_profile_order_and_scopes_companions() {
        let directory =
            std::env::temp_dir().join(format!("blobray-ir-build-inputs-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:rom\"\npath = \"rom.elf\"\n\n[[inputs]]\nrole = \"source-artifact:archive\"\npath = \"archive.elf\"\n\n[[inputs]]\nrole = \"source-companion:rom\"\npath = \"archive.elf\"\n\n[[inputs]]\nrole = \"source-companion:archive\"\npath = \"rom.elf\"\n",
        )
        .unwrap();
        let run_spec = RunSpec::load(&path).unwrap();
        let mut selected = profile("rom-only");
        selected.sources = vec!["rom".to_owned()];
        let inputs = resolve_inputs(&selected, &run_spec).unwrap();
        let mut combined = profile("combined");
        combined.sources = vec!["archive".to_owned(), "rom".to_owned()];
        let combined_inputs = resolve_inputs(&combined, &run_spec).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(inputs.artifacts.len(), 1);
        assert_eq!(inputs.artifacts[0].0, "rom");
        assert_eq!(inputs.companions.len(), 1);
        assert!(inputs.companions[0].ends_with("archive.elf"));
        assert_eq!(
            combined_inputs
                .artifacts
                .iter()
                .map(|(source, _)| source.as_str())
                .collect::<Vec<_>>(),
            ["archive", "rom"]
        );
        assert!(combined_inputs.companions.is_empty());
    }

    #[test]
    fn source_selection_preserves_one_logical_sources_ordered_archive_set() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-project-primary-archive-set-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:ble-controller\"\npath = \"libble_app.a\"\n\n[[inputs]]\nrole = \"source-artifact:ble-controller\"\npath = \"libbtdm_common.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        let mut selected = profile("ble-controller");
        selected.sources = vec!["ble-controller".to_owned()];
        let inputs = resolve_inputs(&selected, &run).unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            inputs
                .artifacts
                .iter()
                .map(|(source, path)| (
                    source.as_str(),
                    path.file_name().unwrap().to_str().unwrap()
                ))
                .collect::<Vec<_>>(),
            [
                ("ble-controller", "libble_app.a"),
                ("ble-controller", "libbtdm_common.a")
            ]
        );
        assert!(inputs.companions.is_empty());
    }

    #[test]
    fn input_preflight_names_the_profile_role_and_missing_path() {
        let missing =
            std::env::temp_dir().join(format!("blobray-missing-ir-input-{}", std::process::id()));
        let selected = profile("wifi-lifecycle");
        let inputs = ResolvedInputs {
            artifacts: vec![("libpp".to_owned(), missing.clone())],
            inventories: Default::default(),
            companions: Vec::new(),
        };

        let error = validate_inputs(&selected, &inputs).unwrap_err();

        assert!(matches!(
            error,
            crate::error::BlobrayError::ProjectIrInput {
                profile,
                role,
                path,
                source,
            } if profile == "wifi-lifecycle"
                && role == "source-artifact:libpp"
                && path == missing
                && source.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn profile_preserves_every_origin_archive_for_one_source() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-project-multiple-inventories-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:wifi\"\npath = \"wifi.elf\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libnet80211.a\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libpp.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        let mut selected = profile("wifi");
        selected.sources = vec!["wifi".to_owned()];
        let inputs = resolve_inputs(&selected, &run).unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            inputs
                .inventories
                .iter()
                .map(|(source, path)| (
                    source.as_str(),
                    path.file_name().unwrap().to_str().unwrap()
                ))
                .collect::<Vec<_>>(),
            [("wifi", "libnet80211.a"), ("wifi", "libpp.a")]
        );
    }

    #[test]
    fn profile_preserves_every_code_companion_for_one_source() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-project-multiple-companions-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:wifi\"\npath = \"wifi.elf\"\n\n[[inputs]]\nrole = \"source-companion:wifi\"\npath = \"rom.elf\"\n\n[[inputs]]\nrole = \"source-companion:wifi\"\npath = \"libphy.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        let mut selected = profile("wifi");
        selected.sources = vec!["wifi".to_owned()];
        let inputs = resolve_inputs(&selected, &run).unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            inputs
                .companions
                .iter()
                .map(|path| path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["libphy.a", "rom.elf"]
        );
    }
}
