//! Explicit completion conditions for bounded concrete replay.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ExecutionGoal {
    #[default]
    Return,
    ReachSymbol {
        symbol: String,
    },
    ObserveCall {
        symbol: String,
    },
    ObserveFifoDequeue {
        service_id: String,
        #[serde(default)]
        value: Option<u32>,
    },
}
