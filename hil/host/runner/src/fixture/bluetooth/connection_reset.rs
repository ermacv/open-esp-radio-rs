//! A fixed LE 1M connection and ACL echo followed by Reset, without Disconnect.

use super::{
    Result,
    hci::Socket,
    model::{ConnectionReset, PeerAddress},
};
use bt_hci::{
    FromHciBytes,
    cmd::{
        Cmd,
        controller_baseband::SetEventMask,
        le::{LeCreateConn, LeSetEventMask},
    },
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    event::{Event, le::LeEvent},
    param::{
        AddrKind, BdAddr, ConnHandle, Duration as HciDuration, EventMask, LeConnRole, LeEventMask,
    },
};
use std::time::{Duration, Instant};

pub(super) const ACL_ECHO_PAYLOAD: [u8; 12] = [
    8, 0, 0xff, 0xff, b'O', b'E', b'R', b'-', b'A', b'C', b'L', 1,
];

pub(super) fn run(
    user: &Socket,
    peer: PeerAddress,
    hold_ms: u16,
    report: &mut ConnectionReset,
) -> Result<()> {
    user.command(SetEventMask::new(
        EventMask::new()
            .enable_le_meta(true)
            .enable_hardware_error(true),
    ))?;
    // Request the legacy completion only, so there is one exact peer/role check.
    user.command(LeSetEventMask::new(
        LeEventMask::new().enable_le_conn_complete(true),
    ))?;
    let command = LeCreateConn::new(
        HciDuration::from_millis(60),
        HciDuration::from_millis(30),
        false,
        AddrKind::PUBLIC,
        BdAddr::new(peer.0),
        AddrKind::PUBLIC,
        HciDuration::from_millis(100),
        HciDuration::from_millis(100),
        0,
        HciDuration::from_millis(2_000),
        HciDuration::from_millis(0),
        HciDuration::from_millis(0),
    );
    let started = Instant::now();
    user.send_command(&command)?;
    let deadline = started + Duration::from_secs(10);
    let handle = loop {
        let packet = user.receive(deadline)?;
        if let Some(handle) = connection_created(&packet, peer)? {
            break handle;
        }
    };
    let connected = Instant::now();
    report.connection_complete = true;
    report.connection_after_micros = Some(connected.duration_since(started).as_micros() as u64);

    let acl = AclPacket::new(
        handle,
        AclPacketBoundary::FirstNonFlushable,
        AclBroadcastFlag::PointToPoint,
        &ACL_ECHO_PAYLOAD,
    );
    user.send_acl(&acl)?;
    report.acl_sent = true;
    report.acl_payload_bytes = Some(ACL_ECHO_PAYLOAD.len() as u16);
    let acl_started = Instant::now();
    let acl_deadline = acl_started + Duration::from_secs(5);
    loop {
        let packet = user.receive(acl_deadline)?;
        if acl_echoed(&packet, handle)? {
            report.acl_echo_received = true;
            report.acl_echo_after_micros = Some(
                acl_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            break;
        }
    }

    oer_process::sleep(Duration::from_millis(u64::from(hold_ms)))?;
    // Capture when Reset is sent; its completion can arrive substantially later.
    report.reset_after_connection_micros = Some(connected.elapsed().as_micros() as u64);
    user.command(bt_hci::cmd::controller_baseband::Reset::new())?;
    report.reset_completed = true;
    Ok(())
}

fn connection_created(packet: &[u8], peer: PeerAddress) -> Result<Option<ConnHandle>> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid connection event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing connection event bytes".into());
    }
    match event {
        Event::CommandStatus(status) if status.cmd_opcode == LeCreateConn::OPCODE => {
            status
                .status
                .to_result()
                .map_err(|error| format!("Create Connection rejected: {error:?}"))?;
            Ok(None)
        }
        Event::Le(LeEvent::LeConnectionComplete(event)) => {
            event
                .status
                .to_result()
                .map_err(|error| format!("connection failed: {error:?}"))?;
            if event.role != LeConnRole::Central
                || event.peer_addr_kind != AddrKind::PUBLIC
                || event.peer_addr != BdAddr::new(peer.0)
                || event.conn_interval != HciDuration::from_millis(100)
                || event.peripheral_latency != 0
                || event.supervision_timeout != HciDuration::from_millis(2_000)
            {
                return Err(
                    "connection completion does not match the requested peer and parameters".into(),
                );
            }
            Ok(Some(event.handle))
        }
        Event::HardwareError(_) => Err("controller hardware error during connection".into()),
        _ => Ok(None),
    }
}

