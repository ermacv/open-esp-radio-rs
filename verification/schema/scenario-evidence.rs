//! Native vendor-comparison evidence index: typed-scenario verdicts per vendor
//! root, and the source digests that keep them current. The index is a
//! directory of shards, one per scenario, so each scenario run rewrites only
//! its own file. A scenario writes its shard after it passed; qualification
//! consumes the directory. Shards carry identities and verdicts only, never
//! vendor bytes.
//!
//! Some facts span scenarios: a vendor location is untriaged only when no
//! claim of any scenario covers it, and a production line is unobserved only
//! when no scenario observes it. Each shard therefore keeps the scenario-local
//! sets from which [`Evidence`] derives those cross-scenario views exactly.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Shard format.
pub const SCHEMA: u32 = 7;
/// Producer command recorded in every shard.
pub const COMMAND: &str = "vendor-scenario";
/// Extension of a shard file; its stem is the scenario name.
pub const SHARD_EXTENSION: &str = "json";
/// The only verdict an entry carries: a claim exists only when its root
/// compared with MATCH in every comparison of the run that selects it.
pub const MATCH: &str = "match";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Index {
    pub schema: u32,
    pub command: String,
    pub target: String,
    /// The scenario whose claims this shard holds; the file stem.
    pub scenario: String,
    /// SHA-256 of each authenticated input the scenario captured, by session
    /// input index, including the production probe ELF.
    pub inputs: BTreeMap<String, String>,
    /// Directory digests every verdict depends on: the production crates of
    /// the scenario's probe image, the probes, the scenario code and the
    /// Blobray engine.
    pub sources: Vec<SourceDigest>,
    pub entries: Vec<Entry>,
    /// Uncovered vendor locations of the scenario's claimed root closures
    /// that no reviewed decision excludes and no claim of this scenario whose
    /// closure contains their function covers, ascending and unique.
    pub untriaged: Vec<Location>,
    /// Every vendor function the scenario's claimed closures contain,
    /// ascending and unique: another scenario's untriaged location in one of
    /// them is covered unless it is untriaged here too.
    pub functions: Vec<String>,
    /// Production hardware source lines the scenario executed without
    /// observing them and no reviewed decision covers, ascending and unique.
    pub unobserved: Vec<SourceLine>,
    /// Production hardware source lines the scenario observed, ascending and
    /// unique.
    pub observed: Vec<SourceLine>,
    /// Persistent vendor bytes a claim's cases write without comparing them
    /// and no reviewed decision covers, coalesced by data symbol, ascending.
    pub unprojected: Vec<StateRange>,
}

/// Persistent vendor bytes a claim's compared cases wrote: those every
/// writing case compares, and the others by whether a reviewed decision
/// covers them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct State {
    pub written: u64,
    pub compared: u64,
    pub reviewed: u64,
    pub untriaged: u64,
}

/// Bytes `offset..offset + length` of a vendor data symbol; bytes outside
/// every sized data symbol are named by their hexadecimal address.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StateRange {
    pub symbol: String,
    pub offset: u32,
    pub length: u32,
}

/// Production PHY source lines a claim's executions executed, by whether a
/// compared observation of those executions depends on them. A line another
/// claim observes can be unobserved here; the index lists only lines no
/// scenario observes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Observation {
    pub executed: u64,
    pub observed: u64,
    /// Unobserved lines a reviewed decision covers.
    pub reviewed: u64,
    /// Unobserved lines without a decision.
    pub untriaged: u64,
}

/// One line of a repository source file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SourceLine {
    pub path: PathBuf,
    pub line: u32,
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
    /// Transfer sites the closure leaves open: indirect transfers followed
    /// only to their executed targets, and unresolved transfers.
    pub open: u64,
    /// Uncovered blocks and directions and open sites a reviewed decision
    /// excludes.
    pub excluded: u64,
    /// Uncovered blocks and directions and open sites without a decision.
    pub untriaged: u64,
}

