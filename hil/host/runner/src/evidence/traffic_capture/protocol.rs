use super::*;

impl SerialCapture {
    /// Open the diagnostics owner before resetting the USB-Serial/JTAG target.
    ///
    /// Traffic qualification needs the DHCP and UDP-ready records from the
    /// current boot. Resetting through a second process after opening this
    /// handle is impossible because `serialport` owns the device exclusively.
    pub(crate) fn start_with_reset(port: &Path) -> Self {
        Self::start_inner(port, true)
    }

    /// Boot-smoke deliberately runs before the full radio/protocol runtime.
    /// Its only contract is that the relocated Embassy timer executes once.
    pub(crate) fn wait_for_boot_smoke(&self, timeout: Duration) -> Result<()> {
        const PASS: &[u8] = b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS";
        const PANIC: &[u8] = b"OPEN_RADIO_HIL runtime=PANIC";
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
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

    fn start_inner(port: &Path, reset_target: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let protocol = Arc::new(ProtocolEvents {
            messages: Mutex::new(Vec::new()),
            health: Mutex::new(ProtocolHealth::default()),
            changed: Condvar::new(),
        });
        let (outbound, outbound_rx) = mpsc::channel::<Zeroizing<Vec<u8>>>();
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let worker_protocol = Arc::clone(&protocol);
        let port = port.to_owned();
        let worker = thread::spawn(move || {
            let mut serial = match open_serial_after_busy_release(&port) {
                Ok(serial) => serial,
                Err(error) => {
                    append(
                        &worker_bytes,
                        format!("serial capture failed for {}: {error}\n", port.display())
                            .as_bytes(),
                    );
                    return;
                }
            };
            if reset_target {
                let _ = serial.clear(serialport::ClearBuffer::Input);
            }
            if reset_target && let Err(error) = reset_usb_serial_jtag(&mut *serial) {
                append(
                    &worker_bytes,
                    format!(
                        "serial target reset failed for {}: {error}\n",
                        port.display()
                    )
                    .as_bytes(),
                );
                return;
            }
            let mut decoder = FrameDecoder::new();
            let mut buffer = [0_u8; 2_048];
            while !worker_stop.load(Ordering::Acquire) {
                while let Ok(frame) = outbound_rx.try_recv() {
                    if let Err(error) = serial.write_all(frame.as_slice()) {
                        append(
                            &worker_bytes,
                            format!("\nserial write failed: {error}\n").as_bytes(),
                        );
                        return;
                    }
                }
                match serial.read(&mut buffer) {
                    Ok(length) => {
                        append(&worker_bytes, &buffer[..length]);
                        let decoder_counters_before_read = decoder.counters();
                        decoder.feed::<Event>(&buffer[..length], |message| match message {
                            Ok(message) => {
                                worker_protocol
                                    .health
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .observe(&message, decoder_counters_before_read);
                                let mut messages = worker_protocol
                                    .messages
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let capacity_exhausted = messages.len() == PROTOCOL_EVENT_CAPACITY;
                                if !capacity_exhausted {
                                    messages.push(message);
                                }
                                drop(messages);
                                if capacity_exhausted {
                                    worker_protocol
                                        .health
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .fail(format!(
                                            "host protocol event capacity {PROTOCOL_EVENT_CAPACITY} exhausted"
                                        ));
                                }
                                worker_protocol.changed.notify_all();
                            }
                            Err(error) => worker_protocol
                                .health
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .decode_error(error),
                        });
                        worker_protocol
                            .health
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .update_decoder_counters(decoder.counters());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(error) => {
                        append(
                            &worker_bytes,
                            format!("\nserial read failed: {error}\n").as_bytes(),
                        );
                        break;
                    }
                }
            }
        });
        Self {
            stop,
            bytes,
            protocol,
            outbound,
            next_host_sequence: AtomicU32::new(1),
            next_session_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    /// Performs one typed host-to-target round trip and returns the current
    /// image capabilities.
    pub(crate) fn request_capabilities(&self, timeout: Duration) -> Result<Capabilities> {
        let _hello = self
            .wait_for_protocol_after(0, timeout, |message| {
                matches!(message.body, Event::Hello(_))
            })
            .ok_or_else(|| {
                self.protocol_failure_or("device did not publish a HIL protocol hello")
            })?;
        let response = self.send_command(0, Command::GetCapabilities, timeout)?;
        match response.body {
            Event::Hello(capabilities) => Ok(capabilities),
            Event::Rejected(reason) => {
                Err(format!("device rejected HIL capability request: {reason:?}").into())
            }
            _ => Err("device returned an invalid HIL capability response".into()),
        }
    }

    /// Establishes the typed link and provisions this boot from host-owned
    /// local configuration. The passphrase is never echoed by the target or
    /// appended to the UART capture.
    pub(crate) fn prepare_protocol(&self, lab: &LabConfig) -> Result<Capabilities> {
        let capabilities = self.request_capabilities(PROTOCOL_READY_TIMEOUT)?;
        let artifact_path = lab.device.startup_artifact.as_deref();
        if artifact_path.is_some() && !capabilities.features.startup_artifact {
            return Err("firmware does not support a host-owned startup artifact".into());
        }
        if !capabilities.features.data_plane_placement {
            return Err("firmware does not support explicit data-plane placement".into());
        }
        let artifact_event_start = self.protocol_event_count();
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path
            && let Some(bytes) = reporting::startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.runtime_initialization {
            self.initialize(lab, PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path
        {
            let status = self.wait_for_startup_artifact_status_after(
                artifact_event_start,
                STARTUP_ARTIFACT_TIMEOUT,
            )?;
            let bytes = self
                .wait_for_startup_artifact_after(artifact_event_start, STARTUP_ARTIFACT_TIMEOUT)?;
            if usize::from(status.total_length) != bytes.len() {
                return Err(format!(
                    "startup artifact status length {} does not match {} returned bytes",
                    status.total_length,
                    bytes.len()
                )
                .into());
            }
            reporting::startup_artifact::persist_atomically(path, &bytes)?;
            eprintln!(
                "startup_artifact={} disposition={:?} bytes={} initialization_elapsed_us={}",
                path.display(),
                status.disposition,
                bytes.len(),
                status.initialization_elapsed_micros,
            );
        }
        Ok(capabilities)
    }

    fn wait_for_startup_artifact_status_after(
        &self,
        start: usize,
        timeout: Duration,
    ) -> Result<StartupArtifactStatus> {
        let event = self
            .wait_for_protocol_after(start, timeout, |message| {
                matches!(&message.body, Event::StartupArtifactReady(_))
            })
            .ok_or("device did not report startup artifact initialization status")?;
        match event.body {
            Event::StartupArtifactReady(status) => Ok(status),
            _ => unreachable!("startup artifact status predicate accepted only status events"),
        }
    }

    fn upload_startup_artifact(&self, bytes: &[u8], timeout: Duration) -> Result<()> {
        for chunk in reporting::startup_artifact::chunks(bytes)? {
            match self
                .send_command(0, Command::UploadStartupArtifact(chunk), timeout)?
                .body
            {
                Event::Accepted => {}
                Event::Rejected(reason) => {
                    return Err(format!("device rejected HIL startup artifact: {reason:?}").into());
                }
                _ => return Err("device returned an invalid startup artifact response".into()),
            }
        }
        Ok(())
    }

    fn wait_for_startup_artifact_after(&self, start: usize, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut cursor = start;
        let mut assembler = reporting::startup_artifact::Assembler::new();
        loop {
            let chunk = self
                .wait_for_startup_artifact_chunk(&mut cursor, deadline)
                .ok_or("device did not return its startup artifact")?;
            if let Some(bytes) = assembler.push(&chunk)? {
                return Ok(bytes);
            }
        }
    }

    fn wait_for_startup_artifact_chunk(
        &self,
        cursor: &mut usize,
        deadline: Instant,
    ) -> Option<StartupArtifactChunk> {
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some((relative, chunk)) = messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find_map(|(relative, message)| match &message.body {
                    Event::StartupArtifact(chunk) => Some((relative, chunk.clone())),
                    _ => None,
                })
            {
                *cursor += relative + 1;
                return Some(chunk);
            }
            *cursor = messages.len();
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return None;
            }
        }
    }

    fn initialize(&self, lab: &LabConfig, timeout: Duration) -> Result<()> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(
            0,
            Command::Initialize(open_esp_radio_hil_protocol::InitializationConfiguration {
                ipv4: lab.station.ipv4(),
                data_plane: lab.data_plane(),
                rx_checksum: lab.rx_checksum(),
                tx_udp_checksum: lab.tx_udp_checksum(),
                tx_buffer: lab.tx_buffer(),
                rx_admission: lab.rx_admission(),
                rx_dispatch: lab.rx_dispatch(),
                rx_continuation: lab.rx_continuation(),
                l1_cache_counters: lab.l1_cache_counters(),
            }),
            timeout,
        )?;
        let request_id = response.request_id;
        match response.body {
            Event::Initialized => return Ok(()),
            Event::Accepted
            | Event::State(StateChange {
                current: SessionState::Idle,
                ..
            }) => {}
            Event::Rejected(reason) => {
                return Err(format!("device rejected HIL initialization: {reason:?}").into());
            }
            _ => return Err("device returned an invalid initialization response".into()),
        }
        self.wait_for_protocol_after(first_event, timeout, |message| {
            message.request_id == request_id && matches!(message.body, Event::Initialized)
        })
        .ok_or_else(|| "device did not complete role-neutral initialization".into())
        .map(|_| ())
    }

    pub(crate) fn prepare_station(
        &self,
        lab: &LabConfig,
        timeout: Duration,
    ) -> Result<Capabilities> {
        let capabilities = self.prepare_protocol(lab)?;
        let lifecycle_cursor = self.station_lifecycle_cursor();
        let handle = self.request_station_start(lab)?;
        self.wait_wifi_role_transition(handle, timeout)?;
        self.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
        Ok(capabilities)
    }

    pub(crate) fn query_operation_status(&self, timeout: Duration) -> Result<OperationStatus> {
        match self.send_command(0, Command::GetStatus, timeout)?.body {
            Event::OperationStatus(status) => Ok(status),
            Event::Rejected(reason) => {
                Err(format!("device rejected operation-status query: {reason:?}").into())
            }
            _ => Err("device returned an invalid operation-status response".into()),
        }
    }

    fn send_command(
        &self,
        session_id: u64,
        body: Command,
        timeout: Duration,
    ) -> Result<Envelope<Event>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before command")?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let command = Envelope::new(boot_id, request_id, session_id, request_id, body);
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL command: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| "serial worker stopped before HIL command")?;
        self.wait_for_protocol_after(event_count, timeout, |message| {
            command_response_matches(message, boot_id, session_id, request_id)
        })
        .ok_or_else(|| self.protocol_failure_or("device did not answer HIL command"))
    }

    fn expect_accepted(
        &self,
        session_id: u64,
        command: Command,
        operation: &str,
        expected_state: SessionState,
    ) -> Result<()> {
        let response = self
            .send_command(session_id, command, PROTOCOL_READY_TIMEOUT)
            .map_err(|error| format!("session {operation} command failed: {error}"))?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::State(StateChange { current, .. }) if current == expected_state => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected session {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid session {operation} response").into()),
        }
    }

    pub(crate) fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let first_event = self.protocol_event_count();
        let direction = config.direction;
        let link_requirements = config.link_requirements;
        let flow_ids = config.flows.map(|flow| flow.map(|flow| flow.flow_id));
        self.expect_accepted(
            session_id,
            Command::Configure(config),
            "configuration",
            SessionState::Configured,
        )?;
        self.expect_accepted(session_id, Command::Arm, "arm", SessionState::Armed)?;
        self.expect_accepted(session_id, Command::Start, "start", SessionState::Running)?;
        let expected_directions: &[Direction] = match direction {
            Direction::Rx => &[Direction::Rx],
            Direction::Tx => &[Direction::Tx],
            Direction::Bidirectional => &[Direction::Rx, Direction::Tx],
        };
        for expected in expected_directions {
            self.wait_for_protocol_after(first_event, PROTOCOL_READY_TIMEOUT, |message| {
                message.session_id == session_id
                    && matches!(
                        message.body,
                        Event::SessionReady(reported)
                            if session_ready_covers(
                                direction,
                                reported,
                                *expected,
                                link_requirements,
                            )
                    )
            })
            .ok_or_else(|| {
                format!(
                    "device did not publish {expected:?} data-plane readiness for session \
                     {session_id}; required TX BlockAck TID: {:?}",
                    link_requirements.tx_block_ack_tid,
                )
            })?;
        }
        Ok(SessionHandle {
            session_id,
            first_event,
            flow_ids,
        })
    }

