//! Machine-readable stdout ownership for the host CLI.

use std::{
    fs::File,
    io::Write as _,
    sync::{Mutex, OnceLock},
};

use serde::Serialize;

use crate::Result;

static MACHINE_STDOUT: OnceLock<Mutex<Box<dyn std::io::Write + Send>>> = OnceLock::new();

pub(crate) fn reserve_machine_stdout() -> Result<()> {
    #[cfg(unix)]
    let output: Box<dyn std::io::Write + Send> = {
        use std::os::fd::FromRawFd as _;

        // SAFETY: `dup` returns a new owned descriptor or -1. Ownership of a
        // successful descriptor is transferred exactly once to `File`.
        let descriptor = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the descriptor was just created by `dup` and is uniquely
        // owned here.
        let file = unsafe { File::from_raw_fd(descriptor) };
        // SAFETY: both standard descriptors are process-owned and valid. This
        // redirects inherited child stdout to the diagnostic stderr stream.
        if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Box::new(file)
    };
    #[cfg(not(unix))]
    let output: Box<dyn std::io::Write + Send> = Box::new(std::io::stdout());

    MACHINE_STDOUT
        .set(Mutex::new(output))
        .map_err(|_| "machine stdout was initialized more than once".into())
}

pub(crate) fn emit_json(value: &impl Serialize, pretty: bool) -> Result<()> {
    let output = MACHINE_STDOUT
        .get()
        .ok_or("machine stdout is not initialized")?;
    let mut output = output
        .lock()
        .map_err(|_| "machine stdout lock is poisoned")?;
    if pretty {
        serde_json::to_writer_pretty(&mut **output, value)?;
    } else {
        serde_json::to_writer(&mut **output, value)?;
    }
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn machine_output_child_harness() {
        if std::env::var_os("OER_HIL_MACHINE_OUTPUT_CHILD").is_none() {
            return;
        }
        super::reserve_machine_stdout().unwrap();
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                "printf 'child stdout diagnostic'; printf 'child stderr diagnostic' >&2",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        super::emit_json(&serde_json::json!({"status": "machine-output-ok"}), false).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn child_diagnostics_are_separate_from_machine_json() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "output::tests::machine_output_child_harness",
                "--nocapture",
            ])
            .env("OER_HIL_MACHINE_OUTPUT_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let machine = stdout
            .lines()
            .find(|line| line.starts_with('{'))
            .expect("machine JSON line");
        let machine: serde_json::Value = serde_json::from_str(machine).unwrap();
        assert_eq!(machine["status"], "machine-output-ok");
        assert!(!stdout.contains("child stdout diagnostic"));
        let diagnostics = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostics.contains("child stdout diagnostic"));
        assert!(diagnostics.contains("child stderr diagnostic"));
    }
}
