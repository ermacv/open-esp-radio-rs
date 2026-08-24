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
/// Recent exact fingerprints retained independently for each RX peer.
pub const ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EspNowRxFingerprint {
    random_value: EspNowRandomValue,
    sequence_number: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EspNowRxPeerSlot {
    peer: Option<(EspNowPeerId, EspNowUnicastAddress)>,
    history: [Option<EspNowRxFingerprint>; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
    next_history: usize,
}

impl EspNowRxPeerSlot {
    const EMPTY: Self = Self {
        peer: None,
        history: [None; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
        next_history: 0,
    };
}

/// One standard OFDM rate explicitly selected for an ESP-NOW peer.
///
/// The values are kept portable instead of exposing a chip MAC rate byte.
/// A backend may support only a subset, but must reject an unsupported value
/// before publishing a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowOfdmRate {
    Mbps6,
    Mbps9,
    Mbps12,
    Mbps18,
    Mbps24,
    Mbps36,
    Mbps48,
    Mbps54,
}

/// One-spatial-stream MCS for the standard ESP-NOW HT20 profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowHtMcs {
    Mcs0,
    Mcs1,
    Mcs2,
    Mcs3,
    Mcs4,
    Mcs5,
    Mcs6,
    Mcs7,
}

impl EspNowHtMcs {
    pub const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => 0,
            Self::Mcs1 => 1,
            Self::Mcs2 => 2,
            Self::Mcs3 => 3,
            Self::Mcs4 => 4,
            Self::Mcs5 => 5,
            Self::Mcs6 => 6,
            Self::Mcs7 => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowHtGuardInterval {
    Long800Ns,
    Short400Ns,
}

/// A complete standard HT20 rate requested for one ESP-NOW peer.
///
/// MCS32 is deliberately absent: duplicate mode has a distinct hardware
/// certification boundary and cannot be represented as a one-stream MCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowHt20Rate {
    mcs: EspNowHtMcs,
    guard_interval: EspNowHtGuardInterval,
}

impl EspNowHt20Rate {
    pub const fn new(mcs: EspNowHtMcs, guard_interval: EspNowHtGuardInterval) -> Self {
        Self {
            mcs,
            guard_interval,
        }
    }

    pub const fn mcs(self) -> EspNowHtMcs {
        self.mcs
    }

    pub const fn guard_interval(self) -> EspNowHtGuardInterval {
        self.guard_interval
    }
}

/// PHY policy requested for one peer.
///
/// The two `StandardP2p*` variants use the dedicated P2P retry arenas on
/// backends which own them. A backend must reject `LongRange` unless it owns
/// the complete LR enable, PLCP, rate and receive-status contract. Merely
/// possessing recovered LR rate schedule bytes is not sufficient.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EspNowPhyMode {
    #[default]
    LegacyDsss1M,
    StandardP2pOfdm(EspNowOfdmRate),
    StandardP2pHt20(EspNowHt20Rate),
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

    /// Snapshot the configured individual peers into one exclusive normal-RX
    /// epoch.
    ///
    /// The caller must supply the station/channel owners which are live at
    /// this transition. This keeps a monitor observation or a stale peer
    /// table from manufacturing receive authority. Duplicate history belongs
    /// to the returned value and therefore cannot leak across stop/restart.
    pub fn begin_rx_epoch(
        &self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
    ) -> Result<EspNowRxEpoch<N>, EspNowReceiveError> {
        validate_active_binding(self.config, active_station, active_channel)?;

        let mut slots = [EspNowRxPeerSlot::EMPTY; N];
        let mut peer_count = 0;
        for (peer, config) in self.peers.iter() {
            let EspNowDestination::Unicast(address) = config.destination else {
                // A broadcast entry grants only broadcast TX authority.
                continue;
            };
            slots[peer.index] = EspNowRxPeerSlot {
                peer: Some((peer, address)),
                history: [None; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
                next_history: 0,
            };
            peer_count += 1;
        }
        Ok(EspNowRxEpoch {
            config: self.config,
            slots,
            peer_count,
        })
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
        validate_active_binding(self.config, active_station, active_channel)?;
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

fn validate_active_binding(
    config: EspNowConfig,
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
) -> Result<(), EspNowReceiveError> {
    if active_station != config.station {
        return Err(EspNowReceiveError::StationBindingMismatch {
            configured: config.station,
            active: active_station,
        });
    }
    if active_channel != config.home_channel {
        return Err(EspNowReceiveError::ChannelMismatch {
            configured: config.home_channel,
            active: active_channel,
        });
    }
    Ok(())
}

/// Peer snapshot and duplicate history owned by one normal receive epoch.
///
/// Dropping this value ends all receive authority. A later epoch must be
/// created again from [`EspNowProtocol::begin_rx_epoch`], which starts with
/// empty duplicate history and the then-current peer generation values.
#[derive(Debug, Eq, PartialEq)]
pub struct EspNowRxEpoch<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    config: EspNowConfig,
    slots: [EspNowRxPeerSlot; N],
    peer_count: usize,
}

impl<const N: usize> EspNowRxEpoch<N> {
    pub const fn config(&self) -> EspNowConfig {
        self.config
    }

    pub const fn peer_count(&self) -> usize {
        self.peer_count
    }

    /// Parse, address-check and admit one complete plaintext v1 MPDU.
    ///
    /// Duplicate identity intentionally combines the configured source with
    /// both on-air collision domains: the opaque random value and the
    /// management sequence number. Suppression happens before any borrowed
    /// payload is published to an integration callback.
    pub fn receive_v1<'frame>(
        &mut self,
        bytes: &'frame [u8],
    ) -> Result<EspNowRxOutcome<'frame>, EspNowReceiveError> {
        let frame = EspNowV1Frame::parse(bytes).map_err(EspNowReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowReceiveError::ForeignDestination(frame.destination()));
        }

        let source = frame.source();
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.peer
                .is_some_and(|(_, configured_source)| configured_source == source)
        }) else {
            return Err(EspNowReceiveError::UnknownPeer(source));
        };
        let (peer, _) = slot
            .peer
            .expect("an ESP-NOW RX slot selected by source is occupied");
        let fingerprint = EspNowRxFingerprint {
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
        };
        if slot.history.contains(&Some(fingerprint)) {
            return Ok(EspNowRxOutcome::Duplicate { peer });
        }
        slot.history[slot.next_history] = Some(fingerprint);
        slot.next_history = (slot.next_history + 1) % ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY;
        Ok(EspNowRxOutcome::Received(EspNowReceivedV1 { peer, frame }))
    }

    /// Clear duplicate state before the surrounding normal-RX owner is
    /// returned to a lifecycle composition root.
    pub fn reset_duplicate_history(&mut self) -> usize {
        let mut cleared = 0;
        for slot in &mut self.slots {
            for fingerprint in &mut slot.history {
                if fingerprint.take().is_some() {
                    cleared += 1;
                }
            }
            slot.next_history = 0;
        }
        cleared
    }
}