fn acl_echoed(packet: &[u8], handle: ConnHandle) -> Result<bool> {
    match packet.first() {
        Some(&2) => {
            let (packet, rest) = AclPacket::from_hci_bytes(&packet[1..])
                .map_err(|error| format!("invalid HCI ACL echo: {error:?}"))?;
            if !rest.is_empty() {
                return Err("trailing HCI ACL echo bytes".into());
            }
            if packet.handle() != handle
                || packet.boundary_flag() != AclPacketBoundary::FirstFlushable
                || packet.broadcast_flag() != AclBroadcastFlag::PointToPoint
                || packet.data() != ACL_ECHO_PAYLOAD
            {
                return Err("HCI ACL echo does not match the transmitted packet".into());
            }
            Ok(true)
        }
        Some(&4) => {
            let (event, rest) = Event::from_hci_bytes(&packet[1..])
                .map_err(|error| format!("invalid HCI event while awaiting ACL echo: {error:?}"))?;
            if !rest.is_empty() {
                return Err("trailing HCI event bytes while awaiting ACL echo".into());
            }
            match event {
                Event::HardwareError(_) => Err("controller hardware error during ACL echo".into()),
                Event::DisconnectionComplete(_) => {
                    Err("connection ended before the ACL echo arrived".into())
                }
                _ => Ok(false),
            }
        }
        _ => Err("unexpected HCI packet while awaiting ACL echo".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(peer: PeerAddress) -> Vec<u8> {
        let mut packet = vec![4, 0x3e, 19, 1, 0, 1, 0, 0, 0];
        packet.extend(peer.0);
        packet.extend([80, 0, 0, 0, 200, 0, 0]);
        packet
    }

    #[test]
    fn only_the_exact_successful_connection_can_trigger_reset() {
        let peer = "30:ED:A0:F3:F6:D1".parse().unwrap();
        let packet = completion(peer);
        assert_eq!(
            connection_created(&packet, peer).unwrap(),
            Some(ConnHandle::new(1))
        );
        for offset in [4, 7, 8, 9, 15, 17, 19] {
            let mut wrong = packet.clone();
            wrong[offset] ^= 1;
            assert!(connection_created(&wrong, peer).is_err(), "offset {offset}");
        }
        assert_eq!(
            connection_created(&[4, 0x0f, 4, 0, 1, 0x0d, 0x20], peer).unwrap(),
            None
        );
        assert!(connection_created(&[4, 0x0f, 4, 0x0c, 1, 0x0d, 0x20], peer).is_err());
        assert!(connection_created(&packet[..packet.len() - 1], peer).is_err());
        let mut extra = packet;
        extra.push(0);
        assert!(connection_created(&extra, peer).is_err());
    }

    #[test]
    fn acl_echo_requires_the_exact_controller_packet() {
        let handle = ConnHandle::new(1);
        let acl = AclPacket::new(
            handle,
            AclPacketBoundary::FirstFlushable,
            AclBroadcastFlag::PointToPoint,
            &ACL_ECHO_PAYLOAD,
        );
        let mut packet = vec![2; 1 + bt_hci::WriteHci::size(&acl)];
        bt_hci::WriteHci::write_hci(&acl, &mut packet[1..]).unwrap();
        assert!(acl_echoed(&packet, handle).unwrap());

        packet[5] ^= 1;
        assert!(acl_echoed(&packet, handle).is_err());
        assert!(!acl_echoed(&[4, 0x13, 1, 0], handle).unwrap());
        assert!(acl_echoed(&[2, 1, 0x20, 12, 0], handle).is_err());
    }
}
