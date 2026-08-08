//! Reachable effect summaries, context projection, and event dispatch projection.

use super::*;

mod effect;
mod event_dispatch;
mod projection;

pub(super) use effect::populate_effect_summaries;
#[cfg(test)]
pub(super) use event_dispatch::project_event_dispatches;

#[derive(Clone)]
pub(super) struct SummaryCallEdge {
    pub(super) target: usize,
    pub(super) site: Option<u32>,
    pub(super) bindings: Vec<LinkedArgumentBinding>,
    pub(super) guard_paths: Option<Vec<LinkedCallGuardPath>>,
}
