//! Compact, freshness-bound interface context consumed by research ranking.
//!
//! The generated document deliberately stores only the interface projection
//! needed by research. It is derived state: reviewed packs and current
//! interface facts remain the authorities, and consumers must reject a stale
//! input digest instead of evaluating those authorities as a fallback.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ProjectSession;
use crate::{Result, interfaces::InterfaceWorkspace};

const CAPABILITY_CONTEXT_SCHEMA: u32 = crate::artifacts::CAPABILITY_CONTEXT.version;
const CAPABILITY_CONTEXT_COMMAND: &str = crate::artifacts::CAPABILITY_CONTEXT.command;
const INPUT_DIGEST_DOMAIN: &[u8] = b"blobray-interface-capability-context-input-v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityContextStatus {
    Matched,
    Incomplete,
    Unknown,
}

impl CapabilityContextStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityContextRequirementKind {
    Operation,
    Effect,
    Call,
}

impl CapabilityContextRequirementKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Operation => "operation",
            Self::Effect => "effect",
            Self::Call => "call",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilityContextLink {
    pub(crate) function: String,
    pub(crate) rule: String,
    pub(crate) status: CapabilityContextStatus,
    pub(crate) requirement_kind: CapabilityContextRequirementKind,
    pub(crate) requirement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_site: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InterfaceObservationResolution {
    Ready,
    NeedsAnchor,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterfaceResearchObservation {
    pub(crate) id: String,
    pub(crate) contract: String,
    pub(crate) source: String,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selector: Option<String>,
    pub(crate) functions: Vec<String>,
    pub(crate) call_sites: Vec<u32>,
    pub(crate) resolution: InterfaceObservationResolution,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceResearchContext {
    pub(crate) links: Vec<CapabilityContextLink>,
    pub(crate) observations: Vec<InterfaceResearchObservation>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapabilityContextDocument {
    schema_version: u32,
    command: String,
    project: String,
    input_digest: String,
    links: Vec<CapabilityContextLink>,
    observations: Vec<InterfaceResearchObservation>,
}

#[derive(Serialize)]
struct CapabilityContextInputIdentity<'a> {
    project: &'a str,
    calling_convention: &'a str,
    compiled_knowledge_identity: &'a str,
    files: &'a [CapabilityContextInputFile],
}

#[derive(Debug, Serialize)]
struct CapabilityContextInputFile {
    role: &'static str,
    ordinal: usize,
    sha256: String,
}

struct CapabilityContextInputPaths<'a> {
    facts: &'a Path,
    pack: &'a Path,
    semantic_catalogs: &'a [PathBuf],
    capability_packs: &'a [PathBuf],
    interface_template_packs: &'a [PathBuf],
}

pub(crate) fn build_and_publish(
    session: &ProjectSession,
    workspace: &InterfaceWorkspace,
    output: &Path,
    check: bool,
) -> Result<()> {
    let paths = require_context_inputs(session)?;
    let report = workspace.evaluate_capabilities(paths.capability_packs)?;
    let document = build_document(
        &session.project.id,
        input_digest(session, paths)?,
        workspace,
        &report,
    );
    validate_document(&document)?;
    super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "interface capability context",
        false,
    )
}

pub(crate) fn load(session: &ProjectSession) -> Result<InterfaceResearchContext> {
    let paths = require_context_inputs(session)?;
    let output = session
        .project
        .interfaces
        .as_ref()
        .and_then(|interfaces| interfaces.capability_context.as_deref())
        .ok_or_else(|| {
            crate::Error::invalid(
                "reviewed interface pack has no generated interface capability context",
            )
        })?;
    let input = std::fs::read_to_string(output).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot read interface capability context {}: {error}",
            output.display()
        ))
    })?;
    let document: CapabilityContextDocument = serde_json::from_str(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot parse interface capability context {}: {error}",
            output.display()
        ))
    })?;
    validate_document(&document)?;
    if document.project != session.project.id {
        return Err(crate::Error::invalid(format!(
            "interface capability context belongs to project {:?}, expected {:?}",
            document.project, session.project.id
        )));
    }
    let expected = input_digest(session, paths)?;
    if document.input_digest != expected {
        return Err(crate::Error::invalid(format!(
            "interface capability context {} is stale; rerun project analyze",
            output.display()
        )));
    }
    Ok(InterfaceResearchContext {
        links: document.links,
        observations: document.observations,
    })
}

