//! Native vendor-comparison evidence index: typed-scenario verdicts per vendor
//! root, and the source digests that keep them current. The scenario runner
//! produces it after every scenario passed; qualification consumes it. The
//! index carries identities and verdicts only, never vendor bytes.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Index format.
pub const SCHEMA: u32 = 3;
/// Producer command recorded in every index.
pub const COMMAND: &str = "vendor-scenario all";
/// The only verdict an entry carries: a claim exists only when its root
/// compared with MATCH in every comparison of the run that selects it.
pub const MATCH: &str = "match";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Index {
    pub schema: u32,
    pub command: String,
    pub target: String,
    /// SHA-256 of each authenticated private input and of the production probe ELF.
    pub inputs: BTreeMap<String, String>,
    /// Directory digests every verdict depends on: production crates, probes,
    /// scenario code and the Blobray engine.
    pub sources: Vec<SourceDigest>,
    pub entries: Vec<Entry>,
    /// Uncovered vendor locations of claimed root closures that no reviewed
    /// decision excludes yet, ascending and unique.
    pub untriaged: Vec<Location>,
}

/// Reached and total basic blocks or branch directions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Count {
    pub reached: u64,
    pub total: u64,
}

/// Vendor coverage of one claimed root's closure over its executions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Coverage {
    pub blocks: Count,
    /// Both directions of every conditional branch.
    pub directions: Count,
    /// Uncovered blocks and directions a reviewed decision excludes.
    pub excluded: u64,
    /// Uncovered blocks and directions without a decision.
    pub untriaged: u64,
}

/// One vendor block or branch direction, by function symbol and offset.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Location {
    pub function: String,
    pub offset: u32,
    pub kind: LocationKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum LocationKind {
    Block,
    Taken,
    Fallthrough,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SourceDigest {
    pub path: PathBuf,
    pub sha256: String,
}

/// One vendor root compared with one compiled production entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Entry {
    /// Scenario that owns the comparison.
    pub suite: String,
    /// Vendor source kind (`archive` or `rom`) and root symbol.
    pub source: String,
    pub symbol: String,
    /// Compiled production entry compared with the root.
    pub production: String,
    pub verdict: String,
    /// Compared cases of the root/entry pair.
    pub cases: u64,
    /// SHA-256 content digests of the effect contracts and output
    /// projections those comparisons selected, ascending and unique. The
    /// contracts are typed values reviewed through git.
    pub reviews: Vec<String>,
    pub coverage: Coverage,
}

/// SHA-256 over every file below `relative`, excluding `target` and hidden
/// directories and Markdown documentation, which establishes no verdict:
/// sorted relative paths, each with its length and bytes.
pub fn digest_directory(root: &Path, relative: &Path) -> Result<String> {
    fn walk(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if name == "target" || name.starts_with('.') {
                    continue;
                }
                walk(&entry.path(), files)?;
            } else if kind.is_file() {
                if !name.ends_with(".md") {
                    files.push(entry.path());
                }
            } else {
                return Err(format!("unsupported source entry {}", entry.path().display()).into());
            }
        }
        Ok(())
    }
    let base = root.join(relative);
    let mut files = vec![];
    walk(&base, &mut files)?;
    files.sort();
    let mut hash = Sha256::new();
    for file in files {
        let name = file.strip_prefix(root)?.to_string_lossy().into_owned();
        let bytes = fs::read(&file)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl Index {
    /// A well-formed index for `target`: supported schema and producer,
    /// valid digests, unique entries and only MATCH verdicts.
    pub fn validate(&self, target: &str) -> Result<()> {
        let valid = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.schema != SCHEMA || self.command != COMMAND || self.target != target {
            return Err("unsupported scenario evidence index".into());
        }
        if self.sources.is_empty()
            || self
                .sources
                .iter()
                .any(|s| !valid(&s.sha256) || !is_relative(&s.path))
            || self.inputs.values().any(|v| !valid(v))
        {
            return Err("scenario evidence index has invalid digests".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if entry.verdict != MATCH
                || entry.cases == 0
                || entry.reviews.windows(2).any(|w| w[0] >= w[1])
                || entry.reviews.iter().any(|r| !valid(r))
            {
                return Err(format!(
                    "entry {} {} is not a MATCH claim",
                    entry.source, entry.symbol
                )
                .into());
            }
            let c = &entry.coverage;
            if c.blocks.reached > c.blocks.total
                || c.directions.reached > c.directions.total
                || c.excluded + c.untriaged
                    != (c.blocks.total - c.blocks.reached)
                        + (c.directions.total - c.directions.reached)
                || (c.untriaged != 0 && self.untriaged.is_empty())
            {
                return Err(format!(
                    "entry {} {} has inconsistent coverage",
                    entry.source, entry.symbol
                )
                .into());
            }
            if !seen.insert((
                &entry.suite,
                &entry.source,
                &entry.symbol,
                &entry.production,
            )) {
                return Err(format!("entry {} {} repeats", entry.source, entry.symbol).into());
            }
        }
        if self.untriaged.windows(2).any(|w| w[0] >= w[1]) {
            return Err("untriaged locations are not ascending and unique".into());
        }
        Ok(())
    }

    /// Every recorded source directory still has its recorded digest.
    pub fn is_current(&self, root: &Path) -> bool {
        self.sources.iter().all(|source| {
            digest_directory(root, &source.path).is_ok_and(|digest| digest == source.sha256)
        })
    }
}

fn is_relative(path: &Path) -> bool {
    path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
