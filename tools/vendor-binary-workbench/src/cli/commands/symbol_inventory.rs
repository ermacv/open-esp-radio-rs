//! Project artifact and symbol facts for manual linkage analysis.

use std::{fmt::Write as _, fs, path::PathBuf};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;
use crate::run_spec::RunSpec;

#[derive(Default)]
struct Options {
    json_report: Option<PathBuf>,
    name_prefix: Option<String>,
    undefined_only: bool,
}

impl Options {
    fn includes(&self, symbol: &LinkageSymbol) -> bool {
        self.name_prefix
            .as_ref()
            .is_none_or(|prefix| symbol.fact.name.starts_with(prefix))
            && (!self.undefined_only
                || symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined)
    }
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json-report" => {
                if options.json_report.is_some() {
                    return Err("duplicate --json-report".into());
                }
                options.json_report =
                    Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            "--name-prefix" => {
                if options.name_prefix.is_some() {
                    return Err("duplicate --name-prefix".into());
                }
                options.name_prefix = Some(take_value(&mut arguments, "--name-prefix")?);
            }
            "--undefined-only" => options.undefined_only = true,
            _ => return Err(format!("unknown symbols inventory option: {argument}").into()),
        }
    }
    Ok(options)
}

fn optional_human(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn print_report(inventory: &ProjectLinkageInventory, options: &Options) {
    for (index, artifact) in inventory.artifacts.iter().enumerate() {
        println!(
            "ARTIFACT\tindex={}\tcontainer={}\tobjects={}\tskipped-members={}\troles={}\tsources={}\tpath={}",
            index,
            artifact.container.label(),
            artifact.objects,
            artifact.skipped_members,
            artifact.roles.join(","),
            artifact.sources.join(","),
            artifact.path.display()
        );
    }
    for symbol in inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
    {
        println!(
            "SYMBOL\tartifact={}\tmember={}\tobject={}\ttable={}\tname={}\tbinding={}\tvisibility={}\tkind={}\tdefinition={}\tsection={}\taddress={:#x}\tsize={}\tscope={}\tresolution={}\tcandidates={}",
            symbol.artifact,
            optional_human(symbol.member.as_deref()),
            symbol.object_kind.label(),
            symbol.fact.table.label(),
            symbol.fact.name,
            symbol.fact.binding.label(),
            symbol.fact.visibility.label(),
            symbol.fact.kind.label(),
            symbol.fact.definition.label(),
            optional_human(symbol.fact.section.as_deref()),
            symbol.fact.address,
            symbol.fact.size,
            symbol.fact.scope.label(),
            symbol.resolution.label(),
            symbol.candidates.len(),
        );
        for candidate in &symbol.candidates {
            println!(
                "CANDIDATE\tname={}\tartifact={}\tmember={}\taddress={:#x}\tkind={}",
                symbol.fact.name,
                candidate.artifact,
                optional_human(candidate.member.as_deref()),
                candidate.address,
                candidate.kind.label(),
            );
        }
    }
    let emitted = inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
        .count();
    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
        })
        .count();
    let exported = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.fact.is_exported_definition())
        .count();
    let unresolved = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.resolution.is_unresolved())
        .count();
    println!(
        "SUMMARY\tartifacts={}\tsymbol-facts={}\temitted={}\texported-definitions={}\tundefined={}\tunresolved-or-associated={}",
        inventory.artifacts.len(),
        inventory.symbols.len(),
        emitted,
        exported,
        undefined,
        unresolved,
    );
}

fn write_optional_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_string(output, value);
    } else {
        output.push_str("null");
    }
}

