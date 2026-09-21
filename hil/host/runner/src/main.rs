//! HIL host process entry point.

use std::error::Error;

mod archive;
mod campaign;
mod cli;
mod command;
mod device;
mod error;
mod evidence;
mod execution;
mod fixture;
mod image;
mod lab;
mod output;
mod reporting;
mod scenario;
mod session;
mod transport;
mod workload;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
pub(crate) use command::repository_root;
pub(crate) use output::emit_json;

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--observer-build")
    {
        println!(
            "{}",
            include_str!(concat!(env!("OUT_DIR"), "/runner-build.json"))
        );
        return;
    }
    if let Err(error) = command::run() {
        eprintln!("error: {error}");
        std::process::exit(if oer_process::is_cancelled(&*error) {
            130
        } else {
            1
        });
    }
}
