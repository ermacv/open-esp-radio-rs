//! Completed leases, the input of budget estimates and the status report.

use std::{fs, io::Write, path::Path};

use serde::{Deserialize, Serialize};

/// A history larger than this keeps only its newest records.
const MAX_BYTES: u64 = 1 << 20;
const RETAINED_RECORDS: usize = 2000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeaseOutcome {
    Released,
    /// The holder was terminated at twice its budget.
    BudgetExceeded,
    /// The holder exited without releasing; another process reaped it.
    Abandoned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub granted_unix: u64,
    pub released_unix: u64,
    pub budget_secs: u64,
    pub outcome: LeaseOutcome,
}

impl LeaseRecord {
    pub fn duration_secs(&self) -> u64 {
        self.released_unix.saturating_sub(self.granted_unix)
    }
}

/// Append a record. Callers hold the state lock, which also serializes the
/// rewrite that bounds the file.
pub(crate) fn append(path: &Path, record: &LeaseRecord) -> crate::Result<()> {
    append_line(path, record)?;
    if fs::metadata(path)?.len() > MAX_BYTES {
        retain_newest::<LeaseRecord>(path)?;
    }
    Ok(())
}

/// Records in append order; lines another version cannot parse are skipped.
pub(crate) fn read(path: &Path) -> crate::Result<Vec<LeaseRecord>> {
    read_lines(path)
}

pub(crate) fn append_line(path: &Path, value: &impl Serialize) -> crate::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)?;
    Ok(())
}

pub(crate) fn read_lines<T: for<'a> Deserialize<'a>>(path: &Path) -> crate::Result<Vec<T>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn retain_newest<T: for<'a> Deserialize<'a> + Serialize>(
    path: &Path,
) -> crate::Result<()> {
    let records = read_lines::<T>(path)?;
    let start = records.len().saturating_sub(RETAINED_RECORDS);
    let mut text = Vec::new();
    for record in &records[start..] {
        text.extend(serde_json::to_vec(record)?);
        text.push(b'\n');
    }
    let temporary = path.with_extension("jsonl.tmp");
    fs::write(&temporary, text)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_skips_unreadable_lines_and_stays_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        assert!(read(&path).unwrap().is_empty());
        let record = LeaseRecord {
            id: 1,
            owner: "phy".into(),
            work: "run x".into(),
            granted_unix: 10,
            released_unix: 70,
            budget_secs: 60,
            outcome: LeaseOutcome::Released,
        };
        append(&path, &record).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"future\":true}\n")
            .unwrap();
        append(&path, &record).unwrap();
        assert_eq!(read(&path).unwrap(), [record.clone(), record.clone()]);
        for _ in 0..RETAINED_RECORDS + 10 {
            append_line(&path, &record).unwrap();
        }
        retain_newest::<LeaseRecord>(&path).unwrap();
        assert_eq!(read(&path).unwrap().len(), RETAINED_RECORDS);
    }
}
