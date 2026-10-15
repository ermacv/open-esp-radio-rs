//! Installation-local admission; emergency entry never takes the ISR mutex.

use core::sync::atomic::{AtomicUsize, Ordering};

use oer_ieee802154::RadioFault;
use oer_radio_port::Poisoned;

const CLOSED: usize = 1 << (usize::BITS - 1);

/// Open implementation policy, measured in finite CPU observations, not
/// microseconds or a vendor guarantee about ISR latency. Expiry leaves all
/// backing in permanent quarantine. HIL must measure the actual duration.
pub(crate) const JOIN_OBSERVATIONS: u32 = 10_000;

/// Admission and the number of admitted ISR/task entries share one atomic
/// word: closing cannot miss a concurrently admitted entry. The gate never
/// reopens after emergency closure, including across attempted reinstall.
pub(crate) struct Admission {
    entries: AtomicUsize,
}

impl Admission {
    pub(crate) const fn new() -> Self {
        Self {
            entries: AtomicUsize::new(0),
        }
    }

    pub(crate) fn enter(&self) -> Result<Entry<'_>, Poisoned<RadioFault>> {
        self.entries
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                (word < CLOSED - 1).then(|| word + 1)
            })
            .map(|_| Entry { admission: self })
            .map_err(|_| Poisoned {
                cause: RadioFault::StateLost,
            })
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.entries.load(Ordering::Acquire) & CLOSED != 0
    }

    /// Close before masking the route; no mutex, wake or hardware access.
    pub(crate) fn close(&self) -> Closed<'_> {
        self.entries.fetch_or(CLOSED, Ordering::AcqRel);
        Closed { admission: self }
    }
}

#[must_use = "retain admission through all ISR/task accesses and publications"]
pub(crate) struct Entry<'a> {
    admission: &'a Admission,
}

impl Drop for Entry<'_> {
    fn drop(&mut self) {
        self.admission.entries.fetch_sub(1, Ordering::Release);
    }
}

#[must_use = "mask the bound route before joining admitted entries"]
pub(crate) struct Closed<'a> {
    admission: &'a Admission,
}

impl Closed<'_> {
    /// CPU exclusion alone never grants device/DMA reclamation permission.
    pub(crate) fn join(self, observations: u32) -> bool {
        for _ in 0..observations {
            if self.admission.entries.load(Ordering::Acquire) == CLOSED {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn entered_isr_on_another_core_cannot_be_missed_or_joined_early() {
        let gate = Arc::new(Admission::new());
        let entered = Arc::new(Barrier::new(2));
        let leave = Arc::new(Barrier::new(2));
        let isr = {
            let (gate, entered, leave) = (gate.clone(), entered.clone(), leave.clone());
            std::thread::spawn(move || {
                let _entry = gate.enter().unwrap();
                entered.wait();
                leave.wait();
            })
        };
        entered.wait();
        assert!(!gate.close().join(3));
        assert!(gate.enter().is_err());
        leave.wait();
        isr.join().unwrap();
        assert!(gate.close().join(1));
        assert!(gate.enter().is_err());
    }

    #[test]
    fn forgotten_entry_and_zero_budget_fail_closed() {
        let gate = Admission::new();
        core::mem::forget(gate.enter().unwrap());
        assert!(!gate.close().join(JOIN_OBSERVATIONS));
        assert!(gate.enter().is_err());
        assert!(!Admission::new().close().join(0));
    }
}
