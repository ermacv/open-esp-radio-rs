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
        #[cfg(target_os = "linux")]
        wait_socket(path, Duration::from_secs(20))?;
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

/// Observe creation before checking existence, so a fast hostapd cannot race
/// the subscription. Only the startup watchdog is time based.
#[cfg(target_os = "linux")]
fn wait_socket(path: &Path, timeout: Duration) -> Result<()> {
    use std::{
        ffi::CString,
        os::{
            fd::{FromRawFd, OwnedFd},
            unix::ffi::OsStrExt,
        },
    };
    let parent = CString::new(
        path.parent()
            .ok_or("control socket has no parent")?
            .as_os_str()
            .as_bytes(),
    )?;
    // SAFETY: no borrowed memory is passed; a successful fd is owned below.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is a newly created, uniquely owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: parent is NUL-terminated and remains alive for this call.
    if unsafe {
        libc::inotify_add_watch(
            fd.as_raw_fd(),
            parent.as_ptr(),
            libc::IN_CREATE | libc::IN_MOVED_TO,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut poll = Poll::new()?;
    poll.registry()
        .register(&mut SourceFd(&fd.as_raw_fd()), Token(0), Interest::READABLE)?;
    let wake = Arc::new(Waker::new(poll.registry(), Token(1))?);
    let _notification = oer_process::notify_on_cancel(move || {
        let _ = wake.wake();
    });
    let deadline = Instant::now() + timeout;
    loop {
        oer_process::check_cancelled()?;
        if path.try_exists()? {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("hostapd did not create its control socket before its deadline".into());
        }
        poll.poll(&mut Events::with_capacity(2), Some(remaining))
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::Interrupted {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        let mut events = [0_u8; 4096];
        // SAFETY: events is a writable buffer of the supplied length. Event
        // contents are irrelevant; readiness only triggers a new path check.
        let count = unsafe { libc::read(fd.as_raw_fd(), events.as_mut_ptr().cast(), events.len()) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                return Err(error.into());
            }
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
