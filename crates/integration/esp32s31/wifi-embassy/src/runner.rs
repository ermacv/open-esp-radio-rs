//! Production Embassy event loop for one ESP32-S31 Wi-Fi radio owner.
//!
//! The runner owns PAC/DMA/TX scheduling. Connected-frame protocol state lives
//! in a separate staged-RX consumer, so parsing cannot extend one hardware
//! service epoch. A backend owns only finite PAC/DMA transactions: it must
//! never wait for an executor primitive while holding a mutable PAC borrow.

use core::future::{Future, pending, ready};

use embassy_futures::{
    select::{Either3, select3},
    yield_now,
};
use open_esp_radio_embassy_net::{PinnedRadioRunner, PinnedTxFrame, RawMutex};

use crate::embassy_irq::EmbassyMacIrqRuntime;

/// State of the one hardware TX transaction currently owned by the runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxProgress {
    /// DMA, acknowledgement or a bounded retry is still in flight.
    Pending,
    /// Hardware no longer owns the pinned network frame.
    Complete,
}

/// Result of one bounded RX bottom-half pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxProgress {
    /// The durable completion frontier was drained within this pass.
    Drained,
    /// Completed descriptors remain, but no independent staging owner is
    /// available. Resume only after protocol processing returns a credit.
    Backpressured,
}

/// Result of one finite control-plane scheduling step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiControlProgress {
    /// No control-plane PAC or TX work is currently ready.
    Idle,
    /// One finite action completed and another action may remain queued.
    More,
    /// A control frame now owns the shared TX transaction.
    TxPending,
    /// The connected policy proved that the peer is no longer reachable.
    /// The runner publishes link-down and returns ownership to its caller.
    Disconnected,
}

/// Coherent runner-owned scheduling facts supplied to one control step.
///
/// This value is sampled before control arbitration, while no TX transaction
/// owns hardware. It prevents power policy from treating an already queued
/// network frame as idle merely because the backend has not claimed it yet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiControlContext {
    pub network_tx_pending: bool,
}

impl WifiControlContext {
    pub const IDLE: Self = Self {
        network_tx_pending: false,
    };
}

/// Reason for inspecting the one active TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxWake {
    /// A coalesced completion, hardware-timeout or collision interrupt fired.
    Interrupt { events: u32 },
    /// The transaction's executor deadline expired without a decisive IRQ.
    Deadline,
}

/// Finite chip-specific operations used by [`WifiRunner`].
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
/// and the lifetime of a pinned network lease belong to [`WifiRunner::run`].
pub trait WifiRunnerBackend<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
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
    /// The backend receives ownership rather than a temporary borrow. An
    /// ordinary copy-based transmitter may release the lease immediately;
    /// a referenced A-MPDU owner may retain it, claim further ready leases
    /// from `network`, and return all of them only after BlockAck/detach.
    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
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
pub struct WifiRunner<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    backend: B,
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
    const QUEUE_DEPTH: usize,
> WifiRunner<'resources, 'irq, M, B, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
where
    B: WifiRunnerBackend<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
{
    async fn service_rx(&mut self) -> Result<(), B::Error> {
        self.rx_backpressured = self.backend.service_rx().await? == WifiRxProgress::Backpressured;
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
            let wake = select3(wait_rx, self.irq.wait_tx(), self.backend.wait_tx_deadline()).await;
            match wake {
                Either3::First(()) => self.service_rx().await?,
                Either3::Second(events) => {
                    progress = self
                        .backend
                        .service_tx(WifiTxWake::Interrupt { events })
                        .await?;
                }
                Either3::Third(()) => {
                    progress = self.backend.service_tx(WifiTxWake::Deadline).await?;
                }
            }
        }
        Ok(())
    }

    pub const fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        backend: B,
    ) -> Self {
        Self {
            irq,
            network,
            backend,
            rx_backpressured: false,
        }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Run the production radio event loop.
    ///
    /// RX is the first future in both selects. Embassy's ordered `select`
    /// therefore preserves the recovered `wDev_ProcessFiq` priority when RX
    /// and TX become ready together. A pinned network lease stays live until
    /// `service_tx` proves that hardware ownership has ended; dropping the
    /// lease then returns that slot to `embassy-net`.
    pub async fn run(&mut self) -> Result<(), B::Error> {
        loop {
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            let control_context = WifiControlContext {
                network_tx_pending: self.network.tx_queue_len() != 0,
            };
            match self.backend.service_control(control_context).await? {
                WifiControlProgress::More => continue,
                WifiControlProgress::TxPending => {
                    self.drive_active_tx().await?;
                    continue;
                }
                WifiControlProgress::Disconnected => {
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(());
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
            match select3(
                wait_rx,
                self.backend.wait_control_ready(),
                self.network.receive_tx(),
            )
            .await
            {
                Either3::First(()) => self.service_rx().await?,
                Either3::Second(()) => {}
                Either3::Third(frame) => {
                    // `receive_tx` may have consumed the first lease after
                    // the context at the top of the loop was sampled. Hold
                    // that lease while control restores PM=0 (if needed),
                    // then publish the data frame only after the AP-visible
                    // state is coherent again.
                    loop {
                        match self
                            .backend
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
                                return Ok(());
                            }
                            WifiControlProgress::Idle => break,
                        }
                    }
                    let progress = self.backend.start_tx(frame, &self.network).await?;
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
        mem::MaybeUninit,
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
    }

    impl Backend {
        fn push(&mut self, event: u8) {
            self.order[self.count] = event;
            self.count += 1;
        }
    }

    impl WifiRunnerBackend<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
        for Backend
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
            _network: &'a PinnedRadioRunner<
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
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);
        let mut run = std::boxed::Box::pin(runner.run());
        let mut context = Context::from_waker(core::task::Waker::noop());

        assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
        enqueue_frame(&mut device);
        assert_eq!(
            embassy_futures::block_on(run.as_mut()),
            Err(TestError::Finished)
        );
        drop(run);
        assert!(runner.backend().network_pending_seen);
    }

    #[test]
    fn rx_is_serviced_before_tx_when_both_irqs_are_ready() {
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.backend().order[..2], [1, 2]);
        assert_eq!(
            runner.backend().tx_wake,
            Some(WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            })
        );
    }

    #[test]
    fn staging_backpressure_gates_new_rx_edges_but_not_tx_completion() {
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.backend().order[..2], [1, 2]);
        assert_eq!(
            runner.backend().tx_wake,
            Some(WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            })
        );
        assert!(irq.rx_signaled());
    }

    #[test]
    fn executor_deadline_services_tx_without_an_interrupt() {
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.backend().order[0], 2);
        assert_eq!(runner.backend().tx_wake, Some(WifiTxWake::Deadline));
    }

    #[test]
    fn rx_control_waits_for_the_active_network_tx_then_precedes_another_lease() {
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        enqueue_frame(&mut device);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);

        assert_eq!(
            embassy_futures::block_on(runner.run()),
            Err(TestError::Finished)
        );
        assert_eq!(runner.backend().order, [1, 2, 3]);
    }

    #[test]
    fn disconnected_control_edge_publishes_link_down_and_returns() {
        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
        let irq = std::boxed::Box::leak(std::boxed::Box::new(
            EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
        ));
        let backend = Backend {
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
        };
        let mut runner = WifiRunner::new(irq, network, backend);

        assert_eq!(embassy_futures::block_on(runner.run()), Ok(()));
        let mut context = Context::from_waker(core::task::Waker::noop());
        assert!(matches!(
            device.link_state(&mut context),
            open_esp_radio_embassy_net::LinkState::Down
        ));
    }
}
