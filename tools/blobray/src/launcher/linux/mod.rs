use super::{Backend, Config, Limits};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod procfs;
mod session;
mod systemd;
#[cfg(test)]
mod tests;

struct Signals {
    received: Arc<AtomicUsize>,
    registrations: Vec<signal_hook::SigId>,
}

impl Signals {
    fn install() -> Result<Self, String> {
        let mut signals = Self {
            received: Arc::new(AtomicUsize::new(0)),
            registrations: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM] {
            signals.registrations.push(
                signal_hook::flag::register_usize(
                    signal,
                    Arc::clone(&signals.received),
                    signal as usize,
                )
                .map_err(|error| format!("cannot install cancellation handler: {error}"))?,
            );
        }
        Ok(signals)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.registrations.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

fn cancellation(received: &AtomicUsize) -> Option<&'static str> {
    match received.load(Ordering::Relaxed) as i32 {
        libc::SIGINT => Some("received INT"),
        libc::SIGTERM => Some("received TERM"),
        _ => None,
    }
}

pub(super) fn run(config: Config) -> Result<u8, String> {
    let signals = Signals::install()?;
    let backend = if config.report_usage && config.backend == Backend::Auto {
        Backend::Watchdog
    } else {
        config.backend
    };
    if backend != Backend::Watchdog && systemd::available(&signals.received)? {
        return systemd::run(&config, &signals.received);
    }
    if cancellation(&signals.received).is_some() {
        return Ok(137);
    }
    if backend == Backend::Systemd {
        return Err("requested user systemd resource scope is unavailable".into());
    }
    let mut command = Command::new(&config.binary);
    command.args(&config.args);
    session::run(
        command,
        Limits::default(),
        config.report_usage,
        &signals.received,
        &mut procfs::Procfs::default(),
    )
}
