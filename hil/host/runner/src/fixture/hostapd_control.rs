//! Event-driven readiness on hostapd's control socket.

use crate::Result;
use mio::{Events, Interest, Poll, Token, Waker, unix::SourceFd};
use std::{
    os::{fd::AsRawFd, unix::net::UnixDatagram},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct Control {
    socket: UnixDatagram,
    poll: Poll,
    deadline: Instant,
    pending_event: bool,
    _notification: oer_process::CancellationNotification,
    _directory: tempfile::TempDir,
}

impl Control {
    pub(crate) fn connect(path: &Path) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let socket = UnixDatagram::bind(directory.path().join("control"))?;
        socket.connect(path)?;
        socket.set_nonblocking(true)?;
        let poll = Poll::new()?;
        poll.registry().register(
            &mut SourceFd(&socket.as_raw_fd()),
            Token(0),
            Interest::READABLE,
        )?;
        let wake = Arc::new(Waker::new(poll.registry(), Token(1))?);
        let notification = oer_process::notify_on_cancel(move || {
            let _ = wake.wake();
        });
        Ok(Self {
            socket,
            poll,
            deadline: Instant::now() + Duration::from_secs(20),
            pending_event: false,
            _notification: notification,
            _directory: directory,
        })
    }

    fn receive(&mut self) -> Result<String> {
        loop {
            oer_process::check_cancelled()?;
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("hostapd did not become ready before its deadline".into());
            }
            let mut bytes = [0; 8192];
            match self.socket.recv(&mut bytes) {
                Ok(length) if length < bytes.len() => {
                    return Ok(std::str::from_utf8(&bytes[..length])?.to_owned());
                }
                Ok(_) => return Err("hostapd response exceeded the control-message limit".into()),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if let Err(error) = self
                .poll
                .poll(&mut Events::with_capacity(2), Some(remaining))
                && error.kind() != std::io::ErrorKind::Interrupted
            {
                return Err(error.into());
            }
        }
    }

    pub(crate) fn request(&mut self, command: &str) -> Result<String> {
        self.socket.send(command.as_bytes())?;
        loop {
            let response = self.receive()?;
            if !response.starts_with('<') {
                return Ok(response);
            }
            require_running(&response)?;
            self.pending_event = true;
        }
    }

    pub(crate) fn wait_enabled(&mut self) -> Result<String> {
        if self.request("ATTACH")?.trim() != "OK" {
            return Err("hostapd rejected event subscription".into());
        }
        loop {
            // Subscribe first, then inspect: readiness cannot fall between the
            // status read and subscription. Further reads follow actual events.
            let status = self.request("STATUS")?;
            if field(&status, "state") == Some("ENABLED") {
                return Ok(status);
            }
            if std::mem::take(&mut self.pending_event) {
                continue;
            }
            let event = self.receive()?;
            require_running(&event)?;
        }
    }
}

fn require_running(event: &str) -> Result<()> {
    if event.contains("AP-DISABLED") || event.contains("CTRL-EVENT-TERMINATING") {
        return Err("hostapd disabled the AP before readiness".into());
    }
    Ok(())
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = self.socket.send(b"DETACH");
    }
}

pub(crate) fn field<'a>(response: &'a str, key: &str) -> Option<&'a str> {
    response
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

#[cfg(test)]
mod tests;
