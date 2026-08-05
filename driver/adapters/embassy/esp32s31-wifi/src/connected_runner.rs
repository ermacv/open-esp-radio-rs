//! Embassy event loop for one connected ESP32-S31 Wi-Fi radio owner.
//!
//! The runner owns PAC/DMA/TX scheduling. Connected-frame protocol state lives
//! in a separate staged-RX consumer, so parsing cannot extend one hardware
//! service epoch. The services owner exposes only finite PAC/DMA transactions: it must
//! never wait for an executor primitive while holding a mutable PAC borrow.

use core::future::{Future, pending, ready};

use embassy_futures::{
    select::{Either, Either3, select, select3},
    yield_now,
};
use open_esp_radio_embassy_net::{
    PinnedTxConsumer, PinnedTxFrame, RawMutex, SplitPinnedRadioRunner,
};
pub use open_esp_radio_esp32s31_wifi_sta::connected_control::{
    ConnectedControlContext as WifiControlContext, ConnectedControlProgress as WifiControlProgress,
};
pub use open_esp_radio_esp32s31_wifi_sta::tx::{WifiTxProgress, WifiTxWake};

use crate::embassy_irq::EmbassyMacIrqRuntime;

/// Result of one bounded RX bottom-half pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxProgress {
    /// The durable completion frontier was drained within this pass.
    Drained,
    /// Completed descriptors remain, but no independent staging owner is
    /// available. Resume only after protocol processing returns a credit.
    Backpressured,
}

/// Terminal, non-error outcome of the connected radio event loop.
///
/// Keeping this distinct from `()` prevents an outer station owner from
/// confusing a proved link loss with a runner that completed without a
/// lifecycle transition. The caller may use this edge to tear down the
/// network stack, release the connected epoch and start reassociation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRunnerExit {
    /// Connected policy proved that the peer is no longer reachable and the
    /// runner published link-down before returning.
    Disconnected,
    /// The outer station lifecycle requested a finite stop. The runner waited
    /// for any active TX transaction to release hardware, published link-down
    /// and returned the same owners as a disconnect without claiming peer
    /// reachability had failed.
    Stopped,
}

/// Finite chip-specific operations used by [`ConnectedRunner`].
///
/// An implementation normally owns the live RX descriptor ring, staging
/// storage, staging publisher, TX descriptor state and a short-lived PAC
/// facade such as `RadioRegisters`. `service_rx` must snapshot and drain one
/// durable RX frontier into independent staging ownership. A separate
/// protocol consumer retains duplicate/protocol history.
///
/// Every method must finish after a bounded number of hardware observations.
/// A method may await a timer edge needed by a typed transaction, but it must
/// release every mutable PAC borrow before that edge. RX-before-TX arbitration
/// and the lifetime of a pinned network lease belong to [`ConnectedRunner::run`].
pub trait ConnectedRunnerServices<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
>
{
    type Error;

    /// Drain one snapshotted RX-success frontier into independent ownership.
    fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_;

    /// Apply at most one owned control event or publish one control frame.
    ///
    /// The runner invokes this only while no network TX transaction owns the
    /// shared descriptor. `More` returns through the scheduler before a new
    /// network lease can be claimed; `TxPending` enters the same IRQ/deadline
    /// completion loop as ordinary data.
    fn service_control<'a>(
        &'a mut self,
        _context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        ready(Ok(WifiControlProgress::Idle))
    }

    /// Wake the outer scheduler for a control timer or independently
    /// published control event. Backends without such a source never wake it.
    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        pending()
    }

    /// Transfer one network-owned frame into the MAC/DMA transaction.
    ///
    /// The services owner receives ownership rather than a temporary borrow. An
    /// ordinary copy-based transmitter may release the lease immediately;
    /// a referenced A-MPDU owner may retain it, claim further ready leases
    /// from `network`, and return all of them only after BlockAck/detach.
    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    /// Wait for the executor deadline of the active TX transaction.
    ///
    /// This future is created only after [`Self::start_tx`] returned
    /// [`WifiTxProgress::Pending`]. A retry may replace the active deadline;
    /// the runner creates a fresh future after every service operation.
    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_;

    /// Inspect, complete, abort or restart the active TX transaction.
    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;
}

/// Single Embassy owner for RX DMA, control, network TX and MAC IRQ order.
pub struct ConnectedRunner<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: SplitPinnedRadioRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    services: B,
    rx_backpressured: bool,
}

