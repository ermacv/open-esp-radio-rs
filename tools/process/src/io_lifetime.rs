//! Retain an already admitted I/O operation through an external command.

use std::{
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
    sync::Arc,
};

/// A lifetime connection, not authority to admit another operation. An empty
/// lifetime is used for operations without a device, such as hub discovery.
#[derive(Clone, Debug, Default)]
pub struct IoLifetime {
    connection: Option<Arc<UnixStream>>,
}

impl IoLifetime {
    pub(crate) fn new(connection: UnixStream) -> Self {
        Self {
            connection: Some(Arc::new(connection)),
        }
    }

    /// Keep exclusion until this command and its ordinary descendants close
    /// their inherited connection, even if the operation's caller dies first.
    /// The child receives neither the lock descriptor nor admission authority.
    pub fn pin(&self, command: &mut Command) -> crate::Result<()> {
        let Some(connection) = &self.connection else {
            return Ok(());
        };
        let connection = connection.try_clone()?;
        // SAFETY: pre_exec only adjusts a live captured descriptor with fcntl.
        // It performs no allocation, locking or application cleanup after fork.
        unsafe {
            command.pre_exec(move || {
                let fd = connection.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}
