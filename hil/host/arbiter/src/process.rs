//! Process identities that survive PID reuse.

use serde::{Deserialize, Serialize};

/// A PID together with its kernel start time. A recycled PID has a different
/// start time, so a stale ticket is never mistaken for a live one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) start_ticks: u64,
}

impl ProcessIdentity {
    pub(crate) fn current() -> crate::Result<Self> {
        Self::of(std::process::id()).ok_or_else(|| "cannot read this process's start time".into())
    }

    pub(crate) fn of(pid: u32) -> Option<Self> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some(Self {
            pid,
            start_ticks: start_ticks(&stat)?,
        })
    }

    pub(crate) fn alive(&self) -> bool {
        Self::of(self.pid).is_some_and(|current| current == *self)
    }
}

/// Field 22 of `/proc/<pid>/stat`. The command name (field 2) is enclosed in
/// parentheses and may itself contain spaces or parentheses.
fn start_ticks(stat: &str) -> Option<u64> {
    let (_, fields) = stat.rsplit_once(')')?;
    // After the command name the next field is field 3 (state).
    fields.split_whitespace().nth(22 - 3)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_time_skips_command_names_with_spaces_and_parentheses() {
        let stat = "42 (a (b) c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 19";
        assert_eq!(start_ticks(stat), Some(777));
        assert_eq!(start_ticks("42 (x) S 1"), None);
    }

    #[test]
    fn a_process_is_alive_until_it_exits() {
        let current = ProcessIdentity::current().unwrap();
        assert!(current.alive());
        let recycled = ProcessIdentity {
            start_ticks: current.start_ticks + 1,
            ..current
        };
        assert!(!recycled.alive());
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let identity = ProcessIdentity::of(child.id());
        child.wait().unwrap();
        assert!(identity.is_none_or(|identity| !identity.alive()));
    }
}
