//! The shared queue and lease holder.

use serde::{Deserialize, Serialize};

use crate::{budget::BudgetSource, process::ProcessIdentity};

pub(crate) const STATE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    pub(crate) schema: u32,
    pub(crate) next_id: u64,
    pub(crate) queue: Vec<Ticket>,
    pub(crate) holder: Option<Holder>,
    /// The previous grant went to a short request ahead of the head, so the
    /// head is served next.
    pub(crate) head_next: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            next_id: 1,
            queue: Vec::new(),
            holder: None,
            head_next: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ticket {
    pub(crate) id: u64,
    pub(crate) owner: String,
    pub(crate) work: String,
    pub(crate) budget_secs: u64,
    pub(crate) budget_source: BudgetSource,
    pub(crate) short: bool,
    pub(crate) process: ProcessIdentity,
    pub(crate) enqueued_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Holder {
    pub(crate) ticket: Ticket,
    /// Exported to commands inside the lease, which then join it.
    pub(crate) token: String,
    pub(crate) granted_unix: u64,
    pub(crate) over_budget: bool,
}
