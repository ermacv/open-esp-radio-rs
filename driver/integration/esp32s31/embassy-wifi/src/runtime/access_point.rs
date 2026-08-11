//! Private ESP32-S31 access-point epoch composition.

use super::*;

type ProductionAccessPointControl = Esp32s31AccessPointControl<
    'static,
    'static,
    'static,
    EmbassyEsp32s31PreconnectedRxDelay,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;
type ProductionPreconnectedRx = Esp32s31PreconnectedRx<
    'static,
    EmbassyEsp32s31PreconnectedRxDelay,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
>;
type ProductionScanRx =
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
type ProductionWifiTxResources = WifiTxResources<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type ProductionAccessPointStopped = EmbassyAccessPointStopped<
    'static,
    'static,
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;

pub(super) struct ProductionAccessPointParked {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_epoch: &'static mut TxStorage,
    scan_table: &'static mut ScanTable,
    resume: ProductionAccessPointStationResume,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
    monitor: ProductionMonitorResources,
}

/// Exact station lifecycle state retained while AP temporarily owns Wi-Fi.
///
/// Only the halted RX ring and the shared TX/network endpoints leave this
/// frontier. The next STA epoch therefore resumes the same allocation and
/// register-arena capabilities; AP switching is not a hidden reset.
enum ProductionAccessPointStationResume {
    Fresh {
        network: StationNetwork,
    },
    Returned {
        phase: ProductionAccessPointReturnedPhase,
        security: Esp32s31StaAttemptSecurity<'static>,
        interrupt_route: EspHalMacInterruptRoute,
        mac_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
        power_runtime: &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex>,
    },
}

enum ProductionAccessPointReturnedPhase {
    InitialScan {
        network: StationNetwork,
        identity: Esp32s31StaIdentity,
    },
    InitialJoin {
        network: StationNetwork,
        station: Esp32s31StaAttemptStation,
    },
    Disconnected {
        network: ConnectedRunningNetwork,
        rx: ConnectedRxEpochResources,
        aggregate_tx: ConnectedAmpduStorage,
        control: &'static ControlResources,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioRegistersRepublish<'static>,
    },
    Reconnected {
        network: StationNetwork,
        rx: ConnectedRxEpochResources,
        aggregate_tx: ConnectedAmpduStorage,
        control: &'static ControlResources,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioRegistersRepublish<'static>,
    },
}

impl ProductionAccessPointStationResume {
    fn radio_runner(&self) -> &NetworkRunner {
        match self {
            Self::Fresh { network }
            | Self::Returned {
                phase:
                    ProductionAccessPointReturnedPhase::InitialScan { network, .. }
                    | ProductionAccessPointReturnedPhase::InitialJoin { network, .. }
                    | ProductionAccessPointReturnedPhase::Reconnected { network, .. },
                ..
            } => network.radio_runner(),
            Self::Returned {
                phase: ProductionAccessPointReturnedPhase::Disconnected { network, .. },
                ..
            } => network.radio_runner(),
        }
    }
}

struct ProductionAccessPointStationResources {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    rx_ring:
        Option<open_esp_radio::esp32s31::wifi::mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>>,
    tx: ProductionOrdinaryTxResources,
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    resume: ProductionAccessPointStationResume,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
}

pub(super) struct ProductionAccessPointTask {
    channel: u8,
    owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
    registers: RadioRegisters,
    interrupts: MacInterruptEpoch,
    service: ProductionAccessPointControl,
    parked: ProductionAccessPointParked,
}

pub(super) struct ProductionAccessPointPreflightFault {
    _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRegisters,
    _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
    _station: ProductionAccessPointStationResources,
    _access_point: ProductionAccessPointResources,
    _monitor: ProductionMonitorResources,
    _detached_control: Option<ControlTx>,
    _receive: Option<ProductionPreconnectedRx>,
}

pub(super) struct ProductionAccessPointRxOwnerFault {
    _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRegisters,
    _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
    _scan_rx: ProductionScanRx,
    _station: ProductionAccessPointStationResources,
    _access_point: ProductionAccessPointResources,
    _monitor: ProductionMonitorResources,
}

pub(super) struct ProductionAccessPointEngineFault {
    _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRegisters,
    _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
    _receive: ProductionPreconnectedRx,
    _transmit: ProductionWifiTxResources,
    _engine: open_esp_radio::esp32s31::wifi::ap::engine::Esp32s31ApEngineStartFailure<'static>,
    _parked: ProductionAccessPointParked,
    _rx_frame: &'static mut [u8],
    _tx_frame: &'static mut [u8],
}

