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
//! cancellation-safe waits. The supervisor can permanently close both directions
//! through [`LeControllerCommandEndpoint::close_transport`]: queued packets remain
//! readable, further publications fail, and transport waiters wake. Closure
//! retains outstanding command authority and does not prove radio quiescence.
//! Graceful [`LeControllerCommandEndpoint::try_retire_transport`] instead requires
//! the next-command token and empty queues, atomically closes admission, and
//! returns [`LeControllerHciRetired`]. Rejected retirement preserves normal
//! traffic and ACL credit return so the caller can finish draining the epoch.
//!
//! Active-peripheral intake leaves ACL data queued while the radio's packet
//! owner is occupied. Commands may bypass that data, preserving FIFO order
//! within each class, so Disconnect, Reset and Host credit returns remain live.
//! Its readiness wait uses the same admission predicate and does not spin on
//! blocked ACL alone. Queue storage covers all advertised initial TX credits;
//! moving a packet into the radio owner does not complete its HCI credit.
//!
//! Graceful retirement can also authorize `restart_transport` on its exact
//! endpoint. Both empty closed queues advance atomically to the next checked
//! generation; bootstrap and initial command authority are renewed. Old Host
//! handles and pending futures reject inside the queue lock before reading,
//! publishing or registering wakers for the new generation. Physical radio
//! readiness is the outer Controller's responsibility, not a transport claim.
//!
//! [`LeControllerBootstrap`] implements a closed software-only HCI command
//! subset for Host initialization and reports the complete production command
//! inventory through the standard 64-octet Supported Commands bitmap;
//! Link-Layer commands remain owned by an outer router.
//! Standard LE Rand is a separate platform service: composition explicitly
//! attaches a borrowed [`LeRandomSource`] before bootstrap. Only a bound
//! endpoint advertises it. All role routers preserve ordinary response order;
//! output backpressure retains the sampled bytes without sampling again.
//! Entropy failure returns Hardware Failure, not successful placeholder data.
//! The portable layer neither imports a hardware RNG nor owns its power state.
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
//! Owned LE Connection Complete, LE Connection Update Complete and
//! Disconnection Complete codecs retain exact Host events across bounded
//! backpressure. Their endpoint publication enforces the standard base and LE
//! event masks; a chip runner still owns the proof that each transition occurred.
//! [`classify_le_controller_command`] joins these portable policies at a finite
//! command boundary: valid bootstrap, DTM, Disconnect, LE Read Remote Features,
//! Read Remote Version Information and Link Layer configuration
//! commands become owned semantic tokens, malformed known commands become owned
//! error responses, and every other opcode becomes an owned Unknown Command
//! completion. Disconnect, remote feature discovery and remote version discovery
//! use standard Command Status before their chip-owned Link Layer lifecycles begin.
//! Standard LTK reply/negative-reply and encryption-event codecs join the
//! production classifier and retain secrets through the live peripheral owner.
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
//! profiles whose advertised ACL capacity exceeds that storage. Active
//! peripheral intake copies one matching Host ACL packet into an owned credit
//! token, which exposes acknowledged-fragment progression and a standard Number
//! Of Completed Packets event without depending on a radio implementation.
//! Controller ACL owners preserve start/continuation boundaries, fragment to
//! the Host buffer profile, and decode the response-less Host Number Of
//! Completed Packets credit command. [`LeHostAclCreditSender`] gives an
//! `ExternalController` integration the corresponding restricted write-only
//! authority because that standard command has no successful response. This
//! crate contains no Link Layer, radio, MMIO, interrupt, executor, allocator,
//! or readiness substitute.

#[cfg(test)]
extern crate std;

mod controller;

pub use controller::random::{
    LeRandCommand, LeRandCommandCompleteEvent, LeRandomSource, LeRandomSourceAlreadyConfigured,
    LeRandomUnavailable,
};
mod transport;
mod wire;

