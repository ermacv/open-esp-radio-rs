//! Portable identity and topology types at the SoftMAC/backend boundary.
//!
//! A virtual interface is protocol state, while a channel context represents
//! one hardware radio tuning context. Keeping their identities distinct is
//! required for STA/AP concurrency: two VIFs may share one channel context
//! even when the hardware cannot tune two channels simultaneously.

/// Stable identity of one virtual Wi-Fi interface within a radio owner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct VifId(u8);

impl VifId {
    pub const PRIMARY: Self = Self(0);

    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Protocol role implemented by one virtual interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VifRole {
    Station,
    AccessPoint,
}

/// Value-only definition of one virtual interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualInterface {
    pub id: VifId,
    pub role: VifRole,
    pub address: [u8; 6],
}

impl VirtualInterface {
    pub const fn new(id: VifId, role: VifRole, address: [u8; 6]) -> Self {
        Self { id, role, address }
    }
}

/// Stable identity of one hardware channel context within a radio owner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ChannelContextId(u8);

impl ChannelContextId {
    pub const PRIMARY: Self = Self(0);

    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Explicit binding between protocol state and a hardware tuning context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VifChannelBinding {
    pub vif: VifId,
    pub channel_context: ChannelContextId,
}

impl VifChannelBinding {
    pub const fn new(vif: VifId, channel_context: ChannelContextId) -> Self {
        Self {
            vif,
            channel_context,
        }
    }
}

/// One VIF together with the hardware context it currently uses.
///
/// Grouping these values avoids parallel `vif_id`/`role`/`address`/`context`
/// argument lists and makes an inconsistent binding unrepresentable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundVirtualInterface {
    pub interface: VirtualInterface,
    pub channel_context: ChannelContextId,
}

impl BoundVirtualInterface {
    pub const fn new(interface: VirtualInterface, channel_context: ChannelContextId) -> Self {
        Self {
            interface,
            channel_context,
        }
    }

    pub const fn binding(self) -> VifChannelBinding {
        VifChannelBinding::new(self.interface.id, self.channel_context)
    }
}

/// Observation point for a passive monitor consumer.
///
/// Monitor is a tap rather than a protocol VIF: a slow observer must not own
/// the normal RX path or force STA/AP protocol state into a synthetic role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorTapPoint {
    /// Raw DMA frame and chip RX status before normalization.
    Raw,
    /// Frame and portable metadata after backend normalization.
    Normalized,
    /// Frame after HMAC validation, decryption and reorder.
    ProtocolValidated,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vif_and_channel_context_are_distinct_domains() {
        let station =
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [0x02, 0, 0, 0, 0, 1]);
        let access_point =
            VirtualInterface::new(VifId::new(1), VifRole::AccessPoint, [0x02, 0, 0, 0, 0, 2]);
        let station_binding = VifChannelBinding::new(station.id, ChannelContextId::PRIMARY);
        let access_point_binding =
            VifChannelBinding::new(access_point.id, ChannelContextId::PRIMARY);

        assert_ne!(station.id, access_point.id);
        assert_eq!(
            station_binding.channel_context,
            access_point_binding.channel_context
        );
        assert_eq!(
            BoundVirtualInterface::new(station, ChannelContextId::PRIMARY).binding(),
            station_binding
        );
    }

    #[test]
    fn monitor_is_an_observation_point_not_a_vif_role() {
        assert_ne!(MonitorTapPoint::Raw, MonitorTapPoint::Normalized);
        assert_ne!(
            MonitorTapPoint::Normalized,
            MonitorTapPoint::ProtocolValidated
        );
    }
}
