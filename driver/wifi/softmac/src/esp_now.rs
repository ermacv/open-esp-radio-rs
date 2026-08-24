//! Portable ESP-NOW protocol ownership for the initial plaintext profile.
//!
//! This module binds ESP-NOW to an existing station VIF and its one home
//! channel. It owns peer admission and frame construction, but deliberately
//! does not claim receive-filter, key-slot, PHY-low-rate or DMA authority.

use core::fmt;

use open_esp_radio_ieee80211::{
    channel::WifiChannel,
    esp_now::{
        EspNowDestination, EspNowRandomValue, EspNowUnicastAddress, EspNowV1Frame, EspNowV1Payload,
        EspNowV1WireError,
    },
    station::StaSequenceCounter,
};

use crate::interface::{BoundVirtualInterface, ChannelContextId, VifRole};

/// Default source-owned peer storage. The const-generic table remains usable
/// with a smaller application-selected capacity.
pub const ESP_NOW_DEFAULT_PEER_CAPACITY: usize = 20;

/// PHY policy requested for one peer.
///
/// A backend must reject `LongRange` unless it owns the complete LR enable,
/// PLCP, rate and receive-status contract. Merely possessing recovered rate
/// schedule bytes is not sufficient.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EspNowPhyMode {
    #[default]
    LegacyDsss1M,
    LongRange,
}

/// Security frontier of the initial service.
///
/// This single variant is intentional: no constructor accepts a PMK/LMK and
/// no backend handoff contains a hardware key selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPeerSecurity {
    Plaintext,
}

/// One ESP-NOW service bound to a station interface and one tuned channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowConfig {
    station: BoundVirtualInterface,
    local_address: EspNowUnicastAddress,
    home_channel: WifiChannel,
}

impl EspNowConfig {
    pub fn new(
        station: BoundVirtualInterface,
        home_channel: WifiChannel,
    ) -> Result<Self, EspNowConfigError> {
        if station.interface.role != VifRole::Station {
            return Err(EspNowConfigError::RequiresStationInterface);
        }
        let local_address = EspNowUnicastAddress::new(station.interface.address)
            .map_err(|_| EspNowConfigError::InvalidStationAddress)?;
        Ok(Self {
            station,
            local_address,
            home_channel,
        })
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn local_address(self) -> EspNowUnicastAddress {
        self.local_address
    }

    pub const fn home_channel(self) -> WifiChannel {
        self.home_channel
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowConfigError {
    RequiresStationInterface,
    InvalidStationAddress,
}

impl fmt::Display for EspNowConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RequiresStationInterface => {
                "ESP-NOW currently requires an existing station virtual interface"
            }
            Self::InvalidStationAddress => {
                "ESP-NOW requires a valid individual station MAC address"
            }
        })
    }
}

impl core::error::Error for EspNowConfigError {}

/// Plaintext configuration for one broadcast or individual peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeerConfig {
    destination: EspNowDestination,
    channel: WifiChannel,
    phy_mode: EspNowPhyMode,
}

impl EspNowPeerConfig {
    /// Configure a peer on the service's home channel using the supported
    /// standard one-megabit Action-frame profile.
    pub const fn plaintext(destination: EspNowDestination, channel: WifiChannel) -> Self {
        Self {
            destination,
            channel,
            phy_mode: EspNowPhyMode::LegacyDsss1M,
        }
    }

    /// Request a typed PHY mode. Unsupported chip backends must fail before
    /// publishing the frame rather than lowering LR to an ordinary rate code.
    pub const fn with_phy_mode(mut self, phy_mode: EspNowPhyMode) -> Self {
        self.phy_mode = phy_mode;
        self
    }

    pub const fn destination(self) -> EspNowDestination {
        self.destination
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Plaintext
    }
}

/// Generation-checked identity of one peer-table slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeerId {
    index: usize,
    generation: u32,
}

impl EspNowPeerId {
    pub const fn index(self) -> usize {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy)]
struct EspNowPeerSlot {
    generation: u32,
    config: Option<EspNowPeerConfig>,
}

impl EspNowPeerSlot {
    const EMPTY: Self = Self {
        generation: 0,
        config: None,
    };
}

/// Fixed-capacity, same-home-channel plaintext peer owner.
pub struct EspNowPeerTable<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    home_channel: WifiChannel,
    slots: [EspNowPeerSlot; N],
    length: usize,
}

impl<const N: usize> EspNowPeerTable<N> {
    pub const fn new(home_channel: WifiChannel) -> Self {
        Self {
            home_channel,
            slots: [EspNowPeerSlot::EMPTY; N],
            length: 0,
        }
    }