impl<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    ConnectedRunner<
        'resources,
        'irq,
        M,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    B: ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    async fn service_rx(&mut self) -> Result<(), B::Error> {
        self.rx_backpressured = self.services.service_rx().await? == WifiRxProgress::Backpressured;
        // One service call owns exactly the completion frontier captured at
        // its start. Yield at that hardware epoch boundary so a separate
        // protocol task can consume staged ownership before another RX epoch.
        yield_now().await;
        Ok(())
    }

    fn discard_stale_tx_wakes(&self) {
        while self.irq.try_take_tx().is_some() {}
    }

    async fn drive_active_tx(&mut self) -> Result<(), B::Error> {
        let mut progress = WifiTxProgress::Pending;
        while progress == WifiTxProgress::Pending {
            let irq = self.irq;
            let rx_backpressured = self.rx_backpressured;
            let wait_rx = async move {
                if rx_backpressured {
                    irq.wait_rx_capacity().await;
                } else {
                    irq.wait_rx().await;
                }
            };
            let wake = select3(
                wait_rx,
                self.irq.wait_tx(),
                self.services.wait_tx_deadline(),
            )
            .await;
            match wake {
                Either3::First(()) => self.service_rx().await?,
                Either3::Second(events) => {
                    progress = self
                        .services
                        .service_tx(WifiTxWake::Interrupt { events })
                        .await?;
                }
                Either3::Third(()) => {
                    progress = self.services.service_tx(WifiTxWake::Deadline).await?;
                }
            }
        }
        Ok(())
    }

    pub const fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: SplitPinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        services: B,
    ) -> Self {
        Self {
            irq,
            network,
            services,
            rx_backpressured: false,
        }
    }

    pub const fn services(&self) -> &B {
        &self.services
    }

    pub fn services_mut(&mut self) -> &mut B {
        &mut self.services
    }

    /// Return the network and hardware owners after the runner exits.
    ///
    /// A station lifecycle must be able to reclaim these values after
    /// [`ConnectedRunnerExit::Disconnected`] in order to stop DMA, clear keys and
    /// construct a later association epoch. Keeping them recoverable also
    /// makes it impossible for `run` to hide teardown behind task-local
    /// globals.
    pub fn into_parts(
        self,
    ) -> (
        SplitPinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        B,
    ) {
        (self.network, self.services)
    }

    /// Run the production radio event loop until connected policy proves the
    /// peer is unreachable.
    pub async fn run(&mut self) -> Result<ConnectedRunnerExit, B::Error> {
        self.run_until(pending()).await
    }

    /// Run the production radio event loop until disconnect or caller stop.
    ///
    /// RX is the first future in both selects. Embassy's ordered `select`
    /// therefore preserves the recovered `wDev_ProcessFiq` priority when RX
    /// and TX become ready together. A pinned network lease stays live until
    /// `service_tx` proves that hardware ownership has ended; dropping the
    /// lease then returns that slot to `embassy-net`.
    ///
    /// `stop` is observed only at transaction boundaries. If it becomes ready
    /// during TX, the normal IRQ/deadline path first releases hardware; the
    /// next idle boundary returns [`ConnectedRunnerExit::Stopped`]. This makes
    /// cancellation bounded without inventing an unsafe descriptor abort.
    pub async fn run_until<S>(&mut self, stop: S) -> Result<ConnectedRunnerExit, B::Error>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        loop {
            // Poll the caller edge before servicing control. `ready(())`
            // makes this a non-blocking ordered probe, with stop winning an
            // exact tie before another transaction can begin.
            if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                self.network
                    .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                return Ok(ConnectedRunnerExit::Stopped);
            }
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            let control_context = WifiControlContext {
                network_tx_pending: self.network.tx_queue_len() != 0,
            };
            match self.services.service_control(control_context).await? {
                WifiControlProgress::More => continue,
                WifiControlProgress::TxPending => {
                    self.drive_active_tx().await?;
                    continue;
                }
                WifiControlProgress::Disconnected => {
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(ConnectedRunnerExit::Disconnected);
                }
                WifiControlProgress::Idle => {}
            }

            let irq = self.irq;
            let rx_backpressured = self.rx_backpressured;
            let wait_rx = async move {
                if rx_backpressured {
                    irq.wait_rx_capacity().await;
                } else {
                    irq.wait_rx().await;
                }
            };
            match select(
                stop.as_mut(),
                select3(
                    wait_rx,
                    self.services.wait_control_ready(),
                    self.network.receive_tx(),
                ),
            )
            .await
            {
                Either::First(()) => {
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(ConnectedRunnerExit::Stopped);
                }
                Either::Second(Either3::First(())) => self.service_rx().await?,
                Either::Second(Either3::Second(())) => {}
                Either::Second(Either3::Third(frame)) => {
                    // `receive_tx` may have consumed the first lease after
                    // the context at the top of the loop was sampled. Hold
                    // that lease while control restores PM=0 (if needed),
                    // then publish the data frame only after the AP-visible
                    // state is coherent again.
                    loop {
                        if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                            drop(frame);
                            self.network
                                .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                            return Ok(ConnectedRunnerExit::Stopped);
                        }
                        match self
                            .services
                            .service_control(WifiControlContext {
                                network_tx_pending: true,
                            })
                            .await?
                        {
                            WifiControlProgress::More => continue,
                            WifiControlProgress::TxPending => {
                                self.drive_active_tx().await?;
                            }
                            WifiControlProgress::Disconnected => {
                                drop(frame);
                                self.network
                                    .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                                return Ok(ConnectedRunnerExit::Disconnected);
                            }
                            WifiControlProgress::Idle => break,
                        }
                    }
                    let network_tx = self.network.tx_consumer();
                    let progress = self.services.start_tx(frame, &network_tx).await?;
                    if progress == WifiTxProgress::Pending {
                        self.drive_active_tx().await?;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, pending},
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll},
    };

    use open_esp_radio_embassy_net::{
        Driver as _, NoopRawMutex, PinnedDevice, PinnedResources, PinnedTxPool, TxToken as _,
    };
    use open_esp_radio_esp32s31_wifi_mac::irq::{MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE};

    use super::*;

    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 32;
    const TRAILER: usize = 8;
    const QUEUE_DEPTH: usize = 1;

    type Resources = PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
    type Pool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
    type Device =
        PinnedDevice<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        Finished,
    }

    struct Backend {
        irq: &'static EmbassyMacIrqRuntime<NoopRawMutex>,
        order: [u8; 3],
        count: usize,
        publish_irq: bool,
        deadline_ready: bool,
        tx_wake: Option<WifiTxWake>,
        queue_control_on_rx: bool,
        control_pending: bool,
        complete_tx_before_control: bool,
        disconnect: bool,
        network_pending_seen: bool,
        backpressure_once: bool,
        repost_rx_when_backpressured: bool,
        stop_after_tx: Option<&'static AtomicBool>,
    }

    impl Backend {
        fn push(&mut self, event: u8) {
            self.order[self.count] = event;
            self.count += 1;
        }
    }

    impl
        ConnectedRunnerServices<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        > for Backend
    {
        type Error = TestError;

        fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_ {
            async move {
                self.push(1);
                if self.backpressure_once {
                    self.backpressure_once = false;
                    if self.repost_rx_when_backpressured {
                        self.irq.publish(MAC_INT_RX_SUCCESS);
                    }
                    return Ok(WifiRxProgress::Backpressured);
                }
                if self.queue_control_on_rx {
                    self.control_pending = true;
                }
                Ok(WifiRxProgress::Drained)
            }
        }

        fn service_control<'a>(
            &'a mut self,
            context: WifiControlContext,
        ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
        where
            'static: 'a,
            NoopRawMutex: 'a,
        {
            async move {
                self.network_pending_seen |= context.network_tx_pending;
                if self.disconnect {
                    return Ok(WifiControlProgress::Disconnected);
                }
                if !self.control_pending {
                    return Ok(WifiControlProgress::Idle);
                }
                self.control_pending = false;
                self.push(3);
                Err(TestError::Finished)
            }
        }

        fn start_tx<'a>(
            &'a mut self,
            _frame: PinnedTxFrame<
                'static,
                NoopRawMutex,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            _network: &'a PinnedTxConsumer<
                'static,
                NoopRawMutex,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
        ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
            async move {
                if self.publish_irq {
                    self.irq.publish(MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS);
                }
                Ok(WifiTxProgress::Pending)
            }
        }

        fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
            let ready = self.deadline_ready;
            async move {
                if !ready {
                    pending::<()>().await;
                }
            }
        }

        fn service_tx<'a>(
            &'a mut self,
            wake: WifiTxWake,
        ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
            async move {
                self.tx_wake = Some(wake);
                self.push(2);
                if let Some(stop) = self.stop_after_tx {
                    stop.store(true, Ordering::Release);
                }
                if self.complete_tx_before_control {
                    Ok(WifiTxProgress::Complete)
                } else {
                    Err(TestError::Finished)
                }
            }
        }
    }

    fn enqueue_frame(device: &mut Device) {
        let mut context = Context::from_waker(core::task::Waker::noop());
        device
            .transmit(&mut context)
            .unwrap()
            .consume(14, |frame| frame.fill(0x5a));
    }

    #[test]
    fn frame_arriving_inside_select_rechecks_control_as_network_pending() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: true,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);
        let mut run = std::boxed::Box::pin(runner.run());
        let mut context = Context::from_waker(core::task::Waker::noop());

        assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
        enqueue_frame(&mut device);
        assert_eq!(
            embassy_futures::block_on(run.as_mut()),
            Err(TestError::Finished)
        );
        drop(run);
        assert!(runner.services().network_pending_seen);
    }

    #[test]
    fn rx_is_serviced_before_tx_when_both_irqs_are_ready() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: true,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.services().order[..2], [1, 2]);
        assert_eq!(
            runner.services().tx_wake,
            Some(WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            })
        );
    }

    #[test]
    fn staging_backpressure_gates_new_rx_edges_but_not_tx_completion() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: true,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: true,
            repost_rx_when_backpressured: true,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.services().order[..2], [1, 2]);
        assert_eq!(
            runner.services().tx_wake,
            Some(WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            })
        );
        assert!(irq.rx_signaled());
    }

    #[test]
    fn executor_deadline_services_tx_without_an_interrupt() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: false,
            deadline_ready: true,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.services().order[0], 2);
        assert_eq!(runner.services().tx_wake, Some(WifiTxWake::Deadline));
    }

    #[test]
    fn rx_control_waits_for_the_active_network_tx_then_precedes_another_lease() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: true,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: true,
            control_pending: false,
            complete_tx_before_control: true,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.services().order, [1, 2, 3]);
    }

    #[test]
    fn caller_stop_publishes_link_down_and_returns_distinct_outcome() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: false,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run_until(core::future::ready(()))),
            Ok(ConnectedRunnerExit::Stopped)
        );
        let mut context = Context::from_waker(core::task::Waker::noop());
        assert!(matches!(
            device.link_state(&mut context),
            open_esp_radio_embassy_net::LinkState::Down
        ));
        let (_network, services) = runner.into_parts();
        assert!(!services.disconnect);
    }

    #[test]
    fn caller_stop_waits_for_active_tx_to_release_hardware() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let stop = std::boxed::Box::leak(std::boxed::Box::new(AtomicBool::new(false)));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: true,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: true,
            disconnect: false,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: Some(stop),
        };
        let stop_future = core::future::poll_fn(|context| {
            if stop.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                context.waker().wake_by_ref();
                Poll::Pending
            }
        });
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run_until(stop_future)),
            Ok(ConnectedRunnerExit::Stopped)
        );
        assert_eq!(runner.services().order[..2], [1, 2]);
        assert_eq!(
            runner.services().tx_wake,
            Some(WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            })
        );
        let mut context = Context::from_waker(core::task::Waker::noop());
        assert!(matches!(
            device.link_state(&mut context),
            open_esp_radio_embassy_net::LinkState::Down
        ));
    }

    #[test]
    fn disconnected_control_edge_publishes_link_down_and_returns() {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let services = Backend {
            irq,
            order: [0; 3],
            count: 0,
            publish_irq: false,
            deadline_ready: false,
            tx_wake: None,
            queue_control_on_rx: false,
            control_pending: false,
            complete_tx_before_control: false,
            disconnect: true,
            network_pending_seen: false,
            backpressure_once: false,
            repost_rx_when_backpressured: false,
            stop_after_tx: None,
        };
        let mut runner = ConnectedRunner::new(irq, network, services);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Ok(ConnectedRunnerExit::Disconnected)
        );
        let mut context = Context::from_waker(core::task::Waker::noop());
        assert!(matches!(
            device.link_state(&mut context),
            open_esp_radio_embassy_net::LinkState::Down
        ));
        let (_network, services) = runner.into_parts();
        assert!(services.disconnect);
    }
}
