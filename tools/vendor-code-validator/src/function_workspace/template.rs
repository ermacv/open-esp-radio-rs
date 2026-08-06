//! One-shot initialization of a human-editable function/context pack.

use std::{
    collections::BTreeMap,
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
        return Err(format!(
            "refusing to overwrite existing function pack {}",
            path.display()
        )
        .into());
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
    output.push_str("schema = 1\n");
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
    for function in facts.root_functions() {
        output.push_str("\n[[functions]]\n");
        writeln!(output, "profile = \"{}\"", toml_string(&function.profile))
            .expect("writing to String cannot fail");
        writeln!(output, "source = \"{}\"", toml_string(&function.source))
            .expect("writing to String cannot fail");
        writeln!(output, "identity = \"{}\"", toml_string(&function.identity))
            .expect("writing to String cannot fail");
        output.push_str("status = \"unreviewed\"\n");
        if !function.review_complete() {
            output.push_str(
                "# Generated evidence is incomplete; review blockers before setting accept-incomplete.\n",
            );
        }
        let mut contexts = BTreeMap::<u8, Vec<_>>::new();
        for field in &function.context_fields {
            contexts.entry(field.argument).or_default().push(field);
        }
        for (argument, fields) in contexts {
            output.push_str("\n[[functions.contexts]]\n");
            writeln!(output, "argument = {argument}").expect("writing to String cannot fail");
            output.push_str("status = \"unreviewed\"\n");
            for field in fields {
                output.push_str("\n[[functions.contexts.fields]]\n");
                writeln!(output, "offset = {}", field.offset)
                    .expect("writing to String cannot fail");
                writeln!(output, "width = {}", field.width).expect("writing to String cannot fail");
                output.push_str("status = \"unreviewed\"\n");
            }
        }
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
