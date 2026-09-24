//! Shared identities and schema-1 import records. No filesystem or database access.
//!
//! Physical identities retain archive ordinals and ELF table section/index pairs.
//! Names and origin paths are lossless metadata, never identity keys. Revisions
//! retain complete inventory outcomes, including unsupported and missing inputs.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

mod data;
pub use data::*;
mod execution;
pub use execution::*;
mod function;
pub use function::*;
mod image;
pub use image::*;
mod temporary;
pub use temporary::*;
mod selection;
pub use selection::*;
mod jobs;
mod resources;
pub use resources::*;
mod memory;
mod stream;
pub use memory::*;
pub use stream::*;
mod validation;
pub use jobs::*;

/// Codes shared by API errors and JSON diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    AlreadyExists,
    Busy,
    Incompatible,
    Integrity,
    SourceChanged,
    DigestMismatch,
    Io,
    Storage,
    DiskFull,
    WorkerExited,
    WorkerProtocol,
    DiagnosticChannel,
    Unavailable,
    Cancelled,
    TimedOut,
    ResourceLimited,
    RecoveryRequired,
    LinkBlocked,
    LinkFailed,
    NeedsExtent,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<Box<StorageFailure>>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            memory: None,
            storage: None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn allocated_bytes(&self) -> u64 {
                self.0.capacity() as u64
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self> {
                if value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        concat!(
                            stringify!($name),
                            " requires 64 lowercase hexadecimal digits"
                        ),
                    ));
                }
                Ok(Self(value))
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                Self::try_from(value.to_owned())
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

identity!(ArtifactId);
identity!(LinkPlanId);
identity!(PreparedImageId);
identity!(FunctionAnalysisId);
identity!(PublicationId);
identity!(KnowledgeRevisionId);
identity!(AssertionId);
identity!(InvestigationPlanId);
identity!(ProjectId);
identity!(RevisionId);
identity!(RunId);
identity!(PlanId);

impl ArtifactId {
    pub fn of_bytes_controlled(bytes: &[u8], control: &mut dyn RunControl) -> Result<Self> {
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        for chunk in bytes.chunks(WORK_BLOCK) {
            control.checkpoint(1)?;
            digest.update(chunk);
        }
        format!("{:x}", digest.finalize()).parse()
    }
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }
}

impl RevisionId {
    /// Content identity of the exact serialized revision manifest.
    pub fn of_manifest(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectLocation {
    Standalone,
    ArchiveMember { ordinal: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectId {
    pub artifact: ArtifactId,
    pub location: ObjectLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolTableKind {
    Static,
    Dynamic,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolId {
    pub object: ObjectId,
    pub table: SymbolTableKind,
    pub table_section: u32,
    pub index: u64,
}

/// Lossless native origin path. Neither variant is used to reopen snapshot data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "kebab-case", deny_unknown_fields)]
pub enum OriginPath {
    UnixBytes { bytes: Vec<u8> },
    WindowsWide { units: Vec<u16> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCode {
    UnavailableInput,
    UnsupportedFormat,
    MalformedContainer,
    MalformedObject,
    MalformedName,
    MalformedTable,
    MissingMember,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub context: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Capture {
    Captured { artifact: ArtifactId, length: u64 },
    Unavailable { diagnostic: Diagnostic },
}

impl Capture {
    pub fn artifact(&self) -> Option<&ArtifactId> {
        match self {
            Self::Captured { artifact, .. } => Some(artifact),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalMember {
    pub ordinal: u64,
    pub name: Vec<u8>,
    pub origin: Option<OriginPath>,
    pub capture: Capture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerKind {
    Elf,
    Archive,
    ThinArchive,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInventory {
    pub kind: ContainerKind,
    /// False if archive framing prevents enumeration of the remaining payloads.
    pub members_complete: bool,
    pub objects: Vec<ObjectInventory>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ArtifactInventory {
    pub fn complete(&self) -> bool {
        self.members_complete
            && self.diagnostics.is_empty()
            && self
                .objects
                .iter()
                .all(|o| o.elf.is_some() && o.diagnostics.is_empty())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectInventory {
    pub id: ObjectId,
    pub name: Option<Vec<u8>>,
    pub content: Option<ArtifactId>,
    pub elf: Option<ElfInventory>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElfInventory {
    pub bits: u8,
    pub little_endian: bool,
    pub machine: u16,
    pub object_type: u16,
    pub entry: u64,
    pub sections: Vec<SectionRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub relocations: Vec<RelocationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionRecord {
    pub index: u32,
    pub name: Option<Vec<u8>>,
    pub section_type: u32,
    pub flags: u64,
    pub address: u64,
    pub file_offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub alignment: u64,
    pub entry_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolRecord {
    pub id: SymbolId,
    pub name: Option<Vec<u8>>,
    pub name_offset: u32,
    pub value: u64,
    pub size: u64,
    pub binding: u8,
    pub symbol_type: u8,
    pub other: u8,
    pub raw_section: u16,
    pub extended_section: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelocationRecord {
    pub section: u32,
    pub index: u64,
    pub offset: u64,
    pub relocation_type: u32,
    pub symbol_table_section: u32,
    pub symbol_index: u32,
    pub addend: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRecord {
    pub role: String,
    pub origin: OriginPath,
    pub expected: Option<ArtifactId>,
    pub capture: Capture,
    pub external_members: Vec<ExternalMember>,
    pub inventory: Option<ArtifactInventory>,
}

/// Selected integer-analysis profile; inventory itself remains format/ISA neutral.
/// ELF floating-point calling convention is recorded separately as `RiscvAbi`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Riscv32Ilp32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub schema: u32,
    pub project: ProjectId,
    pub parent: Option<RevisionId>,
    pub target: Target,
    pub inventory_producer: String,
    pub inputs: Vec<InputRecord>,
}

impl Revision {
    pub fn complete(&self) -> bool {
        self.inputs.iter().all(|i| {
            i.inventory
                .as_ref()
                .is_some_and(ArtifactInventory::complete)
        })
    }

    /// Every separately retained input payload, including captured thin members.
    pub fn captures(&self) -> impl Iterator<Item = &Capture> {
        self.inputs.iter().flat_map(|i| {
            std::iter::once(&i.capture).chain(i.external_members.iter().map(|m| &m.capture))
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub revision_id: RevisionId,
    pub revision: Revision,
}

mod semantics;
pub use semantics::*;

mod investigation;
pub use investigation::*;

mod call_contract;
pub use call_contract::*;
mod function_contract;
pub use function_contract::*;
mod navigation;
pub use navigation::*;
mod access;
pub use access::*;
mod interfaces;
pub use interfaces::*;
mod knowledge;
pub use knowledge::*;

mod audit;
pub use audit::*;

mod assessment;
pub use assessment::*;

mod measurements;
pub use measurements::*;

mod scenarios;
pub use scenarios::*;

mod record_memory;
pub use record_memory::*;

mod reports;
pub use reports::*;

mod flow;
pub use flow::*;

mod memory_slice;
pub use memory_slice::*;
mod event_route;
pub use event_route::*;