/// One vendor block, branch direction or open transfer site, by function
/// symbol and offset.
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
    /// An indirect transfer the closure follows only to its executed
    /// targets; targets no execution reached are outside the closure.
    Followed,
    /// A transfer whose target the closure cannot determine, or that leaves
    /// the executable captured code.
    Unresolved,
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
    /// SHA-256 digests of the reviewed content of the effect contracts and
    /// output projections those comparisons selected, ascending and unique:
    /// rules or fields, applicability and reason, without the endpoints that
    /// name one run's imported revision, so unchanged sources reproduce them.
    /// The contracts are typed values reviewed through git.
    pub reviews: Vec<String>,
    pub coverage: Coverage,
    pub observation: Observation,
    pub state: State,
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
        if self.schema != SCHEMA
            || self.command != COMMAND
            || self.target != target
            || !is_relative(Path::new(&self.scenario))
            || self.scenario.contains('.')
        {
            return Err("unsupported scenario evidence shard".into());
        }
        if self.entries.iter().any(|e| e.suite != self.scenario) {
            return Err(format!("shard {} holds another scenario's entry", self.scenario).into());
        }
        if self.functions.windows(2).any(|w| w[0] >= w[1]) {
            return Err("closure functions are not ascending and unique".into());
        }
        if self.observed.windows(2).any(|w| w[0] >= w[1])
            || self.observed.iter().any(|l| !is_relative(&l.path))
        {
            return Err("observed lines are not relative, ascending and unique".into());
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
                        + c.open
            {
                return Err(format!(
                    "entry {} {} has inconsistent coverage",
                    entry.source, entry.symbol
                )
                .into());
            }
            let o = &entry.observation;
            if o.observed + o.reviewed + o.untriaged != o.executed {
                return Err(format!(
                    "entry {} {} has inconsistent observation",
                    entry.source, entry.symbol
                )
                .into());
            }
            let s = &entry.state;
            if s.compared + s.reviewed + s.untriaged != s.written {
                return Err(format!(
                    "entry {} {} has inconsistent state",
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
        if self.unobserved.windows(2).any(|w| w[0] >= w[1])
            || self.unobserved.iter().any(|l| !is_relative(&l.path))
        {
            return Err("unobserved lines are not relative, ascending and unique".into());
        }
        if self.unprojected.iter().any(|r| r.length == 0)
            || self.unprojected.windows(2).any(|w| {
                w[0].symbol > w[1].symbol
                    || (w[0].symbol == w[1].symbol
                        && u64::from(w[0].offset) + u64::from(w[0].length)
                            >= u64::from(w[1].offset))
            })
        {
            return Err("unprojected state is not coalesced, ascending and unique".into());
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

/// Every shard of an index directory with whether its sources are current.
/// A missing directory holds no shard.
pub struct Evidence {
    pub shards: Vec<(Index, bool)>,
}

impl Evidence {
    /// Load and validate every shard of `directory` below `root`.
    pub fn load(root: &Path, directory: &Path, target: &str) -> Result<Self> {
        let mut shards = vec![];
        let entries = match fs::read_dir(root.join(directory)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { shards });
            }
            Err(error) => return Err(error.into()),
        };
        let mut paths = entries
            .map(|entry| Ok(entry?.path()))
            .collect::<Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some(SHARD_EXTENSION) {
                return Err(format!("unexpected evidence file {}", path.display()).into());
            }
            let shard: Index = serde_json::from_str(&fs::read_to_string(&path)?)?;
            shard
                .validate(target)
                .map_err(|error| format!("evidence shard {}: {error}", path.display()))?;
            if path.file_stem().and_then(|s| s.to_str()) != Some(shard.scenario.as_str()) {
                return Err(
                    format!("evidence shard {} names another scenario", path.display()).into(),
                );
            }
            let current = shard.is_current(root);
            shards.push((shard, current));
        }
        Ok(Self { shards })
    }

    /// Untriaged locations no scenario covers: untriaged in some shard and
    /// in every shard whose closures contain their function.
    pub fn untriaged(&self) -> Vec<Location> {
        let mut all: Vec<Location> = self
            .shards
            .iter()
            .flat_map(|(s, _)| s.untriaged.iter().cloned())
            .filter(|location| {
                self.shards.iter().all(|(s, _)| {
                    s.functions.binary_search(&location.function).is_err()
                        || s.untriaged.binary_search(location).is_ok()
                })
            })
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// Lines some scenario executed without observing and no scenario
    /// observes.
    pub fn unobserved(&self) -> Vec<SourceLine> {
        let observed: std::collections::BTreeSet<&SourceLine> =
            self.shards.iter().flat_map(|(s, _)| &s.observed).collect();
        let lines: std::collections::BTreeSet<SourceLine> = self
            .shards
            .iter()
            .flat_map(|(s, _)| &s.unobserved)
            .filter(|l| !observed.contains(l))
            .cloned()
            .collect();
        lines.into_iter().collect()
    }
}

fn is_relative(path: &Path) -> bool {
    path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