    pub(crate) fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        let deadline = Instant::now() + timeout;
        let evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Transport(_)))
            })
            .ok_or("device did not publish structured session evidence")?;
        let transport = match evidence.body {
            Event::Evidence(EvidenceRecord::Transport(transport)) => transport,
            _ => unreachable!("session evidence predicate accepted only transport evidence"),
        };
        let mut flow_transport = [None; SESSION_FLOW_CAPACITY];
        for (index, flow_id) in session.flow_ids.into_iter().enumerate() {
            let Some(flow_id) = flow_id else {
                continue;
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            let flow_evidence = self
                .wait_for_protocol_after(session.first_event, remaining, |message| {
                    message.session_id == session.session_id
                        && matches!(
                            message.body,
                            Event::Evidence(EvidenceRecord::FlowTransport(flow))
                                if flow.flow_id == flow_id
                        )
                })
                .ok_or_else(|| {
                    format!("device did not publish transport evidence for flow {flow_id}")
                })?;
            flow_transport[index] = Some(match flow_evidence.body {
                Event::Evidence(EvidenceRecord::FlowTransport(flow)) => flow,
                _ => unreachable!("flow predicate accepted only flow transport evidence"),
            });
        }
        let flow_total = TransportEvidence::from_flows(flow_transport);
        if flow_total != transport {
            return Err(format!(
                "per-flow transport total does not match session aggregate: flows={flow_total:?} aggregate={transport:?}"
            )
            .into());
        }
        let link_evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Link(_)))
            })
            .ok_or("device did not publish structured protocol-link evidence")?;
        let link = match link_evidence.body {
            Event::Evidence(EvidenceRecord::Link(link)) => link,
            _ => unreachable!("link evidence predicate accepted only link evidence"),
        };
        if link.rx_cobs_errors != 0
            || link.rx_checksum_errors != 0
            || link.rx_decode_errors != 0
            || link.rx_overflows != 0
            || link.tx_dropped != 0
        {
            return Err(format!("device protocol link is unhealthy: {link:?}").into());
        }
        let stack_evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Stack(_)))
            })
            .ok_or("device did not publish structured stack evidence")?;
        let stack = match stack_evidence.body {
            Event::Evidence(EvidenceRecord::Stack(stack)) => stack,
            _ => unreachable!("stack evidence predicate accepted only stack evidence"),
        };
        validate_stack_usage(stack)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let finished = self
            .wait_for_protocol_after(session.first_event, remaining, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Finished(_))
            })
            .ok_or("device did not finish the structured HIL session")?;
        let finished = match finished.body {
            Event::Finished(finished) => finished,
            _ => unreachable!("session completion predicate accepted only Finished"),
        };
        let radio = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Radio(_)))
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::Radio(radio)) => radio,
                _ => unreachable!("radio predicate accepted only radio evidence"),
            });
        let tx_timing = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::TxAggregateTiming(_))
                    )
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::TxAggregateTiming(timing)) => timing,
                _ => unreachable!("TX timing predicate accepted only aggregate timing evidence"),
            });
        let rx_delivery = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::RxDelivery(_)))
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::RxDelivery(delivery)) => delivery,
                _ => unreachable!("RX delivery predicate accepted only delivery evidence"),
            });
        let network_scheduler = self
            .wait_for_protocol_after(session.first_event, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::NetworkScheduler(_))
                    )
            })
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::NetworkScheduler(evidence)) => evidence,
                _ => unreachable!("scheduler predicate accepted only scheduler evidence"),
            });
        let expected_records = 3
            + u16::try_from(flow_transport.iter().flatten().count())?
            + u16::from(radio.is_some())
            + u16::from(tx_timing.is_some())
            + u16::from(rx_delivery.is_some())
            + u16::from(network_scheduler.is_some());
        if finished.summary.evidence_records != expected_records {
            return Err(format!(
                "device reported {} evidence records but published {expected_records} typed records",
                finished.summary.evidence_records
            )
            .into());
        }
        let mut records = Vec::with_capacity(usize::from(finished.summary.evidence_records));
        records.push(EvidenceRecord::Transport(transport));
        for flow in flow_transport.iter().flatten().copied() {
            records.push(EvidenceRecord::FlowTransport(flow));
        }
        if let Some(radio) = radio {
            records.push(EvidenceRecord::Radio(radio));
        }
        if let Some(timing) = tx_timing {
            records.push(EvidenceRecord::TxAggregateTiming(timing));
        }
        if let Some(delivery) = rx_delivery {
            records.push(EvidenceRecord::RxDelivery(delivery));
        }
        if let Some(scheduler) = network_scheduler {
            records.push(EvidenceRecord::NetworkScheduler(scheduler));
        }
        records.push(EvidenceRecord::Link(link));
        records.push(EvidenceRecord::Stack(stack));
        let checksum = evidence_crc32c(&records)
            .map_err(|error| format!("cannot checksum structured HIL evidence: {error}"))?;
        if checksum != finished.evidence_crc32c {
            return Err(format!(
                "structured HIL evidence checksum mismatch: host={checksum:#010x} device={:#010x}",
                finished.evidence_crc32c
            )
            .into());
        }
        let session_evidence = SessionEvidence {
            transport,
            flow_transport,
            radio,
            tx_timing,
            rx_delivery,
            network_scheduler,
            stack,
            finished,
        };
        if session_evidence.flow_transport.iter().flatten().count()
            != session.flow_ids.iter().flatten().count()
        {
            return Err("structured session lost configured flow evidence".into());
        }
        Ok(session_evidence)
    }

    pub(crate) fn acknowledge_session(&self, session: SessionHandle) -> Result<()> {
        self.expect_accepted(
            session.session_id,
            Command::AcknowledgeResult,
            "acknowledgement",
            SessionState::Idle,
        )
    }

    pub(crate) fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(0, Command::CycleStationEpoch, PROTOCOL_READY_TIMEOUT)?;
        match response.body {
            Event::Accepted => Ok(StationEpochHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected station epoch cycle: {reason:?}").into())
            }
            _ => Err("device returned an invalid station epoch cycle response".into()),
        }
    }

    pub(crate) fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages
            .get(handle.first_event..)
            .unwrap_or_default()
            .iter()
            .find_map(|message| {
                if message.request_id != handle.request_id {
                    return None;
                }
                match message.body {
                    Event::StationEpochCompleted(evidence) => Some(evidence),
                    _ => None,
                }
            })
    }

    fn request_wifi_command(&self, command: Command, operation: &str) -> Result<WifiCommandHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(0, command, PROTOCOL_READY_TIMEOUT)?;
        match response.body {
            Event::Accepted => Ok(WifiCommandHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid {operation} response").into()),
        }
    }

    pub(crate) fn request_station_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopStation, "station stop")
    }

    pub(crate) fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage> {
        let response = self.send_command(0, Command::QueryStackUsage, timeout)?;
        match response.body {
            Event::StackUsage(usage) => {
                validate_stack_usage(usage)?;
                Ok(usage)
            }
            Event::Rejected(reason) => {
                Err(format!("device rejected stack-usage query: {reason:?}").into())
            }
            _ => Err("device returned an invalid stack-usage response".into()),
        }
    }

    pub(crate) fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        let response = self.send_command(0, Command::QueryLinkHealth, timeout)?;
        match response.body {
            Event::LinkHealth(health) => Ok(health),
            Event::Rejected(reason) => {
                Err(format!("device rejected link-health query: {reason:?}").into())
            }
            _ => Err("device returned an invalid link-health response".into()),
        }
    }

    pub(crate) fn probe_timebase(
        &self,
        request: TimebaseProbeRequest,
        timeout: Duration,
    ) -> Result<TimebaseProbeEvidence> {
        let response = self.send_command(0, Command::ProbeTimebase(request), timeout)?;
        match response.body {
            Event::TimebaseProbeCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected timebase probe: {reason:?}").into())
            }
            _ => Err("device returned an invalid timebase-probe response".into()),
        }
    }

    pub(crate) fn probe_ieee802154_event_status(
        &self,
        request: Ieee802154EventStatusProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154EventStatusProbeEvidence> {
        // `send_command` admits only an envelope from this boot, session and
        // request ID; this match then admits only the probe's typed event.
        let response =
            self.send_command(0, Command::ProbeIeee802154EventStatus(request), timeout)?;
        match response.body {
            Event::Ieee802154EventStatusProbeCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 EVENT_STATUS probe: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 EVENT_STATUS probe response".into()),
        }
    }

    pub(crate) fn probe_ieee802154_ed_event(
        &self,
        request: Ieee802154EdEventProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154EdEventProbeEvidence> {
        let response = self.send_command(0, Command::ProbeIeee802154EdEvent(request), timeout)?;
        match response.body {
            Event::Ieee802154EdEventProbeCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 ED event probe: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 ED event probe response".into()),
        }
    }

    pub(crate) fn request_station_start(&self, lab: &LabConfig) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            Command::StartStation(lab.station.protocol_credentials()?),
            "station start",
        )
    }

    pub(crate) fn request_wifi_scan(&self, request: WifiScanRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::ScanWifi(request), "standalone Wi-Fi scan")
    }

    pub(crate) fn request_monitor_start(
        &self,
        request: WifiMonitorRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartMonitor(request), "monitor start")
    }

    pub(crate) fn request_monitor_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopMonitor, "monitor stop")
    }

    pub(crate) fn request_access_point_start(
        &self,
        request: open_esp_radio_hil_protocol::WifiAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartAccessPoint(request), "access-point start")
    }

    pub(crate) fn request_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopAccessPoint, "access-point stop")
    }

    pub(crate) fn request_station_access_point_start(
        &self,
        request: open_esp_radio_hil_protocol::WifiStationAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            Command::StartStationAccessPoint(request),
            "station-access-point start",
        )
    }

    pub(crate) fn request_station_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopStationAccessPoint, "station-access-point stop")
    }

    pub(crate) fn request_monitor_capture(
        &self,
        request: WifiMonitorCaptureRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::CaptureMonitor(request), "finite monitor capture")
    }

    pub(crate) fn wait_wifi_role_transition(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiRoleTransitioned(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the Wi-Fi role transition")?;
        match event.body {
            Event::WifiRoleTransitioned(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("Wi-Fi role transition failed: {failure:?}").into())
            }
            _ => unreachable!("role-transition predicate accepted only terminal role events"),
        }
    }

    pub(crate) fn wait_wifi_scan(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiScanEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiScanCompleted(_))
            })
            .ok_or("device did not complete the standalone Wi-Fi scan")?;
        match event.body {
            Event::WifiScanCompleted(evidence) => Ok(evidence),
            _ => unreachable!("scan predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_monitor_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStarted(_))
            })
            .ok_or("device did not complete monitor start")?;
        match event.body {
            Event::WifiMonitorStarted(evidence) => Ok(evidence),
            _ => unreachable!("monitor-start predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_access_point_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStarted(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the access-point start")?;
        match event.body {
            Event::WifiAccessPointStarted(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point start failed: {failure:?}").into())
            }
            _ => unreachable!("AP-start predicate accepted only terminal AP events"),
        }
    }

    pub(crate) fn wait_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<open_esp_radio_hil_protocol::WifiAccessPointEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStopped(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the access-point stop")?;
        match event.body {
            Event::WifiAccessPointStopped(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point stop failed: {failure:?}").into())
            }
            _ => unreachable!("AP-stop predicate accepted only terminal AP events"),
        }
    }

    pub(crate) fn wait_station_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<open_esp_radio_hil_protocol::WifiStationAccessPointStopEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiStationAccessPointStopped(_) | Event::WifiRoleFailed(_)
                    )
            })
            .ok_or("device did not complete the station-access-point stop")?;
        match event.body {
            Event::WifiStationAccessPointStopped(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("station-access-point stop failed: {failure:?}").into())
            }
            _ => unreachable!("paired-stop predicate accepted only terminal paired events"),
        }
    }

    pub(crate) fn wait_monitor_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiMonitorEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStopped(_))
            })
            .ok_or("device did not complete monitor stop")?;
        match event.body {
            Event::WifiMonitorStopped(evidence) => Ok(evidence),
            _ => unreachable!("monitor-stop predicate accepted only its completion event"),
        }
    }

    pub(crate) fn wait_monitor_capture(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence> {
        let completion = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorCaptureCompleted(_))
            })
            .ok_or("device did not complete finite monitor capture")?;
        let summary = match completion.body {
            Event::WifiMonitorCaptureCompleted(evidence) => evidence,
            _ => unreachable!("capture predicate accepted only terminal capture evidence"),
        };
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let chunks = messages
            .get(handle.first_event..)
            .unwrap_or_default()
            .iter()
            .filter_map(|message| {
                if message.request_id != handle.request_id {
                    return None;
                }
                match &message.body {
                    Event::WifiMonitorFrame(chunk) => Some(chunk.clone()),
                    _ => None,
                }
            })
            .collect();
        Ok(MonitorCaptureEvidence { chunks, summary })
    }

    pub(crate) fn wait_for_connected_station(&self, timeout: Duration) -> Result<u32> {
        self.wait_for_connected_station_after(0, timeout)
    }

    /// Wait for a connected edge published after a caller-owned event cursor.
    ///
    /// Reusing the first connected event of the current boot is incorrect for
    /// role roundtrips: the next AP epoch would then start while the preceding
    /// station restart was still scanning. Callers that initiate a new station
    /// epoch must snapshot [`Self::station_lifecycle_cursor`] before the start
    /// command and use this method.
    pub(crate) fn wait_for_connected_station_after(
        &self,
        first_event: usize,
        timeout: Duration,
    ) -> Result<u32> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let event = self
            .wait_for_protocol_after(first_event, timeout, |message| {
                message.boot_id == boot_id
                    && matches!(
                        message.body,
                        Event::StationLifecycle(StationLifecycleEvent::Connected { .. })
                    )
            })
            .ok_or("device did not publish connected station readiness")?;
        match event.body {
            Event::StationLifecycle(StationLifecycleEvent::Connected { generation }) => {
                Ok(generation)
            }
            _ => unreachable!("connected predicate accepted only a connected lifecycle event"),
        }
    }

    /// Cursor for reliable unsolicited station lifecycle events.
    pub(crate) fn station_lifecycle_cursor(&self) -> usize {
        self.protocol_event_count()
    }

    /// Wait for the next station lifecycle event and advance past it.
    pub(crate) fn wait_station_lifecycle_event(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<StationLifecycleEvent> {
        self.wait_station_lifecycle_event_optional(cursor, timeout)?
            .ok_or_else(|| "device did not publish the next station lifecycle event".into())
    }

    pub(crate) fn wait_station_lifecycle_event_optional(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<Option<StationLifecycleEvent>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(event) = next_station_lifecycle_event(&messages, cursor, boot_id) {
                return Ok(Some(event));
            }
            *cursor = messages.len();
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return Ok(None);
            }
        }
    }

    pub(super) fn latest_boot_id(&self) -> Option<u64> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        latest_boot_id_in(&messages)
    }

    pub(crate) fn observed_protocol_ipv4(
        &self,
        network_interface: WifiNetworkInterface,
    ) -> Option<Ipv4Addr> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let boot_id = latest_boot_id_in(&messages)?;
        messages
            .iter()
            .rev()
            .find_map(|message| match message.body {
                Event::NetworkReady(network)
                    if message.boot_id == boot_id
                        && network.network_interface == network_interface =>
                {
                    Some(Ipv4Addr::from(network.address))
                }
                _ => None,
            })
    }

    pub(crate) fn beacon_loss_count(&self) -> usize {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        beacon_loss_count_in(&messages)
    }

    pub(crate) fn require_no_beacon_loss(&self) -> Result<()> {
        let count = self.beacon_loss_count();
        if count == 0 {
            Ok(())
        } else {
            Err(format!("observed {count} typed station beacon-loss event(s)").into())
        }
    }

    pub(super) fn observed_udp_service(
        &self,
        network_interface: WifiNetworkInterface,
        direction: Direction,
        port: u16,
    ) -> bool {
        self.observed_service(network_interface, Transport::Udp, direction, port)
    }

    pub(super) fn observed_service(
        &self,
        network_interface: WifiNetworkInterface,
        transport: Transport,
        direction: Direction,
        port: u16,
    ) -> bool {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(boot_id) = latest_boot_id_in(&messages) else {
            return false;
        };
        messages.iter().any(|message| match message.body {
            Event::ServiceReady(service) => {
                message.boot_id == boot_id
                    && service.network_interface == network_interface
                    && service.transport == transport
                    && service.direction == direction
                    && service.local_port == port
            }
            _ => false,
        })
    }

    pub(super) fn protocol_event_count(&self) -> usize {
        self.protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn protocol_failure_or(&self, fallback: &str) -> Box<dyn std::error::Error> {
        self.protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .clone()
            .unwrap_or_else(|| fallback.to_owned())
            .into()
    }

    pub(super) fn wait_for_protocol_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Option<Envelope<Event>> {
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(message) = messages
                .get(start..)
                .unwrap_or_default()
                .iter()
                .find(|message| predicate(message))
            {
                return Some(message.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return None;
            }
        }
    }

    /// Stops capture and persists both the byte-exact diagnostic transcript
    /// and the decoded target event stream. Host commands are deliberately not
    /// logged because station/AP commands can contain credentials.
    pub(crate) fn finish_to(mut self, output: &Path) -> Result<String> {
        let protocol_active = self
            .protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active;
        let target_health = protocol_active
            .then(|| {
                self.query_link_health(PROTOCOL_READY_TIMEOUT)
                    .map_err(|error| error.to_string())
            })
            .transpose();
        self.stop_and_join();
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let uart = String::from_utf8_lossy(&bytes).into_owned();
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let health = self
            .protocol
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        fs::create_dir_all(output)?;
        fs::write(output.join("uart.log"), &uart)?;
        let mut protocol_log = Vec::new();
        for message in messages.iter() {
            serde_json::to_writer(
                &mut protocol_log,
                &serde_json::json!({"record": "target-event", "envelope": message}),
            )?;
            protocol_log.push(b'\n');
        }
        serde_json::to_writer(
            &mut protocol_log,
            &serde_json::json!({"record": "link-health", "health": &health}),
        )?;
        protocol_log.push(b'\n');
        serde_json::to_writer(
            &mut protocol_log,
            &serde_json::json!({"record": "target-link-health", "health": &target_health}),
        )?;
        protocol_log.push(b'\n');
        fs::write(output.join("protocol.jsonl"), protocol_log)?;
        if !decode_counters_are_clean(health.counters) {
            return Err(format!(
                "host reported unhealthy HIL frame decoding: {:?}",
                health.counters
            )
            .into());
        }
        if let Some(failure) = health.failure {
            return Err(format!("HIL protocol health failed: {failure}").into());
        }
        if protocol_active {
            let target_health = target_health
                .map_err(|error| format!("target link-health query failed: {error}"))?
                .ok_or("target link-health query returned no result")?;
            validate_target_link_health(target_health)?;
            if target_health.text_dropped != 0 || target_health.text_truncated != 0 {
                eprintln!(
                    "diagnostic_text_loss=dropped:{} truncated:{}",
                    target_health.text_dropped, target_health.text_truncated,
                );
            }
        }
        Ok(uart)
    }

    pub(super) fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn latest_boot_id_in(messages: &[Envelope<Event>]) -> Option<u64> {
    messages
        .iter()
        .rev()
        .find_map(|message| matches!(message.body, Event::Hello(_)).then_some(message.boot_id))
}

pub(super) fn beacon_loss_count_in(messages: &[Envelope<Event>]) -> usize {
    let Some(boot_id) = latest_boot_id_in(messages) else {
        return 0;
    };
    messages
        .iter()
        .filter(|message| {
            message.boot_id == boot_id
                && matches!(
                    message.body,
                    Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                        reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
                        ..
                    })
                )
        })
        .count()
}

