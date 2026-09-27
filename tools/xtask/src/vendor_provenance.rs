//! Provenance of recovered vendor facts: the code fingerprint of every vendor
//! function production and the register model cite.
//!
//! Production records recovered facts in `SOURCE:` comment blocks, and the
//! register model in its evidence source and register/field descriptions. Every
//! identifier in those texts that names a function of a pinned vendor
//! artifact is a reference. The chip's registry records, for each referenced
//! function, the code fingerprint of the revision its facts were reviewed
//! against. `cargo xtask check provenance` fails when a referenced function
//! changed, disappeared or is not registered, so a pin update cannot leave a
//! recovered fact silently describing older code. After reviewing a changed
//! function, `cargo xtask vendor-provenance --accept NAME` records its pinned
//! fingerprint; the registry diff is the review record.
use crate::vendor_fingerprint::Function;
use crate::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tracked registry of `chip`, relative to the repository root.
fn registry_path(chip: &str) -> Result<&'static str> {
    match chip {
        "esp32s31" => Ok("verification/esp32s31/facts/provenance.toml"),
        other => Err(format!("no provenance registry for chip {other}").into()),
    }
}

/// Register-model evidence and model of `chip`, relative to the
/// repository root.
fn register_directories(chip: &str) -> [String; 2] {
    [
        format!("registers/{chip}/evidence"),
        format!("registers/{chip}/model"),
    ]
}

/// Production sources scanned for `SOURCE:` blocks.
const PRODUCTION: &str = "crates";
/// The marker opening a recovered-fact comment block.
const MARKER: &str = "SOURCE";
/// Shortest identifier taken as a reference; shorter words are prose.
const MINIMUM_NAME: usize = 4;

/// One registered function definition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub artifact: String,
    pub member: String,
    pub symbol: String,
    pub code: String,
}

fn parse_registry(text: &str) -> Result<Vec<Entry>> {
    let table: toml::Table = toml::from_str(text)?;
    if table.get("schema").and_then(|v| v.as_integer()) != Some(1) {
        return Err("unsupported provenance registry schema".into());
    }
    let mut entries = vec![];
    for value in table
        .get("function")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let field = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .ok_or_else(|| format!("registry function lacks `{key}`").into())
        };
        entries.push(Entry {
            artifact: field("artifact")?,
            member: field("member")?,
            symbol: field("symbol")?,
            code: field("code")?,
        });
    }
    Ok(entries)
}

fn render_registry(entries: &[Entry]) -> String {
    let mut text = String::from(
        "# Code fingerprints of the vendor functions production `SOURCE:` blocks\n\
         # and the register model cite, as reviewed. Maintained by\n\
         # `cargo xtask vendor-provenance`; checked by `cargo xtask check\n\
         # provenance`.\n\
         schema = 1\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "\n[[function]]\nartifact = \"{}\"\nmember = \"{}\"\nsymbol = \"{}\"\ncode = \"{}\"\n",
            entry.artifact, entry.member, entry.symbol, entry.code
        ));
    }
    text
}

fn identifiers(text: &str, out: &mut BTreeSet<String>) {
    let mut word = String::new();
    for c in text.chars().chain([' ']) {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if word.len() >= MINIMUM_NAME
                && !word.starts_with(|c: char| c.is_ascii_digit())
                && is_code_name(&word)
            {
                out.insert(std::mem::take(&mut word));
            }
            word.clear();
        }
    }
}

/// Whether `word` has the shape of a code identifier rather than prose: an
/// underscore, a digit, or an uppercase letter after the first. A plain
/// lowercase word such as `main` or `abort` is prose even when a vendor
/// symbol shares its name.
fn is_code_name(word: &str) -> bool {
    word.contains('_')
        || word.chars().any(|c| c.is_ascii_digit())
        || word.chars().skip(1).any(|c| c.is_ascii_uppercase())
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
}

/// Identifiers of every `SOURCE:` comment block under `directory`.
fn production_words(directory: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                production_words(&path, out)?;
            }
        } else if name.ends_with(".rs") {
            let text = std::fs::read_to_string(&path)?;
            let mut in_block = false;
            for line in text.lines() {
                if !is_comment(line) {
                    in_block = false;
                    continue;
                }
                in_block |= line.contains(MARKER);
                if in_block {
                    identifiers(line, out);
                }
            }
        }
    }
    Ok(())
}

