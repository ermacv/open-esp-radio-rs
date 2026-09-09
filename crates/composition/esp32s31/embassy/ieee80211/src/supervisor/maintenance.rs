//! PHY maintenance at a completed logical role boundary.
use super::*;
use oer_esp32s31_phy::state::client::{PhyPllTrackClock, PhyTrackTimeError};
use oer_esp32s31_phy::tracking::schedule::Schedule;
use oer_esp32s31_wifi::runtime::{WifiMaintenanceError, WifiMaintenanceFailure};
use oer_esp32s31_wifi_embassy::datapath::maintenance::{StopError, stop_mac};

#[derive(Clone, Copy, Debug)]
pub(super) enum Reason {
    Deadline(PhyTrackTimeError),
    TxNotIdle,
    RxOwner,
    Mac(StopError),
    Rx(oer_esp32s31_wifi_mac::rx::RxRingError),
    RxPrepare(oer_esp32s31_wifi_mac::rx::RxRingError),
    RxStart(oer_esp32s31_wifi_mac::rx::RxRingError),
    Interrupt,
    InterruptOwner,
    Phy(WifiMaintenanceError),
}

impl core::fmt::Display for Reason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Deadline(error) => write!(f, "PHY deadline: {error:?}"),
            Self::TxNotIdle => f.write_str("ordinary TX owner is not idle"),
            Self::RxOwner => f.write_str("unexpected physical RX owner"),
            Self::Mac(error) => write!(f, "MAC stop: {error:?}"),
            Self::Rx(error) => write!(f, "RX stop: {error:?}"),
            Self::RxPrepare(error) => write!(f, "RX prepare after tracking: {error:?}"),
            Self::RxStart(error) => write!(f, "RX publish after tracking: {error:?}"),
            Self::Interrupt => f.write_str("IRQ quiesce failed"),
            Self::InterruptOwner => f.write_str("inactive IRQ owner unavailable"),
            Self::Phy(error) => write!(f, "PHY tracking: {error:?}"),
        }
    }
}

type Resources = (
    ProductionWifiPhysicalResources,
    ProductionStationRoleResources,
    ProductionAccessPointResources,
    ProductionMonitorResources,
);

#[allow(
    clippy::large_enum_variant,
    reason = "failure retains the allocation-free physical epoch"
)]
enum Owner {
    Radio {
        _owner: ProductionWifiOwner,
    },
    Phy {
        _owner: WifiMaintenanceFailure<EspHalRadioPeripheral>,
    },
}

pub(super) struct Failure {
    _owner: Owner,
    _resources: Resources,
    reason: Reason,
}
impl Failure {
    pub(super) const fn reason(&self) -> Reason {
        self.reason
    }
    fn radio(owner: ProductionWifiOwner, resources: Resources, reason: Reason) -> Self {
        Self {
            _owner: Owner::Radio { _owner: owner },
            _resources: resources,
            reason,
        }
    }
}