pub(super) fn next_station_lifecycle_event(
    messages: &[Envelope<Event>],
    cursor: &mut usize,
    boot_id: u64,
) -> Option<StationLifecycleEvent> {
    let (relative, event) = messages
        .get(*cursor..)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .find_map(|(relative, message)| {
            if message.boot_id != boot_id {
                return None;
            }
            match &message.body {
                Event::StationLifecycle(event) => Some((relative, *event)),
                _ => None,
            }
        })?;
    *cursor += relative + 1;
    Some(event)
}

fn decode_counters_are_clean(counters: DecodeCounters) -> bool {
    counters.cobs_errors == 0
        && counters.too_short == 0
        && counters.header_errors == 0
        && counters.framing_version_errors == 0
        && counters.message_kind_errors == 0
        && counters.protocol_version_errors == 0
        && counters.payload_length_errors == 0
        && counters.checksum_errors == 0
        && counters.deserialize_errors == 0
        && counters.overflows == 0
}

fn validate_target_link_health(health: LinkHealth) -> Result<()> {
    if health.rx_cobs_errors != 0
        || health.rx_checksum_errors != 0
        || health.rx_decode_errors != 0
        || health.rx_overflows != 0
        || health.tx_dropped != 0
    {
        return Err(
            format!("target reported unhealthy serialized console transport: {health:?}").into(),
        );
    }
    Ok(())
}
