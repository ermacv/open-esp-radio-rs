#![forbid(unsafe_code)]

use core::{future::Future, sync::atomic::Ordering};

use embassy_net::{Config as NetworkConfig, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_time::Timer;
use open_esp_radio::{
    adapters::wifi::embassy::station_network::RunningStationNetwork,
    esp32s31::wifi::{
        mac::{
            crypto::CcmpKeyHardware,
            tx::{HeEdcaTxopLimit, TxPhyRate},
        },
        sta::peer::Esp32s31StaConnectedLink,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    connected_sta_port::{
        Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaNetworkTxDomain,
        Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxResources,
    },
    connected_sta_teardown::Esp32s31ConnectedStaTeardownFailure,
    network_rx::EmbassyNetConnectedRxSink,
    rx_dma_service::Esp32s31RxEpochResources,
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        Esp32s31ConnectedDriverAssembly, Esp32s31ConnectedDriverAssemblyResources,
        Esp32s31ConnectedEpochStartFailure, Esp32s31ConnectedEpochStarted,
        Esp32s31ConnectedNetworkStartedParts, Esp32s31ConnectedRunObserver,
        Esp32s31ConnectedServiceTeardownFailure, Esp32s31ConnectedStationExit,
        Esp32s31StationReconnectSource, activate_esp32s31_connected_epoch,
        assemble_esp32s31_connected_driver, prepare_esp32s31_connected_service,
        run_and_quiesce_esp32s31_connected_epoch, start_esp32s31_initial_connected_epoch,
        start_esp32s31_reconnected_connected_epoch,
    },
};

struct RadioHilConnectedRunObserver {
    counters: &'static open_esp_radio_hil_esp32s31_telemetry::task_poll::TaskPollCounters,
    enabled: bool,
}

impl Esp32s31ConnectedRunObserver for RadioHilConnectedRunObserver {
    fn observe<'a, F>(&'a mut self, future: F) -> impl Future<Output = F::Output> + 'a
    where
        F: Future + 'a,
    {
        observe_open_radio_task_polls(future, self.counters, self.enabled)
    }
}
use open_esp_radio_hil_protocol::{
    NetworkIpv4Configuration, StationDisconnectReason, StationFaultClassification,
    StationFaultEvidence, StationLifecycleEvent,
};

use crate::{
    console::emergency_log,
    radio_fault::{FaultInjectingConnectedServices, FaultInjectingServicesError},
    radio_hil::{
        HilConnectedRxObserver, NETWORK_FRAME_CAPACITY, NETWORK_RX_QUEUE_DEPTH,
        NETWORK_TX_QUEUE_DEPTH, OPEN_RADIO_MAC_IRQ_CLASSIFICATION, OPEN_RADIO_MAC_IRQ_ENTRIES,
        OPEN_RADIO_RX_PIPELINE_COUNTERS, OPEN_RADIO_TCP_BENCH, OpenRadioRxReloadDelay,
        RX_BLOCK_ACK_SOFTWARE_WINDOW, RX_STAGE_CAPACITY, RX_STAGE_SLOT_COUNT,
        RadioHilConnectedEpochBindings, RadioHilConnectedEpochResources,
        RadioHilConnectedEpochReturn, RadioHilConnectedExit, RadioHilConnectedServiceResources,
        RadioHilConnectedTrafficConfig, RadioHilDisconnectedEpoch, RadioHilStationCommandReceiver,
        RadioHilStationEpochProgress, TX_AMPDU_FRAME_COUNT, connected_network_report_task,
        connected_network_stack_task, connected_rx_protocol_task,
        connected_traffic::observe_open_radio_task_polls, connected_traffic_task,
        injected_tx_source_requires_reset,
    },
};

