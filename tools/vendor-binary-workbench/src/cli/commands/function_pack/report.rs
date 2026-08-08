//! Typed function-workspace lifecycle reports and presentation renderers.

use std::path::Path;

use serde::Serialize;

#[derive(Serialize)]
pub(super) struct FunctionPackDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) inputs: usize,
    pub(super) functions: usize,
    pub(super) root_functions: usize,
    pub(super) context_fields: usize,
    pub(super) path: &'a Path,
}

#[derive(Serialize)]
pub(super) struct FunctionWorkspaceDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) deny_unreviewed: bool,
    pub(super) inputs: usize,
    pub(super) observed_functions: usize,
    pub(super) reviewed_functions: usize,
    pub(super) ignored_functions: usize,
    pub(super) unreviewed_functions: usize,
    pub(super) reviewed_contexts: usize,
    pub(super) ignored_contexts: usize,
    pub(super) unreviewed_contexts: usize,
    pub(super) reviewed_fields: usize,
    pub(super) ignored_fields: usize,
    pub(super) unreviewed_fields: usize,
    pub(super) accepted_incomplete: usize,
    pub(super) pack: &'a Path,
}

#[derive(Serialize)]
pub(super) struct FunctionReviewDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) root_functions: usize,
    pub(super) reviewed: usize,
    pub(super) unreviewed: usize,
    pub(super) contexts: usize,
    pub(super) fields: usize,
    pub(super) interface_links: usize,
    pub(super) output: &'a Path,
}

pub(super) fn print_pack_human(report: &FunctionPackDocument<'_>) {
    outputln!(
        "Function pack: {} — {}",
        report.status,
        report.path.display()
    );
    outputln!(
        "  inputs={} functions={} roots={} context-fields={}",
        report.inputs,
        report.functions,
        report.root_functions,
        report.context_fields
    );
}

pub(super) fn print_pack_tsv(report: &FunctionPackDocument<'_>) {
    outputln!(
        "FUNCTION-PACK\tstatus={}\tinputs={}\tfunctions={}\troot-functions={}\tcontext-fields={}\tpath={}",
        report.status,
        report.inputs,
        report.functions,
        report.root_functions,
        report.context_fields,
        report.path.display()
    );
}

pub(super) fn print_workspace_human(report: &FunctionWorkspaceDocument<'_>) {
    outputln!(
        "Function workspace: {} — {}",
        report.status,
        report.pack.display()
    );
    outputln!(
        "  functions: observed={} reviewed={} ignored={} unreviewed={}",
        report.observed_functions,
        report.reviewed_functions,
        report.ignored_functions,
        report.unreviewed_functions
    );
    outputln!(
        "  contexts: reviewed={} ignored={} unreviewed={}; fields: reviewed={} ignored={} unreviewed={}",
        report.reviewed_contexts,
        report.ignored_contexts,
        report.unreviewed_contexts,
        report.reviewed_fields,
        report.ignored_fields,
        report.unreviewed_fields
    );
}

pub(super) fn print_workspace_tsv(report: &FunctionWorkspaceDocument<'_>) {
    outputln!(
        "FUNCTION-WORKSPACE\tstatus={}\tdeny-unreviewed={}\tinputs={}\tobserved-functions={}\treviewed-functions={}\tignored-functions={}\tunreviewed-functions={}\treviewed-contexts={}\tignored-contexts={}\tunreviewed-contexts={}\treviewed-fields={}\tignored-fields={}\tunreviewed-fields={}\taccepted-incomplete={}\tpack={}",
        report.status,
        report.deny_unreviewed,
        report.inputs,
        report.observed_functions,
        report.reviewed_functions,
        report.ignored_functions,
        report.unreviewed_functions,
        report.reviewed_contexts,
        report.ignored_contexts,
        report.unreviewed_contexts,
        report.reviewed_fields,
        report.ignored_fields,
        report.unreviewed_fields,
        report.accepted_incomplete,
        report.pack.display()
    );
}

pub(super) fn print_review_human(report: &FunctionReviewDocument<'_>) {
    outputln!(
        "Function review: {} — {}",
        report.status,
        report.output.display()
    );
    outputln!(
        "  roots={} reviewed={} unreviewed={} contexts={} fields={} interface-links={}",
        report.root_functions,
        report.reviewed,
        report.unreviewed,
        report.contexts,
        report.fields,
        report.interface_links
    );
}

pub(super) fn print_review_tsv(report: &FunctionReviewDocument<'_>) {
    outputln!(
        "FUNCTION-REVIEW\tstatus={}\troot-functions={}\treviewed={}\tunreviewed={}\tcontexts={}\tfields={}\tinterface-links={}\toutput={}",
        report.status,
        report.root_functions,
        report.reviewed,
        report.unreviewed,
        report.contexts,
        report.fields,
        report.interface_links,
        report.output.display()
    );
}
