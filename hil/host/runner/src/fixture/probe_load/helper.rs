//! Privileged, finite probe generator. It never reconfigures the managed peer.
#[cfg(any(target_os = "linux", test))]
mod frame;
#[cfg(target_os = "linux")]
mod linux;
mod model;

fn main() {
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