fn require_context_inputs(session: &ProjectSession) -> Result<CapabilityContextInputPaths<'_>> {
    let interfaces = session
        .project
        .interfaces
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("project has no [interfaces] table"))?;
    let pack = interfaces
        .pack
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("project [interfaces].pack is absent"))?;
    Ok(CapabilityContextInputPaths {
        facts: &interfaces.facts,
        pack,
        semantic_catalogs: &interfaces.semantic_catalogs,
        capability_packs: &interfaces.capability_packs,
        interface_template_packs: &interfaces.interface_template_packs,
    })
}

fn input_digest(
    session: &ProjectSession,
    paths: CapabilityContextInputPaths<'_>,
) -> Result<String> {
    let compiled_knowledge_identity = match session.target.knowledge_provider.as_deref() {
        Some(provider) => {
            crate::providers::contracts(provider)?;
            crate::providers::analysis_cache_identity(Some(provider))
        }
        None => crate::providers::analysis_cache_identity(None),
    };
    digest_inputs(
        &session.project.id,
        session.target.calling_convention.label(),
        &compiled_knowledge_identity,
        paths,
    )
}

fn digest_inputs(
    project: &str,
    calling_convention: &str,
    compiled_knowledge_identity: &str,
    paths: CapabilityContextInputPaths<'_>,
) -> Result<String> {
    let mut files = Vec::new();
    append_input_digest(&mut files, "facts", 0, paths.facts)?;
    append_input_digest(&mut files, "reviewed-pack", 0, paths.pack)?;
    for (ordinal, path) in paths.semantic_catalogs.iter().enumerate() {
        append_input_digest(&mut files, "semantic-catalog", ordinal, path)?;
    }
    for (ordinal, path) in paths.capability_packs.iter().enumerate() {
        append_input_digest(&mut files, "capability-pack", ordinal, path)?;
    }
    for (ordinal, path) in paths.interface_template_packs.iter().enumerate() {
        append_input_digest(&mut files, "interface-template-pack", ordinal, path)?;
    }
    let identity = CapabilityContextInputIdentity {
        project,
        calling_convention,
        compiled_knowledge_identity,
        files: &files,
    };
    let mut digest = Sha256::new();
    digest.update(INPUT_DIGEST_DOMAIN);
    digest.update(serde_json::to_vec(&identity)?);
    Ok(format!("{:x}", digest.finalize()))
}

fn append_input_digest(
    output: &mut Vec<CapabilityContextInputFile>,
    role: &'static str,
    ordinal: usize,
    path: &Path,
) -> Result<()> {
    output.push(CapabilityContextInputFile {
        role,
        ordinal,
        sha256: crate::artifact_sha256(path)?,
    });
    Ok(())
}

