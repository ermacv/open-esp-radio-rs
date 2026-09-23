//! Immutable container capture and lazily parsed object-owned code catalogs.

use std::{
    ops::{Deref, Range},
    path::Path,
    sync::{Arc, OnceLock},
};

use object::{FileKind, read::archive::ArchiveFile};
use open_radio_vendor_analysis_model::{ObjectLocation, SymbolLocation};
use sha2::{Digest, Sha256};

use super::{
    ArtifactContainerKind, ArtifactInventory, ArtifactSymbolDefinition, CodeSymbolSelection,
};
use crate::Result;

enum CapturedBytes<'a> {
    Borrowed(&'a [u8]),
    Shared(Arc<[u8]>),
}

impl Deref for CapturedBytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

/// A code definition retains its physical occurrence even if names repeat.
#[derive(Clone, Debug)]
pub struct CapturedCodeSymbol {
    pub location: SymbolLocation,
    pub exported: bool,
    pub definition: ArtifactSymbolDefinition,
}

/// Every payload member has an entry, including unsupported and thin members.
#[derive(Debug)]
pub struct CapturedObject {
    location: ObjectLocation,
    name: Option<String>,
    range: Option<Range<usize>>,
    symbols: OnceLock<std::result::Result<Vec<CapturedCodeSymbol>, String>>,
}

impl CapturedObject {
    pub const fn location(&self) -> ObjectLocation {
        self.location
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    /// `None` denotes external thin-archive bytes that have not been captured.
    pub fn byte_range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }
}

/// Immutable bytes and object catalogs owned by one analysis context.
///
/// File paths are locators only. After capture, filesystem changes cannot alter
/// the bytes or digest behind this handle. Each object's code is parsed once,
/// including cached parse failures. Drop the handle to release those catalogs.
pub struct CapturedArtifact<'a> {
    bytes: CapturedBytes<'a>,
    sha256: String,
    container: ArtifactContainerKind,
    objects: Vec<CapturedObject>,
    inventory: OnceLock<std::result::Result<ArtifactInventory, String>>,
    data_symbols: OnceLock<std::result::Result<Vec<super::ArtifactDataSymbolDefinition>, String>>,
    data_objects: OnceLock<std::result::Result<Vec<super::ArtifactDataObjectDefinition>, String>>,
}

impl CapturedArtifact<'static> {
    /// Parse container ownership over shared immutable source bytes without
    /// copying them. The source-set owner may outlive or drop this handle.
    pub fn from_shared(bytes: Arc<[u8]>) -> Result<Self> {
        Self::capture(CapturedBytes::Shared(bytes))
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::from_shared(crate::read_artifact(path)?.into())
    }
}

impl<'a> CapturedArtifact<'a> {
    pub fn from_data(bytes: &'a [u8]) -> Result<Self> {
        Self::capture(CapturedBytes::Borrowed(bytes))
    }

