//! Project configuration and caller-owned input readiness diagnostics.

mod capabilities;
mod code;
mod inputs;
mod interfaces;
mod model;
mod registers;
mod render;
mod verification;

use super::{ProjectContext, Result};
use model::{DoctorReport, RunSpecReport};

pub(super) fn run(context: ProjectContext<'_>) -> Result<bool> {
    let ir_build =
        super::project_ir_doctor::inspect(context.project, context.run_spec, context.target);
    let ir_counts = ir_build.counts();
    let function_workspace = super::project_function_doctor::inspect(context.project);
    let function_counts = function_workspace.counts();
    let run_spec = match (context.run_spec_path, context.run_spec) {
        (Some(path), Some(_)) => RunSpecReport::configured(path.to_owned()),
        (None, None) => RunSpecReport::missing(),
        _ => unreachable!("run-spec path and parsed contents are created together"),
    };
    let mut report = DoctorReport::new(
        &context.project.id,
        context.project_path.to_owned(),
        &context.target.id,
        context.target_path.to_owned(),
        ir_build,
        function_workspace,
        run_spec,
    );
    report.absorb(ir_counts.0, ir_counts.1);
    report.absorb(function_counts.0, function_counts.1);

    capabilities::collect(&context, &mut report);
    code::collect(&context, &mut report);
    registers::collect(&context, &mut report);
    interfaces::collect(&context, &mut report);
    verification::collect(&context, &mut report);
    inputs::collect(&context, &mut report);

    render::render(&report, &context);
    Ok(report.succeeded())
}
