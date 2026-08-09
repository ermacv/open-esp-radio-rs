use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

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
    let mut output = String::new();
    output.push_str("# Human decisions over generated executable-code recovery candidates.\n");
    output
        .push_str("# Do not copy entries between artifact revisions: SHA guards are mandatory.\n");
    output.push_str("schema = 1\n");
    writeln!(output, "id = \"{}\"", toml_string(project_id)).unwrap();
    for input in &facts.inputs {
        output.push_str("\n[[inputs]]\n");
        writeln!(output, "source = \"{}\"", toml_string(&input.source)).unwrap();
        writeln!(output, "artifact-sha256 = \"{}\"", input.artifact_sha256).unwrap();
    }
    for candidate in &facts.candidates {
        output.push_str("\n[[boundaries]]\n");
        writeln!(output, "source = \"{}\"", toml_string(&candidate.source)).unwrap();
        writeln!(
            output,
            "artifact-sha256 = \"{}\"",
            candidate.artifact_sha256
        )
        .unwrap();
        if let Some(member) = &candidate.member {
            writeln!(output, "member = \"{}\"", toml_string(member)).unwrap();
        }
        writeln!(output, "section = \"{}\"", toml_string(&candidate.section)).unwrap();
        writeln!(output, "entry-offset = {:#x}", candidate.entry_offset).unwrap();
        writeln!(
            output,
            "end-exclusive-offset = {:#x}",
            candidate.end_limit_offset
        )
        .unwrap();
        output.push_str("status = \"unreviewed\"\n");
        if !candidate.symbol_names.is_empty() {
            writeln!(
                output,
                "# Suggested names: {}",
                candidate.symbol_names.join(", ")
            )
            .unwrap();
        }
        if !candidate.direct_control_flow.is_empty() {
            writeln!(
                output,
                "# Direct call/tail-call evidence: {} site(s)",
                candidate.direct_control_flow.len()
            )
            .unwrap();
        }
        output.push_str("# To accept: set status = \"accepted\" and add name = \"...\".\n");
        output.push_str("# To reject: set status = \"rejected\" and add reason = \"...\".\n");
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
