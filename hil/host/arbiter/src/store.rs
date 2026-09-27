//! The arbiter directory and its locked state transactions.

use std::{
    fs,
    path::{Path, PathBuf},
};

use fs2::FileExt as _;

use crate::{
    board::BoardEvent,
    history::{self, LeaseOutcome, LeaseRecord},
    state::{STATE_SCHEMA, State},
};

/// Overrides the per-user arbiter directory, for tests and separate stands.
pub const DIRECTORY_ENV: &str = "OER_HIL_ARBITER_DIR";

/// One stand per host user: every checkout shares this directory, as the
/// fixture leases already do.
#[derive(Clone, Debug)]
pub struct Arbiter {
    directory: PathBuf,
}

impl Arbiter {
    pub fn open() -> crate::Result<Self> {
        if let Some(directory) = std::env::var_os(DIRECTORY_ENV) {
            return Self::at(PathBuf::from(directory));
        }
        let home =
            std::env::var_os("HOME").ok_or("HOME is required to locate the HIL stand arbiter")?;
        Self::at(PathBuf::from(home).join(".cache/open-esp-radio/arbiter"))
    }

    pub fn at(directory: impl Into<PathBuf>) -> crate::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn history_path(&self) -> PathBuf {
        self.directory.join("history.jsonl")
    }

    pub(crate) fn board_path(&self) -> PathBuf {
        self.directory.join("board.jsonl")
    }

    /// Read, reap and possibly modify the state under the exclusive lock.
    /// Leases and tickets of processes that no longer exist are removed first.
    pub(crate) fn transaction<T>(
        &self,
        action: impl FnOnce(&mut State) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.locked(|| self.state_transaction(action))
    }

    /// Run `action` under the exclusive lock of every arbiter file.
    pub(crate) fn locked<T>(&self, action: impl FnOnce() -> crate::Result<T>) -> crate::Result<T> {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.directory.join("arbiter.lock"))?;
        lock.lock_exclusive()?;
        let result = action();
        drop(lock);
        result
    }

    fn state_transaction<T>(
        &self,
        action: impl FnOnce(&mut State) -> crate::Result<T>,
    ) -> crate::Result<T> {
        let path = self.directory.join("state.json");
        let before = match fs::read(&path) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                if value["schema"] != STATE_SCHEMA {
                    return Err(format!(
                        "HIL arbiter state {} has schema {}; this checkout reads schema \
                         {STATE_SCHEMA}. Update the checkout",
                        path.display(),
                        value["schema"]
                    )
                    .into());
                }
                serde_json::from_value(value)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(error.into()),
        };
        let mut state = before.clone();
        self.reap(&mut state)?;
        let result = action(&mut state)?;
        if state != before {
            let temporary = path.with_extension("json.tmp");
            fs::write(&temporary, serde_json::to_vec_pretty(&state)?)?;
            fs::rename(&temporary, &path)?;
        }
        Ok(result)
    }

    fn reap(&self, state: &mut State) -> crate::Result<()> {
        state.queue.retain(|ticket| ticket.process.alive());
        if let Some(holder) = state
            .holder
            .take_if(|holder| !holder.ticket.process.alive())
        {
            history::append(
                &self.history_path(),
                &LeaseRecord {
                    id: holder.ticket.id,
                    owner: holder.ticket.owner.clone(),
                    work: holder.ticket.work.clone(),
                    granted_unix: holder.granted_unix,
                    released_unix: crate::unix_now(),
                    budget_secs: holder.ticket.budget_secs,
                    outcome: LeaseOutcome::Abandoned,
                },
            )?;
            crate::notify::send(
                "HIL stand freed",
                &format!(
                    "lease of {} ({}) ended without release",
                    holder.ticket.owner, holder.ticket.work
                ),
            );
        }
        Ok(())
    }

    pub fn history(&self) -> crate::Result<Vec<LeaseRecord>> {
        history::read(&self.history_path())
    }

    pub fn board_events(&self) -> crate::Result<Vec<BoardEvent>> {
        history::read_lines(&self.board_path())
    }

    /// The newest journaled flash of the board with `mac`.
    pub fn latest_flash(&self, mac: &str) -> crate::Result<Option<BoardEvent>> {
        Ok(self.board_events()?.into_iter().rev().find(|event| {
            event.device.as_deref() == Some(mac)
                && matches!(event.kind, crate::BoardEventKind::Flashed { .. })
        }))
    }

    /// Append a change of the board with MAC `device` (the DUT when unknown),
    /// attributed to the current owner.
    pub fn record_board(
        &self,
        device: Option<String>,
        kind: crate::BoardEventKind,
    ) -> crate::Result<()> {
        self.record_board_by(crate::grant::owner_from_environment(), device, kind)
    }

    /// [`Self::record_board`] attributed to `owner`.
    pub fn record_board_by(
        &self,
        owner: String,
        device: Option<String>,
        kind: crate::BoardEventKind,
    ) -> crate::Result<()> {
        let event = BoardEvent {
            unix: crate::unix_now(),
            owner,
            checkout: std::env::current_dir()
                .ok()
                .map(|directory| directory.display().to_string()),
            device,
            kind,
        };
        let path = self.board_path();
        self.locked(|| {
            history::append_line(&path, &event)?;
            if fs::metadata(&path)?.len() > 1 << 20 {
                history::retain_newest::<BoardEvent>(&path)?;
            }
            Ok(())
        })
    }
}
