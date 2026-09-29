//! Identities shared by execution contracts and captured-object selections.
use crate::*;

/// Caller-assigned stable semantic key, scoped by ProjectId. Never a physical selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SubjectId(String);
impl TryFrom<String> for SubjectId {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "subject key must contain 1..256 bytes without control characters",
            ));
        }
        Ok(Self(value))
    }
}
impl From<SubjectId> for String {
    fn from(value: SubjectId) -> Self {
        value.0
    }
}
impl SubjectId {
    pub fn allocated_bytes(&self) -> u64 {
        self.0.capacity() as u64
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Occurrence {
    pub revision: RevisionId,
    pub source: FunctionSource,
    pub object: ObjectId,
    pub symbol: Option<SymbolId>,
}