fn build_document(
    project: &str,
    input_digest: String,
    workspace: &InterfaceWorkspace,
    report: &crate::interfaces::CapabilityEvaluationReport,
) -> CapabilityContextDocument {
    let mut links = BTreeSet::new();
    for rule in &report.rules {
        for requirement in &rule.requirements {
            for evidence in &requirement.matches {
                let Some(function) = evidence.function.as_ref() else {
                    continue;
                };
                links.insert(CapabilityContextLink {
                    function: function.clone(),
                    rule: rule.id.clone(),
                    status: capability_status(rule.status),
                    requirement_kind: requirement_kind(requirement.kind),
                    requirement: requirement.value.clone(),
                    evidence_site: evidence.site,
                });
            }
        }
    }

    let contracts = workspace
        .contracts()
        .iter()
        .map(|contract| (contract.id.as_str(), contract))
        .collect::<BTreeMap<_, _>>();
    let observations = workspace
        .unreviewed_observations()
        .iter()
        .map(|observation| {
            let contract = contracts.get(observation.contract.as_str()).copied();
            let (resolution, anchor, template, diagnostic) = match contract {
                None => (
                    InterfaceObservationResolution::NeedsAnchor,
                    None,
                    None,
                    Some("observation is not bound to a reviewed interface anchor".to_owned()),
                ),
                Some(contract) if contract.template.is_some() => (
                    InterfaceObservationResolution::NeedsAnchor,
                    Some(contract.anchor.clone()),
                    contract.template.clone(),
                    Some(
                        "templated anchors cannot accept an unreviewed additive project slot"
                            .to_owned(),
                    ),
                ),
                Some(contract) => (
                    InterfaceObservationResolution::Ready,
                    Some(contract.anchor.clone()),
                    None,
                    None,
                ),
            };
            let mut functions = observation.functions.clone();
            functions.sort();
            functions.dedup();
            let mut call_sites = observation.call_sites.clone();
            call_sites.sort_unstable();
            call_sites.dedup();
            InterfaceResearchObservation {
                id: observation.id.clone(),
                contract: observation.contract.clone(),
                source: observation.source.clone(),
                offset: observation.offset,
                width: observation.width,
                selector: observation.selector.clone(),
                functions,
                call_sites,
                resolution,
                anchor,
                template,
                diagnostic,
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    CapabilityContextDocument {
        schema_version: CAPABILITY_CONTEXT_SCHEMA,
        command: CAPABILITY_CONTEXT_COMMAND.to_owned(),
        project: project.to_owned(),
        input_digest,
        links: links.into_iter().collect(),
        observations,
    }
}

const fn capability_status(
    status: crate::interfaces::CapabilityMatchStatus,
) -> CapabilityContextStatus {
    match status {
        crate::interfaces::CapabilityMatchStatus::Matched => CapabilityContextStatus::Matched,
        crate::interfaces::CapabilityMatchStatus::Incomplete => CapabilityContextStatus::Incomplete,
        crate::interfaces::CapabilityMatchStatus::Unknown => CapabilityContextStatus::Unknown,
    }
}

const fn requirement_kind(
    kind: crate::interfaces::CapabilityMatcherKind,
) -> CapabilityContextRequirementKind {
    match kind {
        crate::interfaces::CapabilityMatcherKind::Operation => {
            CapabilityContextRequirementKind::Operation
        }
        crate::interfaces::CapabilityMatcherKind::Effect => {
            CapabilityContextRequirementKind::Effect
        }
        crate::interfaces::CapabilityMatcherKind::Call => CapabilityContextRequirementKind::Call,
    }
}

fn validate_document(document: &CapabilityContextDocument) -> Result<()> {
    if document.schema_version != CAPABILITY_CONTEXT_SCHEMA
        || document.command != CAPABILITY_CONTEXT_COMMAND
    {
        return Err(crate::Error::invalid(format!(
            "expected interface capability context schema {CAPABILITY_CONTEXT_SCHEMA} and command {CAPABILITY_CONTEXT_COMMAND:?}"
        )));
    }
    if document.project.trim().is_empty() || document.project.trim() != document.project {
        return Err(crate::Error::invalid(
            "interface capability context project must be non-empty and trimmed",
        ));
    }
    if !valid_sha256(&document.input_digest) {
        return Err(crate::Error::invalid(
            "interface capability context input_digest must be lowercase SHA-256",
        ));
    }
    validate_strict_order("capability links", &document.links)?;
    for link in &document.links {
        if !valid_token(&link.function)
            || !valid_dotted_id(&link.rule)
            || !valid_dotted_id(&link.requirement)
        {
            return Err(crate::Error::invalid(
                "interface capability context contains an invalid capability link",
            ));
        }
    }
    validate_strict_order("interface observations", &document.observations)?;
    let mut observation_ids = BTreeSet::new();
    for observation in &document.observations {
        if !observation_ids.insert(observation.id.as_str()) {
            return Err(crate::Error::invalid(format!(
                "interface capability context has conflicting observation {:?}",
                observation.id
            )));
        }
        validate_observation(observation)?;
    }
    Ok(())
}

fn validate_observation(observation: &InterfaceResearchObservation) -> Result<()> {
    if !valid_token(&observation.id)
        || !valid_token(&observation.contract)
        || observation.source.trim().is_empty()
        || observation.source.trim() != observation.source
        || observation.width == 0
    {
        return Err(crate::Error::invalid(
            "interface capability context contains an invalid interface observation",
        ));
    }
    validate_strict_order("interface observation functions", &observation.functions)?;
    if observation
        .functions
        .iter()
        .any(|value| !valid_token(value))
    {
        return Err(crate::Error::invalid(
            "interface capability context contains an invalid observation function",
        ));
    }
    validate_strict_order("interface observation call sites", &observation.call_sites)?;
    match observation.resolution {
        InterfaceObservationResolution::Ready
            if observation.anchor.as_deref().is_some_and(valid_token)
                && observation.template.is_none()
                && observation.diagnostic.is_none() =>
        {
            Ok(())
        }
        InterfaceObservationResolution::NeedsAnchor
            if observation
                .diagnostic
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty() && value.trim() == value)
                && observation.anchor.as_deref().is_none_or(valid_token)
                && observation.template.as_deref().is_none_or(valid_token) =>
        {
            Ok(())
        }
        _ => Err(crate::Error::invalid(
            "interface capability context observation resolution metadata is inconsistent",
        )),
    }
}

fn validate_strict_order<T: Ord>(kind: &str, values: &[T]) -> Result<()> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(crate::Error::invalid(format!(
            "interface capability context {kind} must be sorted and unique"
        )));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_whitespace)
}