pub(super) struct ProductionAccessPointSecurityMaterialFault {
    _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRegisters,
    _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
    _receive: ProductionPreconnectedRx,
    _transmit: ProductionWifiTxResources,
    _parked: ProductionAccessPointParked,
    _beacon: &'static mut [u8; open_esp_radio::wifi::ieee80211::beacon::WPA2_BEACON_CAPACITY],
    _rx_frame: &'static mut [u8],
    _tx_frame: &'static mut [u8],
}

pub(super) enum ProductionAccessPointPreparationFault {
    StationOwner {
        _wifi: open_esp_radio::esp32s31::wifi::device::runtime::Esp32s31WifiStopped<
            EspHalRadioPeripheral,
        >,
        _station: ProductionStationResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    Preflight {
        _fault: ProductionAccessPointPreflightFault,
    },
    RxOwner {
        _fault: ProductionAccessPointRxOwnerFault,
    },
    SecurityMaterial {
        _fault: ProductionAccessPointSecurityMaterialFault,
    },
    Engine {
        _fault: ProductionAccessPointEngineFault,
    },
}

pub(super) enum ProductionAccessPointTeardownFault {
    Interrupt {
        _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRegisters,
        _interrupts: MacInterruptEpoch,
        _stopped: ProductionAccessPointStopped,
        _parked: ProductionAccessPointParked,
    },
    TxRestore {
        _owner: Esp32s31AccessPointRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRegisters,
        _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
        _ring: open_esp_radio::esp32s31::wifi::mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>,
        _storage: &'static RxStorage,
        _rx_frame: &'static mut [u8],
        _tx_frame: &'static mut [u8],
        _engine: open_esp_radio::esp32s31::wifi::ap::engine::Esp32s31ApEngineStop<'static>,
        _control_report:
            open_esp_radio_esp32s31_wifi_embassy::access_point::Esp32s31AccessPointControlReport,
        _mac_report: open_esp_radio::esp32s31::wifi::ap::mac::Esp32s31ApMacReport,
        _parked: ProductionAccessPointParked,
        _returned_control: ControlTx,
    },
}

fn try_prepare_access_point_station_resources(
    resources: ProductionStationResources,
) -> Result<ProductionAccessPointStationResources, ProductionStationResources> {
    let returned = match resources {
        ProductionStationResources::Fresh(fresh) => {
            let ProductionStationFreshResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                network,
                board,
                station_address,
            } = fresh;
            return Ok(ProductionAccessPointStationResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                resume: ProductionAccessPointStationResume::Fresh { network },
                board,
                station_address,
            });
        }
        ProductionStationResources::Returned(returned) => returned,
    };
    let ProductionStationReusableResources {
        storage,
        board,
        phase,
        security,
        interrupt_route,
        mac_runtime,
        power_runtime,
    } = returned;
    let (dma, tx_epoch, scan_table, scan_frame, ethernet) = storage.into_parts();
    let station_address = board.interface.interface.address;
    let (ring, phase) = match phase {
        Esp32s31StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        } => match receive.into_halted() {
            Ok(ring) => (
                ring,
                ProductionAccessPointReturnedPhase::InitialScan { network, identity },
            ),
            Err(receive) => {
                return Err(ProductionStationResources::Returned(
                    ProductionStationReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::InitialScan {
                            receive,
                            network,
                            identity,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
        Esp32s31StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => match receive.try_into_halted() {
            Ok(ring) => (
                ring,
                ProductionAccessPointReturnedPhase::InitialJoin { network, station },
            ),
            Err(receive) => {
                return Err(ProductionStationResources::Returned(
                    ProductionStationReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::InitialJoin {
                            receive,
                            network,
                            station,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
        Esp32s31StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            station,
            registers,
        } => {
            let (ring, rx) = receive.into_epoch_parts();
            (
                ring,
                ProductionAccessPointReturnedPhase::Disconnected {
                    network,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                },
            )
        }
        Esp32s31StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            station,
            registers,
        } => match receive.try_into_halted() {
            Ok(ring) => (
                ring,
                ProductionAccessPointReturnedPhase::Reconnected {
                    network,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                },
            ),
            Err(receive) => {
                return Err(ProductionStationResources::Returned(
                    ProductionStationReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::Reconnected {
                            network,
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
    };
    Ok(ProductionAccessPointStationResources {
        dma,
        rx_ring: Some(ring),
        tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
        scan_table,
        scan_frame,
        ethernet,
        resume: ProductionAccessPointStationResume::Returned {
            phase,
            security,
            interrupt_route,
            mac_runtime,
            power_runtime,
        },
        board,
        station_address,
    })
}

fn restore_station_resources_after_access_point(
    parked: ProductionAccessPointParked,
    ring: open_esp_radio::esp32s31::wifi::mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
) -> (ProductionStationResources, ProductionMonitorResources) {
    let ProductionAccessPointParked {
        dma,
        tx_epoch,
        scan_table,
        resume,
        board,
        station_address,
        monitor,
    } = parked;
    let resources = match resume {
        ProductionAccessPointStationResume::Fresh { network } => {
            ProductionStationResources::Fresh(ProductionStationFreshResources {
                dma,
                rx_ring: Some(ring),
                tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                scan_table,
                scan_frame,
                ethernet,
                network,
                board,
                station_address,
            })
        }
        ProductionAccessPointStationResume::Returned {
            phase,
            security,
            interrupt_route,
            mac_runtime,
            power_runtime,
        } => {
            let phase = match phase {
                ProductionAccessPointReturnedPhase::InitialScan { network, identity } => {
                    Esp32s31StationStoppedPhaseResources::InitialScan {
                        receive: Esp32s31ScanRx::from_halted(ring, dma.storage()),
                        network,
                        identity,
                    }
                }
                ProductionAccessPointReturnedPhase::InitialJoin { network, station } => {
                    Esp32s31StationStoppedPhaseResources::InitialJoin {
                        receive: Esp32s31PreconnectedRx::from_halted(ring),
                        network,
                        station,
                    }
                }
                ProductionAccessPointReturnedPhase::Disconnected {
                    network,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                } => Esp32s31StationStoppedPhaseResources::Disconnected {
                    network,
                    receive: rx.with_halted_ring(ring),
                    aggregate_tx,
                    control,
                    station,
                    registers,
                },
                ProductionAccessPointReturnedPhase::Reconnected {
                    network,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                } => Esp32s31StationStoppedPhaseResources::Reconnected {
                    network,
                    receive: Esp32s31PreconnectedRx::from_halted(ring),
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                },
            };
            ProductionStationResources::Returned(ProductionStationReusableResources {
                storage: ProductionStationStorage::new(
                    dma, tx_epoch, scan_table, scan_frame, ethernet,
                ),
                board,
                phase,
                security,
                interrupt_route,
                mac_runtime,
                power_runtime,
            })
        }
    };
    (resources, monitor)
}

/// Static resources reserved for one exclusive AP epoch.
pub(super) struct ProductionAccessPointResources {
    pub(super) address: [u8; 6],
    pub(super) beacon:
        &'static mut [u8; open_esp_radio::wifi::ieee80211::beacon::WPA2_BEACON_CAPACITY],
}

impl ProductionWifiEpochRunner {
    pub(super) async fn prepare_access_point_task(
        &self,
        wifi: open_esp_radio::esp32s31::wifi::device::runtime::Esp32s31WifiStopped<
            EspHalRadioPeripheral,
        >,
        station: ProductionStationResources,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
        request: AccessPointRequest,
    ) -> Result<ProductionAccessPointTask, ProductionAccessPointPreparationFault> {
        let station = match try_prepare_access_point_station_resources(station) {
            Ok(station) => station,
            Err(station) => {
                return Err(ProductionAccessPointPreparationFault::StationOwner {
                    _wifi: wifi,
                    _station: station,
                    _access_point: access_point,
                    _monitor: monitor,
                });
            }
        };
        let current_channel = wifi.current_channel();
        let mut materialized = materialize_esp32s31_access_point(wifi, station);
        let requested_channel = request.channel();
        if requested_channel != current_channel {
            let observer = NoopPhyTargetObserver;
            let (phy, platform) = materialized.owner.radio_mut();
            let mut channel =
                Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(phy, platform, observer);
            if await_stack_boundary!(channel.select_channel(
                u16::from(requested_channel.primary()),
                0,
                &mut materialized.registers,
            ))
            .is_err()
            {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _station: materialized.resources,
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _receive: None,
                    },
                });
            }
            materialized.owner.set_current_channel(requested_channel);
        }

        let ProductionAccessPointStationResources {
            dma,
            rx_ring,
            tx,
            scan_table,
            scan_frame,
            ethernet,
            resume,
            board,
            station_address,
        } = materialized.resources;
        let power = materialized.owner.radio_mut().0.tx_target_power_profile();
        let tx_epoch = self.initialize_tx_epoch(tx, power);
        let scan_rx = match rx_ring {
            Some(ring) => Esp32s31ScanRx::from_halted(ring, dma.storage()),
            None => match Esp32s31ScanRx::prepare_initial(
                &mut materialized.registers,
                dma.storage(),
                dma.descriptor_base(),
                dma.buffer_addresses(),
            ) {
                Ok(receive) => receive,
                Err(_) => {
                    return Err(ProductionAccessPointPreparationFault::Preflight {
                        _fault: ProductionAccessPointPreflightFault {
                            _owner: materialized.owner,
                            _registers: materialized.registers,
                            _interrupt_setup: materialized.interrupt_setup,
                            _station: ProductionAccessPointStationResources {
                                dma,
                                rx_ring: None,
                                tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            _access_point: access_point,
                            _monitor: monitor,
                            _detached_control: None,
                            _receive: None,
                        },
                    });
                }
            },
        };
        let halted = match scan_rx.into_halted() {
            Ok(halted) => halted,
            Err(scan_rx) => {
                return Err(ProductionAccessPointPreparationFault::RxOwner {
                    _fault: ProductionAccessPointRxOwnerFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _scan_rx: scan_rx,
                        _station: ProductionAccessPointStationResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                });
            }
        };
        let receive = Esp32s31PreconnectedRx::from_halted(halted);
        let control = match tx_epoch.take_control() {
            Ok(control) => control,
            Err(_) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _station: ProductionAccessPointStationResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _receive: Some(receive),
                    },
                });
            }
        };
        let transmit = match control.try_into_resources() {
            Ok(resources) => resources,
            Err(control) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _station: ProductionAccessPointStationResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: Some(control),
                        _receive: Some(receive),
                    },
                });
            }
        };

        let (ssid, security, channel) = request.into_parts();
        let ProductionAccessPointResources { address, beacon } = access_point;
        let mut gtk_key = [0_u8; 16];
        for word in gtk_key.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        let gtk = match Wpa2Gtk::new(1, true, gtk_key) {
            Ok(gtk) => gtk,
            Err(_) => {
                return Err(ProductionAccessPointPreparationFault::SecurityMaterial {
                    _fault: ProductionAccessPointSecurityMaterialFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _receive: receive,
                        _transmit: transmit,
                        _parked: ProductionAccessPointParked {
                            dma,
                            tx_epoch,
                            scan_table,
                            resume,
                            board,
                            station_address,
                            monitor,
                        },
                        _beacon: beacon,
                        _rx_frame: scan_frame,
                        _tx_frame: ethernet,
                    },
                });
            }
        };
        let service = AccessPointService::new(address, security.into_pmk(), gtk);
        let engine = match Esp32s31ApEngine::start(
            &mut materialized.registers,
            service,
            beacon,
            &ssid,
            channel.primary(),
            AccessPointRequest::BEACON_INTERVAL_TU,
            AccessPointRequest::DTIM_PERIOD,
        ) {
            Ok(engine) => engine,
            Err(engine) => {
                return Err(ProductionAccessPointPreparationFault::Engine {
                    _fault: ProductionAccessPointEngineFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _receive: receive,
                        _transmit: transmit,
                        _engine: engine,
                        _parked: ProductionAccessPointParked {
                            dma,
                            tx_epoch,
                            scan_table,
                            resume,
                            board,
                            station_address,
                            monitor,
                        },
                        _rx_frame: scan_frame,
                        _tx_frame: ethernet,
                    },
                });
            }
        };
        let mac = Esp32s31ApMac::new(
            engine,
            transmit,
            Esp32s31ApTxConfig {
                // The recovered 24 Mbit/s vendor ladder is 24M x2, 18M x2,
                // 6M x3, then 5.5M. Four publications stopped before the
                // first robust OFDM rung and exposed ordinary RF loss directly
                // to UDP. Eight is the shortest complete prefix which reaches
                // every pre-CCK rung plus one final basic-rate publication.
                unicast_publication_limit: 8,
                publication_timeout_micros: TX_COMPLETION_TIMEOUT_US,
            },
        );
        let service =
            Esp32s31AccessPointControl::new(receive, dma.storage(), mac, scan_frame, ethernet);
        Ok(ProductionAccessPointTask {
            channel: channel.primary(),
            owner: materialized.owner,
            registers: materialized.registers,
            interrupts: mac_interrupt_epoch(materialized.interrupt_setup),
            service,
            parked: ProductionAccessPointParked {
                dma,
                tx_epoch,
                scan_table,
                resume,
                board,
                station_address,
                monitor,
            },
        })
    }

    pub(super) fn finish_access_point_task(
        &self,
        task: ProductionAccessPointTask,
    ) -> Result<ProductionSupervisorStopped, ProductionWifiFault> {
        let ProductionAccessPointTask {
            channel,
            owner,
            mut registers,
            interrupts,
            service,
            parked,
        } = task;
        let stopped = match service.try_finish(&mut registers) {
            Ok(stopped) => stopped,
            Err(service) => {
                return Err(ProductionWifiFault::AccessPointRuntime {
                    _task: ProductionAccessPointTask {
                        channel,
                        owner,
                        registers,
                        interrupts,
                        service,
                        parked,
                    },
                });
            }
        };
        let (_route, interrupt_setup, _mac_runtime, _power_runtime) =
            match interrupts.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupts) => {
                    return Err(ProductionWifiFault::AccessPointTeardown {
                        _fault: ProductionAccessPointTeardownFault::Interrupt {
                            _owner: owner,
                            _registers: registers,
                            _interrupts: interrupts,
                            _stopped: stopped,
                            _parked: parked,
                        },
                    });
                }
            };
        let ProductionAccessPointParked {
            dma,
            tx_epoch,
            scan_table,
            resume,
            board,
            station_address,
            monitor,
        } = parked;
        if let Err((_error, returned_control)) = tx_epoch.restore_resources(stopped.transmit) {
            return Err(ProductionWifiFault::AccessPointTeardown {
                _fault: ProductionAccessPointTeardownFault::TxRestore {
                    _owner: owner,
                    _registers: registers,
                    _interrupt_setup: interrupt_setup,
                    _ring: stopped.ring,
                    _storage: stopped.storage,
                    _rx_frame: stopped.rx_frame,
                    _tx_frame: stopped.tx_frame,
                    _engine: stopped.engine,
                    _control_report: stopped.control_report,
                    _mac_report: stopped.mac_report,
                    _parked: ProductionAccessPointParked {
                        dma,
                        tx_epoch,
                        scan_table,
                        resume,
                        board,
                        station_address,
                        monitor,
                    },
                    _returned_control: returned_control,
                },
            });
        }
        let access_point = ProductionAccessPointResources {
            address: stopped.engine.service.address(),
            beacon: stopped.engine.beacon_storage,
        };
        let (station, monitor) = restore_station_resources_after_access_point(
            ProductionAccessPointParked {
                dma,
                tx_epoch,
                scan_table,
                resume,
                board,
                station_address,
                monitor,
            },
            stopped.ring,
            stopped.rx_frame,
            stopped.tx_frame,
        );
        let wifi = owner.into_stopped(registers, interrupt_setup, ());
        Ok(Esp32s31WifiSupervisorStopped::new(
            wifi.wifi,
            station,
            access_point,
            monitor,
        ))
    }
}

