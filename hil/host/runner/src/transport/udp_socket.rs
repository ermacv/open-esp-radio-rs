//! Host UDP socket setup shared by throughput qualifiers.

use std::{io, net::UdpSocket, os::fd::AsRawFd};

/// The saturated ESP32-S31 TX stream can exceed 90 Mbit/s.  The Linux default
/// receive queue is commonly only 212,992 bytes, which is short enough to
/// overflow during an ordinary scheduler pause and falsely attribute host
/// loss to the radio driver.
pub(crate) const QUALIFICATION_RECEIVE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Request and read back a qualification-sized UDP receive queue.
///
/// Linux reports twice the requested `SO_RCVBUF` value because the returned
/// size includes kernel bookkeeping.  Callers retain the read-back value as
/// evidence instead of assuming that the request was accepted.
pub(crate) fn configure_qualification_receive_buffer(socket: &UdpSocket) -> io::Result<usize> {
    let requested = libc::c_int::try_from(QUALIFICATION_RECEIVE_BUFFER_BYTES)
        .expect("qualification UDP receive buffer fits c_int");
    // SAFETY: `socket` owns a live descriptor; `requested` is an initialized
    // integer whose pointer and exact length remain valid for the call.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&raw const requested).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    let mut actual = 0 as libc::c_int;
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: both output pointers refer to live initialized storage with the
    // declared length, and the socket descriptor remains owned for the call.
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&raw mut actual).cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != size_of::<libc::c_int>() || actual <= 0 {
        return Err(io::Error::other("invalid SO_RCVBUF read-back"));
    }
    usize::try_from(actual).map_err(|_| io::Error::other("negative SO_RCVBUF read-back"))
}

/// Opens the reverse conntrack path without inheriting a late ICMP error from
/// a preceding reset-separated run that reused the same UDP four-tuple.
pub(crate) fn open_reverse_flow(socket: &UdpSocket) -> io::Result<()> {
    // A target reset closes its bound UDP port. A delayed ICMP Port
    // Unreachable can be associated with the next connected host socket when
    // the qualification deliberately reuses the fixed port. It describes the
    // preceding epoch, so drain it before sending the new epoch's probe.
    socket.take_error()?;
    socket.send(&[0]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn qualification_socket_reads_back_at_least_the_requested_capacity() {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let actual = configure_qualification_receive_buffer(&socket).unwrap();
        assert!(actual >= QUALIFICATION_RECEIVE_BUFFER_BYTES);
    }
}
