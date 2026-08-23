//! Station role engine composition and finite owner restoration.

use super::*;

pub(super) struct ProductionStationEnginePort<O> {
    mode: ProductionStationMode,
    access_point: ProductionAccessPointResources,
    monitor: ProductionMonitorResources,
    _owner: PhantomData<fn() -> O>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ProductionStationMode {
    Service,
    PairedCutover,
}

impl<O> ProductionStationEnginePort<O> {
    fn new(
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            mode: ProductionStationMode::Service,
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    fn paired_cutover(
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            mode: ProductionStationMode::PairedCutover,
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    pub(super) fn into_parked_roles(
        self,
    ) -> (ProductionAccessPointResources, ProductionMonitorResources) {
        (self.access_point, self.monitor)
    }
}

pub(super) fn production_station_runtime<'state>(
    role: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    interrupt_epoch: MacInterruptEpoch,
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_storage: &'static mut TxStorage,
    scan_table: &'static mut ScanTable,
    frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    board: ProductionStationBoardResources,
) -> ProductionStationRuntime<'state> {
    Esp32s31StationRuntimeResources::new(
        Esp32s31StationRadioResources::new(role, interrupt_epoch),
        Esp32s31StationStorageResources::new(dma, tx_storage, scan_table, frame, ethernet),
        board,
    )
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_scan_epoch(
        &mut self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
            WifiNetworkResources,
        >,
        discovery: StationDiscovery,
    ) -> Esp32s31StationInitialScanExit<
        'security,
        ProductionStationRuntime<'state>,
        RadioRuntimeOwner,
        Esp32s31RxFrontier<
            'static,
            EmbassyEsp32s31RxFrontierDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, hardware, receive, network, identity, mut security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, interrupt_epoch) = radio_resources.parts_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let control = tx_storage
            .take_control()
            .expect("initial scan owns the ordinary TX owner");
        let interrupt_setup = interrupt_epoch
            .setup()
            .expect("initial scan requires a quiesced interrupt epoch");
        let scan_plan = Esp32s31StationScanPlan::new(discovery, None);
        let scan_request = scan_plan.request(identity.station_address);
        let scan = run_esp32s31_station_scan(
            Esp32s31StationScanResources {
                phy,
                platform,
                phy_observer: NoopPhyTargetObserver,
                phy_delay: EmbassyEsp32s31PhyDelay,
                hardware,
                receive,
                control,
                interrupt_setup,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: EmbassyEsp32s31ScanTimer,
            },
            scan_request,
        )
        .await;
        let decision = scan.decision;
        let open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            phy_delay: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("initial scan returned over a live TX owner"));
        complete_esp32s31_station_initial_scan(
            Esp32s31StationInitialScanReturned {
                runtime,
                hardware,
                receive,
                network,
                identity,
                security,
            },
            decision,
            |receive| receive.into_halted().map(Esp32s31RxFrontier::from_halted),
            |runtime, hardware, receive, network, identity, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::InitialScan {
                        hardware,
                        receive,
                        network,
                        identity,
                    },
                    security.into_role(),
                )
            },
            Esp32s31StationInitialScanFailures {
                no_candidate: Esp32s31StaAttemptStage::Candidate,
                receive_handoff: Esp32s31StaAttemptStage::Candidate,
                transaction: Esp32s31StaAttemptStage::Candidate,
                invalid_plan: Esp32s31StaAttemptStage::Candidate,
            },
        )
    }

    #[inline(never)]
    async fn run_connected_epoch(
        &mut self,
        phase: Esp32s31StationConnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ProductionConnectedPhase,
        >,
        control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> StaAttemptOutcome<
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (runtime, connected, security) = phase.into_parts();
        if self.mode == ProductionStationMode::PairedCutover {
            return StaAttemptOutcome::Stopped {
                owner: ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::Connected { connected },
                    security,
                ),
            };
        }
        let ProductionConnectedPhase {
            epoch,
            network,
            station,
            peer,
            pairwise,
            group,
        } = connected;
        let interface = runtime.board().interface;
        let returned = run_connected(
            control,
            ConnectedStationResources::new(
                runtime,
                epoch,
                network,
                interface,
                connected_config(),
                peer,
                pairwise,
                group,
                security,
            ),
        )
        .await;
        let returned = match returned {
            ConnectedStationRunExit::Returned(returned) => returned,
            ConnectedStationRunExit::Faulted(fault) => {
                return StaAttemptOutcome::Faulted {
                    fault: ProductionStationFault {
                        _connected: fault,
                        _station: station,
                    },
                };
            }
        };
        let owner = ProductionStationOwner::new(
            returned.runtime,
            ProductionStationPhase::RunningScan {
                disconnected: returned.disconnected,
                station,
            },
            returned.security,
        );
        match returned.outcome {
            ConnectedStationOutcome::Disconnected(_)
            | ConnectedStationOutcome::ReconnectRequested => StaAttemptOutcome::Disconnected {
                owner,
                next_candidate: StaNextCandidate::Refresh,
            },
            ConnectedStationOutcome::StationStopped(_) => StaAttemptOutcome::Stopped { owner },
            ConnectedStationOutcome::HardwareFailure => StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    open_esp_radio_wifi_sta::station::StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    Esp32s31StaAttemptStage::ConnectedEntry,
                ),
            },
        }
    }

    #[inline(never)]
    async fn run_running_scan_epoch(
        &mut self,
        phase: Esp32s31StationRunningScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedDisconnectedEpoch,
        >,
        discovery: StationDiscovery,
    ) -> Esp32s31StationRunningScanExit<
        'security,
        ProductionStationRuntime<'state>,
        ConnectedReconnectedEpoch,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, disconnected, station, mut security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, interrupt_epoch) = radio_resources.parts_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let Esp32s31RunningScanEpochParts {
            retained,
            hardware,
            rx,
        } = disconnected.into_running_scan_parts();
        let control = tx_storage
            .take_control()
            .expect("connected teardown returned the ordinary TX owner");
        let interrupt_setup = interrupt_epoch
            .setup()
            .expect("running scan requires a quiesced interrupt epoch");
        let scan_plan = Esp32s31StationScanPlan::new(discovery, None);
        let scan_request = scan_plan.request(station.station_address);
        let scan = run_esp32s31_station_scan(
            Esp32s31StationScanResources {
                phy,
                platform,
                phy_observer: NoopPhyTargetObserver,
                phy_delay: EmbassyEsp32s31PhyDelay,
                hardware,
                receive: Esp32s31RunningScanRx::from_stopped(rx),
                control,
                interrupt_setup,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: EmbassyEsp32s31ScanTimer,
            },
            scan_request,
        )
        .await;
        let scan_result = match scan.decision {
            Esp32s31StationScanDecision::Selected { candidate, .. } => {
                Esp32s31StationRunningScanCompletion::Selected(candidate)
            }
            Esp32s31StationScanDecision::NoCandidate { .. } => {
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition: StaFailureDisposition::RefreshCandidate,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
            Esp32s31StationScanDecision::Stopped { .. } => {
                Esp32s31StationRunningScanCompletion::Stopped
            }
            Esp32s31StationScanDecision::Failed { error, .. } => {
                let disposition = esp32s31_station_scan_failure_disposition(&error);
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
            Esp32s31StationScanDecision::InvalidPlan { .. } => {
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition: StaFailureDisposition::Terminal,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
        };
        let open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            phy_delay: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        let rx = receive.into_stopped().unwrap_or_else(|rx| {
            panic!(
                "running scan did not return a halted RX owner: {:?}",
                rx.phase()
            )
        });
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
        let disconnected = retained.restore(hardware, rx);
        complete_esp32s31_station_running_scan(
            runtime,
            disconnected,
            station,
            security,
            scan_result,
            |disconnected| {
                let (network, epoch) =
                    disconnected.prepare_reconnect::<EmbassyEsp32s31RxFrontierDelay>();
                (WifiNetworkResources::Running(network), epoch)
            },
            |runtime, disconnected, station, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::RunningScan {
                        disconnected,
                        station,
                    },
                    security.into_role(),
                )
            },
        )
    }
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_join_epoch<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            Esp32s31RxFrontier<
                'static,
                EmbassyEsp32s31RxFrontierDelay,
                RX_DESCRIPTOR_COUNT,
                RX_BUFFER_SIZE,
            >,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> Esp32s31StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        diagnostics_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut hardware, receive, network, station, security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, _) = radio_resources.parts_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            EmbassyEsp32s31PhyDelay,
            _,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(Esp32s31StationJoinResources {
            hardware: &mut hardware,
            phy,
            platform,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            Esp32s31StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                diagnostics_event!(
                    "open-radio: station attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::InitialJoin {
                            hardware,
                            receive: returned.receive,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            Esp32s31StationJoinOutcome::Connected {
                returned,
                peer,
                pairwise,
                group,
                report,
                progress,
            } => {
                diagnostics_event!(
                    "open-radio: station joined phases={} auth={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                Esp32s31StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Initial {
                            hardware,
                            receive: returned.receive,
                        },
                        network,
                        station: returned.station,
                        peer,
                        pairwise,
                        group,
                    },
                    returned.security,
                )
            }
        }
    }

    #[inline(never)]
    async fn run_reconnected_epoch<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedReconnectedEpoch,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> Esp32s31StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        diagnostics_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut reconnect, network, station, security) = phase.into_parts();
        let (hardware, receive_slot) = reconnect.hardware_and_rx_mut();
        let receive = match receive_slot.take() {
            Ok(receive) => receive,
            Err(_) => {
                return Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station,
                        },
                        security,
                    ),
                    failure: StaAttemptFailure::new(
                        open_esp_radio_wifi_sta::station::StaLifecycleStage::Hardware,
                        open_esp_radio_wifi_sta::station::StaFailureDisposition::Terminal,
                        Esp32s31StaAttemptStage::Candidate,
                    ),
                });
            }
        };
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, _) = radio_resources.parts_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            EmbassyEsp32s31PhyDelay,
            _,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(Esp32s31StationJoinResources {
            hardware,
            phy,
            platform,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            Esp32s31StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                diagnostics_event!(
                    "open-radio: reconnect attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            Esp32s31StationJoinOutcome::Connected {
                returned,
                peer,
                pairwise,
                group,
                report,
                progress,
            } => {
                diagnostics_event!(
                    "open-radio: station rejoined phases={} auth={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                Esp32s31StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Reconnected(reconnect),
                        network,
                        station: returned.station,
                        peer,
                        pairwise,
                        group,
                    },
                    returned.security,
                )
            }
        }
    }
}

