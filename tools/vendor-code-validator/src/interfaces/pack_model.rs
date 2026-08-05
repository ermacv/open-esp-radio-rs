//! Reviewed interface-pack records and the resolved workspace view.

use std::{collections::BTreeSet, fs, path::Path};

use toml_edit::{DocumentMut, Item};

use super::{InterfaceFactStep, InterfaceFacts, SemanticCatalogs};
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReviewStatus {
    Unreviewed,
    Reviewed,
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackOrigin {
    Observed,
    Manual,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InterfaceRootSelector {
    RelocatedSymbol {
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        argument: u8,
    },
    AbsoluteAddress {
        address: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InterfaceGuard {
    ArtifactSha256 {
        sha256: String,
    },
    RuntimeValue {
        purpose: String,
        offset: i32,
        width: u8,
        mask: u64,
        value: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) status: ReviewStatus,
    pub(crate) origin: PackOrigin,
    pub(crate) name: Option<String>,
    pub(crate) arguments: Option<Vec<String>>,
    pub(crate) return_type: Option<String>,
    pub(crate) variadic: bool,
    pub(crate) semantic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceAnchor {
    pub(crate) id: String,
    pub(crate) status: ReviewStatus,
    pub(crate) origin: PackOrigin,
    pub(crate) source: String,
    pub(crate) root: InterfaceRootSelector,
    pub(crate) container_path: Vec<InterfaceFactStep>,
    pub(crate) layout_version: Option<String>,
    pub(crate) pointer_width: Option<u8>,
    pub(crate) layout_size: Option<u32>,
    pub(crate) slot_stride: Option<u8>,
    pub(crate) guards: Vec<InterfaceGuard>,
    pub(crate) slots: Vec<InterfaceSlot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfacePack {
    pub(crate) id: String,
    pub(crate) calling_convention: String,
    pub(crate) anchors: Vec<InterfaceAnchor>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InterfaceWorkspaceSummary {
    pub(crate) fact_tables: usize,
    pub(crate) observed_slots: usize,
    pub(crate) reviewed_anchors: usize,
    pub(crate) ignored_anchors: usize,
    pub(crate) unreviewed_anchors: usize,
    pub(crate) manual_anchors: usize,
    pub(crate) reviewed_slots: usize,
    pub(crate) ignored_slots: usize,
    pub(crate) unreviewed_slots: usize,
    pub(crate) manual_slots: usize,
    pub(crate) semantic_links: usize,
    pub(crate) semantic_operations: usize,
    pub(crate) artifact_guards: usize,
    pub(crate) runtime_guards: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedInterfaceSlot {
    pub(crate) anchor: String,
    pub(crate) source: String,
    pub(crate) layout_version: String,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) name: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) return_type: String,
    pub(crate) variadic: bool,
    pub(crate) semantic: Option<String>,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) struct InterfaceWorkspace {
    summary: InterfaceWorkspaceSummary,
    bindings: Vec<ResolvedInterfaceSlot>,
}

impl InterfaceWorkspace {
    pub(crate) fn load(
        facts_path: &Path,
        pack_path: &Path,
        semantic_paths: &[impl AsRef<Path>],
        calling_convention: &str,
    ) -> Result<Self> {
        let facts = InterfaceFacts::load(facts_path)?;
        let catalogs = SemanticCatalogs::load(semantic_paths)?;
        let pack = InterfacePack::load(pack_path)?;
        let (summary, bindings) = pack.validate(&facts, &catalogs, calling_convention)?;
        Ok(Self { summary, bindings })
    }

    pub(crate) const fn summary(&self) -> InterfaceWorkspaceSummary {
        self.summary
    }

    pub(crate) fn bindings(&self) -> &[ResolvedInterfaceSlot] {
        &self.bindings
    }
}

impl InterfacePack {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<DocumentMut>()?;
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(format!("{} requires schema = 1", path.display()).into());
        }
        super::pack_parse::parse(&document)
    }
}
