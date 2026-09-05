//! Shared bounded-burst UDP source for downlink HIL traffic.

use std::{
    hint,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    thread,
    time::{Duration, Instant},
};

use crate::Result;

const FINAL_SPIN: Duration = Duration::from_micros(30);
const MAX_CATCH_UP_INTERVALS: u32 = 4;
const TERMINAL_MARKERS: usize = 16;
const TERMINAL_MARKER_SPACING: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    pub(crate) address: Ipv4Addr,
    pub(crate) port: u16,
    pub(crate) rate_bps: u64,
    pub(crate) duration: Duration,
    pub(crate) payload: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HostTransmission {
    pub(crate) source: Ipv4Addr,
    pub(crate) bytes: u64,
    pub(crate) datagrams: u64,
    pub(crate) elapsed: Duration,
    pub(crate) maximum_lateness: Duration,
    pub(crate) maximum_catch_up_datagrams: u32,
    pub(crate) deadline_resets: u64,
}

impl HostTransmission {
    pub(crate) fn throughput_bps(self) -> u64 {
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000_000)
            .checked_div(
                u64::try_from(self.elapsed.as_micros())
                    .unwrap_or(u64::MAX)
                    .max(1),
            )
            .unwrap_or(0)
    }

    pub(crate) fn maximum_lateness_us(self) -> u64 {
        u64::try_from(self.maximum_lateness.as_micros()).unwrap_or(u64::MAX)
    }
}

pub(crate) fn send(config: Config) -> Result<HostTransmission> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect(SocketAddrV4::new(config.address, config.port))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    send_with(config, &socket, |packet| socket.send(packet))
}

/// Send one paced flow from an already-bound socket.
///
/// Multi-client AP qualification shares this socket with the reverse
/// target-to-host flow so both directions retain one exact peer endpoint.
pub(crate) fn send_on(socket: &UdpSocket, config: Config) -> Result<HostTransmission> {
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    send_with(config, socket, |packet| {
        socket.send_to(packet, SocketAddrV4::new(config.address, config.port))
    })
}

fn send_with(
    config: Config,
    socket: &UdpSocket,
    mut send_packet: impl FnMut(&[u8]) -> std::io::Result<usize>,
) -> Result<HostTransmission> {
    let source = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("paced UDP sender selected an IPv6 source".into()),
    };
    let mut packet = vec![0x5a; config.payload];
    let interval = packet_interval(config.payload, config.rate_bps)?;
    let maximum_catch_up = interval.saturating_mul(MAX_CATCH_UP_INTERVALS);
    let started = Instant::now();
    let deadline = started + config.duration;
    let mut next = started;
    let mut bytes = 0_u64;
    let mut datagrams = 0_u64;
    let mut maximum_lateness = Duration::ZERO;
    let mut maximum_catch_up_datagrams = 1_u32;
    let mut deadline_resets = 0_u64;

    while Instant::now() < deadline {
        oer_process::check_cancelled()?;
        wait_until(next)?;
        let now = Instant::now();
        let lateness = now.saturating_duration_since(next);
        maximum_lateness = maximum_lateness.max(lateness);
        if lateness > maximum_catch_up {
            // Do not repay an arbitrary scheduler pause as a line-rate burst.
            // One datagram is sent now and the next deadline starts one exact
            // packet interval later.
            next = now;
            deadline_resets = deadline_resets.saturating_add(1);
        } else {
            let catch_up = u32::try_from(lateness.as_nanos() / interval.as_nanos())
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            maximum_catch_up_datagrams = maximum_catch_up_datagrams.max(catch_up);
        }

        packet[..4].copy_from_slice(&i32::try_from(datagrams & i32::MAX as u64)?.to_be_bytes());
        let length = send_packet(&packet)?;
        if length != packet.len() {
            return Err(format!("short UDP send: {length}/{}", packet.len()).into());
        }
        bytes = bytes.saturating_add(length as u64);
        datagrams = datagrams.saturating_add(1);
        next += interval;
    }

    let elapsed = started.elapsed();
    send_terminal_markers_with(&mut send_packet)?;
    Ok(HostTransmission {
        source,
        bytes,
        datagrams,
        elapsed,
        maximum_lateness,
        maximum_catch_up_datagrams,
        deadline_resets,
    })
}

fn send_terminal_markers_with(
    send_packet: &mut impl FnMut(&[u8]) -> std::io::Result<usize>,
) -> Result<()> {
    let marker = (-1_i32).to_be_bytes();
    for index in 0..TERMINAL_MARKERS {
        if index != 0 {
            thread::sleep(TERMINAL_MARKER_SPACING);
        }
        let length = send_packet(&marker)?;
        if length != marker.len() {
            return Err(format!("short UDP terminal send: {length}/{}", marker.len()).into());
        }
    }
    Ok(())
}

#[cfg(test)]
fn send_terminal_markers(socket: &UdpSocket) -> Result<()> {
    send_terminal_markers_with(&mut |packet| socket.send(packet))
}

fn packet_interval(payload: usize, rate_bps: u64) -> Result<Duration> {
    Ok(Duration::from_nanos(
        u64::try_from((payload as u128 * 8 * 1_000_000_000) / rate_bps as u128)?.max(1),
    ))
}

#[allow(
    clippy::disallowed_methods,
    reason = "host traffic pacing uses a bounded 30 us final spin outside production radio code"
)]
fn wait_until(deadline: Instant) -> Result<()> {
    loop {
        oer_process::check_cancelled()?;
        let now = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Ok(());
        };
        if remaining > FINAL_SPIN {
            oer_process::sleep(remaining - FINAL_SPIN)?;
        } else {
            hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests;
