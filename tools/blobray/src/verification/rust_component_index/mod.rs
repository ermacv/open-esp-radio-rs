//! Reviewed Rust component identities joined to workspace source and probe ELF/DWARF facts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use cargo_metadata::{MetadataCommand, TargetKind};
use syn::{ImplItem, Item, Type};

use super::RustComponentCoverage;
use crate::Result;

pub(crate) const RUST_COMPONENT_INDEX_SCHEMA: u32 = 1;

mod compiled;
mod model;
pub(crate) use compiled::compiled_matches;
use compiled::compiled_symbols;
use model::*;

pub(crate) use model::{RustArtifactInput, RustComponentEvidence, RustComponentIndex};

#[derive(Clone, Debug)]
struct SourceDefinition {
    component_id: String,
    item: RustSourceItem,
}

impl RustComponentIndex {
    pub(crate) fn build(
        project_manifest: &Path,
        components: &[RustComponentCoverage],
        artifact_inputs: &[RustArtifactInput],
    ) -> Result<Self> {
        let component_ids = components
            .iter()
            .map(|component| component.component_id.clone())
            .collect::<BTreeSet<_>>();
        Self::build_component_ids(project_manifest, &component_ids, artifact_inputs)
    }

    pub(crate) fn build_component_ids(
        project_manifest: &Path,
        component_ids: &BTreeSet<String>,
        artifact_inputs: &[RustArtifactInput],
    ) -> Result<Self> {
        let (source_definitions, mut diagnostics) =
            source_definitions(project_manifest, component_ids)?;
        let (artifacts, compiled_symbols, artifact_diagnostics) =
            compiled_symbols(artifact_inputs)?;
        diagnostics.extend(artifact_diagnostics);

        let mut summary = RustComponentIndexSummary {
            reviewed_components: component_ids.len(),
            ..RustComponentIndexSummary::default()
        };
        for artifact in &artifacts {
            match artifact.freshness_status {
                "fresh" => summary.artifact_freshness_fresh += 1,
                "stale" => summary.artifact_freshness_stale += 1,
                _ => summary.artifact_freshness_unknown += 1,
            }
        }
        let components = component_ids
            .iter()
            .cloned()
            .map(|component_id| {
                let source_items = source_definitions
                    .iter()
                    .filter(|definition| definition.component_id == component_id)
                    .map(|definition| definition.item.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let compiled_symbols = compiled_symbols
                    .iter()
                    .filter(|symbol| compiled_matches(&component_id, &symbol.demangled))
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let source_status = match source_items.len() {
                    0 => {
                        summary.source_missing += 1;
                        "missing"
                    }
                    1 => {
                        summary.source_resolved += 1;
                        "resolved"
                    }
                    _ => {
                        summary.source_ambiguous += 1;
                        "ambiguous"
                    }
                };
                let compiled_status = if compiled_symbols.is_empty() {
                    summary.compiled_missing += 1;
                    "missing"
                } else {
                    summary.compiled_resolved += 1;
                    "resolved"
                };
                let freshness_status = component_freshness(&source_items, &compiled_symbols);
                match freshness_status {
                    "fresh" => {
                        summary.freshness_checked += 1;
                        summary.freshness_fresh += 1;
                    }
                    "stale" => {
                        summary.freshness_checked += 1;
                        summary.freshness_stale += 1;
                    }
                    _ => summary.freshness_unknown += 1,
                }
                summary.dwarf_locations += compiled_symbols
                    .iter()
                    .filter(|symbol| symbol.source_file.is_some())
                    .count();
                RustComponentEvidence {
                    component_id,
                    source_status,
                    compiled_status,
                    freshness_status,
                    source_items,
                    compiled_symbols,
                }
            })
            .collect();
        diagnostics.sort();
        diagnostics.dedup();
        Ok(Self {
            schema_version: RUST_COMPONENT_INDEX_SCHEMA,
            summary,
            artifacts,
            components,
            diagnostics,
        })
    }

    pub(crate) fn stale_components(&self) -> Vec<&str> {
        self.components
            .iter()
            .filter(|component| component.freshness_status == "stale")
            .map(|component| component.component_id.as_str())
            .collect()
    }

    pub(crate) fn stale_artifacts(&self) -> Vec<&RustComponentArtifact> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.freshness_status == "stale")
            .collect()
    }
}

