//! HIL network composition pieces shared by the runtime's stack variants.
pub mod progress;

#[cfg(feature = "upstream-network")]
pub mod checksum;
#[cfg(any(feature = "embassy-network", feature = "owned-network"))]
pub mod embassy_ipv4;
#[cfg(feature = "upstream-network")]
pub mod ipv4;
#[cfg(any(
    feature = "upstream-network",
    feature = "embassy-network",
    feature = "owned-network"
))]
pub mod sockets;
