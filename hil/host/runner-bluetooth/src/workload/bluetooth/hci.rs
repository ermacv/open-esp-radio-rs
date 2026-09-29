//! Raw HCI commands to the Controller of the `bluetooth_hci` image.
use crate::Result;
use hil_core::session::SerialCapture;
use oer_hil_protocol::bluetooth::{
    BluetoothHciLifecycle, BluetoothHciLifecycleEvidence, BluetoothHciRequest, BluetoothHciResponse,
};
use std::time::Duration;

pub const RESET: u16 = 0x0c03;
pub const READ_BD_ADDR: u16 = 0x1009;
pub const LE_SET_ADVERTISING_PARAMETERS: u16 = 0x2006;
pub const LE_SET_ADVERTISING_DATA: u16 = 0x2008;
pub const LE_SET_SCAN_RESPONSE_DATA: u16 = 0x2009;
pub const LE_SET_ADVERTISING_ENABLE: u16 = 0x200a;
pub const DISCONNECT: u16 = 0x0406;
pub const SET_EVENT_MASK: u16 = 0x0c01;
pub const SET_CONTROLLER_TO_HOST_FLOW_CONTROL: u16 = 0x0c31;
pub const HOST_BUFFER_SIZE: u16 = 0x0c33;
pub const HOST_NUMBER_OF_COMPLETED_PACKETS: u16 = 0x0c35;
pub const LE_READ_BUFFER_SIZE: u16 = 0x2002;

/// The default event mask plus LE Meta.
pub const EVENT_MASK_WITH_LE_META: [u8; 8] = [0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20];

/// Wait for the image's hello and require raw HCI exchanges.
pub fn require(capture: &SerialCapture) -> Result<()> {
    let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
    if !capabilities.has::<oer_hil_protocol::bluetooth::Hci>() {
        return Err("firmware lacks raw Bluetooth HCI exchanges".into());
    }
    Ok(())
}

/// Send one command and return the return parameters of its successful
/// Command Complete.
pub fn command(capture: &SerialCapture, opcode: u16, parameters: &[u8]) -> Result<Vec<u8>> {
    let request = BluetoothHciRequest::Command {
        opcode,
        parameters: heapless::Vec::from_slice(parameters)
            .map_err(|_| format!("HCI command {opcode:#06x} parameters exceed the request"))?,
    };
    match capture.bluetooth_hci(request)? {
        // Code, length, packets, opcode, status, return parameters.
        BluetoothHciResponse::Completed(packet)
            if packet[0] == 0x0e && packet.len() >= 6 && packet[5] == 0 =>
        {
            Ok(packet[6..].to_vec())
        }
        response => Err(format!("HCI command {opcode:#06x} failed: {response:?}").into()),
    }
}

/// Legacy advertising or scan-response data padded to the command's 31
/// octets.
pub fn data(bytes: &[u8]) -> Result<[u8; 32]> {
    let mut parameters = [0; 32];
    if bytes.len() > 31 {
        return Err("legacy advertising data exceeds 31 octets".into());
    }
    parameters[0] = bytes.len() as u8;
    parameters[1..=bytes.len()].copy_from_slice(bytes);
    Ok(parameters)
}

/// The next LE Meta event of `subevent`, skipping other events and ACL data,
/// within `timeout`.
pub fn le_meta_event(capture: &SerialCapture, subevent: u8, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!("no LE Meta subevent {subevent:#04x} within {timeout:?}").into());
        }
        let wait_ms = left.as_millis().min(1_000) as u16;
        match capture.bluetooth_hci(BluetoothHciRequest::NextPacket { wait_ms })? {
            BluetoothHciResponse::Event { packet, .. }
                if packet[0] == 0x3e && packet.get(2) == Some(&subevent) =>
            {
                return Ok(packet.to_vec());
            }
            BluetoothHciResponse::Event { .. }
            | BluetoothHciResponse::Acl { .. }
            | BluetoothHciResponse::NoPacket => {}
            response => return Err(format!("HCI event wait failed: {response:?}").into()),
        }
    }
}

