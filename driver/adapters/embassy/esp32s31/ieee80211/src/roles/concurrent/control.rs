#![expect(
    clippy::manual_async_fn,
    reason = "the concurrent control adapter keeps its borrowed Future service contract explicit"
)]

//! Typed control arbitration for one same-channel station plus SoftAP pair.

use core::future::Future;

use embassy_futures::select::{Either, select};
use embassy_time::{Instant, Timer};

use crate::datapath::{
    DatapathControlContext, DatapathControlProgress,
    paired::{
        DatapathPairRole, DatapathPairedControlProgress, DatapathPairedControlService,
        DatapathPairedStopProgress,
    },
};

pub trait Esp32s31StaApStationControlRole<H, PhysicalTx> {
    type Error: 'static;
    type Exit: 'static;

    fn service_station_control<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        context: DatapathControlContext,
        retain_physical_tx: bool,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a;

    fn station_control_ready(&self, now_micros: u64) -> bool;

    /// Wait for role-local work without borrowing the shared physical TX.
    /// Holding that owner across a sleep would prevent the AP beacon timer
    /// from beginning its own finite transaction.
    fn wait_station_control_ready(&mut self) -> impl Future<Output = ()> + '_;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointControlProgress {
    Idle,
    More,
    TxPending,
}

pub trait Esp32s31StaApAccessPointControlRole<H, PhysicalTx> {
    type Error: 'static;

    fn beacon_publication_due(&self, now_micros: u32) -> bool;

    fn service_access_point_control(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        now_micros: u64,
        retain_physical_tx: bool,
    ) -> Result<Esp32s31StaApAccessPointControlProgress, Self::Error>;

    fn service_access_point_stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
    ) -> Result<DatapathPairedStopProgress, Self::Error>;

    fn next_access_point_control_deadline_micros(
        &self,
        now_micros: u64,
    ) -> Result<u64, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApControlError<StationError, AccessPointError> {
    Station(StationError),
    AccessPoint(AccessPointError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApControlExit<StationExit> {
    Station(StationExit),
}

/// Minimal policy owner above the two role-local control machines.
///
/// A beacon whose publication edge is already due is serviced before other
/// control work. At all other idle boundaries the station gets one finite
/// step followed by one AP step. The arbiter never inspects protocol internals
/// and reports the exact role for every resulting hardware TX transaction.
pub struct Esp32s31StaApControlArbiter {
    next_access_point_deadline_micros: u64,
}

impl Esp32s31StaApControlArbiter {
    pub const fn new() -> Self {
        Self {
            next_access_point_deadline_micros: 0,
        }
    }

    fn map_access_point_progress<E>(
        progress: Esp32s31StaApAccessPointControlProgress,
    ) -> DatapathPairedControlProgress<E> {
        match progress {
            Esp32s31StaApAccessPointControlProgress::Idle => DatapathPairedControlProgress::Idle,
            Esp32s31StaApAccessPointControlProgress::More => DatapathPairedControlProgress::More,
            Esp32s31StaApAccessPointControlProgress::TxPending => {
                DatapathPairedControlProgress::TxPending(DatapathPairRole::Second)
            }
        }
    }
}

impl Default for Esp32s31StaApControlArbiter {
    fn default() -> Self {
        Self::new()
    }
}

impl<H, PhysicalTx, Station, AccessPoint>
    DatapathPairedControlService<H, PhysicalTx, Station, AccessPoint>
    for Esp32s31StaApControlArbiter
where
    Station: Esp32s31StaApStationControlRole<H, PhysicalTx>,
    AccessPoint: Esp32s31StaApAccessPointControlRole<H, PhysicalTx>,
{
    type Error = Esp32s31StaApControlError<Station::Error, AccessPoint::Error>;
    type Exit = Esp32s31StaApControlExit<Station::Exit>;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        context: DatapathControlContext,
        retained_tx: Option<DatapathPairRole>,
    ) -> impl Future<Output = Result<DatapathPairedControlProgress<Self::Exit>, Self::Error>> + 'a
    {
        async move {
            let now = Instant::now().as_micros();
            if retained_tx == Some(DatapathPairRole::Second) {
                let progress = access_point
                    .service_access_point_control(hardware, physical_tx, now, true)
                    .map_err(Esp32s31StaApControlError::AccessPoint)?;
                if progress == Esp32s31StaApAccessPointControlProgress::Idle {
                    self.next_access_point_deadline_micros = access_point
                        .next_access_point_control_deadline_micros(now)
                        .map_err(Esp32s31StaApControlError::AccessPoint)?;
                }
                return Ok(Self::map_access_point_progress(progress));
            }
            if retained_tx == Some(DatapathPairRole::First) {
                return match station
                    .service_station_control(hardware, physical_tx, context, true)
                    .await
                    .map_err(Esp32s31StaApControlError::Station)?
                {
                    DatapathControlProgress::Idle => Ok(DatapathPairedControlProgress::Idle),
                    DatapathControlProgress::More => Ok(DatapathPairedControlProgress::More),
                    DatapathControlProgress::TxPending => Ok(
                        DatapathPairedControlProgress::TxPending(DatapathPairRole::First),
                    ),
                    DatapathControlProgress::Exit(exit) => Ok(DatapathPairedControlProgress::Exit(
                        Esp32s31StaApControlExit::Station(exit),
                    )),
                };
            }
            if access_point.beacon_publication_due(now as u32) {
                let progress = access_point
                    .service_access_point_control(hardware, physical_tx, now, false)
                    .map_err(Esp32s31StaApControlError::AccessPoint)?;
                if progress != Esp32s31StaApAccessPointControlProgress::Idle {
                    return Ok(Self::map_access_point_progress(progress));
                }
            }

            match station
                .service_station_control(hardware, physical_tx, context, false)
                .await
                .map_err(Esp32s31StaApControlError::Station)?
            {
                DatapathControlProgress::Idle => {}
                DatapathControlProgress::More => return Ok(DatapathPairedControlProgress::More),
                DatapathControlProgress::TxPending => {
                    return Ok(DatapathPairedControlProgress::TxPending(
                        DatapathPairRole::First,
                    ));
                }
                DatapathControlProgress::Exit(exit) => {
                    return Ok(DatapathPairedControlProgress::Exit(
                        Esp32s31StaApControlExit::Station(exit),
                    ));
                }
            }

            let now = Instant::now().as_micros();
            let progress = access_point
                .service_access_point_control(hardware, physical_tx, now, false)
                .map_err(Esp32s31StaApControlError::AccessPoint)?;
            if progress == Esp32s31StaApAccessPointControlProgress::Idle {
                self.next_access_point_deadline_micros = access_point
                    .next_access_point_control_deadline_micros(now)
                    .map_err(Esp32s31StaApControlError::AccessPoint)?;
            }
            Ok(Self::map_access_point_progress(progress))
        }
    }

    fn ready(&self, station: &Station, _access_point: &AccessPoint, now_micros: u64) -> bool {
        station.station_control_ready(now_micros)
            || now_micros >= self.next_access_point_deadline_micros
    }

    fn stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        _station: &mut Station,
        access_point: &mut AccessPoint,
    ) -> Result<DatapathPairedStopProgress, Self::Error> {
        access_point
            .service_access_point_stop(hardware, physical_tx)
            .map_err(Esp32s31StaApControlError::AccessPoint)
    }

    fn wait_ready<'a>(
        &'a mut self,
        _physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        _access_point: &'a mut AccessPoint,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            match select(
                station.wait_station_control_ready(),
                Timer::at(Instant::from_micros(self.next_access_point_deadline_micros)),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
