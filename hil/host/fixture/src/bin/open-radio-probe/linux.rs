use super::model::{self, Config, Report};
use std::{
    ffi::CString,
    io::Write,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::Command,
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const MONITOR: &str = "oerprobe0";

fn iw(args: &[&str]) -> Result<String> {
    let output = oer_process::output(
        Command::new("/usr/bin/iw").args(args),
        Some(std::time::Duration::from_secs(5)),
    )?;
    if !output.status.success() {
        return Err(format!("iw failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

struct Monitor {
    owned: bool,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        if self.owned {
            oer_process::cleanup(|| {
                let _ = iw(&["dev", MONITOR, "del"]);
            });
        }
    }
}

struct Control {
    input: i32,
    cancel: std::os::unix::net::UnixStream,
    _notification: oer_process::CancellationNotification,
}
impl Control {
    fn new(input: i32) -> Result<Self> {
        let (read, write) = std::os::unix::net::UnixStream::pair()?;
        write.set_nonblocking(true)?;
        let notification = oer_process::notify_on_cancel(move || {
            let _ = (&write).write(&[1]);
        });
        Ok(Self {
            input,
            cancel: read,
            _notification: notification,
        })
    }
    fn wait(&self, deadline: Instant) -> Result<bool> {
        loop {
            oer_process::check_cancelled()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.input,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.cancel.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let timeout = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            // SAFETY: the array contains exactly two initialized pollfd values.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
            if ready < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            oer_process::check_cancelled()?;
            if fds[0].revents != 0 {
                return Ok(true);
            }
        }
    }
    fn line(&self, deadline: Instant) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            if !self.wait(deadline)? {
                return Err("probe command timeout".into());
            }
            let mut byte = [0];
            // SAFETY: input is the live controller descriptor; byte is a writable one-byte buffer.
            // Do not mix buffered stdin with poll: prefetched bytes would hide readiness.
            let count = unsafe { libc::read(self.input, byte.as_mut_ptr().cast(), 1) };
            if count < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if count == 0 {
                return Err("probe controller disconnected".into());
            }
            if byte[0] == b'\n' {
                return Ok(String::from_utf8(bytes)?);
            }
            bytes.push(byte[0]);
            if bytes.len() > 256 {
                return Err("probe command exceeds 256 bytes".into());
            }
        }
    }
}

fn socket() -> Result<OwnedFd> {
    let name = CString::new(MONITOR)?;
    // SAFETY: name is NUL-terminated and lives through the call.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: socket creates a new descriptor; no pointers or borrowed owners.
    let raw = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            3u16.to_be() as i32,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful socket returns one fresh owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let address = libc::sockaddr_ll {
        sll_family: libc::AF_PACKET as u16,
        sll_protocol: 3u16.to_be(),
        sll_ifindex: index as i32,
        sll_hatype: 0,
        sll_pkttype: 0,
        sll_halen: 0,
        sll_addr: [0; 8],
    };
    // SAFETY: address points to an initialized sockaddr_ll of the supplied size.
    if unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_ll).cast(),
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(fd)
}

pub fn run() -> Result<()> {
    let _signals = oer_process::install_signal_handlers()?;
    let control = Control::new(0)?;
    use std::os::unix::fs::OpenOptionsExt as _;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/run/open-radio-probe.lock")?;
    fs2::FileExt::try_lock_exclusive(&lock)?;
    let config: Config =
        serde_json::from_str(&control.line(Instant::now() + std::time::Duration::from_secs(10))?)?;
    config.validate()?;
    // The fixture's controlled client is wlan0. An existing monitor or missing
    // association is rejected before acquiring any resources.
    let link = iw(&["dev", "wlan0", "link"])?;
    let bssid_text = link
        .lines()
        .find_map(|s| s.trim().strip_prefix("Connected to "))
        .and_then(|s| s.split_whitespace().next())
        .ok_or("probe source is not associated")?;
    let bytes: Vec<u8> = bssid_text
        .split(':')
        .map(|v| u8::from_str_radix(v, 16))
        .collect::<std::result::Result<_, _>>()?;
    let bssid: [u8; 6] = bytes.try_into().map_err(|_| "invalid BSSID")?;
    let frequency = 2407 + u16::from(config.channel) * 5;
    if !link
        .lines()
        .any(|s| s.trim() == format!("freq: {frequency}"))
        || !link
            .lines()
            .any(|s| s.trim() == format!("SSID: {}", config.ssid))
    {
        return Err("probe source association does not match scenario SSID/channel".into());
    }
    if iw(&["dev"])?
        .lines()
        .any(|line| line.trim() == format!("Interface {MONITOR}"))
    {
        return Err("probe monitor is already owned".into());
    }
    let info = iw(&["dev", "wlan0", "info"])?;
    let phy = info
        .lines()
        .find_map(|s| s.trim().strip_prefix("wiphy "))
        .ok_or("missing source PHY")?;
    let mut monitor = Monitor { owned: true };
    iw(&[
        "phy",
        &format!("phy{phy}"),
        "interface",
        "add",
        MONITOR,
        "type",
        "monitor",
    ])?;
    let status = oer_process::output(
        Command::new("/usr/bin/ip").args(["link", "set", MONITOR, "up"]),
        Some(std::time::Duration::from_secs(5)),
    )?
    .status;
    if !status.success() {
        return Err("cannot enable probe monitor".into());
    }
    let fd = socket()?;
    println!("{}", model::READY);
    std::io::stdout().flush()?;
    if control.line(Instant::now() + std::time::Duration::from_secs(30))? != "start" {
        return Err("expected probe Start".into());
    }
    let start = Instant::now();
    let mut report = Report {
        bssid,
        ..Report::default()
    };
    let result = (|| -> Result<()> {
        for index in 0..model::REQUESTS {
            let request = model::request(index).expect("bounded request index");
            let deadline = start + std::time::Duration::from_micros(request.offset_us);
            if control.wait(deadline)? {
                return Err("probe workload cancelled or controller disconnected".into());
            }
            let lateness = Instant::now()
                .saturating_duration_since(deadline)
                .as_micros() as u64;
            report.maximum_lateness_us = report.maximum_lateness_us.max(lateness);
            if lateness > model::MAX_LATENESS_US {
                return Err("probe pacing deadline missed; no catch-up burst permitted".into());
            }
            let frame = super::frame::encode(&config, bssid, request);
            // SAFETY: frame remains readable for the synchronous send call.
            let written =
                unsafe { libc::send(fd.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0) };
            if written < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if written as usize != frame.len() {
                return Err("partial probe injection".into());
            }
            report.submitted += 1;
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.error = Some(error.to_string());
    }
    drop(fd);
    // Report only after teardown. Failure of explicit removal is evidence,
    // while Drop remains the fallback on every earlier error.
    if let Err(error) = iw(&["dev", MONITOR, "del"]) {
        report.error = Some(format!("probe cleanup failed: {error}"));
    } else {
        monitor.owned = false;
    }
    serde_json::to_writer(std::io::stdout().lock(), &report)?;
    println!();
    report.validate()?;
    Ok(())
}

#[cfg(test)]
mod tests;
