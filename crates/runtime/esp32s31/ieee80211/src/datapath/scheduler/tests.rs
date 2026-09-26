use super::*;
use core::task::{Context, Poll, Waker};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_embassy_net_owned::OwnedEndpointResources;
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

fn exercise_stop(active: bool, fail_completion: bool) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
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
            fail_completion,
            stops: 0,
        },
    );
    runner.prepared_tx_interface = Some(interface);
    if active {
        runner.begin_active_tx(interface, DatapathTxOrigin::Network);
    }
    let control = execution::Control::<NoopRawMutex>::new();
    control.request_stop();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut run = core::pin::pin!(runner.run_controlled(&control));
        if active {
            assert!(run.as_mut().poll(&mut cx).is_pending());
            // Repoll without an event cannot complete the active owner.
            assert!(
                run.as_mut().poll(&mut cx).is_pending(),
                "stop cannot drop in-flight TX"
            );
            completion.signal(());
        }
        let expected = if fail_completion {
            Err(())
        } else {
            Ok(DatapathRunnerExit::Stopped)
        };
        assert_eq!(run.as_mut().poll(&mut cx), Poll::Ready(expected));
    }
    assert_eq!(runner.services.completions, usize::from(active));
    if fail_completion {
        // The failed completion retains the live transaction and the link;
        // the latched stop finishes it on the next run of the same owner.
        assert_eq!(runner.active_tx_interface, Some(interface));
        assert!(device.link_is_up());
        runner.services.fail_completion = false;
        completion.signal(());
        let mut run = core::pin::pin!(runner.run_controlled(&control));
        assert_eq!(
            run.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathRunnerExit::Stopped))
        );
    }
    assert!(control.stop_requested());
    assert!(!device.link_is_up());
    assert!(runner.services.prepared.is_none());
    assert_eq!(runner.services.stops, 1);
}

#[test]
fn stop_cancels_prepared_tx_and_takes_the_link_down() {
    exercise_stop(false, false);
}

#[test]
fn stop_waits_for_active_tx_event_before_returning() {
    exercise_stop(true, false);
}

#[test]
fn failed_tx_drain_retains_the_transaction_and_the_latched_stop() {
    exercise_stop(true, true);
}

#[test]
fn control_exchange_waits_for_each_terminal_event_without_admitting_prepared_data() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
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

    let original = runner.services.prepared.as_ref().unwrap().as_ptr();
    let mut step = 0;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut exchange = core::pin::pin!(runner.run_control_exchange(|services| {
            assert_eq!(services.completions, step);
            step += 1;
            Ok(if step <= 2 {
                DatapathControlProgress::TxPending
            } else {
                DatapathControlProgress::Idle
            })
        }));
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        completion.signal(());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        completion.signal(());
        assert_eq!(exchange.as_mut().poll(&mut cx), Poll::Ready(Ok(None)));
    }
    assert_eq!(step, 3);
    assert_eq!(runner.services.completions, 2);
    assert_eq!(runner.services.stops, 0);
    assert_eq!(
        runner.services.prepared.as_ref().unwrap().as_ptr(),
        original
    );
    assert_eq!(runner.prepared_tx_interface, Some(interface));
    assert!(runner.active_tx_interface.is_none());
    let _ = device;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChainEvent {
    StartNetwork,
    StartPrepared(u8),
    Completed,
    Control,
}

/// A saturated role: every completion leaves a one-MPDU successor until the
/// configured count is exhausted.
struct ChainServices<'a> {
    completion: &'a Signal<NoopRawMutex, ()>,
    log: std::vec::Vec<ChainEvent>,
    successors: u8,
    prepared: Option<u8>,
    control_after_successor: u8,
    control_pending: bool,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for ChainServices<'_>
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
        self.log.push(ChainEvent::StartNetwork);
        Ok(WifiTxProgress::Pending)
    }

    async fn wait_tx_deadline(&mut self) {
        self.completion.wait().await;
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        self.log.push(ChainEvent::Completed);
        let published = self
            .log
            .iter()
            .filter(|event| matches!(event, ChainEvent::StartPrepared(_)))
            .count() as u8;
        if published < self.successors {
            self.prepared = Some(published + 1);
        }
        if published == self.control_after_successor {
            self.control_pending = true;
        }
        Ok(WifiTxProgress::Complete)
    }

    fn control_ready(&self, _: u64) -> bool {
        self.control_pending
    }

    async fn service_control(
        &mut self,
        _: DatapathControlContext,
    ) -> Result<DatapathControlProgress<()>, ()> {
        self.control_pending = false;
        self.log.push(ChainEvent::Control);
        Ok(DatapathControlProgress::Idle)
    }

    fn has_prepared_tx(&self) -> bool {
        self.prepared.is_some()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        usize::from(self.prepared.is_some())
    }

    /// A 32-MPDU Block Ack window: every successor is far below its target
    /// and must still be published without waiting for more owners.
    fn tx_batch_demand<I>(&self, _: NetworkInterfaceId, _: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        TxBatchDemand {
            target: 32,
            ready: usize::from(self.prepared.is_some()),
        }
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        let successor = self.prepared.take().expect("a complete successor");
        self.log.push(ChainEvent::StartPrepared(successor));
        Ok(WifiTxProgress::Pending)
    }
}

#[test]
fn partial_successors_publish_without_waiting_but_yield_to_ready_control() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let mut frame = allocator.try_alloc().unwrap();
    frame.set_len(15);
    frame.fill(0);
    frame[..6].fill(4);
    device.transmit(frame).unwrap();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let finished = Signal::<NoopRawMutex, ()>::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        ChainServices {
            completion: &completion,
            log: std::vec::Vec::new(),
            successors: 3,
            prepared: None,
            control_after_successor: 1,
            control_pending: false,
        },
    );
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut run = core::pin::pin!(runner.run_until(finished.wait()));
        // Four exchanges: the network frame and three successors.
        for _ in 0..4 {
            assert!(run.as_mut().poll(&mut cx).is_pending());
            completion.signal(());
        }
        finished.signal(());
        assert_eq!(
            run.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathRunnerExit::Stopped))
        );
    }
    assert_eq!(
        runner.services.log,
        [
            // A fresh runner services control once before admitting data.
            ChainEvent::Control,
            ChainEvent::StartNetwork,
            ChainEvent::Completed,
            ChainEvent::StartPrepared(1),
            ChainEvent::Completed,
            ChainEvent::Control,
            ChainEvent::StartPrepared(2),
            ChainEvent::Completed,
            ChainEvent::StartPrepared(3),
            ChainEvent::Completed,
        ]
    );
}
