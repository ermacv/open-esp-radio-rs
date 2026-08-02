//! Production composition of finite ESP32-S31 RX, control and network-TX services.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use open_esp_radio_embassy_net::{PinnedRadioRunner, PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi_mac::{connected_rx::ConnectedRxDispatcher, tx::TxHardware};

use crate::{
    runner::{
        WifiControlContext, WifiControlProgress, WifiRunnerBackend, WifiRxProgress, WifiTxProgress,
        WifiTxWake,
    },
    single_mpdu_tx::{
        Esp32s31SingleMpduTx, SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
    },
};

/// One RX owner that drains a durable descriptor frontier and dispatches every
/// staged frame. Implementations may await bounded reload timer edges but must
/// not retain the mutable hardware borrow across an await.
pub trait Esp32s31ConnectedRxService<H> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        dispatcher: &'a mut ConnectedRxDispatcher,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a;
}

/// One owned control scheduler sharing the backend's PAC and TX transaction.
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

/// Connected network-TX transaction owned by the production backend.
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
        network: &'a PinnedRadioRunner<
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31WifiBackendError<RxError, ControlError = Infallible, TxError = SingleMpduTxError> {
    Rx(RxError),
    Control(ControlError),
    Tx(TxError),
}

/// Unique production owner for the shared register capability, RX ring,
/// connected control and the selected ordinary/aggregate TX owner.
pub struct Esp32s31WifiBackend<H, R, X, C = NoConnectedControl> {
    hardware: H,
    rx: R,
    tx: X,
    control: C,
}

impl<H, R, X> Esp32s31WifiBackend<H, R, X, NoConnectedControl> {
    pub const fn new(hardware: H, rx: R, tx: X) -> Self {
        Self {
            hardware,
            rx,
            tx,
            control: NoConnectedControl,
        }
    }
}

impl<H, R, X, C> Esp32s31WifiBackend<H, R, X, C> {
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
> WifiRunnerBackend<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31WifiBackend<H, R, X, C>
where
    M: RawMutex,
    R: Esp32s31ConnectedRxService<H>,
    X: Esp32s31NetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    C: Esp32s31ControlService<H, X>,
{
    type Error = Esp32s31WifiBackendError<R::Error, C::Error, X::Error>;

    fn service_rx<'a>(
        &'a mut self,
        dispatcher: &'a mut ConnectedRxDispatcher,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a {
        async move {
            self.rx
                .service(&mut self.hardware, dispatcher)
                .await
                .map_err(Esp32s31WifiBackendError::Rx)
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
                .map_err(Esp32s31WifiBackendError::Control)
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
        network: &'a PinnedRadioRunner<
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
                .map_err(Esp32s31WifiBackendError::Tx)
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
                .map_err(Esp32s31WifiBackendError::Tx)
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
        _network: &'a PinnedRadioRunner<
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
