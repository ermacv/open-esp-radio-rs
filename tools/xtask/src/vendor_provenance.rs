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
//! fingerprint; the registry diff is the review record. `NAME` is a symbol,
//! or the `artifact[member]::symbol` form the check prints, which narrows it
//! to that member; a name neither registered nor pinned is an error.
//!
//! A bare name cites every pinned copy of the function: the vendor library's
//! and, where the ROM carries one, the ROM's. A citation in the same
//! `artifact[member]::symbol` form (`libpp[pm.o]::pm_parse_beacon`,
//! `rom[]::pm_tbtt_process`) cites only that copy, so a fact reviewed
//! against one copy does not claim the others; naming a copy no pinned
//! artifact defines is an error. Accepting a copy no citation names any
//! longer removes its registration.
use crate::vendor_fingerprint::Function;
use crate::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tracked registry of `chip`, relative to the repository root.
fn registry_path(root: &Path, chip: &str) -> Result<String> {
    Ok(crate::chips::Chip::new(root, chip)?.provenance_registry())
}

/// Register-model evidence and model of `chip`, relative to the
/// repository root.
fn register_directories(chip: &str) -> [String; 2] {
    [
        format!("registers/{chip}/evidence"),
        format!("registers/{chip}/model"),
    ]
}

/// Production sources scanned for `SOURCE:` blocks: Rust files and, with
/// `#` comments, TOML files such as the platform's ROM function summaries.
const PRODUCTION: [&str; 2] = ["crates", "platform"];
/// The chip's reviewed verification decisions, relative to its
/// verification directory: the coverage decisions' data and the scenario
/// crate's decision modules.
const DECISIONS: [&str; 2] = ["decisions", "scenarios/src/decisions"];
/// Extensions of the decision files.
const DECISION_EXTENSIONS: [&str; 2] = ["toml", "rs"];
/// Shortest identifier taken as a reference; shorter words are prose.
const MINIMUM_NAME: usize = 4;

/// One registered function definition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub artifact: String,
    pub member: String,
    pub symbol: String,
    pub code: String,
    /// The verification decision files that name the function: their
    /// exclusions describe its code at `code`.
    pub decisions: Vec<String>,
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
        let decisions = value
            .get("decisions")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "registry decision is not a file name".into())
            })
            .collect::<Result<Vec<String>>>()?;
        entries.push(Entry {
            artifact: field("artifact")?,
            member: field("member")?,
            symbol: field("symbol")?,
            code: field("code")?,
            decisions,
        });
    }
    Ok(entries)
}

fn render_registry(entries: &[Entry]) -> String {
    let mut text = String::from(
        "# Code fingerprints of the vendor functions production `SOURCE:` blocks\n\
         # and the register model cite, and those the verification decisions\n\
         # exclude code of, as reviewed. Maintained by\n\
         # `cargo xtask vendor-provenance`; checked by `cargo xtask check\n\
         # provenance`.\n\
         schema = 1\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "\n[[function]]\nartifact = \"{}\"\nmember = \"{}\"\nsymbol = \"{}\"\ncode = \"{}\"\n",
            entry.artifact, entry.member, entry.symbol, entry.code
        ));
        if !entry.decisions.is_empty() {
            let files: Vec<String> = entry.decisions.iter().map(|f| format!("\"{f}\"")).collect();
            text.push_str(&format!("decisions = [{}]\n", files.join(", ")));
        }
    }
    text
}

/// Code-shaped identifiers of `text`. A word directly followed by `.o` is
/// an archive member name such as `phy_init.o`, not a function.
fn identifiers(text: &str, out: &mut BTreeSet<String>) {
    let mut word = String::new();
    let characters: Vec<char> = text.chars().chain([' ']).collect();
    for (index, &c) in characters.iter().enumerate() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            let member = c == '.'
                && characters.get(index + 1) == Some(&'o')
                && !characters
                    .get(index + 2)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
            if word.len() >= MINIMUM_NAME
                && !member
                && !word.starts_with(|c: char| c.is_ascii_digit())
                && is_code_name(&word)
            {
                out.insert(std::mem::take(&mut word));
            }
            word.clear();
        }
    }
}

