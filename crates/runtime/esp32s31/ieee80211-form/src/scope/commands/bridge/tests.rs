extern crate std;
mod payload;
use super::*;
use core::{cell::Cell, future::Future, pin::pin, task::Waker};
use oer_ieee80211_lower_mac::{ChannelWidth, LowerMacCapabilities, TxOutcome, WithdrawCause};
use oer_radio_port::{Quarantined, branded};
use std::{boxed::Box, sync::mpsc, thread, time::Instant, vec::Vec};

struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Interface;
    const CAPS: LowerMacCapabilities =
        oer_esp32s31_ieee80211::lower_mac::esp32s31_lower_mac_capabilities(2048);
}
fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture expected a published reply"),
    }
}
fn pending<F: Future>(future: F) {
    assert!(
        pin!(future)
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}
struct Counted<'a>(&'a Cell<usize>);
impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
#[derive(Default)]
struct Hardware {
    operations: Vec<Operation>,
    keys: Vec<[u8; 16]>,
}
impl ModelHardware for Hardware {
    fn apply(&mut self, operation: Operation, key: &[u8; 16]) -> Observation {
        self.operations.push(operation);
        if matches!(
            operation,
            Operation::InstallPairwise(_) | Operation::MutateGroup(_)
        ) {
            self.keys.push(*key);
        }
        Observation(self.operations.len() as u64)
    }
}
type Store<'id, 'context> = Held<
    'id,
    Caps,
    Counted<'context>,
    Counted<'context>,
    Counted<'context>,
    payload::Mpdu,
    2,
    2,
    4,
    15,
>;

