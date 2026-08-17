use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use super::{CodeBoundaryPack, CodeBoundaryStatus, ReviewedCodeBoundary, ReviewedCodeInput};
use crate::{Result, artifacts::symbol_inventory::CodeBoundaryFacts};

pub(crate) fn write_code_boundary_pack_template(
    path: &Path,
    facts: &CodeBoundaryFacts,
    project_id: &str,
) -> Result<()> {
    if path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing reviewed code-boundary pack {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let pack = CodeBoundaryPack {
        schema: 1,
        id: project_id.to_owned(),
        inputs: facts
            .inputs
            .iter()
            .map(|input| ReviewedCodeInput {
                source: input.source.clone(),
                artifact_sha256: input.artifact_sha256.clone(),
            })
            .collect(),
        boundaries: facts
            .candidates
            .iter()
            .map(|candidate| ReviewedCodeBoundary {
                source: candidate.source.clone(),
                artifact_sha256: candidate.artifact_sha256.clone(),
                member: candidate.member.clone(),
                section: candidate.section.clone(),
                entry_offset: candidate.entry_offset,
                end_exclusive_offset: candidate.end_limit_offset,
                status: CodeBoundaryStatus::Unreviewed,
                name: None,
                reason: None,
            })
            .collect(),
    };
    let output = render_code_boundary_pack(&pack, facts);
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

pub(super) fn render_code_boundary_pack(
    pack: &CodeBoundaryPack,
    facts: &CodeBoundaryFacts,
) -> String {
    let mut output = String::new();
    output.push_str("# Human decisions over generated executable-code recovery candidates.\n");
    output
        .push_str("# Do not copy entries between artifact revisions: SHA guards are mandatory.\n");
    output.push_str("schema = 1\n");
    writeln!(output, "id = \"{}\"", toml_string(&pack.id)).unwrap();
    for input in &pack.inputs {
        output.push_str("\n[[inputs]]\n");
        writeln!(output, "source = \"{}\"", toml_string(&input.source)).unwrap();
        writeln!(output, "artifact-sha256 = \"{}\"", input.artifact_sha256).unwrap();
    }
    for review in &pack.boundaries {
        let fact = facts.candidates.iter().find(|candidate| {
            candidate.source == review.source
                && candidate.artifact_sha256 == review.artifact_sha256
                && candidate.member == review.member
                && candidate.section == review.section
                && candidate.entry_offset == review.entry_offset
        });
        output.push_str("\n[[boundaries]]\n");
        writeln!(output, "source = \"{}\"", toml_string(&review.source)).unwrap();
        writeln!(output, "artifact-sha256 = \"{}\"", review.artifact_sha256).unwrap();
        if let Some(member) = &review.member {
            writeln!(output, "member = \"{}\"", toml_string(member)).unwrap();
        }
        writeln!(output, "section = \"{}\"", toml_string(&review.section)).unwrap();
        writeln!(output, "entry-offset = {:#x}", review.entry_offset).unwrap();
        writeln!(
            output,
            "end-exclusive-offset = {:#x}",
            review.end_exclusive_offset
        )
        .unwrap();
        let status = match review.status {
            CodeBoundaryStatus::Unreviewed => "unreviewed",
            CodeBoundaryStatus::Accepted => "accepted",
            CodeBoundaryStatus::Rejected => "rejected",
        };
        writeln!(output, "status = \"{status}\"").unwrap();
        if let Some(name) = &review.name {
            writeln!(output, "name = \"{}\"", toml_string(name)).unwrap();
        }
        if let Some(reason) = &review.reason {
            writeln!(output, "reason = \"{}\"", toml_string(reason)).unwrap();
        }
        if let Some(candidate) = fact.filter(|candidate| !candidate.symbol_names.is_empty()) {
            writeln!(
                output,
                "# Suggested names: {}",
                candidate.symbol_names.join(", ")
            )
            .unwrap();
        }
        if let Some(candidate) = fact.filter(|candidate| !candidate.direct_control_flow.is_empty())
        {
            writeln!(
                output,
                "# Direct call/tail-call evidence: {} site(s)",
                candidate.direct_control_flow.len()
            )
            .unwrap();
        }
        if review.status == CodeBoundaryStatus::Unreviewed {
            output.push_str("# To accept: set status = \"accepted\" and add name = \"...\".\n");
            output.push_str("# To reject: set status = \"rejected\" and add reason = \"...\".\n");
        }
    }
    output
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
