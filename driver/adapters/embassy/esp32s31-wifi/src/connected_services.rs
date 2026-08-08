//! Finite ESP32-S31 RX, control and network-TX services for one connected epoch.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use open_esp_radio_embassy_net::{PinnedTxConsumer, PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi_mac::tx::TxHardware;

use crate::{
    connected_runner::{
        ConnectedRunnerServices, WifiControlContext, WifiControlProgress, WifiRxProgress,
        WifiTxProgress, WifiTxWake,
    },
    single_mpdu_tx::{
        Esp32s31SingleMpduTx, SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
    },
};

/// One RX owner that copies a finite descriptor frontier into independent
/// staging ownership. Protocol dispatch belongs to a separate consumer.
/// Implementations may await bounded reload timer edges but must not retain the
/// mutable hardware borrow across an await.
pub trait Esp32s31ConnectedRxService<H> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a;
}

/// One owned control scheduler sharing the services owner's PAC and TX transaction.
pub trait Esp32s31ControlService<H, X> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a;

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoConnectedControl;

impl<H, X> Esp32s31ControlService<H, X> for NoConnectedControl {
    type Error = Infallible;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _tx: &'a mut X,
        _context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a {
        ready(Ok(WifiControlProgress::Idle))
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
pub trait Esp32s31NetworkTxService<
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
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

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

    fn start_prepared<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _network: &'a PinnedTxConsumer<
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
        _network: &'a PinnedTxConsumer<
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
pub enum Esp32s31ConnectedServicesError<
    RxError,
    ControlError = Infallible,
    TxError = SingleMpduTxError,
> {
    Rx(RxError),
    Control(ControlError),
    Tx(TxError),
}

/// Unique connected-epoch owner for the shared register capability, RX ring,
/// connected control and the selected ordinary/aggregate TX owner.
pub struct Esp32s31ConnectedServices<H, R, X, C = NoConnectedControl> {
    hardware: H,
    rx: R,
    tx: X,
    control: C,
}

impl<H, R, X> Esp32s31ConnectedServices<H, R, X, NoConnectedControl> {
    pub const fn new(hardware: H, rx: R, tx: X) -> Self {
        Self {
            hardware,
            rx,
            tx,
            control: NoConnectedControl,
        }
    }
}

impl<H, R, X, C> Esp32s31ConnectedServices<H, R, X, C> {
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
> ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31ConnectedServices<H, R, X, C>
where
    M: RawMutex,
    R: Esp32s31ConnectedRxService<H>,
    X: Esp32s31NetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    C: Esp32s31ControlService<H, X>,
{
    type Error = Esp32s31ConnectedServicesError<R::Error, C::Error, X::Error>;

    fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_ {
        async move {
            self.rx
                .service(&mut self.hardware)
                .await
                .map_err(Esp32s31ConnectedServicesError::Rx)
        }
    }

    fn service_control<'a>(
        &'a mut self,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            self.control
                .service(&mut self.hardware, &mut self.tx, context)
                .await
                .map_err(Esp32s31ConnectedServicesError::Control)
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
        network: &'a PinnedTxConsumer<
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
                .map_err(Esp32s31ConnectedServicesError::Tx)
        }
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
                .map_err(Esp32s31ConnectedServicesError::Tx)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.tx.has_prepared()
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        network: &'a PinnedTxConsumer<
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
                .map_err(Esp32s31ConnectedServicesError::Tx)
        }
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        self.tx
            .cancel_prepared()
            .map_err(Esp32s31ConnectedServicesError::Tx)
    }

    fn can_prepare_tx(&self) -> bool {
        self.tx.can_prepare()
    }

    fn prepare_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxConsumer<
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
                .map_err(Esp32s31ConnectedServicesError::Tx)
        }
    }
}

impl<
    'resources,
    'slot,
    M,
    H,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const TX_BUFFER_SIZE: usize,
> Esp32s31NetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31SingleMpduTx<'slot, P, E, T, TX_BUFFER_SIZE>
where
    M: RawMutex,
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Error = SingleMpduTxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        _network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = Esp32s31SingleMpduTx::start(self, hardware, frame.ethernet())?;
            // Ordinary TX copied the complete Ethernet body into its private
            // pinned slot before publishing DMA, so the network lease is no
            // longer hardware-visible at this boundary.
            drop(frame);
            Ok(progress)
        }
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        Esp32s31SingleMpduTx::wait_deadline(self)
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        Esp32s31SingleMpduTx::service(self, hardware, wake)
    }
}
