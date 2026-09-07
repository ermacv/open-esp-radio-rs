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

/// Linux exports the socket's cumulative drop count, including a final lost
/// packet with no subsequent ancillary message. Read before and after collection.
#[cfg(target_os = "linux")]
pub(crate) fn kernel_drops(socket: &UdpSocket) -> io::Result<Option<u32>> {
    // Linux uapi: linux/sock_diag.h, enum SK_MEMINFO_* (DROPS is entry eight).
    let mut info = [0_u32; 9];
    let mut length = size_of_val(&info) as libc::socklen_t;
    let result = unsafe {
        // SAFETY: the output array and length are live for the exact supplied size.
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_MEMINFO,
            info.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if (length as usize) < size_of_val(&info) {
        return Err(io::Error::other(
            "SO_MEMINFO omitted socket drop accounting",
        ));
    }
    Ok(Some(info[8]))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn kernel_drops(_socket: &UdpSocket) -> io::Result<Option<u32>> {
    Ok(None)
}

/// Confirm the exact UDP flow before load can evict unresolved-neighbor packets.
/// A retry deadline is failure recovery, never an assumed readiness delay.
pub(crate) fn confirm_reverse_flow(
    socket: &UdpSocket,
    timeout: std::time::Duration,
) -> crate::Result<()> {
    use crate::transport::events::{EventPoll, deadline_after};
    use open_esp_radio_hil_protocol::UdpProbe;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64)
        .wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed));
    let request = UdpProbe {
        nonce,
        response: false,
    }
    .encode();
    let expected = UdpProbe {
        nonce,
        response: true,
    };
    let mut poll = EventPoll::new()?;
    poll.register(socket, false)?;
    let deadline = deadline_after(timeout);
    let mut next_send = Instant::now();
    let mut packet = [0_u8; UdpProbe::LENGTH + 1];
    loop {
        oer_process::check_cancelled()?;
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "UDP reverse-flow probe to {} timed out without its matching response",
                socket.peer_addr()?
            )
            .into());
        }
        if now >= next_send {
            socket.send(&request)?;
            next_send = now + Duration::from_millis(250);
        }
        // Per-call nonblocking receive preserves the socket's later send mode.
        let length = unsafe {
            // SAFETY: the live socket and output slice cover the exact lengths.
            libc::recv(
                socket.as_raw_fd(),
                packet.as_mut_ptr().cast(),
                packet.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if length >= 0 {
            if UdpProbe::decode(&packet[..length as usize]) == Some(expected) {
                return Ok(());
            }
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        poll.wait(Some(next_send.min(deadline)))?;
    }
}

#[cfg(test)]
mod tests;