/// Identifiers of every `description` in `value`, at any depth.
fn description_words(value: &toml::Value, out: &mut BTreeSet<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                match value {
                    toml::Value::String(text) if key == "description" => identifiers(text, out),
                    other => description_words(other, out),
                }
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                description_words(value, out);
            }
        }
        _ => {}
    }
}

/// Identifiers of every description in the TOML files under `directory`:
/// evidence sources and the model's register and field descriptions.
fn register_words(directory: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            register_words(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let table: toml::Table = toml::from_str(&std::fs::read_to_string(&path)?)?;
            description_words(&toml::Value::Table(table), out);
        }
    }
    Ok(())
}

/// Every function of every fetched vendor artifact of `chip`, by artifact.
fn pinned_functions(
    ctx: &Context,
    chip: &str,
) -> Result<BTreeMap<String, (PathBuf, Vec<Function>)>> {
    let mut out = BTreeMap::new();
    for artifact in crate::vendor_fetch::pinned(ctx, chip)? {
        if artifact.local {
            continue;
        }
        let bytes = std::fs::read(&artifact.path)
            .map_err(|e| format!("{}: {e}", artifact.path.display()))?;
        if !crate::vendor_fingerprint::is_binary(&bytes) {
            continue;
        }
        let functions = crate::vendor_fingerprint::functions(&bytes)?;
        out.insert(artifact.id, (artifact.path, functions));
    }
    Ok(out)
}

struct Survey {
    registry: Vec<Entry>,
    /// Current definitions: (artifact, member, symbol) -> code.
    current: BTreeMap<(String, String, String), String>,
    /// Referenced names: cited identifiers that name a current or registered
    /// function.
    references: BTreeSet<String>,
    /// Every cited identifier, whether or not it names a function.
    words: BTreeSet<String>,
    pinned: BTreeMap<String, (PathBuf, Vec<Function>)>,
}