/// The `artifact[member]::symbol` forms of `text`, and the text without
/// them. Only a form whose `artifact` is a pinned artifact id is a
/// qualified citation; the survey cites any other's symbol bare.
fn qualified_citations(text: &str) -> (String, Vec<(String, String, String)>) {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut bare = String::with_capacity(text.len());
    let mut copies = vec![];
    let mut rest = text;
    while let Some(at) = rest.find("]::") {
        let before = &rest[..at];
        let parsed = before.rfind('[').and_then(|open| {
            let member = &before[open + 1..];
            let artifact_start = before[..open]
                .rfind(|c: char| !(identifier(c) || c == '-' || c == '.'))
                .map_or(0, |i| i + 1);
            let artifact = &before[artifact_start..open];
            let after = &rest[at + 3..];
            let symbol_len = after.find(|c: char| !identifier(c)).unwrap_or(after.len());
            let symbol = &after[..symbol_len];
            (!artifact.is_empty() && !symbol.is_empty() && !member.contains(char::is_whitespace))
                .then(|| {
                    (
                        artifact_start,
                        artifact,
                        member,
                        symbol,
                        at + 3 + symbol_len,
                    )
                })
        });
        match parsed {
            Some((start, artifact, member, symbol, end)) => {
                bare.push_str(&rest[..start]);
                bare.push(' ');
                copies.push((artifact.to_owned(), member.to_owned(), symbol.to_owned()));
                rest = &rest[end..];
            }
            None => {
                bare.push_str(&rest[..at + 3]);
                rest = &rest[at + 3..];
            }
        }
    }
    bare.push_str(rest);
    (bare, copies)
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

/// Code-shaped string literals of every decision file of `directory`, by
/// name, with the files that name them: a decision's `Place` names the
/// vendor function whose code it excludes.
fn decision_words(directory: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut words: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(words);
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DECISION_EXTENSIONS.contains(&e))
        {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("decision file name")?
            .to_owned();
        for literal in std::fs::read_to_string(&path)?
            .split('"')
            .skip(1)
            .step_by(2)
        {
            if !literal.is_empty()
                && literal
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && is_code_name(literal)
            {
                words
                    .entry(literal.to_owned())
                    .or_default()
                    .insert(file.clone());
            }
        }
    }
    Ok(words)
}

/// The chips with a vendor verification project, which a citation may name.
fn citable_chips(root: &Path, supported: &[String]) -> Vec<String> {
    supported
        .iter()
        .filter(|chip| crate::chips::Chip::new(root, chip).is_ok())
        .cloned()
        .collect()
}

/// Identifiers of every `SOURCE:` comment block under `directory` that cites
/// `chip`, a violation for every block that is malformed or names its chips
/// wrongly, and the location and identifiers of every chip-neutral block
/// that names no chip (see [`crate::source_citation`]). Paths are reported
/// and placed relative to `base`.
/// A violation for every chip-neutral block without a chip list that cites
/// one of the scanned chip's `known` functions. A block that cites no vendor
/// function, such as a standard or a HIL record, needs no chip.
fn uncharted_citations(
    uncharted: &[(String, BTreeSet<String>)],
    known: &BTreeSet<&str>,
) -> Vec<String> {
    uncharted
        .iter()
        .filter_map(|(location, block)| {
            block
                .iter()
                .find(|w| known.contains(w.as_str()))
                .map(|function| {
                    format!(
                        "{location}: SOURCE block cites `{function}` under a chip-neutral path but \
                     names no chip"
                    )
                })
        })
        .collect()
}

/// The chip a production scan attributes blocks to.
struct Scan<'a> {
    /// Base the scanned paths are placed and reported relative to.
    base: &'a Path,
    chip: &'a str,
    supported: &'a [String],
    /// Chips a citation may name.
    citable: &'a [String],
}

/// What a production scan found.
#[derive(Default)]
struct Found {
    words: BTreeSet<String>,
    /// Copies cited by `artifact[member]::symbol`, by symbol.
    qualified: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Where each qualified citation was written, for its diagnostics.
    qualified_at: Vec<(String, String, String, String)>,
    problems: Vec<String>,
    /// Location and identifiers of every neutral block that names no chip.
    uncharted: Vec<(String, BTreeSet<String>)>,
}

