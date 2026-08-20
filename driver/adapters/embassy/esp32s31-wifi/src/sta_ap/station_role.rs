//! Connected-station protocol, TX and control ownership for paired WDEV.

use core::future::Future;

use open_esp_radio_embassy_net::{PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex};
use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::WifiTxWake,
};
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware;

use super::{Esp32s31StaApStationRxRole, Esp32s31StagedRxFrame};
use crate::{
    ampdu_resources::AggregateTxResources,
    connected_control::{
        ConnectedControlError, ConnectedControlHardware, Esp32s31ConnectedControl,
    },
    connected_sta_port::{Esp32s31ConnectedStaDrivers, Esp32s31ConnectedStaReport},
    station_tx::{AggregateTxError, Esp32s31ConnectedTx, Esp32s31ConnectedTxParked},
    wdev::{
        WdevControlContext, WdevControlProgress, WdevNetworkRx, WdevRxProgress, WifiTxProgress,
        paired::{
            WdevPairRole, WdevPairedNetworkTxService, WdevPairedPhysicalTx,
            WdevPairedPhysicalTxError, WdevPairedRoleOwner, WdevPairedRoleTransitionError,
        },
        services::WdevNetworkTxService,
    },
};

/// Connected STA owners after the standalone runner boundary has been
/// removed and the physical TX pair has been parked in the shared owner.
///
/// The physical RX transport remains separate because the paired runtime
/// replaces it with one common STA/AP classifier. The station role retains
/// only its protocol consumer, connected control, and parked TX state.
pub struct Esp32s31StaApStationPrepared<H, PhysicalRx, Station, PhysicalTx> {
    pub hardware: H,
    pub physical_rx: PhysicalRx,
    pub station: Station,
    pub physical_tx: PhysicalTx,
    pub report: Esp32s31ConnectedStaReport,
}

/// Fail-closed station preparation frontier.
///
/// A busy connected TX transaction prevents paired composition. Every owner
/// is returned in its original active station form; callers cannot continue
/// with a partly materialized shared TX graph.
pub struct Esp32s31StaApStationPrepareFailure<H, PhysicalRx, Station> {
    pub hardware: H,
    pub physical_rx: PhysicalRx,
    pub station: Station,
    pub report: Esp32s31ConnectedStaReport,
}

