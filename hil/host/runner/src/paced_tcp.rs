//! Bounded, paced host-to-target TCP stream producer for HIL qualification.

use std::{
    hint,
    io::Write as _,
    net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpStream},
    thread,
    time::{Duration, Instant},
};

use crate::Result;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const FINAL_SPIN: Duration = Duration::from_micros(30);
const MAX_CATCH_UP_INTERVALS: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    pub(crate) address: Ipv4Addr,
    pub(crate) port: u16,
    pub(crate) rate_bps: u64,
    pub(crate) duration: Duration,
    pub(crate) chunk_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HostTransmission {
    pub(crate) bytes: u64,
    pub(crate) writes: u64,
    pub(crate) elapsed: Duration,
    pub(crate) maximum_lateness: Duration,
    pub(crate) maximum_catch_up_writes: u32,
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
    let address = SocketAddrV4::new(config.address, config.port);
    let mut stream = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let chunk = vec![0x5a; config.chunk_bytes];
    let interval = chunk_interval(config.chunk_bytes, config.rate_bps)?;
    let maximum_catch_up = interval.saturating_mul(MAX_CATCH_UP_INTERVALS);
    let started = Instant::now();
    let deadline = started + config.duration;
    let mut next = started;
    let mut bytes = 0_u64;
    let mut writes = 0_u64;
    let mut maximum_lateness = Duration::ZERO;
    let mut maximum_catch_up_writes = 1_u32;
    let mut deadline_resets = 0_u64;

    while Instant::now() < deadline {
        wait_until(next);
        let now = Instant::now();
        let lateness = now.saturating_duration_since(next);
        maximum_lateness = maximum_lateness.max(lateness);
        if lateness > maximum_catch_up {
            next = now;
            deadline_resets = deadline_resets.saturating_add(1);
        } else {
            let catch_up = u32::try_from(lateness.as_nanos() / interval.as_nanos())
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            maximum_catch_up_writes = maximum_catch_up_writes.max(catch_up);
        }

        stream.write_all(&chunk)?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        writes = writes.saturating_add(1);
        next += interval;
    }

    let elapsed = started.elapsed();
    stream.shutdown(Shutdown::Write)?;
    Ok(HostTransmission {
        bytes,
        writes,
        elapsed,
        maximum_lateness,
        maximum_catch_up_writes,
        deadline_resets,
    })
}

fn chunk_interval(chunk_bytes: usize, rate_bps: u64) -> Result<Duration> {
    Ok(Duration::from_nanos(
        u64::try_from((chunk_bytes as u128 * 8 * 1_000_000_000) / rate_bps as u128)?.max(1),
    ))
}

fn wait_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return;
        };
        if remaining > FINAL_SPIN {
            thread::sleep(remaining - FINAL_SPIN);
        } else {
            hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_interval_matches_requested_rate() {
        assert_eq!(
            chunk_interval(14_600, 80_000_000).unwrap(),
            Duration::from_nanos(1_460_000)
        );
    }
}
