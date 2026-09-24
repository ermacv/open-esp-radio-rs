//! Static observable relations over saved semantic IR, distinct from machine execution.
use crate::*;
pub const STATIC_TRACE_POLICY: u32 = 2;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceTarget {
    pub ir: ArtifactId,
    pub profile: String,
    pub entry: FunctionAnalysisId,
    /// Explicit integer ABI assumption, including ordinary call/return behavior.
    pub abi: CallAbi,
    /// Unspecified nonzero registers are symbolic entry inputs, not zero initialized.
    pub registers: Vec<TraceRegister>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRegister {
    pub register: u8,
    pub value: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceObservation {
    /// Sorted, disjoint physical intervals; an access crossing a boundary blocks exactness.
    pub ranges: Vec<ImageRegion>,
    pub fences: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRequest {
    pub left: TraceTarget,
    pub right: Option<TraceTarget>,
    pub observation: TraceObservation,
}
impl TraceTarget {
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error::new(ErrorCode::InvalidRequest, "invalid static trace target");
        if self.profile.is_empty() || self.profile.len() > 128 || self.registers.len() > 31 {
            return Err(invalid());
        }
        let mut seen = 1u32;
        for input in &self.registers {
            if input.register > 31 || seen & (1 << input.register) != 0 {
                return Err(invalid());
            }
            seen |= 1 << input.register;
        }
        Ok(())
    }
}
impl TraceObservation {
    pub fn validate(&self) -> Result<()> {
        let invalid = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid static trace observation scope",
            )
        };
        if self.ranges.len() > 256 || self.ranges.is_empty() && !self.fences {
            return Err(invalid());
        }
        let mut end = 0u64;
        for range in &self.ranges {
            if range.length == 0 || u64::from(range.start) < end {
                return Err(invalid());
            }
            end = u64::from(range.start)
                .checked_add(range.length)
                .ok_or_else(invalid)?;
            if end > 1u64 << 32 {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
impl TraceRequest {
    pub fn validate(&self) -> Result<()> {
        self.observation.validate()?;
        self.left.validate()?;
        if let Some(right) = &self.right {
            right.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TraceValue {
    Constant { value: u32 },
    Expression { id: u32 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TraceExpression {
    EntryRegister {
        register: u8,
    },
    Read {
        event: u64,
        width: u8,
        signed: bool,
    },
    Integer {
        op: IntegerOp,
        left: TraceValue,
        right: TraceValue,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceSide {
    Left,
    Right,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSite {
    pub invocation: u64,
    pub analysis: FunctionAnalysisId,
    pub record: u64,
    pub offset: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TraceEvent {
    Memory {
        access: MemoryKind,
        address: u32,
        width: u8,
        value: Option<TraceValue>,
    },
    Fence {
        predecessor: u8,
        successor: u8,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceBlocker {
    ComposedInterpretation,
    PartialFunction,
    MissingInstruction,
    UnknownCondition,
    Loop,
    RecursiveCall,
    UnresolvedCall,
    OutsideProfile,
    MissingCallInputs,
    UnknownAddress,
    CrossingRange,
    UnknownValue,
    UnsupportedEffect,
    UnsupportedFence,
    UnexpandedTransfer,
    NoReturn,
    UnresolvedValueEquality,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TraceRecord {
    Expression {
        id: u32,
        expression: TraceExpression,
    },
    Invocation {
        side: TraceSide,
        invocation: u64,
        parent: Option<TraceSite>,
        analysis: FunctionAnalysisId,
    },
    Event {
        side: TraceSide,
        index: u64,
        site: TraceSite,
        event: TraceEvent,
    },
    Blocked {
        side: TraceSide,
        site: Option<TraceSite>,
        reason: TraceBlocker,
    },
    Branch {
        side: TraceSide,
        site: TraceSite,
        taken: bool,
    },
    Return {
        side: TraceSide,
        site: TraceSite,
        low: Option<TraceValue>,
        high: Option<TraceValue>,
    },
    Difference {
        index: u64,
        left: Option<TraceEvent>,
        right: Option<TraceEvent>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceOutcome {
    pub exact: bool,
    pub events: u64,
    pub invocations: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSummary {
    pub schema: u32,
    pub policy: u32,
    pub request: TraceRequest,
    pub left: TraceOutcome,
    pub right: Option<TraceOutcome>,
    pub verdict: Option<ComparisonVerdict>,
}

impl TraceSummary {
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self.schema != 1
            || self.policy != STATIC_TRACE_POLICY
            || self.request.right.is_some() != self.right.is_some()
            || self.right.is_some() != self.verdict.is_some()
            || self.right.is_some_and(|r| !r.exact || !self.left.exact)
                && self.verdict != Some(ComparisonVerdict::Incomplete)
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "inconsistent static trace summary",
            ));
        }
        Ok(())
    }
}
