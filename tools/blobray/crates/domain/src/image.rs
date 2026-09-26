//! Portable synthetic image recipes and retained evidence. No tool discovery or I/O.
use crate::*;

/// ELF-declared floating-point calling convention, separate from instruction
/// coverage and from the integer analysis profile. No execution support implied.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiscvAbi {
    Ilp32,
    Ilp32f,
    Ilp32d,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrySelection {
    pub input: u64,
    pub symbol: SymbolId,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRegion {
    pub start: u32,
    pub length: u64,
}
impl ImageRegion {
    pub fn end(&self) -> Option<u64> {
        u64::from(self.start)
            .checked_add(self.length)
            .filter(|end| *end <= 1u64 << 32)
    }
    pub fn contains(&self, start: u64, length: u64) -> bool {
        start >= u64::from(self.start)
            && start
                .checked_add(length)
                .zip(self.end())
                .is_some_and(|(end, limit)| end <= limit)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLayout {
    pub code: ImageRegion,
    pub data: ImageRegion,
}
impl ImageLayout {
    pub fn validate(&self) -> Result<()> {
        for region in [self.code, self.data] {
            if region.length == 0 || region.start % 4096 != 0 || region.end().is_none() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "image regions must be nonempty, page-aligned and within RV32",
                ));
            }
        }
        if u64::from(self.code.start) < self.data.end().unwrap()
            && u64::from(self.data.start) < self.code.end().unwrap()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "image regions overlap",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    #[serde(default)]
    pub companions: Vec<EntrySelection>,
    pub revision: Option<RevisionId>,
    /// Zero-based captured input occurrences, in linker order. No implicit inputs.
    pub inputs: Vec<u64>,
    pub entry: EntrySelection,
    #[serde(default)]
    pub roots: Vec<EntrySelection>,
    pub layout: ImageLayout,
    /// Undefined names no captured input defines, deliberately bound to
    /// `ABSENT_SYMBOL_ADDRESS`: executing or reading one stops with an
    /// execution gap, so only never-reached references may name them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
}
/// Companions proposed for a link request by a trial link. Every name the
/// selected closure leaves unresolved is either resolved to exactly one defined
/// function or data object of the first candidate input that defines it, or
/// reported unresolved. A proposal grants nothing: the client copies the exact
/// selections into its retained link request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionProposal {
    pub resolved: Vec<ProposedCompanion>,
    /// Linker-visible names without a definition in any candidate input.
    pub unresolved: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedCompanion {
    pub name: String,
    pub selection: EntrySelection,
}
/// Native convenience selection. Only one defined entry in the explicit input
/// qualifies; ambiguity returns candidates, never a preferred definition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedLinkRequest {
    #[serde(default)]
    pub companions: Vec<NamedCompanion>,
    pub revision: Option<RevisionId>,
    pub inputs: Vec<u64>,
    pub entry_input: u64,
    pub entry_name: Vec<u8>,
    pub layout: ImageLayout,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkerIdentity {
    pub implementation: String,
    pub version: String,
    pub executable: ArtifactId,
}
/// Roots of one linked image: the entry and every additional root.
pub const MAX_IMAGE_ROOTS: usize = 64;
/// Unmapped address every absent name resolves to.
pub const ABSENT_SYMBOL_ADDRESS: u32 = 0xffff_fff0;
/// Absent names one link request may declare.
pub const MAX_ABSENT_SYMBOLS: usize = 64;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRecipe {
    pub linker_contract: LinkerContract,
    pub companions: Vec<EntrySelection>,
    pub schema: u32,
    pub policy: u32,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub inputs: Vec<u64>,
    pub entry: EntrySelection,
    pub roots: Vec<EntrySelection>,
    pub layout: ImageLayout,
    pub linker: LinkerIdentity,
    /// Names bound to `ABSENT_SYMBOL_ADDRESS`; see `LinkRequest::absent`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBlocker {
    pub input: Option<u64>,
    pub code: ErrorCode,
    pub message: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPlanDescription {
    pub id: LinkPlanId,
    pub recipe: LinkRecipe,
    pub blockers: Vec<LinkBlocker>,
}
impl LinkPlanDescription {
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSegment {
    pub address: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub flags: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRoot {
    pub selection: EntrySelection,
    pub address: u64,
    pub size: u64,
    pub name: Vec<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageManifest {
    pub abi: RiscvAbi,
    /// Bounded observations from the successful linker process.
    pub linker_diagnostics: LinkerDiagnostics,
    pub schema: u32,
    pub synthetic: bool,
    pub plan: LinkPlanDescription,
    pub elf: ArtifactId,
    pub map: ArtifactId,
    pub extraction: ArtifactId,
    pub provenance: ArtifactId,
    pub observations: ArtifactId,
    pub entry: u64,
    pub roots: Vec<ResolvedRoot>,
    pub segments: Vec<ImageSegment>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMapping {
    pub input: u64,
    pub object: ObjectId,
    pub payload: ArtifactId,
    pub section: Vec<u8>,
    pub address: u64,
    pub size: u64,
    /// Exact only when one original section is identified and its extent is unchanged.
    pub exact: bool,
}

/// The successful tool exit and the last 8192 stderr bytes; never an unbounded log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkerDiagnostics {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_tail: Vec<u8>,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedCompanion {
    pub input: u64,
    pub name: String,
}

/// Semantic analysis profile; tool version is independently retained in identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkerContract {
    #[serde(rename = "static-analysis-elf-link-v1")]
    ElfAnalysisLinkV1,
}

/// Physical imported occurrence, including repeated bindings of identical bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkObject {
    pub input: u64,
    pub object: ObjectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkEvidenceSource {
    Map,
    Extraction,
}

/// Byte interval in a retained raw output, before normalization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEvidenceSpan {
    pub source: LinkEvidenceSource,
    pub offset: u64,
    pub length: u64,
}

/// Adapter observations are claims checked by application, not publication authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LinkObservation {
    SectionPlacement {
        object: LinkObject,
        section: Vec<u8>,
        address: u64,
        size: u64,
        evidence: LinkEvidenceSpan,
    },
    ArchiveExtraction {
        object: LinkObject,
        cause: Option<Vec<u8>>,
        referring: Option<LinkObject>,
        evidence: LinkEvidenceSpan,
    },
    ToolExit {
        code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkObservationRecord {
    pub schema: u32,
    pub observation: LinkObservation,
}

impl LinkRecipe {
    /// Shared portable compatibility check; adapters own executable capabilities.
    pub fn validate_contract(&self) -> Result<()> {
        if self.schema != 2
            || self.policy != 5
            || self.linker_contract != LinkerContract::ElfAnalysisLinkV1
        {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported link recipe",
            ));
        }
        if self.linker.implementation.is_empty() || self.linker.version.is_empty() {
            return Err(Error::new(ErrorCode::Integrity, "missing linker identity"));
        }
        Ok(())
    }
}
