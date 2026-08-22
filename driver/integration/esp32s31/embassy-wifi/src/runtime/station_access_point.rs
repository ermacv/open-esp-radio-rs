//! Same-channel production STA+AP lifecycle transaction.
//!
//! This module is the only composition root allowed to turn a connected
//! station frontier into the paired WDEV graph. It must retain one physical
//! RX producer, one physical TX owner and one MAC interrupt epoch, then return
//! every owner through the ordinary stopped supervisor frontier.

use core::future::ready;
use embassy_time::Timer;

use super::*;
use open_esp_radio::wifi::ieee80211::vif::StaApRxAddresses;
use open_esp_radio::{
    StationAccessPointRequest, StationRequest, StationScanChannels, StationScanPolicy,
};
use open_esp_radio_esp32s31_wifi::wdev::lifecycle::{
    StaApLifecycle, StaApReceiveIdentities, apply_sta_ap_register_action, sta_ap_register_action,
};
use open_esp_radio_esp32s31_wifi_embassy::{
    access_point::{
        Esp32s31AccessPointAmpdu, Esp32s31AccessPointProtocolProcessor,
        network_tx::Esp32s31AccessPointNetworkTx,
        paired_role::{
            Esp32s31StaApAccessPointRole, finish_sta_ap_access_point_role,
            park_sta_ap_access_point_role,
        },
    },
    connected_sta_port::{
        Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaDriverParts,
        Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaPort,
        Esp32s31ConnectedStaRxProcessorResources, Esp32s31ConnectedStaTxResources,
    },
    connected_sta_teardown::{Esp32s31AlreadyStoppedRx, Esp32s31ConnectedStaTeardownPort},
    embassy_rx::EmbassyEsp32s31RxDmaObservationDelay,
    resource_profile::{
        ESP32S31_DEFAULT_RX_REORDER_WINDOW as RX_REORDER_WINDOW,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
    },
    rx_dma_service::{Esp32s31RxEpochResources, Esp32s31StagedRxEpoch},
    sta_ap::{
        Esp32s31StaApControlExit, Esp32s31StaApRxService, Esp32s31StaApStationPrepared,
        Esp32s31StaApStationRxSink, compose_sta_ap_wdev_runner, compose_sta_ap_wdev_services,
        finish_sta_ap_station, prepare_sta_ap_station,
    },
    station::{
        ConnectedWpa2Security, Esp32s31ConnectedEpochResources, Esp32s31ConnectedEpochStarted,
        Esp32s31ConnectedNetworkStartedParts, activate_esp32s31_connected_epoch,
        prepare_esp32s31_connected_service, start_esp32s31_initial_connected_epoch,
        start_esp32s31_reconnected_connected_epoch,
    },
    wdev::{WdevRunnerExit, paired::WdevPairRole, services::WdevServiceSet},
};
use open_esp_radio_wifi_embassy::station_network::RunningStationNetwork;

use crate::connected::{
    IRQ_RUNTIME, RX_REORDER_COMMANDS, RX_REORDER_STORAGE, RX_STAGE_POOL, STA_AP_STAGED_RX_QUEUE,
    STAGED_RX_QUEUE,
};

/// Result of advancing the finite station lifecycle to its paired cutover
/// edge. A normal pre-connect terminal edge remains reusable; a hardware
/// contradiction remains quarantined in the existing production fault type.
enum ProductionPairedStationFrontier {
    Connected {
        resources: ConnectedStationResources<'static, 'static>,
        station: Esp32s31StaAttemptStation,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    },
    NotConnected(ProductionSupervisorStopped),
}

fn constrain_station_to_paired_channel(
    request: StationRequest,
    channel: WifiChannel,
) -> StationRequest {
    let (discovery, security, reconnect) = request.into_parts();
    let scan = discovery.scan();
    let channels = StationScanChannels::from_primary_channels(&[channel.primary()])
        .expect("validated 2.4-GHz AP channel is a valid station scan channel");
    StationRequest::new(
        discovery.ssid(),
        security,
        reconnect,
        StationScanPolicy::new(channels, scan.dwell(), scan.association_preference()),
    )
}

