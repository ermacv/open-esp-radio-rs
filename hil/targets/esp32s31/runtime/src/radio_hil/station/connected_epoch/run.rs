#![forbid(unsafe_code)]

use core::cell::RefCell;

use embassy_net::{Config as NetworkConfig, Ipv4Address, Ipv4Cidr, StackResources, StaticConfigV4};
use embassy_net_driver::LinkState;
use embassy_time::Timer;
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        aggregate_tx::AggregateTxResources,
        connected_runner::ConnectedRunner,
        connected_sta_port::{
            Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaDriverParts,
            Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaPort,
            Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxResources,
        },
        connected_sta_teardown::{
            Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownPort,
        },
        cooperative_hardware::CooperativeRadioHardware,
        network_rx::EmbassyNetConnectedRxSink,
        rx_dma_service::Esp32s31RxEpochResources,
        sta_tx_epoch::Esp32s31StaTxEpochExt,
        station::{
            Esp32s31ConnectedStationExit, Esp32s31ConnectedTaskStopOutcome,
            Esp32s31StationReconnectSource, run_esp32s31_connected_station_epoch,
            stop_esp32s31_connected_task_group,
        },
        station_epoch::Esp32s31ReconnectedStaEpochParts,
    },
    esp32s31::wifi::{
        dma::tx_ampdu_storage::AmpduDmaStorage,
        lmac::{
            crypto::{CcmpKeyHardware, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
            init::MAC_COLD_RX_INTERRUPT_MASK,
            tx::{HeEdcaTxopLimit, TxPhyRate},
            tx_ampdu::{HtAmpduTxResources, HtAmpduTxStorage},
        },
        sta::peer::Esp32s31StaConnectedLink,
    },
    wifi::ieee80211::station::StaTxSequenceCounters,
};
use open_esp_radio_hil_protocol::{
    NetworkIpv4Configuration, StationDisconnectReason, StationFaultClassification,
    StationFaultEvidence, StationLifecycleEvent,
};

use crate::{
    console::emergency_log,
    radio_fault::{FaultInjectingConnectedServices, FaultInjectingServicesError},
    radio_hil::{
        ControlResources, HilConnectedRxObserver, NETWORK_FRAME_CAPACITY, NETWORK_RX_QUEUE_DEPTH,
        NETWORK_TX_QUEUE_DEPTH, OpenRadioRxReloadDelay, RX_BLOCK_ACK_SOFTWARE_WINDOW,
        RX_STAGE_CAPACITY, RX_STAGE_SLOT_COUNT, RadioHilConnectedEpochBindings,
        RadioHilConnectedEpochResources, RadioHilConnectedEpochReturn, RadioHilConnectedExit,
        RadioHilConnectedTaskFixture, RadioHilConnectedTaskGroup, RadioHilConnectedTrafficConfig,
        RadioHilDisconnectedEpoch, RadioHilRunningNetwork, RadioHilStaNetwork,
        RadioHilStationCommandReceiver, RadioHilStationEpochProgress, StaAssociationSecurity,
        StaConnectedSession, TX_AMPDU_FRAME_COUNT, connected_network_report_task,
        connected_network_stack_task, connected_rx_protocol_task,
        connected_traffic::observe_open_radio_task_polls, connected_traffic_task,
        injected_tx_source_requires_reset,
    },
};

