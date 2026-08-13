use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewScopesDocument {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) project: String,
    pub(crate) scopes: Vec<ReviewScopeReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewScopeMmio {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) linked_ir: bool,
    pub(crate) static_discovery: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewScopeEffect {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) operation: String,
    pub(crate) target: String,
    pub(crate) width: Option<u8>,
    pub(crate) value: Option<String>,
    pub(crate) modified_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
    pub(crate) arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewScopeTransaction {
    pub(crate) id: String,
    pub(crate) identity: String,
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) fingerprint: String,
    pub(crate) paths: Vec<Vec<String>>,
    pub(crate) effects: Vec<ReviewScopeEffect>,
}

/// A root-cause-grouped item in the scope-driven human review queue.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewQueueItem {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) priority: u8,
    pub(crate) severity: String,
    pub(crate) occurrences: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) sites: Vec<u32>,
    pub(crate) channels: Vec<String>,
    pub(crate) message: String,
}

/// Qualification of the explicit Rust replacement boundary for this scope.
///
/// Reachable vendor helpers and their blockers remain analysis inventory;
/// they do not require invented one-to-one Rust component identities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReplacementQualification {
    #[default]
    NotPublished,
    Qualified,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewScopeReport {
    pub(crate) id: String,
    pub(crate) publication: bool,
    /// Current verification assurance joined when the structural document is
    /// loaded.  It is deliberately absent from `review-scopes.json`: project
    /// analysis runs before verification and must not persist a stale proof
    /// snapshot.
    #[serde(skip)]
    pub(crate) replacement_qualification: ReplacementQualification,
    pub(crate) analysis_inventory_complete: bool,
    pub(crate) profiles: Vec<String>,
    pub(crate) roots: usize,
    pub(crate) functions: usize,
    /// Distinct explicit roots that require reviewed Rust coverage.
    pub(crate) replacement_functions: usize,
    /// Exact explicit scope roots requiring either verification evidence or a
    /// reviewed feature-policy disposition.
    pub(crate) replacement_function_keys: Vec<String>,
    /// Every reachable function with a direct observable effect. This is the
    /// fail-closed feature denominator and is intentionally independent from
    /// the explicit Rust replacement roots above.
    pub(crate) transaction_functions: usize,
    pub(crate) transaction_keys: Vec<String>,
    pub(crate) transactions: Vec<ReviewScopeTransaction>,
    pub(crate) function_identities: Vec<String>,
    pub(crate) function_keys: Vec<String>,
    pub(crate) complete_functions: usize,
    pub(crate) mmio_registers: usize,
    pub(crate) linked_mmio_registers: usize,
    pub(crate) static_mmio_registers: usize,
    pub(crate) mmio: Vec<ReviewScopeMmio>,
    pub(crate) table_calls: usize,
    pub(crate) context_fields: usize,
    pub(crate) memory_fields: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) decode_blocker_functions: usize,
    pub(crate) direct_blockers: usize,
    pub(crate) call_graph_blockers: usize,
    pub(crate) reference_blockers: usize,
    pub(crate) unresolved_calls: usize,
    #[serde(skip)]
    pub(crate) replacement_behavioral_matches: usize,
    #[serde(skip)]
    pub(crate) replacement_production_matches: usize,
    #[serde(skip)]
    pub(crate) replacement_bounded_matches: usize,
    #[serde(skip)]
    pub(crate) replacement_probe_only_matches: usize,
    #[serde(skip)]
    pub(crate) replacement_unmapped_matches: usize,
    #[serde(skip)]
    pub(crate) replacement_mismatches: usize,
    #[serde(skip)]
    pub(crate) replacement_incomplete: usize,
    #[serde(skip)]
    pub(crate) replacement_unqualified: usize,
    #[serde(skip)]
    pub(crate) replacement_uncovered: usize,
    /// Ordered by actionable priority and then stable root-cause identity.
    pub(crate) review_queue: Vec<ReviewQueueItem>,
}

impl ReviewScopeReport {
    pub(crate) fn has_analysis_inventory_blockers(&self) -> bool {
        self.decode_blockers != 0
            || self.direct_blockers != 0
            || self.call_graph_blockers != 0
            || self.reference_blockers != 0
            || self.unresolved_calls != 0
    }

    pub(crate) fn has_replacement_qualification_blockers(&self) -> bool {
        self.replacement_mismatches != 0
            || self.replacement_incomplete != 0
            || self.replacement_unqualified != 0
            || self.replacement_uncovered != 0
            || self.replacement_probe_only_matches != 0
            || self.replacement_unmapped_matches != 0
    }
}

#[derive(Deserialize)]
pub(super) struct VerificationDocument {
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) replacement_graph: StoredReplacementGraph,
}

#[derive(Deserialize)]
pub(super) struct StoredReplacementGraph {
    pub(super) replacements: Vec<StoredReplacement>,
}

#[derive(Deserialize)]
pub(super) struct StoredReplacement {
    pub(super) vendor: StoredVendorFunction,
    pub(super) status: String,
    pub(super) rust: Option<StoredRustReplacement>,
}

#[derive(Deserialize)]
pub(super) struct StoredRustReplacement {
    pub(super) production_component: Option<String>,
    #[serde(default)]
    pub(super) verification_probes: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct StoredVendorFunction {
    pub(super) source: String,
    pub(super) symbol: String,
}
