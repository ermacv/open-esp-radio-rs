//! One-shot initialization of a sparse, human-edited interface overlay.

use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use super::InterfaceFacts;
use crate::Result;

pub(crate) fn write_pack_template(
    path: &Path,
    _facts: &InterfaceFacts,
    project_id: &str,
    calling_convention: &str,
) -> Result<()> {
    if path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing interface pack {}",
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
    output
        .push_str("# Human-reviewed interface layouts. Generated discovery facts stay separate.\n");
    output.push_str("schema = 2\n");
    writeln!(
        output,
        "id = \"{}\"",
        toml_string(&identifier_from(project_id))
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "calling-convention = \"{}\"",
        toml_string(calling_convention)
    )
    .expect("writing to String cannot fail");
    output.push_str(
        "\n# Sparse review overlay: add only status = \"reviewed\" or \"ignored\" anchors and slots.\n# Missing generated tables and slots remain visible in `interfaces validate` as review backlog.\n",
    );

    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn identifier_from(value: &str) -> String {
    let mut output = String::new();
    let mut last_separator = false;
    for character in value.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() {
            output.push(character);
            last_separator = false;
        } else if !last_separator && !output.is_empty() {
            output.push('-');
            last_separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "interface-pack".to_owned()
    } else {
        output
    }
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
