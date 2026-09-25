//! A fake serial link to the target for session and workload tests.
use super::*;
use oer_hil_protocol::{Capabilities, FeatureCapabilities};
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

/// A temporary capture output directory, removed on drop.
pub struct Output(pub PathBuf);

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Output {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oer-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(crate) struct Serial {
    pub(crate) input: TestRead,
    pub(crate) fail_write: bool,
    pub(crate) writes: Option<mpsc::Sender<Vec<u8>>>,
}

pub(crate) struct TestRead {
    stream: std::os::unix::net::UnixStream,
    failure: Arc<Mutex<Option<io::Error>>>,
}
/// The target side of a fake serial link: send frames or a read failure.
#[derive(Clone)]
pub struct Input {
    stream: Arc<std::os::unix::net::UnixStream>,
    failure: Arc<Mutex<Option<io::Error>>>,
}
impl Input {
    pub fn send(&self, input: io::Result<Vec<u8>>) -> io::Result<()> {
        match input {
            Ok(bytes) => (&*self.stream).write_all(&bytes),
            Err(error) => {
                *self.failure.lock().unwrap() = Some(error);
                self.stream.shutdown(std::net::Shutdown::Write)
            }
        }
    }
}
pub(crate) fn serial_pair() -> (Input, TestRead) {
    let (host, peer) = std::os::unix::net::UnixStream::pair().unwrap();
    host.set_nonblocking(true).unwrap();
    let failure = Arc::new(Mutex::new(None));
    (
        Input {
            stream: Arc::new(peer),
            failure: Arc::clone(&failure),
        },
        TestRead {
            stream: host,
            failure,
        },
    )
}
impl AsRawFd for Serial {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.input.stream.as_raw_fd()
    }
}
impl Read for Serial {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.input.stream.read(buffer) {
            Ok(0) => self.input.failure.lock().unwrap().take().map_or(Ok(0), Err),
            result => result,
        }
    }
}

impl Write for Serial {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            if let Some(writes) = &self.writes {
                writes
                    .send(bytes.to_vec())
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
            }
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A capture over a fake serial link whose writes succeed or fail.
pub fn capture(output: &Output, fail_write: bool) -> (SerialCapture, Input) {
    let (input, rx) = serial_pair();
    let capture = SerialCapture::start_transport(&output.0, move || {
        Ok(Serial {
            input: rx,
            fail_write,
            writes: None,
        })
    })
    .unwrap();
    (capture, input)
}

/// Encode one target event as a wire frame.
pub fn frame(event: Envelope<Event>) -> Vec<u8> {
    FrameEncoder::new().encode(&event).unwrap().to_vec()
}

/// Deliver the boot `Hello` and wait until the capture observes it.
pub fn activate(capture: &SerialCapture, input: &Input) {
    input.send(Ok(frame(hello(7, 0)))).unwrap();
    capture
        .wait_for_protocol_after(0, Duration::from_secs(2), |message| {
            matches!(message.body, Event::Hello(_))
        })
        .unwrap()
        .unwrap();
}

/// A capture whose host commands are delivered to the returned receiver.
pub fn capture_with_commands(output: &Output) -> (SerialCapture, Input, mpsc::Receiver<Vec<u8>>) {
    let (input, rx) = serial_pair();
    let (writes, commands) = mpsc::channel();
    let capture = SerialCapture::start_transport(&output.0, move || {
        Ok(Serial {
            input: rx,
            fail_write: false,
            writes: Some(writes),
        })
    })
    .unwrap();
    (capture, input, commands)
}

/// Decode the next host command written to a [`capture_with_commands`] link.
pub fn receive_command(writes: &mpsc::Receiver<Vec<u8>>) -> Envelope<Command> {
    let bytes = writes.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut command = None;
    FrameDecoder::new().feed::<Command>(&bytes, |decoded| command = Some(decoded.unwrap()));
    command.unwrap()
}

/// A target `Hello` with the default HIL capabilities.
pub fn hello(boot_id: u64, message_sequence: u32) -> Envelope<Event> {
    Envelope::new(
        boot_id,
        message_sequence,
        0,
        0,
        Event::Hello(Capabilities {
            features: FeatureCapabilities {
                bluetooth_secure_gatt: false,
                bluetooth_gatt: false,
                phy_fault_injection: false,
                bluetooth_dtm: false,
                bluetooth_peripheral: false,
                bluetooth_phy_maintenance: false,
                bluetooth_watchdog_reset: false,
                system_watchdog: false,
                udp: true,
                tcp: true,
                rx: true,
                tx: true,
                bidirectional: true,
                runtime_initialization: true,
                runtime_configuration: true,
                structured_evidence: true,
                udp_multi_flow: false,
                startup_artifact: true,
                station_epoch_control: true,
                station_pause: true,
                wifi_role_control: true,
                wifi_access_point: true,
                simultaneous_station_access_point: true,
                wifi_monitor_capture: true,
                station_lifecycle_events: true,
                driver_observation_evidence: true,
                rx_delivery_evidence: true,
                rx_ownership_evidence: false,
                phy_rx_hot_sram: false,
                task_poll_evidence: false,
                tx_architecture_probe: false,
                core0_rx_cycle_evidence: false,
                mac_irq_evidence: false,
                psram_task_stack: false,
                network_scheduler_evidence: false,
                data_plane_placement: true,
                timebase_probe: true,
                memory_benchmark: false,
                ieee802154_event_status_probe: false,
                ieee802154_ed_event_probe: false,
            },
            maximum_payload_bytes: 1,
            maximum_wire_frame_bytes: 1,
        }),
    )
}
