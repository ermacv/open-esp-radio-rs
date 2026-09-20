//! Unprivileged owner of the installed helper's bounded ATT-parameter lease.
use super::model::Adapter;
use crate::Result;
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn require_clean(adapter: Adapter) -> Result<()> {
    let journal = format!("/run/open-radio-bluetooth/{adapter}.att-parameters.json");
    if Path::new(&journal).try_exists()? {
        return Err(format!("unfinished ATT parameter lease: {journal}; recover using sudo /usr/local/libexec/open-radio-bluetooth restore-att-parameters --adapter {adapter}").into());
    }
    Ok(())
}

pub(crate) fn preflight(adapter: Adapter) -> Result<()> {
    super::att::preflight(adapter)?;
    super::preflight(adapter)?;
    let output = oer_process::output(
        Command::new("sudo").args([
            "-n",
            "-l",
            "/usr/local/libexec/open-radio-bluetooth",
            "att-parameters",
            "--adapter",
            &adapter.to_string(),
        ]),
        Some(Duration::from_secs(5)),
    )?;
    if !output.status.success() {
        return Err("install the ATT parameter helper: cargo hil fixture install --provider linux-bluetooth".into());
    }
    Ok(())
}

pub(crate) struct Lease {
    child: Option<oer_process::owned::Child>,
    failure: Option<String>,
}
impl Lease {
    pub(crate) fn acquire(adapter: Adapter, output: &Path) -> Result<Self> {
        preflight(adapter)?;
        let mut command = Command::new("sudo");
        command
            .args([
                "-n",
                "/usr/local/libexec/open-radio-bluetooth",
                "att-parameters",
                "--adapter",
                &adapter.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(output.join("att-parameters.stderr"))?);
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        expect_line(
            child.stdout.as_mut().ok_or("ATT helper stdout absent")?,
            b"ATT_PARAMETERS_READY_V1\n",
        )?;
        Ok(Self {
            child: Some(child),
            failure: None,
        })
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        drop(child.stdin.take()); // EOF transfers cleanup responsibility back to root.
        let result = (|| -> Result<()> {
            expect_line(
                child.stdout.as_mut().ok_or("ATT helper stdout absent")?,
                b"ATT_PARAMETERS_RESTORED_V1\n",
            )?;
            if !child.wait_timeout(Some(Duration::from_secs(20)))?.success() {
                return Err("ATT parameter restoration helper failed".into());
            }
            Ok(())
        })();
        if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = oer_process::cleanup(|| self.restore()) {
            eprintln!("ATT parameter cleanup: {error}");
        }
    }
}

fn expect_line(pipe: &mut (impl Read + AsRawFd), expected: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    for expected_byte in expected {
        loop {
            oer_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("ATT helper handshake timeout".into());
            }
            let mut fd = libc::pollfd {
                fd: pipe.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: a single initialized pollfd borrows the live pipe descriptor.
            let ready = unsafe { libc::poll(&mut fd, 1, 100) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            if ready == 0 {
                continue;
            }
            let mut byte = [0];
            if pipe.read(&mut byte)? != 1 || byte[0] != *expected_byte {
                return Err("ATT helper handshake failed; inspect att-parameters.stderr and recovery journal".into());
            }
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::net::UnixStream};
    #[test]
    fn handshake_requires_exact_complete_message() {
        for (message, valid) in [
            (b"READY\n".as_slice(), true),
            (b"READY", false),
            (b"ready\n", false),
        ] {
            let (mut receiver, mut sender) = UnixStream::pair().unwrap();
            sender.write_all(message).unwrap();
            drop(sender);
            assert_eq!(expect_line(&mut receiver, b"READY\n").is_ok(), valid);
        }
    }
}
