//! Versioned Linux Bluetooth helper contract shared by preparation and execution.
//!
//! Feature expectations are independent of the production implementation. Tests
//! exercise the production LL response against this profile; matching the mask
//! alone does not establish encrypted interoperability.

pub const CONNECTION_RESET_SCHEMA: u32 = 10;
pub const HELPER_CAPABILITIES: &str =
    "schema=10 termination=peer-reset,peer-rfkill,target-disconnect,target-reset";

/// Encryption, Peripheral Feature Exchange, Ping and CSA #2.
/// The peer must support the complete negotiated profile.
pub const EXPECTED_REMOTE_FEATURES: [u8; 8] = [0x19, 0x40, 0, 0, 0, 0, 0, 0];