/// One station role owner lent independently to common RX, TX and control
/// turns. None of its fields owns physical DMA, IRQ or network endpoints.
pub struct Esp32s31StaApStationRole<Rx, Tx, Control> {
    rx: Rx,
    tx: Tx,
    control: Control,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationTxOwnershipError {
    AlreadyActive,
    AlreadyParked,
    Busy,
    Physical(WdevPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationTxError {
    Operation(AggregateTxError),
    Ownership(Esp32s31StaApStationTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationControlError {
    Operation(ConnectedControlError),
    Ownership(Esp32s31StaApStationTxOwnershipError),
}

type Esp32s31StaApStationBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

type Esp32s31StaApConnectedTx<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> = Esp32s31ConnectedTx<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    QUEUE_DEPTH,
    SLOTS,
    AMPDU_BUFFER_SIZE,
    ORDINARY_BUFFER_SIZE,
>;

type Esp32s31StaApStationPhysicalTx<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> = WdevPairedPhysicalTx<
    WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    AggregateTxResources<
        'ampdu,
        Esp32s31StaApStationBacking<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        SLOTS,
        AMPDU_BUFFER_SIZE,
    >,
>;

/// Establish the station side of the paired ownership graph. The returned
/// role retains association policy only; the exact ordinary/A-MPDU pair is
/// held once by `WdevPairedPhysicalTx`.
#[allow(clippy::type_complexity)]
pub fn park_sta_ap_station_role<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    Rx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>(
    role: Esp32s31StaApStationRole<
        Rx,
        Esp32s31ConnectedTx<
            'slot,
            'ampdu,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
    >,
) -> Result<
    (
        Esp32s31StaApStationRole<
            Rx,
            WdevPairedRoleOwner<
                Esp32s31ConnectedTx<
                    'slot,
                    'ampdu,
                    'resources,
                    M,
                    P,
                    E,
                    T,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ),
    Esp32s31StaApStationRole<
        Rx,
        Esp32s31ConnectedTx<
            'slot,
            'ampdu,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
    >,
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    let Esp32s31StaApStationRole { rx, tx, control } = role;
    match tx.try_park() {
        Ok((ordinary, aggregate, parked)) => Ok((
            Esp32s31StaApStationRole {
                rx,
                tx: WdevPairedRoleOwner::parked(parked),
                control,
            },
            WdevPairedPhysicalTx::new(ordinary, aggregate),
        )),
        Err(tx) => Err(Esp32s31StaApStationRole { rx, tx, control }),
    }
}

/// Remove the standalone connected-services boundary and establish the STA
/// half of one paired WDEV composition.
///
/// This is the cutover transaction used by production integration: hardware
/// and DMA stay role-neutral, station protocol/control become one logical
/// role, and ordinary plus A-MPDU resources acquire exactly one physical
/// owner. No network or interrupt runner is created here.
#[allow(clippy::type_complexity)]
pub fn prepare_sta_ap_station<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    H,
    PhysicalRx,
    Protocol,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>(
    drivers: Esp32s31ConnectedStaDrivers<
        H,
        PhysicalRx,
        Esp32s31ConnectedTx<
            'slot,
            'ampdu,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
        Protocol,
    >,
) -> Result<
    Esp32s31StaApStationPrepared<
        H,
        PhysicalRx,
        Esp32s31StaApStationRole<
            Protocol,
            WdevPairedRoleOwner<
                Esp32s31ConnectedTx<
                    'slot,
                    'ampdu,
                    'resources,
                    M,
                    P,
                    E,
                    T,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >,
    Esp32s31StaApStationPrepareFailure<
        H,
        PhysicalRx,
        Esp32s31StaApStationRole<
            Protocol,
            Esp32s31ConnectedTx<
                'slot,
                'ampdu,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Control,
        >,
    >,
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    let Esp32s31ConnectedStaDrivers {
        services,
        protocol,
        report,
    } = drivers;
    let (hardware, physical_rx, tx, control) = services.into_parts();
    match park_sta_ap_station_role(Esp32s31StaApStationRole::new(protocol, tx, control)) {
        Ok((station, physical_tx)) => Ok(Esp32s31StaApStationPrepared {
            hardware,
            physical_rx,
            station,
            physical_tx,
            report,
        }),
        Err(station) => Err(Esp32s31StaApStationPrepareFailure {
            hardware,
            physical_rx,
            station,
            report,
        }),
    }
}

impl<Rx, Tx, Control> Esp32s31StaApStationRole<Rx, Tx, Control> {
    pub const fn new(rx: Rx, tx: Tx, control: Control) -> Self {
        Self { rx, tx, control }
    }

    pub const fn rx(&self) -> &Rx {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut Rx {
        &mut self.rx
    }

    pub const fn tx(&self) -> &Tx {
        &self.tx
    }

    pub fn tx_mut(&mut self) -> &mut Tx {
        &mut self.tx
    }

    pub const fn control(&self) -> &Control {
        &self.control
    }

    pub fn control_mut(&mut self) -> &mut Control {
        &mut self.control
    }

    pub fn control_and_tx_mut(&mut self) -> (&mut Control, &mut Tx) {
        (&mut self.control, &mut self.tx)
    }

    pub fn into_parts(self) -> (Rx, Tx, Control) {
        (self.rx, self.tx, self.control)
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    Rx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    Esp32s31StaApStationRole<
        Rx,
        WdevPairedRoleOwner<
            Esp32s31ConnectedTx<
                'slot,
                'ampdu,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
        >,
        Control,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    fn activate_tx(
        &mut self,
        physical: &mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31StaApStationTxOwnershipError> {
        if !self.tx.is_parked() {
            return Err(Esp32s31StaApStationTxOwnershipError::AlreadyActive);
        }
        let (ordinary, aggregate) = physical
            .try_lend(WdevPairRole::First)
            .map_err(Esp32s31StaApStationTxOwnershipError::Physical)?;
        self.tx
            .try_activate(|parked| {
                Ok::<_, (core::convert::Infallible, _)>(Esp32s31ConnectedTx::resume(
                    ordinary, aggregate, parked,
                ))
            })
            .map_err(|error| match error {
                WdevPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyActive
                }
                WdevPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked
                }
                WdevPairedRoleTransitionError::Conversion(never) => match never {},
            })
    }

    fn park_tx(
        &mut self,
        physical: &mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31StaApStationTxOwnershipError> {
        if self.tx.is_parked() {
            return Err(Esp32s31StaApStationTxOwnershipError::AlreadyParked);
        }
        self.tx
            .try_park(|active| match active.try_park() {
                Ok((ordinary, aggregate, parked)) => {
                    match physical.restore(WdevPairRole::First, ordinary, aggregate) {
                        Ok(()) => Ok(parked),
                        Err((error, ordinary, aggregate)) => Err((
                            Esp32s31StaApStationTxOwnershipError::Physical(error),
                            Esp32s31ConnectedTx::resume(ordinary, aggregate, parked),
                        )),
                    }
                }
                Err(active) => Err((Esp32s31StaApStationTxOwnershipError::Busy, active)),
            })
            .map_err(|error| match error {
                WdevPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyActive
                }
                WdevPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked
                }
                WdevPairedRoleTransitionError::Conversion(error) => error,
            })
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    'control,
    M,
    H,
    P,
    E,
    T,
    Rx,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
    const CONTROL_CAPACITY: usize,
>
    super::Esp32s31StaApStationControlRole<
        H,
        Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >
    for Esp32s31StaApStationRole<
        Rx,
        WdevPairedRoleOwner<
            Esp32s31ConnectedTx<
                'slot,
                'ampdu,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
        >,
        Esp32s31ConnectedControl<'control, M, CONTROL_CAPACITY>,
    >
where
    M: RawMutex,
    H: ConnectedControlHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    type Error = Esp32s31StaApStationControlError;
    type Exit = open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;

    fn service_station_control<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            if self.tx.is_parked() {
                self.activate_tx(physical)
                    .map_err(Esp32s31StaApStationControlError::Ownership)?;
            }
            let result = self
                .control
                .service_with_context(
                    hardware,
                    self.tx
                        .active_mut()
                        .expect("station control owns the physical TX pair"),
                    context,
                )
                .await
                .map_err(Esp32s31StaApStationControlError::Operation);
            let tx_pending = self.tx.active().is_some_and(Esp32s31ConnectedTx::active);
            if !tx_pending
                && let Err(error) = self.park_tx(physical)
                && result.is_ok()
            {
                return Err(Esp32s31StaApStationControlError::Ownership(error));
            }
            result
        }
    }

    fn wait_station_control_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.control.wait_ready_without_tx()
    }
}

impl<'pool, Rx, Tx, Control, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS> for Esp32s31StaApStationRole<Rx, Tx, Control>
where
    Rx: Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS>,
{
    type Dispatch = Rx::Dispatch;
    type Error = Rx::Error;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn WdevNetworkRx,
    ) -> Result<WdevRxProgress, Self::Error> {
        self.rx.publish_pending_rx(network)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        network: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        self.rx.service_station_rx(frame, network)
    }

