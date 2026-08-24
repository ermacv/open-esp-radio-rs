//! Sole integration SPI for one connected station epoch.
//!
//! The submodules are implementation owners, not alternate public facades.
//! Integration code composes the connected role through the types and finite
//! transitions re-exported here.

mod assembly;
mod epoch;
pub(crate) mod port;
mod preparation;
mod shutdown;
mod start;
mod transaction;

pub use assembly::{
    Esp32s31ConnectedDriverAssembly, Esp32s31ConnectedDriverAssemblyFailure,
    Esp32s31ConnectedDriverAssemblyResources, assemble_esp32s31_connected_driver,
};
pub use epoch::{
    Esp32s31ConnectedEpochResources, Esp32s31ConnectedServiceParts,
    Esp32s31ConnectedServiceResources, Esp32s31ConnectedStationExit,
    Esp32s31StationReconnectSource, activate_esp32s31_connected_epoch,
    run_esp32s31_connected_station_epoch,
};
#[cfg(test)]
pub(in crate::roles::station) use epoch::{
    coalesce_disconnected_station_command, complete_connected_station_command,
};
pub use port::{
    Esp32s31ConnectedStaBlockAckPolicy, Esp32s31ConnectedStaCompositionFailure,
    Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaConfigError,
    Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaDriverParts,
    Esp32s31ConnectedStaDrivers, Esp32s31ConnectedStaEspNowRxError,
    Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPort,
    Esp32s31ConnectedStaPrepareFailure, Esp32s31ConnectedStaRateConfig, Esp32s31ConnectedStaReport,
    Esp32s31ConnectedStaRxPolicy, Esp32s31ConnectedStaRxProcessorResources,
    Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxHandoffFailure,
    Esp32s31ConnectedStaTxPolicy, Esp32s31ConnectedStaTxResources,
};
pub use preparation::{
    Esp32s31ConnectedNetworkStarted, Esp32s31ConnectedNetworkStartedParts,
    Esp32s31ConnectedServicePrepareFailure, Esp32s31PreparedConnectedService,
    Esp32s31PreparedConnectedServiceParts, prepare_esp32s31_connected_service,
};
pub use shutdown::{
    Esp32s31ConnectedEpochQuiesceFailure, Esp32s31ConnectedEpochQuiesced,
    Esp32s31ConnectedEpochRunnerOwner, Esp32s31ConnectedEpochTeardown,
    Esp32s31ConnectedEpochTeardownFailure, quiesce_esp32s31_connected_epoch,
};
pub use start::{
    Esp32s31ConnectedEpochStartFailure, Esp32s31ConnectedEpochStartPhase,
    Esp32s31ConnectedEpochStarted, Esp32s31ConnectedRxMaterializer,
    Esp32s31InitialConnectedEpochResources, start_esp32s31_initial_connected_epoch,
    start_esp32s31_reconnected_connected_epoch,
};
pub use transaction::{
    Esp32s31ConnectedEpochCompleted, Esp32s31ConnectedEpochStopped, Esp32s31ConnectedRunObserver,
    Esp32s31ConnectedRunQuiesceFailure, Esp32s31ConnectedServiceTeardownFailure,
    Esp32s31ConnectedStationRunner, NoopEsp32s31ConnectedRunObserver,
    run_and_quiesce_esp32s31_connected_epoch,
};

pub use super::command::{Esp32s31StationCommand, Esp32s31StationCommandReceiver};
pub use super::control::{ConnectedControlShutdown, ConnectedWpa2Security};
pub use super::control_mailbox::{ConnectedControlPublisher, ConnectedControlResources};
pub use super::epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch};
pub use super::esp_now_mailbox::{
    EspNowMailboxConnectedRxSink, EspNowOwnedRxEvent, EspNowRxMailboxEpochError,
    EspNowRxMailboxResources, EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher,
    EspNowRxReceiver,
};
pub use super::network::EmbassyNetConnectedRxSink;
pub use super::rx_protocol::{
    ConnectedRxProtocolSink, Esp32s31ConnectedRxProtocol, Esp32s31ConnectedRxProtocolStopped,
    Esp32s31ConnectedRxProtocolStorage,
};
pub use super::teardown::{
    Esp32s31AlreadyStoppedRx, Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownPort,
};
pub use super::tx::Esp32s31ConnectedTx;
pub use super::tx_epoch::Esp32s31StaTxEpochExt;

/// Exact connected-driver teardown failure while retaining the complete
/// concrete no-allocation owner frontier.
pub type Esp32s31ConnectedDriverTeardownFailure<
    'resources,
    M,
    H,
    R,
    S,
    X,
    const CONTROL_CAPACITY: usize,
    RE,
> = super::teardown::Esp32s31ConnectedStaTeardownFailure<
    H,
    R,
    S,
    X,
    super::control::Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    super::control::ConnectedControlError,
    RE,
>;

/// Concrete DATAPATH service graph used by production connected composition.
pub type Esp32s31ConnectedDriverServices<'resources, M, H, R, X, const CONTROL_CAPACITY: usize> =
    crate::datapath::services::SingleRoleServices<
        H,
        R,
        X,
        super::control::Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    >;
