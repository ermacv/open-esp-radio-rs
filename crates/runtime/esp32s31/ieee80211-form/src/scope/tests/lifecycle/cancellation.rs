//! Cancel the actual Driver future at each await, using run_port's Retire.
use super::*;

#[derive(Clone, Copy, Debug)]
enum AwaitPoint {
    CoreRequest,
    ExecutorRetirement,
}
#[derive(Clone, Copy, Debug)]
enum AfterCancel {
    Finish,
    Retire,
    Abandon,
}

#[derive(Clone)]
pub(super) struct Cut {
    point: AwaitPoint,
    after: AfterCancel,
    slot: &'static ScopeSlot,
    pub cancel: Arc<AtomicBool>,
    polls: Count,
    finished: Count,
    work: work::Audit,
}
impl Cut {
    pub fn polled(&self, storage: &Storage) {
        self.polls.increment();
        assert_eq!(
            storage.irq().signals.retirement_requested(),
            matches!(self.point, AwaitPoint::ExecutorRetirement),
            "the poll must reach the selected await"
        );
        assert_eq!(self.work.descriptors.get(), 2);
        assert_eq!(self.work.queued.get(), 2);
        assert_eq!(self.work.buffer_drops.get(), 0);
    }

    pub fn cancelled(&self, storage: &Storage, bound: DrainBound) {
        assert_eq!(self.work.descriptors.get(), 2, "cancel keeps DMA ownership");
        assert_eq!(self.work.queued.get(), 2);
        assert_eq!(self.work.buffer_drops.get(), 0);
        let retired = matches!(self.after, AfterCancel::Retire) && self.polls.get() != 0;
        match self.after {
            AfterCancel::Retire if retired => {
                // Same retirement signal and bounded step as run_body uses.
                // No owner is held by the cancelled future; the step accepts
                // the intact backing and accounts for every descriptor.
                storage.irq().signals.retire.store(true, Ordering::Release);
                dispatch(self.slot);
                assert!(storage.irq().retired.load(Ordering::Acquire));
                assert_eq!(self.work.descriptors.get(), 0);
            }
            AfterCancel::Abandon => {
                assert!(matches!(storage.abandon(bound), Quiescence::Drained));
            }
            // Before first poll there is no registration to retire. finish
            // handles the initialized, unstarted owner synchronously.
            _ => {}
        }
    }

    pub fn finished(&self, state: &S31State, storage: &Storage, bound: DrainBound) {
        let retired = matches!(self.after, AfterCancel::Retire) && self.polls.get() != 0;
        match state {
            State::Released(released) => {
                assert!(retired);
                assert!(matches!(&released.owners, DetachedOwners::Retired(_)));
            }
            State::Quarantined(_) => {
                assert!(!retired);
                assert!(matches!(storage.shutdown.get(), Some(Quiescence::Drained)));
            }
        }
        assert!(!self.slot.open.load(Ordering::Acquire));
        assert!(self.slot.entry.load(Ordering::Acquire).is_null());
        assert_eq!(self.work.descriptors.get(), 0, "no live descriptor remains");
        assert_eq!(self.work.queued.get(), 0, "no queued DMA owner remains");
        assert_eq!(self.work.buffer_drops.get(), 2, "each buffer returned once");
        // The core may abandon after finish, e.g. when its outer future drops.
        // The cached shutdown reply must not revisit the extracted regions.
        assert!(matches!(storage.abandon(bound), Quiescence::Drained));
        self.finished.increment();
    }
}

#[test]
fn cancellation_at_every_drive_await_preserves_retire_abandon_and_finish() {
    for point in [AwaitPoint::CoreRequest, AwaitPoint::ExecutorRetirement] {
        for polls in 0..=3 {
            for after in [
                AfterCancel::Finish,
                AfterCancel::Retire,
                AfterCancel::Abandon,
            ] {
                let (route, slot, platform) = make_route();
                let mut hardware = hardware(&platform);
                let (work, audit) = work::DmaQueue::populated();
                hardware.work = work;
                let stops = hardware.stops.clone();
                let drops = hardware.drops.clone();
                let cut = Cut {
                    point,
                    after,
                    slot,
                    cancel: Arc::default(),
                    polls: Count::default(),
                    finished: Count::default(),
                    work: audit,
                };
                let caller = Caller {
                    cancellation: Some(cut.clone()),
                    ..Caller::default()
                };
                caller.end.store(
                    matches!(point, AwaitPoint::ExecutorRetirement),
                    Ordering::Release,
                );
                let context = std::string::String::from("non-static context");
                let mut latch = Latch::new();
                let scope = new_scope(lease(), hardware, Interrupts::default(), route, &mut latch);
                let (installed, body) = installed(scope, &caller, &context);
                let mut run = Box::pin(run_port(installed, body));
                for _ in 0..polls {
                    assert!(poll(run.as_mut()).is_pending());
                }
                assert_eq!(cut.polls.get(), polls, "{point:?}, {after:?}");
                cut.cancel.store(true, Ordering::Release);
                caller.end.store(true, Ordering::Release);
                let Poll::Ready(PortEnd::Uninstalled {
                    uninstalled,
                    output: 7,
                }) = poll(run.as_mut())
                else {
                    panic!("the core hands retained Driver to consuming finish");
                };
                assert_eq!(cut.finished.get(), 1, "{point:?}, {after:?}, {polls}");
                assert_eq!(stops.get(), 1);
                assert!(platform.masked.load(Ordering::Acquire));
                drop(uninstalled);
                drop(run);
                assert_eq!(stops.get(), 1, "outer cancellation must not stop twice");
                assert_eq!(
                    drops.get(),
                    usize::from(matches!(after, AfterCancel::Retire) && polls != 0)
                );
                assert!(latch.into_return().is_none(), "finish recovered explicitly");
            }
        }
    }
}
