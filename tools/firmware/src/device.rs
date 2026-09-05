//! Exclusive ownership of a serial device across host tools and checkouts.

use crate::Result;
use fs2::FileExt;
use serialport::SerialPortType;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub struct DeviceLease {
    file: File,
    port: PathBuf,
}

impl DeviceLease {
    pub fn acquire(port: &Path) -> Result<Self> {
        let canonical = fs::canonicalize(port)?;
        let identity = serialport::available_ports()?
            .into_iter()
            .find_map(|candidate| {
                if fs::canonicalize(&candidate.port_name).ok().as_ref() != Some(&canonical) {
                    return None;
                }
                match candidate.port_type {
                    SerialPortType::UsbPort(usb) => usb
                        .serial_number
                        .filter(|serial| !serial.is_empty())
                        .map(|serial| format!("usb:{:04x}:{:04x}:{serial}", usb.vid, usb.pid)),
                    _ => None,
                }
            })
            .unwrap_or_else(|| format!("serial:{}", canonical.display()));
        Self::acquire_identity(&lease_directory()?, &identity, port)
    }

    /// Resolve automatic selection once, then keep that device for the whole
    /// flash transaction. Multiple USB ports require explicit selection.
    pub fn select(port: Option<&Path>) -> Result<Self> {
        if let Some(port) = port {
            return Self::acquire(port);
        }
        let ports = serialport::available_ports()?
            .into_iter()
            .filter(|port| matches!(port.port_type, SerialPortType::UsbPort(_)))
            .collect::<Vec<_>>();
        let [port] = ports.as_slice() else {
            return Err(
                "automatic flashing requires exactly one USB serial device; specify --port".into(),
            );
        };
        Self::acquire(Path::new(&port.port_name))
    }

    pub fn port(&self) -> &Path {
        &self.port
    }

    fn acquire_identity(directory: &Path, identity: &str, port: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join(format!(
            "serial-{:x}.lock",
            Sha256::digest(identity.as_bytes())
        ));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        if file.try_lock_exclusive().is_err() {
            let mut owner = String::new();
            file.read_to_string(&mut owner)?;
            return Err(format!(
                "serial device is already leased by {} ({})",
                owner.trim(),
                path.display()
            )
            .into());
        }
        let mut lease = Self {
            file,
            port: port.to_owned(),
        };
        lease.file.set_len(0)?;
        lease.file.seek(SeekFrom::Start(0))?;
        writeln!(lease.file, "pid={}", std::process::id())?;
        lease.file.flush()?;
        Ok(lease)
    }
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Per-user host ownership, independent of a repository's target directory.
pub fn lease_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or("HOME is required to locate host device leases")?;
    Ok(PathBuf::from(home).join(".cache/open-esp-radio/leases"))
}

#[cfg(test)]
mod tests;