fn survey(ctx: &Context, chip: &str) -> Result<Survey> {
    let registry = match std::fs::read_to_string(ctx.root.join(registry_path(chip)?)) {
        Ok(text) => parse_registry(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let pinned = pinned_functions(ctx, chip)?;
    let mut current = BTreeMap::new();
    for (artifact, (_, functions)) in &pinned {
        for f in functions {
            current.insert(
                (artifact.clone(), f.member.clone(), f.name.clone()),
                f.code.clone(),
            );
        }
    }
    let mut words = BTreeSet::new();
    production_words(&ctx.root.join(PRODUCTION), &mut words)?;
    for directory in register_directories(chip) {
        register_words(&ctx.root.join(directory), &mut words)?;
    }
    let known: BTreeSet<&str> = current
        .keys()
        .map(|(_, _, name)| name.as_str())
        .chain(registry.iter().map(|e| e.symbol.as_str()))
        .collect();
    let references = words
        .iter()
        .filter(|w| known.contains(w.as_str()))
        .cloned()
        .collect();
    Ok(Survey {
        registry,
        current,
        references,
        words,
        pinned,
    })
}

/// Every provenance violation of `chip`, one line each.
pub fn violations(ctx: &Context, chip: &str) -> Result<Vec<String>> {
    let survey = survey(ctx, chip)?;
    let mut problems = vec![];
    let registered: BTreeSet<(&str, &str, &str)> = survey
        .registry
        .iter()
        .map(|e| (e.artifact.as_str(), e.member.as_str(), e.symbol.as_str()))
        .collect();
    for entry in &survey.registry {
        let key = (
            entry.artifact.clone(),
            entry.member.clone(),
            entry.symbol.clone(),
        );
        if !survey.references.contains(&entry.symbol) {
            problems.push(format!(
                "{}::{} is registered but no longer cited",
                entry.artifact, entry.symbol
            ));
        }
        match survey.current.get(&key) {
            None => problems.push(format!(
                "{}[{}]::{} is cited but absent from the pinned artifact; it was renamed or \
                 removed (see `cargo xtask vendor-diff`)",
                entry.artifact, entry.member, entry.symbol
            )),
            Some(code) if *code != entry.code => problems.push(format!(
                "{}[{}]::{} changed since its cited facts were reviewed",
                entry.artifact, entry.member, entry.symbol
            )),
            Some(_) => {}
        }
    }
    for (artifact, member, symbol) in survey.current.keys() {
        if survey.references.contains(symbol)
            && !registered.contains(&(artifact.as_str(), member.as_str(), symbol.as_str()))
        {
            problems.push(format!(
                "{artifact}[{member}]::{symbol} is cited but not registered"
            ));
        }
    }
    Ok(problems)
}

/// The check: fails with every violation.
pub fn check(ctx: &Context, chip: &str) -> Result<()> {
    let problems = violations(ctx, chip)?;
    if problems.is_empty() {
        println!(
            "vendor provenance passed: {} cited functions",
            survey_count(ctx, chip)?
        );
        return Ok(());
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    Err(format!("{} vendor provenance violations", problems.len()).into())
}

fn survey_count(ctx: &Context, chip: &str) -> Result<usize> {
    Ok(parse_registry(&std::fs::read_to_string(
        ctx.root.join(registry_path(chip)?),
    )?)?
    .len())
}

/// Record reviewed fingerprints. `accept` names the functions whose pinned
/// code was reviewed; `rebuild` recomputes the whole registry from the
/// current citations, taking fingerprints from the namesake files in
/// `baseline` (the revision the facts were observed in) where present.
pub fn update(
    ctx: &Context,
    chip: &str,
    accept: &[String],
    rebuild: bool,
    baseline: Option<PathBuf>,
) -> Result<()> {
    let survey = survey(ctx, chip)?;
    let mut entries: BTreeSet<Entry> = if rebuild {
        BTreeSet::new()
    } else {
        survey.registry.iter().cloned().collect()
    };
    let current_entries = |symbol: &str| -> Vec<Entry> {
        survey
            .current
            .iter()
            .filter(|((_, _, name), _)| name == symbol)
            .map(|((artifact, member, name), code)| Entry {
                artifact: artifact.clone(),
                member: member.clone(),
                symbol: name.clone(),
                code: code.clone(),
            })
            .collect()
    };
    if rebuild {
        let mut baselines: BTreeMap<String, Vec<Function>> = BTreeMap::new();
        if let Some(directory) = &baseline {
            for (artifact, (path, _)) in &survey.pinned {
                let Some(name) = path.file_name() else {
                    continue;
                };
                let old = directory.join(name);
                if old.is_file() {
                    baselines.insert(artifact.clone(), crate::vendor_diff::read(&old)?);
                }
            }
        }
        // Cited names only the baseline defines: renamed or removed since
        // their facts were observed.
        for (artifact, functions) in &baselines {
            for f in functions {
                if survey.words.contains(&f.name)
                    && !survey.current.keys().any(|(_, _, name)| *name == f.name)
                {
                    entries.insert(Entry {
                        artifact: artifact.clone(),
                        member: f.member.clone(),
                        symbol: f.name.clone(),
                        code: f.code.clone(),
                    });
                }
            }
        }
        for symbol in &survey.references {
            for entry in current_entries(symbol) {
                let observed = baselines.get(&entry.artifact).and_then(|functions| {
                    functions
                        .iter()
                        .find(|f| f.name == entry.symbol)
                        .map(|f| f.code.clone())
                });
                // A function the baseline lacks is new: its citation was
                // written against the current revision.
                let code = observed.unwrap_or_else(|| entry.code.clone());
                entries.insert(Entry { code, ..entry });
            }
        }
    }
    for symbol in accept {
        entries.retain(|e| &e.symbol != symbol);
        let current = current_entries(symbol);
        if current.is_empty() {
            println!("{symbol}: no pinned definition; its registration is removed");
        }
        entries.extend(current);
    }
    let entries: Vec<Entry> = entries.into_iter().collect();
    std::fs::write(
        ctx.root.join(registry_path(chip)?),
        render_registry(&entries),
    )?;
    println!("{} registered functions", entries.len());
    Ok(())
}

#[cfg(test)]
mod tests;