pub(in crate::radio_hil) async fn run_connected_network<'fixture, 'security>(
    resources: RadioHilConnectedServiceResources<'fixture, 'security>,
    generation: u32,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
) -> RadioHilConnectedEpochReturn<'fixture, 'security> {
    let prepared = prepare_esp32s31_connected_service::<
        TX_AMPDU_FRAME_COUNT,
        RX_BLOCK_ACK_SOFTWARE_WINDOW,
        _,
        _,
        _,
    >(resources)
    .unwrap_or_else(|failure| panic!("invalid connected STA policy: {:?}", failure.error));
    let reconnected_epoch = matches!(
        prepared.epoch(),
        RadioHilConnectedEpochResources::Reconnected(_)
    );
    let started = prepared.start_network(|fixture, device, connected_plan| {
        let RadioHilConnectedEpochBindings {
            storage,
            services: _,
            policy,
        } = fixture.board().connected_epoch_bindings();
        let station_address = connected_plan.link().station_address;
        let stack_resources = storage.stack.take();
        let mut seed = [0_u8; 8];
        seed[..6].copy_from_slice(&station_address);
        seed[6..].copy_from_slice(&0x31a5_u16.to_le_bytes());
        // Keep the controlled local throughput setup independent of DHCP
        // while preserving DHCP as an end-to-end router test.
        let network_config = match policy.ipv4 {
            NetworkIpv4Configuration::Dhcp => NetworkConfig::dhcpv4(Default::default()),
            NetworkIpv4Configuration::Static {
                address,
                prefix_length,
                gateway,
            } => NetworkConfig::ipv4_static(StaticConfigV4 {
                address: Ipv4Cidr::new(Ipv4Address::from_octets(address), prefix_length),
                gateway: gateway.map(Ipv4Address::from_octets),
                dns_servers: Default::default(),
            }),
        };
        let (stack, stack_runner) = embassy_net::new(
            device,
            network_config,
            stack_resources,
            u64::from_le_bytes(seed),
        );
        (stack, stack_runner)
    });
    let Esp32s31ConnectedNetworkStartedParts {
        runtime: fixture,
        epoch: epoch_resources,
        stack,
        network: network_runner,
        initial_network_task: stack_runner,
        plan: connected_plan,
        pairwise: pairwise_slot,
        group: group_slot,
        security,
    } = started.into_parts();
    let runtime = fixture.into_parts();
    let (mut role, mut interrupt_epoch) = runtime.radio.into_parts();
    let (_state, platform) = role.radio_mut();
    let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
    let (rx_storage, descriptor_base, buffer_addresses) = dma.into_parts();
    let (
        spawner,
        protocol_spawner,
        station_interface,
        connected_tasks,
        connected_rx,
        network_report,
        connected_epoch,
        station_control_resources,
    ) = runtime.board.into_parts();
    let RadioHilConnectedEpochBindings {
        mut storage,
        services: epoch_services,
        policy: epoch_policy,
    } = connected_epoch;
    let rx_protocol_runtime = storage.rx_protocol;
    let open_esp_radio::esp32s31::wifi::sta::attempt::Esp32s31StaAttemptSecurity {
        pmk,
        supplicant_nonce,
        sequences,
        message4_protection,
        ..
    } = security;
    let connected_rx_irq_start = epoch_services.irq.rx_post_count();
    let connected_mac_irq_start = OPEN_RADIO_MAC_IRQ_ENTRIES.load(Ordering::Relaxed);
    let connected_irq_classification_start = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.snapshot();
    let connected_rx_pipeline_start = OPEN_RADIO_RX_PIPELINE_COUNTERS.snapshot();
    let link = connected_plan.link();
    let Esp32s31StaConnectedLink {
        station_address,
        association_phy,
        ..
    } = link;
    // The polling-only scan/auth path kept every MAC interrupt masked. Consume
    // the last task-side enable/clear capability immediately before the
    // connected path enables the ISR-owned RX/TX Signal sink.
    // After `activate`, ordinary `RadioRegisters` cannot touch those
    // registers.
    if let Err(error) = activate_esp32s31_connected_epoch(&mut interrupt_epoch, platform) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=production-interrupt-start \
             error={error:?} reset_required=1"
        ));
        // The host resets the board after observing the typed failure line.
        // Retain the complete function frame until then.
        loop {
            Timer::after_secs(60).await;
        }
    }

    let data_tx_rate = connected_plan.data_tx_rate();
    let benchmark_tx_rate = connected_plan.aggregate_tx_rate();
    let peer_ampdu_limit = tx_storage
        .control()
        .expect("control TX owner is present before connected handoff")
        .policy()
        .ht_ampdu()
        .maximum_aggregate_bytes();
    let rate_ampdu_limit = match benchmark_tx_rate {
        TxPhyRate::Legacy(_) => 0,
        TxPhyRate::Ht(rate) => u32::from(rate.vendor_ampdu_byte_limit().unwrap_or(u16::MAX)),
        TxPhyRate::He(rate) => rate.maximum_apep_bytes(HeEdcaTxopLimit::DEFAULT),
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-start \
         frame_capacity={NETWORK_FRAME_CAPACITY} \
         rx_queue_depth={NETWORK_RX_QUEUE_DEPTH} tx_queue_depth={NETWORK_TX_QUEUE_DEPTH} \
         rx_stage_slots={RX_STAGE_SLOT_COUNT} rx_stage_capacity={RX_STAGE_CAPACITY} \
         rx_ba_window={RX_BLOCK_ACK_SOFTWARE_WINDOW} \
         bandwidth_mhz={} phy={} data_rate_code={:#04x} data_rate_kbps={} \
         ampdu_rate_code={:#04x} ampdu_rate_kbps={} peer_ampdu_limit={} rate_ampdu_limit={}",
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
        benchmark_tx_rate.code(),
        benchmark_tx_rate.nominal_kbps(),
        peer_ampdu_limit,
        rate_ampdu_limit,
    ));

    let (staged_rx_sender, staged_rx_receiver) = epoch_services.staged_rx.split();
    let start = match epoch_resources {
        RadioHilConnectedEpochResources::Initial { hardware, receive } => {
            let Some(initial) = storage.initial.take() else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=connected-initial-resources-unavailable"
                ));
                let _owners = (hardware, receive, network_runner);
                loop {
                    Timer::after_secs(60).await;
                }
            };
            let rx = Esp32s31RxEpochResources::new(
                rx_storage,
                epoch_services.rx_stage_pool,
                staged_rx_sender,
                OpenRadioRxReloadDelay,
            )
            .with_pipeline_observer(epoch_services.rx_pipeline);
            start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx)).await
        }
        RadioHilConnectedEpochResources::Reconnected(epoch) => {
            start_esp32s31_reconnected_connected_epoch(epoch).await
        }
    };
    let started = match start {
        Ok(started) => started,
        Err(failure) => {
            match &failure {
                Esp32s31ConnectedEpochStartFailure::RegisterPublication { error, .. } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-register-publication error={error:?} reset_required=1"
                    ));
                }
                Esp32s31ConnectedEpochStartFailure::Receive { phase, error, .. } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch={phase:?} \
                         error={error:?} reset_required=1"
                    ));
                }
            }
            let _owners = (failure, network_runner);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let Esp32s31ConnectedEpochStarted {
        hardware,
        rx,
        aggregate_tx: tx_ampdu_storage,
        control: control_resources,
    } = started;
    // The policy is statically dispatched inside the production RX owner. It
    // can only narrow admission for a real completed unit; descriptor recycle
    // and staging remain exclusively owned by `Esp32s31ConnectedRx`.
    let rx = rx.with_stage_admission_policy(epoch_services.faults);
    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new(
        network_rx,
        HilConnectedRxObserver::new(control_publisher, station_address, connected_rx),
    )
    .with_pipeline_observer(epoch_services.rx_pipeline);
    let (rx_reorder_sender, rx_reorder_receiver) = epoch_services.rx_reorder_commands.split();
    let tx_sequences = sequences;
    let control_tx = tx_storage
        .take_control()
        .expect("control TX owner moves into the connected runner exactly once");
    let registers = hardware.register_access();
    let assembled =
        match assemble_esp32s31_connected_driver(Esp32s31ConnectedDriverAssemblyResources {
            plan: connected_plan,
            irq: epoch_services.irq,
            network: network_runner,
            hardware,
            rx,
            protocol: Esp32s31ConnectedStaRxProtocolResources {
                frames: staged_rx_receiver,
                irq: epoch_services.irq,
                sink: rx_sink,
                mpdu: frame,
                ethernet,
                reorder_commands: rx_reorder_receiver,
                reorder_storage: epoch_services.rx_reorder_storage,
                runtime: rx_protocol_runtime,
                reorder_scratch: None,
                pipeline_observer: Some(epoch_services.rx_pipeline),
            },
            tx: Esp32s31ConnectedStaTxResources {
                control: control_tx,
                aggregate: tx_ampdu_storage,
                pairwise_key: pairwise_slot,
                sequences: tx_sequences,
                aggregate_tx_observer: Some(epoch_services.aggregate_tx),
                network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
            },
            control: Esp32s31ConnectedStaControlResources {
                receiver: control_receiver,
                reorder_commands: rx_reorder_sender,
            },
            map_services: |services| {
                FaultInjectingConnectedServices::new(services, epoch_services.faults)
            },
        }) {
            Ok(assembled) => assembled,
            Err(failure) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-connected-compose error=tx-owner-active reset_required=1"
                ));
                let _retained_owners = failure;
                loop {
                    Timer::after_secs(60).await;
                }
            }
        };
    let Esp32s31ConnectedDriverAssembly {
        runner: radio_runner,
        protocol: rx_protocol,
        report: _,
    } = assembled;
    let (connected_task_group, protocol_endpoint, traffic_endpoint) =
        match connected_tasks.start_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-connected-task-start error={error:?} reset_required=1"
                ));
                loop {
                    Timer::after_secs(60).await;
                }
            }
        };
    let network_started = stack_runner.is_some();
    if let Some(stack_runner) = stack_runner {
        let stack_task = connected_network_stack_task(stack_runner, connected_tasks)
            .unwrap_or_else(|_| panic!("connected network task allocation failed"));
        spawner.spawn(stack_task);
        let report_task = connected_network_report_task(stack, network_report)
            .unwrap_or_else(|_| panic!("connected network report task allocation failed"));
        spawner.spawn(report_task);
        let traffic_task = connected_traffic_task(stack, registers)
            .unwrap_or_else(|_| panic!("connected traffic task allocation failed"));
        spawner.spawn(traffic_task);
    }
    // embassy-net intentionally stores its Stack/Runner state behind a
    // RefCell and is therefore !Send. Keep that owner, the PAC runner and the
    // MMIO-backed tasks on Core 0. The staged protocol owns only cross-core
    // CriticalSectionRawMutex queues and is compiler-proven Send, so moving it
    // to Core 1 removes one long cooperative poll interval without inventing
    // a fixed per-wake frame ceiling.
    let protocol_task = connected_rx_protocol_task(rx_protocol, connected_tasks, protocol_endpoint)
        .unwrap_or_else(|_| panic!("connected RX protocol task allocation failed"));
    protocol_spawner.spawn(protocol_task);
    epoch_services
        .traffic_start
        .send(RadioHilConnectedTrafficConfig {
            association_phy,
            data_tx_rate: benchmark_tx_rate,
            endpoint: traffic_endpoint,
        })
        .await;
    let benchmark_topology = if OPEN_RADIO_TCP_BENCH {
        "core0-io+core1-pattern"
    } else {
        "core0"
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-task-topology \
         network=core0 rx_protocol=core1 radio=sta-parent-core0 \
         report=core0 benchmark={} network_started={}",
        benchmark_topology,
        u8::from(network_started)
    ));
    crate::console::publish_station_lifecycle(StationLifecycleEvent::Connected { generation })
        .await;
    if reconnected_epoch {
        epoch_services
            .station_reporter
            .report(RadioHilStationEpochProgress::ConnectedRunnerStarted);
    }

    // The radio loop intentionally remains in this parent STA future. Other
    // long-running owners still have independent executor tasks/wakers, while
    // disconnect returns RX/TX/control ownership into the same scope that
    // retains the GTK and platform token. A spawned task could only report
    // the edge and would strand those values in its private task storage.
    let mut run_observer = RadioHilConnectedRunObserver {
        counters: connected_tasks.radio_polls(),
        enabled: connected_tasks.telemetry_enabled(),
    };
    let stopped = match run_and_quiesce_esp32s31_connected_epoch(
        interrupt_epoch,
        platform,
        radio_runner,
        station_control,
        connected_task_group,
        &mut run_observer,
        |exit, runner| match exit {
            Esp32s31ConnectedStationExit::Disconnected => {
                let control = runner.services().inner().control();
                let beacon_monitor = control.beacon_monitor();
                let beacon_lost = control.beacon_lost();
                let rx_irqs = epoch_services
                    .irq
                    .rx_post_count()
                    .wrapping_sub(connected_rx_irq_start);
                let mac_irq_entries = OPEN_RADIO_MAC_IRQ_ENTRIES
                    .load(Ordering::Relaxed)
                    .wrapping_sub(connected_mac_irq_start);
                let irq = OPEN_RADIO_MAC_IRQ_CLASSIFICATION
                    .snapshot()
                    .wrapping_delta_since(connected_irq_classification_start);
                let pipeline = OPEN_RADIO_RX_PIPELINE_COUNTERS
                    .snapshot()
                    .wrapping_delta_since(connected_rx_pipeline_start);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner \
                 exit=disconnected beacon_lost={} beacons_observed={} \
                 beacon_deadline_us={:?} last_control_event={:?} last_tx_failure={:?} \
                 rx_irqs={} mac_irq_entries={} irq_rx_only={} irq_rx_mixed={} \
                 irq_tx_only={} irq_tx_mixed={} irq_other_only={} irq_spurious={} \
                 rx_service_calls={} rx_frontier={} rx_admitted={} protocol_frames={} \
                 tx_active={} tx_prepared={} tx_queue={:?} ordinary_slot={:?} \
                 ordinary_word0={:#010x}",
                    u8::from(beacon_lost),
                    beacon_monitor.map_or(0, |monitor| monitor.observed()),
                    beacon_monitor.and_then(|monitor| monitor.deadline_micros()),
                    control.last_event(),
                    control.last_tx_failure(),
                    rx_irqs,
                    mac_irq_entries,
                    irq.rx_only_entries,
                    irq.rx_mixed_entries,
                    irq.tx_only_entries,
                    irq.tx_mixed_entries,
                    irq.other_only_entries,
                    irq.spurious_entries,
                    pipeline.service_calls,
                    pipeline.completion_frontier_frames,
                    pipeline.admitted_frames,
                    pipeline.protocol_frames,
                    u8::from(runner.services().inner().tx().active()),
                    u8::from(runner.services().inner().tx().has_prepared_network_tx()),
                    runner.services().inner().tx().queue_state(),
                    runner.services().inner().tx().ordinary_slot_state(),
                    runner.services().inner().tx().ordinary_descriptor_word0(),
                ));
                let tx = runner.services().inner().tx();
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner-tx-owner \
                     active={} prepared={} queue={:?} ordinary={:?} ordinary_word0={:#010x}",
                    u8::from(tx.active()),
                    u8::from(tx.has_prepared_network_tx()),
                    tx.queue_state(),
                    tx.ordinary_slot_state(),
                    tx.ordinary_descriptor_word0(),
                ));
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner-ampdu-owner \
                     metadata_state={} dma_free={} held={} standby_free={:?}",
                    tx.aggregate_slot_state_code(),
                    u8::from(tx.aggregate_dma_is_free()),
                    tx.aggregate_held_backings(),
                    tx.standby_aggregate_is_fully_free(),
                ));
                let aggregate = epoch_services.aggregate_tx.snapshot();
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner-ampdu-history \
                     singles={} ba_transitions={} prepared={} standby_prepared={} \
                     publications={} completed={}",
                    aggregate.network_single_mpdu_started,
                    aggregate.block_ack_operational_transitions,
                    aggregate.aggregates_prepared,
                    aggregate.standby_prepared,
                    aggregate.aggregate_publications,
                    aggregate.aggregates_completed,
                ));
                RadioHilConnectedExit::Disconnected { beacon_lost }
            }
            Esp32s31ConnectedStationExit::ReconnectRequested { source } => {
                let source = match source {
                    Esp32s31StationReconnectSource::Controller => "station-controller",
                    Esp32s31StationReconnectSource::CoalescedDisconnect => "coalesced-disconnect",
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=production-runner-stop \
                 source={source} command=Reconnect"
                ));
                RadioHilConnectedExit::ReconnectRequested
            }
            Esp32s31ConnectedStationExit::StationStopped(command) => {
                RadioHilConnectedExit::StationStopped(command)
            }
            Esp32s31ConnectedStationExit::HardwareFailure(error) => match error {
                FaultInjectingServicesError::InjectedTxAfterPublication { fault, source } => {
                    let reset_required = injected_tx_source_requires_reset(&source);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result={} stage=production-runner-fault \
                         injection={:?} request_id={} reset_required={} source={source:?}",
                        if reset_required { "PASS" } else { "FAIL" },
                        fault.injection,
                        fault.request_id,
                        u8::from(reset_required),
                    ));
                    RadioHilConnectedExit::InjectedTxFault {
                        fault,
                        reset_required,
                    }
                }
                FaultInjectingServicesError::Inner(error) => {
                    let services = runner.services().inner();
                    let tx = services.tx();
                    let control = services.control();
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error={error:?} \
                         tx_active={} tx_prepared={} tx_queue={:?} ordinary_slot={:?} \
                         ordinary_word0={:#010x} control_in_flight={} \
                         last_control_event={:?} last_control_tx_failure={:?}",
                        u8::from(tx.active()),
                        u8::from(tx.has_prepared_network_tx()),
                        tx.queue_state(),
                        tx.ordinary_slot_state(),
                        tx.ordinary_descriptor_word0(),
                        u8::from(control.tx_in_flight()),
                        control.last_event(),
                        control.last_tx_failure(),
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-failure-tx-owner \
                         ordinary={:?} ordinary_word0={:#010x} aggregate={:?} \
                         aggregate_dma_free={} standby_free={:?}",
                        tx.ordinary_slot_state(),
                        tx.ordinary_descriptor_word0(),
                        tx.aggregate_slot_state(),
                        u8::from(tx.aggregate_dma_is_free()),
                        tx.standby_aggregate_is_fully_free(),
                    ));
                    RadioHilConnectedExit::HardwareFailure
                }
                FaultInjectingServicesError::InjectionContractViolation { fault, progress } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner-fault \
                         injection={:?} request_id={} contract_progress={progress:?}",
                        fault.injection, fault.request_id,
                    ));
                    RadioHilConnectedExit::HardwareFailure
                }
            },
        },
    )
    .await
    {
        Ok(stopped) => stopped,
        Err(pending) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-interrupt-stop \
                 error={:?} state=stopping",
                pending.error,
            ));
            let _owners = pending;
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let runner_exit = stopped.exit;
    let interrupt_drain = stopped.quiesced.interrupt_drain;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-interrupt-stop \
         rx_wake={} rx_capacity_wake={} tx_events={:#010x} power_events={:#010x}",
        u8::from(interrupt_drain.mac.rx),
        u8::from(interrupt_drain.mac.rx_capacity),
        interrupt_drain.mac.tx_events,
        interrupt_drain.power_events,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-benchmark-stopped"
    ));
    let protocol_shutdown = stopped.quiesced.tasks.shutdown();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stopped \
         queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
        protocol_shutdown.queued_frames,
        protocol_shutdown.retained_frames,
        protocol_shutdown.reorder_commands,
        protocol_shutdown.active_reorders,
    ));
    let stopped = stopped.map_services(|services| services.into_inner());
    let teardown = match stopped.try_teardown(group_slot) {
        Ok(teardown) => teardown,
        Err(Esp32s31ConnectedServiceTeardownFailure {
            interrupt,
            network,
            tasks,
            error:
                Esp32s31ConnectedStaTeardownFailure::Control {
                    error,
                    services,
                    group_key,
                },
            ..
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-control-stop error={error:?}"
            ));
            let _owners = (interrupt, network, tasks, services, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedServiceTeardownFailure {
            interrupt,
            network,
            tasks,
            error:
                Esp32s31ConnectedStaTeardownFailure::Rx {
                    error,
                    hardware,
                    rx,
                    tx,
                    control,
                    group_key,
                },
            ..
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-dma-stop error={error:?}"
            ));
            let _owners = (
                interrupt, network, tasks, hardware, rx, tx, control, group_key,
            );
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedServiceTeardownFailure {
            interrupt,
            network,
            tasks,
            error:
                Esp32s31ConnectedStaTeardownFailure::TxActive {
                    hardware,
                    stopped_rx,
                    tx,
                    control,
                    group_key,
                },
            ..
        }) => {
            if let RadioHilConnectedExit::InjectedTxFault {
                fault,
                reset_required,
            } = runner_exit
            {
                let tx_owner_reset_required = tx.is_reset_required();
                let complete = reset_required && tx_owner_reset_required;
                let evidence = StationFaultEvidence::ConnectedTxResetRequired {
                    classification: if complete {
                        StationFaultClassification::RadioResetRequired
                    } else {
                        StationFaultClassification::ContractViolation
                    },
                    runner_returned: true,
                    executor_tasks_stopped: true,
                    rx_dma_stopped: true,
                    tx_owner_reset_required,
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=production-station-fault-frontier \
                     injection={:?} request_id={} runner_returned=1 tasks_stopped=1 \
                     rx_dma_stopped=1 tx_reset_required={} source_reset_required={}",
                    if evidence.is_complete() {
                        "PASS"
                    } else {
                        "FAIL"
                    },
                    fault.injection,
                    fault.request_id,
                    u8::from(tx_owner_reset_required),
                    u8::from(reset_required),
                ));
                crate::console::publish_station_fault(fault.request_id, evidence).await;
            } else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=production-connected-tx-return error=aggregate-active"
                ));
            }
            let _owners = (
                interrupt, network, tasks, hardware, stopped_rx, tx, control, group_key,
            );
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let interrupt_epoch = teardown.interrupt;
    let stopped_protocol = teardown.tasks;
    let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
    storage.rx_protocol = rx_protocol_runtime;
    let connected_epoch = RadioHilConnectedEpochBindings {
        storage,
        services: epoch_services,
        policy: epoch_policy,
    };
    let network = teardown.network;
    let teardown = teardown.driver;
    let control_shutdown = teardown.control;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-control-stop \
         rx_ba={} tx_ba={} discarded_events={} in_flight={:?}",
        control_shutdown.rx_block_ack_agreements,
        control_shutdown.tx_block_ack_sessions,
        control_shutdown.discarded_events,
        control_shutdown.in_flight,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-dma-stop \
         descriptor_base={:#010x} queued_frames={}",
        teardown.stopped_rx.ring().descriptor_base(),
        teardown.stopped_rx.queued_frames(),
    ));
    let pairwise_cleared = teardown
        .hardware
        .ccmp_entry_is_valid(teardown.keys.pairwise_hardware_index)
        == Some(false);
    let group_cleared = teardown
        .hardware
        .ccmp_entry_is_valid(teardown.keys.group_hardware_index)
        == Some(false);
    let keys_cleared = pairwise_cleared && group_cleared;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result={} stage=production-connected-key-clear \
         pairwise_slot={} group_slot={} pairwise_cleared={} group_cleared={}",
        if keys_cleared { "PASS" } else { "FAIL" },
        teardown.keys.pairwise_hardware_index,
        teardown.keys.group_hardware_index,
        u8::from(pairwise_cleared),
        u8::from(group_cleared),
    ));
    let sequences = teardown.sequences;
    tx_storage
        .restore_resources(teardown.tx_resources)
        .unwrap_or_else(|_| panic!("connected TX return found a live owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-connected-tx-return"
    ));
    let disconnected = RadioHilDisconnectedEpoch::new(
        RunningStationNetwork::new(stack, network),
        teardown.hardware,
        teardown.stopped_rx,
        teardown.aggregate,
        control_resources,
    );
    let lifecycle_event = match runner_exit {
        RadioHilConnectedExit::Disconnected { beacon_lost } => {
            Some(StationLifecycleEvent::Disconnected {
                generation,
                reason: if beacon_lost {
                    StationDisconnectReason::BeaconLoss
                } else {
                    StationDisconnectReason::LinkPolicy
                },
            })
        }
        RadioHilConnectedExit::ReconnectRequested => Some(StationLifecycleEvent::Disconnected {
            generation,
            reason: StationDisconnectReason::ReconnectRequested,
        }),
        RadioHilConnectedExit::StationStopped(_)
        | RadioHilConnectedExit::InjectedTxFault { .. }
        | RadioHilConnectedExit::HardwareFailure => None,
    };
    if let Some(event) = lifecycle_event {
        crate::console::publish_station_lifecycle(event).await;
    }
    if matches!(runner_exit, RadioHilConnectedExit::ReconnectRequested) {
        epoch_services
            .station_reporter
            .report(RadioHilStationEpochProgress::RunnerStopped);
    }
    RadioHilConnectedEpochReturn {
        fixture:
            open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRuntimeResources::new(
                open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRadioResources::new(
                    role,
                    interrupt_epoch,
                ),
                open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationStorageResources::new(
                    super::super::RadioHilStationDmaResources::new(
                        rx_storage,
                        descriptor_base,
                        buffer_addresses,
                    ),
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                ),
                super::super::RadioHilStationBoardResources::new(
                    spawner,
                    protocol_spawner,
                    station_interface,
                    connected_tasks,
                    connected_rx,
                    network_report,
                    connected_epoch,
                    station_control_resources,
                ),
            ),
        disconnected,
        security: open_esp_radio::esp32s31::wifi::sta::attempt::Esp32s31StaAttemptSecurity::new(
            pmk,
            supplicant_nonce,
            sequences,
            message4_protection,
        ),
        exit: runner_exit,
    }
}
