//! Application-facing ESP32-S31 station lifecycle facade.
//!
//! The public facade preserves stable application paths while the implementation
//! separates command publication, one connected epoch and the outer reconnect
//! lifecycle.

#[cfg(target_arch = "riscv32")]
pub mod attempt;
mod backend;
mod command;
mod composer;
pub mod connected;
pub mod control;
pub mod control_mailbox;
pub mod epoch;
pub mod esp_now_mailbox;
#[cfg(target_arch = "riscv32")]
mod join;
#[cfg(any(target_arch = "riscv32", test))]
mod join_port;
#[cfg(target_arch = "riscv32")]
mod join_time;
mod lifecycle;
pub mod network;
#[cfg(target_arch = "riscv32")]
mod reclaim;
mod resources;
pub mod runtime;
pub mod rx_protocol;
#[cfg(target_arch = "riscv32")]
mod scan;
pub mod teardown;
pub mod tx;
pub mod tx_epoch;
mod tx_service;
#[cfg(target_arch = "riscv32")]
mod wpa2_port;
#[cfg(target_arch = "riscv32")]
mod wpa2_time;

pub use backend::Esp32s31StationAttemptRunner;

pub use self::control::{
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
pub use runtime::{
    Esp32s31StaApStationControlError, Esp32s31StaApStationFinishFailure,
    Esp32s31StaApStationFinishReason, Esp32s31StaApStationPrepareFailure,
    Esp32s31StaApStationPrepared, Esp32s31StaApStationTxError,
    Esp32s31StaApStationTxOwnershipError, StationRoleRuntime, finish_sta_ap_station,
    park_sta_ap_station_role, prepare_sta_ap_station,
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
