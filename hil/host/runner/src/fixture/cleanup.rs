//! Per-repetition cleanup evidence, including restoration from destructors.

use crate::Result;
use std::{
    cell::RefCell,
    marker::PhantomData,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, serde::Serialize)]
pub(crate) struct Record {
    operation: &'static str,
    duration_millis: u128,
    pub(crate) failure: Option<String>,
}
thread_local! { static RECORDS: RefCell<Option<Vec<Record>>> = const { RefCell::new(None) }; }
thread_local! { static QUARANTINE: RefCell<Option<String>> = const { RefCell::new(None) }; }

pub(crate) fn require_healthy() -> Result<()> {
    QUARANTINE.with_borrow(|failure| match failure {
        Some(failure) => Err(super::Error::new(format!(
            "fixture quarantined after failed restoration: {failure}"
        ))
        .into()),
        None => Ok(()),
    })
}

pub(crate) struct Scope {
    output: PathBuf,
    previous: Option<Vec<Record>>,
    finished: bool,
    _thread: PhantomData<Rc<()>>,
}
impl Scope {
    pub(crate) fn new(output: &Path) -> Self {
        Self {
            output: output.join("cleanup.json"),
            previous: RECORDS.replace(Some(Vec::new())),
            finished: false,
            _thread: PhantomData,
        }
    }
    pub(crate) fn finish(mut self) -> Result<Vec<Record>> {
        let records = RECORDS.replace(self.previous.take()).unwrap_or_default();
        self.finished = true;
        crate::evidence::run::atomic_json(&self.output, &records)?;
        Ok(records)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if !self.finished {
            let records = RECORDS.replace(self.previous.take()).unwrap_or_default();
            if let Err(error) = crate::evidence::run::atomic_json(&self.output, &records) {
                eprintln!("cannot preserve fixture cleanup evidence: {error}");
            }
        }
    }
}

pub(crate) fn record(operation: &'static str, restore: impl FnOnce() -> Result<()>) {
    let start = std::time::Instant::now();
    let failure = oer_process::cleanup(restore)
        .err()
        .map(|error| error.to_string());
    if let Some(error) = &failure {
        eprintln!("fixture cleanup failed ({operation}): {error}");
        QUARANTINE.with_borrow_mut(|failure| {
            failure.get_or_insert_with(|| format!("{operation}: {error}"));
        });
    }
    RECORDS.with_borrow_mut(|records| {
        if let Some(records) = records {
            records.push(Record {
                operation,
                duration_millis: start.elapsed().as_millis(),
                failure,
            });
        }
    });
}

pub(crate) fn command(operation: &'static str, command: &mut std::process::Command) {
    use oer_process::CommandExt as _;
    record(operation, || {
        let status = command.supervised_status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{operation} failed with {status}").into())
        }
    });
}

#[cfg(test)]
mod tests;