/// No connected task runs here: ordinary and aggregate TX resources have
/// returned, while the live RX ring and IRQ route may still be installed.
pub(super) async fn maintain(
    stopped: ProductionSupervisorStopped,
) -> Result<ProductionSupervisorStopped, Failure> {
    let (mut wifi, mut physical, station, access_point, monitor) = stopped.into_parts();
    let snapshot = match &mut wifi {
        ProductionWifiOwner::Cold(owner) => owner.phy_client_snapshot(),
        ProductionWifiOwner::Live { owner, .. } => owner.radio_mut().0.client_snapshot(),
    };
    let mut clock = EmbassyPhyClock;
    let due = match snapshot.tracking_schedule_at(clock.now_micros()) {
        Ok(Schedule::Due(_)) => true,
        Ok(Schedule::Inactive | Schedule::At(_)) => false,
        Err(error) => {
            return Err(Failure::radio(
                wifi,
                (physical, station, access_point, monitor),
                Reason::Deadline(error),
            ));
        }
    };
    if !due {
        return Ok(WifiSupervisorStopped::new(
            wifi,
            physical,
            station,
            access_point,
            monitor,
        ));
    }
    let tx_idle = match &physical.tx {
        ProductionOrdinaryTxResources::Uninitialized(_) => true,
        ProductionOrdinaryTxResources::Epoch(epoch) => {
            epoch.control().is_ok_and(|control| control.is_idle())
        }
    };
    if !tx_idle {
        return Err(Failure::radio(
            wifi,
            (physical, station, access_point, monitor),
            Reason::TxNotIdle,
        ));
    }
    // AggregateTxResources is the idle typestate, already returned by the role.
    let restore_live = matches!(&wifi, ProductionWifiOwner::Live { .. });
    if restore_live && physical.rx_ring.is_none() {
        return Err(Failure::radio(
            wifi,
            (physical, station, access_point, monitor),
            Reason::RxOwner,
        ));
    }
    let stopped = match wifi {
        ProductionWifiOwner::Cold(stopped) => stopped,
        ProductionWifiOwner::Live {
            mut owner,
            mut registers,
            mut interrupts,
        } => {
            if let Err(error) = await_stack_boundary!(stop_mac(&mut registers, &mut clock, 100_000))
            {
                return Err(Failure::radio(
                    ProductionWifiOwner::Live {
                        owner,
                        registers,
                        interrupts,
                    },
                    (physical, station, access_point, monitor),
                    Reason::Mac(error),
                ));
            }
            if let Some(ring) = physical.rx_ring.take() {
                physical.rx_ring = Some(match ring {
                    ProductionRxRing::Halted(ring) => ProductionRxRing::Halted(ring),
                    ProductionRxRing::Live(ring) => match ring.try_stop(&mut registers) {
                        Ok(ring) => ProductionRxRing::Halted(ring),
                        Err((ring, error)) => {
                            physical.rx_ring = Some(ProductionRxRing::Live(ring));
                            return Err(Failure::radio(
                                ProductionWifiOwner::Live {
                                    owner,
                                    registers,
                                    interrupts,
                                },
                                (physical, station, access_point, monitor),
                                Reason::Rx(error),
                            ));
                        }
                    },
                });
            }
            if interrupts.is_active() {
                let (_, platform) = owner.radio_mut();
                if let Err(error) = interrupts.quiesce(platform) {
                    diagnostics_event!("open-radio: PHY IRQ quiesce failed: {:?}", error);
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::Interrupt,
                    ));
                }
            }
            let (_, setup, _, _) = match interrupts.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupts) => {
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::InterruptOwner,
                    ));
                }
            };
            owner.into_stopped(registers, setup, ()).wifi
        }
    };
    let (wifi, outcome) = match await_stack_boundary!(
        stopped.maintain_phy::<EmbassyPhyDelay, _>(&mut clock, NoopPhyTargetObserver)
    ) {
        Ok(result) => result,
        Err(failure) => {
            return Err(Failure {
                reason: Reason::Phy(failure.error()),
                _owner: Owner::Phy { _owner: failure },
                _resources: (physical, station, access_point, monitor),
            });
        }
    };
    diagnostics_event!(
        "open-radio: stopped PHY maintenance completed tracking={} inhibited={}",
        outcome.is_some(),
        outcome.is_some_and(|outcome| outcome.tracking_inhibited)
    );
    let wifi = if restore_live {
        // Restore the exact pre-maintenance live-ring handoff expected by a
        // disconnected STA, without changing its MLME/network resume state.
        let materialized = materialize_production_wifi(ProductionWifiOwner::Cold(wifi), ());
        let mut owner = materialized.owner;
        let mut registers = materialized.registers;
        let mut interrupts = materialized.interrupts;
        let (_, platform) = owner.radio_mut();
        if let Err(error) = interrupts.activate_or_resume_rx_moderated(
            platform,
            oer_esp32s31_wifi_mac::init::MAC_COLD_RX_INTERRUPT_MASK,
        ) {
            diagnostics_event!("open-radio: PHY IRQ restore failed: {:?}", error);
            return Err(Failure::radio(
                ProductionWifiOwner::Live {
                    owner,
                    registers,
                    interrupts,
                },
                (physical, station, access_point, monitor),
                Reason::Interrupt,
            ));
        }
        let ring = match physical.rx_ring.take() {
            Some(ProductionRxRing::Halted(ring)) => ring,
            unexpected => {
                physical.rx_ring = unexpected;
                return Err(Failure::radio(
                    ProductionWifiOwner::Live {
                        owner,
                        registers,
                        interrupts,
                    },
                    (physical, station, access_point, monitor),
                    Reason::RxOwner,
                ));
            }
        };
        {
            let prepared = match physical.dma.storage().prepare_halted(ring, &mut registers) {
                Ok(prepared) => prepared,
                Err((ring, error)) => {
                    physical.rx_ring = Some(ProductionRxRing::Halted(ring));
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::RxPrepare(error),
                    ));
                }
            };
            physical.rx_ring = Some(match prepared.try_start(&mut registers) {
                Ok(ring) => ProductionRxRing::Live(ring),
                Err((prepared, error)) => {
                    physical.rx_ring = Some(ProductionRxRing::Halted(prepared.into_halted()));
                    return Err(Failure::radio(
                        ProductionWifiOwner::Live {
                            owner,
                            registers,
                            interrupts,
                        },
                        (physical, station, access_point, monitor),
                        Reason::RxStart(error),
                    ));
                }
            });
        }
        ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        }
    } else {
        ProductionWifiOwner::Cold(wifi)
    };
    // MAC remains stopped. The next role owns its normal policy and resume.
    Ok(WifiSupervisorStopped::new(
        wifi,
        physical,
        station,
        access_point,
        monitor,
    ))
}