#[test]
fn full_setup_and_phase_keep_fifo_whole_non_send_owners_and_three_dma_fragments() {
    let drops = Cell::new(0);
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (mut tx, mut replies) = held.split(r.issuer, r.enable, r.disable);
        tx.configure(Configuration::Station {
            permission: r.station.configure.with_context(Counted(&drops)),
            receive: StationReceive::OtherBssManagement,
        });
        tx.retune(r.retune, Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
        tx.configure(Configuration::AccessPoint {
            permission: r.access_point.configure.with_context(Counted(&drops)),
            receive: SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
        });
        let mut tx = tx.enable();
        // All S cells coexist; setup is serviced before any phase request.
        pending(replies.setup());
        let mut hw = Hardware::default();
        for _ in 0..CommandPools::<2, 2>::SETUP_STORAGE {
            assert!(executor.step(&mut hw).is_some());
        }
        let Applied {
            command:
                SetupCommand::Configure(Configuration::Station {
                    permission: sta, ..
                }),
            ..
        } = applied(replies.setup().await)
        else {
            panic!("station")
        };
        let Applied {
            command: SetupCommand::Retune {
                permission: retune, ..
            },
            ..
        } = applied(replies.setup().await)
        else {
            panic!("retune")
        };
        let Applied {
            command: SetupCommand::Configure(Configuration::AccessPoint { permission: ap, .. }),
            ..
        } = applied(replies.setup().await)
        else {
            panic!("AP")
        };
        let Applied {
            command: SetupCommand::Enable(enable),
            ..
        } = applied(replies.setup().await)
        else {
            panic!("enable")
        };
        tx.close_gate(r.gate);
        executor.step(&mut hw);
        let closing = applied(replies.phase().await);
        let closed = tx.closed(replies.gate_barrier(closing).ok().expect("applied close"));
        let open = tx.open_gate(closed);
        let mut images = Vec::new();
        for (n, (ticket, admission)) in r.tickets.into_iter().zip(r.core.tickets).enumerate() {
            let mut operation = payload::admit(Counted(&drops), ticket, n as u8);
            images.push(payload::Mpdu::project(operation.execute()));
            tx.submit(operation, TxAdmission::Open(admission));
        }
        tx.close_gate(open);
        tx.configure(Configuration::Station {
            permission: sta,
            receive: StationReceive::Off,
        });
        tx.configure(Configuration::AccessPoint {
            permission: ap,
            receive: SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
        });
        tx.sample_tsf(Sampling::Station(r.station.tsf));
        tx.sample_tsf(Sampling::AccessPoint(r.access_point.tsf));
        for permission in r.installs {
            tx.install_pairwise(permission, Ccmp128Key([0xa5; 16]));
        }
        for permission in r.removes {
            tx.remove_pairwise(permission);
        }
        for permission in r.groups {
            tx.mutate_group(permission, Ccmp128Key([0x5a; 16]));
        }
        let tx = tx.disable();
        assert_eq!(drops.get(), 0);
        // All P requests and later all P replies fit, without receiving in between.
        for _ in 0..CommandPools::<2, 2>::PHASE_STORAGE {
            assert!(executor.step(&mut hw).is_some());
        }
        assert!(executor.step(&mut hw).is_none());
        let mut frames = 0;
        let mut terminals = r.terminals;
        for _ in 0..14 {
            let reply = applied(replies.phase().await);
            if let PhaseCommand::Submit(mut submission) = reply.command {
                assert_eq!(
                    payload::Mpdu::project(submission.operation.execute()),
                    images[frames]
                );
                let returned = submission
                    .operation
                    .complete(TxOutcome::Withdrawn(WithdrawCause::GateClosed))
                    .into_parts(&mut terminals);
                assert!(core::ptr::eq(returned.context.0, &drops));
                for (body, lease) in returned.payload.fragments.iter().enumerate() {
                    assert_eq!(lease.ethernet(), &[frames as u8 + body as u8; 32]);
                }
                frames += 1;
            }
        }
        let disable = applied(replies.phase().await);
        let mut tx = tx.finished(
            replies
                .disabled(enable, disable)
                .ok()
                .expect("applied disable"),
        );
        tx.retune(retune, Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap());
        executor.step(&mut hw);
        assert!(matches!(
            applied(replies.setup().await).command,
            SetupCommand::Retune { .. }
        ));
        assert_eq!(frames, 2);
        assert_eq!(hw.keys, [[0xa5; 16], [0xa5; 16], [0x5a; 16], [0x5a; 16]]);
        assert_eq!(
            &hw.operations[5..20],
            &[
                Operation::OpenGate,
                Operation::Transmit(images[0]),
                Operation::Transmit(images[1]),
                Operation::CloseGate,
                Operation::Configure(ReceiveImage::Off),
                Operation::Configure(ReceiveImage::AccessPoint(ReceiveFilter::NONE)),
                Operation::SampleStationTsf,
                Operation::SampleAccessPointTsf,
                Operation::InstallPairwise(0),
                Operation::InstallPairwise(1),
                Operation::RemovePairwise(0),
                Operation::RemovePairwise(1),
                Operation::MutateGroup(0),
                Operation::MutateGroup(1),
                Operation::Disable,
            ]
        );
    }));
    assert_eq!(drops.get(), 4);
}

#[test]
fn permanent_key_cell_is_zero_before_the_reply_becomes_visible() {
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let records = held.phase_lane.records;
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        executor.step(&mut Hardware::default());
        let _enable = applied(replies.setup().await);
        for (n, permission) in r.installs.into_iter().enumerate() {
            let secret = [0x31 + n as u8; 16];
            tx.install_pairwise(permission, Ccmp128Key(secret));
            let mut name = executor.phase.receive.try_receive().unwrap();
            // Pause inside the step, between execution and reply publication.
            records.with_mut(&mut name, |record| {
                assert_eq!(record.key, secret);
                let mut hw = Hardware::default();
                record.execute(&mut hw);
                assert_eq!(hw.keys, [secret]);
                assert_eq!(record.key, [0; 16]);
            });
            pending(replies.phase());
            executor.phase.reply.send(name);
            let Applied {
                command: PhaseCommand::InstallPairwise { key, .. },
                ..
            } = applied(replies.phase().await)
            else {
                panic!("key")
            };
            assert_eq!(key.0, [0; 16]);
        }
    }));
}

#[test]
fn repeated_response_names_reuse_only_their_original_installation() {
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        let mut hardware = Hardware::default();
        executor.step(&mut hardware);
        let _enable = applied(replies.setup().await);
        let mut tsf = r.station.tsf;
        for n in 0..128 {
            tx.sample_tsf(Sampling::Station(tsf));
            pending(replies.phase());
            executor.step(&mut hardware);
            let Applied {
                command: PhaseCommand::SampleTsf(Sampling::Station(returned)),
                observation,
            } = applied(replies.phase().await)
            else {
                panic!("tsf")
            };
            assert_eq!(observation, Observation(n + 2));
            tsf = returned;
        }
    }));
}