fn valid_dotted_id(value: &str) -> bool {
    valid_token(value)
        && value.split('.').all(|component| {
            !component.is_empty()
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> CapabilityContextDocument {
        CapabilityContextDocument {
            schema_version: CAPABILITY_CONTEXT_SCHEMA,
            command: CAPABILITY_CONTEXT_COMMAND.to_owned(),
            project: "fixture".to_owned(),
            input_digest: "a".repeat(64),
            links: vec![CapabilityContextLink {
                function: "leaf".to_owned(),
                rule: "fixture.radio.ready".to_owned(),
                status: CapabilityContextStatus::Matched,
                requirement_kind: CapabilityContextRequirementKind::Call,
                requirement: "runtime.call".to_owned(),
                evidence_site: Some(0x1000),
            }],
            observations: vec![InterfaceResearchObservation {
                id: "fixture-observation".to_owned(),
                contract: "fixture-contract".to_owned(),
                source: "fixture".to_owned(),
                offset: 4,
                width: 32,
                selector: None,
                functions: vec!["leaf".to_owned()],
                call_sites: vec![0x1000],
                resolution: InterfaceObservationResolution::Ready,
                anchor: Some("fixture-anchor".to_owned()),
                template: None,
                diagnostic: None,
            }],
        }
    }

    #[test]
    fn strict_document_rejects_duplicates_and_inconsistent_resolution() {
        let mut value = document();
        assert!(validate_document(&value).is_ok());

        value.links.push(value.links[0].clone());
        assert!(validate_document(&value).is_err());
        value.links.pop();

        value.observations[0].resolution = InterfaceObservationResolution::NeedsAnchor;
        assert!(validate_document(&value).is_err());
    }

    #[test]
    fn input_digest_is_content_bound_but_path_independent() {
        let root =
            std::env::temp_dir().join(format!("blobray-capability-context-{}", std::process::id()));
        let left = root.join("left");
        let right = root.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        for directory in [&left, &right] {
            for name in ["facts", "pack", "semantic", "capability", "template"] {
                std::fs::write(directory.join(name), name).unwrap();
            }
        }
        let left_facts = left.join("facts");
        let left_pack = left.join("pack");
        let left_semantics = [left.join("semantic")];
        let left_capabilities = [left.join("capability")];
        let left_templates = [left.join("template")];
        let left_paths = CapabilityContextInputPaths {
            facts: &left_facts,
            pack: &left_pack,
            semantic_catalogs: &left_semantics,
            capability_packs: &left_capabilities,
            interface_template_packs: &left_templates,
        };
        let right_facts = right.join("facts");
        let right_pack = right.join("pack");
        let right_semantics = [right.join("semantic")];
        let right_capabilities = [right.join("capability")];
        let right_templates = [right.join("template")];
        let right_paths = || CapabilityContextInputPaths {
            facts: &right_facts,
            pack: &right_pack,
            semantic_catalogs: &right_semantics,
            capability_packs: &right_capabilities,
            interface_template_packs: &right_templates,
        };
        let left_digest =
            digest_inputs("fixture", "riscv-ilp32", "provider@1", left_paths).unwrap();
        let right_digest =
            digest_inputs("fixture", "riscv-ilp32", "provider@1", right_paths()).unwrap();
        assert_eq!(left_digest, right_digest);

        std::fs::write(right.join("capability"), "changed").unwrap();
        let changed = digest_inputs("fixture", "riscv-ilp32", "provider@1", right_paths()).unwrap();
        assert_ne!(left_digest, changed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn strict_json_rejects_unknown_fields() {
        let mut value = serde_json::to_value(document()).unwrap();
        value["legacy"] = serde_json::json!(true);
        assert!(serde_json::from_value::<CapabilityContextDocument>(value).is_err());
    }
}
