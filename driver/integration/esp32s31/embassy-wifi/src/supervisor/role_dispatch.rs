//! Supervisor command dispatch and finite standalone role epochs.

use super::*;

fn standalone_scan_report(
    table: &ScanTable<SCAN_RECORD_CAPACITY>,
    generation: open_esp_radio::RadioSubsystemGeneration,
) -> WifiScanReport {
    let summary = table.summary();
    let mut results = [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY];
    for (destination, source) in results.iter_mut().zip(table.records()) {
        *destination = WifiScanResult::new(
            source.ssid,
            source.ssid_len,
            source.bssid,
            source.channel,
            source.rssi,
            source.privacy,
            source.rsn,
            source.legacy_wpa,
            source.ht_capability_ie_present,
            source.he_capability_ie_len != 0,
        );
    }
    WifiScanReport::new(
        generation,
        results,
        summary.records as u8,
        summary.observed_frames,
        summary.dropped_unique_bss,
    )
}
impl ProductionWifiEpochRunner {
    async fn run_standalone_scan(
        &mut self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: WifiScanRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (wifi, physical, mut station, access_point, monitor) = stopped.into_parts();
        let mut materialized = materialize_esp32s31_wifi_role(wifi, physical);
        let (dma, rx_ring, tx, aggregate_tx) = materialized.resources.into_parts();
        let (phy, platform) = materialized.owner.radio_mut();
        let tx_epoch = self.initialize_tx_epoch(tx, phy.tx_target_power_profile());
        activate_promiscuous_receive(&mut materialized.registers);
        let receive = match rx_ring {
            Some(ring) => Esp32s31ScanRx::from_halted(ring, dma.storage()),
            None => match Esp32s31ScanRx::prepare_initial(
                &mut materialized.registers,
                dma.storage(),
                dma.descriptor_base(),
                dma.buffer_addresses(),
            ) {
                Ok(receive) => receive,
                Err(error) => {
                    let faulted = ProductionWifiFault::StandaloneScanInitialRx {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _physical: ProductionWifiPhysicalResources::new(
                            dma,
                            None,
                            ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        ),
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                        _error: error,
                    };
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                            WifiScanFailure::Faulted {
                                error: self.fault_error(&faulted),
                            },
                        )))
                        .await;
                    return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                }
            },
        };
        let control = tx_epoch
            .take_control()
            .expect("standalone scan starts from an idle ordinary TX owner");
        let mut channels = [0_u8; 14];
        let mut channel_count = 0_usize;
        for channel in request.channels().primary_channels() {
            channels[channel_count] = channel;
            channel_count += 1;
        }
        let scan_request = Esp32s31StationScanRequest::new(
            open_esp_radio_esp32s31_wifi_sta::scan::Esp32s31StaScanConfig::new(
                request.dwell_millis(),
            )
            .expect("WifiScanRequest stores a nonzero dwell"),
            &channels[..channel_count],
            station.station_address(),
            &[],
            &ESP32S31_STATION_PROBE_RATES,
        )
        .with_descriptor_capacity(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
        .without_candidate_selection();
        let mut sequence = open_esp_radio_ieee80211::station::StaSequenceCounter::new(
            (self.trng.random() & 0x0fff) as u16,
        );
        let (scan_table, scan_frame) = station.scan_storage();
        let scan = run_esp32s31_station_scan(
            Esp32s31StationScanResources {
                phy,
                platform,
                phy_observer: NoopPhyTargetObserver,
                phy_delay: EmbassyEsp32s31PhyDelay,
                hardware: materialized.registers,
                receive,
                control,
                interrupt_setup: &materialized.interrupt_setup,
                table: scan_table,
                frame: scan_frame,
                scan_observer: ProductionScanObserver,
                sequence: &mut sequence,
                timer: EmbassyEsp32s31ScanTimer,
            },
            scan_request,
        )
        .await;
        let completed = matches!(
            scan.decision,
            Esp32s31StationScanDecision::NoCandidate { .. }
        );
        let open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationScanReturned {
            hardware: registers,
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
        if let Err((error, returned_control)) = tx_epoch.restore_control(control) {
            let faulted = ProductionWifiFault::StandaloneScanReturn {
                _fault: ProductionStandaloneScanReturnFault::TxRestore {
                    _owner: materialized.owner,
                    _registers: registers,
                    _interrupt_setup: materialized.interrupt_setup,
                    _dma: dma,
                    _receive: receive,
                    _tx_epoch: tx_epoch,
                    _aggregate_tx: aggregate_tx,
                    _station: station,
                    _access_point: access_point,
                    _monitor: monitor,
                    _error: error,
                    _returned_control: returned_control,
                },
            };
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                    WifiScanFailure::Faulted {
                        error: self.fault_error(&faulted),
                    },
                )))
                .await;
            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
        }
        let ring = match receive.into_halted() {
            Ok(ring) => ring,
            Err(receive) => {
                let faulted = ProductionWifiFault::StandaloneScanReturn {
                    _fault: ProductionStandaloneScanReturnFault::ReceiveStillActive {
                        _owner: materialized.owner,
                        _registers: registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _dma: dma,
                        _receive: receive,
                        _tx_epoch: tx_epoch,
                        _aggregate_tx: aggregate_tx,
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                };
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Faulted {
                            error: self.fault_error(&faulted),
                        },
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        let report = standalone_scan_report(station.scan_table(), generation);
        let wifi = materialized
            .owner
            .into_stopped(registers, materialized.interrupt_setup, ());
        let stopped = Esp32s31WifiSupervisorStopped::new(
            wifi.wifi,
            ProductionWifiPhysicalResources::new(
                dma,
                Some(ring),
                ProductionOrdinaryTxResources::Epoch(tx_epoch),
                aggregate_tx,
            ),
            station,
            access_point,
            monitor,
        );
        let response = if completed {
            Ok(report)
        } else {
            Err(WifiScanFailure::Returned {
                request,
                error: Esp32s31RadioError::HardwareFault,
            })
        };
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Scan(response))
            .await;
        EmbassyWifiRoleEpochOutcome::Stopped(stopped)
    }

    async fn run_station_service(
        &self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        await_stack_boundary!(run_esp32s31_station_supervisor_epoch(
            endpoint,
            Esp32s31StationSupervisorEpoch::new(stopped, request, generation),
            |stopped, request| {
                self.prepare_station_task(stopped, request, ProductionStationMode::Service)
            },
            Esp32s31StationSupervisorHooks::new(
                |output: ProductionStationExit| {
                    let resources = match output {
                        Esp32s31StationExit::Stopped {
                            resources,
                            progress,
                            reason,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch stopped attempts={} connected_epochs={} reason={reason:?}",
                                progress.attempts_started,
                                progress.connected_epochs,
                            );
                            resources
                        }
                        Esp32s31StationExit::RetryExhausted {
                            resources,
                            progress,
                            failure,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch exhausted attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            #[cfg(feature = "diagnostics")]
                            if let Some(hooks) = resources.owner().runtime.board().diagnostics {
                                (hooks.station_attempt)(
                                    crate::Esp32s31StationAttemptObservation::RetryExhausted {
                                        attempts: progress.final_generation_attempt,
                                        stage: failure.stage,
                                    },
                                );
                            }
                            resources
                        }
                        Esp32s31StationExit::Terminal {
                            resources,
                            progress,
                            failure,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch ended attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            resources
                        }
                        Esp32s31StationExit::Faulted { fault, runner, .. } => {
                            return EmbassyWifiRoleFrontier::Faulted(
                                ProductionWifiFault::Station {
                                    _fault: fault,
                                    _runner: runner,
                                },
                            );
                        }
                    };
                    match restore_production_station_frontier(resources) {
                        Ok(stopped) => EmbassyWifiRoleFrontier::Stopped(stopped),
                        Err(faulted) => EmbassyWifiRoleFrontier::Faulted(faulted),
                    }
                },
                Esp32s31RadioError::RoleActive,
                |_faulted: &ProductionWifiFault| Esp32s31RadioError::HardwareFault,
            ),
        ))
    }
}
impl EmbassyWifiRoleEpochRunner<CriticalSectionRawMutex> for ProductionWifiEpochRunner {
    type Stopped = ProductionSupervisorStopped;
    type Faulted = ProductionWifiFault;
    type Error = Esp32s31RadioError;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error {
        Esp32s31RadioError::Planning(error)
    }

    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error {
        let _ = faulted;
        Esp32s31RadioError::HardwareFault
    }

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, CriticalSectionRawMutex, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a {
        async move {
            match service {
                WifiServiceRequest::StandaloneScan { request, .. } => {
                    await_stack_boundary!(
                        self.run_standalone_scan(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::StandaloneMonitor { plan, request } => {
                    let Some(monitor_plan) = plan.standalone_monitor() else {
                        endpoint
                            .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::rejected(
                                    request,
                                    Esp32s31RadioError::Planning(
                                        WifiServicePlanningError::Request(
                                            open_esp_radio::WifiServiceRequestError::NotStandaloneMonitorTopology,
                                        ),
                                    ),
                                ),
                            )))
                            .await;
                        return EmbassyWifiRoleEpochOutcome::NotStarted(stopped);
                    };
                    let channel_policy = request.channel_policy();
                    let channel = channel_policy.initial_channel();
                    let snapshot_length = request.capture_policy().snapshot_length();
                    let (
                        wifi,
                        physical_resources,
                        station_resources,
                        access_point_resources,
                        monitor_resources,
                    ) = stopped.into_parts();
                    let (physical_resources, halted_ring) =
                        physical_resources.take_halted_rx();
                    let discarded = self.monitor_capture.discard_queued();
                    crate::monitor::record_discarded_monitor_frames(discarded);
                    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(
                        monitor_plan,
                        wifi,
                        monitor_resources.bind(generation, snapshot_length, halted_ring),
                    ) {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            let faulted = ProductionWifiFault::MonitorBuild {
                                _failure: failure,
                                _physical: physical_resources,
                                _station: station_resources,
                            };
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                    WifiStartFailure::faulted(self.fault_error(&faulted)),
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let mut observer = NoopPhyTargetObserver;
                    if let Err(error) = await_stack_boundary!(
                        task.switch_channel::<EmbassyEsp32s31PhyDelay, _>(channel, &mut observer),
                    ) {
                        let faulted = ProductionWifiFault::MonitorChannel {
                            _error: error,
                            _task: task,
                            _physical: physical_resources,
                            _station: station_resources,
                        };
                        endpoint
                            .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::faulted(self.fault_error(&faulted)),
                            )))
                            .await;
                        return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                    }
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                            WifiStartReport::new(generation),
                        )))
                        .await;
                    let exit = await_stack_boundary!(drive_esp32s31_monitor_role(
                        endpoint,
                        &mut controller,
                        task,
                        channel_policy,
                        EmbassyEsp32s31PhyDelay,
                        &mut observer,
                        Esp32s31RadioError::RoleActive,
                    ));
                    let frontier = await_stack_boundary!(finish_embassy_wifi_active_role(
                        endpoint,
                        generation,
                        exit,
                        |output| match output {
                            Esp32s31MonitorTaskExit::Stopped { stopped, .. } => {
                                let discarded = self.monitor_capture.discard_queued();
                                crate::monitor::record_discarded_monitor_frames(discarded);
                                let (monitor, halted_ring) =
                                    ProductionMonitorResources::from_stopped(stopped.resources);
                                EmbassyWifiRoleFrontier::Stopped(
                                    Esp32s31WifiSupervisorStopped::new(
                                        stopped.wifi,
                                        physical_resources.restore_halted_rx(halted_ring),
                                        station_resources,
                                        access_point_resources,
                                        monitor,
                                    ),
                                )
                            }
                            Esp32s31MonitorTaskExit::Faulted { task, .. } => {
                                EmbassyWifiRoleFrontier::Faulted(
                                    ProductionWifiFault::MonitorRuntime {
                                        _task: task,
                                        _physical: physical_resources,
                                        _station: station_resources,
                                    },
                                )
                            }
                        },
                        |_faulted| Esp32s31RadioError::HardwareFault,
                    ));
                    match frontier {
                        EmbassyWifiRoleFrontier::Stopped(stopped) => {
                            EmbassyWifiRoleEpochOutcome::Stopped(stopped)
                        }
                        EmbassyWifiRoleFrontier::Faulted(faulted) => {
                            EmbassyWifiRoleEpochOutcome::Faulted(faulted)
                        }
                    }
                }
                WifiServiceRequest::Station { request, .. } => {
                    await_stack_boundary!(
                        self.run_station_service(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::AccessPoint { request, .. } => {
                    await_stack_boundary!(
                        self.run_access_point_service(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::StationAccessPoint { request, .. } => {
                    await_stack_boundary!(
                        self.run_station_access_point_service(
                            endpoint, stopped, request, generation
                        )
                    )
                }
            }
        }
    }
}
