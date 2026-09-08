//! Linux HCI sockets with bounded, cancellable command completion.
//! HCI payloads use bt-hci; only Linux management and H4 framing live here.

use super::Result;
use bt_hci::{
    FromHciBytes,
    cmd::SyncCmd,
    event::{CommandCompleteWithStatus, Event},
};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::{Duration, Instant},
};

pub(super) struct Socket(OwnedFd);

#[derive(Debug)]
pub(super) struct ManagementError {
    opcode: u16,
    pub(super) status: u8,
}
impl std::fmt::Display for ManagementError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            output,
            "management {:#06x}: status {:#04x}",
            self.opcode, self.status
        )
    }
}
impl std::error::Error for ManagementError {}

impl Socket {
    pub(super) fn open(index: u16, channel: u16) -> Result<Self> {
        #[repr(C)]
        struct Address {
            family: libc::sa_family_t,
            index: u16,
            channel: u16,
        }
        // SAFETY: socket creates an owned descriptor, with no borrowed memory.
        let fd = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                1,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: fd is newly created and ownership is transferred once.
        let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        let address = Address {
            family: libc::AF_BLUETOOTH as _,
            index,
            channel,
        };
        // SAFETY: address is a live, correctly sized Linux sockaddr_hci.
        if unsafe {
            libc::bind(
                socket.0.as_raw_fd(),
                (&address as *const Address).cast(),
                size_of::<Address>() as _,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(socket)
    }

    pub(super) fn send(&self, bytes: &[u8]) -> Result<()> {
        oer_process::check_cancelled()?;
        // SAFETY: bytes is readable for its length and the descriptor is owned.
        let count = unsafe {
            libc::send(
                self.0.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if count as usize != bytes.len() {
            return Err("short HCI socket write".into());
        }
        Ok(())
    }

    pub(super) fn receive(&self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            oer_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("HCI response timeout".into());
            }
            let mut bytes = vec![0; 4096];
            // SAFETY: bytes provides writable storage and the descriptor is owned.
            let count = unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_TRUNC,
                )
            };
            if count >= 0 {
                if count == 0 || count as usize > bytes.len() {
                    return Err("empty or truncated HCI packet".into());
                }
                bytes.truncate(count as usize);
                return Ok(bytes);
            }
            let error = io::Error::last_os_error();
            if !matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) {
                return Err(error.into());
            }
            oer_process::sleep(Duration::from_millis(10))?;
        }
    }

    pub(super) fn command<C: SyncCmd>(&self, command: C) -> Result<C::Return> {
        let mut bytes = vec![0; 1 + bt_hci::WriteHci::size(&command)];
        bytes[0] = 1;
        bt_hci::WriteHci::write_hci(&command, &mut bytes[1..])
            .map_err(|error| format!("HCI encode: {error:?}"))?;
        self.send(&bytes)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = self.receive(deadline)?;
            if let Some(value) = completion::<C>(&packet)? {
                return Ok(value);
            }
        }
    }

    pub(super) fn management(&self, index: u16, opcode: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes.extend(opcode.to_le_bytes());
        bytes.extend(index.to_le_bytes());
        bytes.extend((payload.len() as u16).to_le_bytes());
        bytes.extend(payload);
        self.send(&bytes)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = self.receive(deadline)?;
            if let Some(value) = management_completion(&packet, index, opcode)? {
                return Ok(value);
            }
        }
    }
}

fn completion<C: SyncCmd>(packet: &[u8]) -> Result<Option<C::Return>> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid HCI event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing HCI event bytes".into());
    }
    match event {
        Event::CommandComplete(event) if event.cmd_opcode == C::OPCODE => {
            let event = CommandCompleteWithStatus::try_from(event)
                .map_err(|error| format!("HCI completion: {error:?}"))?;
            event
                .to_result::<C>()
                .map(Some)
                .map_err(|error| format!("HCI {:?} rejected: {error:?}", C::OPCODE).into())
        }
        Event::CommandStatus(event) if event.cmd_opcode == C::OPCODE => {
            event
                .status
                .to_result()
                .map_err(|error| format!("HCI {:?} status: {error:?}", C::OPCODE))?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn management_completion(packet: &[u8], index: u16, opcode: u16) -> Result<Option<Vec<u8>>> {
    if packet.len() < 6 {
        return Err("short management header".into());
    }
    let read = |offset| u16::from_le_bytes([packet[offset], packet[offset + 1]]);
    if packet.len() != 6 + usize::from(read(4)) {
        return Err("invalid management length".into());
    }
    if read(2) != index || !matches!(read(0), 1 | 2) {
        return Ok(None);
    }
    if packet.len() < 9 {
        return Err("short management completion".into());
    }
    if read(6) != opcode {
        return Ok(None);
    }
    if packet[8] != 0 {
        return Err(ManagementError {
            opcode,
            status: packet[8],
        }
        .into());
    }
    if read(0) == 2 {
        return Ok(None);
    }
    Ok(Some(packet[9..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::cmd::le::LeTestEnd;
    #[test]
    fn socket_exchange_and_expired_deadline_are_bounded_without_hardware() {
        let (client, server) = std::os::unix::net::UnixDatagram::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let peer = std::thread::spawn(move || {
            let mut request = [0; 64];
            let count = server.recv(&mut request).unwrap();
            assert!(count > 1 && request[0] == 1);
            server.send(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0]).unwrap();
        });
        let socket = Socket(client.into());
        assert_eq!(socket.command(LeTestEnd::new()).unwrap(), 42);
        peer.join().unwrap();
        assert!(
            socket
                .receive(Instant::now())
                .unwrap_err()
                .to_string()
                .contains("timeout")
        );
    }
    #[test]
    fn receiver_count_requires_matching_successful_complete_packet() {
        assert_eq!(
            completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0]).unwrap(),
            Some(42)
        );
        assert!(
            completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1e, 0x20, 0, 42, 0])
                .unwrap()
                .is_none()
        );
        assert!(completion::<LeTestEnd>(&[4, 14, 4, 1, 0x1f, 0x20, 0x0c]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0, 0]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 15, 4, 0x0c, 1, 0x1f, 0x20]).is_err());
    }
    #[test]
    fn management_ignores_other_adapters_and_checks_status_and_length() {
        let packet = [1, 0, 0, 0, 4, 0, 4, 0, 0, 123];
        assert_eq!(
            management_completion(&packet, 0, 4).unwrap(),
            Some(vec![123])
        );
        assert!(management_completion(&packet, 1, 4).unwrap().is_none());
        assert!(management_completion(&packet, 0, 5).unwrap().is_none());
        let mut rejected = packet;
        rejected[8] = 0x0a;
        assert!(management_completion(&rejected, 0, 4).is_err());
        assert!(management_completion(&packet[..9], 0, 4).is_err());
    }
}