fn component_freshness(
    source_items: &[RustSourceItem],
    compiled_symbols: &[RustCompiledSymbol],
) -> &'static str {
    if source_items.is_empty() || compiled_symbols.is_empty() {
        return "unknown";
    }
    let source_times = source_items
        .iter()
        .map(|item| std::fs::metadata(&item.path).and_then(|metadata| metadata.modified()))
        .collect::<std::io::Result<Vec<_>>>();
    let artifact_times = compiled_symbols
        .iter()
        .map(|symbol| std::fs::metadata(&symbol.artifact).and_then(|metadata| metadata.modified()))
        .collect::<std::io::Result<Vec<_>>>();
    let (Ok(source_times), Ok(artifact_times)) = (source_times, artifact_times) else {
        return "unknown";
    };
    let newest_source = source_times.into_iter().max();
    let oldest_artifact = artifact_times.into_iter().min();
    match (newest_source, oldest_artifact) {
        (Some(source), Some(artifact)) if source > artifact => "stale",
        (Some(_), Some(_)) => "fresh",
        _ => "unknown",
    }
}

fn source_definitions(
    project_manifest: &Path,
    component_ids: &BTreeSet<String>,
) -> Result<(Vec<SourceDefinition>, Vec<String>)> {
    if component_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let cargo_manifest = project_manifest
        .ancestors()
        .map(|directory| directory.join("Cargo.toml"))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "cannot locate Cargo workspace above {}",
                project_manifest.display()
            ))
        })?;
    let metadata = MetadataCommand::new()
        .manifest_path(&cargo_manifest)
        .no_deps()
        .exec()
        .map_err(|error| {
            crate::Error::invalid(format!(
                "cannot inspect Cargo workspace {}: {error}",
                cargo_manifest.display()
            ))
        })?;
    let required_crates = component_ids
        .iter()
        .filter_map(|component| component.split("::").next())
        .collect::<BTreeSet<_>>();
    let mut crate_roots = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for package in metadata.packages {
        for target in package.targets {
            let crate_name = target.name.replace('-', "_");
            if required_crates.contains(crate_name.as_str())
                && target
                    .kind
                    .iter()
                    .any(|kind| matches!(kind, TargetKind::Lib | TargetKind::RLib))
                && let Some(root) = target.src_path.as_std_path().parent()
            {
                crate_roots
                    .entry(crate_name)
                    .or_default()
                    .insert(root.to_owned());
            }
        }
    }
    let mut definitions = Vec::new();
    let mut diagnostics = Vec::new();
    for required in required_crates {
        let Some(roots) = crate_roots.get(required) else {
            diagnostics.push(format!(
                "reviewed Rust crate {required:?} is absent from Cargo workspace metadata"
            ));
            continue;
        };
        for root in roots {
            for path in rust_files(root)? {
                let input = std::fs::read_to_string(&path)?;
                let syntax = match syn::parse_file(&input) {
                    Ok(syntax) => syntax,
                    Err(error) => {
                        diagnostics.push(format!(
                            "cannot parse Rust source {}: {error}",
                            path.display()
                        ));
                        continue;
                    }
                };
                let module = file_module(required, root, &path);
                collect_items(&syntax.items, &module, &path, &input, &mut definitions);
            }
        }
    }
    definitions.retain(|definition| component_ids.contains(&definition.component_id));
    definitions.sort_by(|left, right| {
        (&left.component_id, &left.item).cmp(&(&right.component_id, &right.item))
    });
    definitions
        .dedup_by(|left, right| left.component_id == right.component_id && left.item == right.item);
    Ok((definitions, diagnostics))
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files)?;
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn file_module(crate_name: &str, root: &Path, path: &Path) -> Vec<String> {
    let mut segments = vec![crate_name.to_owned()];
    let Ok(relative) = path.strip_prefix(root) else {
        return segments;
    };
    let mut components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let Some(file) = components.pop() else {
        return segments;
    };
    segments.extend(components);
    let stem = Path::new(&file)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    if !matches!(stem.as_str(), "lib" | "main" | "mod") {
        segments.push(stem);
    }
    segments
}