fn write_json_report(
    path: &std::path::Path,
    inventory: &ProjectLinkageInventory,
    options: &Options,
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 2,\n  \"command\": \"symbols inventory\",\n");
    output.push_str("  \"linkage_mode\": \"association-only\",\n");
    output.push_str("  \"linker_resolution_claim\": false,\n");
    output.push_str("  \"artifacts\": [\n");
    for (index, artifact) in inventory.artifacts.iter().enumerate() {
        write!(output, "    {{\"index\": {index}, \"artifact\": ")
            .expect("writing to String cannot fail");
        write_artifact(&mut output, &artifact.path)?;
        output.push_str(", \"roles\": ");
        write_strings(&mut output, &artifact.roles);
        output.push_str(", \"sources\": ");
        write_strings(&mut output, &artifact.sources);
        writeln!(
            output,
            ", \"container\": \"{}\", \"objects\": {}, \"skipped_members\": {}}}{}",
            artifact.container.label(),
            artifact.objects,
            artifact.skipped_members,
            if index + 1 == inventory.artifacts.len() {
                ""
            } else {
                ","
            }
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("  ],\n  \"symbols\": [\n");
    let symbols = inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
        .collect::<Vec<_>>();
    for (index, symbol) in symbols.iter().enumerate() {
        write!(
            output,
            "    {{\"artifact\": {}, \"member\": ",
            symbol.artifact
        )
        .expect("writing to String cannot fail");
        write_optional_string(&mut output, symbol.member.as_deref());
        output.push_str(", \"object_kind\": ");
        write_string(&mut output, symbol.object_kind.label());
        output.push_str(", \"table\": ");
        write_string(&mut output, symbol.fact.table.label());
        output.push_str(", \"name\": ");
        write_string(&mut output, &symbol.fact.name);
        output.push_str(", \"binding\": ");
        write_string(&mut output, &symbol.fact.binding.label());
        output.push_str(", \"visibility\": ");
        write_string(&mut output, &symbol.fact.visibility.label());
        output.push_str(", \"kind\": ");
        write_string(&mut output, symbol.fact.kind.label());
        output.push_str(", \"definition\": ");
        write_string(&mut output, symbol.fact.definition.label());
        output.push_str(", \"section\": ");
        write_optional_string(&mut output, symbol.fact.section.as_deref());
        write!(
            output,
            ", \"address\": \"{:#x}\", \"size\": {}, \"scope\": ",
            symbol.fact.address, symbol.fact.size
        )
        .expect("writing to String cannot fail");
        write_string(&mut output, symbol.fact.scope.label());
        output.push_str(", \"resolution\": ");
        write_string(&mut output, symbol.resolution.label());
        output.push_str(", \"candidates\": [");
        for (candidate_index, candidate) in symbol.candidates.iter().enumerate() {
            if candidate_index != 0 {
                output.push_str(", ");
            }
            write!(
                output,
                "{{\"artifact\": {}, \"member\": ",
                candidate.artifact
            )
            .expect("writing to String cannot fail");
            write_optional_string(&mut output, candidate.member.as_deref());
            write!(
                output,
                ", \"address\": \"{:#x}\", \"kind\": \"{}\"}}",
                candidate.address,
                candidate.kind.label()
            )
            .expect("writing to String cannot fail");
        }
        writeln!(
            output,
            "]}}{}",
            if index + 1 == symbols.len() { "" } else { "," }
        )
        .expect("writing to String cannot fail");
    }
    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
        })
        .count();
    let exported = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.fact.is_exported_definition())
        .count();
    let unresolved = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.resolution.is_unresolved())
        .count();
    write!(
        output,
        "  ],\n  \"summary\": {{\"artifacts\": {}, \"symbol_facts\": {}, \"emitted\": {}, \"exported_definitions\": {}, \"undefined\": {}, \"unresolved_or_associated\": {}}}\n}}\n",
        inventory.artifacts.len(),
        inventory.symbols.len(),
        symbols.len(),
        exported,
        undefined,
        unresolved,
    )
    .expect("writing to String cannot fail");
    fs::write(path, output)?;
    Ok(())
}

pub(super) fn run(arguments: Vec<String>, run_spec: &RunSpec) -> Result<bool> {
    let options = parse_options(arguments)?;
    let inventory = build_project_linkage_inventory(run_spec.inputs())?;
    print_report(&inventory, &options);
    if let Some(path) = options.json_report.as_deref() {
        write_json_report(path, &inventory, &options)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inventory_filters() {
        let options = parse_options(vec![
            "--undefined-only".to_owned(),
            "--name-prefix".to_owned(),
            "osi_".to_owned(),
            "--json-report".to_owned(),
            "symbols.json".to_owned(),
        ])
        .unwrap();
        assert!(options.undefined_only);
        assert_eq!(options.name_prefix.as_deref(), Some("osi_"));
        assert_eq!(options.json_report, Some(PathBuf::from("symbols.json")));
    }
}
