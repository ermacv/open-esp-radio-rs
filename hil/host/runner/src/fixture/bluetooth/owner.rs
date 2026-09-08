//! Acquire the exclusive Linux user channel and restore power/rfkill on release.

use super::{
    Result,
    hci::{ManagementError, Socket},
    model::{Adapter, Check},
};
use bt_hci::cmd::{
    controller_baseband::Reset,
    info::{ReadBdAddr, ReadLocalSupportedCmds, ReadLocalVersionInformation},
    le::{LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::PathBuf,
    time::{Duration, Instant},
};

struct Rfkill {
    index: u32,
    path: PathBuf,
    identity: PathBuf,
    blocked: bool,
}

impl Rfkill {
    fn read(adapter: Adapter) -> Result<Self> {
        for entry in fs::read_dir("/sys/class/rfkill")? {
            let path = entry?.path();
            if fs::read_to_string(path.join("type"))?.trim() != "bluetooth"
                || fs::read_to_string(path.join("name"))?.trim() != adapter.to_string()
            {
                continue;
            }
            if fs::read_to_string(path.join("hard"))?.trim() != "0" {
                return Err("Bluetooth adapter is hardware blocked".into());
            }
            return Ok(Self {
                index: fs::read_to_string(path.join("index"))?.trim().parse()?,
                blocked: fs::read_to_string(path.join("soft"))?.trim() == "1",
                identity: fs::canonicalize(&path)?,
                path,
            });
        }
        Err(format!("no rfkill identity for {adapter}").into())
    }
    fn set(&self, blocked: bool) -> Result<()> {
        if fs::canonicalize(&self.path)? != self.identity
            || fs::read_to_string(self.path.join("index"))?
                .trim()
                .parse::<u32>()?
                != self.index
        {
            return Err("rfkill identity changed".into());
        }
        let mut event = [0; 8];
        event[..4].copy_from_slice(&self.index.to_ne_bytes());
        event[4..].copy_from_slice(&[2, 2, u8::from(blocked), 0]);
        OpenOptions::new()
            .write(true)
            .open("/dev/rfkill")?
            .write_all(&event)?;
        let actual = fs::read_to_string(self.path.join("soft"))?;
        if (actual.trim() == "1") != blocked {
            return Err("rfkill state did not change".into());
        }
        Ok(())
    }
}

struct Owner {
    adapter: Adapter,
    management: Socket,
    user: Option<Socket>,
    rfkill: Rfkill,
    powered: bool,
    identity: Vec<u8>,
    restore_needed: bool,
    _lock: File,
}

impl Owner {
    fn snapshot(adapter: Adapter) -> Result<Self> {
        // SAFETY: geteuid only reads the process identity.
        if unsafe { libc::geteuid() } != 0 {
            return Err("DTM requires the installed privileged Bluetooth helper".into());
        }
        match fs::DirBuilder::new()
            .mode(0o755)
            .create("/run/open-radio-bluetooth")
        {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata("/run/open-radio-bluetooth")?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err("Bluetooth lock directory must be owned and writable only by root".into());
        }
        // The lock directory is created by this helper, never under a
        // caller-writable directory. O_NOFOLLOW rejects a substituted leaf.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(format!("/run/open-radio-bluetooth/{adapter}.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)?;
        let management = Socket::open(u16::MAX, 3)?;
        let info = management.management(adapter.0, 4, &[])?;
        let powered = powered(&info)?;
        if powered {
            let connections = management.management(adapter.0, 0x15, &[])?;
            if connections != [0, 0] {
                return Err(
                    "Bluetooth adapter has active connections; use a dedicated fixture adapter"
                        .into(),
                );
            }
        }
        let rfkill = Rfkill::read(adapter)?;
        Ok(Self {
            adapter,
            management,
            user: None,
            rfkill,
            powered,
            identity: info[..6].to_vec(),
            restore_needed: false,
            _lock: lock,
        })
    }

    fn acquire(&mut self) -> Result<()> {
        self.restore_needed = true;
        self.rfkill.set(false)?;
        self.manage_retry(5, &[0])?;
        self.user = Some(Socket::open(self.adapter.0, 1)?);
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        if !self.restore_needed {
            return Ok(());
        }
        // Closing the user channel re-registers the controller with the kernel.
        // Reset is attempted first even when a DTM start timed out.
        let mut errors = Vec::new();
        if let Some(user) = self.user.take() {
            if let Err(error) = user.command(Reset::new()) {
                errors.push(format!("controller reset: {error}"));
            }
            drop(user);
        }
        let info = self.manage_retry(4, &[]);
        let mut same_adapter = true;
        match info {
            Ok(info) if info.get(..6) == Some(&self.identity[..]) => {
                if let Err(error) = self.manage_retry(5, &[u8::from(self.powered)]) {
                    errors.push(format!("power restore: {error}"));
                }
            }
            Ok(_) => {
                same_adapter = false;
                errors.push("adapter identity changed during restoration".into());
            }
            Err(error) => errors.push(format!("adapter re-registration: {error}")),
        }
        // A management failure must not skip rfkill recovery. The original
        // sysfs identity is checked independently before writing its index.
        if same_adapter {
            if let Err(error) = self.rfkill.set(self.rfkill.blocked) {
                errors.push(format!("rfkill restore: {error}"));
            }
            match self
                .management
                .management(self.adapter.0, 4, &[])
                .and_then(|info| powered(&info))
            {
                Ok(actual) if actual == self.powered => {}
                Ok(_) => errors.push("restored power differs from snapshot".into()),
                Err(error) => errors.push(format!("power verification: {error}")),
            }
        }
        self.restore_needed = false;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; ").into())
        }
    }

    fn manage_retry(&self, opcode: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match self.management.management(self.adapter.0, opcode, payload) {
                Err(error)
                    if Instant::now() < deadline
                        && error
                            .downcast_ref::<ManagementError>()
                            .is_some_and(|error| matches!(error.status, 0x0a | 0x11)) =>
                {
                    oer_process::sleep(Duration::from_millis(50))?;
                }
                result => return result,
            }
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.restore_needed
            && let Err(error) = oer_process::cleanup(|| self.restore())
        {
            eprintln!("Bluetooth restoration failed: {error}");
        }
    }
}