#[cfg(not(feature = "diagnostics"))]
pub(super) type ProductionStationRunner<'state, 'security> = Esp32s31StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
>;

#[cfg(feature = "diagnostics")]
pub(super) type ProductionStationRunner<'state, 'security> = Esp32s31StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    ProductionStationObserver,
>;

#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
pub(super) struct ProductionStationObserver {
    station_attempt: Option<fn(crate::Esp32s31StationAttemptObservation)>,
}

#[cfg(feature = "diagnostics")]
impl<'state, 'security>
    Esp32s31StationEngineObserver<
        'security,
        CriticalSectionRawMutex,
        ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    > for ProductionStationObserver
{
    fn backoff_started(&mut self, _delay_millis: u32, reason: StaBackoffReason) {
        let StaBackoffReason::AttemptFailed { stage, attempt } = reason else {
            return;
        };
        if let Some(station_attempt) = self.station_attempt {
            station_attempt(crate::Esp32s31StationAttemptObservation::AttemptFailed {
                attempt,
                stage,
            });
        }
    }
}

impl<'state, 'security> Esp32s31StationEnginePort<'security, CriticalSectionRawMutex>
    for ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>
{
    type Runtime = ProductionStationRuntime<'state>;
    type InitialHardware = RadioRuntimeOwner;
    type InitialScanRx =
        Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
    type RxFrontier = Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >;
    type Network = WifiNetworkResources;
    type Disconnected = ConnectedDisconnectedEpoch;
    type Reconnected = ConnectedReconnectedEpoch;
    type Connected = ProductionConnectedPhase;
    type Error = Esp32s31StaAttemptStage;
    type Fault = ProductionStationFault<'state, 'security>;

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_scan_epoch(phase, discovery)
    }

    fn run_initial_join<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_join_epoch(phase, context)
    }

    fn run_running_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_running_scan_epoch(phase, discovery)
    }

    fn run_reconnected<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
        >,
        context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_reconnected_epoch(phase, context)
    }

    fn run_connected<'a>(
        &'a mut self,
        phase: Esp32s31StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        _context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_connected_epoch(phase, control)
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        Esp32s31StaAttemptStage::Candidate
    }
}

