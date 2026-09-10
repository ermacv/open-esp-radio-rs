//! A fixed LE 1M connection followed by Reset, without a Disconnect command.

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
    event::{Event, le::LeEvent},
    param::{AddrKind, BdAddr, Duration as HciDuration, EventMask, LeConnRole, LeEventMask},
};
use std::time::{Duration, Instant};

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
    loop {
        let packet = user.receive(deadline)?;
        if connection_created(&packet, peer)? {
            break;
        }
    }
    let connected = Instant::now();
    report.connection_complete = true;
    report.connection_after_micros = Some(connected.duration_since(started).as_micros() as u64);
    oer_process::sleep(Duration::from_millis(u64::from(hold_ms)))?;
    // Capture when Reset is sent; its completion can arrive substantially later.
    report.reset_after_connection_micros = Some(connected.elapsed().as_micros() as u64);
    user.command(bt_hci::cmd::controller_baseband::Reset::new())?;
    report.reset_completed = true;
    Ok(())
}

fn connection_created(packet: &[u8], peer: PeerAddress) -> Result<bool> {
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
            Ok(false)
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
            Ok(true)
        }
        Event::HardwareError(_) => Err("controller hardware error during connection".into()),
        _ => Ok(false),
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
        assert!(connection_created(&packet, peer).unwrap());
        for offset in [4, 7, 8, 9, 15, 17, 19] {
            let mut wrong = packet.clone();
            wrong[offset] ^= 1;
            assert!(connection_created(&wrong, peer).is_err(), "offset {offset}");
        }
        assert!(!connection_created(&[4, 0x0f, 4, 0, 1, 0x0d, 0x20], peer).unwrap());
        assert!(connection_created(&[4, 0x0f, 4, 0x0c, 1, 0x0d, 0x20], peer).is_err());
        assert!(connection_created(&packet[..packet.len() - 1], peer).is_err());
        let mut extra = packet;
        extra.push(0);
        assert!(connection_created(&extra, peer).is_err());
    }
}
