//! Immutable address and buffer profile reported during Host bootstrap.

use super::*;

/// Invalid immutable parameters for the initial Host/Controller profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapConfigError {
    /// Trouble cannot transmit when the Controller reports a zero ACL size.
    ZeroAclDataPacketLength,
    /// Trouble cannot acquire link credits when the Controller reports zero.
    ZeroAclDataPacketCount,
}

/// Public Bluetooth device identity in canonical display order.
///
/// The first byte is the octet printed first in an EUI-48 address. HCI carries
/// `BD_ADDR` least-significant octet first, so keeping canonical bytes in a
/// separate type prevents a platform eFuse reader from leaking byte-order
/// policy into every Controller bootstrap caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPublicDeviceAddress([u8; 6]);

impl BluetoothPublicDeviceAddress {
    /// Construct an address from canonical EUI-48 bytes.
    pub const fn from_canonical_bytes(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    /// Return the canonical EUI-48 byte sequence unchanged.
    pub const fn canonical_bytes(self) -> [u8; 6] {
        self.0
    }

    pub(crate) const fn hci_wire_bytes(self) -> [u8; 6] {
        let [byte_0, byte_1, byte_2, byte_3, byte_4, byte_5] = self.0;
        [byte_5, byte_4, byte_3, byte_2, byte_1, byte_0]
    }

    pub(super) fn hci_wire_address(self) -> BdAddr {
        BdAddr::new(self.hci_wire_bytes())
    }
}

/// Immutable, non-radio values reported during HCI Host bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerBootstrapConfig {
    pub(super) public_address: BluetoothPublicDeviceAddress,
    pub(super) le_acl_data_packet_length: u16,
    pub(super) total_num_le_acl_data_packets: u8,
}

impl LeControllerBootstrapConfig {
    /// Create a bounded initial LE Host profile.
    ///
    /// A zero public address is allowed for deployments which provide a random
    /// address. This value is a software HCI report and is not proof that an
    /// ESP32-S31 address or packet buffer has reached hardware.
    pub const fn new(
        public_address: BluetoothPublicDeviceAddress,
        le_acl_data_packet_length: u16,
        total_num_le_acl_data_packets: u8,
    ) -> Result<Self, BootstrapConfigError> {
        if le_acl_data_packet_length == 0 {
            return Err(BootstrapConfigError::ZeroAclDataPacketLength);
        }
        if total_num_le_acl_data_packets == 0 {
            return Err(BootstrapConfigError::ZeroAclDataPacketCount);
        }
        Ok(Self {
            public_address,
            le_acl_data_packet_length,
            total_num_le_acl_data_packets,
        })
    }

    /// Public address in canonical EUI-48 display order.
    pub const fn public_address(&self) -> BluetoothPublicDeviceAddress {
        self.public_address
    }

    /// Maximum Host-to-Controller LE ACL data payload reported to the Host.
    pub const fn le_acl_data_packet_length(&self) -> u16 {
        self.le_acl_data_packet_length
    }

    /// Initial number of Host-to-Controller LE ACL credits.
    pub const fn total_num_le_acl_data_packets(&self) -> u8 {
        self.total_num_le_acl_data_packets
    }

    /// Number of implemented filter accept list entries.
    ///
    /// The initial profile has no list owner and therefore reports zero.
    pub const fn filter_accept_list_size(&self) -> u8 {
        0
    }
}