    fn has_pending_rx(&self) -> bool {
        self.rx.has_pending_rx()
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    Rx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    WdevPairedNetworkTxService<
        'resources,
        M,
        H,
        Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >
    for Esp32s31StaApStationRole<
        Rx,
        WdevPairedRoleOwner<
            Esp32s31ConnectedTx<
                'slot,
                'ampdu,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
        >,
        Control,
    >
where
    M: RawMutex,
    H: HtAmpduHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    type Error = Esp32s31StaApStationTxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
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
            if self.tx.is_parked() {
                self.activate_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            let progress = self
                .tx
                .active_mut()
                .expect("station network TX owns the physical pair")
                .start(hardware, frame, network)
                .await
                .map_err(Esp32s31StaApStationTxError::Operation)?;
            if progress == WifiTxProgress::Complete {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn wait_deadline<'a>(
        &'a mut self,
        _physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            self.tx
                .active_mut()
                .expect("paired scheduler retains station TX until terminal")
                .wait_deadline()
                .await;
        }
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = self
                .tx
                .active_mut()
                .ok_or(Esp32s31StaApStationTxError::Ownership(
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked,
                ))?
                .service(hardware, wake)
                .await
                .map_err(Esp32s31StaApStationTxError::Operation)?;
            if progress == WifiTxProgress::Complete {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn has_prepared(&self) -> bool {
        self.tx
            .active()
            .is_some_and(Esp32s31ConnectedTx::has_prepared_network_tx)
    }

    fn preferred_batch_size(&self) -> usize {
        self.tx
            .active()
            .map_or(1, Esp32s31ConnectedTx::preferred_network_batch_size)
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx
            .active()
            .map_or(0, Esp32s31ConnectedTx::prepared_network_frame_count)
    }

    fn start_prepared<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
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
            let progress = self
                .tx
                .active_mut()
                .ok_or(Esp32s31StaApStationTxError::Ownership(
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked,
                ))?
                .start_prepared(hardware, network)
                .await
                .map_err(Esp32s31StaApStationTxError::Operation)?;
            if progress == WifiTxProgress::Complete {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn cancel_prepared(
        &mut self,
        physical: &mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<(), Self::Error> {
        <Esp32s31StaApConnectedTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        > as WdevNetworkTxService<
            'resources,
            M,
            H,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >>::cancel_prepared(self.tx.active_mut().ok_or(
            Esp32s31StaApStationTxError::Ownership(
                Esp32s31StaApStationTxOwnershipError::AlreadyParked,
            ),
        )?)
        .map_err(Esp32s31StaApStationTxError::Operation)?;
        self.park_tx(physical)
            .map_err(Esp32s31StaApStationTxError::Ownership)
    }

    fn can_prepare(
        &self,
        _physical: &Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> bool {
        self.tx.active().is_some_and(|active| {
            <Esp32s31StaApConnectedTx<
                'resources,
                'slot,
                'ampdu,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            > as WdevNetworkTxService<
                'resources,
                M,
                H,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >>::can_prepare(active)
        })
    }

    fn prepare<'a>(
        &'a mut self,
        physical: &'a mut Esp32s31StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
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
        async move {
            if self.tx.is_parked() {
                self.activate_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            <Esp32s31StaApConnectedTx<
                'resources,
                'slot,
                'ampdu,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            > as WdevNetworkTxService<
                'resources,
                M,
                H,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >>::prepare(
                self.tx
                    .active_mut()
                    .expect("station prepare owns the physical pair"),
                frame,
                network,
            )
            .await
            .map_err(Esp32s31StaApStationTxError::Operation)
        }
    }
}

impl<
    'resources,
    M,
    H,
    Rx,
    Tx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> WdevNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31StaApStationRole<Rx, Tx, Control>
where
    M: RawMutex,
    Tx: WdevNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
{
    type Error = Tx::Error;

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
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx.start(hardware, frame, network)
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.tx.wait_deadline()
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx.service(hardware, wake)
    }

    fn has_prepared(&self) -> bool {
        self.tx.has_prepared()
    }

    fn preferred_batch_size(&self) -> usize {
        self.tx.preferred_batch_size()
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx.prepared_frame_count()
    }

    fn start_prepared<'a>(
        &'a mut self,
        hardware: &'a mut H,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx.start_prepared(hardware, network)
    }

    fn cancel_prepared(&mut self) -> Result<(), Self::Error> {
        self.tx.cancel_prepared()
    }

    fn can_prepare(&self) -> bool {
        self.tx.can_prepare()
    }

    fn prepare<'a>(
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
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
    {
        self.tx.prepare(frame, network)
    }
}
