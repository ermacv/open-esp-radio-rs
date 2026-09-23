//! Shared data-only interface observations used by queries and frontends.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InterfaceFactRoot {
    RelocatedSymbol {
        reference: open_radio_vendor_contracts::SymbolReference,
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        owner: crate::artifact::CodeIdentity,
        argument: u8,
    },
    AbsoluteAddress {
        data_address: open_radio_vendor_contracts::DataAddressResolution,
        address: u32,
    },
}

impl InterfaceFactRoot {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RelocatedSymbol { .. } => "relocated-symbol",
            Self::FunctionArgument { .. } => "function-argument",
            Self::AbsoluteAddress { .. } => "absolute-address",
        }
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct InterfaceFactSelector {
    pub argument: u8,
    pub scale: u32,
    pub addend: i32,
}

impl InterfaceFactSelector {
    pub fn canonical(self) -> String {
        format!("arg{}*{}{:+#x}", self.argument, self.scale, self.addend)
    }

    pub fn index_for_offset(self, offset: i32) -> Option<u32> {
        let delta = i64::from(offset) - i64::from(self.addend);
        (delta >= 0 && self.scale != 0 && delta % i64::from(self.scale) == 0)
            .then(|| u32::try_from(delta / i64::from(self.scale)).ok())
            .flatten()
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct InterfaceFactStep {
    pub site: Option<u32>,
    pub offset: i32,
    pub width: u8,
    pub selector: Option<InterfaceFactSelector>,
}

/// Match table layout independently of observed instruction provenance.
pub(crate) fn same_step_shape(left: &[InterfaceFactStep], right: &[InterfaceFactStep]) -> bool {
    left.iter()
        .map(|s| (s.offset, s.width, s.selector))
        .eq(right.iter().map(|s| (s.offset, s.width, s.selector)))
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceFactSlot {
    pub offset: i32,
    pub width: u8,
    pub selector: Option<InterfaceFactSelector>,
    pub functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceTableFact {
    pub artifact: usize,
    pub root: InterfaceFactRoot,
    pub container_path: Vec<InterfaceFactStep>,
    pub slots: Vec<InterfaceFactSlot>,
    pub functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceArgumentFact {
    pub value: crate::interface_discovery::InterfaceArgumentValue,
    pub index: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceCallFact {
    pub owner: crate::artifact::CodeIdentity,
    pub link_register: u8,
    pub target_offset: i32,
    pub artifact: usize,
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub slot_load_site: Option<u32>,
    pub kind: String,
    pub root: InterfaceFactRoot,
    pub loads: Vec<InterfaceFactStep>,
    pub container_depth: usize,
    pub slot_offset: Option<i32>,
    pub jalr_offset: i32,
    pub arguments: Vec<InterfaceArgumentFact>,
    pub root_linkage: InterfaceRootLinkageFact,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceAssignmentFact {
    pub owner: crate::artifact::CodeIdentity,
    pub target_loads: Vec<InterfaceFactStep>,
    pub target_offset: i32,
    pub artifact: usize,
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub root: InterfaceFactRoot,
    pub container_path: Vec<InterfaceFactStep>,
    pub offset: i32,
    pub width: u8,
    pub target: InterfaceFactRoot,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceRootLinkageFact {
    pub symbols: Vec<String>,
    pub resolutions: Vec<String>,
    pub candidates: Vec<InterfaceSymbolLocationFact>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSymbolLocationFact {
    pub location: crate::SymbolLocation,
    pub artifact: usize,
    pub member: Option<String>,
    pub address: u32,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceFactArtifact {
    pub index: usize,
    pub sources: BTreeSet<String>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
/// Data-only interface query: observations, source references and explicit gaps.
///
/// These records carry evidence; they do not assert executable semantics or
/// complete table layouts. Reviewed associations are held separately.
pub struct InterfaceFacts {
    pub decode_blockers: Vec<InterfaceDecodeBlockerFact>,
    pub analysis_failures: Vec<InterfaceDecodeFailureFact>,
    pub limits: crate::interface_discovery::InterfaceDiscoveryLimits,
    pub gaps: Vec<InterfaceGapFact>,
    pub artifacts: Vec<InterfaceFactArtifact>,
    pub tables: Vec<InterfaceTableFact>,
    pub calls: Vec<InterfaceCallFact>,
    pub assignments: Vec<InterfaceAssignmentFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDecodeFailureFact {
    pub owner: crate::artifact::CodeIdentity,
    pub artifact: usize,
    pub member: Option<String>,
    pub function: String,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDecodeBlockerFact {
    pub owner: crate::artifact::CodeIdentity,
    pub artifact: usize,
    pub member: Option<String>,
    pub function: String,
    pub address: u32,
    pub width: u8,
    pub raw: u32,
    pub class: String,
    pub linear_control_flow: bool,
}

/// A source-scoped discovery gap with retained instruction and register evidence.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceGapFact {
    pub artifact: usize,
    pub evidence: crate::interface_discovery::InterfaceAnalysisGap,
}