fn collect_items(
    items: &[Item],
    module: &[String],
    path: &Path,
    input: &str,
    output: &mut Vec<SourceDefinition>,
) {
    for item in items {
        match item {
            Item::Fn(item) => push_source(
                output,
                module,
                &item.sig.ident.to_string(),
                "function",
                path,
                input,
            ),
            Item::Struct(item) => push_source(
                output,
                module,
                &item.ident.to_string(),
                "struct",
                path,
                input,
            ),
            Item::Enum(item) => {
                push_source(output, module, &item.ident.to_string(), "enum", path, input)
            }
            Item::Union(item) => push_source(
                output,
                module,
                &item.ident.to_string(),
                "union",
                path,
                input,
            ),
            Item::Trait(item) => push_source(
                output,
                module,
                &item.ident.to_string(),
                "trait",
                path,
                input,
            ),
            Item::Type(item) => {
                push_source(output, module, &item.ident.to_string(), "type", path, input)
            }
            Item::Const(item) => push_source(
                output,
                module,
                &item.ident.to_string(),
                "const",
                path,
                input,
            ),
            Item::Static(item) => push_source(
                output,
                module,
                &item.ident.to_string(),
                "static",
                path,
                input,
            ),
            Item::Mod(item) => {
                if let Some((_, items)) = &item.content {
                    let mut nested = module.to_vec();
                    nested.push(item.ident.to_string());
                    collect_items(items, &nested, path, input, output);
                }
            }
            Item::Impl(item) => {
                for mut owner in impl_owners(module, item.self_ty.as_ref()) {
                    for associated in &item.items {
                        let (name, kind) = match associated {
                            ImplItem::Fn(item) => (item.sig.ident.to_string(), "method"),
                            ImplItem::Const(item) => (item.ident.to_string(), "associated-const"),
                            ImplItem::Type(item) => (item.ident.to_string(), "associated-type"),
                            ImplItem::Macro(item) => {
                                let Ok(method) =
                                    syn::parse2::<syn::ImplItemFn>(item.mac.tokens.clone())
                                else {
                                    continue;
                                };
                                (method.sig.ident.to_string(), "method")
                            }
                            _ => continue,
                        };
                        owner.push(name.clone());
                        push_source_path(output, &owner, kind, path, input, &name);
                        owner.pop();
                    }
                }
            }
            Item::Macro(item) => {
                if let Ok(file) = syn::parse2::<syn::File>(item.mac.tokens.clone()) {
                    collect_items(&file.items, module, path, input, output);
                }
            }
            _ => {}
        }
    }
}

fn impl_owners(module: &[String], ty: &Type) -> Vec<Vec<String>> {
    let Type::Path(path) = ty else {
        return Vec::new();
    };
    let parts = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let Some(first) = parts.first() else {
        return Vec::new();
    };
    let Some(crate_name) = module.first() else {
        return Vec::new();
    };
    if first == crate_name {
        vec![parts]
    } else if first == "crate" {
        let mut owner = vec![crate_name.clone()];
        owner.extend(parts.into_iter().skip(1));
        vec![owner]
    } else if parts.len() == 1 {
        let mut local = module.to_vec();
        local.extend(parts.iter().cloned());
        let mut root = vec![crate_name.clone()];
        root.extend(parts);
        if root == local {
            vec![local]
        } else {
            vec![local, root]
        }
    } else {
        let mut owner = module.to_vec();
        owner.extend(parts);
        vec![owner]
    }
}

fn push_source(
    output: &mut Vec<SourceDefinition>,
    module: &[String],
    name: &str,
    kind: &'static str,
    path: &Path,
    input: &str,
) {
    let mut component = module.to_vec();
    component.push(name.to_owned());
    push_source_path(output, &component, kind, path, input, name);
}

