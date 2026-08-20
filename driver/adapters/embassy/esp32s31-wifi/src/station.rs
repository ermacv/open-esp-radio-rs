//! Application-facing ESP32-S31 station lifecycle facade.
//!
//! The public facade preserves stable application paths while the implementation
//! separates command publication, one connected epoch and the outer reconnect
//! lifecycle.

mod backend;
mod command;
mod composer;
mod connected_assembly;
mod connected_epoch;
mod connected_preparation;
mod connected_shutdown;
mod connected_start;
mod connected_transaction;
#[cfg(target_arch = "riscv32")]
mod join;
mod lifecycle;
#[cfg(target_arch = "riscv32")]
mod reclaim;
mod resources;
#[cfg(target_arch = "riscv32")]
mod scan;
mod tx_service;

pub use backend::Esp32s31StationAttemptRunner;

pub use crate::connected_control::{
    ConnectedControlShutdown, ConnectedWpa2Security, ConnectedWpa2SecurityEvidence,
    ConnectedWpa2SecurityFailure,
};
pub use command::{
    Esp32s31StationCommand, Esp32s31StationCommandReceiver, Esp32s31StationCompletion,
    Esp32s31StationControlError, Esp32s31StationControlResources, Esp32s31StationController,
};
pub use composer::{
    Esp32s31StationConnectedPhase, Esp32s31StationEngine, Esp32s31StationEngineObserver,
    Esp32s31StationEngineOwner, Esp32s31StationEnginePort, Esp32s31StationInitialJoinPhase,
    Esp32s31StationInitialScanExit, Esp32s31StationInitialScanPhase, Esp32s31StationJoinExit,
    Esp32s31StationReconnectedPhase, Esp32s31StationRunningScanCompletion,
    Esp32s31StationRunningScanExit, Esp32s31StationRunningScanPhase, Esp32s31StationServiceOwner,
    Esp32s31StationServicePhase, Esp32s31StationServicePhaseKind,
    NoopEsp32s31StationEngineObserver, complete_esp32s31_station_running_scan,
};
pub use connected_assembly::{
    Esp32s31ConnectedDriverAssembly, Esp32s31ConnectedDriverAssemblyFailure,
    Esp32s31ConnectedDriverAssemblyResources, assemble_esp32s31_connected_driver,
};
pub use connected_epoch::{
    Esp32s31ConnectedEpochResources, Esp32s31ConnectedServiceParts,
    Esp32s31ConnectedServiceResources, Esp32s31ConnectedStationExit,
    Esp32s31StationReconnectSource, activate_esp32s31_connected_epoch,
    run_esp32s31_connected_station_epoch,
};
pub use connected_preparation::{
    Esp32s31ConnectedNetworkStarted, Esp32s31ConnectedNetworkStartedParts,
    Esp32s31ConnectedServicePrepareFailure, Esp32s31PreparedConnectedService,
    Esp32s31PreparedConnectedServiceParts, prepare_esp32s31_connected_service,
};
pub use connected_shutdown::{
    Esp32s31ConnectedEpochQuiesceFailure, Esp32s31ConnectedEpochQuiesced,
    Esp32s31ConnectedEpochRunnerOwner, Esp32s31ConnectedEpochTeardown,
    Esp32s31ConnectedEpochTeardownFailure, quiesce_esp32s31_connected_epoch,
};
pub use connected_start::{
    Esp32s31ConnectedEpochStartFailure, Esp32s31ConnectedEpochStartPhase,
    Esp32s31ConnectedEpochStarted, Esp32s31ConnectedRxMaterializer,
    Esp32s31InitialConnectedEpochResources, start_esp32s31_initial_connected_epoch,
    start_esp32s31_reconnected_connected_epoch,
};
pub use connected_transaction::{
    Esp32s31ConnectedEpochCompleted, Esp32s31ConnectedEpochStopped, Esp32s31ConnectedRunObserver,
    Esp32s31ConnectedRunQuiesceFailure, Esp32s31ConnectedServiceTeardownFailure,
    Esp32s31ConnectedStationRunner, NoopEsp32s31ConnectedRunObserver,
    run_and_quiesce_esp32s31_connected_epoch,
};
/// Exact connected-driver teardown failure for the Embassy control service.
///
/// This alias keeps the concrete control scheduler private while preserving
/// every owner at the station fault boundary.
pub type Esp32s31ConnectedDriverTeardownFailure<
    'resources,
    M,
    H,
    R,
    S,
    X,
    const CONTROL_CAPACITY: usize,
    RE,
