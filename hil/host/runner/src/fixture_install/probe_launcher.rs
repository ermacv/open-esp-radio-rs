fn main() -> std::process::ExitCode {
    open_esp_radio_hil_runner::fixture_install::launcher::main(
        open_esp_radio_hil_runner::fixture_install::launcher::LaunchTarget::Probe,
    )
}
