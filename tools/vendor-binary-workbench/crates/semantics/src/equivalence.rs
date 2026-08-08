//! Shared result vocabulary for physical and semantic equivalence checks.

use serde::Serialize;

/// The observation layer used by a comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EquivalenceMode {
    /// Exact machine-visible effects: addresses, widths, values, order, RAM, and return value.
    Physical,
    /// Reviewed meanings and explicitly allowed replacements of observable effects.
    Semantic,
}

impl EquivalenceMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Semantic => "semantic",
        }
    }
}

/// Closed outcome shared by every verification mode.
///
/// `Incomplete` is deliberately not an error: it means the comparison did not
/// have enough modeled evidence to make an equivalence claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EquivalenceVerdict {
    Match,
    Diff,
    Incomplete,
}

impl EquivalenceVerdict {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Diff => "DIFF",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

/// Presentation-neutral result of one comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EquivalenceOutcome {
    pub mode: EquivalenceMode,
    pub verdict: EquivalenceVerdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl EquivalenceOutcome {
    pub const fn matched(mode: EquivalenceMode) -> Self {
        Self {
            mode,
            verdict: EquivalenceVerdict::Match,
            reason: None,
        }
    }

    pub fn different(mode: EquivalenceMode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            verdict: EquivalenceVerdict::Diff,
            reason: Some(reason.into()),
        }
    }

    pub fn incomplete(mode: EquivalenceMode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            verdict: EquivalenceVerdict::Incomplete,
            reason: Some(reason.into()),
        }
    }
}