/// Send one command and return its Command Status.
pub fn command_status(capture: &SerialCapture, opcode: u16, parameters: &[u8]) -> Result<u8> {
    let request = BluetoothHciRequest::Command {
        opcode,
        parameters: heapless::Vec::from_slice(parameters)
            .map_err(|_| format!("HCI command {opcode:#06x} parameters exceed the request"))?,
    };
    match capture.bluetooth_hci(request)? {
        BluetoothHciResponse::Completed(packet) if packet[0] == 0x0f && packet.len() >= 3 => {
            Ok(packet[2])
        }
        response => Err(format!("HCI command {opcode:#06x} failed: {response:?}").into()),
    }
}

/// One Controller-to-Host packet returned by [`next_packet`].
#[derive(Debug, PartialEq, Eq)]
pub enum Packet {
    /// Event code, parameter length and parameters.
    Event(Vec<u8>),
    /// ACL header and data.
    Acl(Vec<u8>),
}

/// The oldest queued Controller packet within `wait`, or `None`. A packet
/// lost to the image's bounded queue fails the exchange: no Host decision
/// may rest on an incomplete stream.
pub fn next_packet(capture: &SerialCapture, wait: Duration) -> Result<Option<Packet>> {
    let wait_ms = wait.as_millis().min(u128::from(u16::MAX)) as u16;
    let (packet, dropped) = match capture
        .bluetooth_hci(BluetoothHciRequest::NextPacket { wait_ms })?
    {
        BluetoothHciResponse::Event { packet, dropped } => {
            (Packet::Event(packet.to_vec()), dropped)
        }
        BluetoothHciResponse::Acl { packet, dropped } => (Packet::Acl(packet.to_vec()), dropped),
        BluetoothHciResponse::NoPacket => return Ok(None),
        response => return Err(format!("HCI packet wait failed: {response:?}").into()),
    };
    if dropped != 0 {
        return Err(format!("the image dropped {dropped} Controller packets").into());
    }
    Ok(Some(packet))
}

/// Send one ACL data packet, header included.
pub fn acl(capture: &SerialCapture, packet: &[u8]) -> Result<()> {
    let request = BluetoothHciRequest::Acl {
        packet: heapless::Vec::from_slice(packet)
            .map_err(|_| "HCI ACL packet exceeds the request")?,
    };
    match capture.bluetooth_hci(request)? {
        BluetoothHciResponse::Accepted => Ok(()),
        response => Err(format!("HCI ACL packet was not accepted: {response:?}").into()),
    }
}

/// End the Controller epoch: Reset, retire the Host end and stop the
/// Controller, then start the next epoch for [`BluetoothHciLifecycle::Restart`].
pub fn lifecycle(
    capture: &SerialCapture,
    operation: BluetoothHciLifecycle,
) -> Result<BluetoothHciLifecycleEvidence> {
    match capture.bluetooth_hci(BluetoothHciRequest::Lifecycle(operation))? {
        BluetoothHciResponse::Lifecycle(evidence) => Ok(evidence),
        response => Err(format!("Controller {operation:?} failed: {response:?}").into()),
    }
}

/// Return `count` consumed ACL packets of `handle` to the Controller.
pub fn return_credits(capture: &SerialCapture, handle: u16, count: u16) -> Result<()> {
    let mut parameters = [1, 0, 0, 0, 0];
    parameters[1..3].copy_from_slice(&handle.to_le_bytes());
    parameters[3..5].copy_from_slice(&count.to_le_bytes());
    let request = BluetoothHciRequest::Command {
        opcode: HOST_NUMBER_OF_COMPLETED_PACKETS,
        parameters: heapless::Vec::from_slice(&parameters).expect("five octets fit"),
    };
    match capture.bluetooth_hci(request)? {
        BluetoothHciResponse::Accepted => Ok(()),
        response => Err(format!("Host Number Of Completed Packets failed: {response:?}").into()),
    }
}
