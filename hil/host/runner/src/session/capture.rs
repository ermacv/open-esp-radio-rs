//! Serial I/O and evidence persistence. Protocol operations live in `protocol`.

use super::protocol::decode_counters_are_clean;
use super::*;

impl SerialCapture {
    pub(crate) fn record_into(
        mut self,
        recorder: crate::evidence::measurements::CaptureRecorder,
    ) -> Self {
        self.measurements = Some(recorder);
        self
    }
    /// Observe an already-running target. Do not clear input, toggle reset
    /// lines, upload artifacts or initialize the runtime.
    pub(crate) fn attach(port: &Path, output: &Path) -> Result<Self> {
        let port = port.to_owned();
        Self::start_transport_at(output, CaptureOrigin::Attachment, move || {
            open_serial_after_busy_release(&port)
                .map_err(|error| LinkError::transport(format!("serial attach failed: {error}")))
        })
    }

    /// Own the output before opening and resetting the target, so even setup
    /// failures and early returns leave a capture and a structured diagnosis.
    pub(crate) fn start_with_reset(port: &Path, output: &Path) -> Result<Self> {
        let port = port.to_owned();
        Self::start_transport(output, move || {
            let mut serial = open_serial_after_busy_release(&port).map_err(|error| {
                LinkError::transport(format!(
                    "serial open failed for {}: {error}",
                    port.display()
                ))
            })?;
            reset_usb_serial_jtag(&mut *serial).map_err(|error| {
                LinkError::transport(format!("serial target reset failed: {error}"))
            })?;
            Ok(serial)
        })
    }

    fn start_transport<T: Read + Write + 'static>(
        output: &Path,
        open: impl FnOnce() -> std::result::Result<T, LinkError> + Send + 'static,
    ) -> Result<Self> {
        Self::start_transport_at(output, CaptureOrigin::Boot, open)
    }

    fn start_transport_at<T: Read + Write + 'static>(
        output: &Path,
        origin: CaptureOrigin,
        open: impl FnOnce() -> std::result::Result<T, LinkError> + Send + 'static,
    ) -> Result<Self> {
        fs::create_dir_all(output)?;
        let raw = fs::File::create(output.join("uart.bin"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let protocol = Arc::new(ProtocolEvents::default());
        protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .health
            .origin = origin;
        let (outbound, outbound_rx) = mpsc::channel();
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let worker_protocol = Arc::clone(&protocol);
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut serial = open()?;
                capture_serial(
                    &mut serial,
                    raw,
                    &worker_stop,
                    &worker_bytes,
                    &worker_protocol,
                    outbound_rx,
                )
            }));
            let failure = match result {
                Ok(result) => result.err(),
                Err(_) => Some(LinkError::transport("serial worker panicked")),
            };
            worker_protocol.close(failure);
        });
        Ok(Self {
            stop,
            bytes,
            protocol,
            outbound,
            next_host_sequence: AtomicU32::new(1),
            next_session_id: AtomicU64::new(1),
            worker: Some(worker),
            output: output.to_owned(),
            persisted: false,
            measurements: None,
        })
    }

    /// Boot smoke precedes the typed radio runtime; its text marker is the
    /// contract for the relocated Embassy timer executing once.
    pub(crate) fn wait_for_boot_smoke(&self, timeout: Duration) -> Result<()> {
        const PASS: &[u8] = b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS";
        const PANIC: &[u8] = b"OPEN_RADIO_HIL runtime=PANIC";
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.check_link()?;
            let bytes = self
                .bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bytes
                .windows(PANIC.len())
                .any(|candidate| candidate == PANIC)
            {
                return Err("boot-smoke runtime panicked".into());
            }
            if bytes.windows(PASS.len()).any(|candidate| candidate == PASS) {
                return Ok(());
            }
            drop(bytes);
            thread::sleep(Duration::from_millis(10));
        }
        Err("boot-smoke timer completion was not observed".into())
    }

    pub(crate) fn finish_with<T>(self, result: Result<T>) -> Result<T> {
        Self::combine_result(result, self.finish())
    }

    pub(crate) fn finish_observation_with<T>(self, result: Result<T>) -> Result<T> {
        Self::combine_result(result, self.finish_capture(false))
    }

    fn combine_result<T>(result: Result<T>, finalization: Result<String>) -> Result<T> {
        match (result, finalization) {
            (Ok(value), Ok(_)) => Ok(value),
            (Err(primary), Ok(_)) => Err(primary),
            (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(finalization)) => Err(Box::new(error::FinalizationError {
                primary,
                finalization,
            })),
        }
    }

    /// Finish the exchange, then persist evidence before returning any link
    /// failure. `uart.bin` contains only received bytes; `uart.log` is a lossy
    /// text view. Host commands are never recorded (they may hold credentials).
    pub(crate) fn finish(self) -> Result<String> {
        self.finish_capture(true)
    }

    /// Observation includes counters from before this attachment. Persist them
    /// without interpreting historical target errors as a new scenario failure.
    fn finish_capture(mut self, qualify_target_health: bool) -> Result<String> {
        let active = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .health
            .active;
        let target_health = if active && self.check_link().is_ok() {
            Some(self.query_link_health(PROTOCOL_READY_TIMEOUT))
        } else {
            None
        };
        self.stop_and_join();
        let uart = self.persist(target_health.as_ref(), true)?;
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = &state.failure {
            return Err(error.clone().into());
        }
        if !decode_counters_are_clean(state.health.counters) {
            return Err(LinkError::protocol(format!(
                "host reported unhealthy HIL frame decoding: {:?}",
                state.health.counters,
            ))
            .into());
        }
        if let Some(result) = target_health {
            let health = result?;
            if qualify_target_health {
                protocol::validate_target_link_health(health)?;
            }
            if health.text_dropped != 0 || health.text_truncated != 0 {
                eprintln!(
                    "diagnostic_text_loss=dropped:{} truncated:{}",
                    health.text_dropped, health.text_truncated
                );
            }
        }
        Ok(uart)
    }

    pub(super) fn persist(
        &mut self,
        target_health: Option<&Result<LinkHealth>>,
        finalized: bool,
    ) -> Result<String> {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let uart = String::from_utf8_lossy(&bytes).into_owned();
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Return decoded observations to the repetition before fallible disk
        // writes. Even a capture storage failure must retain available values.
        let observations = self
            .measurements
            .as_ref()
            .map(|recorder| recorder.record(&state.messages, bytes.len() as u64));
        // Rewrite from memory as well: a failed raw-file write still gets a
        // final attempt to preserve every byte accepted by the serial reader.
        crate::evidence::run::atomic_write(&self.output.join("uart.bin"), &bytes)?;
        crate::evidence::run::atomic_write(&self.output.join("uart.log"), uart.as_bytes())?;
        let mut log = Vec::new();
        for message in &state.messages {
            serde_json::to_writer(
                &mut log,
                &serde_json::json!({"record": "target-event", "envelope": message}),
            )?;
            log.push(b'\n');
        }
        for record in [
            serde_json::json!({"record": "link-health", "health": state.health}),
            serde_json::json!({"record": "capture-end", "finalized": finalized, "cancelled": oer_process::cancellation_requested(), "failure": state.failure}),
            serde_json::json!({"record": "target-link-health", "health": target_health.map(|result|
                result.as_ref().map_err(|error| error.to_string()))}),
        ] {
            serde_json::to_writer(&mut log, &record)?;
            log.push(b'\n');
        }
        crate::evidence::run::atomic_write(&self.output.join("protocol.jsonl"), &log)?;
        if let Some(observations) = observations {
            crate::evidence::run::atomic_json(
                &self.output.join("measurements.json"),
                &serde_json::json!({
                    "schema": 1, "finalized": finalized, "failure": state.failure,
                    "measurements": observations,
                }),
            )?;
        }
        self.persisted = true;
        Ok(uart)
    }

    pub(super) fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            self.protocol
                .close(Some(LinkError::transport("serial worker panicked")));
        }
    }
}