fn production_words(scan: &Scan<'_>, directory: &Path, found: &mut Found) -> Result<()> {
    use crate::source_citation::{Attribution, Syntax, attribute, blocks_in, place};
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                production_words(scan, &path, found)?;
            }
        } else if let Some(syntax) = Syntax::of(name) {
            let relative = path.strip_prefix(scan.base).unwrap_or(&path);
            let Some(place) = place(relative, scan.chip, scan.supported) else {
                continue;
            };
            let text = std::fs::read_to_string(&path)?;
            let blocks = match blocks_in(&text, syntax) {
                Ok(blocks) => blocks,
                Err(malformed) => {
                    found.problems.push(format!(
                        "{}:{}: malformed SOURCE marker: {}",
                        relative.display(),
                        malformed.line,
                        malformed.reason
                    ));
                    continue;
                }
            };
            for block in blocks {
                match attribute(&block, place, scan.chip, scan.citable) {
                    Attribution::Cited => {
                        let (bare, copies) = qualified_citations(&block.text);
                        identifiers(&bare, &mut found.words);
                        for (artifact, member, symbol) in copies {
                            found.qualified_at.push((
                                format!("{}:{}", relative.display(), block.line),
                                artifact.clone(),
                                member.clone(),
                                symbol.clone(),
                            ));
                            found
                                .qualified
                                .entry(symbol)
                                .or_default()
                                .insert((artifact, member));
                        }
                    }
                    Attribution::NotCited => {}
                    Attribution::Uncharted => {
                        let mut words = BTreeSet::new();
                        identifiers(&block.text, &mut words);
                        found
                            .uncharted
                            .push((format!("{}:{}", relative.display(), block.line), words));
                    }
                    Attribution::Invalid(reason) => found.problems.push(format!(
                        "{}:{}: SOURCE block {reason}",
                        relative.display(),
                        block.line
                    )),
                }
            }
        }
    }
    Ok(())
}

/// The function names of the platform's ROM summaries at `path`: each one
/// cites its function whatever the name's shape (`memset` is code there, not
/// prose). No file, no names.
fn summary_names(path: &Path) -> Result<BTreeSet<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(BTreeSet::new());
    };
    let table: toml::Table =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut names = BTreeSet::new();
    for function in table
        .get("function")
        .and_then(|f| f.as_array())
        .into_iter()
        .flatten()
    {
        let name = function
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{}: a [[function]] without a name", path.display()))?;
        names.insert(name.to_owned());
    }
    Ok(names)
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

/// Every function of every fetched vendor artifact of `chip`, by artifact,
/// and the name of every symbol they define.
#[allow(clippy::type_complexity, reason = "read once and split by the survey")]
fn pinned_functions(
    ctx: &Context,
    chip: &str,
) -> Result<(BTreeMap<String, (PathBuf, Vec<Function>)>, BTreeSet<String>)> {
    let mut out = BTreeMap::new();
    let mut symbols = BTreeSet::new();
    for artifact in crate::vendor_fetch::pinned(ctx, chip)? {
        if artifact.local {
            continue;
        }
        let bytes = std::fs::read(&artifact.path)
            .map_err(|e| format!("{}: {e}", artifact.path.display()))?;
        if !crate::vendor_fingerprint::is_binary(&bytes) {
            continue;
        }
        let (functions, defined) = crate::vendor_fingerprint::functions_and_symbols(&bytes)?;
        symbols.extend(defined);
        out.insert(artifact.id, (artifact.path, functions));
    }
    Ok((out, symbols))
}

struct Survey {
    /// Malformed or wrongly attributed `SOURCE` blocks.
    citations: Vec<String>,
    registry: Vec<Entry>,
    /// Current definitions: (artifact, member, symbol) -> code.
    current: BTreeMap<(String, String, String), String>,
    /// Referenced names: bare cited identifiers that name a current or
    /// registered function, citing every copy.
    references: BTreeSet<String>,
    /// Copies cited by `artifact[member]::symbol`, by symbol.
    qualified: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Each qualified citation with where it was written.
    qualified_at: Vec<(String, String, String, String)>,
    /// Every cited identifier, whether or not it names a function.
    words: BTreeSet<String>,
    /// Names the verification decisions cite, with the files citing them.
    decisions: BTreeMap<String, BTreeSet<String>>,
    pinned: BTreeMap<String, (PathBuf, Vec<Function>)>,
    /// Obfuscated vendor symbols the chip's vendor documents name that no
    /// pinned artifact defines.
    documents: Vec<String>,
}

