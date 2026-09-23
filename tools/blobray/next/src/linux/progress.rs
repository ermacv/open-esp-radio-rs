//! Host clock, signal token and bounded worker checkpoint publication.
use super::*;
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub fn now_ms() -> u64 {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: valid output pointer; CLOCK_MONOTONIC is supported on this Linux host.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, time.as_mut_ptr()) };
    assert_eq!(result, 0, "CLOCK_MONOTONIC unavailable");
    // SAFETY: successful clock_gettime initialized both fields.
    let time = unsafe { time.assume_init() };
    time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000
}

pub(super) fn save(stage: &Path, record: &ProgressRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).map_err(|e| unavailable(e.to_string()))?;
    if bytes.len() > 65536 {
        return Err(unavailable("progress exceeds 64 KiB"));
    }
    let temporary = stage.join("progress.tmp");
    fs::write(&temporary, bytes).map_err(diagnostic_io)?;
    fs::rename(temporary, stage.join("progress.json")).map_err(diagnostic_io)
}

fn diagnostic_io(error: std::io::Error) -> Error {
    let mut error = storage_io(error);
    if error.code != ErrorCode::DiskFull {
        error.code = ErrorCode::DiagnosticChannel;
    }
    error
}

/// Used only by the private worker entry point, never by an embedded API caller.
pub struct WorkerEnvironment {
    stage: PathBuf,
    run: RunId,
    cancelled: Arc<AtomicBool>,
    signals: Vec<signal_hook::SigId>,
    last_write: Cell<Option<u64>>,
}
impl WorkerEnvironment {
    pub fn new(stage: &Path, run: RunId) -> Result<Self> {
        let mut result = Self {
            stage: stage.to_owned(),
            run,
            cancelled: Arc::new(AtomicBool::new(false)),
            signals: Vec::new(),
            last_write: Cell::new(None),
        };
        for signal in [libc::SIGTERM, libc::SIGINT] {
            result
                .signals
                .push(signal_hook::flag::register(signal, result.cancelled.clone()).map_err(io)?);
        }
        Ok(result)
    }
    pub fn finish(&self, progress: &RunProgress) -> Result<()> {
        save(
            &self.stage,
            &ProgressRecord {
                schema: 1,
                run: self.run.clone(),
                progress: *progress,
            },
        )
    }
}
impl RunEnvironment for WorkerEnvironment {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    fn observe(&self, progress: &RunProgress) -> Result<()> {
        let now = now_ms();
        if self
            .last_write
            .get()
            .is_none_or(|previous| now.saturating_sub(previous) >= 100)
        {
            self.finish(progress)?;
            self.last_write.set(Some(now));
        }
        Ok(())
    }
}
impl Drop for WorkerEnvironment {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
    }
}
