#![forbid(unsafe_code)]

mod attempts;
mod connected_epoch;
mod connected_rx_observer;
mod initial_scan;
mod lifecycle;
mod network_reporting;
mod owners;
mod reporting;
mod running_scan;
mod scenario;
mod stopped_role_transition;

pub(in crate::radio_hil) use connected_epoch::{
    RadioHilConnectedEpochBindings, RadioHilConnectedEpochPolicy, RadioHilConnectedEpochServices,
    RadioHilConnectedEpochStorage, RadioHilConnectedStaticResourceError,
    RadioHilConnectedTaskBindings, RadioHilConnectedTrafficTaskEndpoint,
    RadioHilInitialConnectedStaticResources, connected_network_stack_task,
    connected_rx_protocol_task, run_connected_network,
};
pub(in crate::radio_hil) use connected_rx_observer::{
    HilConnectedRxObserver, RadioHilConnectedRxBindings,
};
pub(in crate::radio_hil) use initial_scan::{
    RadioHilInitialScanResources, prepare_initial_station_scan, run_initial_station_scan_attempt,
};
pub(in crate::radio_hil) use lifecycle::{
    RadioHilStaLifecycleFailure, RadioHilStationEngine, RadioHilStationEngineObserver,
    RadioHilStationEnginePort, protocol_station_failure_reason, protocol_station_failure_stage,
    radio_hil_station_discovery,
};
pub(in crate::radio_hil) use network_reporting::{
    RadioHilNetworkReportBindings, connected_network_report_task,
};
pub(in crate::radio_hil) use owners::{
    RadioHilConnectedEpochResources, RadioHilConnectedEpochReturn, RadioHilConnectedExit,
    RadioHilConnectedServiceResources, RadioHilConnectedTaskFixture, RadioHilRunningNetwork,
    RadioHilStaLifecycleOwner, RadioHilStaNetwork, RadioHilStationBoardResources,
    RadioHilStationDmaResources, RadioHilStationPhase, RadioHilStationReusableResources,
    injected_tx_source_requires_reset, try_reclaim_station_runtime, try_restart_station_runtime,
};
pub(in crate::radio_hil) use reporting::{
    RadioHilStaJoinObserver, RadioHilStationEpochCoordinator, RadioHilStationEpochProgress,
    RadioHilStationEpochProgressChannel, RadioHilStationEpochReporter, station_control_task,
    station_restart_control_task,
};
pub(in crate::radio_hil) use running_scan::{
    RadioHilRunningScanContext, RadioHilRunningScanFailure, qualify_disconnected_running_scan,
};
pub(in crate::radio_hil) use scenario::run_full_station_hil;
pub(in crate::radio_hil) use stopped_role_transition::TransitionMonitorRxStorage;
pub(in crate::radio_hil) use stopped_role_transition::qualify_station_monitor_station_owner_round_trip;
