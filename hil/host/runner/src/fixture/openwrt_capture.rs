//! Scoped OpenWrt packet capture on an owned monitor or an existing interface.
//! Readiness and stop use process events; only created interfaces are removed.
use super::capture_process;
use crate::{Result, lab::config::OpenWrtConfig};
use oer_process::CommandExt as _;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;

struct RemoteInterface {
    create_interface: bool,
    interface: String,
    directory: String,
}

impl RemoteInterface {
    fn new(interface: String) -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            create_interface: true,
            interface,
            directory: format!(
                "/tmp/oer-capture-{}-{epoch}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        })
    }

    fn capture(&self) -> String {
        format!("{}/capture.pcap", self.directory)
    }

    fn start_script(
        &self,
        config: &OpenWrtConfig,
        filter: &str,
        immediate: bool,
        duration: Duration,
    ) -> String {
        let monitor = &self.interface;
        let directory = &self.directory;
        let remote = self.capture();
        let lifetime = duration.saturating_add(Duration::from_secs(120));
        let mode = if immediate && !self.create_interface {
            "--immediate-mode -s 128"
        } else if immediate {
            "--immediate-mode -s 512"
        } else {
            "-s 128"
        };
        let program = format!(
            "tcpdump -i {monitor} -n {mode} -U -w {remote} {}",
            capture_process::quote(filter)
        );
        if !self.create_interface {
            let controlled = capture_process::controlled(&program, ":");
            let script = format!("set -eu; umask 077; mkdir {directory}; {controlled}");
            return format!(
                "LC_ALL=C timeout -s TERM {} sh -c {}",
                lifetime.as_secs(),
                capture_process::quote(&script)
            );
        }
        let controlled = capture_process::controlled(&program, "cleanup");
        let script = format!(
            "set -eu; \
             if iw dev {monitor} info >/dev/null 2>&1; then echo 'monitor interface already exists' >&2; exit 1; fi; \
             wiphy=$(iw dev {wireless} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
             test -n \"$wiphy\"; \
             umask 077; mkdir {directory}; \
             cleanup() {{ iw dev {monitor} del >/dev/null 2>&1 || true; }}; \
             trap cleanup EXIT; \
             trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM; \
             iw phy \"$wiphy\" interface add {monitor} type monitor; \
             ip link set {monitor} up; \
             {controlled}",
            wireless = config.wireless_interface,
        );
        format!(
            "LC_ALL=C timeout -s TERM {} sh -c {}",
            lifetime.as_secs(),
            capture_process::quote(&script)
        )
    }

    fn cleanup_script(&self) -> String {
        if !self.create_interface {
            return format!(
                "if test -d {0}; then rm -f {1}; rmdir {0}; fi",
                self.directory,
                self.capture()
            );
        }
        // The private directory is acquired only after rejecting an existing
        // interface. A failed preflight/spawn therefore cannot delete it.
        format!(
            "set -eu; if test -d {directory}; then \
             if iw dev {monitor} info >/dev/null 2>&1; then iw dev {monitor} del; fi; \
             rm -f {capture}; rmdir {directory}; fi",
            directory = self.directory,
            monitor = self.interface,
            capture = self.capture(),
        )
    }
}

pub(super) struct RemoteCapture {
    config: OpenWrtConfig,
    remote: RemoteInterface,
    output: PathBuf,
    child: Option<capture_process::Capture>,
}

impl RemoteCapture {
    pub(super) fn output_path(&self) -> &Path {
        &self.output
    }

    pub(super) fn start_monitor(
        config: &OpenWrtConfig,
        output: PathBuf,
        filter: &str,
        immediate: bool,
        duration: Duration,
    ) -> Result<Self> {
        let monitor = config
            .monitor_interface
            .clone()
            .ok_or("OpenWrt capture requires station_fixture.monitor_interface")?;
        oer_process::check_cancelled()?;
        let remote = RemoteInterface::new(monitor)?;
        let script = remote.start_script(config, filter, immediate, duration);
        // Own cleanup before starting the remote process or waiting for readiness.
        let mut owner = Self {
            config: config.clone(),
            remote,
            output,
            child: None,
        };
        owner.child = Some(capture_process::Capture::start(
            &mut ssh(config, &script),
            format!("tcpdump: listening on {},", owner.remote.interface),
            duration.saturating_add(Duration::from_secs(120)),
        )?);
        Ok(owner)
    }

    pub(super) fn start_managed(
        config: &OpenWrtConfig,
        interface: &str,
        output: PathBuf,
        filter: &str,
        duration: Duration,
    ) -> Result<Self> {
        let mut remote = RemoteInterface::new(interface.to_owned())?;
        remote.create_interface = false;
        let script = remote.start_script(config, filter, true, duration);
        let mut owner = Self {
            config: config.clone(),
            remote,
            output,
            child: None,
        };
        owner.child = Some(capture_process::Capture::start(
            &mut ssh(config, &script),
            format!("tcpdump: listening on {},", interface),
            duration.saturating_add(Duration::from_secs(120)),
        )?);
        Ok(owner)
    }

    pub(super) fn finish_capture(&mut self) -> Result<(u64, u64)> {
        let child = self.child.take().expect("packet capture owns its child");
        let output = child.finish()?;
        if !output.status.success() {
            return Err(crate::fixture::Error::new(format!(
                "OpenWrt packet capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = summary_value(&summary, "packets captured")
            .ok_or("OpenWrt packet capture omitted its packet count")?;
        let kernel_dropped = summary_value(&summary, "packets dropped by kernel")
            .ok_or("tcpdump omitted its drop count")?;
        if kernel_dropped != 0 {
            return Err(crate::fixture::Error::new(format!(
                "OpenWrt packet capture dropped {kernel_dropped} packets in its capture socket"
            ))
            .into());
        }
        copy_remote(&self.config, &self.remote.capture(), &self.output)?;
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(crate::fixture::Error::new(format!(
                "OpenWrt packet capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            ))
            .into());
        }
        Ok((captured_frames, kernel_dropped))
    }
}

impl Drop for RemoteCapture {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            drop(self.child.take());
            let cleanup = self.remote.cleanup_script();
            crate::fixture::cleanup::command(
                "clean up OpenWrt capture",
                &mut ssh(&self.config, &cleanup),
            );
        });
    }
}

fn copy_remote(config: &OpenWrtConfig, remote: &str, local: &Path) -> Result<()> {
    let file = File::create(local)?;
    let status = ssh(config, &format!("cat {remote}"))
        .stdout(Stdio::from(file))
        .supervised_status()?;
    if !status.success() {
        return Err(crate::fixture::Error::new(
            "cannot copy OpenWrt packet capture to the run directory",
        )
        .into());
    }
    Ok(())
}

pub(super) fn ssh(config: &OpenWrtConfig, script: &str) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script);
    command
}

fn summary_value(summary: &str, suffix: &str) -> Option<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_suffix(suffix)?.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests;
