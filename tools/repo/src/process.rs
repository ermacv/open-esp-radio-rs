//! Direct subprocess arguments and owned command lifetimes, without a shell.

pub mod owned;

use crate::Result;
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

fn cancellation() -> &'static Arc<AtomicBool> {
    static CANCELLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    CANCELLED.get_or_init(|| Arc::new(AtomicBool::new(false)))
}
pub(crate) fn cancellation_requested() -> bool {
    cancellation().load(Ordering::Relaxed)
}

pub struct SignalGuard {
    #[cfg(unix)]
    handlers: Vec<signal_hook::SigId>,
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}
pub fn install_signal_handlers() -> Result<SignalGuard> {
    #[cfg(unix)]
    {
        let mut guard = SignalGuard {
            handlers: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            guard.handlers.push(signal_hook::flag::register(
                signal,
                Arc::clone(cancellation()),
            )?);
        }
        Ok(guard)
    }
    #[cfg(not(unix))]
    Err("xtask process-group cancellation is unsupported on this host".into())
}

pub fn run(command: &mut Command) -> Result<()> {
    run_with_shutdown_grace(command, std::time::Duration::from_secs(1))
}

/// Give a nested supervisor time to clean up the sessions or services it owns.
/// This bounds shutdown only; it does not limit command runtime or resources.
pub fn run_with_shutdown_grace(
    command: &mut Command,
    shutdown_grace: std::time::Duration,
) -> Result<()> {
    let mut child = owned::Child::spawn_with_shutdown_grace(command, shutdown_grace)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("{command:?} failed with {status}").into());
    }
    Ok(())
}
fn drain(
    mut input: impl Read + Send + 'static,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}
fn collect(handle: std::thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    Ok(handle
        .join()
        .map_err(|_| "subprocess output reader panicked")??)
}
/// Capture noninteractive probes: close stdin and drain both output streams.
pub fn capture(command: &mut Command) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = owned::Child::spawn(command)?;
    let stdout = drain(
        child
            .take_stdout()
            .ok_or("subprocess stdout pipe missing")?,
    );
    let stderr = drain(
        child
            .take_stderr()
            .ok_or("subprocess stderr pipe missing")?,
    );
    let status = child.wait();
    let stdout = collect(stdout);
    let stderr = collect(stderr);
    let stdout = stdout?;
    let stderr = stderr?;
    let status = status?;
    if !status.success() {
        return Err(format!(
            "{command:?} failed with {status}:\n{}",
            String::from_utf8_lossy(&stderr)
        )
        .into());
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