fn survey(ctx: &Context, chip: &str) -> Result<Survey> {
    let registry = match std::fs::read_to_string(ctx.root.join(registry_path(&ctx.root, chip)?)) {
        Ok(text) => parse_registry(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let (pinned, defined) = pinned_functions(ctx, chip)?;
    let mut current = BTreeMap::new();
    for (artifact, (_, functions)) in &pinned {
        for f in functions {
            current.insert(
                (artifact.clone(), f.member.clone(), f.name.clone()),
                f.code.clone(),
            );
        }
    }
    let supported = crate::chips::supported(&ctx.root)?;
    let scanned = crate::chips::Chip::new(&ctx.root, chip)?;
    let citable = citable_chips(&ctx.root, &supported);
    let mut found = Found::default();
    for directory in PRODUCTION {
        production_words(
            &Scan {
                base: &ctx.root,
                chip: scanned.name(),
                supported: &supported,
                citable: &citable,
            },
            &ctx.root.join(directory),
            &mut found,
        )?;
    }
    let Found {
        mut words,
        qualified: forms,
        qualified_at: forms_at,
        problems: mut citations,
        uncharted,
    } = found;
    // `libpp[pm.o]::f` names one copy; `libpp.a[pm.o]::f` or any other
    // form whose artifact is no pinned id cites `f` bare.
    let mut qualified: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for (symbol, copies) in forms {
        for (artifact, member) in copies {
            if pinned.contains_key(&artifact) {
                qualified
                    .entry(symbol.clone())
                    .or_default()
                    .insert((artifact, member));
            } else {
                words.insert(symbol.clone());
            }
        }
    }
    let qualified_at: Vec<_> = forms_at
        .into_iter()
        .filter(|(_, artifact, _, _)| pinned.contains_key(artifact))
        .collect();

    for directory in register_directories(chip) {
        register_words(&ctx.root.join(directory), &mut words)?;
    }
    let mut decisions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for directory in DECISIONS {
        for (word, files) in decision_words(&ctx.root.join(scanned.verification(directory)))? {
            decisions.entry(word).or_default().extend(files);
        }
    }
    words.extend(decisions.keys().cloned());
    words.extend(summary_names(&ctx.root.join(scanned.rom_summaries()))?);
    let known: BTreeSet<&str> = current
        .keys()
        .map(|(_, _, name)| name.as_str())
        .chain(registry.iter().map(|e| e.symbol.as_str()))
        .collect();
    citations.extend(uncharted_citations(&uncharted, &known));
    let references = words
        .iter()
        .filter(|w| known.contains(w.as_str()))
        .cloned()
        .collect();
    let documents = if pinned.is_empty() {
        vec![]
    } else {
        document_symbols(ctx, chip, &pinned, &defined)?
    };
    // Only names of pinned or registered functions are decision citations.
    let decisions = decisions
        .into_iter()
        .filter(|(name, _)| known.contains(name.as_str()))
        .collect();
    Ok(Survey {
        documents,
        registry,
        current,
        references,
        qualified,
        qualified_at,
        words,
        decisions,
        pinned,
        citations,
    })
}

impl Survey {
    /// Whether some citation names this copy: its bare name, or the copy.
    fn cites(&self, artifact: &str, member: &str, symbol: &str) -> bool {
        self.references.contains(symbol)
            || self
                .qualified
                .get(symbol)
                .is_some_and(|copies| copies.contains(&(artifact.to_owned(), member.to_owned())))
    }
}

/// Undefined obfuscated symbols of every document under `docs/vendor/<chip>`.
fn document_symbols(
    ctx: &Context,
    chip: &str,
    pinned: &BTreeMap<String, (PathBuf, Vec<Function>)>,
    defined: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let directory = ctx.root.join("docs/vendor").join(chip);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(vec![]);
    };
    let artifacts = pinned.keys().cloned().collect::<Vec<_>>().join(", ");
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().and_then(|e| e.to_str()) == Some("md"));
    paths.sort();
    let mut problems = vec![];
    for path in paths {
        let relative = path.strip_prefix(&ctx.root).unwrap_or(&path);
        problems.extend(docs::undefined_symbols(
            &relative.display().to_string(),
            &std::fs::read_to_string(&path)?,
            defined,
            &artifacts,
        ));
    }
    Ok(problems)
}

