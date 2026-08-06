#![forbid(unsafe_code)]

mod connected_epoch;
mod connected_rx_observer;
mod lifecycle;
mod network_reporting;
mod owners;
mod reporting;
mod running_scan;

pub(in crate::radio_hil) use connected_epoch::{
    RadioHilConnectedTaskBindings, RadioHilConnectedTaskGroup, connected_network_stack_task,
    connected_rx_protocol_task, run_connected_network,
};
pub(in crate::radio_hil) use connected_rx_observer::{
    HilConnectedRxObserver, RadioHilConnectedRxBindings,
};
pub(in crate::radio_hil) use lifecycle::{
    RadioHilStaLifecycleBackend, RadioHilStaLifecycleFailure, protocol_station_failure_reason,
    protocol_station_failure_stage,
};
pub(in crate::radio_hil) use network_reporting::{
    RadioHilNetworkReportBindings, connected_network_report_task,
};
pub(in crate::radio_hil) use owners::{
    RadioHilAuthenticationReady, RadioHilConnectedEpochResources, RadioHilConnectedEpochReturn,
    RadioHilConnectedExit, RadioHilConnectedFixture, RadioHilConnectedTaskFixture,
    RadioHilReconnectReady, RadioHilRunningNetwork, RadioHilRunningScanReady,
    RadioHilStaLifecycleOwner, RadioHilStaNetwork, StaAssociationSecurity, StaConnectedSession,
    StaJoinTarget, injected_tx_source_requires_reset,
};
pub(in crate::radio_hil) use reporting::{
    RadioHilStaJoinObserver, RadioHilStationEpochCoordinator, RadioHilStationEpochProgress,
    RadioHilStationEpochProgressChannel, RadioHilStationEpochReporter, station_control_task,
};
pub(in crate::radio_hil) use running_scan::{
    RadioHilRunningScanContext, RadioHilRunningScanFailure, qualify_disconnected_running_scan,
};
