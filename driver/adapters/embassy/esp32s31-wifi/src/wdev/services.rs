//! Finite role-neutral RX, control and network-TX service composition.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use open_esp_radio_embassy_net::{PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex};

use crate::wdev::{
    WdevControlContext, WdevControlProgress, WdevRxProgress, WdevServices, WifiTxProgress,
    WifiTxWake,
};

/// One RX owner that copies a finite descriptor frontier into independent
/// staging ownership. Protocol dispatch belongs to a separate consumer.
/// Implementations may await bounded reload timer edges but must not retain the
/// mutable hardware borrow across an await.
pub trait WdevRxService<H> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a;
}

/// One owned control scheduler sharing the services owner's PAC and TX transaction.
pub trait WdevControlService<H, X> {
    type Error;
    type Exit;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a;

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoWdevControl;

impl<H, X> WdevControlService<H, X> for NoWdevControl {
    type Error = Infallible;
    type Exit = Infallible;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _tx: &'a mut X,
        _context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a {
        ready(Ok(WdevControlProgress::Idle))
    }

    fn wait_ready<'a>(&'a mut self, _tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        pending()
    }
}

/// Connected network-TX transaction owned by the connected services graph.
///
/// Taking the first lease by value is essential for referenced DMA. The
/// service may synchronously claim further ready leases from `network`, but
/// never retains the runner capability itself.
pub trait WdevNetworkTxService<
    'resources,
    M: RawMutex,
    H,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
>
{
    type Error;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    /// Network leases claimed by the most recent successful start.
    fn last_started_frame_count(&self) -> usize {
        1
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    /// Whether a software-owned network batch is ready for scheduler
    /// admission. It is not hardware-owned and must not suppress control
    /// service at the transaction boundary.
    fn has_prepared(&self) -> bool {
        false
    }

    fn preferred_batch_size(&self) -> usize {
        1
    }

    fn prepared_frame_count(&self) -> usize {
        0
    }

    fn start_prepared<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        ready(Ok(WifiTxProgress::Complete))
    }

    /// Release a software-owned standby batch at stop/disconnect.
    fn cancel_prepared(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn can_prepare(&self) -> bool {
        false
    }

    fn prepare<'a>(
        &'a mut self,
        _frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
    {
        ready(Ok(()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevServiceError<RxError, ControlError = Infallible, TxError = Infallible> {
    Rx(RxError),
    Control(ControlError),
    Tx(TxError),
}

/// Unique connected-epoch owner for the shared register capability, RX ring,
/// connected control and the selected ordinary/aggregate TX owner.
pub struct WdevServiceSet<H, R, X, C = NoWdevControl> {
    hardware: H,
    rx: R,
    tx: X,
    control: C,
}

impl<H, R, X> WdevServiceSet<H, R, X, NoWdevControl> {
    pub const fn new(hardware: H, rx: R, tx: X) -> Self {
        Self {
            hardware,
            rx,
            tx,
            control: NoWdevControl,
        }
    }
}

impl<H, R, X, C> WdevServiceSet<H, R, X, C> {
    pub const fn with_control(hardware: H, rx: R, tx: X, control: C) -> Self {
        Self {
            hardware,
            rx,
            tx,
            control,
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut R {
        &mut self.rx
    }

    pub const fn tx(&self) -> &X {
        &self.tx
    }

    pub fn tx_mut(&mut self) -> &mut X {
        &mut self.tx
    }

    pub const fn control(&self) -> &C {
        &self.control
    }

    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    /// Recover every connected-epoch owner for explicit station teardown.
    ///
    /// This is intentionally a consuming operation: hardware, RX, TX and
    /// control may only be separated after the outer runner has stopped
    /// scheduling them.
    pub fn into_parts(self) -> (H, R, X, C) {
        (self.hardware, self.rx, self.tx, self.control)
    }
}

impl<
    'resources,
    M,
    H,
    R,
    X,
    C,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for WdevServiceSet<H, R, X, C>
where
    M: RawMutex,
    R: WdevRxService<H>,
    X: WdevNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    C: WdevControlService<H, X>,
{
    type Error = WdevServiceError<R::Error, C::Error, X::Error>;
    type Exit = C::Exit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn crate::wdev::WdevNetworkRxSet,
        _context: crate::wdev::WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            self.rx
                .service(&mut self.hardware)
                .await
                .map_err(WdevServiceError::Rx)
        }
    }

    fn service_control<'a>(
        &'a mut self,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            self.control
                .service(&mut self.hardware, &mut self.tx, context)
                .await
                .map_err(WdevServiceError::Control)
        }
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        self.control.wait_ready(&mut self.tx)
    }

    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.tx
                .start(&mut self.hardware, frame, network)
                .await
                .map_err(WdevServiceError::Tx)
        }
    }

    fn last_started_tx_frame_count(&self) -> usize {
        self.tx.last_started_frame_count()
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.tx.wait_deadline()
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.tx
                .service(&mut self.hardware, wake)
                .await
                .map_err(WdevServiceError::Tx)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.tx.has_prepared()
    }

    fn preferred_tx_batch_size(&self) -> usize {
        self.tx.preferred_batch_size()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        self.tx.prepared_frame_count()
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.tx
                .start_prepared(&mut self.hardware, network)
                .await
                .map_err(WdevServiceError::Tx)
        }
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        self.tx.cancel_prepared().map_err(WdevServiceError::Tx)
    }

    fn can_prepare_tx(&self) -> bool {
        self.tx.can_prepare()
    }

    fn prepare_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            self.tx
                .prepare(frame, network)
                .await
                .map_err(WdevServiceError::Tx)
        }
    }
}
