fn main() -> std::process::ExitCode {
    open_esp_radio_hil_fixture_install::launcher::main(
        open_esp_radio_hil_fixture_install::launcher::LaunchTarget::Network,
    )
}