    fn capture(bytes: CapturedBytes<'a>) -> Result<Self> {
        let mut objects = Vec::new();
        let container = match FileKind::parse(bytes.as_ref())? {
            FileKind::Archive => {
                let archive = ArchiveFile::parse(bytes.as_ref())?;
                for (ordinal, member) in archive.members().enumerate() {
                    let member = member?;
                    let range = if member.is_thin() {
                        None
                    } else {
                        // Validate against the captured container before conversion.
                        member.data(bytes.as_ref())?;
                        let (offset, size) = member.file_range();
                        let start =
                            usize::try_from(offset).map_err(|_| "member offset overflows")?;
                        let length = usize::try_from(size).map_err(|_| "member size overflows")?;
                        let end = start.checked_add(length).ok_or("member range overflows")?;
                        Some(start..end)
                    };
                    objects.push(CapturedObject {
                        location: ObjectLocation::ArchiveMember {
                            ordinal: ordinal as u64,
                        },
                        name: Some(String::from_utf8_lossy(member.name()).into_owned()),
                        range,
                        symbols: OnceLock::new(),
                    });
                }
                ArtifactContainerKind::Archive
            }
            FileKind::Elf32 => {
                objects.push(CapturedObject {
                    location: ObjectLocation::Standalone,
                    name: None,
                    range: Some(0..bytes.len()),
                    symbols: OnceLock::new(),
                });
                ArtifactContainerKind::Elf32
            }
            kind => return Err(format!("unsupported artifact kind: {kind:?}").into()),
        };
        Ok(Self {
            sha256: format!("{:x}", Sha256::digest(bytes.as_ref())),
            bytes,
            container,
            objects,
            inventory: OnceLock::new(),
            data_symbols: OnceLock::new(),
            data_objects: OnceLock::new(),
        })
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub const fn container(&self) -> ArtifactContainerKind {
        self.container
    }
    pub fn objects(&self) -> &[CapturedObject] {
        &self.objects
    }

    /// Complete symbol/member accounting over the same captured bytes as code queries.
    pub fn inventory(&self) -> Result<&ArtifactInventory> {
        self.inventory
            .get_or_init(|| {
                super::inventory::inspect_capture(self).map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| error.clone().into())
    }

    /// Sized linked data symbols from this exact capture; repeated occurrences
    /// are retained even when every display attribute is equal.
    pub fn data_symbols(&self) -> Result<&[super::ArtifactDataSymbolDefinition]> {
        self.data_symbols
            .get_or_init(|| {
                super::symbols::captured_data_symbols(self).map_err(|error| error.to_string())
            })
            .as_ref()
            .map(Vec::as_slice)
            .map_err(|error| error.clone().into())
    }

    /// Static initializers and relocations share the code catalog's bytes and
    /// physical object partition. The parsed result belongs to this capture.
    pub fn data_objects(&self) -> Result<&[super::ArtifactDataObjectDefinition]> {
        self.data_objects
            .get_or_init(|| {
                super::data_objects::captured_data_objects(self).map_err(|error| error.to_string())
            })
            .as_ref()
            .map(Vec::as_slice)
            .map_err(|error| error.clone().into())
    }

    pub fn object(&self, location: ObjectLocation) -> Option<&CapturedObject> {
        let index = match (self.container, location) {
            (ArtifactContainerKind::Elf32, ObjectLocation::Standalone) => 0,
            (ArtifactContainerKind::Archive, ObjectLocation::ArchiveMember { ordinal }) => {
                usize::try_from(ordinal).ok()?
            }
            _ => return None,
        };
        self.objects.get(index)
    }

    /// Exact payload bytes, including objects whose format cannot be analyzed.
    /// Missing locations return `None`; uncaptured thin members return an error.
    pub fn object_bytes(&self, location: ObjectLocation) -> Result<Option<&[u8]>> {
        let Some(object) = self.object(location) else {
            return Ok(None);
        };
        let range = object.range.as_ref().ok_or_else(|| {
            format!(
                "uncaptured thin-archive member {:?} at {:?}",
                object.name, object.location
            )
        })?;
        Ok(Some(&self.bytes[range.clone()]))
    }

    /// Read a physical symbol without using a name/address as a unique key.
    pub fn code_symbol(&self, location: SymbolLocation) -> Result<Option<&CapturedCodeSymbol>> {
        let Some(object) = self.object(location.object) else {
            return Ok(None);
        };
        Ok(self
            .object_symbols(object)?
            .iter()
            .find(|symbol| symbol.location == location))
    }

    /// Reviewed ranges are read from the same immutable bytes as ordinary symbols.
    pub fn reviewed_code_ranges(
        &self,
        ranges: &[super::ReviewedCodeRange],
    ) -> Result<Vec<ArtifactSymbolDefinition>> {
        super::symbols::reviewed_code_ranges(self, ranges)
    }

    pub fn code_symbols(
        &self,
        prefix: &str,
        selection: CodeSymbolSelection,
    ) -> Result<Vec<&CapturedCodeSymbol>> {
        let mut selected = Vec::new();
        for object in &self.objects {
            selected.extend(self.object_symbols(object)?.iter().filter(|symbol| {
                symbol.definition.name.starts_with(prefix)
                    && (selection.includes_local() || symbol.exported)
            }));
        }
        selected.sort_by_key(|symbol| symbol.location);
        Ok(selected)
    }

    fn object_symbols<'b>(&self, object: &'b CapturedObject) -> Result<&'b [CapturedCodeSymbol]> {
        let cached = object.symbols.get_or_init(|| {
            let range = object.range.as_ref().ok_or_else(|| {
                format!(
                    "uncaptured thin-archive member {:?} at {:?}",
                    object.name, object.location
                )
            })?;
            let data = &self.bytes[range.clone()];
            // Unsupported payloads remain objects in this capture. They cannot
            // be lifted as code; the inventory pass records their coverage gap.
            if !matches!(FileKind::parse(data), Ok(FileKind::Elf32)) {
                return Ok(Vec::new());
            }
            super::symbols::collect_object_symbols(
                data,
                object.name(),
                object.location,
                self.sha256(),
            )
            .map_err(|error| format!("object {:?}: {error}", object.location))
        });
        cached
            .as_ref()
            .map(Vec::as_slice)
            .map_err(|error| error.clone().into())
    }
}
