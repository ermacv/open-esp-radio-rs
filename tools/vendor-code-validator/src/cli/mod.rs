//! Command parsing and dispatch for the validator binary.

mod args;
mod commands;
mod json;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

use crate::*;
use args::{Command, Invocation};

pub(crate) fn usage() {
    eprintln!(
        "usage: vendor-code-validator GROUP COMMAND --target-spec PATH [--run-spec PATH] [OPTIONS]\n\nworkflows:\n  inspect   analyze | trace | compare\n  reference generate | generate-batch\n  execute   run | compare\n  verify    profiles | source | inventory | contract channel | contract rf-init\n  image     audit-targets\n\nThe target spec supplies architecture, calling convention, SVDs and checked harness data.\nA caller-owned run spec may bind input roles to local artifact paths. Legacy flat command names are temporarily accepted."
    );
}

pub(crate) fn run() -> Result<bool> {
    let Invocation {
        command,
        target_spec,
        run_spec,
        mut svd_paths,
        arguments: mut filtered,
    } = Invocation::parse(env::args().skip(1))?;
    let target = TargetSpec::load(&target_spec)?;
    target.require_available_backend()?;
    target.require_available_harness()?;
    eprintln!(
        "TARGET\tid={}\tharness={}\tarchitecture={}\tcalling-convention={}\tendianness={}\tpointer-width={}\trust-target={}",
        target.id,
        target.harness,
        target.architecture.label(),
        target.calling_convention.label(),
        target.endianness.label(),
        target.pointer_width,
        target.rust_target,
    );
    if let Some(path) = run_spec {
        RunSpec::load(&path)?.append_defaults(&mut filtered);
    }
    if svd_paths.is_empty() {
        svd_paths = target.svd_paths.clone();
    }
    if svd_paths.is_empty() && command != Command::AuditDirectTargets {
        return Err("target spec has no SVD and command has no --svd override".into());
    }
    if command == Command::VerifyAll {
        append_default_path(&mut filtered, "--profiles", target.profiles.as_deref());
        append_default_path(
            &mut filtered,
            "--dispositions",
            target.dispositions.as_deref(),
        );
        append_default_path(
            &mut filtered,
            "--evidence-baseline",
            target.evidence_baseline.as_deref(),
        );
    }
    let svd = MmioRegisterMap::load_all(&svd_paths)?;
    commands::run(command, filtered, &svd, &target)
}

fn append_default_path(arguments: &mut Vec<String>, option: &str, path: Option<&std::path::Path>) {
    if arguments.iter().any(|argument| argument == option) {
        return;
    }
    if let Some(path) = path {
        arguments.push(option.to_owned());
        arguments.push(path.display().to_string());
    }
}
