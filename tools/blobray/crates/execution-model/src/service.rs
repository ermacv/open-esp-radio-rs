//! Scenario-owned stateful external services.
//!
//! These models deliberately describe mechanism-neutral FIFO behavior. A
//! knowledge provider binds concrete ABI functions such as an RTOS queue API to
//! these operations; the executor never learns RTOS or vendor vocabulary.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FifoServiceInstance {
    pub id: String,
    pub handle: u32,
    pub item_width: u8,
    pub capacity: usize,
    #[serde(default)]
    pub items: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ServiceValueSource {
    Argument { argument: u8, width: u8 },
    PrivateStackPointer { pointer_argument: u8, width: u8 },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ServiceOutput {
    PrivateStackPointer { pointer_argument: u8, width: u8 },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum FifoServiceOperation {
    Enqueue {
        item: ServiceValueSource,
        success_return: u32,
        full_return: u32,
        wake_output: Option<ServiceOutput>,
    },
    Dequeue {
        output: ServiceOutput,
        success_return: u32,
        empty_return: u32,
    },
    Len,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FifoServiceBinding {
    pub symbol: String,
    pub service_id: String,
    pub handle_argument: u8,
    pub operation: FifoServiceOperation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FifoLifecycleEvent {
    Enqueued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
        woke_receiver: bool,
    },
    Dequeued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
    },
    Full {
        service_id: String,
        site: u32,
        value: u32,
        depth: usize,
    },
    Empty {
        service_id: String,
        site: u32,
    },
    Length {
        service_id: String,
        site: u32,
        depth: usize,
    },
}
