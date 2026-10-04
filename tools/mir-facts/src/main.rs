//! `oer-mir-facts`: a `RUSTC_WRAPPER` that compiles every crate unchanged and,
//! for each crate built for the firmware target, records the indirect-call
//! facts its MIR states into `$OER_MIR_FACTS_DIR` (see [`facts`]).
//!
//! The wrapper runs the toolchain's own compiler in process, with a callback
//! that only reads the analysed crate, so the build's outputs are those of
//! the plain compiler. Host crates (build scripts, procedural macros) and
//! queries such as `--print` or `-vV` go to the real `rustc` untouched.
#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
#[macro_use]
extern crate rustc_public;

mod collect;
mod facts;

use std::path::PathBuf;
use std::process::Command;

/// Where the facts of each crate are written; unset, the wrapper only
/// compiles.
const FACTS_DIR_ENV: &str = "OER_MIR_FACTS_DIR";
/// The target whose crates carry facts: the firmware's.
const TARGET_ENV: &str = "OER_MIR_FACTS_TARGET";

fn main() {
    // `RUSTC_WRAPPER` passes the real compiler first.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("oer-mir-facts: run as RUSTC_WRAPPER, with the compiler as first argument");
        std::process::exit(2);
    }
    let rustc = args[0].clone();
    let target = std::env::var(TARGET_ENV).ok();
    let directory = std::env::var_os(FACTS_DIR_ENV).map(PathBuf::from);
    let for_target = target.as_deref().is_some_and(|target| {
        args.windows(2)
            .any(|pair| pair[0] == "--target" && pair[1] == target)
            || args.iter().any(|arg| arg == &format!("--target={target}"))
    });
    let compiles = args.iter().any(|arg| arg.ends_with(".rs"));
    let (Some(directory), true, true) = (directory, for_target, compiles) else {
        let status = Command::new(&rustc)
            .args(&args[1..])
            .status()
            .unwrap_or_else(|error| {
                eprintln!("oer-mir-facts: cannot run {rustc}: {error}");
                std::process::exit(2)
            });
        std::process::exit(status.code().unwrap_or(1));
    };
    // The in-process compiler takes its own name first, as `rustc` would.
    args[0] = "rustc".to_owned();
    let result = run_with_tcx!(&args, |tcx| {
        let facts = collect::crate_facts(tcx);
        if let Err(error) = facts::write(&directory, &facts) {
            eprintln!("oer-mir-facts: {error}");
            std::process::exit(1);
        }
        std::ops::ControlFlow::<(), ()>::Continue(())
    });
    match result {
        Ok(()) | Err(rustc_public::CompilerError::Skipped) => {}
        Err(rustc_public::CompilerError::Interrupted(())) => {}
        Err(rustc_public::CompilerError::Failed) => std::process::exit(1),
    }
}
