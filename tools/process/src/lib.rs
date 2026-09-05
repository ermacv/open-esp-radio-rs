//! Direct subprocess arguments and owned command lifetimes, without a shell.

pub mod owned;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod cancellation;
pub use cancellation::{Cancelled, check_cancelled, cleanup, is_cancelled, sleep};
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
pub fn cancellation_requested() -> bool {
    cancellation().load(Ordering::Relaxed) && !cancellation::in_cleanup()
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
    Err("host process-group cancellation is unsupported on this host".into())
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
        return Err(format!(
            "{} failed with {status}",
            command.get_program().to_string_lossy()
        )
        .into());
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
    let output = output(command, None)?;
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}:\n{}",
            command.get_program().to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

/// Capture a command without interpreting its exit status or printing arguments.
pub fn output(command: &mut Command, timeout: Option<std::time::Duration>) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    owned::Child::spawn(command)?.wait_with_output_timeout(timeout)
}

pub(crate) fn collect_output(
    mut child: owned::Child,
    timeout: Option<std::time::Duration>,
) -> Result<Output> {
    drop(child.stdin.take());
    let stdout = child.take_stdout().map(drain);
    let stderr = child.take_stderr().map(drain);
    let status = child.wait_timeout(timeout);
    let stdout = stdout.map(collect).transpose();
    let stderr = stderr.map(collect).transpose();
    let status = status?;
    let stdout = stdout?.unwrap_or_default();
    let stderr = stderr?.unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Bounded, cancellable equivalents of Command's blocking operations.
/// Long builds and captures should use an explicit workload-derived timeout.
pub trait CommandExt {
    fn supervised_output(&mut self) -> Result<Output>;
    fn supervised_status(&mut self) -> Result<std::process::ExitStatus>;
    fn spawn_owned(&mut self) -> Result<owned::Child>;
}

impl CommandExt for Command {
    fn supervised_output(&mut self) -> Result<Output> {
        output(self, Some(std::time::Duration::from_secs(120)))
    }
    fn supervised_status(&mut self) -> Result<std::process::ExitStatus> {
        owned::Child::spawn(self)?.wait_timeout(Some(std::time::Duration::from_secs(120)))
    }
    fn spawn_owned(&mut self) -> Result<owned::Child> {
        owned::Child::spawn(self)
    }
}