/// Every provenance violation of `chip`, one line each.
pub fn violations(ctx: &Context, chip: &str) -> Result<Vec<String>> {
    Ok(problems_of(&survey(ctx, chip)?))
}

/// The provenance problems of one survey.
fn problems_of(survey: &Survey) -> Vec<String> {
    let mut problems = survey.citations.clone();
    for (at, artifact, member, symbol) in &survey.qualified_at {
        if !survey
            .current
            .contains_key(&(artifact.clone(), member.clone(), symbol.clone()))
        {
            problems.push(format!(
                "{at}: cites {artifact}[{member}]::{symbol}, which no pinned artifact defines"
            ));
        }
    }
    problems.extend(survey.documents.iter().cloned());
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
        if !survey.cites(&entry.artifact, &entry.member, &entry.symbol) {
            problems.push(format!(
                "{}[{}]::{} is registered but no longer cited; `cargo xtask vendor-provenance --accept {}[{}]::{}` removes it",
                entry.artifact, entry.member, entry.symbol, entry.artifact, entry.member, entry.symbol
            ));
        }
        match survey.current.get(&key) {
            None => problems.push(format!(
                "{}[{}]::{} is cited but absent from the pinned artifact; it was renamed or \
                 removed (see `cargo xtask vendor-diff`)",
                entry.artifact, entry.member, entry.symbol
            )),
            Some(code) if *code != entry.code => problems.push(format!(
                "{}[{}]::{} changed since its cited facts were reviewed{}",
                entry.artifact,
                entry.member,
                entry.symbol,
                review_hint(survey.decisions.get(&entry.symbol))
            )),
            Some(_) => {}
        }
        let citing: Vec<String> = survey
            .decisions
            .get(&entry.symbol)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        if citing != entry.decisions {
            problems.push(format!(
                "{}::{} is registered for decisions {:?} but cited by {:?}; review them and run \
                 `cargo xtask vendor-provenance --accept {}`",
                entry.artifact, entry.symbol, entry.decisions, citing, entry.symbol
            ));
        }
    }
    for (artifact, member, symbol) in survey.current.keys() {
        if survey.cites(artifact, member, symbol)
            && !registered.contains(&(artifact.as_str(), member.as_str(), symbol.as_str()))
        {
            problems.push(format!(
                "{artifact}[{member}]::{symbol} is cited but not registered"
            ));
        }
    }
    problems
}

/// What a changed function's reviewer re-reads: the exclusions of every
/// decision file that names it.
fn review_hint(decisions: Option<&BTreeSet<String>>) -> String {
    match decisions {
        Some(files) if !files.is_empty() => {
            let files: Vec<String> = files.iter().map(|f| format!("decisions/{f}")).collect();
            format!("; re-review the exclusions in {}", files.join(", "))
        }
        _ => String::new(),
    }
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
    Ok(registered(ctx, chip)?.len())
}

/// The registered functions of `chip`: every vendor function production,
/// the register model or a verification decision cites.
pub fn registered(ctx: &Context, chip: &str) -> Result<Vec<Entry>> {
    parse_registry(&std::fs::read_to_string(
        ctx.root.join(registry_path(&ctx.root, chip)?),
    )?)
}

