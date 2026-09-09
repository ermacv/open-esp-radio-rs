use super::*;
use core::task::{Context, Poll, Waker};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_embassy_net::OwnedEndpointResources;
use std::boxed::Box;
use xarxa_driver::{PacketPool, PacketPoolStorage};

struct Services<'a> {
    completion: &'a Signal<NoopRawMutex, ()>,
    prepared: Option<Box<[u8; 14]>>,
    completions: usize,
    fail_completion: bool,
    stops: usize,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for Services<'_>
{
    type Error = ();
    type Exit = ();

    async fn service_rx(
        &mut self,
        _: &mut dyn DatapathNetworkRxSet,
        _: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, ()> {
        Ok(DatapathRxProgress::Drained)
    }

    async fn start_tx<'a, I>(&'a mut self, _: S, _: &'a I) -> Result<WifiTxProgress, ()>
    where
        S: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P> + 'a,
    {
        panic!("a requested boundary must not admit TX")
    }

    async fn wait_tx_deadline(&mut self) {
        self.completion.wait().await;
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        self.completions += 1;
        if self.fail_completion {
            Err(())
        } else {
            Ok(WifiTxProgress::Complete)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.prepared.is_some()
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        assert_eq!(*self.prepared.take().expect("retained frame"), [0x42; 14]);
        self.completion.signal(());
        Ok(WifiTxProgress::Complete)
    }

    fn cancel_prepared_tx<I>(&mut self, _: &I) -> Result<(), ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        self.prepared.take();
        Ok(())
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, ()> {
        self.stops += 1;
        Ok(DatapathStopProgress::Stopped)
    }
}

fn exercise_pause(active: bool, fail_completion: bool) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = network::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let prepared = Box::new([0x42; 14]);
    let original = prepared.as_ptr();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        Services {
            completion: &completion,
            prepared: Some(prepared),
            completions: 0,
            fail_completion,
            stops: 0,
        },
    );
    runner.prepared_tx_interface = Some(interface);
    let batch_deadline = Instant::now() + Duration::from_secs(1);
    runner.tx_batch_states[0].collection_deadline = Some(batch_deadline);
    if active {
        runner.begin_active_tx(interface, DatapathTxOrigin::Network);
    }
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut pause = core::pin::pin!(runner.run_until_pause(ready(())));
        if active {
            assert!(pause.as_mut().poll(&mut cx).is_pending());
            // Repoll without an event cannot complete the active owner.
            assert!(pause.as_mut().poll(&mut cx).is_pending());
            completion.signal(());
        }
        let expected = if fail_completion {
            Err(())
        } else {
            Ok(DatapathPauseExit::Paused)
        };
        assert_eq!(pause.as_mut().poll(&mut cx), Poll::Ready(expected));
    }
    assert!(device.link_is_up());
    // Type-changing owner transfer must not reconstruct scheduler state.
    let failed = runner
        .try_map_services(Err::<(), _>)
        .err()
        .expect("failure retains runner");
    runner = failed
        .try_map_services(Ok::<_, ()>)
        .unwrap_or_else(|_| panic!("successful restoration"));
    assert_eq!(runner.services.stops, 0);
    assert_eq!(
        runner.tx_batch_states[0].collection_deadline,
        Some(batch_deadline)
    );
    assert_eq!(
        runner.services.prepared.as_ref().unwrap().as_ptr(),
        original
    );
    assert_eq!(runner.prepared_tx_interface, Some(interface));
    assert_eq!(runner.services.completions, usize::from(active));
    assert_eq!(
        runner.active_tx_interface,
        fail_completion.then_some(interface)
    );

    // Recovery/resumption uses the very same owner. A second pause still must
    // preserve it, and an explicit terminal stop must retain its old semantics.
    runner.services.fail_completion = false;
    if fail_completion {
        completion.signal(());
    }
    {
        let mut pause = core::pin::pin!(runner.run_until_pause(ready(())));
        assert_eq!(
            pause.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathPauseExit::Paused))
        );
    }
    assert_eq!(
        runner.services.prepared.as_ref().unwrap().as_ptr(),
        original
    );
    assert!(device.link_is_up());
    if active {
        // Resume the normal scheduler: the original prepared frame is sent,
        // and its completion notification requests the next pause.
        let mut resumed = core::pin::pin!(runner.run_until_pause(completion.wait()));
        assert_eq!(
            resumed.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathPauseExit::Paused))
        );
    }
    assert!(device.link_is_up());
    assert_eq!(runner.services.prepared.is_none(), active);
    {
        let mut stop = core::pin::pin!(runner.run_until(ready(())));
        assert_eq!(
            stop.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathRunnerExit::Stopped))
        );
    }
    assert!(!device.link_is_up());
    assert!(runner.services.prepared.is_none());
    assert_eq!(runner.services.stops, 1);
}

#[test]
fn pause_retains_prepared_owner_and_link_until_explicit_stop() {
    exercise_pause(false, false);
}

#[test]
fn pause_waits_for_active_tx_event_before_returning() {
    exercise_pause(true, false);
}

#[test]
fn failed_pause_retains_active_tx_for_recovery() {
    exercise_pause(true, true);
}

#[test]
fn controlled_stop_upgrades_pause_during_tx_drain_or_while_paused() {
    for stop_during_drain in [true, false] {
        let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
        let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
        let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
        let interface = NetworkInterfaceId::new(0);
        let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
        let resources = Box::leak(Box::new(
            PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
        ));
        let pool =
            PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
        let network = network::OwnedDatapathNetwork::new(owned, resources.split(pool));
        network.set_link_state(interface, LinkState::Up);
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let completion = Signal::new();
        let mut runner = DatapathRunner::new(
            &irq,
            network,
            interface,
            Services {
                completion: &completion,
                prepared: Some(Box::new([0x42; 14])),
                completions: 0,
                fail_completion: false,
                stops: 0,
            },
        );
        runner.prepared_tx_interface = Some(interface);
        runner.begin_active_tx(interface, DatapathTxOrigin::Network);
        let control = execution::Control::<NoopRawMutex>::new();
        control.request_pause();
        let mut cx = Context::from_waker(Waker::noop());
        {
            let mut run = core::pin::pin!(runner.run_controlled(&control));
            assert!(run.as_mut().poll(&mut cx).is_pending());
            if stop_during_drain {
                control.request_stop();
                control.request_pause();
                assert!(
                    run.as_mut().poll(&mut cx).is_pending(),
                    "stop cannot drop in-flight TX"
                );
            }
            completion.signal(());
            assert_eq!(
                run.as_mut().poll(&mut cx),
                Poll::Ready(Ok(if stop_during_drain {
                    execution::Exit::Stopped
                } else {
                    execution::Exit::Paused
                }))
            );
        }
        assert_eq!(runner.services.completions, 1);
        if !stop_during_drain {
            assert!(device.link_is_up());
            assert!(runner.services.prepared.is_some());
            control.request_stop();
            control.request_pause();
            let mut run = core::pin::pin!(runner.run_controlled(&control));
            assert_eq!(
                run.as_mut().poll(&mut cx),
                Poll::Ready(Ok(execution::Exit::Stopped))
            );
        }
        assert!(control.stop_requested());
        assert!(!device.link_is_up());
        assert!(runner.services.prepared.is_none());
        assert_eq!(runner.services.stops, 1);
    }
}