pub use bt_hci;
pub use controller::bootstrap::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BluetoothPublicDeviceAddress, BootstrapCommand,
    BootstrapCommandCompleteEvent, BootstrapConfigError, BootstrapHostBuffers, BootstrapPhase,
    LeControllerBootstrap, LeControllerBootstrapConfig, OwnedBootstrapCommand,
    le_controller_supported_commands,
};
pub use controller::classification::{
    LeControllerCommandClassification, classify_le_controller_command,
};
pub use controller::le::acl::{
    LE_ACL_DATA_PACKET_CAPACITY, LE_CONTROLLER_ACL_PACKET_CAPACITY,
    LE_HOST_COMPLETED_PACKETS_ERROR_EVENT_CAPACITY, LE_NUMBER_OF_COMPLETED_PACKETS_EVENT_CAPACITY,
    LeControllerAclPacket, LeControllerToHostAclProfile, LeHostAclFragment, LeHostAclPacket,
    LeHostAclPacketRejection, LeHostCompletedPacketsCommand, LeHostCompletedPacketsErrorEvent,
    LeNumberOfCompletedPacketsEvent,
};
pub(crate) use controller::le::advertising::LeLegacyAdvertisingIdleEnableDisposition;
pub use controller::le::advertising::{
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
pub use controller::le::dtm::{
    LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY, LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE,
    LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE,
    LeDtmActiveSessionDisposition, LeDtmChannel, LeDtmCommand, LeDtmCommandCompleteEvent,
    LeDtmCommandDecodeError, LeDtmCommandKind, LeDtmIdleSessionDisposition, LeDtmModulationIndex,
    LeDtmPayloadPattern, LeDtmPhy, LeReceiverTestCommand, LeTestEndCommand,
    LeTransmitterTestCommand,
};
pub use controller::le::peripheral::{
    LE_CONNECTION_UPDATE_COMPLETE_EVENT_CAPACITY, LE_DISCONNECT_COMMAND_STATUS_EVENT_CAPACITY,
    LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY, LE_ENCRYPTION_CHANGE_EVENT_CAPACITY,
    LE_ENCRYPTION_KEY_REFRESH_COMPLETE_EVENT_CAPACITY,
    LE_LONG_TERM_KEY_COMMAND_COMPLETE_EVENT_CAPACITY, LE_LONG_TERM_KEY_REQUEST_EVENT_CAPACITY,
    LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY,
    LE_READ_REMOTE_FEATURES_COMMAND_STATUS_EVENT_CAPACITY,
    LE_READ_REMOTE_FEATURES_COMPLETE_EVENT_CAPACITY,
    LE_READ_REMOTE_VERSION_INFORMATION_COMMAND_STATUS_EVENT_CAPACITY,
    LE_READ_REMOTE_VERSION_INFORMATION_COMPLETE_EVENT_CAPACITY, LeConnectionUpdateCompleteEvent,
    LeDisconnectCommand, LeDisconnectCommandStatusEvent, LeDisconnectionCompleteEvent,
    LeEncryptionChangeEvent, LeEncryptionKeyRefreshCompleteEvent,
    LeLongTermKeyCommandCompleteEvent, LeLongTermKeyCommandDecodeError, LeLongTermKeyRequestEvent,
    LeLongTermKeyRequestNegativeReplyCommand, LeLongTermKeyRequestReplyCommand,
    LePeripheralConnectionCompleteEvent, LePeripheralConnectionCompleteEventError,
    LeReadRemoteFeaturesCommand, LeReadRemoteFeaturesCommandStatusEvent,
    LeReadRemoteFeaturesCompleteEvent, LeReadRemoteVersionInformationCommand,
    LeReadRemoteVersionInformationCommandStatusEvent, LeReadRemoteVersionInformationCompleteEvent,
};
pub use controller::le::scanning::{
    LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY,
    LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY, LeLegacyAdvertisingReportEvent,
    LeLegacyAdvertisingReportEventError, LeLegacyPassiveScanParameters, LeLegacyScanningCommand,
    LeLegacyScanningCommandCompleteEvent, LeLegacyScanningCommandKind,
    LeLegacyScanningConfigurationCommand, LeLegacyScanningDecodeError,
    LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableCommand, LeLegacyScanningEnableRequest,
};
pub use controller::order::{
    LeControllerAcceptedDisconnect, LeControllerAcceptedLongTermKeyReply,
    LeControllerActiveDtmCommandRoute, LeControllerActiveLegacyAdvertisingCommandRoute,
    LeControllerActiveLegacyScanningCommandRoute, LeControllerActivePeripheralCommandRoute,
    LeControllerActivePeripheralIntake, LeControllerClassifiedCommand,
    LeControllerClassifiedCommandRoute, LeControllerCommandIntake, LeControllerCommandReady,
    LeControllerDeferredDisconnect, LeControllerDeferredDtmCommand,
    LeControllerDeferredLegacyAdvertisingDisable,
    LeControllerDeferredLegacyConnectableAdvertisingStart,
    LeControllerDeferredLegacyNonconnectableAdvertisingStart,
    LeControllerDeferredLegacyScanningDisable, LeControllerDeferredLegacyScanningStart,
    LeControllerDeferredLongTermKeyNegativeReply, LeControllerDeferredLongTermKeyReply,
    LeControllerDeferredReadRemoteFeatures, LeControllerDeferredReadRemoteVersionInformation,
    LeControllerDeferredReceiverStart, LeControllerDeferredTestEnd,
    LeControllerDeferredTransmitterStart, LeControllerEndpointMismatch,
    LeControllerIdleClassifiedCommandRoute, LeControllerResetBarrier, LeControllerResetCompletion,
    LeControllerResponsePending, LeControllerResponsePublication,
};
pub use controller::response::{
    HciControllerResponse, LeControllerCommandComplete, UnknownCommandCompleteEvent,
};
pub use controller::{
    LeControllerCommandEndpoint, LeControllerCommandReadyClaim, LeControllerHciEndpoints,
    LeControllerHciResources, LeControllerHciResourcesError, LeControllerHciRestartError,
    LeControllerHciRetired, LeControllerHciRetirementError, LeLegacyAdvertisingReportPublication,
    LePeripheralConnectionEventPublication,
};
pub(crate) use transport::{
    HciActivePeripheralIntake, HciClassifiedCommandIntake, InProcessHciChannel,
    InProcessHciControllerEndpoint,
};
pub use transport::{
    HciChannelError, HciEpochBound, HciEpochIdentity, InProcessHciHostTransport,
    LeHostAclCreditSender,
};

pub use transport::{
    ControllerToHostQueue, ControllerToHostQueueError, INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY,
};

pub use wire::{HciCommandPacket, HostToControllerFrame};
