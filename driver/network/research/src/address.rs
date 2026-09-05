use open_esp_radio_wifi_datapath::RadioEgressKey;

/// Ethernet MAC address used by the research engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub const BROADCAST: Self = Self([0xff; 6]);

    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 6] {
        self.0
    }

    pub fn is_broadcast(self) -> bool {
        self.0 == Self::BROADCAST.0
    }
}

/// IPv4 address in network byte order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Address([u8; 4]);

impl Ipv4Address {
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0
    }
}

/// UDP endpoint used by synchronous application callbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpEndpoint {
    pub address: Ipv4Address,
    pub port: u16,
}

/// Route already resolved by network policy and classified by the radio.
///
/// Keeping link resolution outside a scarce TX token is intentional. The
/// destination MAC is packet data; the opaque radio key owns current VIF/peer
/// generation and TID scheduling identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedIpv4Route {
    pub destination_mac: MacAddress,
    pub destination_ip: Ipv4Address,
    pub radio: RadioEgressKey,
}