fn powered(info: &[u8]) -> Result<bool> {
    if info.len() != 280 {
        return Err("invalid Read Controller Information response".into());
    }
    Ok(u32::from_le_bytes(info[13..17].try_into()?) & 1 != 0)
}

// Keep completion and restoration evidence separate: successful RX/TX cannot
// hide cleanup failure, and a preparation error must still run restoration.
fn checked_lifetime<T>(
    owner: &mut T,
    report: &mut Check,
    run: impl FnOnce(&mut T, &mut Check) -> Result<()>,
    restore: impl FnOnce(&mut T) -> Result<()>,
) {
    if let Err(error) = run(owner, report) {
        report.errors.push(error.to_string());
    }
    match oer_process::cleanup(|| restore(owner)) {
        Ok(()) => report.restored = true,
        Err(error) => report.errors.push(format!("restore: {error}")),
    }
}

pub(super) fn check(adapter: Adapter, report: &mut Check) -> Result<()> {
    let mut owner = Owner::snapshot(adapter)?;
    report.initial_powered = Some(owner.powered);
    report.initial_soft_blocked = Some(owner.rfkill.blocked);
    checked_lifetime(
        &mut owner,
        report,
        |owner, report| {
            owner.acquire()?;
            let user = owner.user.as_ref().ok_or("missing exclusive HCI channel")?;
            user.command(Reset::new())?;
            report.address = Some(format!("{:?}", user.command(ReadBdAddr::new())?));
            report.version = Some(format!(
                "{:?}",
                user.command(ReadLocalVersionInformation::new())?
            ));
            let mask = user.command(ReadLocalSupportedCmds::new())?;
            report.dtm_v2_advertised =
                mask.le_receiver_test_v2() && mask.le_transmitter_test_v2() && mask.le_test_end();
            if !report.dtm_v2_advertised {
                return Err("adapter does not advertise DTM v2 RX/TX/Test End".into());
            }
            user.command(LeReceiverTestV2::new(0, 1, 0))?;
            report.rx_started = true;
            oer_process::sleep(Duration::from_millis(100))?;
            report.rx_packets = Some(user.command(LeTestEnd::new())?);
            user.command(LeTransmitterTestV2::new(0, 37, 0, 1))?;
            report.tx_started = true;
            oer_process::sleep(Duration::from_millis(100))?;
            let count = user.command(LeTestEnd::new())?;
            if count != 0 {
                return Err("transmitter Test End returned a nonzero receiver count".into());
            }
            report.tx_test_end = true;
            Ok(())
        },
        Owner::restore,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_acquisition_failure_restores_the_mutated_owner() {
        let mut unblocked = false;
        let mut report = Check::new(Adapter(0));
        checked_lifetime(
            &mut unblocked,
            &mut report,
            |state, _| {
                *state = true;
                Err("exclusive channel busy".into())
            },
            |state| {
                assert!(*state);
                *state = false;
                Ok(())
            },
        );
        assert!(!unblocked);
        assert!(report.restored);
        assert_eq!(report.errors, ["exclusive channel busy"]);
        assert!(!report.passed(Adapter(0)));
    }
    #[test]
    fn restoration_failure_is_retained_alongside_original_error() {
        let mut report = Check::new(Adapter(0));
        checked_lifetime(
            &mut (),
            &mut report,
            |_, _| Err("RX rejected".into()),
            |_| Err("power restore failed".into()),
        );
        assert!(!report.restored);
        assert_eq!(
            report.errors,
            ["RX rejected", "restore: power restore failed"]
        );
    }
}
