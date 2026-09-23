//! Project-level generation of reproducible linked-IR reports.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::output_set::OutputSet;
use super::project_inputs::{ResolvedInputs, resolve_inputs};
use serde::Serialize;

use crate::{
    MmioMap, Result, TargetSpec,
    linked_ir_export::{self, ProjectIrDocuments},
    project::ProjectSpec,
    project_ir::ProjectIrProfile,
    run_spec::RunSpec,
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

/// Borrowed analysis inputs; output and query-store permissions are supplied
/// separately by the owning workflow.
pub(crate) struct ProjectIrBuildContext<'project, 'input> {
    pub(crate) captures: &'input crate::source_set::CapturedSourceSet,
    pub(crate) project: &'project ProjectSpec,
    pub(crate) run_spec: &'input RunSpec,
    pub(crate) svd: &'input MmioMap,
    pub(crate) target: &'input TargetSpec,
}

pub(crate) fn build_project_ir<'a>(
    captures: &crate::source_set::CapturedSourceSet,
    request: ProjectIrBuildRequest,
    project_manifest: &Path,
    project: &'a ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<BuildDocument<'a>> {
    let outputs = select_profiles(&project.ir_profiles, &request.profiles)?
        .into_iter()
        .map(|profile| {
            Ok((
                profile.id.clone(),
                OutputSet::new(&profile_outputs(profile), request.check)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let review_outputs = if request.refresh_review_scopes {
        project
            .review
            .as_ref()
            .map(|review| OutputSet::new(std::slice::from_ref(&review.output), request.check))
            .transpose()?
    } else {
        None
    };
    // Check mode must reproduce evidence without creating, migrating or
    // updating persistent query-cache state. It intentionally performs
    // uncached function analysis; write mode keeps the normal fact-store path.
    let mut function_fact_store = (!request.check)
        .then(|| crate::application::query_store::QueryStore::open(project_manifest))
        .transpose()?;
    build_project_ir_impl(
        ProjectIrBuildContext {
            captures,
            project,
            run_spec,
            svd,
            target,
        },
        request,
        function_fact_store.as_mut(),
        &outputs,
        review_outputs.as_ref(),
    )
}

/// Build project IR while borrowing the caller's lifetime-owned query store.
///
/// Project-wide orchestration already holds the store's exclusive lock. This
/// entry point reuses that writer for function facts instead of trying to open
/// a second `QueryStore` for the same database. Check mode never consults the
/// supplied store, matching [`build_project_ir`]'s read-only behavior.
pub(crate) fn build_project_ir_with_outputs<'a>(
    context: ProjectIrBuildContext<'a, '_>,
    request: ProjectIrBuildRequest,
    function_fact_store: Option<&mut crate::application::query_store::QueryStore>,
    outputs: &BTreeMap<String, OutputSet>,
) -> Result<BuildDocument<'a>> {
    if request.refresh_review_scopes {
        return Err(crate::Error::invalid(
            "coordinator-owned IR work cannot publish the separate review-scopes output",
        ));
    }
    let function_fact_store = if request.check {
        None
    } else {
        function_fact_store
    };
    build_project_ir_impl(context, request, function_fact_store, outputs, None)
}

fn profile_outputs(profile: &ProjectIrProfile) -> Vec<PathBuf> {
    crate::artifacts::bundle_files(&profile.output)
        .chain(std::iter::once(profile.output.join(super::coverage::FILE)))
        .collect()
}

fn build_project_ir_impl<'a>(
    context: ProjectIrBuildContext<'a, '_>,
    request: ProjectIrBuildRequest,
    mut function_fact_store: Option<&mut crate::application::query_store::QueryStore>,
    outputs: &BTreeMap<String, OutputSet>,
    review_outputs: Option<&OutputSet>,
) -> Result<BuildDocument<'a>> {
    let ProjectIrBuildContext {
        captures,
        project,
        run_spec,
        svd,
        target,
    } = context;
    let selected = select_profiles(&project.ir_profiles, &request.profiles)?;
    if outputs.len() != selected.len() {
        return Err(crate::Error::invalid(
            "IR output bindings do not match selected profiles",
        ));
    }
    for profile in &selected {
        let bound = outputs.get(&profile.id).ok_or_else(|| {
            crate::Error::invalid(format!(
                "IR profile {:?} has no output bindings",
                profile.id
            ))
        })?;
        if bound.paths() != profile_outputs(profile) || bound.check() != request.check {
            return Err(crate::Error::invalid(format!(
                "IR profile {:?} changed its output bindings or mode",
                profile.id
            )));
        }
    }
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
    let reviewed_bindings =
        open_radio_vendor_review::ReviewKnowledge::load_all(&project.reviewed_knowledge)
            .and_then(|knowledge| knowledge.select_for(&project.review_context))
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot select reviewed entity bindings for linked-IR publication: {error}"
                ))
            })?
            .bindings()
            .values()
            .map(|binding| (binding.occurrence.clone(), binding.semantic.clone()))
            .collect::<BTreeMap<_, _>>();
    let mut built = Vec::with_capacity(selected.len());
    let mut stale = Vec::new();
    for (profile, inputs) in selected.into_iter().zip(resolved) {
        // The coordinator checks all profiles before building them together.
        // Capture each profile's actual function queries during execution,
        // independently of the order of those earlier cache lookups.
        if let Some(store) = function_fact_store.as_deref_mut() {
            store.begin_stage_queries(&format!("linked-ir:{}", profile.id));
        }
        let documents =
            linked_ir_export::generate_project_profile(linked_ir_export::ProjectProfileRequest {
                captures,
                inputs: inputs.artifacts,
                inventories: inputs.inventories,
                companions: inputs.companions,
                source_companions: inputs.source_companions,
                profile,
                run_spec,
                svd,
                target,
                effective_code: &effective_code,
                interfaces: interfaces.as_ref(),
                interface_origins: &interface_origins,
                reviewed_bindings: &reviewed_bindings,
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
        stale.extend(outputs[&profile.id].bundle(&profile.output, bundle)?);
        built.push(BuiltProfileSummary {
            profile,
            sources,
            functions,
            decode_blockers,
            registers,
            field_candidates,
            documents: 9,
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
    if let Some(review) = review_outputs {
        let document = crate::review_scopes::build_document(project)?;
        review
            .file(0, "review scope report")?
            .json(&document, true)?;
        review.require_complete()?;
        document_count += 1;
    }
    for output in outputs.values() {
        output.require_complete()?;
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
    for (source, path) in &inputs.source_companions {
        validate_input_file(profile, format!("source-companion:{source}"), path)?;
    }
    for path in &inputs.companions {
        validate_input_file(profile, "companion".to_owned(), path)?;
    }
    super::project_inputs::validate_ir_context(inputs)
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

/// Exact caller-owned files that can affect one linked-IR profile. This is
/// shared with the project-analysis cache so changing one link unit cannot
/// invalidate every unrelated profile.
pub(crate) fn profile_input_paths(
    profile: &ProjectIrProfile,
    run_spec: &RunSpec,
) -> Result<Vec<PathBuf>> {
    let inputs = resolve_inputs(profile, run_spec)?;
    super::project_inputs::validate_ir_context(&inputs)?;
    let mut paths = inputs
        .artifacts
        .into_iter()
        .map(|(_, path)| path)
        .chain(inputs.inventories.into_iter().map(|(_, path)| path))
        .chain(inputs.companions)
        .chain(inputs.source_companions.into_iter().map(|(_, path)| path))
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
        assert_eq!(inputs.source_companions.len(), 1);
        assert!(inputs.source_companions[0].1.ends_with("archive.elf"));
        assert_eq!(
            combined_inputs
                .artifacts
                .iter()
                .map(|(source, _)| source.as_str())
                .collect::<Vec<_>>(),
            ["archive", "rom"]
        );
        assert_eq!(combined_inputs.source_companions.len(), 2);
        assert_eq!(combined_inputs.source_companions[0].0, "rom");
        assert_eq!(combined_inputs.source_companions[1].0, "archive");
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
            source_companions: Vec::new(),
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
                .source_companions
                .iter()
                .map(|(_, path)| path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["rom.elf", "libphy.a"]
        );
    }
}