pub(super) type ProductionStationTask = Esp32s31StationTask<
    'static,
    CriticalSectionRawMutex,
    ProductionStationRunner<'static, 'static>,
>;
pub(super) type ProductionStationControl = Esp32s31StationController<'static, CriticalSectionRawMutex>;
pub(super) type ProductionStationExit = Esp32s31StationExit<
    ProductionStationOwner<'static, 'static>,
    ProductionStationRunner<'static, 'static>,
    Esp32s31StaAttemptStage,
    ProductionStationFault<'static, 'static>,
>;

pub(super) fn restore_production_station_frontier(
    resources: open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationReturnedResources<
        ProductionStationOwner<'static, 'static>,
        ProductionStationRunner<'static, 'static>,
    >,
) -> Result<ProductionSupervisorStopped, ProductionWifiFault> {
    let (owner, runner) = resources.into_parts();
    match try_reclaim_production_station(owner) {
        Ok(stopped) => {
            let (access_point, monitor) = runner.into_port().into_parked_roles();
            let resources = ProductionWifiStoppedResources::Returned(stopped.resources);
            match try_split_wifi_stopped_resources(resources) {
                Ok((physical, station)) => Ok(Esp32s31WifiSupervisorStopped::new(
                    stopped.wifi,
                    physical,
                    station,
                    access_point,
                    monitor,
                )),
                Err(resources) => Err(ProductionWifiFault::StoppedOwner {
                    _wifi: stopped.wifi,
                    _resources: resources,
                    _access_point: access_point,
                    _monitor: monitor,
                }),
            }
        }
        Err(failure) => Err(ProductionWifiFault::Reclaim {
            _station: failure,
            _runner: runner,
        }),
    }
}