impl ProductionWifiEpochRunner {
    async fn advance_station_to_paired_frontier(
        &self,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
    ) -> Result<ProductionPairedStationFrontier, ProductionWifiFault> {
        let (_control, mut task) =
            self.prepare_station_task(stopped, request, ProductionStationMode::PairedCutover)?;
        let exit = await_stack_boundary!(task.run());
        let resources = match exit {
            Esp32s31StationExit::Stopped {
                resources,
                reason:
                    open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationStopReason::Backend,
                ..
            } => resources,
            Esp32s31StationExit::Stopped { resources, .. }
            | Esp32s31StationExit::RetryExhausted { resources, .. }
            | Esp32s31StationExit::Terminal { resources, .. } => {
                return restore_production_station_frontier(resources)
                    .map(ProductionPairedStationFrontier::NotConnected);
            }
            Esp32s31StationExit::Faulted { fault, runner, .. } => {
                return Err(ProductionWifiFault::Station {
                    _fault: fault,
                    _runner: runner,
                });
            }
        };
        let (owner, runner) = resources.into_parts();
        let (runtime, phase, security) = owner.into_parts();
        let ProductionStationPhase::Connected { connected } = phase else {
            return Err(ProductionWifiFault::PairedStationPhase {
                _owner: ProductionStationOwner::new(runtime, phase, security),
                _runner: runner,
            });
        };
        let ProductionConnectedPhase {
            epoch,
            network,
            station,
            peer,
            pairwise,
            group,
        } = connected;
        let (access_point, monitor) = runner.into_port().into_parked_roles();
        let interface = runtime.board().interface;
        Ok(ProductionPairedStationFrontier::Connected {
            resources: ConnectedStationResources::new(
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
            station,
            access_point,
            monitor,
        })
    }

    /// Run the one production same-channel STA+AP epoch.
    ///
    /// Capability publication remains false until this transaction and its
    /// inverse pass the full HIL matrix. The implementation nevertheless
    /// exists in production code so HIL exercises the compiled owner graph,
    /// not a shadow harness.
    pub(super) async fn run_station_access_point_service(
        &mut self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: StationAccessPointRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (station_request, access_point_request) = request.into_parts();
        let station_request =
            constrain_station_to_paired_channel(station_request, access_point_request.channel());
        let frontier = match await_stack_boundary!(
            self.advance_station_to_paired_frontier(stopped, station_request)
        ) {
            Ok(frontier) => frontier,
            Err(faulted) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(self.fault_error(&faulted)),
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        let ProductionPairedStationFrontier::Connected {
            resources,
            station,
            access_point,
            monitor,
        } = frontier
        else {
            let ProductionPairedStationFrontier::NotConnected(stopped) = frontier else {
                unreachable!("paired station frontier has two exhaustive states")
            };
            endpoint
                .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(Esp32s31RadioError::HardwareFault),
                )))
                .await;
            qualification_event!("open-radio: paired cutover failed before connected frontier");
            return EmbassyWifiRoleEpochOutcome::NotStarted(stopped);
        };

