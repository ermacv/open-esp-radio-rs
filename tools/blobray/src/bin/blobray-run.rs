//! Resource-limited launcher, independent of the repository xtask.
#[path = "../launcher/mod.rs"]
mod launcher;

fn main() -> std::process::ExitCode {
    launcher::main()
}
