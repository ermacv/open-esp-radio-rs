//! Authenticated source/member/root accounting, independent of semantic completeness.
//!
//! Coverage is computed with IR and read without decoding by diagnostics. A
//! represented function may retain research blockers; missing roots cannot.
use super::project_inputs::resolve_inputs;
use crate::{
    Result, artifact,
    project::{AnalysisSymbolFamilyDisposition, ProjectSpec},
    project_ir::ProjectIrProfile,
    run_spec::RunSpec,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) use crate::artifacts::COVERAGE_FILE as FILE;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputIdentity {
    role: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootOutcome {
    pub code_identity: crate::artifact::CodeIdentity,
    pub source: String,
    pub artifact_sha256: String,
    pub member: Option<String>,
    pub section: Option<String>,
    pub symbol: String,
    pub address: u64,
    pub selected: bool,
    pub outcome: String,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberOutcome {
    pub source: String,
    pub artifact_sha256: String,
    pub role: String,
    pub member: artifact::ArtifactMemberOutcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileCoverage {
    schema: u32,
    configuration: String,
    inputs: Vec<InputIdentity>,
    products: BTreeMap<String, String>,
    pub roots: Vec<RootOutcome>,
    pub members: Vec<MemberOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageObligation {
    pub id: String,
    pub source: Option<String>,
    pub profile: Option<String>,
    pub status: String,
    pub roots: Vec<RootOutcome>,
    pub members: Vec<MemberOutcome>,
    pub issues: Vec<String>,
}

impl CoverageObligation {
    pub(crate) fn complete(&self) -> bool {
        self.issues.is_empty()
    }
}

fn configuration(profile: &ProjectIrProfile) -> String {
    format!(
        "{:?}|{:?}|{}|{}",
        profile.sources, profile.roots, profile.include_reachable, profile.entry_contract
    )
}

fn identities(
    profile: &ProjectIrProfile,
    run: &RunSpec,
    digest: impl Fn(&Path) -> Result<String>,
) -> Result<Vec<InputIdentity>> {
    let resolved = resolve_inputs(profile, run)?;
    resolved
        .artifacts
        .into_iter()
        .map(|(source, path)| (format!("source-artifact:{source}"), path))
        .chain(
            resolved
                .inventories
                .into_iter()
                .map(|(source, path)| (format!("source-inventory:{source}"), path)),
        )
        .chain(
            resolved
                .source_companions
                .into_iter()
                .map(|(source, path)| (format!("source-companion:{source}"), path)),
        )
        .chain(
            resolved
                .companions
                .into_iter()
                .map(|path| ("companion".to_owned(), path)),
        )
        .map(|(role, path)| {
            Ok(InputIdentity {
                sha256: digest(&path)?,
                role,
                path,
            })
        })
        .collect()
}

/// Build from actual primary bytes and the functions emitted by the analyzer.
pub(crate) fn build(
    captures: &crate::source_set::CapturedSourceSet,
    profile: &ProjectIrProfile,
    run: &RunSpec,
    report: &crate::LinkedIrReport,
) -> Result<ProfileCoverage> {
    let inputs = identities(profile, run, |path| Ok(captures.sha256(path)?.to_owned()))?;
    let functions = report
        .functions
        .iter()
        .map(|function| {
            (
                (function.source.as_str(), function.code_identity.clone()),
                function,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let root_blockers = report
        .root_blockers
        .iter()
        .map(|root| {
            (
                (root.source.as_str(), root.code_identity.clone()),
                root.reason.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut roots = Vec::new();
    let mut members = Vec::new();
    for input in &inputs {
        let inventory = captures.artifact(&input.path)?.inventory()?;
        let source = input.role.split_once(':').map_or("", |(_, source)| source);
        members.extend(
            inventory
                .members
                .iter()
                .cloned()
                .map(|member| MemberOutcome {
                    source: source.to_owned(),
                    artifact_sha256: input.sha256.clone(),
                    role: input.role.clone(),
                    member,
                }),
        );
        if !input.role.starts_with("source-artifact:") {
            continue;
        }
        for object in &inventory.objects {
            for symbol in object.symbols.iter().filter(|symbol| {
                symbol.kind == artifact::ArtifactSymbolKind::Text
                    && symbol.definition.is_definition()
            }) {
                let selected = symbol.name.starts_with(profile.roots.symbol_prefix());
                let code_identity = artifact::CodeIdentity::Symbol {
                    artifact_sha256: input.sha256.clone(),
                    location: crate::SymbolLocation {
                        object: object.location,
                        table: symbol.table,
                        index: symbol.index,
                    },
                };
                let key = (source, code_identity.clone());
                let (outcome, blockers) = if let Some(function) = functions.get(&key) {
                    let blockers = function
                        .decode_blockers
                        .iter()
                        .map(|blocker| format!("{blocker:?}"))
                        .chain(
                            function
                                .direct_diagnostics
                                .iter()
                                .chain(&function.call_graph_diagnostics)
                                .chain(&function.reference_diagnostics)
                                .map(|blocker| format!("{blocker:?}")),
                        )
                        .collect::<Vec<_>>();
                    (
                        if blockers.is_empty() {
                            "analyzed"
                        } else {
                            "blocked"
                        },
                        blockers,
                    )
                } else if let Some(reason) = root_blockers.get(&key) {
                    ("blocked", vec![reason.clone()])
                } else if !selected {
                    ("not-selected", Vec::new())
                } else if symbol.size == 0 {
                    (
                        "blocked",
                        vec!["zero-sized symbol has no recovered function boundary".to_owned()],
                    )
                } else {
                    (
                        "missing",
                        vec!["selected function is absent from linked IR".to_owned()],
                    )
                };
                roots.push(RootOutcome {
                    code_identity,
                    source: source.to_owned(),
                    artifact_sha256: input.sha256.clone(),
                    member: object.member.clone(),
                    section: symbol.section.clone(),
                    symbol: symbol.name.clone(),
                    address: symbol.address,
                    selected,
                    outcome: outcome.to_owned(),
                    blockers,
                });
            }
        }
    }
    roots.sort_by(|a, b| (&a.source, &a.code_identity).cmp(&(&b.source, &b.code_identity)));
    roots.dedup();
    Ok(ProfileCoverage {
        schema: 2,
        configuration: configuration(profile),
        inputs,
        products: BTreeMap::new(),
        roots,
        members,
    })
}

pub(crate) fn seal(mut coverage: ProfileCoverage, root: &Path) -> Result<Vec<u8>> {
    for path in crate::artifacts::bundle_files(root) {
        coverage.products.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            crate::artifact_sha256(&path)?,
        );
    }
    Ok(serde_json::to_vec(&coverage)?)
}

pub(crate) fn read(profile: &ProjectIrProfile, run: &RunSpec) -> Result<ProfileCoverage> {
    let coverage: ProfileCoverage =
        serde_json::from_slice(&std::fs::read(profile.output.join(FILE))?)?;
    if coverage.schema != 2
        || coverage.configuration != configuration(profile)
        || coverage.inputs != identities(profile, run, crate::artifact_sha256)?
    {
        return Err(crate::Error::invalid(format!(
            "stale coverage for profile {:?}: input bytes, order or configuration changed",
            profile.id
        )));
    }
    let expected = crate::artifacts::bundle_files(&profile.output).collect::<Vec<_>>();
    if coverage.products.len() != expected.len() {
        return Err(crate::Error::invalid(
            "coverage product identity is incomplete",
        ));
    }
    for path in expected {
        let name = path.file_name().unwrap().to_string_lossy();
        if coverage.products.get(name.as_ref()) != Some(&crate::artifact_sha256(&path)?) {
            return Err(crate::Error::invalid(format!(
                "stale coverage: linked-IR product {} changed",
                path.display()
            )));
        }
    }
    Ok(coverage)
}

/// Configuration-only obligations also exist when their IR profile is absent.
pub(crate) fn declarations(
    project: &ProjectSpec,
    run: Option<&RunSpec>,
) -> Vec<CoverageObligation> {
    let mut obligations = Vec::new();
    for family in &project.analysis_symbol_families {
        if family.disposition != AnalysisSymbolFamilyDisposition::Required {
            continue;
        }
        let mut obligation = CoverageObligation {
            id: family.id.clone(),
            source: Some(family.source.clone()),
            profile: family.profile.clone(),
            status: "declared".to_owned(),
            roots: Vec::new(),
            members: Vec::new(),
            issues: Vec::new(),
        };
        let bound = run.is_some_and(|run| run.inputs().iter().any(|input| {
            matches!(&input.role, crate::run_spec::InputRole::SourceArtifact(source) if source.as_str() == family.source)
                && input.path.is_file()
        }));
        if !bound {
            obligation.issues.push(format!("required source-artifact:{} is missing; inventory does not satisfy executable analysis", family.source));
        }
        let profile = project
            .ir_profiles
            .iter()
            .find(|profile| Some(&profile.id) == family.profile.as_ref());
        match profile {
            None => obligation.issues.push(format!(
                "required IR profile {:?} is not configured",
                family.profile
            )),
            Some(profile)
                if !profile.sources.is_empty() && !profile.sources.contains(&family.source) =>
            {
                obligation.issues.push(format!(
                    "IR profile {} does not select required source {}",
                    profile.id, family.source
                ));
            }
            _ => (),
        }
        if !obligation.complete() {
            obligation.status = "blocked".to_owned();
        }
        obligations.push(obligation);
    }
    obligations
}

/// Shared gate for status, doctor and analysis/check. No semantic debt gate.
pub(crate) fn inspect(project: &ProjectSpec, run: Option<&RunSpec>) -> Vec<CoverageObligation> {
    let mut obligations = declarations(project, run);
    for profile in &project.ir_profiles {
        let mut item = CoverageObligation {
            id: format!("profile:{}", profile.id),
            source: None,
            profile: Some(profile.id.clone()),
            status: "covered".to_owned(),
            roots: Vec::new(),
            members: Vec::new(),
            issues: Vec::new(),
        };
        let loaded = run
            .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))
            .and_then(|run| read(profile, run));
        match loaded {
            Err(error) => item.issues.push(format!(
                "coverage for {} is unavailable or stale: {error}",
                profile.id
            )),
            Ok(coverage) => {
                for member in &coverage.members {
                    if let Some(reason) = &member.member.reason {
                        item.issues.push(format!(
                            "{} member #{} {:?}: {}: {reason}",
                            member.role,
                            member.member.ordinal,
                            member.member.name,
                            member.member.status
                        ));
                    }
                }
                for root in &coverage.roots {
                    if root.selected && !matches!(root.outcome.as_str(), "analyzed" | "blocked") {
                        item.issues.push(format!(
                            "missing root {}:{}:{:?}:{}@{:#x}",
                            root.source,
                            root.artifact_sha256,
                            root.member,
                            root.symbol,
                            root.address
                        ));
                    }
                }
                for obligation in obligations
                    .iter_mut()
                    .filter(|item| item.profile.as_ref() == Some(&profile.id))
                {
                    let family = project
                        .analysis_symbol_families
                        .iter()
                        .find(|family| family.id == obligation.id)
                        .unwrap();
                    obligation.roots = coverage
                        .roots
                        .iter()
                        .filter(|root| {
                            root.source == family.source
                                && root.symbol.starts_with(&family.symbol_prefix)
                        })
                        .cloned()
                        .collect();
                    if obligation.roots.is_empty() {
                        obligation.issues.push(format!(
                            "required prefix {:?} matched zero function definitions",
                            family.symbol_prefix
                        ));
                    }
                    for root in &obligation.roots {
                        if !matches!(root.outcome.as_str(), "analyzed" | "blocked") {
                            obligation.issues.push(format!(
                                "required root {}:{}:{:?}:{}@{:#x} is {}",
                                root.source,
                                root.artifact_sha256,
                                root.member,
                                root.symbol,
                                root.address,
                                root.outcome
                            ));
                        }
                    }
                    obligation.members = coverage
                        .members
                        .iter()
                        .filter(|member| member.source == family.source)
                        .cloned()
                        .collect();
                }
                item.roots = coverage
                    .roots
                    .into_iter()
                    .filter(|root| {
                        root.selected && !matches!(root.outcome.as_str(), "analyzed" | "blocked")
                    })
                    .collect();
                item.members = coverage.members;
            }
        }
        for obligation in obligations
            .iter_mut()
            .filter(|item| item.profile.as_ref() == Some(&profile.id))
        {
            obligation.issues.extend(item.issues.iter().cloned());
        }
        if !item.complete() {
            item.status = "incomplete".to_owned();
        }
        obligations.push(item);
    }
    for obligation in &mut obligations {
        obligation.status = if obligation.complete() {
            "covered"
        } else {
            "incomplete"
        }
        .to_owned();
    }
    obligations
}

pub(crate) fn require_complete(obligations: &[CoverageObligation]) -> Result<()> {
    let issues = obligations
        .iter()
        .flat_map(|item| {
            item.issues
                .iter()
                .map(|issue| format!("{}: {issue}", item.id))
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(crate::Error::invalid(issues.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonempty_report_cannot_hide_a_missing_root_but_decode_debt_is_accounted() {
        use object::{
            Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind,
            SymbolScope,
            write::{Object, Symbol, SymbolSection},
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vendor.o");
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        for (name, bytes) in [
            ("api_known", [0x67, 0x80, 0, 0]),
            ("api_unknown", [0xff; 4]),
        ] {
            let value = object.append_section_data(section, &bytes, 4);
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size: 4,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            });
        }
        std::fs::write(&path, object.write().unwrap()).unwrap();
        let local = directory.path().join("local.toml");
        std::fs::write(
            &local,
            "schema = 1\n[[inputs]]\nrole = \"source-artifact:vendor\"\npath = \"vendor.o\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&local).unwrap();
        let profile = ProjectIrProfile {
            id: "all".to_owned(),
            sources: vec!["vendor".to_owned()],
            roots: crate::project_ir::ProjectIrRoots::All,
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: directory.path().join("all.ir"),
        };
        let resolver = crate::ReferenceResolver::load_all_code_with_entry_contract(
            &path,
            &[],
            crate::providers::riscv_or_neutral(None).unwrap(),
            crate::providers::entry_contract_or_neutral(None, "none").unwrap(),
        )
        .unwrap();
        let mut report = crate::build_linked_ir_for_source_with_cache(
            &resolver,
            &crate::MmioMap {
                registers: Vec::new(),
                regions: Vec::new(),
            },
            crate::analysis::LinkedIrSourceOptions {
                symbol_prefix: "",
                source: "vendor",
                artifact_sha256: &crate::artifact_sha256(&path).unwrap(),
                namespace_identities: true,
                include_reachable: true,
                jobs: 1,
                compact_projected_actions: false,
            },
            None,
        );
        let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
        let covered = build(&captures, &profile, &run, &report).unwrap();
        assert_eq!(covered.roots.len(), 2);
        let unknown = covered
            .roots
            .iter()
            .find(|root| root.symbol == "api_unknown")
            .unwrap();
        assert_eq!(unknown.outcome, "blocked");
        assert!(!unknown.blockers.is_empty());
        report
            .functions
            .retain(|function| function.symbol != "api_known");
        assert!(!report.functions.is_empty());
        let partial = build(&captures, &profile, &run, &report).unwrap();
        assert_eq!(
            partial
                .roots
                .iter()
                .find(|root| root.symbol == "api_known")
                .unwrap()
                .outcome,
            "missing"
        );
    }
}