impl ProductionWifiEpochRunner {
    pub(super) fn initialize_tx_epoch(
        &self,
        tx: ProductionOrdinaryTxResources,
        power: PhyTxTargetPowerProfile,
    ) -> &'static mut TxStorage {
        match tx {
            ProductionOrdinaryTxResources::Uninitialized(tx_slot) => TX_STATE.init_with(|| {
                TxStorage::from_slot(
                    tx_slot,
                    power,
                    tx_entropy as fn() -> u32,
                    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
                    ControlTxConfig {
                        unicast_attempt_limit: 4,
                        completion_timeout_us: TX_COMPLETION_TIMEOUT_US,
                        poll_interval_us: 1,
                    },
                )
            }),
            ProductionOrdinaryTxResources::Epoch(tx) => tx,
        }
    }

    fn fresh_security(&self, security: StationSecurity) -> Esp32s31StaAttemptSecurity<'static> {
        let mut supplicant_nonce = [0; 32];
        for word in supplicant_nonce.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        Esp32s31StaAttemptSecurity::new(
            security.into_pmk(),
            supplicant_nonce,
            StaTxSequenceCounters::new((self.trng.random() & 0x0fff) as u16),
            open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::Unprotected,
        )
    }

    pub(super) fn prepare_station_task(
        &self,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        mode: ProductionStationMode,
    ) -> Result<(ProductionStationControl, ProductionStationTask), ProductionWifiFault> {
        let (discovery, security, reconnect) = request.into_parts();
        let (wifi, physical_resources, station_role, access_point_resources, monitor_resources) =
            stopped.into_parts();
        let station_resources = join_station_activation_resources(physical_resources, station_role);
        let security = self.fresh_security(security);
        let owner = match station_resources {
            ProductionWifiStoppedResources::Fresh(fresh) => {
                let mut materialized = materialize_esp32s31_wifi_role(wifi, fresh);
                let ProductionWifiFreshResources {
                    dma,
                    rx_ring,
                    tx,
                    scan_table,
                    scan_frame,
                    ethernet,
                    network,
                    board,
                    station_address,
                } = materialized.resources;
                let mut registers = materialized.registers;
                let (phy, _) = materialized.owner.radio_mut();
                let tx_storage = self.initialize_tx_epoch(tx, phy.tx_target_power_profile());
                activate_promiscuous_receive(&mut registers);
                let scan_rx = match rx_ring {
                    Some(ring) => Esp32s31ScanRx::from_halted(ring, dma.storage()),
                    None => match Esp32s31ScanRx::prepare_initial(
                        &mut registers,
                        dma.storage(),
                        dma.descriptor_base(),
                        dma.buffer_addresses(),
                    ) {
                        Ok(scan_rx) => scan_rx,
                        Err(error) => {
                            return Err(ProductionWifiFault::InitialRx {
                                _fault: ProductionInitialRxFault {
                                    _error: error,
                                    _owner: materialized.owner,
                                    _registers: registers,
                                    _interrupt_setup: materialized.interrupt_setup,
                                    _dma: dma,
                                    _tx_storage: tx_storage,
                                    _scan_table: scan_table,
                                    _scan_frame: scan_frame,
                                    _ethernet: ethernet,
                                    _network: network,
                                    _board: board,
                                    _station_address: station_address,
                                    _security: security,
                                },
                            });
                        }
                    },
                };
                ProductionStationOwner::new(
                    production_station_runtime(
                        materialized.owner,
                        mac_interrupt_epoch(materialized.interrupt_setup),
                        dma,
                        tx_storage,
                        scan_table,
                        scan_frame,
                        ethernet,
                        board,
                    ),
                    ProductionStationPhase::InitialScan {
                        hardware: registers,
                        receive: scan_rx,
                        network,
                        identity: Esp32s31StaIdentity {
                            station_address,
                            association_preference: discovery.scan().association_preference(),
                        },
                    },
                    security,
                )
            }
            ProductionWifiStoppedResources::Returned(returned) => {
                let materialized = materialize_esp32s31_wifi_role(wifi, returned);
                let ProductionWifiReusableResources {
                    storage,
                    board,
                    phase,
                    security: previous_security,
                    interrupt_route,
                    mac_runtime,
                    power_runtime,
                } = materialized.resources;
                let rx_storage = storage.parts().0.storage();
                let identity = Esp32s31StaIdentity {
                    station_address: board.interface.interface.address,
                    association_preference: discovery.scan().association_preference(),
                };
                let phase = match try_rebind_esp32s31_station_phase(phase, rx_storage, identity) {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: materialized.registers,
                                _interrupt_setup: materialized.interrupt_setup,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                                _interrupt_route: interrupt_route,
                                _mac_runtime: mac_runtime,
                                _power_runtime: power_runtime,
                            },
                        });
                    }
                };
                let phase = match try_restore_esp32s31_station_phase(materialized.registers, phase)
                {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: failure.registers,
                                _interrupt_setup: materialized.interrupt_setup,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                                _interrupt_route: interrupt_route,
                                _mac_runtime: mac_runtime,
                                _power_runtime: power_runtime,
                            },
                        });
                    }
                };
                let interrupt = MacInterruptEpoch::new(
                    interrupt_route,
                    materialized.interrupt_setup,
                    mac_runtime,
                    power_runtime,
                );
                // Dropping the previous security value here zeroizes its PMK
                // only after the old finite station task returned completely.
                drop(previous_security);
                ProductionStationOwner::new(
                    Esp32s31StationRuntimeResources::new(
                        Esp32s31StationRadioResources::new(materialized.owner, interrupt),
                        storage,
                        board,
                    ),
                    phase,
                    security,
                )
            }
        };
        let port = match mode {
            ProductionStationMode::Service => {
                ProductionStationEnginePort::new(access_point_resources, monitor_resources)
            }
            ProductionStationMode::PairedCutover => ProductionStationEnginePort::paired_cutover(
                access_point_resources,
                monitor_resources,
            ),
        };
        #[cfg(feature = "diagnostics")]
        let runner = ProductionStationRunner::with_observer(
            port,
            discovery,
            ProductionStationObserver {
                station_attempt: owner
                    .runtime
                    .board()
                    .diagnostics
                    .map(|hooks| hooks.station_attempt),
            },
        );
        #[cfg(not(feature = "diagnostics"))]
        let runner = ProductionStationRunner::new(port, discovery);
        prepare_esp32s31_station_task(
            Esp32s31StationConfig::new(reconnect),
            Esp32s31StationStartResources::new(owner),
            self.station_control,
            runner,
        )
        .map_err(|failure| ProductionWifiFault::TaskPreparation { _failure: failure })
    }
}