/// Result of ESP-NOW admission before integration publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxOutcome<'frame> {
    Received(EspNowReceivedV1<'frame>),
    Duplicate { peer: EspNowPeerId },
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

/// Owned v1 datagram safe to retain after the RX staging lease is released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedReceivedV1 {
    peer: EspNowPeerId,
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    random_value: EspNowRandomValue,
    sequence_number: u16,
    retry: bool,
    payload_length: u8,
    payload: [u8; open_esp_radio_ieee80211::esp_now::ESP_NOW_V1_MAX_PAYLOAD_LEN],
}

impl EspNowOwnedReceivedV1 {
    /// Copy one already-admitted borrowed datagram into fixed-capacity owned
    /// storage. The wire codec proves the payload length is at most 250.
    pub fn copy_from(received: EspNowReceivedV1<'_>) -> Self {
        let frame = received.frame();
        let payload = frame.action().payload().bytes();
        let mut owned_payload =
            [0_u8; open_esp_radio_ieee80211::esp_now::ESP_NOW_V1_MAX_PAYLOAD_LEN];
        owned_payload[..payload.len()].copy_from_slice(payload);
        Self {
            peer: received.peer(),
            destination: frame.destination(),
            source: frame.source(),
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
            retry: frame.retry(),
            payload_length: payload.len() as u8,
            payload: owned_payload,
        }
    }

    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(&self) -> EspNowDestination {
        self.destination
    }

    pub const fn source(&self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn sequence_number(&self) -> u16 {
        self.sequence_number
    }

    pub const fn retry(&self) -> bool {
        self.retry
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
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