#[test]
fn cancelled_reply_waits_at_each_handoff_leave_finish_all_owners_and_empty_queues() {
    // Before request receive, inside the finite step, and after reply publish.
    // Reply waits are the bridge's only await; publication itself has none.
    for handoff in 0..3 {
        for polls in 0..=3 {
            let drops = Cell::new(0);
            ready(branded::<CommandPools<2, 2>, _>(async |mint| {
                let r = mint.into_pools();
                let (mut held, mut executor) =
                    Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
                let records = held.phase_lane.records;
                let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
                let mut tx = setup.enable();
                // Cancellation of the setup await, also before its first poll.
                let mut wait = Box::pin(replies.setup());
                for _ in 0..polls {
                    assert!(
                        wait.as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_pending()
                    );
                }
                drop(wait);
                executor.step(&mut Hardware::default());
                let Applied {
                    command: SetupCommand::Enable(enable),
                    ..
                } = applied(replies.setup().await)
                else {
                    panic!("enable")
                };
                let [ticket, _] = r.tickets;
                let [admission, _] = r.core.tickets;
                let (operation, pool) = payload::admit_with_pool(Counted(&drops), ticket, 0);
                tx.submit(operation, TxAdmission::Open(admission));
                if handoff == 0 {
                    let mut wait = Box::pin(replies.phase());
                    for _ in 0..polls {
                        assert!(
                            wait.as_mut()
                                .poll(&mut Context::from_waker(Waker::noop()))
                                .is_pending()
                        );
                    }
                    drop(wait);
                    assert_eq!(pool.claimed_slots(), MODEL_FRAGMENTS);
                    executor.step(&mut Hardware::default());
                } else {
                    let mut name = executor.phase.receive.try_receive().unwrap();
                    let mut wait = Box::pin(replies.phase());
                    for _ in 0..polls {
                        assert!(
                            wait.as_mut()
                                .poll(&mut Context::from_waker(Waker::noop()))
                                .is_pending()
                        );
                    }
                    if handoff == 1 {
                        drop(wait);
                    } else {
                        records
                            .with_mut(&mut name, |record| record.execute(&mut Hardware::default()));
                        executor.phase.reply.send(name);
                        // Ready-but-unpolled future still owns no reply.
                        drop(wait);
                        let Applied {
                            command: PhaseCommand::Submit(submission),
                            ..
                        } = applied(replies.phase().await)
                        else {
                            panic!("submit")
                        };
                        assert_eq!(pool.claimed_slots(), MODEL_FRAGMENTS);
                        drop(submission);
                        assert_eq!(pool.claimed_slots(), 0);
                        assert_eq!(drops.get(), 1);
                        let _disable_sender = tx.disable();
                        executor.step(&mut Hardware::default());
                        let disable = applied(replies.phase().await);
                        let _reply = replies
                            .disabled(enable, disable)
                            .ok()
                            .expect("applied disable");
                        assert!(executor.phase.receive.try_receive().is_err());
                        pending(replies.phase());
                        return;
                    }
                    records.with_mut(&mut name, |record| record.execute(&mut Hardware::default()));
                    executor.phase.reply.send(name);
                }
                assert_eq!(drops.get(), 0);
                let Applied {
                    command: PhaseCommand::Submit(submission),
                    ..
                } = applied(replies.phase().await)
                else {
                    panic!("submit")
                };
                drop(submission);
                let _disable_sender = tx.disable();
                executor.step(&mut Hardware::default());
                let disable = applied(replies.phase().await);
                let _reply = replies
                    .disabled(enable, disable)
                    .ok()
                    .expect("applied disable");
                assert!(executor.phase.receive.try_receive().is_err());
                pending(replies.phase());
                assert_eq!(pool.claimed_slots(), 0);
            }));
            assert_eq!(drops.get(), 1);
        }
    }
}

