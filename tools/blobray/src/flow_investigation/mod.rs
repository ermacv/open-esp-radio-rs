//! Bounded, project-aware inter-function evidence investigation.
//!
//! The report is navigation evidence. Structural reachability, reviewed event
//! routes and executable equivalence remain distinct claims.

mod cfg;
mod effects;
mod event;
mod model;
mod operation_memory;
mod project_graph;
mod target;
mod value;

pub(crate) use model::*;

use crate::{ProjectSpec, Result};

pub(crate) fn investigate(
    request: FlowInvestigationRequest<'_>,
    project: &ProjectSpec,
) -> Result<FlowInvestigationReport> {
    match request {
        FlowInvestigationRequest::Target(request) => target::investigate(request, project),
        FlowInvestigationRequest::Effects(request) => effects::investigate(request, project),
        FlowInvestigationRequest::Publication(request) => {
            operation_memory::investigate(request, project)
        }
        FlowInvestigationRequest::EventRoute(request) => event::investigate(request, project),
    }
}

pub(crate) fn investigate_event_routes_with_workspace(
    route_ids: &[String],
    max_depth: usize,
    project: &ProjectSpec,
    workspace: &crate::function_workspace::FunctionWorkspace,
) -> Result<Vec<FlowInvestigationReport>> {
    event::investigate_many_with_workspace(route_ids, max_depth, project, workspace)
}
