//! Event-driven UDP collection, bounded by a correlated target result.

use super::{ActiveBurst, Burst};
use crate::{
    Result,
    transport::events::{EventPoll, deadline_after},
};
use std::{
    fs, io,
    net::{Ipv4Addr, UdpSocket},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Only missing packets wait this long after Finished. Complete delivery exits
/// immediately; this deadline never attests stack or radio drain completion.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum Stop {
    Finished {
        expected: Option<u64>,
        deadline: Instant,
    },
    Aborted,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum End {
    Delivered,
    DeliveryDeadline,
    TargetUnavailable,
    SessionDeadline,
    Cancelled,
    ReceiveError,
    HostOverflow,
    Aborted,
}

pub(crate) struct Receiver {
    stop: Arc<Mutex<Option<Stop>>>,
    wake: Arc<mio::Waker>,
    worker: Option<thread::JoinHandle<Result<Vec<Burst>>>>,
}

impl Receiver {
    pub(crate) fn start(
        socket: &UdpSocket,
        target: Ipv4Addr,
        maximum_wait: Duration,
        output: &Path,
        label: &str,
    ) -> Result<Self> {
        fs::create_dir_all(output)?;
        let output = output.join(format!("{label}-reception.json"));
        let socket = socket.try_clone()?;
        let poll = EventPoll::new()?;
        poll.register(&socket, false)?;
        let wake = poll.waker();
        let stop = Arc::new(Mutex::new(None));
        let worker_stop = Arc::clone(&stop);
        let deadline = deadline_after(maximum_wait);
        let drops_before = crate::transport::udp::kernel_drops(&socket)?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let worker = thread::spawn(move || {
            let _ = ready_tx.send(());
            collect(
                socket,
                target,
                deadline,
                worker_stop,
                poll,
                output,
                drops_before,
            )
        });
        ready_rx
            .recv()
            .map_err(|_| "UDP collector stopped before readiness")?;
        Ok(Self {
            stop,
            wake,
            worker: Some(worker),
        })
    }

    /// Publish one terminal edge to all flow collectors before joining any of them.
    pub(crate) fn target_finished(&self, expected_datagrams: Option<u64>) {
        let mut stop = self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stop.is_none() {
            *stop = Some(Stop::Finished {
                expected: expected_datagrams,
                deadline: Instant::now() + DELIVERY_TIMEOUT,
            });
        }
        drop(stop);
        let _ = self.wake.wake();
    }

    pub(crate) fn finish(mut self, expected_datagrams: Option<u64>) -> Result<Vec<Burst>> {
        self.target_finished(expected_datagrams);
        self.join()
    }

    fn signal(&self, stop: Stop) {
        *self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stop);
        // The collector may already have stopped on an I/O error or deadline.
        let _ = self.wake.wake();
    }

    fn join(&mut self) -> Result<Vec<Burst>> {
        self.worker
            .take()
            .expect("collector joined once")
            .join()
            .map_err(|_| "UDP collector panicked")?
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.signal(Stop::Aborted);
            if let Err(error) = self.join() {
                eprintln!("UDP collector cleanup: {error}");
            }
        }
    }
}

