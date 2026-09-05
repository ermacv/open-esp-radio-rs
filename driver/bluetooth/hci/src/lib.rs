#![no_std]
#![forbid(unsafe_code)]

//! Executor-neutral transport and storage at the Bluetooth Host Controller
//! Interface.
//!
//! [`LeControllerHciResources`] exposes a Host transport accepted by
//! `bt_hci::ExternalController` and one affine combined Controller endpoint.
//! Its crate-private channel carries HCI packet bodies with a separate typed
//! packet kind, so no UART/H4 framing exists inside the process. Both
//! directions have statically bounded storage, wake-driven backpressure and
//! cancellation-safe waits.
//! [`LeControllerBootstrap`] implements a closed software-only HCI command
//! subset for Host initialization; Link-Layer commands remain owned by an
//! outer router.
//! The separate closed LE DTM codec normalizes Receiver/Transmitter Test v1 and
//! v2 plus Test End into owned semantic commands. Its reviewed idle/active
//! session policy retains start/Test End ownership for a hardware runner and
//! builds only the exact no-test success or active-start busy responses; it
//! does not dispatch commands or claim radio work.
//! The legacy advertising codec separately decodes the standard Set
//! Parameters, Set Data, Set Scan Response Data and Set Enable commands into
//! owned semantic values for distinct nonconnectable and connectable roles.
//! Configuration commands update one reset-scoped owner under exact response
//! order. Set Enable is refined into a role-specific deferred start, preventing
//! response-capable input from entering a nonconnectable chip runner.
//! Legacy passive scanning follows the same boundary: standard Set Scan
//! Parameters and Set Scan Enable commands become owned timing and duplicate
//! policy, while affine start/disable continuations delay success until a chip
//! runner proves hardware `RUN` or quiescence.
//! [`classify_le_controller_command`] joins these portable policies at a finite
//! command boundary: valid bootstrap, DTM and Link Layer configuration commands
//! become owned semantic tokens, malformed known commands become owned error
//! responses, and every other opcode becomes an owned Unknown Command
//! completion.
//! Classification never advances bootstrap state, leaves no result borrowing
//! receive scratch storage, and keeps Reset plus other bootstrap commands
//! available to session-aware policy before explicit dispatch.
//! [`LeControllerHciResources`] binds transport storage and bootstrap state to
//! one affine Controller epoch. Its sole split exposes a Host transport and one
//! combined Controller command endpoint: the raw Controller transport and
//! mutable bootstrap state cannot be separated or mutated through public
//! accessors. Command intake consumes the sole command-ready token and returns
//! an opaque classification/order aggregate accepted only by idle or active
//! session routing. Readiness waits borrow that token and reserve nothing, so
//! cancellation cannot lose authority or consume a packet. A hardware session
//! runner can retain an accepted command across asynchronous radio transitions
//! and output backpressure without a synchronous-dispatch compatibility layer.
//! Resource construction rejects
//! profiles whose advertised ACL capacity exceeds that storage. This crate
//! contains no Link Layer, radio, MMIO, interrupt, executor, allocator, or
//! readiness substitute.

#[cfg(test)]
extern crate std;

mod bootstrap;
mod command;
mod controller;
mod transport;

pub use bootstrap::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BluetoothPublicDeviceAddress, BootstrapCommand,
    BootstrapCommandCompleteEvent, BootstrapConfigError, BootstrapHostBuffers, BootstrapPhase,
    LeControllerBootstrap, LeControllerBootstrapConfig, OwnedBootstrapCommand,
};
pub use bt_hci;
pub(crate) use command::advertising::LeLegacyAdvertisingIdleEnableDisposition;
pub use command::advertising::{
    LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY, LE_LEGACY_ADVERTISING_DATA_CAPACITY,
    LeLegacyAdvertisingAddress, LeLegacyAdvertisingCommand,
    LeLegacyAdvertisingCommandCompleteEvent, LeLegacyAdvertisingCommandKind,
    LeLegacyAdvertisingConfigurationCommand, LeLegacyAdvertisingData,
    LeLegacyAdvertisingDecodeError, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingIntervalRange, LeLegacyAdvertisingOwnAddressKind,
    LeLegacyAdvertisingParameters, LeLegacyAdvertisingPrimaryChannels, LeLegacyAdvertisingRole,
    LeLegacyConnectableAdvertisingEnableRequest, LeLegacyNonconnectableAdvertisingEnableRequest,
    LeLegacyScanResponseData,
};
pub use command::classification::{
    LeControllerCommandClassification, classify_le_controller_command,
};
pub use command::dtm::{
    LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY, LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE,
    LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE,
    LeDtmActiveSessionDisposition, LeDtmChannel, LeDtmCommand, LeDtmCommandCompleteEvent,
    LeDtmCommandDecodeError, LeDtmCommandKind, LeDtmIdleSessionDisposition, LeDtmModulationIndex,
    LeDtmPayloadPattern, LeDtmPhy, LeReceiverTestCommand, LeTestEndCommand,
    LeTransmitterTestCommand,
};
pub use command::order::{
    LeControllerActiveDtmCommandRoute, LeControllerActiveLegacyAdvertisingCommandRoute,
    LeControllerActiveLegacyScanningCommandRoute, LeControllerClassifiedCommand,
    LeControllerClassifiedCommandRoute, LeControllerCommandIntake, LeControllerCommandReady,
    LeControllerDeferredDtmCommand, LeControllerDeferredLegacyAdvertisingDisable,
    LeControllerDeferredLegacyConnectableAdvertisingStart,
    LeControllerDeferredLegacyNonconnectableAdvertisingStart,
    LeControllerDeferredLegacyScanningDisable, LeControllerDeferredLegacyScanningStart,
    LeControllerDeferredReceiverStart, LeControllerDeferredTestEnd,
    LeControllerDeferredTransmitterStart, LeControllerEndpointMismatch,
    LeControllerIdleClassifiedCommandRoute, LeControllerResetBarrier, LeControllerResetCompletion,
    LeControllerResponsePending, LeControllerResponsePublication,
};
pub use command::response::{
    HciControllerResponse, LeControllerCommandComplete, UnknownCommandCompleteEvent,
};
pub use command::scanning::{
    LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY,
    LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY, LeLegacyAdvertisingReportEvent,
    LeLegacyAdvertisingReportEventError, LeLegacyPassiveScanParameters, LeLegacyScanningCommand,
    LeLegacyScanningCommandCompleteEvent, LeLegacyScanningCommandKind,
    LeLegacyScanningConfigurationCommand, LeLegacyScanningDecodeError,
    LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableCommand, LeLegacyScanningEnableRequest,
};
pub use controller::{
    LeControllerCommandEndpoint, LeControllerCommandReadyClaim, LeControllerHciEndpoints,
    LeControllerHciResources, LeControllerHciResourcesError, LeLegacyAdvertisingReportPublication,
};
pub use transport::{
    HciChannelError, HciCommandPacket, HciEpochBound, HciEpochIdentity, HostToControllerFrame,
    InProcessHciHostTransport,
};
pub(crate) use transport::{
    HciClassifiedCommandIntake, InProcessHciChannel, InProcessHciControllerEndpoint,
};

pub use transport::{
    ControllerToHostQueue, ControllerToHostQueueError, INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY,
};