    pub const fn home_channel(&self) -> WifiChannel {
        self.home_channel
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn add(&mut self, config: EspNowPeerConfig) -> Result<EspNowPeerId, EspNowPeerTableError> {
        if config.channel != self.home_channel {
            return Err(EspNowPeerTableError::ChannelMismatch {
                peer: config.channel,
                home: self.home_channel,
            });
        }

        let mut reusable = None;
        let mut exhausted_slot = false;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .config
                .is_some_and(|peer| peer.destination == config.destination)
            {
                return Err(EspNowPeerTableError::Duplicate(config.destination));
            }
            if slot.config.is_none() && reusable.is_none() {
                if slot.generation == u32::MAX {
                    exhausted_slot = true;
                } else {
                    reusable = Some(index);
                }
            }
        }

        let Some(index) = reusable else {
            return Err(if exhausted_slot && self.length < N {
                EspNowPeerTableError::GenerationExhausted
            } else {
                EspNowPeerTableError::Full
            });
        };
        let slot = &mut self.slots[index];
        slot.generation += 1;
        slot.config = Some(config);
        self.length += 1;
        Ok(EspNowPeerId {
            index,
            generation: slot.generation,
        })
    }

    pub fn get(&self, peer: EspNowPeerId) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        let Some(slot) = self.slots.get(peer.index) else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowPeerTableError::Stale(peer));
        }
        slot.config.ok_or(EspNowPeerTableError::Stale(peer))
    }

    pub fn find(&self, destination: EspNowDestination) -> Option<EspNowPeerId> {
        self.slots
            .iter()
            .enumerate()
            .find(|(_, slot)| {
                slot.config
                    .is_some_and(|peer| peer.destination == destination)
            })
            .map(|(index, slot)| EspNowPeerId {
                index,
                generation: slot.generation,
            })
    }

    pub fn remove(&mut self, peer: EspNowPeerId) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        let Some(slot) = self.slots.get_mut(peer.index) else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowPeerTableError::Stale(peer));
        }
        let Some(config) = slot.config.take() else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        self.length -= 1;
        Ok(config)
    }

    pub const fn iter(&self) -> EspNowPeers<'_, N> {
        EspNowPeers {
            table: self,
            next: 0,
        }
    }
}

/// Borrowed iterator over occupied peer slots.
pub struct EspNowPeers<'table, const N: usize> {
    table: &'table EspNowPeerTable<N>,
    next: usize,
}

impl<const N: usize> Iterator for EspNowPeers<'_, N> {
    type Item = (EspNowPeerId, EspNowPeerConfig);

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < N {
            let index = self.next;
            self.next += 1;
            let slot = self.table.slots[index];
            if let Some(config) = slot.config {
                return Some((
                    EspNowPeerId {
                        index,
                        generation: slot.generation,
                    },
                    config,
                ));
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(N.saturating_sub(self.next)))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPeerTableError {
    Full,
    Duplicate(EspNowDestination),
    ChannelMismatch {
        peer: WifiChannel,
        home: WifiChannel,
    },
    GenerationExhausted,
    Stale(EspNowPeerId),
}

impl fmt::Display for EspNowPeerTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("the ESP-NOW peer table is full"),
            Self::Duplicate(destination) => {
                write!(
                    formatter,
                    "ESP-NOW peer {destination:?} is already configured"
                )
            }
            Self::ChannelMismatch { peer, home } => write!(
                formatter,
                "ESP-NOW peer channel {peer:?} differs from home channel {home:?}"
            ),
            Self::GenerationExhausted => {
                formatter.write_str("an ESP-NOW peer generation counter is exhausted")
            }
            Self::Stale(peer) => write!(formatter, "ESP-NOW peer handle {peer:?} is stale"),
        }
    }
}

impl core::error::Error for EspNowPeerTableError {}

/// Portable ESP-NOW protocol owner. Sequence ownership remains explicit so a
/// station service can share its existing non-QoS sequence space correctly.
pub struct EspNowProtocol<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    config: EspNowConfig,
    peers: EspNowPeerTable<N>,
}

impl<const N: usize> EspNowProtocol<N> {
    pub const fn new(config: EspNowConfig) -> Self {
        Self {
            config,
            peers: EspNowPeerTable::new(config.home_channel),
        }
    }

    pub const fn config(&self) -> EspNowConfig {
        self.config
    }

    pub const fn peers(&self) -> &EspNowPeerTable<N> {
        &self.peers
    }

