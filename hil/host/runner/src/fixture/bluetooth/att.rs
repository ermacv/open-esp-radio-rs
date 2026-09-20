//! Kernel/BlueZ fixed ATT fixture. The runner holds the ordinary adapter lease.
//! Requires an initially powered-off adapter; restores its power and rfkill state.
use super::model::{Adapter, PeerAddress};
use crate::Result;
use std::{
    fs,
    io::{self, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn bus(adapter: Adapter, args: &[&str]) -> Result<String> {
    let mut command = Command::new("busctl");
    command
        .args([
            "--system",
            "--timeout=5",
            args[0],
            "org.bluez",
            &format!("/org/bluez/{adapter}"),
            "org.bluez.Adapter1",
        ])
        .args(&args[1..]);
    let output = oer_process::output(&mut command, Some(Duration::from_secs(6)))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn power(adapter: Adapter, powered: bool) -> Result<()> {
    // BlueZ can still be re-registering the controller just after rfkill clears.
    // Retry the idempotent request, retaining a finite setup/cleanup deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = bus(
            adapter,
            &[
                "set-property",
                "Powered",
                "b",
                if powered { "true" } else { "false" },
            ],
        );
        match result {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => oer_process::sleep(Duration::from_millis(100))?,
        }
    }
}
pub(crate) fn preflight(adapter: Adapter) -> Result<()> {
    super::att_parameters::require_clean(adapter)?;
    if bus(adapter, &["get-property", "Powered"])? != "b false" {
        return Err("ATT calibration fixture requires an initially powered-off adapter".into());
    }
    Ok(())
}
pub(crate) struct Owner {
    adapter: Adapter,
    address: PeerAddress,
    rfkill: PathBuf,
    identity: PathBuf,
    index: u32,
    blocked: bool,
    restored: bool,
    parameters: Option<super::att_parameters::Lease>,
}
impl Owner {
    pub(crate) fn acquire(adapter: Adapter, output: &Path) -> Result<Self> {
        Self::acquire_profile(adapter, output, false)
    }
    pub(crate) fn acquire_calibration(adapter: Adapter, output: &Path) -> Result<Self> {
        Self::acquire_profile(adapter, output, true)
    }
    fn acquire_profile(adapter: Adapter, output: &Path, calibration: bool) -> Result<Self> {
        preflight(adapter)?;
        let address = bus(adapter, &["get-property", "Address"])?;
        let address: PeerAddress = address
            .strip_prefix("s \"")
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("invalid BlueZ address")?
            .parse()?;
        let rfkill = fs::read_dir("/sys/class/rfkill")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                fs::read_to_string(p.join("name")).is_ok_and(|s| s.trim() == adapter.to_string())
            })
            .ok_or("missing Bluetooth rfkill")?;
        if fs::read_to_string(rfkill.join("hard"))?.trim() != "0" {
            return Err("Bluetooth adapter hardware blocked".into());
        }
        let mut owner = Self {
            adapter,
            address,
            identity: fs::canonicalize(&rfkill)?,
            index: fs::read_to_string(rfkill.join("index"))?.trim().parse()?,
            blocked: fs::read_to_string(rfkill.join("soft"))?.trim() == "1",
            rfkill,
            restored: false,
            parameters: None,
        };
        crate::evidence::run::atomic_json(
            &output.join("adapter-before.json"),
            &serde_json::json!({"adapter": adapter.to_string(), "address": address.to_string(), "powered": false, "soft_blocked": owner.blocked}),
        )?;
        if calibration {
            owner.parameters = Some(super::att_parameters::Lease::acquire(adapter, output)?);
        }
        owner.block(false)?;
        power(adapter, true)?;
        if bus(adapter, &["get-property", "Powered"])? != "b true" {
            return Err("BlueZ did not power adapter".into());
        }
        Ok(owner)
    }
    fn block(&self, blocked: bool) -> Result<()> {
        if fs::canonicalize(&self.rfkill)? != self.identity {
            return Err("rfkill identity changed".into());
        }
        let mut event = [0u8; 8];
        event[..4].copy_from_slice(&self.index.to_ne_bytes());
        event[4..].copy_from_slice(&[2, 2, u8::from(blocked), 0]);
        fs::OpenOptions::new()
            .write(true)
            .open("/dev/rfkill")?
            .write_all(&event)?;
        if (fs::read_to_string(self.rfkill.join("soft"))?.trim() == "1") != blocked {
            return Err("rfkill restore mismatch".into());
        }
        Ok(())
    }
    pub(crate) fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        let power = power(self.adapter, false);
        let block = self.block(self.blocked);
        // Even a failed power/rfkill request must release the helper. It checks
        // adapter identity and independently restores the original snapshot.
        let parameters = self
            .parameters
            .as_mut()
            .map_or(Ok(()), |lease| lease.restore());
        let errors: Vec<_> = [power, block, parameters]
            .into_iter()
            .filter_map(|result| result.err().map(|error| error.to_string()))
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("; ").into());
        }
        if bus(self.adapter, &["get-property", "Powered"])? != "b false" {
            return Err("adapter power restore mismatch".into());
        }
        self.restored = true;
        Ok(())
    }
    pub(crate) fn connect(&self, peer: PeerAddress) -> Result<Att> {
        Att::connect(self.address, peer)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if !self.restored {
            let _ = oer_process::cleanup(|| self.restore());
        }
    }
}