pub(in crate::radio_hil) async fn run_connected_network<'fixture, 'security>(
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    epoch_resources: RadioHilConnectedEpochResources,
    session: StaConnectedSession<'security>,
    pairwise_slot: StaPairwiseCcmpSlot,
    group_slot: StaGroupCcmpSlot,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
) -> RadioHilConnectedEpochReturn<'fixture, 'security> {
    let reconnected_epoch = matches!(
        &epoch_resources,
        RadioHilConnectedEpochResources::Reconnected(_)
    );
    let RadioHilConnectedTaskFixture {
        spawner,
        protocol_spawner,
        state,
        platform,
        interrupt_epoch,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        frame,
        ethernet,
        connected_tasks,
        connected_rx,
        network_report,
        connected_epoch,
    } = fixture;
    let RadioHilConnectedEpochBindings {
        storage,
        services: epoch_services,
        policy,
    } = connected_epoch;
    let StaConnectedSession {
        generation,
        peer,
        network,
        pmk,
        supplicant_nonce,
        sequences,
    } = session;
    let connected_plan =
        Esp32s31ConnectedStaPort::prepare::<TX_AMPDU_FRAME_COUNT>(peer, policy.station)
            .unwrap_or_else(|failure| panic!("invalid connected STA policy: {:?}", failure.error));
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
    interrupt_epoch
        .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
        .unwrap_or_else(|error| panic!("MAC interrupt epoch activation failed: {error:?}"));

    let (stack, network_runner, stack_runner) = match network {
        RadioHilStaNetwork::Unstarted { device, runner } => {
            let stack_resources = storage.stack.init(StackResources::new());
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
            (stack, runner, Some(stack_runner))
        }
        RadioHilStaNetwork::Running(network) => (network.stack, network.runner, None),
    };
    network_runner.set_link_state(LinkState::Up);
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
    let (hardware, rx, tx_ampdu_storage, control_resources) = match epoch_resources {
        RadioHilConnectedEpochResources::Initial { registers, rx } => {
            let rx_ring = match rx.try_into_live_with_storage(registers, rx_storage).await {
                Ok(ring) => ring,
                Err(failure) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=initial error={:?}",
                        failure.error,
                    ));
                    let _owner = failure.owner;
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            };
            let rx = Esp32s31RxEpochResources::new(
                rx_storage,
                epoch_services.rx_stage_pool,
                staged_rx_sender,
                OpenRadioRxReloadDelay,
            )
            .with_pipeline_observer(epoch_services.rx_pipeline)
            .with_live_ring(rx_ring);
            // The production aggregate owner is descriptor-only
            // (`BUFFER_SIZE == 0`), so constructing it in the static cell does
            // not materialize the former 55-KiB payload arena on this task's
            // stack. This edge belongs exclusively to the first epoch.
            let ampdu = HtAmpduTxResources::pin_static(
                storage.ampdu_metadata.init_with(HtAmpduTxStorage::new),
                storage.ampdu_dma.init_with(AmpduDmaStorage::new),
            )
            .expect("A-MPDU metadata and descriptor storage must be valid");
            let standby_ampdu = HtAmpduTxResources::pin_static(
                storage
                    .ampdu_standby_metadata
                    .init_with(HtAmpduTxStorage::new),
                storage.ampdu_standby_dma.init_with(AmpduDmaStorage::new),
            )
            .expect("standby A-MPDU metadata and descriptor storage must be valid");
            let ampdu = AggregateTxResources::pipelined(ampdu, standby_ampdu);
            let control_resources = storage.control.init(ControlResources::new());
            let registers = storage.registers.init(RefCell::new(registers));
            (
                CooperativeRadioHardware::new(registers),
                rx,
                ampdu,
                &*control_resources,
            )
        }
        RadioHilConnectedEpochResources::Reconnected(epoch) => {
            let Esp32s31ReconnectedStaEpochParts {
                mut hardware,
                rx,
                rx_resources,
                aggregate_tx: ampdu,
                control: control_resources,
            } = epoch.into_parts();
            let rx_ring = match rx
                .try_into_live_with_storage(&mut hardware, rx_storage)
                .await
            {
                Ok(ring) => ring,
                Err(failure) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=reconnected \
                         error={:?}",
                        failure.error,
                    ));
                    let _owners = (
                        hardware,
                        failure.owner,
                        rx_resources,
                        ampdu,
                        control_resources,
                        network_runner,
                    );
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            };
            let rx = rx_resources.with_live_ring(rx_ring);
            (hardware, rx, ampdu, control_resources)
        }
    };
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
    let rx_protocol = Esp32s31ConnectedStaPort::build_rx_protocol(
        &connected_plan,
        Esp32s31ConnectedStaRxProtocolResources {
            frames: staged_rx_receiver,
            irq: epoch_services.irq,
            sink: rx_sink,
            mpdu: frame,
            ethernet,
            reorder_commands: rx_reorder_receiver,
            reorder_storage: epoch_services.rx_reorder_storage,
            reorder_scratch: None,
            pipeline_observer: Some(epoch_services.rx_pipeline),
        },
    );

    let tx_sequences = core::mem::replace(sequences, StaTxSequenceCounters::new(0));
    let control_tx = tx_storage
        .take_control()
        .expect("control TX owner moves into the connected runner exactly once");
    let tx = Esp32s31ConnectedStaPort::build_tx(
        &connected_plan,
        Esp32s31ConnectedStaTxResources {
            control: control_tx,
            aggregate: tx_ampdu_storage,
            pairwise_key: pairwise_slot,
            sequences: tx_sequences,
            aggregate_tx_observer: Some(epoch_services.aggregate_tx),
            network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
        },
    )
    .unwrap_or_else(|_failure| panic!("connected handoff requires an idle control TX owner"));
    let control = Esp32s31ConnectedStaPort::build_control(
        &connected_plan,
        Esp32s31ConnectedStaControlResources {
            receiver: control_receiver,
            reorder_commands: rx_reorder_sender,
        },
    );

    let registers = hardware.register_cell();
    let drivers = Esp32s31ConnectedStaPort::assemble(
        connected_plan,
        Esp32s31ConnectedStaDriverParts {
            hardware,
            rx,
            tx,
            control,
            protocol: rx_protocol,
        },
    );
    let rx_protocol = drivers.protocol;
    let connected_services =
        FaultInjectingConnectedServices::new(drivers.services, epoch_services.faults);
    let mut radio_runner =
        ConnectedRunner::new(epoch_services.irq, network_runner, connected_services);

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
    let protocol_task = connected_rx_protocol_task(rx_protocol, connected_tasks)
        .unwrap_or_else(|_| panic!("connected RX protocol task allocation failed"));
    protocol_spawner.spawn(protocol_task);
    epoch_services
        .traffic_start
        .send(RadioHilConnectedTrafficConfig {
            association_phy,
            data_tx_rate: benchmark_tx_rate,
        })
        .await;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-task-topology \
         network=core0 rx_protocol=core1 radio=sta-parent-core0 \
         report=core0 benchmark=core0 network_started={}",
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
    let runner_exit = match observe_open_radio_task_polls(
        run_esp32s31_connected_station_epoch(&mut radio_runner, station_control),
        connected_tasks.radio_polls(),
        connected_tasks.telemetry_enabled(),
    )
    .await
    {
        Esp32s31ConnectedStationExit::Disconnected => {
            let control = radio_runner.services().inner().control();
            let beacon_monitor = control.beacon_monitor();
            let beacon_lost = control.beacon_lost();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner \
                 exit=disconnected beacon_lost={} beacons_observed={} \
                 beacon_deadline_us={:?} last_control_event={:?} last_tx_failure={:?}",
                u8::from(beacon_lost),
                beacon_monitor.map_or(0, |monitor| monitor.observed()),
                beacon_monitor.and_then(|monitor| monitor.deadline_micros()),
                control.last_event(),
                control.last_tx_failure(),
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
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error={error:?}"
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
    };
    // Close hardware publication before stopping the protocol consumer. The
    // radio runner no longer schedules RX/control; masking both CPU and
    // peripheral routes now makes the command/frame drain finite and prevents
    // a stale wake from leaking into the next connected epoch.
    let interrupt_drain = interrupt_epoch
        .quiesce(platform)
        .unwrap_or_else(|error| panic!("MAC interrupt epoch quiescence failed: {error:?}"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-interrupt-stop \
         rx_wake={} rx_capacity_wake={} tx_events={:#010x} power_events={:#010x}",
        u8::from(interrupt_drain.mac.rx),
        u8::from(interrupt_drain.mac.rx_capacity),
        interrupt_drain.mac.tx_events,
        interrupt_drain.power_events,
    ));
    // No spawned task may retain a PAC borrow when this epoch returns. The
    // benchmark is the only task besides the radio runner that receives the
    // register cell; stop it before waiting for protocol ownership release.
    let mut connected_task_group = RadioHilConnectedTaskGroup::new(connected_tasks);
    let stopped_protocol = match stop_esp32s31_connected_task_group(
        &mut connected_task_group,
        epoch_services.task_stop_timeout,
    )
    .await
    {
        Esp32s31ConnectedTaskStopOutcome::Stopped(stopped) => stopped,
        Esp32s31ConnectedTaskStopOutcome::ResetRequired { timeout } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-connected-task-stop error=timeout \
                 timeout_ms={} reset_required=1",
                timeout.as_millis(),
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-benchmark-stopped"
    ));
    let protocol_shutdown = stopped_protocol.shutdown();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stopped \
         queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
        protocol_shutdown.queued_frames,
        protocol_shutdown.retained_frames,
        protocol_shutdown.reorder_commands,
        protocol_shutdown.active_reorders,
    ));
    let (frame, ethernet) = stopped_protocol.into_scratch();
    let (network, services) = radio_runner.into_parts();
    let services = services.into_inner();
    let teardown = match Esp32s31ConnectedStaTeardownPort::try_teardown(services, group_slot) {
        Ok(teardown) => teardown,
        Err(Esp32s31ConnectedStaTeardownFailure::Control {
            error,
            services,
            group_key,
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-control-stop error={error:?}"
            ));
            let _owners = (network, services, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedStaTeardownFailure::Rx {
            error,
            hardware,
            rx,
            tx,
            control,
            group_key,
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-dma-stop error={error:?}"
            ));
            let _owners = (network, hardware, rx, tx, control, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedStaTeardownFailure::TxActive {
            hardware,
            stopped_rx,
            tx,
            control,
            group_key,
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
            let _owners = (network, hardware, stopped_rx, tx, control, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
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
    *sequences = teardown.sequences;
    tx_storage
        .restore_resources(teardown.tx_resources)
        .unwrap_or_else(|_| panic!("connected TX return found a live owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-connected-tx-return"
    ));
    let disconnected = RadioHilDisconnectedEpoch::new(
        RadioHilRunningNetwork {
            stack,
            runner: network,
        },
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
        fixture: RadioHilConnectedTaskFixture {
            spawner,
            protocol_spawner,
            state,
            platform,
            interrupt_epoch,
            rx_storage,
            tx_storage,
            descriptor_base,
            buffer_addresses,
            scan_table,
            frame,
            ethernet,
            connected_tasks,
            connected_rx,
            network_report,
            connected_epoch,
        },
        disconnected,
        security: StaAssociationSecurity {
            pmk,
            supplicant_nonce,
            sequences,
        },
        exit: runner_exit,
    }
}
