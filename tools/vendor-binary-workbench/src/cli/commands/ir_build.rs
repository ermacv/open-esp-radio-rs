//! Project-level generation of reproducible linked-IR reports.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{MmioRegisterMap, Result, TargetSpec, export_ir};
use crate::cli::{IrBuildArgs, args::OutputFormat};
use crate::{
    project::ProjectSpec,
    project_ir::ProjectIrProfile,
    run_spec::{InputRole, RunSpec},
};

#[derive(Debug, Default, Eq, PartialEq)]
struct BuildOptions {
    profiles: BTreeSet<String>,
    check: bool,
}

struct BuiltProfile<'a> {
    profile: &'a ProjectIrProfile,
    documents: export_ir::ProjectIrDocuments,
}

struct ResolvedInputs {
    artifacts: Vec<(String, PathBuf)>,
    companions: Vec<PathBuf>,
}

#[derive(Serialize)]
struct ProfileDocument<'a> {
    id: &'a str,
    status: &'static str,
    sources: usize,
    functions: usize,
    registers: usize,
    field_candidates: usize,
    json: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    pseudo: Option<&'a Path>,
}

#[derive(Serialize)]
struct BuildDocument<'a> {
    schema: u32,
    command: &'static str,
    mode: &'static str,
    status: &'static str,
    profiles: Vec<ProfileDocument<'a>>,
    documents: usize,
}

pub(super) fn run(
    arguments: IrBuildArgs,
    project: &ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let options = BuildOptions {
        profiles: arguments.profile.into_iter().collect(),
        check: arguments.check,
    };
    let selected = select_profiles(&project.ir_profiles, &options.profiles)?;
    let mut built = Vec::with_capacity(selected.len());
    for profile in selected {
        let inputs = resolve_inputs(profile, run_spec)?;
        let documents = export_ir::generate_project_profile(
            inputs.artifacts,
            inputs.companions,
            profile,
            svd,
            target,
        )?;
        built.push(BuiltProfile { profile, documents });
    }

    if options.check {
        check_all(&built)?;
    } else {
        write_all(&built)?;
    }
    let status = if options.check { "verified" } else { "written" };
    let document_count = built
        .iter()
        .map(|built| 1 + usize::from(built.documents.pseudo.is_some()))
        .sum::<usize>();
    let document = BuildDocument {
        schema: 1,
        command: "ir build",
        mode: if options.check { "check" } else { "write" },
        status,
        profiles: built
            .iter()
            .map(|built| ProfileDocument {
                id: &built.profile.id,
                status,
                sources: built.documents.sources,
                functions: built.documents.functions,
                registers: built.documents.registers,
                field_candidates: built.documents.field_candidates,
                json: &built.profile.output,
                pseudo: built.profile.pseudo_rust.as_deref(),
            })
            .collect(),
        documents: document_count,
    };
    if !crate::cli::output::structured(&document) {
        match crate::cli::output::format() {
            OutputFormat::Human => print_human(&document),
            OutputFormat::Tsv => print_tsv(&document),
            OutputFormat::Json | OutputFormat::Jsonl => {
                unreachable!("structured IR build output was already emitted")
            }
        }
    }
    Ok(true)
}

fn print_human(document: &BuildDocument<'_>) {
    outputln!(
        "IR build: {} ({} profile{}, {} document{})",
        document.status,
        document.profiles.len(),
        if document.profiles.len() == 1 {
            ""
        } else {
            "s"
        },
        document.documents,
        if document.documents == 1 { "" } else { "s" }
    );
    for profile in &document.profiles {
        outputln!(
            "  {:<20} functions={:<6} registers={:<5} fields={:<5} {}",
            profile.id,
            profile.functions,
            profile.registers,
            profile.field_candidates,
            profile.json.display()
        );
        if let Some(pseudo) = profile.pseudo {
            outputln!("  {:<20} pseudo={}", "", pseudo.display());
        }
    }
}

fn print_tsv(document: &BuildDocument<'_>) {
    for profile in &document.profiles {
        outputln!(
            "IR-PROFILE\tstatus={}\tid={}\tsources={}\tfunctions={}\tregisters={}\tfield-candidates={}\tjson={}\tpseudo={}",
            profile.status,
            profile.id,
            profile.sources,
            profile.functions,
            profile.registers,
            profile.field_candidates,
            profile.json.display(),
            profile
                .pseudo
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string())
        );
    }
    outputln!(
        "IR-BUILD\tstatus={}\tprofiles={}\tdocuments={}",
        document.status,
        document.profiles.len(),
        document.documents
    );
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

fn check_all(profiles: &[BuiltProfile<'_>]) -> Result<()> {
    let mut stale = Vec::new();
    for built in profiles {
        check_document(&built.profile.output, &built.documents.json, &mut stale);
        if let (Some(path), Some(contents)) = (
            built.profile.pseudo_rust.as_deref(),
            built.documents.pseudo.as_deref(),
        ) {
            check_document(path, contents, &mut stale);
        }
    }
    if stale.is_empty() {
        Ok(())
    } else {
        Err(crate::Error::invalid(format!(
            "generated project IR differs or is missing: {}; rerun ir build without --check",
            stale
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

fn check_document(path: &Path, expected: &str, stale: &mut Vec<PathBuf>) {
    if !matches!(fs::read_to_string(path), Ok(contents) if contents == expected) {
        stale.push(path.to_owned());
    }
}

fn write_all(profiles: &[BuiltProfile<'_>]) -> Result<()> {
    for built in profiles {
        write_document(&built.profile.output, &built.documents.json)?;
        if let (Some(path), Some(contents)) = (
            built.profile.pseudo_rust.as_deref(),
            built.documents.pseudo.as_deref(),
        ) {
            write_document(path, contents)?;
        }
    }
    Ok(())
}

fn write_document(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> ProjectIrProfile {
        ProjectIrProfile {
            id: id.to_owned(),
            sources: Vec::new(),
            symbol_prefix: String::new(),
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: PathBuf::from(format!("{id}.json")),
            pseudo_rust: None,
        }
    }

    #[test]
    fn options_select_profiles_and_check_mode() {
        let arguments = IrBuildArgs {
            profile: vec!["phy".to_owned()],
            check: true,
        };
        let options = BuildOptions {
            profiles: arguments.profile.into_iter().collect(),
            check: arguments.check,
        };
        assert_eq!(options.profiles, ["phy".to_owned()].into());
        assert!(options.check);
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
        let path = directory.join("local.run");
        std::fs::write(
            &path,
            "schema 1\ninput source-artifact:rom rom.elf\ninput source-artifact:archive archive.elf\ninput source-companion:rom archive.elf\ninput source-companion:archive rom.elf\n",
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