// Linux sockaddr_l2 from bluetooth/l2cap.h; cid is little endian, address is
// the HCI byte order. All padding is initialized before passing it to libc.
#[repr(C)]
struct Address {
    family: u16,
    psm: u16,
    address: [u8; 6],
    cid: u16,
    kind: u8,
}
fn address(value: PeerAddress) -> Address {
    // SAFETY: every field is an integer; zero is a valid value for all fields.
    let mut a: Address = unsafe { std::mem::zeroed() };
    a.family = libc::AF_BLUETOOTH as u16;
    a.address = value.0;
    a.cid = 4u16.to_le();
    a.kind = 1;
    a
}
pub(crate) struct Att(OwnedFd);
impl Att {
    fn connect(local: PeerAddress, peer: PeerAddress) -> Result<Self> {
        // SAFETY: creates an independent kernel L2CAP descriptor, no pointers.
        let fd = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: this successful socket call transfers the unique descriptor.
        let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        let security = [1u8, 0];
        // SAFETY: readable two-byte Linux bt_security value, live descriptor.
        if unsafe { libc::setsockopt(fd, 274, 4, security.as_ptr().cast(), 2) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let local = address(local);
        let remote = address(peer);
        // SAFETY: initialized sockaddr_l2 and correct size remain live for bind.
        if unsafe {
            libc::bind(
                fd,
                (&local as *const Address).cast(),
                std::mem::size_of::<Address>() as _,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: same initialized Linux address layout for the remote endpoint.
        let result = unsafe {
            libc::connect(
                fd,
                (&remote as *const Address).cast(),
                std::mem::size_of::<Address>() as _,
            )
        };
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(io::Error::last_os_error().into());
        }
        socket.wait(libc::POLLOUT, Instant::now() + Duration::from_secs(10))?;
        let mut error = 0i32;
        let mut len = std::mem::size_of_val(&error) as libc::socklen_t;
        // SAFETY: writable error integer and length, live socket.
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut len,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error).into());
        }
        Ok(socket)
    }
    fn wait(&self, events: i16, deadline: Instant) -> Result<()> {
        loop {
            if oer_process::cancellation_requested() {
                return Err("ATT fixture cancelled".into());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("ATT fixture deadline exceeded".into());
            }
            let mut poll = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events,
                revents: 0,
            };
            // SAFETY: one initialized pollfd remains writable for the call.
            let result = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(100) as i32) };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::last_os_error().into());
            }
            if result > 0 {
                if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    return Err("ATT socket disconnected".into());
                }
                if poll.revents & events != 0 {
                    return Ok(());
                }
            }
        }
    }
    pub(crate) fn send(&self, bytes: &[u8]) -> Result<()> {
        self.wait(libc::POLLOUT, Instant::now() + Duration::from_secs(2))?;
        // SAFETY: bytes is readable for exactly its length; descriptor is owned.
        let n = unsafe {
            libc::send(
                self.0.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n != bytes.len() as isize {
            return Err("incomplete ATT send".into());
        }
        Ok(())
    }
    pub(crate) fn receive(&self) -> Result<Vec<u8>> {
        self.wait(libc::POLLIN, Instant::now() + Duration::from_secs(2))?;
        let mut bytes = [0u8; 256];
        // SAFETY: buffer is writable for its declared size, socket is live.
        let n = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
            )
        };
        if n <= 0 || n as usize > bytes.len() {
            return Err("invalid ATT packet size".into());
        }
        Ok(bytes[..n as usize].to_vec())
    }
}
