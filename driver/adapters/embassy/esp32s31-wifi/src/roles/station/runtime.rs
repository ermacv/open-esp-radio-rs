#![expect(
    clippy::manual_async_fn,
    reason = "role implementations retain explicit borrowed Future contracts"
)]
#![expect(
    clippy::result_large_err,
    reason = "no-alloc station park and activation failures return the exact role owners"
)]

//! Connected-station protocol, TX and control ownership for paired DATAPATH.

use core::future::Future;

use open_esp_radio_embassy_net::{
    PinnedNetworkTxFrame, PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex,
};
use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::WifiTxWake,
};
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware;

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;

use crate::{
    datapath::rx::staging::Esp32s31StagedRxFrame,
    datapath::tx::resources::AggregateTxResources,
    datapath::{
        DatapathControlContext, DatapathControlProgress, DatapathRxProgress, WifiTxProgress,
        network::DatapathNetworkRx,
        paired::{
            DatapathPairRole, DatapathPairedNetworkTxService, DatapathPairedPhysicalTx,
            DatapathPairedPhysicalTxError, DatapathPairedRoleOwner,
            DatapathPairedRoleTransitionError,
        },
        services::{DatapathNetworkTxService, SingleRoleServices},
    },
    roles::station::connected::port::{Esp32s31ConnectedStaDrivers, Esp32s31ConnectedStaReport},
    roles::station::tx::{AggregateTxError, Esp32s31ConnectedTx, Esp32s31ConnectedTxParked},
    roles::{
        concurrent::{Esp32s31StaApStationControlRole, Esp32s31StaApStationRxRole},
        station::control::{
            ConnectedControlError, ConnectedControlHardware, Esp32s31ConnectedControl,
        },
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

/// Why a paired station cannot return to the ordinary connected owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationFinishReason {
    StationTxActive,
    PhysicalTxLent(DatapathPairRole),
}

/// Fail-closed paired-to-connected frontier.
///
/// The complete prepared graph is retained unchanged.  A caller may finish
/// the outstanding transaction and retry; it never has to manufacture a TX
/// owner from policy state.
pub struct Esp32s31StaApStationFinishFailure<H, PhysicalRx, Station, PhysicalTx> {
    pub reason: Esp32s31StaApStationFinishReason,
    pub prepared: Esp32s31StaApStationPrepared<H, PhysicalRx, Station, PhysicalTx>,
}

/// One station role owner lent independently to common RX, TX and control
/// turns. None of its fields owns physical DMA, IRQ or network endpoints.
pub type StationRoleRuntime<Rx, Tx, Control> =
    crate::datapath::services::RoleRuntime<Rx, Tx, Control>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationTxOwnershipError {
    AlreadyActive,
    AlreadyParked,
    Busy,
    Physical(DatapathPairedPhysicalTxError),
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
> = DatapathPairedPhysicalTx<
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
/// held once by `DatapathPairedPhysicalTx`.
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
    role: StationRoleRuntime<
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
        StationRoleRuntime<
            Rx,
            DatapathPairedRoleOwner<
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
    StationRoleRuntime<
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
    let (rx, tx, control) = role.into_parts();
    match tx.try_park() {
        Ok((ordinary, aggregate, parked)) => Ok((
            StationRoleRuntime::new(rx, DatapathPairedRoleOwner::parked(parked), control),
            DatapathPairedPhysicalTx::new(ordinary, aggregate),
        )),
        Err(tx) => Err(StationRoleRuntime::new(rx, tx, control)),
    }
}

/// Remove the standalone connected-services boundary and establish the STA
/// half of one paired DATAPATH composition.
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
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
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
        StationRoleRuntime<
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
    let Esp32s31ConnectedStaDrivers { services, report } = drivers;
    let (hardware, rx, tx, control) = services.into_parts();
    let (physical_rx, protocol) = rx.into_parts();
    match park_sta_ap_station_role(StationRoleRuntime::new(protocol, tx, control)) {
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

/// Rejoin a quiescent paired STA role with the exact physical TX resources
/// removed by [`prepare_sta_ap_station`].
///
/// This is the sole inverse cutover transaction.  It succeeds only after the
/// paired DATAPATH has reached an idle terminal edge: neither logical role may
/// retain ordinary or aggregate hardware authority.
#[allow(clippy::type_complexity)]
pub fn finish_sta_ap_station<
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
    prepared: Esp32s31StaApStationPrepared<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
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
) -> Result<
    Esp32s31ConnectedStaDrivers<
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
    Esp32s31StaApStationFinishFailure<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
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
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    'resources: 'ampdu,
{
    if !prepared.station.tx().is_parked() {
        return Err(Esp32s31StaApStationFinishFailure {
            reason: Esp32s31StaApStationFinishReason::StationTxActive,
            prepared,
        });
    }
    if let Some(role) = prepared.physical_tx.lent_to() {
        return Err(Esp32s31StaApStationFinishFailure {
            reason: Esp32s31StaApStationFinishReason::PhysicalTxLent(role),
            prepared,
        });
    }

    let Esp32s31StaApStationPrepared {
        hardware,
        physical_rx,
        station,
        physical_tx,
        report,
    } = prepared;
    let (protocol, tx, control) = station.into_parts();
    let parked = tx
        .try_into_parked()
        .unwrap_or_else(|_| unreachable!("station TX was checked quiescent"));
    let (ordinary, aggregate) = physical_tx
        .try_into_resources()
        .unwrap_or_else(|_| unreachable!("physical TX was checked available"));
    let tx = Esp32s31ConnectedTx::resume(ordinary, aggregate, parked);

    Ok(Esp32s31ConnectedStaDrivers {
        services: SingleRoleServices::with_control(
            hardware,
            crate::roles::station::connected::Esp32s31ConnectedStaRxService::new(
                physical_rx,
                protocol,
            ),
            tx,
            control,
        ),
        report,
    })
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
    StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
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
        if !self.tx_mut().is_parked() {
            return Err(Esp32s31StaApStationTxOwnershipError::AlreadyActive);
        }
        let (ordinary, aggregate) = physical
            .try_lend(DatapathPairRole::First)
            .map_err(Esp32s31StaApStationTxOwnershipError::Physical)?;
        self.tx_mut()
            .try_activate(|parked| {
                Ok::<_, (core::convert::Infallible, _)>(Esp32s31ConnectedTx::resume(
                    ordinary, aggregate, parked,
                ))
            })
            .map_err(|error| match error {
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(never) => match never {},
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
        if self.tx_mut().is_parked() {
            return Err(Esp32s31StaApStationTxOwnershipError::AlreadyParked);
        }
        self.tx_mut()
            .try_park(|active| match active.try_park() {
                Ok((ordinary, aggregate, parked)) => {
                    match physical.restore(DatapathPairRole::First, ordinary, aggregate) {
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
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(error) => error,
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
    Esp32s31StaApStationControlRole<
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
    for StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
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
        context: DatapathControlContext,
        retain_physical_tx: bool,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            if self.tx_mut().is_parked() {
                self.activate_tx(physical)
                    .map_err(Esp32s31StaApStationControlError::Ownership)?;
            }
            let result = {
                let (control, tx) = self.control_and_tx_mut();
                control
                    .service_with_context(
                        hardware,
                        tx.active_mut()
                            .expect("station control owns the physical TX pair"),
                        context,
                    )
                    .await
                    .map_err(Esp32s31StaApStationControlError::Operation)
            };
            let tx_pending = self
                .tx_mut()
                .active()
                .is_some_and(Esp32s31ConnectedTx::active);
            if !tx_pending
                && !retain_physical_tx
                && let Err(error) = self.park_tx(physical)
                && result.is_ok()
            {
                return Err(Esp32s31StaApStationControlError::Ownership(error));
            }
            result
        }
    }

    fn station_control_ready(&self, now_micros: u64) -> bool {
        self.control().has_immediate_work()
            || self
                .control()
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= now_micros)
    }

    fn wait_station_control_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.control_mut().wait_ready_without_tx()
    }
}

impl<'pool, Rx, Tx, Control, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS> for StationRoleRuntime<Rx, Tx, Control>
where
    Rx: Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS>,
{
    type Dispatch = Rx::Dispatch;
    type Error = Rx::Error;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        self.rx_mut().publish_pending_rx(network)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        self.rx_mut().service_station_rx(frame, network)
    }

    fn has_pending_rx(&self) -> bool {
        self.rx().has_pending_rx()
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
    DatapathPairedNetworkTxService<
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
    for StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
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

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_radio_snapshot(
        &self,
        physical: &Esp32s31StaApStationPhysicalTx<
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
        demand: open_esp_radio_wifi_softmac::WifiEgressDemand<
            open_esp_radio_embassy_net::EgressKey,
        >,
    ) -> Option<crate::datapath::egress::DatapathHtEgressSnapshot> {
        if let Some(active) = self.tx().active() {
            return active.egress_radio_snapshot(demand);
        }
        let Some((ordinary, _)) = physical.available() else {
            return crate::datapath::egress::rejected_ht_egress_snapshot(
                crate::datapath::egress::DatapathEgressSnapshotRejection::RoleUnavailable,
            );
        };
        let Some(parked) = self.tx().parked_state() else {
            return crate::datapath::egress::rejected_ht_egress_snapshot(
                crate::datapath::egress::DatapathEgressSnapshotRejection::RoleUnavailable,
            );
        };
        parked.egress_radio_snapshot(
            demand,
            FRAME_CAPACITY,
            ordinary.policy.ht_ampdu().maximum_aggregate_bytes(),
        )
    }

    fn last_started_frame_count(&self) -> usize {
        self.tx()
            .active()
            .map_or(1, Esp32s31ConnectedTx::active_network_frame_count)
    }

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
        frame: PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
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
            if self.tx_mut().is_parked() {
                self.activate_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            let progress = self
                .tx_mut()
                .active_mut()
                .expect("station network TX owns the physical pair")
                .start(hardware, frame, network)
                .await
                .map_err(Esp32s31StaApStationTxError::Operation)?;
            let retained = self
                .tx()
                .active()
                .is_some_and(Esp32s31ConnectedTx::has_prepared_network_tx);
            if progress == WifiTxProgress::Complete && !retained {
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
            self.tx_mut()
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
                .tx_mut()
                .active_mut()
                .ok_or(Esp32s31StaApStationTxError::Ownership(
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked,
                ))?
                .service(hardware, wake)
                .await
                .map_err(Esp32s31StaApStationTxError::Operation)?;
            let retained = self
                .tx()
                .active()
                .is_some_and(Esp32s31ConnectedTx::has_prepared_network_tx);
            if progress == WifiTxProgress::Complete && !retained {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn has_prepared(&self) -> bool {
        self.tx()
            .active()
            .is_some_and(Esp32s31ConnectedTx::has_prepared_network_tx)
    }

    fn preferred_batch_size(&self) -> usize {
        self.tx()
            .active()
            .map_or(1, Esp32s31ConnectedTx::preferred_network_batch_size)
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx()
            .active()
            .map_or(0, Esp32s31ConnectedTx::prepared_network_frame_count)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        if let Some(tx) = self.tx_mut().active_mut() {
            tx.mark_prepared_scheduler_phase(phase, at_micros);
        }
    }

    fn start_prepared(
        &mut self,
        hardware: &mut H,
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
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        let progress = self
            .tx_mut()
            .active_mut()
            .ok_or(Esp32s31StaApStationTxError::Ownership(
                Esp32s31StaApStationTxOwnershipError::AlreadyParked,
            ))?
            .start_prepared(hardware, network)
            .map_err(Esp32s31StaApStationTxError::Operation)?;
        let retained = self
            .tx()
            .active()
            .is_some_and(Esp32s31ConnectedTx::has_prepared_network_tx);
        if progress == WifiTxProgress::Complete && !retained {
            self.park_tx(physical)
                .map_err(Esp32s31StaApStationTxError::Ownership)?;
        }
        Ok(progress)
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
        network: Option<
            &PinnedTxInterfaceConsumer<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
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
        > as DatapathNetworkTxService<
            'resources,
            M,
            H,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >>::cancel_prepared(
            self.tx_mut()
                .active_mut()
                .ok_or(Esp32s31StaApStationTxError::Ownership(
                    Esp32s31StaApStationTxOwnershipError::AlreadyParked,
                ))?,
            network,
        )
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
        self.tx().active().is_some_and(|active| {
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
            > as DatapathNetworkTxService<
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
        frame: PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
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
            if self.tx_mut().is_parked() {
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
            > as DatapathNetworkTxService<
                'resources,
                M,
                H,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >>::prepare(
                self.tx_mut()
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
> DatapathNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for StationRoleRuntime<Rx, Tx, Control>
where
    M: RawMutex,
    Tx: DatapathNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
{
    type Error = Tx::Error;

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_radio_snapshot(
        &self,
        demand: open_esp_radio_wifi_softmac::WifiEgressDemand<
            open_esp_radio_embassy_net::EgressKey,
        >,
    ) -> Option<crate::datapath::egress::DatapathHtEgressSnapshot> {
        self.tx().egress_radio_snapshot(demand)
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx_mut().start(hardware, frame, network)
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.tx_mut().wait_deadline()
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx_mut().service(hardware, wake)
    }

    fn has_prepared(&self) -> bool {
        self.tx().has_prepared()
    }

    fn preferred_batch_size(&self) -> usize {
        self.tx().preferred_batch_size()
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx().prepared_frame_count()
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        self.tx_mut()
            .mark_prepared_scheduler_phase(phase, at_micros);
    }

    fn start_prepared(
        &mut self,
        hardware: &mut H,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        self.tx_mut().start_prepared(hardware, network)
    }

    fn cancel_prepared(
        &mut self,
        network: Option<
            &PinnedTxInterfaceConsumer<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
        >,
    ) -> Result<(), Self::Error> {
        self.tx_mut().cancel_prepared(network)
    }

    fn can_prepare(&self) -> bool {
        self.tx().can_prepare()
    }

    fn prepare<'a>(
        &'a mut self,
        frame: PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
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
        self.tx_mut().prepare(frame, network)
    }
}
