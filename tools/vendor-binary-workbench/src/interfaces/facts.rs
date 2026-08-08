//! Loading immutable JSON emitted by `interfaces discover`.

use std::{collections::BTreeSet, fs, path::Path};

use crate::{Result, error::WorkbenchError};

mod parse;
mod validate;

pub(crate) use validate::validate_sha256;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum InterfaceFactRoot {
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

impl InterfaceFactRoot {
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::RelocatedSymbol { .. } => "relocated-symbol",
            Self::FunctionArgument { .. } => "function-argument",
            Self::AbsoluteAddress { .. } => "absolute-address",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceFactStep {
    pub(crate) offset: i32,
    pub(crate) width: u8,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceFactSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceTableFact {
    pub(crate) artifact: usize,
    pub(crate) root: InterfaceFactRoot,
    pub(crate) container_path: Vec<InterfaceFactStep>,
    pub(crate) slots: Vec<InterfaceFactSlot>,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceArgumentFact {
    pub(crate) index: usize,
    pub(crate) kind: String,
    pub(crate) expression: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct InterfaceCallFact {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) function_address: u32,
    pub(crate) site: u32,
    pub(crate) kind: String,
    pub(crate) root: InterfaceFactRoot,
    pub(crate) loads: Vec<InterfaceFactStep>,
    pub(crate) container_depth: usize,
    pub(crate) slot_offset: Option<i32>,
    pub(crate) jalr_offset: i32,
    pub(crate) arguments: Vec<InterfaceArgumentFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceFactArtifact {
    pub(crate) index: usize,
    pub(crate) sources: BTreeSet<String>,
    pub(crate) sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceFacts {
    pub(crate) artifacts: Vec<InterfaceFactArtifact>,
    pub(crate) tables: Vec<InterfaceTableFact>,
    pub(crate) calls: Vec<InterfaceCallFact>,
}

impl InterfaceFacts {
    #[tracing::instrument(name = "load_interface_facts", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        parse::parse(&input).map_err(|error| {
            WorkbenchError::manifest_document("interface discovery report", path, &input, error)
        })
    }

    pub(crate) fn artifact(&self, index: usize) -> Option<&InterfaceFactArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.index == index)
    }

    pub(crate) fn observed_slots(&self) -> usize {
        self.tables.iter().map(|table| table.slots.len()).sum()
    }

    pub(crate) const fn observed_calls(&self) -> usize {
        self.calls.len()
    }
}
