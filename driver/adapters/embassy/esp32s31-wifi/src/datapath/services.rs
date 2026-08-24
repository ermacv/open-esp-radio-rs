#![expect(
    clippy::manual_async_fn,
    reason = "service implementations keep explicit borrowed Future contracts across role adapters"
)]

//! Finite role-neutral RX, control and network-TX service composition.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use open_esp_radio_embassy_net::{PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex};

use crate::datapath::{
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathServices,
    WifiTxProgress, WifiTxWake,
};

/// One RX owner that copies a finite descriptor frontier into independent
/// staging ownership. Protocol dispatch belongs to a separate consumer.
/// Implementations may await bounded reload timer edges but must not retain the
/// mutable hardware borrow across an await.
pub trait DatapathRxService<H> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a;
}

/// One owned control scheduler sharing the services owner's PAC and TX transaction.
pub trait DatapathControlService<H, X> {
    type Error;
    type Exit;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a;

    /// O(1) readiness observation used at a physical TX boundary.
    fn ready(&self, _tx: &X, _now_micros: u64) -> bool {
        true
    }

    /// Require one control transition before the scheduler may claim a new
    /// network TX lease (for example, to advertise PM=0 first).
    fn required_before_network_tx(&self) -> bool {
        false
    }

    fn required_before_stop(&self) -> bool {
        false
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoDatapathControl;

impl<H, X> DatapathControlService<H, X> for NoDatapathControl {
    type Error = Infallible;
    type Exit = Infallible;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _tx: &'a mut X,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        ready(Ok(DatapathControlProgress::Idle))
    }

    fn ready(&self, _tx: &X, _now_micros: u64) -> bool {
        false
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
pub trait DatapathNetworkTxService<
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

    fn start_prepared(
        &mut self,
        _hardware: &mut H,
        _network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        Ok(WifiTxProgress::Complete)
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
pub enum DatapathServiceError<RxError, ControlError = Infallible, TxError = Infallible> {
    Rx(RxError),
    Control(ControlError),
    Tx(TxError),
}

/// Role-local RX, TX and control ownership independent of the physical
/// register capability.
///
/// A standalone composition stores this value beside its hardware owner. A
/// concurrent composition moves the same value into one logical interface
/// slot. The container intentionally has no STA/AP policy of its own.
pub struct RoleRuntime<R, X, C> {
    rx: R,
    tx: X,
    control: C,
}

impl<R, X, C> RoleRuntime<R, X, C> {
    pub const fn new(rx: R, tx: X, control: C) -> Self {
        Self { rx, tx, control }
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

    pub fn control_and_tx_mut(&mut self) -> (&mut C, &mut X) {
        (&mut self.control, &mut self.tx)
    }

    pub fn into_parts(self) -> (R, X, C) {
        (self.rx, self.tx, self.control)
    }
}

/// Unique connected-epoch owner for the shared register capability, RX ring,
/// connected control and the selected ordinary/aggregate TX owner.
pub struct SingleRoleServices<H, R, X, C = NoDatapathControl> {
    hardware: H,
    role: RoleRuntime<R, X, C>,
}

impl<H, R, X> SingleRoleServices<H, R, X, NoDatapathControl> {
    pub const fn new(hardware: H, rx: R, tx: X) -> Self {
        Self {
            hardware,
            role: RoleRuntime::new(rx, tx, NoDatapathControl),
        }
    }
}

impl<H, R, X, C> SingleRoleServices<H, R, X, C> {
    pub const fn with_control(hardware: H, rx: R, tx: X, control: C) -> Self {
        Self {
            hardware,
            role: RoleRuntime::new(rx, tx, control),
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    pub const fn rx(&self) -> &R {
        self.role.rx()
    }

    pub fn rx_mut(&mut self) -> &mut R {
        self.role.rx_mut()
    }

    pub const fn tx(&self) -> &X {
        self.role.tx()
    }

    pub fn tx_mut(&mut self) -> &mut X {
        self.role.tx_mut()
    }

    pub const fn control(&self) -> &C {
        self.role.control()
    }

    pub fn control_mut(&mut self) -> &mut C {
        self.role.control_mut()
    }

    pub const fn role(&self) -> &RoleRuntime<R, X, C> {
        &self.role
    }

    pub fn role_mut(&mut self) -> &mut RoleRuntime<R, X, C> {
        &mut self.role
    }

    /// Recover every connected-epoch owner for explicit station teardown.
    ///
    /// This is intentionally a consuming operation: hardware, RX, TX and
    /// control may only be separated after the outer runner has stopped
    /// scheduling them.
    pub fn into_parts(self) -> (H, R, X, C) {
        let (rx, tx, control) = self.role.into_parts();
        (self.hardware, rx, tx, control)
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
> DatapathServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for SingleRoleServices<H, R, X, C>
where
    M: RawMutex,
    R: DatapathRxService<H>,
    X: DatapathNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    C: DatapathControlService<H, X>,
{
    type Error = DatapathServiceError<R::Error, C::Error, X::Error>;
    type Exit = C::Exit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn crate::datapath::network::DatapathNetworkRxSet,
        _context: crate::datapath::DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.role
                .rx
                .service(&mut self.hardware)
                .await
                .map_err(DatapathServiceError::Rx)
        }
    }

    fn service_control<'a>(
        &'a mut self,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            self.role
                .control
                .service(&mut self.hardware, &mut self.role.tx, context)
                .await
                .map_err(DatapathServiceError::Control)
        }
    }

    fn control_ready(&self, now_micros: u64) -> bool {
        self.role.control.ready(&self.role.tx, now_micros)
    }

    fn control_required_before_network_tx(&self) -> bool {
        self.role.control.required_before_network_tx()
    }

    fn control_required_before_stop(&self) -> bool {
        self.role.control.required_before_stop()
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        self.role.control.wait_ready(&mut self.role.tx)
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
            self.role
                .tx
                .start(&mut self.hardware, frame, network)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }

    fn last_started_tx_frame_count(&self) -> usize {
        self.role.tx.last_started_frame_count()
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.role.tx.wait_deadline()
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.role
                .tx
                .service(&mut self.hardware, wake)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.role.tx.has_prepared()
    }

    fn preferred_tx_batch_size(&self) -> usize {
        self.role.tx.preferred_batch_size()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        self.role.tx.prepared_frame_count()
    }

    fn start_prepared_tx(
        &mut self,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        self.role
            .tx
            .start_prepared(&mut self.hardware, network)
            .map_err(DatapathServiceError::Tx)
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        self.role
            .tx
            .cancel_prepared()
            .map_err(DatapathServiceError::Tx)
    }

    fn can_prepare_tx(&self) -> bool {
        self.role.tx.can_prepare()
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
            self.role
                .tx
                .prepare(frame, network)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }
}