struct PausedHardware {
    entered: mpsc::Sender<()>,
    proceed: mpsc::Receiver<()>,
    seen: Option<TxImage>,
}
impl ModelHardware for PausedHardware {
    fn apply(&mut self, operation: Operation, _: &[u8; 16]) -> Observation {
        if let Operation::Transmit(image) = operation {
            self.seen = Some(image);
            self.entered.send(()).unwrap();
            self.proceed.recv().unwrap();
        }
        Observation(1)
    }
}

#[test]
#[allow(
    clippy::drop_non_drop,
    reason = "end endpoint loans and destroy the executor container explicitly"
)]
fn second_core_step_survives_forget_or_quarantine_without_context_or_dma_drop() {
    for quarantine in [false, true] {
        let drops = Cell::new(0);
        ready(branded::<CommandPools<2, 2>, _>(async |mint| {
            let r = mint.into_pools();
            let (mut held, mut executor) =
                Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
            let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
            let mut tx = setup.enable();
            executor.step(&mut Hardware::default());
            let _enable = applied(replies.setup().await);
            let [ticket, _] = r.tickets;
            let [admission, _] = r.core.tickets;
            let (mut operation, pool) = payload::admit_with_pool(Counted(&drops), ticket, 3);
            let image = payload::Mpdu::project(operation.execute());
            tx.submit(operation, TxAdmission::Open(admission));
            let (entered_tx, entered_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                let mut hardware = PausedHardware {
                    entered: entered_tx,
                    proceed: resume_rx,
                    seen: None,
                };
                executor.step(&mut hardware);
                assert_eq!(hardware.seen, Some(image));
                // No path back to Held; publishing this reply remains valid
                // after Held's allocation or the caller's future disappears.
                executor
            });
            entered_rx.recv().unwrap();
            let mut wait = Box::pin(replies.phase());
            assert!(
                wait.as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            core::mem::forget(wait);
            drop(tx);
            drop(replies);
            if quarantine {
                drop(Quarantined::new(held));
            } else {
                core::mem::forget(held);
            }
            assert_eq!(drops.get(), 0);
            assert_eq!(pool.claimed_slots(), MODEL_FRAGMENTS);
            resume_tx.send(()).unwrap();
            let executor = worker.join().unwrap();
            drop(executor);
            assert_eq!(drops.get(), 0);
            assert_eq!(pool.claimed_slots(), MODEL_FRAGMENTS);
        }));
        assert_eq!(drops.get(), 0);
    }
}

#[test]
fn cancellation_during_another_cores_step_returns_original_owner_after_join() {
    let drops = Cell::new(0);
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        executor.step(&mut Hardware::default());
        let _enable = applied(replies.setup().await);
        let [ticket, _] = r.tickets;
        let [admission, _] = r.core.tickets;
        let (operation, pool) = payload::admit_with_pool(Counted(&drops), ticket, 8);
        tx.submit(operation, TxAdmission::Open(admission));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            executor.step(&mut PausedHardware {
                entered: entered_tx,
                proceed: resume_rx,
                seen: None,
            });
            executor
        });
        entered_rx.recv().unwrap();
        pending(replies.phase()); // drops the pending future during the step
        assert_eq!(drops.get(), 0);
        assert_eq!(pool.claimed_slots(), MODEL_FRAGMENTS);
        resume_tx.send(()).unwrap();
        let _executor = worker.join().unwrap();
        let Applied {
            command: PhaseCommand::Submit(submission),
            ..
        } = applied(replies.phase().await)
        else {
            panic!("submit")
        };
        drop(submission);
        assert_eq!(pool.claimed_slots(), 0);
    }));
    assert_eq!(drops.get(), 1);
}

