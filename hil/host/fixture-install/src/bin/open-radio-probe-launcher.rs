#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_hil_fixture_install::launcher::main(oer_hil_fixture_install::launcher::LaunchTarget::Probe)
}
