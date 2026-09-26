//! HIL host process entry point.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod cli;
mod command;
mod execution;
mod fixture;
mod scenario;
#[cfg(test)]
mod tests;

pub(crate) use hil_core::{Result, emit_json, repository_root};

/// This executable's host build record, embedded by its build script.
const RUNNER_BUILD: &str = include_str!(concat!(env!("OUT_DIR"), "/runner-build.json"));

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--observer-build")
    {
        println!("{RUNNER_BUILD}");
        return;
    }
    let registered =
        hil_core::evidence::run::register_runner(hil_core::evidence::run::RunnerBuild {
            record: RUNNER_BUILD,
            package: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        });
    if let Err(error) = registered.and_then(|()| command::run()) {
        eprintln!("error: {error}");
        std::process::exit(if oer_process::is_cancelled(&*error) {
            130
        } else {
            1
        });
    }
}