#[test]
fn command_to_step_host_latency_fixture() {
    // Host scheduling observation only: never a production DrainBound or
    // retune deadline. Includes direct projection and inter-thread scheduling.
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        executor.step(&mut Hardware::default());
        let _enable = applied(replies.setup().await);
        struct WakeThread(thread::Thread);
        impl std::task::Wake for WakeThread {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let (stamp_tx, stamp_rx) = mpsc::channel::<Instant>();
        let (time_tx, time_rx) = mpsc::channel();
        let (parked_tx, parked_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            struct Sample {
                stamps: mpsc::Receiver<Instant>,
                times: mpsc::Sender<std::time::Duration>,
            }
            impl ModelHardware for Sample {
                fn apply(&mut self, _: Operation, _: &[u8; 16]) -> Observation {
                    self.times
                        .send(self.stamps.recv().unwrap().elapsed())
                        .unwrap();
                    Observation(0)
                }
            }
            let waker = Waker::from(std::sync::Arc::new(WakeThread(thread::current())));
            let mut cx = Context::from_waker(&waker);
            let mut hardware = Sample {
                stamps: stamp_rx,
                times: time_tx,
            };
            assert!(executor.poll_step(&mut cx, &mut hardware).is_pending());
            parked_tx.send(()).unwrap();
            for _ in 0..256 {
                while executor.poll_step(&mut cx, &mut hardware).is_pending() {
                    thread::park();
                }
            }
        });
        parked_rx.recv().unwrap();
        let caller_waker = Waker::from(std::sync::Arc::new(WakeThread(thread::current())));
        let mut tsf = r.station.tsf;
        let mut samples = Vec::new();
        for _ in 0..256 {
            let sent = Instant::now();
            // Instrumentation only. The actual wake is DirectSender's waker,
            // registered by poll_step, not a second command/kick channel.
            stamp_tx.send(sent).unwrap();
            tx.sample_tsf(Sampling::Station(tsf));
            samples.push(time_rx.recv().unwrap());
            let reply = loop {
                if let Poll::Ready(reply) =
                    replies.phase.poll(&mut Context::from_waker(&caller_waker))
                {
                    break reply;
                }
                thread::park();
            };
            let PhaseCommand::SampleTsf(Sampling::Station(returned)) = applied(reply).command
            else {
                panic!("tsf")
            };
            tsf = returned;
        }
        worker.join().unwrap();
        samples.sort();
        std::eprintln!(
            "host command->step fixture n={} p50={:?} p95={:?} max={:?}; includes OS scheduling, not S31 bounds",
            samples.len(),
            samples[128],
            samples[243],
            samples[255]
        );
        assert_eq!(samples.len(), 256);
        std::eprintln!(
            "host bridge sizes: backing={} Held={} Executor={} bytes",
            core::mem::size_of::<ExecutionStorage<4, 15>>(),
            core::mem::size_of::<Store<'_, '_>>(),
            core::mem::size_of::<Executor<4, 15>>()
        );
    }));
}

#[test]
fn identical_layouts_keep_private_names_in_their_original_installation() {
    ready(branded::<CommandPools<2, 2>, _>(async |first| {
        ready(branded::<CommandPools<2, 2>, _>(async |second| {
            let a = first.into_pools();
            let b = second.into_pools();
            let (mut first, mut first_step) =
                Store::bind(a.bridge, Box::leak(Box::new(ExecutionStorage::new())));
            let (mut second, mut second_step) =
                Store::bind(b.bridge, Box::leak(Box::new(ExecutionStorage::new())));
            let (a_tx, mut a_rx) = first.split(a.issuer, a.enable, a.disable);
            let (b_tx, mut b_rx) = second.split(b.issuer, b.enable, b.disable);
            let mut a_tx = a_tx.enable();
            let mut b_tx = b_tx.enable();
            let mut hardware = Hardware::default();
            first_step.step(&mut hardware);
            second_step.step(&mut hardware);
            let _a_enable = applied(a_rx.setup().await);
            let _b_enable = applied(b_rx.setup().await);
            a_tx.sample_tsf(Sampling::Station(a.station.tsf));
            b_tx.sample_tsf(Sampling::Station(b.station.tsf));
            first_step.step(&mut hardware);
            pending(b_rx.phase());
            assert_eq!(applied(a_rx.phase().await).observation, Observation(3));
            second_step.step(&mut hardware);
            assert_eq!(applied(b_rx.phase().await).observation, Observation(4));
            pending(a_rx.phase());
        }));
    }));
}