/// Record reviewed fingerprints. `accept` names the functions whose pinned
/// code was reviewed, and each prints how its fingerprint moves; with
/// `show`, its annotated pinned code is printed first. `rebuild` recomputes
/// the whole registry from the current citations, taking fingerprints from
/// the namesake files in `baseline` (the revision the facts were observed
/// in) where present.
pub fn update(
    ctx: &Context,
    chip: &str,
    accept: &[String],
    rebuild: bool,
    baseline: Option<PathBuf>,
    show: bool,
) -> Result<()> {
    let survey = survey(ctx, chip)?;
    let decisions_of = |name: &str| -> Vec<String> {
        survey
            .decisions
            .get(name)
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    };
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
                decisions: decisions_of(name),
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
                if (survey.words.contains(&f.name) || survey.qualified.contains_key(&f.name))
                    && !survey.current.keys().any(|(_, _, name)| *name == f.name)
                {
                    entries.insert(Entry {
                        artifact: artifact.clone(),
                        member: f.member.clone(),
                        symbol: f.name.clone(),
                        code: f.code.clone(),
                        decisions: decisions_of(&f.name),
                    });
                }
            }
        }
        let cited = survey
            .references
            .iter()
            .chain(survey.qualified.keys())
            .collect::<BTreeSet<_>>();
        for symbol in cited {
            for entry in current_entries(symbol)
                .into_iter()
                .filter(|e| survey.cites(&e.artifact, &e.member, &e.symbol))
            {
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
    for accepted in accept {
        let (place, symbol) = accepted_function(accepted);
        let selected = |e: &Entry| {
            e.symbol == symbol && place.is_none_or(|(a, m)| e.artifact == a && e.member == m)
        };
        let previous: Vec<Entry> = entries.iter().filter(|e| selected(e)).cloned().collect();
        let registered = !previous.is_empty();
        entries.retain(|e| !selected(e));
        let defined: Vec<Entry> = current_entries(symbol)
            .into_iter()
            .filter(|e| selected(e))
            .collect();
        if defined.is_empty() && !registered {
            return Err(
                format!("{accepted}: neither registered nor defined by a pinned artifact").into(),
            );
        }
        let (current, uncited): (Vec<Entry>, Vec<Entry>) = defined
            .into_iter()
            .partition(|e| survey.cites(&e.artifact, &e.member, &e.symbol));
        if current.is_empty() {
            if uncited.is_empty() {
                println!("{accepted}: no pinned definition; its registration is removed");
            } else {
                for e in &uncited {
                    println!(
                        "{}[{}]::{}: no citation names this copy; its registration is removed",
                        e.artifact, e.member, e.symbol
                    );
                }
            }
        }
        if show
            && !current.is_empty()
            && crate::vendor_scenario::run(ctx, chip, &["show".into(), symbol.into()])?
                != std::process::ExitCode::SUCCESS
        {
            return Err(format!("{accepted}: showing its pinned code failed").into());
        }
        for entry in &current {
            println!("{}", fingerprint_move(entry, &previous));
            if !entry.decisions.is_empty() {
                println!(
                    "{symbol}: accepted with the exclusions of {}",
                    entry.decisions.join(", ")
                );
            }
        }
        entries.extend(current);
    }
    let entries: Vec<Entry> = entries.into_iter().collect();
    std::fs::write(
        ctx.root.join(registry_path(&ctx.root, chip)?),
        render_registry(&entries),
    )?;
    println!("{} registered functions", entries.len());
    Ok(())
}

/// How accepting `entry` moves its registered fingerprint, one line.
fn fingerprint_move(entry: &Entry, previous: &[Entry]) -> String {
    let name = format!("{}[{}]::{}", entry.artifact, entry.member, entry.symbol);
    match previous
        .iter()
        .find(|e| e.artifact == entry.artifact && e.member == entry.member)
    {
        None => format!("{name}: newly registered at {}", entry.code),
        Some(old) if old.code == entry.code => format!("{name}: unchanged at {}", entry.code),
        Some(old) => format!("{name}: {} -> {}", old.code, entry.code),
    }
}

/// The function an `--accept` argument names: a bare symbol, or the
/// `artifact[member]::symbol` form the check prints, which narrows it to
/// that artifact member.
fn accepted_function(accepted: &str) -> (Option<(&str, &str)>, &str) {
    let Some((place, symbol)) = accepted.rsplit_once("::") else {
        return (None, accepted);
    };
    match place
        .strip_suffix(']')
        .and_then(|place| place.split_once('['))
    {
        Some((artifact, member)) => (Some((artifact, member)), symbol),
        None => (None, accepted),
    }
}

mod docs;

#[cfg(test)]
mod tests;
