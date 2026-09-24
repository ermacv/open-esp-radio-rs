//! Configured packaging of saved semantic facts. Packaging grants no execution claim.
use crate::*;

pub const SEMANTIC_IR_SCHEMA: u32 = 1;
pub const SEMANTIC_IR_POLICY: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum IrRoots {
    All,
    /// Names come from explicitly selected publication membership. A named
    /// function with unavailable name metadata is an error, not silently excluded.
    NamePrefix {
        prefix: Vec<u8>,
    },
    /// Exact saved interpretations, not a name-based choice among alternatives.
    Analyses {
        analyses: Vec<FunctionAnalysisId>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrProfile {
    pub name: String,
    pub roots: IrRoots,
    pub include_reachable: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrBuildRequest {
    pub scope: NavigationScope,
    pub profiles: Vec<IrProfile>,
}
impl IrBuildRequest {
    /// Validate the bounded configuration. Physical membership and name/target
    /// resolution are application responsibilities, using this frozen scope.
    pub fn validate(&self) -> Result<()> {
        let invalid = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid semantic IR build configuration",
            )
        };
        if self.profiles.is_empty()
            || self.profiles.len() > 32
            || self.scope.publications.len() > 64
            || self.scope.analyses.len() > 256
            || self.scope.publications.is_empty() && self.scope.analyses.is_empty()
        {
            return Err(invalid());
        }
        let mut roots = 0usize;
        for (i, p) in self.profiles.iter().enumerate() {
            if p.name.is_empty()
                || p.name.len() > 128
                || !p
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                || self.profiles[..i].iter().any(|other| other.name == p.name)
            {
                return Err(invalid());
            }
            match &p.roots {
                IrRoots::All => (),
                IrRoots::NamePrefix { prefix } if !prefix.is_empty() && prefix.len() <= 256 => (),
                IrRoots::Analyses { analyses } if !analyses.is_empty() => {
                    roots = roots.checked_add(analyses.len()).ok_or_else(invalid)?;
                    if roots > 256 {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrProfileSummary {
    pub name: String,
    pub roots: u64,
    pub functions: u64,
    pub partial_functions: u64,
    /// Selected calls lacking exactly one target within the selected scope.
    pub unresolved_links: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticIrManifest {
    pub schema: u32,
    pub policy: u32,
    pub project: ProjectId,
    pub request: IrBuildRequest,
    pub records: ArtifactId,
    pub record_count: u64,
    pub functions: u64,
    pub provenance_functions: u64,
    pub unavailable_entries: u64,
    pub profiles: Vec<IrProfileSummary>,
}
/// Stored index rows refer to original immutable function streams. Read/export
/// inserts their complete `Fact` rows without creating a second IR encoding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SemanticIrRecord {
    Function {
        function: NavigationFunction,
        manifest: Box<FunctionManifest>,
        name: Option<Vec<u8>>,
        /// Zero-based indices in the manifest's profile list.
        profiles: Vec<u8>,
        roots: Vec<u8>,
        /// Retained solely as transitive evidence, not added to root/call selection.
        provenance_only: bool,
    },
    Call {
        record: Box<NavigationRecord>,
    },
    Unavailable {
        record: Box<NavigationRecord>,
    },
    Knowledge {
        entry: Box<KnowledgeEntry>,
    },
    Fact {
        analysis: FunctionAnalysisId,
        record: u64,
        fact: Box<FunctionRecord>,
    },
}