    pub fn add_peer(
        &mut self,
        peer: EspNowPeerConfig,
    ) -> Result<EspNowPeerId, EspNowPeerTableError> {
        self.peers.add(peer)
    }

    pub fn remove_peer(
        &mut self,
        peer: EspNowPeerId,
    ) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        self.peers.remove(peer)
    }

    /// Resolve a peer, validate the payload, then consume one shared station
    /// management/non-QoS sequence number and return a backend handoff.
    pub fn prepare_v1_tx<'payload>(
        &self,
        peer: EspNowPeerId,
        sequence: &mut StaSequenceCounter,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<EspNowPreparedV1Tx<'payload>, EspNowSendError> {
        let peer_config = self.peers.get(peer).map_err(EspNowSendError::Peer)?;
        // Validate before advancing the shared sequence space.
        EspNowV1Payload::new(payload).map_err(EspNowSendError::Wire)?;
        let frame = EspNowV1Frame::new(
            peer_config.destination,
            self.config.local_address,
            sequence.take(),
            random_value,
            payload,
        )
        .map_err(EspNowSendError::Wire)?;
        Ok(EspNowPreparedV1Tx {
            peer,
            home_channel: self.config.home_channel,
            station: self.config.station,
            phy_mode: peer_config.phy_mode,
            frame,
        })
    }

    /// Parse and admit a plaintext frame addressed to this service from one
    /// configured individual peer. A configured broadcast destination grants
    /// TX authority only; it never acts as a wildcard RX peer. The active
    /// binding and channel must come from the exclusive normal RX owner; a
    /// passive monitor observation is not sufficient authority.
    pub fn receive_v1<'frame>(
        &self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        bytes: &'frame [u8],
    ) -> Result<EspNowReceivedV1<'frame>, EspNowReceiveError> {
        if active_station != self.config.station {
            return Err(EspNowReceiveError::StationBindingMismatch {
                configured: self.config.station,
                active: active_station,
            });
        }
        if active_channel != self.config.home_channel {
            return Err(EspNowReceiveError::ChannelMismatch {
                configured: self.config.home_channel,
                active: active_channel,
            });
        }
        let frame = EspNowV1Frame::parse(bytes).map_err(EspNowReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowReceiveError::ForeignDestination(frame.destination()));
        }
        let source = frame.source();
        let Some(peer) = self.peers.find(EspNowDestination::Unicast(source)) else {
            return Err(EspNowReceiveError::UnknownPeer(source));
        };
        Ok(EspNowReceivedV1 { peer, frame })
    }
}

/// Fully validated plaintext MPDU handoff to one channel-bound backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPreparedV1Tx<'payload> {
    peer: EspNowPeerId,
    home_channel: WifiChannel,
    station: BoundVirtualInterface,
    phy_mode: EspNowPhyMode,
    frame: EspNowV1Frame<'payload>,
}

impl EspNowPreparedV1Tx<'_> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(self) -> EspNowDestination {
        self.frame.destination()
    }

    pub const fn home_channel(self) -> WifiChannel {
        self.home_channel
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Plaintext
    }

    pub const fn encoded_len(self) -> usize {
        self.frame.encoded_len()
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        self.frame.encode(output)
    }
}

/// Protocol-admitted borrowed receive frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowReceivedV1<'frame> {
    peer: EspNowPeerId,
    frame: EspNowV1Frame<'frame>,
}

impl<'frame> EspNowReceivedV1<'frame> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn frame(self) -> EspNowV1Frame<'frame> {
        self.frame
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowSendError {
    Peer(EspNowPeerTableError),
    Wire(EspNowV1WireError),
}

impl fmt::Display for EspNowSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(error) => write!(formatter, "ESP-NOW peer error: {error}"),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
        }
    }
}

impl core::error::Error for EspNowSendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowReceiveError {
    StationBindingMismatch {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelMismatch {
        configured: WifiChannel,
        active: WifiChannel,
    },
    Wire(EspNowV1WireError),
    ForeignDestination(EspNowDestination),
    UnknownPeer(EspNowUnicastAddress),
}

impl fmt::Display for EspNowReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StationBindingMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW configured station binding {configured:?} differs from active binding {active:?}"
            ),
            Self::ChannelMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW configured channel {configured:?} differs from active channel {active:?}"
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
            Self::ForeignDestination(destination) => {
                write!(formatter, "ESP-NOW frame is addressed to {destination:?}")
            }
            Self::UnknownPeer(source) => {
                write!(formatter, "ESP-NOW source {source:?} is not configured")
            }
        }
    }
}

impl core::error::Error for EspNowReceiveError {}