async fn wait_for_access_point_stop(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, CriticalSectionRawMutex, Esp32s31RadioError>,
) {
    loop {
        match endpoint.receive().await {
            EmbassyWifiSupervisorCommand::Stop => return,
            EmbassyWifiSupervisorCommand::Scan(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected {
                            request,
                            error: Esp32s31RadioError::RoleActive(
                                EmbassyWifiStartKind::StandaloneScan,
                            ),
                        },
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartStation(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Station(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::Station),
                        ),
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartAccessPoint(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::AccessPoint),
                        ),
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartMonitor(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::StandaloneMonitor),
                        ),
                    )))
                    .await;
            }
        }
    }
}
impl ProductionWifiEpochRunner {
    pub(super) async fn run_access_point_service(
        &mut self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: AccessPointRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (wifi, station, access_point, monitor) = stopped.into_parts();
        let mut task = match await_stack_boundary!(self.prepare_access_point_task(
            wifi,
            station,
            access_point,
            monitor,
            request,
        )) {
            Ok(task) => task,
            Err(fault) => {
                let faulted = ProductionWifiFault::AccessPointPreparation { _fault: fault };
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::faulted(self.fault_error(&faulted)),
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        endpoint
            .respond(EmbassyWifiSupervisorResponse::AccessPoint(Ok(
                WifiStartReport::new(generation),
            )))
            .await;
        let result = {
            let network = task.parked.resume.radio_runner();
            let (_, platform) = task.owner.radio_mut();
            await_stack_boundary!(task.service.run_until_stopped(
                &mut task.registers,
                &mut task.interrupts,
                &*platform,
                network,
                wait_for_access_point_stop(endpoint),
                || {
                    let mut nonce = [0_u8; 32];
                    for word in nonce.chunks_exact_mut(4) {
                        word.copy_from_slice(&self.trng.random().to_le_bytes());
                    }
                    let replay =
                        (u64::from(self.trng.random()) << 32) | u64::from(self.trng.random());
                    (nonce, replay)
                },
            ))
        };
        #[cfg(feature = "qualification")]
        if let Ok(report) = &result
            && let Some(hooks) = task.parked.board.qualification
        {
            (hooks.access_point)(crate::Esp32s31AccessPointObservation {
                channel: task.channel,
                beacons_transmitted: report.mac.beacons_transmitted,
                missed_beacon_intervals: report.control.missed_beacon_intervals,
                maximum_beacon_lateness_micros: report.control.maximum_beacon_lateness_micros,
                tx_interrupt_wakes: report.control.tx_interrupt_wakes,
                tx_deadline_wakes: report.control.tx_deadline_wakes,
                maximum_tx_pending_micros: report.control.maximum_tx_pending_micros,
                maximum_rx_service_micros: report.control.maximum_rx_service_micros,
                maximum_network_backpressure_micros: report
                    .control
                    .maximum_network_backpressure_micros,
                authentication_responses: report.mac.authentication_responses_transmitted,
                association_responses: report.mac.association_responses_transmitted,
                authorized_peers: report.engine.authorized_peers,
                peer_removals: report.engine.peer_removals,
                completed_rx_descriptors: report.control.completed_rx_descriptors,
                ignored_rx_frames: report.control.ignored_rx_frames,
                rx_mic_failures: report.control.rx_mic_failures,
                rx_quarantined_frames: report.control.rx_quarantined_frames,
                rx_view_rejected: report.control.rx_view_rejected,
                control_frames_staged: report.control.control_frames_staged,
                control_frames_dropped_while_busy: report.control.control_frames_dropped_while_busy,
                ethernet_frames_staged: report.control.ethernet_frames_staged,
                ethernet_arp_requests_staged: report.control.ethernet_arp_requests_staged,
                ethernet_tcp_frames_staged: report.control.ethernet_tcp_frames_staged,
                network_tx_frames_observed: report.control.network_tx_frames_observed,
                network_tx_arp_requests: report.control.network_tx_arp_requests,
                network_tx_arp_replies: report.control.network_tx_arp_replies,
                network_tx_rejected_no_peer: report.control.network_tx_rejected_no_peer,
                network_tx_rejected_destination: report.control.network_tx_rejected_destination,
                network_tx_frames_rejected: report.control.network_tx_frames_rejected,
                data_frames_transmitted: report.mac.data_frames_transmitted,
                tx_hardware_failures: report.mac.tx_failures.hardware_failures,
                tx_hardware_timeouts: report.mac.tx_failures.hardware_timeouts,
                tx_collision_limits: report.mac.tx_failures.collision_limits,
                tx_last_hardware_status: report.mac.tx_failures.last_hardware_status,
                protected_data_frames: report.control.protected_data_frames,
                protected_data_unauthorized: report.control.protected_data_unauthorized,
                protected_data_foreign: report.control.protected_data_foreign,
                protected_data_duplicates: report.control.protected_data_duplicates,
                protected_data_radio_rejected: report.control.protected_data_radio_rejected,
                protected_data_protocol_rejected: report.control.protected_data_protocol_rejected,
            });
        }
        if let Err(_error) = result {
            let faulted = ProductionWifiFault::AccessPointRuntime { _task: task };
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Stop(Err(
                    self.fault_error(&faulted)
                )))
                .await;
            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
        }
        let stopped = match self.finish_access_point_task(task) {
            Ok(stopped) => stopped,
            Err(faulted) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Stop(Err(
                        self.fault_error(&faulted)
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                WifiStopReport::new(generation),
            )))
            .await;
        EmbassyWifiRoleEpochOutcome::Stopped(stopped)
    }
}
