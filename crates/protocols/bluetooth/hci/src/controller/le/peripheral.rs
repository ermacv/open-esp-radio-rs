//! Owned standard HCI events for an established LE peripheral connection.
//!
//! The Link Layer and chip runtime remain responsible for proving connection
//! establishment and teardown. This module only owns their Host-visible HCI
//! representation so the exact event can survive bounded output backpressure.

use bt_hci::{
    PacketKind,
    event::{EventKind, le::LeConnectionComplete, le::LeEventParams},
    param::{AddrKind, BdAddr, ClockAccuracy, ConnHandle, Duration, LeConnRole, Status},
};

use crate::HciControllerResponse;

/// Complete LE Connection Complete event size without an H4 indicator.
pub const LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY: usize = 21;
/// Complete Disconnection Complete event size without an H4 indicator.
pub const LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY: usize = 6;

/// Why an established legacy connection cannot be represented by this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralConnectionCompleteEventError {
    /// Legacy CONNECT_IND carries only a public or random peer address kind.
    UnsupportedPeerAddressKind(AddrKind),
    /// A failed-completion constructor was given the success status.
    SuccessfulFailureStatus,
}

/// One owned successful LE Connection Complete event for the peripheral role.
///
/// `bt-hci` 0.10 exposes parsing but not construction for Controller events.
/// This owner accepts its field-domain types and is regression-decoded through
/// the standard event model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePeripheralConnectionCompleteEvent {
    bytes: [u8; LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY],
}

struct LeConnectionCompleteFields {
    handle: ConnHandle,
    peer_address_kind: AddrKind,
    peer_address: BdAddr,
    connection_interval: Duration<1_250>,
    peripheral_latency: u16,
    supervision_timeout: Duration<10_000>,
    central_clock_accuracy: ClockAccuracy,
}

impl LePeripheralConnectionCompleteEvent {
    /// Build a successful standard LE Connection Complete event.
    pub fn new(
        handle: ConnHandle,
        peer_address_kind: AddrKind,
        peer_address: BdAddr,
        connection_interval: Duration<1_250>,
        peripheral_latency: u16,
        supervision_timeout: Duration<10_000>,
        central_clock_accuracy: ClockAccuracy,
    ) -> Result<Self, LePeripheralConnectionCompleteEventError> {
        if peer_address_kind != AddrKind::PUBLIC && peer_address_kind != AddrKind::RANDOM {
            return Err(
                LePeripheralConnectionCompleteEventError::UnsupportedPeerAddressKind(
                    peer_address_kind,
                ),
            );
        }

        Ok(Self::from_fields(
            Status::SUCCESS,
            LeConnectionCompleteFields {
                handle,
                peer_address_kind,
                peer_address,
                connection_interval,
                peripheral_latency,
                supervision_timeout,
                central_clock_accuracy,
            },
        ))
    }

    /// Build a failed LE Connection Complete event with no allocated handle.
    pub fn failed(status: Status) -> Result<Self, LePeripheralConnectionCompleteEventError> {
        if status == Status::SUCCESS {
            return Err(LePeripheralConnectionCompleteEventError::SuccessfulFailureStatus);
        }
        let mut bytes = [0; LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = (LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8;
        bytes[2] = LeConnectionComplete::SUBEVENT_CODE;
        bytes[3] = status.into_inner();
        Ok(Self { bytes })
    }

    fn from_fields(status: Status, fields: LeConnectionCompleteFields) -> Self {
        let LeConnectionCompleteFields {
            handle,
            peer_address_kind,
            peer_address,
            connection_interval,
            peripheral_latency,
            supervision_timeout,
            central_clock_accuracy,
        } = fields;
        let handle = handle.raw().to_le_bytes();
        let interval = connection_interval.as_u16().to_le_bytes();
        let latency = peripheral_latency.to_le_bytes();
        let timeout = supervision_timeout.as_u16().to_le_bytes();
        let mut bytes = [0; LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = (LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8;
        bytes[2] = LeConnectionComplete::SUBEVENT_CODE;
        bytes[3] = status.into_inner();
        bytes[4..6].copy_from_slice(&handle);
        bytes[6] = LeConnRole::Peripheral as u8;
        bytes[7] = peer_address_kind.as_raw();
        bytes[8..14].copy_from_slice(peer_address.raw());
        bytes[14..16].copy_from_slice(&interval);
        bytes[16..18].copy_from_slice(&latency);
        bytes[18..20].copy_from_slice(&timeout);
        bytes[20] = central_clock_accuracy as u8;
        Self { bytes }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LePeripheralConnectionCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// One owned successful Disconnection Complete event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDisconnectionCompleteEvent {
    bytes: [u8; LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY],
}

impl LeDisconnectionCompleteEvent {
    /// Build a standard Disconnection Complete event for a terminated link.
    pub fn new(handle: ConnHandle, reason: Status) -> Self {
        let handle = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::DisconnectionComplete.0,
                (LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8,
                Status::SUCCESS.into_inner(),
                handle[0],
                handle[1],
                reason.into_inner(),
            ],
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeDisconnectionCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests;
