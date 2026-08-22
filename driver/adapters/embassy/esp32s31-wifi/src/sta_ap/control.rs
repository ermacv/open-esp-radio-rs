//! Typed control arbitration for one same-channel station plus SoftAP pair.

use core::future::Future;

use embassy_futures::select::{Either, select};
use embassy_time::{Instant, Timer};

use crate::wdev::{
    WdevControlContext, WdevControlProgress,
    paired::{
        WdevPairRole, WdevPairedControlProgress, WdevPairedControlService, WdevPairedStopProgress,
    },
};

pub trait Esp32s31StaApStationControlRole<H, PhysicalTx> {
    type Error;
    type Exit;

    fn service_station_control<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        context: WdevControlContext,
        retain_physical_tx: bool,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a;

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
    type Error;

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
    ) -> Result<WdevPairedStopProgress, Self::Error>;

    fn next_access_point_control_delay_millis(&self, now_micros: u64) -> Result<u32, Self::Error>;
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
    next_access_point_delay_millis: u32,
}

impl Esp32s31StaApControlArbiter {
    pub const fn new() -> Self {
        Self {
            next_access_point_delay_millis: 0,
        }
    }

    fn map_access_point_progress<E>(
        progress: Esp32s31StaApAccessPointControlProgress,
    ) -> WdevPairedControlProgress<E> {
        match progress {
            Esp32s31StaApAccessPointControlProgress::Idle => WdevPairedControlProgress::Idle,
            Esp32s31StaApAccessPointControlProgress::More => WdevPairedControlProgress::More,
            Esp32s31StaApAccessPointControlProgress::TxPending => {
                WdevPairedControlProgress::TxPending(WdevPairRole::Second)
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
    WdevPairedControlService<H, PhysicalTx, Station, AccessPoint> for Esp32s31StaApControlArbiter
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
        context: WdevControlContext,
        retained_tx: Option<WdevPairRole>,
    ) -> impl Future<Output = Result<WdevPairedControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            let now = Instant::now().as_micros();
            if retained_tx == Some(WdevPairRole::Second) {
                return access_point
                    .service_access_point_control(hardware, physical_tx, now, true)
                    .map(Self::map_access_point_progress)
                    .map_err(Esp32s31StaApControlError::AccessPoint);
            }
            if retained_tx == Some(WdevPairRole::First) {
                return match station
                    .service_station_control(hardware, physical_tx, context, true)
                    .await
                    .map_err(Esp32s31StaApControlError::Station)?
                {
                    WdevControlProgress::Idle => Ok(WdevPairedControlProgress::Idle),
                    WdevControlProgress::More => Ok(WdevPairedControlProgress::More),
                    WdevControlProgress::TxPending => {
                        Ok(WdevPairedControlProgress::TxPending(WdevPairRole::First))
                    }
                    WdevControlProgress::Exit(exit) => Ok(WdevPairedControlProgress::Exit(
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
                WdevControlProgress::Idle => {}
                WdevControlProgress::More => return Ok(WdevPairedControlProgress::More),
                WdevControlProgress::TxPending => {
                    return Ok(WdevPairedControlProgress::TxPending(WdevPairRole::First));
                }
                WdevControlProgress::Exit(exit) => {
                    return Ok(WdevPairedControlProgress::Exit(
                        Esp32s31StaApControlExit::Station(exit),
                    ));
                }
            }

            let now = Instant::now().as_micros();
            let progress = access_point
                .service_access_point_control(hardware, physical_tx, now, false)
                .map_err(Esp32s31StaApControlError::AccessPoint)?;
            if progress == Esp32s31StaApAccessPointControlProgress::Idle {
                self.next_access_point_delay_millis = access_point
                    .next_access_point_control_delay_millis(now)
                    .map_err(Esp32s31StaApControlError::AccessPoint)?;
            }
            Ok(Self::map_access_point_progress(progress))
        }
    }

    fn stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        _station: &mut Station,
        access_point: &mut AccessPoint,
    ) -> Result<WdevPairedStopProgress, Self::Error> {
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
                Timer::after_millis(u64::from(self.next_access_point_delay_millis)),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{convert::Infallible, future::pending};

    use super::*;

    struct Station {
        calls: usize,
        progress: WdevControlProgress<u8>,
    }

    impl Esp32s31StaApStationControlRole<(), ()> for Station {
        type Error = Infallible;
        type Exit = u8;

        fn service_station_control<'a>(
            &'a mut self,
            _hardware: &'a mut (),
            _physical_tx: &'a mut (),
            _context: WdevControlContext,
            _retain_physical_tx: bool,
        ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
        {
            async move {
                self.calls += 1;
                Ok(self.progress)
            }
        }

        fn wait_station_control_ready(&mut self) -> impl Future<Output = ()> + '_ {
            pending()
        }
    }

    struct AccessPoint {
        due: bool,
        calls: usize,
        progress: Esp32s31StaApAccessPointControlProgress,
    }

    impl Esp32s31StaApAccessPointControlRole<(), ()> for AccessPoint {
        type Error = Infallible;

        fn beacon_publication_due(&self, _now_micros: u32) -> bool {
            self.due
        }

        fn service_access_point_control(
            &mut self,
            _hardware: &mut (),
            _physical_tx: &mut (),
            _now_micros: u64,
            _retain_physical_tx: bool,
        ) -> Result<Esp32s31StaApAccessPointControlProgress, Self::Error> {
            self.calls += 1;
            Ok(self.progress)
        }

        fn service_access_point_stop(
            &mut self,
            _hardware: &mut (),
            _physical_tx: &mut (),
        ) -> Result<WdevPairedStopProgress, Self::Error> {
            Ok(WdevPairedStopProgress::Stopped)
        }

        fn next_access_point_control_delay_millis(
            &self,
            _now_micros: u64,
        ) -> Result<u32, Self::Error> {
            Ok(1)
        }
    }

    fn service(
        station: &mut Station,
        access_point: &mut AccessPoint,
    ) -> WdevPairedControlProgress<Esp32s31StaApControlExit<u8>> {
        service_with_retained(station, access_point, None)
    }

    fn service_with_retained(
        station: &mut Station,
        access_point: &mut AccessPoint,
        retained: Option<WdevPairRole>,
    ) -> WdevPairedControlProgress<Esp32s31StaApControlExit<u8>> {
        embassy_futures::block_on(WdevPairedControlService::service(
            &mut Esp32s31StaApControlArbiter::new(),
            &mut (),
            &mut (),
            station,
            access_point,
            WdevControlContext::IDLE,
            retained,
        ))
        .unwrap()
    }

    #[test]
    fn due_beacon_owns_tx_before_station_control_is_polled() {
        let mut station = Station {
            calls: 0,
            progress: WdevControlProgress::TxPending,
        };
        let mut access_point = AccessPoint {
            due: true,
            calls: 0,
            progress: Esp32s31StaApAccessPointControlProgress::TxPending,
        };

        assert_eq!(
            service(&mut station, &mut access_point),
            WdevPairedControlProgress::TxPending(WdevPairRole::Second)
        );
        assert_eq!(station.calls, 0);
        assert_eq!(access_point.calls, 1);
    }

    #[test]
    fn station_control_tx_retains_first_role_identity() {
        let mut station = Station {
            calls: 0,
            progress: WdevControlProgress::TxPending,
        };
        let mut access_point = AccessPoint {
            due: false,
            calls: 0,
            progress: Esp32s31StaApAccessPointControlProgress::TxPending,
        };

        assert_eq!(
            service(&mut station, &mut access_point),
            WdevPairedControlProgress::TxPending(WdevPairRole::First)
        );
        assert_eq!(station.calls, 1);
        assert_eq!(access_point.calls, 0);
    }

    #[test]
    fn idle_station_yields_same_turn_to_access_point_control() {
        let mut station = Station {
            calls: 0,
            progress: WdevControlProgress::Idle,
        };
        let mut access_point = AccessPoint {
            due: false,
            calls: 0,
            progress: Esp32s31StaApAccessPointControlProgress::TxPending,
        };

        assert_eq!(
            service(&mut station, &mut access_point),
            WdevPairedControlProgress::TxPending(WdevPairRole::Second)
        );
        assert_eq!(station.calls, 1);
        assert_eq!(access_point.calls, 1);
    }

    #[test]
    fn retained_station_tx_excludes_access_point_control() {
        let mut station = Station {
            calls: 0,
            progress: WdevControlProgress::Idle,
        };
        let mut access_point = AccessPoint {
            due: true,
            calls: 0,
            progress: Esp32s31StaApAccessPointControlProgress::TxPending,
        };

        assert_eq!(
            service_with_retained(&mut station, &mut access_point, Some(WdevPairRole::First)),
            WdevPairedControlProgress::Idle
        );
        assert_eq!(station.calls, 1);
        assert_eq!(access_point.calls, 0);
    }

    #[test]
    fn retained_access_point_tx_excludes_station_control() {
        let mut station = Station {
            calls: 0,
            progress: WdevControlProgress::TxPending,
        };
        let mut access_point = AccessPoint {
            due: false,
            calls: 0,
            progress: Esp32s31StaApAccessPointControlProgress::Idle,
        };

        assert_eq!(
            service_with_retained(&mut station, &mut access_point, Some(WdevPairRole::Second)),
            WdevPairedControlProgress::Idle
        );
        assert_eq!(station.calls, 0);
        assert_eq!(access_point.calls, 1);
    }
}