fn push_source_path(
    output: &mut Vec<SourceDefinition>,
    component: &[String],
    kind: &'static str,
    path: &Path,
    input: &str,
    name: &str,
) {
    let declaration = match kind {
        "function" | "method" => format!("fn {name}"),
        "struct" => format!("struct {name}"),
        "enum" => format!("enum {name}"),
        "union" => format!("union {name}"),
        "trait" => format!("trait {name}"),
        "type" | "associated-type" => format!("type {name}"),
        "const" | "associated-const" => format!("const {name}"),
        "static" => format!("static {name}"),
        _ => name.to_owned(),
    };
    let line = input
        .lines()
        .position(|line| {
            line.match_indices(&declaration).any(|(start, _)| {
                let end = start + declaration.len();
                end == line.len() || !rust_identifier_byte(line.as_bytes()[end])
            })
        })
        .map_or(1, |line| line + 1);
    output.push(SourceDefinition {
        component_id: component.join("::"),
        item: RustSourceItem {
            path: path.display().to_string(),
            line,
            kind,
        },
    });
}

const fn rust_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_modules_follow_rust_module_paths() {
        let root = Path::new("/workspace/crate/src");
        assert_eq!(
            file_module("radio", root, Path::new("/workspace/crate/src/lib.rs")),
            ["radio"]
        );
        assert_eq!(
            file_module("radio", root, Path::new("/workspace/crate/src/phy/mod.rs")),
            ["radio", "phy"]
        );
        assert_eq!(
            file_module(
                "radio",
                root,
                Path::new("/workspace/crate/src/phy/state.rs")
            ),
            ["radio", "phy", "state"]
        );
    }

    #[test]
    fn source_ast_indexes_functions_types_and_methods() {
        let syntax = syn::parse_file(
            "pub struct State; impl State { pub fn update(&mut self) {} } pub fn init() {}",
        )
        .unwrap();
        let mut definitions = Vec::new();
        collect_items(
            &syntax.items,
            &["radio".to_owned(), "phy".to_owned()],
            Path::new("state.rs"),
            "pub struct State; impl State { pub fn update(&mut self) {} } pub fn init() {}",
            &mut definitions,
        );
        let names = definitions
            .iter()
            .map(|definition| definition.component_id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(names.contains("radio::phy::State"));
        assert!(names.contains("radio::phy::State::update"));
        assert!(names.contains("radio::phy::init"));
    }

    #[test]
    fn compiled_matching_is_component_bounded() {
        assert!(compiled_matches(
            "crate_a::module::run",
            "crate_a::module::run"
        ));
        assert!(compiled_matches(
            "crate_a::module::State",
            "crate_a::module::State::update"
        ));
        assert!(!compiled_matches(
            "crate_a::module::run",
            "crate_a::module::runner"
        ));
        assert!(compiled_matches(
            "crate_a::registers::RadioRegisters::publish",
            "<crate_a::RadioRegisters>::publish"
        ));
        assert!(!compiled_matches(
            "crate_a::registers::RadioRegisters::publish",
            "<crate_b::RadioRegisters>::publish"
        ));
        assert!(!compiled_matches(
            "crate_a::registers::RadioRegisters::publish",
            "<crate_a::OtherRegisters>::publish"
        ));
        assert!(!compiled_matches(
            "crate_a::registers::RadioRegisters::publish",
            "<crate_a::RadioRegisters>::publisher"
        ));
    }

    #[test]
    fn component_freshness_rejects_a_source_newer_than_its_probe() {
        let directory =
            std::env::temp_dir().join(format!("blobray-freshness-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.rs");
        let artifact = directory.join("probe.elf");
        std::fs::write(&source, "fn operation() {}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(&artifact, b"probe").unwrap();

        let source_items = [RustSourceItem {
            path: source.display().to_string(),
            line: 1,
            kind: "function",
        }];
        let compiled = [RustCompiledSymbol {
            artifact: artifact.display().to_string(),
            demangled: "crate::operation".to_owned(),
            address: "0x1000".to_owned(),
            size: 4,
            source_file: Some(source.display().to_string()),
            source_line: Some(1),
            source_column: Some(1),
        }];
        assert_eq!(component_freshness(&source_items, &compiled), "fresh");

        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(&source, "fn operation() { changed(); }\n").unwrap();
        assert_eq!(component_freshness(&source_items, &compiled), "stale");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
