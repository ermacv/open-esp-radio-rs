//! Project configuration and caller-owned input readiness diagnostics.

mod capabilities;
mod code;
mod inputs;
mod interfaces;
mod model;
mod registers;
mod render;
mod revision;
mod verification;

use super::{ProjectContext, Result};
use model::{DoctorReport, RunSpecReport};

pub(super) fn run(context: ProjectContext<'_>) -> Result<bool> {
    let total_started = std::time::Instant::now();
    let section_started = std::time::Instant::now();
    let ir_build =
        super::project_ir_doctor::inspect(context.project, context.run_spec, context.target);
    let ir_duration = section_started.elapsed();
    let section_started = std::time::Instant::now();
    let ir_counts = ir_build.counts();
    let function_workspace = super::project_function_doctor::inspect(context.project);
    let function_duration = section_started.elapsed();
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
    report.timing("ir-build", ir_duration);
    report.timing("function-workspace", function_duration);

    let section_started = std::time::Instant::now();
    capabilities::collect(&context, &mut report);
    report.timing("capabilities", section_started.elapsed());
    let section_started = std::time::Instant::now();
    code::collect(&context, &mut report);
    report.timing("code-boundaries", section_started.elapsed());
    let section_started = std::time::Instant::now();
    registers::collect(&context, &mut report);
    report.timing("registers", section_started.elapsed());
    let section_started = std::time::Instant::now();
    interfaces::collect(&context, &mut report);
    report.timing("interfaces", section_started.elapsed());
    let section_started = std::time::Instant::now();
    revision::collect(&context, &mut report);
    report.timing("revision-workflow", section_started.elapsed());
    let section_started = std::time::Instant::now();
    verification::collect(&context, &mut report);
    report.timing("verification", section_started.elapsed());
    let section_started = std::time::Instant::now();
    inputs::collect(&context, &mut report);
    report.timing("inputs", section_started.elapsed());
    report.duration_ms = total_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);

    render::render(&report, &context);
    Ok(report.succeeded())
}