#[test]
fn command_acquire_precedes_desired_settings_and_ba_reads_on_another_core() {
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    struct Desired {
        settings: AtomicU32,
        ba: AtomicU32,
    }
    struct ReadDesired(Arc<Desired>);
    impl ModelHardware for ReadDesired {
        fn apply(&mut self, operation: Operation, _: &[u8; 16]) -> Observation {
            if matches!(operation, Operation::SampleStationTsf) {
                // Relaxed data reads rely on the command's Release/Acquire,
                // not on the test's separate worker-start synchronization.
                assert_eq!(self.0.settings.load(Ordering::Relaxed), 17);
                assert_eq!(self.0.ba.load(Ordering::Relaxed), 29);
            }
            Observation(0)
        }
    }
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        executor.step(&mut Hardware::default());
        let _enable = applied(replies.setup().await);
        let desired = Arc::new(Desired {
            settings: AtomicU32::new(0),
            ba: AtomicU32::new(0),
        });
        let mut hardware = ReadDesired(desired.clone());
        let worker = thread::spawn(move || {
            loop {
                if executor.step(&mut hardware).is_some() {
                    return executor;
                }
                thread::yield_now();
            }
        });
        desired.settings.store(17, Ordering::Relaxed);
        desired.ba.store(29, Ordering::Relaxed);
        tx.sample_tsf(Sampling::Station(r.station.tsf));
        let _executor = worker.join().unwrap();
        assert!(matches!(
            applied(replies.phase().await).command,
            PhaseCommand::SampleTsf(_)
        ));
    }));
}

#[test]
fn finish_after_cancel_withdraws_pending_commands_and_returns_every_dma_lease() {
    for completed in 0..=2 {
        let drops = Cell::new(0);
        ready(branded::<CommandPools<2, 2>, _>(async |mint| {
            let r = mint.into_pools();
            let (mut held, mut executor) =
                Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
            let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
            let mut tx = setup.enable();
            let mut hardware = Hardware::default();
            executor.step(&mut hardware);
            let _enable = replies.setup().await;
            let mut pools = Vec::new();
            for (n, (ticket, admission)) in r.tickets.into_iter().zip(r.core.tickets).enumerate() {
                let (operation, pool) = payload::admit_with_pool(Counted(&drops), ticket, n as u8);
                tx.submit(operation, TxAdmission::Open(admission));
                pools.push(pool);
            }
            for _ in 0..completed {
                executor.step(&mut hardware);
            }
            // Cancel even a ready-but-unpolled reply wait; no accepted owner
            // lives in it. Freeze publication before the consuming model finish.
            let wait = Box::pin(replies.phase());
            drop(wait);
            drop(tx);
            executor.withdraw_pending();
            for n in 0..2 {
                let reply = replies.phase().await;
                if n < completed {
                    assert!(matches!(reply.completion(), Completion::Applied(_)));
                } else {
                    assert_eq!(reply.completion(), Completion::Withdrawn);
                }
                assert!(matches!(reply.command(), PhaseCommand::Submit(_)));
                drop(reply);
            }
            pending(replies.phase());
            assert_eq!(hardware.operations.len(), 1 + completed);
            for pool in pools {
                assert_eq!(pool.claimed_slots(), 0);
            }
        }));
        assert_eq!(drops.get(), 2);
    }
}

fn applied<T>(reply: Reply<T>) -> Applied<T> {
    match reply {
        Reply::Applied(reply) => reply,
        Reply::Withdrawn(_) => panic!("fixture expected an applied step"),
    }
}

#[test]
fn withdrawn_close_and_disable_never_produce_applied_receipts() {
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let (mut held, mut executor) =
            Store::bind(r.bridge, Box::leak(Box::new(ExecutionStorage::new())));
        let (setup, mut replies) = held.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        let mut hardware = Hardware::default();
        executor.step(&mut hardware);
        let _enable = applied(replies.setup().await);
        tx.close_gate(r.gate);
        let _frozen = tx.disable();
        executor.withdraw_pending();
        assert!(matches!(
            replies.phase().await,
            Reply::Withdrawn(PhaseCommand::CloseGate(_))
        ));
        assert!(matches!(
            replies.phase().await,
            Reply::Withdrawn(PhaseCommand::Disable(_))
        ));
        assert_eq!(hardware.operations, [Operation::Enable]);
        pending(replies.phase());
    }));
}
