//! One-shot initialization of a human-editable function/context pack.

use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use super::FunctionFacts;
use crate::Result;

pub(crate) fn write_function_pack_template(
    path: &Path,
    facts: &FunctionFacts,
    project_id: &str,
) -> Result<()> {
    if path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing function pack {}",
            path.display()
        )));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::new();
    output.push_str(
        "# Human-reviewed function and context names. Generated linked IR stays separate.\n",
    );
    output.push_str("schema = 11\n");
    writeln!(output, "id = \"{}\"", toml_string(project_id))
        .expect("writing to String cannot fail");
    for input in &facts.inputs {
        output.push_str("\n[[inputs]]\n");
        writeln!(output, "profile = \"{}\"", toml_string(&input.profile))
            .expect("writing to String cannot fail");
        writeln!(output, "source = \"{}\"", toml_string(&input.source))
            .expect("writing to String cannot fail");
        writeln!(output, "artifact-sha256 = \"{}\"", input.sha256)
            .expect("writing to String cannot fail");
    }
    output.push_str(
        "\n# Sparse review overlay: add only status = \"reviewed\" or \"ignored\" decisions.\n# Missing generated functions, contexts, and fields remain visible as unreviewed backlog.\n",
    );
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
