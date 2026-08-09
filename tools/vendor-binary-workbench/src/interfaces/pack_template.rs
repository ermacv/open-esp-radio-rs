//! One-shot generation of an editable interface pack from immutable facts.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use super::{InterfaceFactRoot, InterfaceFactStep, InterfaceFacts};
use crate::Result;

pub(crate) fn write_pack_template(
    path: &Path,
    facts: &InterfaceFacts,
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
    output.push_str("schema = 1\n");
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
    let mut ids = BTreeSet::new();
    for (table_index, table) in facts.tables.iter().enumerate() {
        if table
            .container_path
            .iter()
            .any(|step| step.selector.is_some())
        {
            return Err(crate::Error::invalid(
                "cannot initialize a reviewed anchor from an indexed container path; review the generated facts manually",
            ));
        }
        let artifact = facts
            .artifact(table.artifact)
            .expect("validated interface facts reference an artifact");
        if artifact.sources.len() != 1 {
            return Err(crate::Error::invalid(format!(
                "artifact {} has {} source identities; generate a filtered facts report before initializing a pack",
                artifact.index,
                artifact.sources.len()
            )));
        }
        let source = artifact.sources.iter().next().expect("one source");
        let root_name = match &table.root {
            InterfaceFactRoot::RelocatedSymbol { symbol, .. } => symbol.clone(),
            InterfaceFactRoot::FunctionArgument { argument } => format!("arg{argument}"),
            InterfaceFactRoot::AbsoluteAddress { address } => format!("address_{address:08x}"),
        };
        let base_id = identifier_from(&format!("{source}.{root_name}"));
        let mut id = base_id.clone();
        let mut suffix = 2usize;
        while !ids.insert(id.clone()) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        output.push_str("\n[[anchors]]\n");
        writeln!(output, "id = \"{}\"", toml_string(&id)).expect("writing to String cannot fail");
        output.push_str("status = \"unreviewed\"\norigin = \"observed\"\n");
        writeln!(output, "source = \"{}\"", toml_string(source))
            .expect("writing to String cannot fail");
        write_root(&mut output, &table.root);
        write_steps(&mut output, "container-path", &table.container_path);
        output.push_str("layout-version = \"unreviewed\"\n");
        let pointer_width = table
            .slots
            .iter()
            .map(|slot| slot.width)
            .max()
            .unwrap_or(32);
        let pointer_bytes = u32::from(pointer_width) / 8;
        let layout_size = table
            .slots
            .iter()
            .filter(|slot| slot.selector.is_none())
            .filter_map(|slot| u32::try_from(slot.offset).ok())
            .filter_map(|offset| offset.checked_add(pointer_bytes))
            .max()
            .unwrap_or(pointer_bytes);
        writeln!(output, "pointer-width = {pointer_width}").expect("writing to String cannot fail");
        writeln!(output, "layout-size = {layout_size}").expect("writing to String cannot fail");
        writeln!(output, "slot-stride = {pointer_bytes}").expect("writing to String cannot fail");
        output.push_str(
            "# execution-contract = \"platform.table-v1\" # optional compiled harness table ID\n",
        );
        if let Some(sha256) = &artifact.sha256 {
            output.push_str("\n[[anchors.guards]]\nkind = \"artifact-sha256\"\n");
            writeln!(output, "sha256 = \"{sha256}\"").expect("writing to String cannot fail");
        } else {
            output.push_str("# Before review, add an artifact-sha256 or runtime-value guard.\n");
        }
        let indexed_arguments = table
            .slots
            .iter()
            .filter_map(|slot| slot.selector.map(|selector| selector.argument))
            .collect::<BTreeSet<_>>();
        for argument in indexed_arguments {
            output.push_str(
                "\n# An indexed call is not bound until this reviewed control-flow contract is completed.\n",
            );
            output.push_str("# [[anchors.index-domains]]\n");
            writeln!(output, "# argument = {argument}").expect("writing to String cannot fail");
            output.push_str("# min = 0\n# max = 0\n");
            output.push_str("# evidence = \"reviewed caller precondition or branch evidence\"\n");
        }
        for slot in &table.slots {
            if let Some(selector) = slot.selector {
                writeln!(
                    output,
                    "# INDEXED-SLOT evidence: fixed-offset={} width={} selector=arg{}*{}{:+#x}; review its index domain, then add only the corresponding slots with origin = \"manual\".",
                    slot.offset,
                    slot.width,
                    selector.argument,
                    selector.scale,
                    selector.addend,
                )
                .expect("writing to String cannot fail");
                continue;
            }
            output.push_str("\n[[anchors.slots]]\n");
            writeln!(output, "offset = {}", slot.offset).expect("writing to String cannot fail");
            writeln!(output, "width = {}", slot.width).expect("writing to String cannot fail");
            output.push_str("status = \"unreviewed\"\norigin = \"observed\"\n");
            output.push_str(
                "# execution-model = \"operation-id\" # requires anchor execution-contract\n",
            );
        }
        if table_index + 1 == facts.tables.len() {
            output.push('\n');
        }
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(output.as_bytes())?;
    Ok(())
}

fn write_root(output: &mut String, root: &InterfaceFactRoot) {
    writeln!(output, "root-kind = \"{}\"", root.kind()).expect("writing to String cannot fail");
    match root {
        InterfaceFactRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
        } => {
            writeln!(output, "symbol = \"{}\"", toml_string(symbol))
                .expect("writing to String cannot fail");
            if let Some(member) = member {
                writeln!(output, "member = \"{}\"", toml_string(member))
                    .expect("writing to String cannot fail");
            }
            writeln!(output, "addend = {addend}").expect("writing to String cannot fail");
            writeln!(output, "addressing = \"{addressing}\"")
                .expect("writing to String cannot fail");
        }
        InterfaceFactRoot::FunctionArgument { argument } => {
            writeln!(output, "argument = {argument}").expect("writing to String cannot fail");
        }
        InterfaceFactRoot::AbsoluteAddress { address } => {
            writeln!(output, "address = {address:#010x}").expect("writing to String cannot fail");
        }
    }
}

fn write_steps(output: &mut String, key: &str, steps: &[InterfaceFactStep]) {
    write!(output, "{key} = [").expect("writing to String cannot fail");
    for (index, step) in steps.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write!(
            output,
            "{{ offset = {}, width = {} }}",
            step.offset, step.width
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("]\n");
}

pub(super) fn identifier_from(value: &str) -> String {
    let mut output = String::new();
    let mut last_separator = false;
    for character in value.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() {
            output.push(character);
            last_separator = false;
        } else if !output.is_empty() && !last_separator {
            output.push('-');
            last_separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() || !output.as_bytes()[0].is_ascii_lowercase() {
        format!("project-{output}")
    } else {
        output
    }
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
