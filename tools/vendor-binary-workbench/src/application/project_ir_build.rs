//! Project-level generation of reproducible linked-IR reports.

use std::{
    collections::BTreeSet,
    fs,
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
    pub(crate) json: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pseudo: Option<&'a Path>,
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
    project: &'a ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<BuildDocument<'a>> {
    let selected = select_profiles(&project.ir_profiles, &request.profiles)?;
    let effective_code = crate::analysis::EffectiveCodeCatalog::load(project)?;
    let interfaces = linked_ir_export::load_project_interfaces(project, target)?;
    let interface_origins = linked_ir_export::load_project_interface_origins(project)?;
    let mut built = Vec::with_capacity(selected.len());
    let mut stale = Vec::new();
    for profile in selected {
        let inputs = resolve_inputs(profile, run_spec)?;
        let documents = linked_ir_export::generate_project_profile(
            inputs.artifacts,
            inputs.companions,
            profile,
            svd,
            target,
            &effective_code,
            interfaces.as_ref(),
            &interface_origins,
            request.jobs,
        )?;
        if request.check {
            check_profile(profile, &documents, &mut stale);
        } else {
            write_profile(profile, &documents)?;
        }
        built.push(BuiltProfileSummary {
            profile,
            sources: documents.sources,
            functions: documents.functions,
            decode_blockers: documents.decode_blockers,
            registers: documents.registers,
            field_candidates: documents.field_candidates,
            documents: 1 + usize::from(documents.pseudo.is_some()),
        });
        // Artifact-wide JSON and pseudo-Rust strings are intentionally
        // dropped before the next profile is analyzed.
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
    if request.refresh_review_scopes && project.review.is_some() {
        let workspace = project
            .review
            .as_ref()
            .expect("review workspace is configured");
        let document = crate::review_scopes::build_document(project)?;
        super::generated_file::write_or_check(
            &workspace.output,
            &crate::review_scopes::render_document(&document)?,
            request.check,
            "review scope report",
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
                json: &built.profile.output,
                pseudo: built.profile.pseudo_rust.as_deref(),
            })
            .collect(),
        documents: document_count,
    })
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
        .collect::<std::collections::BTreeMap<_, _>>();
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
            .map(|source| {
                bound
                    .get(source.as_str())
                    .map(|path| (source.clone(), (*path).clone()))
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "IR profile {:?} requests missing run-spec role source-artifact:{source}",
                            profile.id
                        )
                        )
                    })
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
        companions: companions.into_iter().collect(),
    })
}

fn check_profile(
    profile: &ProjectIrProfile,
    documents: &ProjectIrDocuments,
    stale: &mut Vec<PathBuf>,
) {
    check_document(&profile.output, &documents.json, stale);
    if let (Some(path), Some(contents)) =
        (profile.pseudo_rust.as_deref(), documents.pseudo.as_deref())
    {
        check_document(path, contents, stale);
    }
}

fn check_document(path: &Path, expected: &str, stale: &mut Vec<PathBuf>) {
    if !matches!(fs::read_to_string(path), Ok(contents) if contents == expected) {
        stale.push(path.to_owned());
    }
}

fn write_profile(profile: &ProjectIrProfile, documents: &ProjectIrDocuments) -> Result<()> {
    super::generated_file::write_or_check(&profile.output, &documents.json, false, "linked IR")?;
    if let (Some(path), Some(contents)) =
        (profile.pseudo_rust.as_deref(), documents.pseudo.as_deref())
    {
        super::generated_file::write_or_check(path, contents, false, "pseudo-Rust")?;
    }
    Ok(())
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
            pseudo_rust: None,
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
        let directory = std::env::temp_dir().join(format!(
            "vendor-workbench-ir-build-inputs-{}",
            std::process::id()
        ));
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
}
