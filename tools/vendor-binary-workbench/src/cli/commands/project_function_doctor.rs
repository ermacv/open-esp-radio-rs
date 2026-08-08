//! Function/context workspace readiness diagnostics.

use crate::{
    function_workspace::{FunctionFacts, FunctionWorkspace},
    project::ProjectSpec,
};

pub(super) fn inspect(project: &ProjectSpec) -> (usize, usize) {
    let Some(paths) = &project.functions else {
        outputln!("CAPABILITY\tfunction-workspace\tnot-configured");
        return (0, 0);
    };
    let reports = match project.function_ir_reports() {
        Ok(reports) => reports,
        Err(error) => {
            outputln!("CAPABILITY\tfunction-workspace\tinvalid-config\terror={error}");
            return (1, 0);
        }
    };
    let missing = reports
        .iter()
        .filter(|(_, report)| !report.is_file())
        .count();
    if missing != 0 {
        outputln!(
            "CAPABILITY\tfunction-workspace\tnot-generated\tprofiles={}\tmissing={}\tpack={}\treview-output={}",
            reports.len(),
            missing,
            paths.pack.display(),
            display_optional(paths.review_output.as_deref())
        );
        return (0, 1);
    }
    let facts = match FunctionFacts::load(&reports) {
        Ok(facts) => facts,
        Err(error) => {
            outputln!(
                "CAPABILITY\tfunction-workspace\tinvalid-facts\tprofiles={}\terror={error}",
                reports.len()
            );
            return (1, 0);
        }
    };
    if !paths.pack.is_file() {
        let root_functions = facts.root_functions().count();
        let context_fields = facts
            .root_functions()
            .map(|function| function.context_fields.len())
            .sum::<usize>();
        outputln!(
            "CAPABILITY\tfunction-workspace\tpack-not-initialized\tprofiles={}\tfunctions={}\troot-functions={}\tcontext-fields={}\tpack={}\treview-output={}",
            reports.len(),
            facts.functions.len(),
            root_functions,
            context_fields,
            paths.pack.display(),
            display_optional(paths.review_output.as_deref())
        );
        return (0, 1);
    }
    match FunctionWorkspace::load(&reports, &paths.pack) {
        Ok(workspace) => {
            let summary = workspace.summary();
            outputln!(
                "CAPABILITY\tfunction-workspace\tavailable\tprofiles={}\troot-functions={}\treviewed-functions={}\tignored-functions={}\tunreviewed-functions={}\treviewed-contexts={}\tignored-contexts={}\tunreviewed-contexts={}\treviewed-fields={}\tignored-fields={}\tunreviewed-fields={}\taccepted-incomplete={}\tpack={}\treview-output={}",
                reports.len(),
                summary.observed_functions,
                summary.reviewed_functions,
                summary.ignored_functions,
                summary.unreviewed_functions,
                summary.reviewed_contexts,
                summary.ignored_contexts,
                summary.unreviewed_contexts,
                summary.reviewed_fields,
                summary.ignored_fields,
                summary.unreviewed_fields,
                summary.accepted_incomplete,
                paths.pack.display(),
                display_optional(paths.review_output.as_deref())
            );
            (0, 0)
        }
        Err(error) => {
            outputln!(
                "CAPABILITY\tfunction-workspace\tinvalid\tprofiles={}\tpack={}\terror={error}",
                reports.len(),
                paths.pack.display()
            );
            (1, 0)
        }
    }
}

fn display_optional(path: Option<&std::path::Path>) -> String {
    path.map_or_else(|| "-".to_owned(), |path| path.display().to_string())
}