fn capture_serial(
    serial: &mut (impl Read + Write),
    mut raw: fs::File,
    stop: &AtomicBool,
    bytes: &Mutex<Vec<u8>>,
    protocol: &ProtocolEvents,
    outbound: mpsc::Receiver<Zeroizing<Vec<u8>>>,
) -> std::result::Result<(), LinkError> {
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0_u8; 2_048];
    while !stop.load(Ordering::Acquire) {
        while let Ok(frame) = outbound.try_recv() {
            serial
                .write_all(&frame)
                .map_err(|error| LinkError::transport(format!("serial write failed: {error}")))?;
        }
        let length = match serial.read(&mut buffer) {
            Ok(0) => return Err(LinkError::transport("serial reader reached end of stream")),
            Ok(length) => length,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(LinkError::transport(format!("serial read failed: {error}"))),
        };
        let chunk = &buffer[..length];
        append(bytes, chunk);
        raw.write_all(chunk).map_err(|error| {
            LinkError::transport(format!("cannot persist raw serial bytes: {error}"))
        })?;
        let mut state = protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before_read = decoder.counters();
        decoder.feed::<Event>(chunk, |message| {
            match message {
                Ok(message) => {
                    if let Some(boot_id) = state.health.boot_id
                        && boot_id != message.boot_id
                    {
                        state.fail(LinkError::protocol(format!(
                            "target rebooted during capture: boot {boot_id} became {}",
                            message.boot_id,
                        )));
                    }
                    state.health.observe(&message, before_read);
                    if state.messages.len() < PROTOCOL_EVENT_CAPACITY {
                        state.messages.push(message);
                    } else {
                        state.health.fail(format!(
                            "host protocol event capacity {PROTOCOL_EVENT_CAPACITY} exhausted"
                        ));
                    }
                }
                Err(error) => {
                    // A recognizable incompatible header is actionable even
                    // before Hello. Arbitrary boot text is not a wire failure.
                    if matches!(
                        error,
                        open_esp_radio_hil_protocol::DecodeError::ProtocolVersion
                            | open_esp_radio_hil_protocol::DecodeError::FramingVersion
                    ) {
                        state.health.fail(error.to_string());
                    } else {
                        state.health.decode_error(error);
                    }
                }
            }
            if let Some(error) = state.health.failure.clone() {
                state.fail(LinkError::protocol(error));
            }
        });
        state.health.update_decoder_counters(decoder.counters());
        if state.health.active && !decode_counters_are_clean(state.health.counters) {
            let counters = state.health.counters;
            state.fail(LinkError::protocol(format!(
                "host HIL frame decoding failed: {counters:?}"
            )));
        }
        protocol.changed.notify_all();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
