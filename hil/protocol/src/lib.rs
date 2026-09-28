#![no_std]
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter and the qualification manifest that selects it.

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
mod bluetooth_gatt;
#[cfg(feature = "bluetooth")]
mod bluetooth_secure_gatt;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{
    BLUETOOTH_HCI_EVENT_BYTES, BLUETOOTH_HCI_PARAMETER_BYTES,
    BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS, BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES,
    BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS, BLUETOOTH_REFRESH_EDIV, BLUETOOTH_REFRESH_LTK,
    BLUETOOTH_REFRESH_RAND, BLUETOOTH_TEST_EDIV, BLUETOOTH_TEST_LTK, BLUETOOTH_TEST_RAND,
    BluetoothDtmEvidence, BluetoothDtmOperation, BluetoothDtmResult, BluetoothDtmRxDiagnostics,
    BluetoothHciRequest, BluetoothHciResponse, BluetoothPeripheralTermination,
    BluetoothSecurityFailure, bluetooth_peripheral_acl_payload,
    bluetooth_peripheral_acl_payload_for_sequence,
};
#[cfg(feature = "bluetooth")]
pub use bluetooth_gatt::BluetoothGattEvidence;
#[cfg(feature = "bluetooth")]
pub use bluetooth_secure_gatt::{
    BluetoothAdvertisingStartRejection, BluetoothGattApplicationFailure, BluetoothGattResetOutcome,
    BluetoothGattResetReadGate, BluetoothGattShutdown, BluetoothGattStopCause,
    BluetoothNumericChallenge, BluetoothNumericDecision, BluetoothSecureGattEvidence,
};

#[cfg(feature = "wifi")]
mod stream_pattern;
#[cfg(feature = "wifi")]
mod udp_probe;
#[cfg(feature = "wifi")]
mod wifi_airtime;
#[cfg(feature = "wifi")]
mod wifi_rx;
#[cfg(feature = "wifi")]
mod wifi_tx;
#[cfg(feature = "wifi")]
pub use stream_pattern::{fill_stream_pattern, stream_pattern_byte, stream_pattern_matches};
#[cfg(feature = "wifi")]
pub use udp_probe::UdpProbe;
#[cfg(feature = "wifi")]
pub use wifi_airtime::{WifiAirtimePeer, WifiAirtimePeerEvidence, WifiAirtimeReport};
#[cfg(feature = "wifi")]
pub use wifi_rx::{WifiRxRejection, WifiRxRejectionReason};
#[cfg(feature = "wifi")]
pub use wifi_tx::WifiTxRetentionEvidence;

#[cfg(feature = "system")]
mod memory_benchmark;
#[cfg(feature = "system")]
pub use memory_benchmark::{
    MemoryBenchmarkEvidence, MemoryBenchmarkMode, MemoryBenchmarkRequest, MemoryBenchmarkSource,
    MemoryBenchmarkStop,
};

// The shared core: framing, the envelope and its command and event sets,
// capabilities, boot and post-mortem evidence, the trace, PHY diagnostics and
// startup artifacts.
mod absent;
pub use absent::Absent;
mod framing;
#[cfg(feature = "async-io")]
mod io;
mod message;
mod phy_fault;
mod phy_register_image;
mod system;
mod trace;
#[allow(unused_imports, reason = "empty while every family is on")]
pub use absent::stand_ins::*;
pub use framing::{
    DecodeCounters, DecodeError, EncodeError, FrameDecoder, FrameEncoder, MAX_POSTCARD_BYTES,
    MAX_WIRE_FRAME_BYTES, RequestIdentity, evidence_crc32c, ieee802154_frame_crc32c,
    startup_artifact_crc32c,
};
#[cfg(feature = "async-io")]
pub use io::write_frame;
pub use message::*;
pub use phy_fault::{
    PhyFaultCommand, PhyFaultEvidence, PhyFaultMode, PhyFaultPhase, PhyTrackingCommand,
    PhyTrackingEvidence,
};
pub use phy_register_image::{
    PHY_REGISTER_IMAGE_WORDS, PhyAnalogImageBytes, PhyRegisterImageRequest, PhyRegisterImageWords,
};
pub use system::{
    BootEvidence, CHECKPOINT_NAME_BYTES, Checkpoint, Fault, HangFault, HangTarget, HartState,
    POST_MORTEM_CHECKPOINT_PAGE, POST_MORTEM_CHECKPOINTS, PanicFault, PostMortemCheckpoints,
    PostMortemSummary, ResetReason, TaskSlot, TaskStall, WatchdogTestMode,
};
pub use trace::{
    TRACE_ENTRY_PAGE, TRACE_SNAPSHOT_PAGE, TraceControl, TraceEntries, TraceEntry,
    TraceSnapshotPage, TraceStatus,
};