        let parts = resources.into_parts();
        let prepared = match prepare_esp32s31_connected_service::<
            TX_AMPDU_FRAME_COUNT,
            RX_REORDER_WINDOW,
            _,
            _,
            _,
        >(
            open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31ConnectedServiceResources::new(
                parts.runtime,
                parts.epoch,
                parts.network,
                parts.interface,
                parts.config,
                parts.peer,
                parts.pairwise,
                parts.group,
                parts.security,
            ),
        ) {
            Ok(prepared) => prepared,
            Err(failure) => {
                // The station lifecycle used this same validated config to
                // produce the connected frontier. A contradiction here is a
                // non-reusable software invariant, not a recoverable request.
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(Esp32s31RadioError::HardwareFault),
                    )))
                    .await;
                qualification_event!(
                    "open-radio: paired cutover rejected connected service policy: {:?}",
                    failure.error
                );
                let error = failure.error;
                return EmbassyWifiRoleEpochOutcome::Faulted(
                    ProductionWifiFault::PairedConnected {
                        _fault: ConnectedStationFault::InvalidConnectedPolicy {
                            _resources: failure.into_resources(),
                            _error: error,
                        },
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                );
            }
        };
        let mut started = prepared.start_network(|_runtime, (), _plan| ((), ()));
        let station_channel = station.selected_channel().unwrap_or_else(|_| {
            let (runtime, _) = started.runtime_and_epoch_mut();
            runtime.radio().owner().current_channel()
        });
        qualification_event!(
            "open-radio: paired channel station={station_channel:?} requested={:?}",
            access_point_request.channel()
        );
        if station_channel != access_point_request.channel() {
            endpoint
                .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(Esp32s31RadioError::Planning(
                        WifiServicePlanningError::Request(
                            open_esp_radio::WifiServiceRequestError::StationAccessPointChannelMismatch,
                        ),
                    )),
                )))
                .await;
            qualification_event!("open-radio: paired cutover channel mismatch");
            return EmbassyWifiRoleEpochOutcome::Faulted(
                ProductionWifiFault::PairedChannelMismatch {
                    _started: started,
                    _station: station,
                    _access_point: access_point,
                    _monitor: monitor,
                },
            );
        }
        let activation = {
            let (runtime, epoch) = started.runtime_and_epoch_mut();
            let (radio, _storage, _board) = runtime.split_mut();
            let (_phy, platform, interrupt) = radio.parts_mut();
            match interrupt.setup_mut() {
                Ok(setup) => {
                    let prepared = match epoch {
                        Esp32s31ConnectedEpochResources::Initial { hardware, .. } => {
                            Ok(setup.prepare_connected_sta_without_power_save(hardware))
                        }
                        Esp32s31ConnectedEpochResources::Reconnected(reconnected) => reconnected
                            .hardware_mut()
                            .register_access()
                            .try_prepare_connected_sta_without_power_save(setup)
                            .map_err(|_| ()),
                    };
                    prepared.map_err(|_| ()).and_then(|prepared| {
                        activate_esp32s31_connected_epoch(interrupt, platform, prepared)
                            .map_err(|_| ())
                    })
                }
                Err(_) => Err(()),
            }
        };
        if activation.is_err() {
            endpoint
                .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(Esp32s31RadioError::HardwareFault),
                )))
                .await;
            return EmbassyWifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedConnected {
                _fault: ConnectedStationFault::InterruptActivation { _started: started },
                _station: station,
                _access_point: access_point,
                _monitor: monitor,
            });
        }

        await_stack_boundary!(self.run_station_access_point_active(
            endpoint,
            started,
            station,
            access_point,
            monitor,
            access_point_request,
            station_channel,
            generation,
        ))
    }

    /// Own the active paired epoch after association, channel validation and
    /// interrupt activation. Keeping this as a separate poll boundary avoids
    /// retaining the large station-association transaction on the CPU stack
    /// while the long-running dual-interface WDEV graph is active.
    async fn run_station_access_point_active(
        &mut self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        started: crate::connected::ConnectedNetworkStarted<'static, 'static>,
        station: Esp32s31StaAttemptStation,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
        access_point_request: AccessPointRequest,
        station_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let Esp32s31ConnectedNetworkStartedParts {
            runtime,
            epoch,
            stack: (),
            network: network_runner,
            initial_network_task,
            mut plan,
            pairwise,
            group,
            security,
        } = started.into_parts();
        debug_assert!(initial_network_task.is_none());
        let _ = initial_network_task;
        let runtime = runtime.into_parts();
        let (mut role, mut interrupt_epoch) = runtime.radio.into_parts();
        role.set_current_channel(station_channel);
        let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
        let mut board = runtime.board;

        let (standalone_sender, _standalone_receiver) = STAGED_RX_QUEUE.split();
        let start = match epoch {
            ConnectedStationEpoch::Initial { hardware, receive } => {
                let initial = board
                    .initial_connected
                    .take()
                    .expect("paired initial connected frontier owns static resources");
                let rx = Esp32s31RxEpochResources::new(
                    dma.storage(),
                    &RX_STAGE_POOL,
                    standalone_sender,
                    EmbassyEsp32s31RxDmaObservationDelay,
                );
                start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx)).await
            }
            ConnectedStationEpoch::Reconnected(epoch) => {
                start_esp32s31_reconnected_connected_epoch(epoch).await
            }
        };
        let Esp32s31ConnectedEpochStarted {
            mut hardware,
            rx,
            aggregate_tx: aggregate,
            control: control_resources,
        } = match start {
            Ok(started) => started,
            Err(failure) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(Esp32s31RadioError::HardwareFault),
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(
                    ProductionWifiFault::PairedConnected {
                        _fault: ConnectedStationFault::EpochStart {
                            _runtime: production_station_runtime(
                                role,
                                interrupt_epoch,
                                dma,
                                tx_storage,
                                scan_table,
                                frame,
                                ethernet,
                                board,
                            ),
                            _failure: failure,
                            _stack: (),
                            _network: network_runner,
                            _initial_network_task: initial_network_task,
                            _plan: plan,
                            _pairwise: pairwise,
                            _group: group,
                            _security: security,
                        },
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                );
            }
        };
        let mut live_rx = rx;
        let stopped_rx = loop {
            match live_rx.try_stop(&mut hardware) {
                Ok(stopped) => break stopped,
                Err((returned, error)) => {
                    qualification_event!(
                        "open-radio: paired RX cutover stop still pending: {error:?}"
                    );
                    live_rx = returned;
                    Timer::after_millis(1).await;
                }
            }
        };
        let (paired_sender, paired_consumer) = STA_AP_STAGED_RX_QUEUE.split();
        let receive_identities = StaApReceiveIdentities {
            station_address: plan.link().station_address,
            station_bssid: plan.link().bssid,
            access_point_address: access_point.address,
        };
        let paired_rx = Esp32s31StagedRxEpoch::try_from_stopped_sta_ap(
            stopped_rx,
            paired_sender,
            plan.rx_config().ingress,
            StaApRxAddresses {
                station: receive_identities.station_address,
                station_bssid: receive_identities.station_bssid,
                access_point: receive_identities.access_point_address,
            },
        )
        .unwrap_or_else(|_| unreachable!("new connected RX has no queued standalone leases"));
        let common_rx = Esp32s31StaApRxService::new(paired_rx, paired_consumer);

        let control_tx = tx_storage
            .take_control()
            .unwrap_or_else(|_| unreachable!("paired cutover starts with idle ordinary TX"));
        let ProductionStationBoardResources {
            interface,
            rx_protocol_runtime,
            sta_ap_rx_batch,
            initial_connected,
            #[cfg(feature = "qualification")]
            qualification,
        } = board;
        let Esp32s31StaAttemptSecurity {
            pmk,
            supplicant_nonce,
            sequences,
            message4_protection,
            connected,
            ..
        } = security;
        let (control_publisher, control_receiver) = control_resources.split();
        let station_sink = Esp32s31StaApStationRxSink::new(sta_ap_rx_batch, control_publisher);
        let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
        let station_rx = Esp32s31ConnectedStaPort::build_rx_processor(
            &plan,
            Esp32s31ConnectedStaRxProcessorResources {
                irq: &IRQ_RUNTIME,
                sink: station_sink,
                mpdu: frame,
                ethernet,
                reorder_commands: reorder_receiver,
                reorder_storage: &RX_REORDER_STORAGE,
                runtime: rx_protocol_runtime,
                reorder_scratch: None,
                pipeline_observer: {
                    #[cfg(feature = "qualification")]
                    {
                        qualification.map(|hooks| hooks.rx_pipeline)
                    }
                    #[cfg(not(feature = "qualification"))]
                    {
                        None
                    }
                },
            },
        );
        let station_tx = Esp32s31ConnectedStaPort::build_tx(
            &mut plan,
            Esp32s31ConnectedStaTxResources {
                control: control_tx,
                aggregate,
                pairwise_key: pairwise,
                sequences,
                aggregate_tx_observer: {
                    #[cfg(feature = "qualification")]
                    {
                        qualification.map(|hooks| hooks.aggregate_tx)
                    }
                    #[cfg(not(feature = "qualification"))]
                    {
                        None
                    }
                },
                network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
            },
        )
        .unwrap_or_else(|_| unreachable!("paired cutover starts with quiescent connected TX"));
        let mut station_control = Esp32s31ConnectedStaPort::build_control(
            &plan,
            Esp32s31ConnectedStaControlResources {
                receiver: control_receiver,
                reorder_commands: reorder_sender,
                rx_block_ack: &super::PRODUCTION_RX_BLOCK_ACK,
            },
        );
        station_control
            .install_wpa2_security(ConnectedWpa2Security::new(
                connected.expect("installed station keys retain supplicant state"),
                group,
            ))
            .unwrap_or_else(|_| unreachable!("fresh station control has no security session"));
        let station_drivers = Esp32s31ConnectedStaPort::assemble(
            plan,
            Esp32s31ConnectedStaDriverParts {
                hardware,
                rx: (),
                tx: station_tx,
                control: station_control,
                protocol: station_rx,
            },
        );
        let prepared_station = prepare_sta_ap_station(station_drivers)
            .unwrap_or_else(|_| unreachable!("new connected TX can be parked before WDEV runs"));

        let Esp32s31StaApStationPrepared {
            mut hardware,
            physical_rx: (),
            station: station_role,
            mut physical_tx,
            report: station_report,
        } = prepared_station;
        let (ordinary, aggregate_resources) = physical_tx
            .try_lend(WdevPairRole::Second)
            .unwrap_or_else(|_| unreachable!("station preparation returns available physical TX"));
        let (ssid, access_point_security, channel, client_limit, inactive_timeout) =
            access_point_request.into_parts();
        let ProductionAccessPointResources {
            address,
            beacon,
            rx_frame,
            tx_frame,
            peer_storage,
            pairwise_storage,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
        } = access_point;
        assert!(
            core::ptr::eq(&super::PRODUCTION_RX_BLOCK_ACK, rx_block_ack),
            "paired STA and AP must share the one ordinary RX BlockAck bank owner",
        );
        let mut gtk_key = [0_u8; 16];
        for word in gtk_key.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        let gtk = Wpa2Gtk::new(1, true, gtk_key)
            .unwrap_or_else(|_| unreachable!("production GTK key id is statically valid"));
        let access_point_service = AccessPointService::new(
            address,
            access_point_security.into_pmk(),
            gtk,
            client_limit,
            inactive_timeout,
            peer_storage,
        );
        let engine = Esp32s31ApEngine::start(
            &mut hardware,
            access_point_service,
            beacon,
            pairwise_storage,
            &ssid,
            channel,
            AccessPointRequest::BEACON_INTERVAL_TU,
            AccessPointRequest::DTIM_PERIOD,
        )
        .unwrap_or_else(|_| {
            unreachable!("validated paired AP resources must start on the associated channel")
        });
        let mut lifecycle = StaApLifecycle::new();
        let station_transition = lifecycle
            .start_station(channel)
            .unwrap_or_else(|_| unreachable!("fresh paired lifecycle accepts its station owner"));
        debug_assert!(matches!(
            sta_ap_register_action(station_transition, receive_identities),
            open_esp_radio_esp32s31_wifi::wdev::lifecycle::StaApRegisterAction::None
        ));
        let access_point_transition = lifecycle
            .start_access_point(channel)
            .unwrap_or_else(|_| unreachable!("validated paired roles share one exact channel"));
        apply_sta_ap_register_action(
            &mut hardware,
            sta_ap_register_action(access_point_transition, receive_identities),
        );
        let maximum_aggregate_bytes = ordinary.policy.ht_ampdu().maximum_aggregate_bytes();
        let access_point_aggregate = Esp32s31AccessPointAmpdu::new(
            aggregate_resources,
            maximum_aggregate_bytes,
            open_esp_radio::esp32s31::wifi::mac::tx_runtime::VENDOR_LONG_RETRY_LIMIT,
        );
        let access_point_mac = Esp32s31ApMac::new(
            engine,
            ordinary,
            Esp32s31ApTxConfig {
                publication_timeout_micros: TX_COMPLETION_TIMEOUT_US,
            },
        );
        let access_point_processor = Esp32s31AccessPointProtocolProcessor::new(
            access_point_mac,
            rx_frame,
            tx_frame,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
        );
        let access_point_network_tx = Esp32s31AccessPointNetworkTx::<RadioTxBacking>::new({
            #[cfg(feature = "qualification")]
            {
                qualification.map(|hooks| hooks.aggregate_tx)
            }
            #[cfg(not(feature = "qualification"))]
            {
                None
            }
        });
        let access_point_role = Esp32s31StaApAccessPointRole::new(
            access_point_processor,
            access_point_network_tx,
            || {
                let mut nonce = [0_u8; 32];
                for word in nonce.chunks_exact_mut(4) {
                    word.copy_from_slice(&Rng::new().random().to_le_bytes());
                }
                let replay =
                    (u64::from(Rng::new().random()) << 32) | u64::from(Rng::new().random());
                (nonce, replay)
            },
            publish_access_point_shared_network_rx as fn(u8),
        );
        let access_point_role = park_sta_ap_access_point_role(
            access_point_role,
            access_point_aggregate,
            &mut physical_tx,
        )
        .unwrap_or_else(|_| unreachable!("new paired AP TX has no active transaction"));

        let services = compose_sta_ap_wdev_services(
            hardware,
            physical_tx,
            common_rx,
            station_role,
            access_point_role,
        );
        let mut paired_runner = compose_sta_ap_wdev_runner(&IRQ_RUNTIME, network_runner, services);
        #[cfg(feature = "qualification")]
        let rx_statistics_before = paired_runner
            .services()
            .hardware()
            .receive_statistics_snapshot();
        {
            let (hardware, rx) = paired_runner.services_mut().hardware_and_rx_mut();
            rx.start(hardware).await.unwrap_or_else(|_| {
                unreachable!("paired RX restarts the just-stopped descriptor epoch")
            });
        }
        endpoint
            .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Ok(
                WifiStartReport::new(generation),
            )))
            .await;
        let run_exit = await_stack_boundary!(paired_runner.run_until(
            super::access_point::wait_for_active_wifi_role_stop(endpoint),
        ));
        let (stop_requested, active_fault, station_disconnect) = match run_exit {
            Ok(WdevRunnerExit::Stopped) => (true, false, None),
            Ok(WdevRunnerExit::Role(Esp32s31StaApControlExit::Station(reason))) => {
                (false, false, Some(reason))
            }
            Err(error) => {
                qualification_event!("open-radio: paired WDEV service failed: {error:?}");
                (false, true, None)
            }
        };
        if let Some(reason) = station_disconnect {
            qualification_event!("open-radio: paired station disconnected reason={reason:?}");
            #[cfg(feature = "qualification")]
            if let Some(hooks) = qualification {
                (hooks.station_lifecycle)(
                    crate::Esp32s31StationLifecycleObservation::Disconnected(reason),
                );
            }
        }
        if !stop_requested {
            // Beacon loss or another station control exit tears down the
            // whole same-channel pair. The AP never survives on a channel
            // whose upstream station association no longer owns.
            loop {
                match await_stack_boundary!(paired_runner.run_until(ready(()))) {
                    Ok(WdevRunnerExit::Stopped) => break,
                    Ok(WdevRunnerExit::Role(_)) => {}
                    Err(error) => {
                        qualification_event!(
                            "open-radio: paired WDEV rollback still pending: {error:?}"
                        );
                        Timer::after_millis(1).await;
                    }
                }
            }
        }

        let (_, platform) = role.radio_mut();
        loop {
            match interrupt_epoch.quiesce(&*platform) {
                Ok(_) => break,
                Err(open_esp_radio_esp32s31_wifi_embassy::embassy_irq::Esp32s31MacInterruptEpochQuiesceError::AlreadyQuiesced) => {
                    break;
                }
                Err(open_esp_radio_esp32s31_wifi_embassy::embassy_irq::Esp32s31MacInterruptEpochQuiesceError::Route(error)) => {
                    qualification_event!(
                        "open-radio: paired MAC interrupt quiescence still pending: {error:?}"
                    );
                    Timer::after_millis(1).await;
                }
            }
        }
        let (network_runner, services) = paired_runner.into_parts();
        let (
            mut hardware,
            physical_tx,
            mut common_rx,
            station_role,
            access_point_role,
            _control_arbiter,
        ) = services.into_parts();
        common_rx
            .stop(&mut hardware)
            .unwrap_or_else(|_| unreachable!("stopped paired WDEV has no live RX transaction"));
        let route_report = common_rx.route_report();
        qualification_event!(
            "open-radio: paired RX routes total={} sta={} ap={} foreign={} ambiguous={} malformed={} hardware_error={}",
            route_report.total(),
            route_report.station,
            route_report.access_point,
            route_report.foreign,
            route_report.ambiguous,
            route_report.malformed,
            route_report.hardware_error,
        );
        #[cfg(feature = "qualification")]
        let rx_statistics_delta = hardware
            .receive_statistics_snapshot()
            .primary
            .wrapping_delta_since(rx_statistics_before.primary);
        let (paired_rx, mut paired_consumer) = common_rx.into_parts();
        let producer_report = paired_rx.report();
        qualification_event!(
            "open-radio: paired RX producer completed={} staged={} overload_dropped={} overload_recycled={} overload_sta={} overload_ap={} critical_reserved={} critical_blocked={} min_pool={:?} min_queue={:?}",
            producer_report.completed_units,
            producer_report.staged_units,
            producer_report.overload_discarded_units,
            producer_report.overload_recycled_descriptors,
            producer_report.overload_station_units,
            producer_report.overload_access_point_units,
            producer_report.critical_reserve_admissions,
            producer_report.critical_admission_blocked,
            producer_report.minimum_pool_credits,
            producer_report.minimum_queue_credits,
        );
        let discarded = paired_consumer.discard_queued();
        qualification_debug!(
            "open-radio: paired RX stopped discarded={} serviced={}",
            discarded,
            paired_rx.report().completed_units,
        );
        let (standalone_sender, _standalone_receiver) = STAGED_RX_QUEUE.split();
        let stopped_rx = paired_rx
            .try_into_standalone_stopped(standalone_sender)
            .unwrap_or_else(|_| unreachable!("paired queue was drained before standalone restore"));

        let access_point_finished =
            finish_sta_ap_access_point_role(access_point_role, physical_tx, &mut hardware)
                .unwrap_or_else(|_| unreachable!("paired WDEV stop parks every AP TX owner"));
        let access_point_transition = lifecycle
            .stop_access_point()
            .unwrap_or_else(|_| unreachable!("active paired lifecycle retains its AP role"));
        apply_sta_ap_register_action(
            &mut hardware,
            sta_ap_register_action(access_point_transition, receive_identities),
        );
        lifecycle
            .stop_station()
            .unwrap_or_else(|_| unreachable!("paired teardown retains its final station role"));
        let open_esp_radio_esp32s31_wifi_embassy::access_point::paired_role::Esp32s31StaApAccessPointFinished {
            stopped: access_point_stopped,
            network_tx: _access_point_network_tx,
            security_material: _access_point_security_material,
            publish_shared_rx: _access_point_shared_rx,
            physical_tx,
        } = access_point_finished;
        #[cfg(feature = "qualification")]
        if let Some(hooks) = qualification {
            super::access_point::publish_access_point_observation(
                hooks.access_point,
                station_channel,
                &access_point_stopped.control_report,
                &access_point_stopped.mac_report,
                &access_point_stopped.engine.report,
                rx_statistics_delta.buffer_full,
                rx_statistics_delta.fifo_overflow,
            );
        }
        let prepared_station = Esp32s31StaApStationPrepared {
            hardware,
            physical_rx: (),
            station: station_role,
            physical_tx,
            report: station_report,
        };
        let station_drivers = finish_sta_ap_station(prepared_station)
            .unwrap_or_else(|_| unreachable!("paired WDEV stop parks every station TX owner"));

        let open_esp_radio_esp32s31_wifi_embassy::connected_sta_port::Esp32s31ConnectedStaDrivers {
            services: station_services,
            protocol: station_protocol,
            report: _,
        } = station_drivers;
        let (hardware, (), station_tx, mut station_control) = station_services.into_parts();
        let station_security = station_control
            .take_wpa2_security()
            .expect("paired station control returns its association security owner");
        let (_connected, group) = station_security.into_parts();
        let (stopped_protocol, station_sink) = station_protocol.into_stopped_with_sink();
        let (sta_ap_rx_batch, _control_publisher) = station_sink.into_parts();
        let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
        let teardown = Esp32s31ConnectedStaTeardownPort::try_teardown(
            WdevServiceSet::with_control(
                hardware,
                Esp32s31AlreadyStoppedRx::new(stopped_rx),
                station_tx,
                station_control,
            ),
            group,
        )
        .unwrap_or_else(|_| unreachable!("paired WDEV stop returns idle station services"));
        tx_storage
            .restore_resources(teardown.tx_resources)
            .unwrap_or_else(|_| {
                unreachable!("paired station returns the detached ordinary TX owner")
            });
        let disconnected = ConnectedDisconnectedEpoch::new(
            RunningStationNetwork::new((), network_runner),
            teardown.hardware,
            teardown.stopped_rx,
            teardown.aggregate,
            control_resources,
        );
        let station_security = Esp32s31StaAttemptSecurity::new(
            pmk,
            supplicant_nonce,
            teardown.sequences,
            message4_protection,
        );
        let station_owner = ProductionStationOwner::new(
            production_station_runtime(
                role,
                interrupt_epoch,
                dma,
                tx_storage,
                scan_table,
                frame,
                ethernet,
                ProductionStationBoardResources {
                    interface,
                    rx_protocol_runtime,
                    sta_ap_rx_batch,
                    initial_connected,
                    #[cfg(feature = "qualification")]
                    qualification,
                },
            ),
            ProductionStationPhase::RunningScan {
                disconnected,
                station,
            },
            station_security,
        );
        let open_esp_radio::esp32s31::wifi::ap::engine::Esp32s31ApEngineStop {
            service,
            beacon_storage,
            pairwise_storage,
            security: _,
            report: _engine_report,
        } = access_point_stopped.engine;
        let access_point = ProductionAccessPointResources {
            address: service.address(),
            beacon: beacon_storage,
            rx_frame: access_point_stopped.rx_frame,
            tx_frame: access_point_stopped.tx_frame,
            peer_storage: service.into_peer_storage(),
            pairwise_storage,
            rx_dispatcher: access_point_stopped.data_rx,
            rx_block_ack: access_point_stopped.rx_block_ack,
            rx_reorder: access_point_stopped.rx_reorder,
            rx_reorder_storage: access_point_stopped.rx_reorder_storage,
        };
        let stopped = match try_reclaim_production_station(station_owner) {
            Ok(stopped) => stopped,
            Err(failure) => {
                return EmbassyWifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedReclaim {
                    _station: failure,
                    _access_point: access_point,
                    _monitor: monitor,
                });
            }
        };
        let resources = ProductionWifiStoppedResources::Returned(stopped.resources);
        let (physical, station) = match try_split_wifi_stopped_resources(resources) {
            Ok(resources) => resources,
            Err(resources) => {
                return EmbassyWifiRoleEpochOutcome::Faulted(ProductionWifiFault::StoppedOwner {
                    _wifi: stopped.wifi,
                    _resources: resources,
                    _access_point: access_point,
                    _monitor: monitor,
                });
            }
        };
        let stopped = Esp32s31WifiSupervisorStopped::new(
            stopped.wifi,
            physical,
            station,
            access_point,
            monitor,
        );
        if active_fault {
            return EmbassyWifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedStopped {
                _stopped: stopped,
            });
        }
        if stop_requested {
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                    WifiStopReport::new(generation),
                )))
                .await;
        }
        EmbassyWifiRoleEpochOutcome::Stopped(stopped)
    }
}