fn collect(
    socket: UdpSocket,
    target: Ipv4Addr,
    session_deadline: Instant,
    stop: Arc<Mutex<Option<Stop>>>,
    mut poll: EventPoll,
    output: PathBuf,
    drops_before: Option<u32>,
) -> Result<Vec<Burst>> {
    let mut packet = [0_u8; 2048];
    let mut active: Option<ActiveBurst> = None;
    let started = Instant::now();
    let mut expected = None;
    let mut error = None;
    let mut end = 'collect: loop {
        if let Err(cause) = oer_process::check_cancelled() {
            error = Some(cause);
            break End::Cancelled;
        }
        let mut drained = false;
        {
            // Bound CPU residence while preserving edge-triggered readiness:
            // if the budget expires, resume draining without another poll wait.
            for _ in 0..64 {
                match receive(&socket, &mut packet) {
                    Ok((length, source)) if source == target && length >= 4 => {
                        // A retried preflight response may arrive after Start. It is
                        // control traffic and never part of delivery accounting.
                        if open_esp_radio_hil_protocol::UdpProbe::decode(&packet[..length])
                            .is_some()
                        {
                            continue;
                        }
                        let sequence =
                            u32::from_be_bytes(packet[..4].try_into().expect("four-byte sequence"));
                        let now = Instant::now();
                        match &mut active {
                            Some(active) => active.push(sequence, length, now),
                            None => active = Some(ActiveBurst::new(sequence, length, now)),
                        }
                    }
                    Ok(_) => {}
                    Err(cause) if cause.kind() == io::ErrorKind::WouldBlock => {
                        drained = true;
                        break;
                    }
                    Err(cause) if cause.kind() == io::ErrorKind::Interrupted => {}
                    Err(cause) => {
                        error = Some(cause.into());
                        break 'collect End::ReceiveError;
                    }
                }
            }
        }
        let command = *stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = match command {
            Some(Stop::Aborted) => break End::Aborted,
            Some(Stop::Finished { expected: None, .. }) => break End::TargetUnavailable,
            Some(Stop::Finished {
                expected: Some(count),
                deadline,
            }) => {
                expected = Some(count);
                let unique = active
                    .as_ref()
                    .map_or(0, |active| active.seen_sequences.len() as u64);
                if unique >= count {
                    break End::Delivered;
                }
                if Instant::now() >= deadline {
                    break End::DeliveryDeadline;
                }
                deadline
            }
            None => {
                if Instant::now() >= session_deadline {
                    error = Some("UDP collection deadline expired before target Finished".into());
                    break End::SessionDeadline;
                }
                session_deadline
            }
        };
        if !drained {
            continue;
        }
        if let Err(cause) = poll.wait(Some(deadline)) {
            error = Some(cause.into());
            break End::ReceiveError;
        }
        // Check the socket on both I/O and control edges. No shared descriptor
        // flags change, so the concurrent sender keeps its blocking contract.
    };
    let unique = active
        .as_ref()
        .map_or(0, |active| active.seen_sequences.len() as u64);
    let bursts: Vec<_> = active.into_iter().map(ActiveBurst::finish).collect();
    let kernel_drops = match crate::transport::udp::kernel_drops(&socket) {
        Ok(after) => after
            .zip(drops_before)
            .map(|(after, before)| after.wrapping_sub(before)),
        Err(cause) => {
            error = Some(cause.into());
            None
        }
    };
    if let Some(dropped) = kernel_drops
        && dropped != 0
    {
        end = End::HostOverflow;
        error = Some(io::Error::other(format!("host UDP socket dropped {dropped} datagrams in the kernel; delivery is not a valid radio-only measurement")).into());
    }
    fs::write(
        output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 2,
            "host_kernel_drops": kernel_drops,
            "completion": end,
            "expected_datagrams": expected,
            "received_unique_datagrams": unique,
            "undelivered_datagrams": expected.map(|count| count.saturating_sub(unique)),
            "elapsed_micros": started.elapsed().as_micros(),
            "bursts": bursts,
            "error": error.as_ref().map(|error| error.to_string()),
        }))?,
    )?;
    match error {
        Some(error) => Err(error),
        None => Ok(bursts),
    }
}

/// MSG_DONTWAIT is local to this receive, unlike O_NONBLOCK on a cloned socket.
fn receive(socket: &UdpSocket, packet: &mut [u8]) -> io::Result<(usize, Ipv4Addr)> {
    // SAFETY: sockaddr_storage is valid zero-initialized output storage. The
    // kernel receives exact live buffer lengths and owns neither pointer.
    let mut source: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&source) as libc::socklen_t;
    let received = unsafe {
        libc::recvfrom(
            socket.as_raw_fd(),
            packet.as_mut_ptr().cast(),
            packet.len(),
            libc::MSG_DONTWAIT,
            (&raw mut source).cast(),
            &raw mut length,
        )
    };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if i32::from(source.ss_family) != libc::AF_INET
        || (length as usize) < std::mem::size_of::<libc::sockaddr_in>()
    {
        return Err(io::Error::other("UDP collector requires an IPv4 peer"));
    }
    // SAFETY: family and initialized length were checked; unaligned read does
    // not borrow the storage through a differently aligned reference.
    let source = unsafe {
        (&raw const source)
            .cast::<libc::sockaddr_in>()
            .read_unaligned()
    };
    Ok((
        received as usize,
        Ipv4Addr::from(source.sin_addr.s_addr.to_ne_bytes()),
    ))
}

#[cfg(test)]
mod tests;
