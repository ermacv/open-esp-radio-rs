//! Privileged, finite probe generator. It never reconfigures the managed peer.
#[cfg(any(target_os = "linux", test))]
use open_esp_radio_hil_fixture::probe::frame;
#[cfg(target_os = "linux")]
mod linux;
use open_esp_radio_hil_fixture::probe::model;

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var("OPEN_RADIO_GENERATION_BOUND").as_deref() != Ok("linux-net-probe") {
        use std::os::unix::process::CommandExt as _;
        let error = std::process::Command::new("/usr/local/libexec/open-radio-probe-launcher")
            .args(std::env::args_os().skip(1))
            .exec();
        eprintln!("probe launcher: {error}");
        std::process::exit(1);
    }
    #[cfg(target_os = "linux")]
    let _software =
        match open_esp_radio_hil_fixture_install::launcher::adopt_lease("linux-net-probe") {
            Ok(lease) => lease,
            Err(error) => {
                eprintln!("probe software lease: {error}");
                std::process::exit(1);
            }
        };
    #[cfg(target_os = "linux")]
    let result = linux::run();
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> =
        Err("probe injection requires Linux".into());
    if let Err(error) = result {
        eprintln!("probe helper: {error}");
        std::process::exit(1);
    }
}