> = crate::connected_sta_teardown::Esp32s31ConnectedStaTeardownFailure<
    H,
    R,
    S,
    X,
    crate::connected_control::Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    crate::connected_control::ConnectedControlError,
    RE,
>;
/// Concrete service aggregate used by the production WDEV runner while
/// the Embassy control scheduler remains an implementation detail.
pub type Esp32s31ConnectedDriverServices<'resources, M, H, R, X, const CONTROL_CAPACITY: usize> =
    crate::wdev::services::WdevServiceSet<
        H,
        R,
        X,
        crate::connected_control::Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    >;
#[cfg(target_arch = "riscv32")]
pub use join::{
    Esp32s31StationJoinError, Esp32s31StationJoinOutcome, Esp32s31StationJoinResources,
    Esp32s31StationJoinReturned, run_esp32s31_station_join,
};
pub use lifecycle::{
    Esp32s31StationConfig, Esp32s31StationExit, Esp32s31StationPrepareFailure,
    Esp32s31StationReturnedResources, Esp32s31StationStartResources, Esp32s31StationStopReason,
    Esp32s31StationTask, prepare_esp32s31_station_task,
};
#[cfg(target_arch = "riscv32")]
pub use open_esp_radio_esp32s31_hal::radio_arena::Esp32s31RadioOwnerRepublish;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRoleOwner;
#[cfg(target_arch = "riscv32")]
pub use reclaim::{
    Esp32s31StationInterruptEpochState, Esp32s31StationPhaseRebindFailure,
    Esp32s31StationPhaseReclaimError, Esp32s31StationPhaseReclaimFailure,
    Esp32s31StationPhaseReclaimed, Esp32s31StationPhaseRestoreFailure,
    Esp32s31StationRuntimeReclaimFailure, Esp32s31StationRuntimeReclaimed,
    Esp32s31StationStoppedPhaseResources, try_rebind_esp32s31_station_phase,
    try_reclaim_esp32s31_station_phase, try_reclaim_esp32s31_station_runtime,
    try_restore_esp32s31_station_phase,
};
pub use resources::{
    Esp32s31StationDmaResources, Esp32s31StationRadioOwner, Esp32s31StationRadioResources,
    Esp32s31StationRuntimeParts, Esp32s31StationRuntimeResources, Esp32s31StationStorageResources,
};

#[cfg(target_arch = "riscv32")]
impl<P> Esp32s31StationRadioOwner for Esp32s31WifiRoleOwner<P> {
    type Platform = P;

    fn radio_mut(
        &mut self,
    ) -> (
        &mut open_esp_radio_esp32s31_phy::PhyState,
        &mut Self::Platform,
    ) {
        Esp32s31WifiRoleOwner::radio_mut(self)
    }
}
#[cfg(target_arch = "riscv32")]
pub use scan::{
    ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY, ESP32S31_STATION_PROBE_RATES,
    Esp32s31StationInitialScanFailures, Esp32s31StationInitialScanReturned,
    Esp32s31StationScanDecision, Esp32s31StationScanOutcome, Esp32s31StationScanPlan,
    Esp32s31StationScanRequest, Esp32s31StationScanResources, Esp32s31StationScanReturned,
    complete_esp32s31_station_initial_scan, esp32s31_station_scan_failure_disposition,
    run_esp32s31_station_scan,
};

#[cfg(test)]
mod tests;
