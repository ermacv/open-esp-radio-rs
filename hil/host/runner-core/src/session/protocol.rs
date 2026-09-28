use super::*;

#[derive(serde::Serialize)]
pub struct Observation {
    boot_id: u64,
    capabilities: Capabilities,
    operation: OperationStatus,
    /// None means the current target state cannot safely snapshot the stacks.
    stack: Option<StackUsage>,
    link: LinkHealth,
}

impl SerialCapture {
    pub fn observe(&self, timeout: Duration) -> Result<Observation> {
        let capabilities = self.discover(timeout)?;
        let boot_id = self
            .latest_boot_id()
            .ok_or("discovery omitted the target boot identity")?;
        let operation = self.query_operation_status(timeout)?;
        let stack = self.inspect_stack_usage(timeout)?;
        let link = self.query_link_health(timeout)?;
        Ok(Observation {
            boot_id,
            capabilities,
            operation,
            stack,
            link,
        })
    }

    pub fn discover(&self, timeout: Duration) -> Result<Capabilities> {
        let response = self.exchange(0, 0, Command::GetCapabilities, timeout)?;
        match response.body {
            Event::Hello(capabilities) if response.boot_id != 0 => Ok(capabilities),
            Event::Rejected(reason) => Err(format!("device rejected read-only discovery: {reason:?}; firmware must support boot discovery").into()),
            _ => Err("device returned an invalid boot discovery response".into()),
        }
    }

    pub fn inspect_stack_usage(&self, timeout: Duration) -> Result<Option<StackUsage>> {
        match self
            .send_command(0, Command::QueryStackUsage, timeout)?
            .body
        {
            Event::StackUsage(stack) => Ok(Some(stack)),
            Event::Rejected(oer_hil_protocol::RejectReason::InvalidState) => Ok(None),
            Event::Rejected(reason) => {
                Err(format!("device rejected stack observation: {reason:?}").into())
            }
            _ => Err("device returned an invalid stack observation".into()),
        }
    }

    /// Performs one typed host-to-target round trip and returns the current
    /// image capabilities.
    pub fn request_capabilities(&self, timeout: Duration) -> Result<Capabilities> {
        let _hello = self
            .wait_for_protocol_after(0, timeout, |message| {
                matches!(message.body, Event::Hello(_))
            })?
            .ok_or("device did not publish a HIL protocol hello")?;
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
    fn prepare_protocol(
        &self,
        target: Target<'_>,
    ) -> Result<(Capabilities, Option<StartupArtifactStatus>)> {
        let capabilities = self.request_capabilities(PROTOCOL_READY_TIMEOUT)?;
        let artifact_path = target.lab.device.startup_artifact.as_deref();
        if artifact_path.is_some() && !capabilities.features.startup_artifact {
            return Err("firmware does not support a host-owned startup artifact".into());
        }
        if !capabilities.features.data_plane_placement {
            return Err("firmware does not support explicit data-plane placement".into());
        }
        let artifact_event_start = self.protocol_event_count();
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path
            && let Some(bytes) = crate::session::startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
            crate::lab::lock::record_board(
                &target.lab.device.serial,
                oer_hil_arbiter::BoardEventKind::StartupArtifactUploaded {
                    path: path.display().to_string(),
                    sha256: crate::durable::sha256_bytes(&bytes),
                },
            );
        }
        if capabilities.features.runtime_initialization {
            self.initialize(target, PROTOCOL_READY_TIMEOUT)?;
        }
        let startup_artifact_status = if capabilities.features.startup_artifact
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
            crate::session::startup_artifact::persist_atomically(path, &bytes)?;
            crate::lab::lock::record_board(
                &target.lab.device.serial,
                oer_hil_arbiter::BoardEventKind::StartupArtifactWritten {
                    path: path.display().to_string(),
                    sha256: crate::durable::sha256_bytes(&bytes),
                    disposition: format!("{:?}", status.disposition),
                },
            );
            eprintln!(
                "startup_artifact={} disposition={:?} bytes={} initialization_elapsed_us={}",
                path.display(),
                status.disposition,
                bytes.len(),
                status.initialization_elapsed_micros,
            );
            Some(status)
        } else {
            None
        };
        Ok((capabilities, startup_artifact_status))
    }

    fn wait_for_startup_artifact_status_after(
        &self,
        start: usize,
        timeout: Duration,
    ) -> Result<StartupArtifactStatus> {
        let event = self
            .wait_for_protocol_after(start, timeout, |message| {
                matches!(&message.body, Event::StartupArtifactReady(_))
            })?
            .ok_or("device did not report startup artifact initialization status")?;
        match event.body {
            Event::StartupArtifactReady(status) => Ok(status),
            _ => unreachable!("startup artifact status predicate accepted only status events"),
        }
    }

    fn upload_startup_artifact(&self, bytes: &[u8], timeout: Duration) -> Result<()> {
        for chunk in crate::session::startup_artifact::chunks(bytes)? {
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
        let deadline = crate::transport::events::deadline_after(timeout);
        let mut cursor = start;
        let mut assembler = crate::session::startup_artifact::Assembler::new();
        loop {
            let chunk = self
                .wait_for_startup_artifact_chunk(&mut cursor, deadline)?
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
    ) -> Result<Option<StartupArtifactChunk>> {
        Ok(self
            .wait_for_protocol_cursor(
                cursor,
                deadline.saturating_duration_since(Instant::now()),
                |message| matches!(message.body, Event::StartupArtifact(_)),
            )?
            .map(|message| match message.body {
                Event::StartupArtifact(chunk) => chunk,
                _ => unreachable!("artifact predicate accepted only artifact chunks"),
            }))
    }

    fn initialize(&self, target: Target<'_>, timeout: Duration) -> Result<()> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(
            0,
            Command::Initialize(oer_hil_protocol::InitializationConfiguration {
                ap_scheduler: target.settings.ap_scheduler,
                ipv4: target.lab.station.ipv4(),
                data_plane: target.settings.data_plane,
                rx_checksum: target.settings.rx_checksum,
                tx_udp_checksum: target.settings.tx_udp_checksum,
                tx_buffer: target.settings.tx_buffer,
                rx_continuation: target.settings.rx_continuation,
                l1_cache_counters: target.settings.l1_cache_counters,
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
        })?
        .ok_or_else(|| "device did not complete role-neutral initialization".into())
        .map(|_| ())
    }

    /// Establish the typed link and run role-neutral initialization,
    /// exchanging the configured startup artifact, without starting a role.
    pub fn prepare_startup(
        &self,
        target: Target<'_>,
    ) -> Result<(Capabilities, Option<StartupArtifactStatus>)> {
        self.prepare_protocol(target)
    }

    /// Initialize and submit a real station start without assuming that an AP
    /// exists. The caller must observe its lifecycle and terminal outcome.
    pub fn begin_station_attempt(
        &self,
        target: Target<'_>,
    ) -> Result<(Capabilities, WifiCommandHandle)> {
        let (capabilities, _) = self.prepare_protocol(target)?;
        let handle = self.request_station_start(target)?;
        Ok((capabilities, handle))
    }

    pub fn prepare_station(&self, target: Target<'_>, timeout: Duration) -> Result<Capabilities> {
        self.prepare_station_with_startup_artifact_status(target, timeout)
            .map(|(capabilities, _)| capabilities)
    }

    pub fn prepare_station_with_startup_artifact_status(
        &self,
        target: Target<'_>,
        timeout: Duration,
    ) -> Result<(Capabilities, Option<StartupArtifactStatus>)> {
        let (capabilities, startup_artifact_status) = self.prepare_protocol(target)?;
        let lifecycle_cursor = self.station_lifecycle_cursor();
        let handle = self.request_station_start(target)?;
        self.wait_wifi_role_transition(handle, timeout)?;
        self.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
        Ok((capabilities, startup_artifact_status))
    }

    /// Read a window of the target's radio-PHY register image.
    pub fn read_phy_register_image(
        &self,
        request: oer_hil_protocol::PhyRegisterImageRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::PhyRegisterImageWords> {
        match self
            .send_command(0, Command::PhyRegisterImage(request), timeout)?
            .body
        {
            Event::PhyRegisterImage(words) => Ok(words),
            Event::Rejected(reason) => {
                Err(format!("device rejected a register image read: {reason:?}").into())
            }
            _ => Err("device returned an invalid register image response".into()),
        }
    }

    /// Read a window of the target's analog image.
    pub fn read_phy_analog_image(
        &self,
        request: oer_hil_protocol::PhyRegisterImageRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::PhyAnalogImageBytes> {
        match self
            .send_command(0, Command::PhyAnalogImage(request), timeout)?
            .body
        {
            Event::PhyAnalogImage(bytes) => Ok(bytes),
            Event::Rejected(reason) => {
                Err(format!("device rejected an analog image read: {reason:?}").into())
            }
            _ => Err("device returned an invalid analog image response".into()),
        }
    }

    pub fn query_operation_status(&self, timeout: Duration) -> Result<OperationStatus> {
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
        self.check_link()?;
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before command")?;
        self.exchange(boot_id, session_id, body, timeout)
    }

    fn exchange(
        &self,
        boot_id: u64,
        session_id: u64,
        body: Command,
        timeout: Duration,
    ) -> Result<Envelope<Event>> {
        self.check_link()?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let kind = super::command_kind(&body);
        let command = Envelope::new(boot_id, request_id, session_id, request_id, body);
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL command: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| LinkError::transport("serial worker stopped before HIL command"))?;
        self.worker_wake.wake()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sent
            .push(super::SentCommand {
                request_id,
                session_id,
                kind,
                host_sent_unix_micros: super::host_unix_micros(),
            });
        self.wait_for_protocol_after(event_count, timeout, |message| {
            command_response_matches(
                message,
                if boot_id == 0 {
                    message.boot_id
                } else {
                    boot_id
                },
                session_id,
                request_id,
            )
        })?
        .ok_or_else(|| "device did not answer HIL command".into())
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
            .map_err(|error| {
                crate::error::context(format!("session {operation} command failed"), error)
            })?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::State(StateChange { current, .. }) if current == expected_state => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected session {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid session {operation} response").into()),
        }
    }

    pub fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        self.start_session_with_id(session_id, config)
    }

    /// Reserve one exact current-boot/session identity before Configure so
    /// both UDP payload directions can be correlated to this lifecycle stage.
    pub fn start_identified_udp_session(
        &self,
        build: impl FnOnce(oer_hil_protocol::UdpSessionPayloadIdentity) -> SessionConfig,
    ) -> Result<(SessionHandle, oer_hil_protocol::UdpSessionPayloadIdentity)> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device omitted the current boot identity")?;
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let identity = oer_hil_protocol::UdpSessionPayloadIdentity::new(
            boot_id.rotate_left(32).wrapping_add(session_id),
        );
        let session = self.start_session_with_id(session_id, build(identity))?;
        Ok((session, identity))
    }

    fn start_session_with_id(
        &self,
        session_id: u64,
        config: SessionConfig,
    ) -> Result<SessionHandle> {
        let first_event = self.protocol_event_count();
        let direction = config.direction;
        let link_requirements = config.link_requirements;
        let flow_ids = config.flows.map(|flow| flow.map(|flow| flow.flow_id));
        let handle = SessionHandle {
            session_id,
            first_event,
            flow_ids,
        };
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
            self.wait_for_session_event(handle, PROTOCOL_READY_TIMEOUT, |message| {
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
            })?
            .ok_or_else(|| {
                format!(
                    "device did not publish {expected:?} data-plane readiness for session \
                     {session_id}; required TX BlockAck TID: {:?}",
                    link_requirements.tx_block_ack_tid,
                )
            })?;
        }
        Ok(handle)
    }

    fn wait_for_session_event(
        &self,
        session: SessionHandle,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Result<Option<Envelope<Event>>> {
        let event = self.wait_for_protocol_after(session.first_event, timeout, |message| {
            message.session_id == session.session_id
                && (predicate(message) || matches!(message.body, Event::Failed(_)))
        })?;
        if let Some(Envelope {
            body: Event::Failed(reason),
            ..
        }) = event
        {
            return Err(format!("target session {} failed: {reason:?}", session.session_id).into());
        }
        Ok(event)
    }

    pub fn wait_for_udp_rx_started(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<u64> {
        let event = self
            .wait_for_session_event(session, timeout, |message| {
                matches!(message.body, Event::UdpRxStarted { datagrams: 256 })
            })?
            .ok_or("device did not confirm UDP delivery before maintenance")?;
        let Event::UdpRxStarted { datagrams } = event.body else {
            unreachable!()
        };
        Ok(datagrams)
    }

    pub fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        let deadline = crate::transport::events::deadline_after(timeout);
        let evidence = self
            .wait_for_session_event(
                session,
                deadline.saturating_duration_since(Instant::now()),
                |message| {
                    message.session_id == session.session_id
                        && matches!(message.body, Event::Evidence(EvidenceRecord::Transport(_)))
                },
            )?
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
                .wait_for_session_event(session, remaining, |message| {
                    message.session_id == session.session_id
                        && matches!(
                            message.body,
                            Event::Evidence(EvidenceRecord::FlowTransport(flow))
                                if flow.flow_id == flow_id
                        )
                })?
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
            .wait_for_session_event(
                session,
                deadline.saturating_duration_since(Instant::now()),
                |message| {
                    message.session_id == session.session_id
                        && matches!(message.body, Event::Evidence(EvidenceRecord::Link(_)))
                },
            )?
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
            return Err(LinkError::protocol(format!(
                "device protocol link is unhealthy: {link:?}"
            ))
            .into());
        }
        let stack_evidence = self
            .wait_for_session_event(
                session,
                deadline.saturating_duration_since(Instant::now()),
                |message| {
                    message.session_id == session.session_id
                        && matches!(message.body, Event::Evidence(EvidenceRecord::Stack(_)))
                },
            )?
            .ok_or("device did not publish structured stack evidence")?;
        let stack = match stack_evidence.body {
            Event::Evidence(EvidenceRecord::Stack(stack)) => stack,
            _ => unreachable!("stack evidence predicate accepted only stack evidence"),
        };
        validate_stack_usage(stack)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let finished = self
            .wait_for_session_event(session, remaining, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Finished(_))
            })?
            .ok_or("device did not finish the structured HIL session")?;
        let finished = match finished.body {
            Event::Finished(finished) => finished,
            _ => unreachable!("session completion predicate accepted only Finished"),
        };
        let radio = self
            .wait_for_session_event(session, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Radio(_)))
            })?
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::Radio(radio)) => radio,
                _ => unreachable!("radio predicate accepted only radio evidence"),
            });
        let tx_timing = self
            .wait_for_session_event(session, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::TxAggregateTiming(_))
                    )
            })?
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::TxAggregateTiming(timing)) => timing,
                _ => unreachable!("TX timing predicate accepted only aggregate timing evidence"),
            });
        let rx_delivery = self
            .wait_for_session_event(session, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::RxDelivery(_)))
            })?
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::RxDelivery(delivery)) => delivery,
                _ => unreachable!("RX delivery predicate accepted only delivery evidence"),
            });
        let network_scheduler = self
            .wait_for_session_event(session, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(
                        message.body,
                        Event::Evidence(EvidenceRecord::NetworkScheduler(_))
                    )
            })?
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::NetworkScheduler(evidence)) => evidence,
                _ => unreachable!("scheduler predicate accepted only scheduler evidence"),
            });
        let rx_zero_copy = self
            .wait_for_session_event(session, Duration::ZERO, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::RxZeroCopy(_)))
            })?
            .map(|event| match event.body {
                Event::Evidence(EvidenceRecord::RxZeroCopy(evidence)) => evidence,
                _ => unreachable!("zero-copy predicate accepted only zero-copy evidence"),
            });
        if let Some(zero_copy) = rx_zero_copy {
            super::validation::validate_rx_zero_copy(zero_copy)?;
        }
        let expected_records = 3
            + u16::try_from(flow_transport.iter().flatten().count())?
            + u16::from(radio.is_some())
            + u16::from(tx_timing.is_some())
            + u16::from(rx_delivery.is_some())
            + u16::from(network_scheduler.is_some())
            + u16::from(rx_zero_copy.is_some());
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
        if let Some(zero_copy) = rx_zero_copy {
            records.push(EvidenceRecord::RxZeroCopy(zero_copy));
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
            rx_zero_copy,
            stack,
            link,
            finished,
        };
        if session_evidence.flow_transport.iter().flatten().count()
            != session.flow_ids.iter().flatten().count()
        {
            return Err("structured session lost configured flow evidence".into());
        }
        Ok(session_evidence)
    }

    /// Verify that the target retained the complete immutable result before
    /// authorizing its removal. This happens after the measured traffic.
    pub fn acknowledge_session(&self, session: SessionHandle) -> Result<()> {
        let original = self.wait_for_session(session, Duration::ZERO)?;
        let first_event = self.protocol_event_count();
        let response = self.send_command(
            session.session_id,
            Command::ReplayResult,
            PROTOCOL_READY_TIMEOUT,
        )?;
        if let Event::Rejected(reason) = response.body {
            return Err(LinkError::protocol(format!(
                "target cannot replay completed session {}: {reason:?}",
                session.session_id
            ))
            .into());
        }
        let replay = self.wait_for_session(
            SessionHandle {
                first_event,
                ..session
            },
            PROTOCOL_READY_TIMEOUT,
        )?;
        if replay != original {
            return Err(LinkError::protocol(format!(
                "target changed the retained result for session {}",
                session.session_id
            ))
            .into());
        }
        self.expect_accepted(
            session.session_id,
            Command::AcknowledgeResult,
            "acknowledgement",
            SessionState::Idle,
        )
    }

    pub fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
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

    pub fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .messages
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
            Event::Accepted => {
                let state = self
                    .protocol
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let accepted_offset = state.messages[first_event..]
                    .iter()
                    .position(|message| {
                        message.boot_id == response.boot_id
                            && message.session_id == 0
                            && message.request_id == response.request_id
                            && message.message_sequence == response.message_sequence
                    })
                    .ok_or("accepted Wi-Fi command disappeared from the capture")?;
                Ok(WifiCommandHandle {
                    boot_id: response.boot_id,
                    request_id: response.request_id,
                    first_event: first_event + accepted_offset + 1,
                })
            }
            Event::Rejected(reason) => {
                Err(format!("device rejected {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid {operation} response").into()),
        }
    }

    pub fn request_station_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopStation, "station stop")
    }

    pub fn request_radio_restart(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::RestartRadio, "idle radio restart")
    }

    pub fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage> {
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

    pub fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        let response = self.send_command(0, Command::QueryLinkHealth, timeout)?;
        match response.body {
            Event::LinkHealth(health) => Ok(health),
            Event::Rejected(reason) => {
                Err(format!("device rejected link-health query: {reason:?}").into())
            }
            _ => Err("device returned an invalid link-health response".into()),
        }
    }

    pub fn probe_memory_benchmark(
        &self,
        request: oer_hil_protocol::MemoryBenchmarkRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::MemoryBenchmarkEvidence> {
        let response = self.send_command(0, Command::ProbeMemoryBenchmark(request), timeout)?;
        match response.body {
            Event::MemoryBenchmarkCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected memory benchmark: {reason:?}").into())
            }
            _ => Err("device returned an invalid memory-benchmark response".into()),
        }
    }

    pub fn bluetooth_dtm(
        &self,
        operation: oer_hil_protocol::BluetoothDtmOperation,
    ) -> Result<oer_hil_protocol::BluetoothDtmEvidence> {
        match self
            .send_command(0, Command::BluetoothDtm(operation), Duration::from_secs(5))?
            .body
        {
            Event::BluetoothDtm(evidence) if evidence.completed(operation) => Ok(evidence),
            response => Err(format!("Bluetooth {operation:?} failed: {response:?}").into()),
        }
    }

    /// One raw HCI exchange with the Controller of a `bluetooth_hci` image.
    pub fn bluetooth_hci(
        &self,
        request: oer_hil_protocol::BluetoothHciRequest,
    ) -> Result<oer_hil_protocol::BluetoothHciResponse> {
        let wait = match &request {
            oer_hil_protocol::BluetoothHciRequest::NextPacket { wait_ms } => {
                Duration::from_millis(u64::from(*wait_ms))
            }
            oer_hil_protocol::BluetoothHciRequest::Command { .. }
            | oer_hil_protocol::BluetoothHciRequest::Acl { .. } => Duration::ZERO,
            // Stopping and starting the Controller on the shared radio.
            oer_hil_protocol::BluetoothHciRequest::Lifecycle(_) => Duration::from_secs(15),
        };
        match self
            .send_command(
                0,
                Command::BluetoothHci(request),
                Duration::from_secs(5) + wait,
            )?
            .body
        {
            Event::BluetoothHci(response) => Ok(response),
            response => Err(format!("Bluetooth HCI exchange rejected: {response:?}").into()),
        }
    }

    /// The standalone GATT image's observation, accepted only with its
    /// single-core Bluetooth IRQ stack evidence.
    pub fn bluetooth_gatt(&self) -> Result<oer_hil_protocol::BluetoothGattEvidence> {
        let evidence = self.bluetooth_gatt_observation()?;
        self.require_bluetooth_irq_stack()?;
        Ok(evidence)
    }

    /// The GATT application's observation without an image-specific stack
    /// policy; the joint Wi-Fi/Bluetooth image reports its stacks through the
    /// Wi-Fi evidence instead.
    pub fn bluetooth_gatt_observation(&self) -> Result<oer_hil_protocol::BluetoothGattEvidence> {
        match self
            .send_command(0, Command::QueryBluetoothGatt, Duration::from_secs(2))?
            .body
        {
            Event::BluetoothGatt(evidence) => Ok(evidence),
            response => Err(format!("invalid GATT observation: {response:?}").into()),
        }
    }

    pub fn bluetooth_secure_gatt(&self) -> Result<oer_hil_protocol::BluetoothSecureGattEvidence> {
        let evidence = self.bluetooth_secure_gatt_snapshot()?;
        self.require_bluetooth_irq_stack()?;
        Ok(evidence)
    }

    /// Protocol observation only. IRQ watermark scanning masks interrupts;
    /// callers selecting this path must explicitly own their sampling policy.
    pub fn bluetooth_secure_gatt_snapshot(
        &self,
    ) -> Result<oer_hil_protocol::BluetoothSecureGattEvidence> {
        match self
            .send_command(0, Command::QueryBluetoothSecureGatt, Duration::from_secs(2))?
            .body
        {
            Event::BluetoothSecureGatt(evidence) => Ok(evidence),
            response => Err(format!("invalid secure GATT observation: {response:?}").into()),
        }
    }

    pub fn restart_bluetooth_gatt(&self, boot: u64, epoch: u32) -> Result<()> {
        if boot == 0 {
            return Err("secure GATT restart requires the observed boot identity".into());
        }
        match self
            .exchange(
                boot,
                0,
                Command::RestartBluetoothGatt { epoch },
                Duration::from_secs(2),
            )?
            .body
        {
            Event::BluetoothSecureGatt(e) if e.epoch == epoch && e.restarting => Ok(()),
            response => Err(format!("secure GATT restart rejected: {response:?}").into()),
        }
    }

    pub fn fail_next_bluetooth_gatt_bond_load(&self, boot: u64, epoch: u32) -> Result<()> {
        if boot == 0 {
            return Err("bond load fault requires observed boot identity".into());
        }
        match self
            .exchange(
                boot,
                0,
                Command::FailNextBluetoothGattBondLoad { epoch },
                Duration::from_secs(2),
            )?
            .body
        {
            Event::BluetoothSecureGatt(e)
                if e.epoch == epoch && e.bond_load_fault_armed && e.bond_load_failures == 0 =>
            {
                Ok(())
            }
            response => Err(format!("bond load fault rejected: {response:?}").into()),
        }
    }

    pub fn bluetooth_gatt_reset_read_gate(
        &self,
        boot: u64,
        epoch: u32,
        release: bool,
    ) -> Result<()> {
        use oer_hil_protocol::BluetoothGattResetReadGate as Phase;
        if boot == 0 {
            return Err("Reset gate requires observed boot identity".into());
        }
        let expected = if release {
            Phase::Released
        } else {
            Phase::Armed
        };
        match self
            .exchange(
                boot,
                0,
                Command::BluetoothGattResetReadGate { epoch, release },
                Duration::from_secs(2),
            )?
            .body
        {
            Event::BluetoothSecureGatt(e) if e.epoch == epoch && e.reset_read_gate == expected => {
                Ok(())
            }
            response => Err(format!("Reset reader gate rejected: {response:?}").into()),
        }
    }

    pub fn require_bluetooth_gatt_restart_rejected(&self, boot: u64, epoch: u32) -> Result<()> {
        match self
            .exchange(
                boot,
                0,
                Command::RestartBluetoothGatt { epoch },
                Duration::from_secs(2),
            )?
            .body
        {
            Event::Rejected(oer_hil_protocol::RejectReason::InvalidState) => Ok(()),
            response => Err(format!(
                "terminal GATT epoch accepted restart or lost identity: {response:?}"
            )
            .into()),
        }
    }

    pub fn fail_bluetooth_gatt_reset_read(&self, boot: u64, epoch: u32) -> Result<()> {
        if boot == 0 {
            return Err("Reset read fault requires observed boot identity".into());
        }
        match self
            .exchange(
                boot,
                0,
                Command::FailBluetoothGattResetRead { epoch },
                Duration::from_secs(2),
            )?
            .body
        {
            Event::BluetoothSecureGatt(e)
                if e.epoch == epoch
                    && e.reset_read_gate
                        == oer_hil_protocol::BluetoothGattResetReadGate::FailureRequested =>
            {
                Ok(())
            }
            response => Err(format!("Reset read fault rejected: {response:?}").into()),
        }
    }

    pub fn confirm_bluetooth_gatt(
        &self,
        boot: u64,
        decision: oer_hil_protocol::BluetoothNumericDecision,
    ) -> Result<()> {
        if boot == 0 {
            return Err("Numeric Comparison requires the displayed boot identity".into());
        }
        match self
            .exchange(
                boot,
                0,
                Command::ConfirmBluetoothGatt(decision),
                Duration::from_secs(2),
            )?
            .body
        {
            Event::BluetoothGattDecisionRecorded(recorded) if recorded == decision => Ok(()),
            response => Err(format!("Numeric Comparison decision rejected: {response:?}").into()),
        }
    }

    pub fn boot_status(&self) -> Result<oer_hil_protocol::BootEvidence> {
        match self
            .send_command(0, Command::GetBootStatus, Duration::from_secs(5))?
            .body
        {
            Event::BootStatus(evidence) => Ok(evidence),
            response => Err(format!("invalid boot status: {response:?}").into()),
        }
    }

    /// The previous boot's post-mortem checkpoints, oldest first: `count`
    /// of them, as its boot evidence reports, fetched page by page.
    pub fn post_mortem_checkpoints(&self, count: u8) -> Result<Vec<oer_hil_protocol::Checkpoint>> {
        let mut checkpoints = Vec::with_capacity(usize::from(count));
        while checkpoints.len() < usize::from(count) {
            let first = checkpoints.len() as u8;
            match self
                .send_command(
                    0,
                    Command::GetPostMortemCheckpoints { first },
                    Duration::from_secs(5),
                )?
                .body
            {
                Event::PostMortemCheckpoints(page)
                    if page.first == first && !page.checkpoints.is_empty() =>
                {
                    checkpoints.extend(page.checkpoints);
                }
                response => {
                    return Err(format!("invalid post-mortem page {first}: {response:?}").into());
                }
            }
        }
        checkpoints.truncate(usize::from(count));
        Ok(checkpoints)
    }

    /// Arm the program-counter profile once the boot's hello arrived; the
    /// capture drains it into `profile.json` when it finishes.
    pub fn profiled(mut self, profile: crate::scenario::ProfileRequest) -> Result<Self> {
        self.wait_for_protocol_after(0, PROTOCOL_READY_TIMEOUT, |message| {
            matches!(message.body, Event::Hello(_))
        })?
        .ok_or("device did not publish a HIL protocol hello before the profile")?;
        match self
            .send_command(
                0,
                Command::ProfileControl(profile.control()),
                Duration::from_secs(5),
            )?
            .body
        {
            Event::ProfileStatus(status) if status.armed => {}
            response => {
                return Err(format!("the image did not arm the profile: {response:?}").into());
            }
        }
        self.profile = Some(profile);
        Ok(self)
    }

    /// The closed window's status and each hart's raw `(pc, ra)` samples;
    /// the profile is disarmed afterwards.
    pub(crate) fn drain_profile(&self) -> Result<DrainedProfile> {
        let status = match self
            .send_command(
                0,
                Command::ProfileControl(oer_hil_protocol::ProfileControl::Status),
                Duration::from_secs(5),
            )?
            .body
        {
            Event::ProfileStatus(status) => status,
            response => return Err(format!("profile status rejected: {response:?}").into()),
        };
        let mut samples = [Vec::new(), Vec::new()];
        if !status.open {
            for (hart, pairs) in samples.iter_mut().enumerate() {
                while (pairs.len() as u32) < status.samples[hart] {
                    let first = pairs.len() as u32;
                    match self
                        .send_command(
                            0,
                            Command::GetProfileSamples {
                                hart: hart as u8,
                                first,
                            },
                            Duration::from_secs(5),
                        )?
                        .body
                    {
                        Event::ProfileSamples(page)
                            if page.first == first && !page.samples.is_empty() =>
                        {
                            pairs.extend(page.samples);
                        }
                        response => {
                            return Err(format!(
                                "profile page {first} of hart {hart} rejected: {response:?}"
                            )
                            .into());
                        }
                    }
                }
            }
        }
        let _ = self.send_command(
            0,
            Command::ProfileControl(oer_hil_protocol::ProfileControl::Disarm),
            Duration::from_secs(5),
        );
        Ok((status, samples))
    }

    /// Report, start or re-mask the target's event trace.
    pub fn trace_control(
        &self,
        control: oer_hil_protocol::TraceControl,
    ) -> Result<oer_hil_protocol::TraceStatus> {
        match self
            .send_command(0, Command::TraceControl(control), Duration::from_secs(5))?
            .body
        {
            Event::TraceStatus(status) => Ok(status),
            response => Err(format!("trace control {control:?} rejected: {response:?}").into()),
        }
    }

    /// Every complete trace entry in storage, in storage order.
    pub fn trace_entries(&self, slots: u16) -> Result<Vec<oer_hil_protocol::TraceEntry>> {
        let mut entries = Vec::new();
        let mut first = 0;
        while first < slots {
            match self
                .send_command(
                    0,
                    Command::GetTraceEntries { first },
                    Duration::from_secs(5),
                )?
                .body
            {
                Event::TraceEntries(page) if page.first == first && page.next > first => {
                    entries.extend(page.entries);
                    first = page.next;
                }
                response => {
                    return Err(format!("invalid trace page {first}: {response:?}").into());
                }
            }
        }
        Ok(entries)
    }

    /// The words of the snapshot in `slot`, with its page header; `None`
    /// when the slot holds none or was overwritten while it was read.
    pub fn trace_snapshot(
        &self,
        slot: u8,
    ) -> Result<Option<(oer_hil_protocol::TraceSnapshotPage, Vec<u32>)>> {
        let mut words = Vec::new();
        let mut header = None;
        loop {
            let offset = u16::try_from(words.len())?;
            match self
                .send_command(
                    0,
                    Command::GetTraceSnapshot { slot, offset },
                    Duration::from_secs(5),
                )?
                .body
            {
                Event::TraceSnapshot(None) => return Ok(None),
                Event::TraceSnapshot(Some(page)) if page.offset == offset => {
                    let done = page.words.is_empty()
                        || words.len() + page.words.len() >= usize::from(page.len);
                    words.extend(page.words.iter().copied());
                    header.get_or_insert(page);
                    if done {
                        return Ok(header.map(|header| (header, words)));
                    }
                }
                response => {
                    return Err(format!("invalid trace snapshot page: {response:?}").into());
                }
            }
        }
    }

    /// Stall `target`'s executor; the target's hang watchdog then resets it.
    pub fn inject_hang(&self, target: oer_hil_protocol::HangTarget) -> Result<()> {
        match self
            .send_command(0, Command::InjectHang(target), Duration::from_secs(5))?
            .body
        {
            Event::HangInjected(observed) if observed == target => Ok(()),
            response => Err(format!("hang injection {target:?} rejected: {response:?}").into()),
        }
    }

    pub fn system_watchdog_test(&self, mode: oer_hil_protocol::WatchdogTestMode) -> Result<()> {
        match self
            .send_command(0, Command::SystemWatchdogTest(mode), Duration::from_secs(5))?
            .body
        {
            Event::SystemWatchdogTest(observed) if observed == mode => Ok(()),
            response => Err(format!("watchdog {mode:?} rejected: {response:?}").into()),
        }
    }

    pub fn phy_fault(
        &self,
        command: oer_hil_protocol::PhyFaultCommand,
    ) -> Result<oer_hil_protocol::PhyFaultEvidence> {
        match self
            .send_command(0, Command::PhyFault(command), Duration::from_secs(2))?
            .body
        {
            Event::PhyFault(evidence) => Ok(evidence),
            response => Err(format!("PHY fault control {command:?} rejected: {response:?}").into()),
        }
    }

    /// Suspend, resume or report the shared PHY's periodic tracking timer.
    pub fn phy_tracking(
        &self,
        command: oer_hil_protocol::PhyTrackingCommand,
    ) -> Result<oer_hil_protocol::PhyTrackingEvidence> {
        match self
            .send_command(0, Command::PhyTracking(command), Duration::from_secs(2))?
            .body
        {
            Event::PhyTracking(evidence) => Ok(evidence),
            response => {
                Err(format!("PHY tracking control {command:?} rejected: {response:?}").into())
            }
        }
    }

    pub fn require_bluetooth_irq_stack(&self) -> Result<()> {
        let response =
            self.send_command(0, Command::QueryInterruptStackUsage, Duration::from_secs(5))?;
        match response.body {
            Event::InterruptStackUsage { cpu0, cpu1 } => {
                super::validation::validate_bluetooth_irq_stack(cpu0, cpu1)
            }
            response => Err(format!(
                "Bluetooth IRQ stack evidence missing or below policy: {response:?}"
            )
            .into()),
        }
    }

    pub fn probe_timebase(
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

    pub fn probe_ieee802154_event_status(
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

    pub fn probe_ieee802154_ed_event(
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

    pub fn probe_ieee802154_route(
        &self,
        request: Ieee802154RouteProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154RouteProbeEvidence> {
        match self
            .send_command(0, Command::ProbeIeee802154Route(request), timeout)?
            .body
        {
            Event::Ieee802154RouteProbeCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected the IEEE 802.15.4 route probe: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 route probe response".into()),
        }
    }

    pub fn run_ieee802154_air_check(
        &self,
        request: Ieee802154AirCheckRequest,
        timeout: Duration,
    ) -> Result<Ieee802154AirCheckEvidence> {
        let response = self.send_command(0, Command::RunIeee802154AirCheck(request), timeout)?;
        match response.body {
            Event::Ieee802154AirCheckCompleted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 air check: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 air check response".into()),
        }
    }

    pub fn start_ieee802154_session(
        &self,
        config: Ieee802154SessionConfig,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self
            .send_command(0, Command::StartIeee802154Session(config), timeout)?
            .body
        {
            Event::Ieee802154SessionStarted(result) => Ok(result),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session start: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 session start response".into()),
        }
    }

    pub fn transmit_ieee802154_session(
        &self,
        request: Ieee802154SessionTransmitRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionTransmitEvidence> {
        match self
            .send_command(0, Command::TransmitIeee802154Session(request), timeout)?
            .body
        {
            Event::Ieee802154SessionTransmitted(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session transmit: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 session transmit response".into()),
        }
    }

    fn ieee802154_session_accepted(&self, command: Command, what: &str) -> Result<()> {
        match self.send_command(0, command, Duration::from_secs(5))?.body {
            Event::Accepted => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session {what}: {reason:?}").into())
            }
            _ => Err(
                format!("device returned an invalid IEEE 802.15.4 session {what} response").into(),
            ),
        }
    }

    pub fn receive_ieee802154_session(&self) -> Result<()> {
        self.ieee802154_session_accepted(Command::ReceiveIeee802154Session, "receive")
    }

    pub fn set_ieee802154_session_pending(
        &self,
        request: Ieee802154SessionPendingRequest,
    ) -> Result<()> {
        self.ieee802154_session_accepted(Command::SetIeee802154SessionPending(request), "pending")
    }

    pub fn collect_ieee802154_session(&self) -> Result<Ieee802154SessionReceiveEvidence> {
        match self
            .send_command(0, Command::CollectIeee802154Session, Duration::from_secs(5))?
            .body
        {
            Event::Ieee802154SessionReceived(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session collect: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 session collect response".into()),
        }
    }

    pub fn maintain_ieee802154_session_phy(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionPhyMaintenance> {
        match self
            .send_command(0, Command::MaintainIeee802154SessionPhy, timeout)?
            .body
        {
            Event::Ieee802154SessionPhyMaintained(outcome) => Ok(outcome),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 PHY maintenance: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 PHY maintenance response".into()),
        }
    }

    /// Start OpenThread over the composed client and join the dataset's
    /// network.
    pub fn start_ieee802154_thread(
        &self,
        request: Ieee802154ThreadStartRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self
            .send_command(0, Command::StartIeee802154Thread(request), timeout)?
            .body
        {
            Event::Ieee802154ThreadStarted(result) => Ok(result),
            Event::Rejected(reason) => {
                Err(format!("device rejected the Thread session start: {reason:?}").into())
            }
            _ => Err("device returned an invalid Thread session start response".into()),
        }
    }

    pub fn query_ieee802154_thread(&self, timeout: Duration) -> Result<Ieee802154ThreadState> {
        match self
            .send_command(0, Command::QueryIeee802154Thread, timeout)?
            .body
        {
            Event::Ieee802154ThreadState(state) => Ok(state),
            Event::Rejected(reason) => {
                Err(format!("device rejected the Thread state query: {reason:?}").into())
            }
            _ => Err("device returned an invalid Thread state".into()),
        }
    }

    pub fn send_ieee802154_thread(
        &self,
        request: Ieee802154ThreadSendRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self
            .send_command(0, Command::SendIeee802154Thread(request), timeout)?
            .body
        {
            Event::Ieee802154ThreadSent(result) => Ok(result),
            Event::Rejected(reason) => {
                Err(format!("device rejected the Thread datagram: {reason:?}").into())
            }
            _ => Err("device returned an invalid Thread send response".into()),
        }
    }

    pub fn collect_ieee802154_thread(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154ThreadReceiveEvidence> {
        match self
            .send_command(0, Command::CollectIeee802154Thread, timeout)?
            .body
        {
            Event::Ieee802154ThreadReceived(evidence) => Ok(evidence),
            Event::Rejected(reason) => {
                Err(format!("device rejected the Thread collection: {reason:?}").into())
            }
            _ => Err("device returned an invalid Thread collection".into()),
        }
    }

    pub fn stop_ieee802154_thread(&self, timeout: Duration) -> Result<Ieee802154SessionResult> {
        match self
            .send_command(0, Command::StopIeee802154Thread, timeout)?
            .body
        {
            Event::Ieee802154ThreadStopped(result) => Ok(result),
            Event::Rejected(reason) => {
                Err(format!("device rejected the Thread session stop: {reason:?}").into())
            }
            _ => Err("device returned an invalid Thread session stop response".into()),
        }
    }

    /// Stop the running session's client and start it again, then apply the
    /// session configuration and receive.
    pub fn restart_ieee802154_session_radio(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionRestartEvidence> {
        match self
            .send_command(0, Command::RestartIeee802154SessionRadio, timeout)?
            .body
        {
            Event::Ieee802154SessionRadioRestarted(evidence) => Ok(evidence),
            Event::Rejected(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 session radio restart: {reason:?}"
            )
            .into()),
            _ => Err("device returned an invalid IEEE 802.15.4 session radio restart".into()),
        }
    }

    /// Read the live RSSI of the most recent baseband reception in the
    /// running session.
    pub fn read_ieee802154_session_recent_rssi(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionRecentRssi> {
        match self
            .send_command(0, Command::ReadIeee802154SessionRecentRssi, timeout)?
            .body
        {
            Event::Ieee802154SessionRecentRssi(evidence) => Ok(evidence),
            Event::Rejected(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 session RSSI read: {reason:?}"
            )
            .into()),
            _ => Err("device returned an invalid IEEE 802.15.4 session RSSI read".into()),
        }
    }

    /// Scan the energy on one channel and assess it once in the running
    /// session.
    pub fn assess_ieee802154_session_channel(
        &self,
        request: Ieee802154SessionAssessRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionAssessment> {
        match self
            .send_command(0, Command::AssessIeee802154SessionChannel(request), timeout)?
            .body
        {
            Event::Ieee802154SessionAssessed(assessment) => Ok(assessment),
            Event::Rejected(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 channel assessment: {reason:?}"
            )
            .into()),
            _ => Err("device returned an invalid IEEE 802.15.4 channel assessment".into()),
        }
    }

    pub fn stop_ieee802154_session(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionStopEvidence> {
        match self
            .send_command(0, Command::StopIeee802154Session, timeout)?
            .body
        {
            Event::Ieee802154SessionStopped(result) => Ok(result),
            Event::Rejected(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session stop: {reason:?}").into())
            }
            _ => Err("device returned an invalid IEEE 802.15.4 session stop response".into()),
        }
    }

    pub fn request_station_start(&self, target: Target<'_>) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            Command::StartStation(target.lab.station.protocol_credentials()?),
            "station start",
        )
    }

    pub fn request_wifi_scan(&self, request: WifiScanRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::ScanWifi(request), "standalone Wi-Fi scan")
    }

    pub fn request_monitor_start(&self, request: WifiMonitorRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartMonitor(request), "monitor start")
    }

    pub fn request_monitor_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopMonitor, "monitor stop")
    }

    pub fn request_access_point_start(
        &self,
        request: oer_hil_protocol::WifiAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StartAccessPoint(request), "access-point start")
    }

    pub fn request_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopAccessPoint, "access-point stop")
    }

    pub fn request_station_access_point_start(
        &self,
        request: oer_hil_protocol::WifiStationAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            Command::StartStationAccessPoint(request),
            "station-access-point start",
        )
    }

    pub fn request_station_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::StopStationAccessPoint, "station-access-point stop")
    }

    pub fn request_monitor_capture(
        &self,
        request: WifiMonitorCaptureRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(Command::CaptureMonitor(request), "finite monitor capture")
    }

    fn wait_for_wifi_event(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Result<Option<Envelope<Event>>> {
        let event = self.wait_for_protocol_after(handle.first_event, timeout, |message| {
            message.boot_id == handle.boot_id
                && message.session_id == 0
                && message.request_id == handle.request_id
                && (predicate(message)
                    || matches!(message.body, Event::WifiRoleFailed(_) | Event::Failed(_)))
        })?;
        if let Some(message) = &event {
            match message.body {
                Event::WifiRoleFailed(reason) => {
                    return Err(format!(
                        "Wi-Fi operation {} failed: {reason:?}",
                        handle.request_id
                    )
                    .into());
                }
                Event::Failed(reason) => {
                    return Err(format!(
                        "target operation {} failed: {reason:?}",
                        handle.request_id
                    )
                    .into());
                }
                _ => {}
            }
        }
        Ok(event)
    }

    pub fn wait_wifi_role_transition(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiRoleTransitioned(_) | Event::WifiRoleFailed(_)
                    )
            })?
            .ok_or("device did not complete the Wi-Fi role transition")?;
        match event.body {
            Event::WifiRoleTransitioned(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("Wi-Fi role transition failed: {failure:?}").into())
            }
            _ => unreachable!("role-transition predicate accepted only terminal role events"),
        }
    }

    pub fn wait_wifi_radio_restart(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRadioRestartEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiRadioRestarted(_) | Event::WifiRoleFailed(_)
                    )
            })?
            .ok_or("device did not complete the idle radio restart")?;
        match event.body {
            Event::WifiRadioRestarted(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("idle radio restart failed: {failure:?}").into())
            }
            _ => unreachable!("radio-restart predicate accepted only terminal restart events"),
        }
    }

    pub fn wait_wifi_scan(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiScanEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiScanCompleted(_))
            })?
            .ok_or("device did not complete the standalone Wi-Fi scan")?;
        match event.body {
            Event::WifiScanCompleted(evidence) => Ok(evidence),
            _ => unreachable!("scan predicate accepted only its completion event"),
        }
    }

    pub fn wait_monitor_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStarted(_))
            })?
            .ok_or("device did not complete monitor start")?;
        match event.body {
            Event::WifiMonitorStarted(evidence) => Ok(evidence),
            _ => unreachable!("monitor-start predicate accepted only its completion event"),
        }
    }

    pub fn wait_access_point_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStarted(_) | Event::WifiRoleFailed(_)
                    )
            })?
            .ok_or("device did not complete the access-point start")?;
        match event.body {
            Event::WifiAccessPointStarted(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point start failed: {failure:?}").into())
            }
            _ => unreachable!("AP-start predicate accepted only terminal AP events"),
        }
    }

    pub fn wait_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::WifiAccessPointEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiAccessPointStopped(_) | Event::WifiRoleFailed(_)
                    )
            })?
            .ok_or("device did not complete the access-point stop")?;
        match event.body {
            Event::WifiAccessPointStopped(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("access-point stop failed: {failure:?}").into())
            }
            _ => unreachable!("AP-stop predicate accepted only terminal AP events"),
        }
    }

    pub fn wait_station_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::WifiStationAccessPointStopEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(
                        message.body,
                        Event::WifiStationAccessPointStopped(_) | Event::WifiRoleFailed(_)
                    )
            })?
            .ok_or("device did not complete the station-access-point stop")?;
        match event.body {
            Event::WifiStationAccessPointStopped(evidence) => Ok(evidence),
            Event::WifiRoleFailed(failure) => {
                Err(format!("station-access-point stop failed: {failure:?}").into())
            }
            _ => unreachable!("paired-stop predicate accepted only terminal paired events"),
        }
    }

    pub fn wait_monitor_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiMonitorEvidence> {
        let event = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorStopped(_))
            })?
            .ok_or("device did not complete monitor stop")?;
        match event.body {
            Event::WifiMonitorStopped(evidence) => Ok(evidence),
            _ => unreachable!("monitor-stop predicate accepted only its completion event"),
        }
    }

    pub fn wait_monitor_capture(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence> {
        let completion = self
            .wait_for_wifi_event(handle, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::WifiMonitorCaptureCompleted(_))
            })?
            .ok_or("device did not complete finite monitor capture")?;
        let summary = match completion.body {
            Event::WifiMonitorCaptureCompleted(evidence) => evidence,
            _ => unreachable!("capture predicate accepted only terminal capture evidence"),
        };
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let chunks = state
            .messages
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

    pub fn wait_for_connected_station(&self, timeout: Duration) -> Result<u32> {
        self.wait_for_connected_station_after(0, timeout)
    }

    /// Wait for a connected edge published after a caller-owned event cursor.
    ///
    /// Reusing the first connected event of the current boot is incorrect for
    /// role roundtrips: the next AP epoch would then start while the preceding
    /// station restart was still scanning. Callers that initiate a new station
    /// epoch must snapshot [`Self::station_lifecycle_cursor`] before the start
    /// command and use this method.
    pub fn wait_for_connected_station_after(
        &self,
        first_event: usize,
        timeout: Duration,
    ) -> Result<u32> {
        Ok(self
            .wait_for_connected_station_link_after(first_event, timeout)?
            .generation)
    }

    pub fn wait_for_connected_station_link_after(
        &self,
        first_event: usize,
        timeout: Duration,
    ) -> Result<StationConnectionObservation> {
        let deadline = Instant::now() + timeout;
        let mut cursor = first_event;
        loop {
            let event = self
                .wait_station_lifecycle_event_optional(
                    &mut cursor,
                    deadline.saturating_duration_since(Instant::now()),
                )?
                .ok_or("device did not publish connected station readiness")?;
            match event {
                StationLifecycleEvent::Connected {
                    generation,
                    association_bandwidth_mhz,
                    security,
                } => {
                    return Ok(StationConnectionObservation {
                        generation,
                        association_bandwidth_mhz,
                        security,
                        event_cursor_after: cursor,
                    });
                }
                StationLifecycleEvent::AttemptFailed { .. } => {}
                other => {
                    return Err(format!(
                        "station published {other:?} before the new connected frontier"
                    )
                    .into());
                }
            }
        }
    }

    /// A lifecycle stage needs a newly published network endpoint; the last
    /// boot-scoped address is not proof that its successor has configured IP.
    pub fn wait_for_network_ready_after(
        &self,
        first_event: usize,
        interface: WifiNetworkInterface,
        timeout: Duration,
    ) -> Result<Ipv4Addr> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device omitted the current boot identity")?;
        let event = self.wait_for_protocol_after(first_event, timeout, |message| {
            message.boot_id == boot_id
                && message.session_id == 0
                && message.request_id == 0
                && matches!(message.body, Event::NetworkReady(info) if info.network_interface == interface)
        })?.ok_or("new station stage did not publish a fresh network endpoint")?;
        match event.body {
            Event::NetworkReady(info) => Ok(Ipv4Addr::from(info.address)),
            _ => unreachable!("network predicate accepted only a network endpoint"),
        }
    }

    /// Cursor for reliable unsolicited station lifecycle events.
    pub fn station_lifecycle_cursor(&self) -> usize {
        self.protocol_event_count()
    }

    /// Wait for the next station lifecycle event and advance past it.
    pub fn wait_station_lifecycle_event(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<StationLifecycleEvent> {
        self.wait_station_lifecycle_event_optional(cursor, timeout)?
            .ok_or_else(|| "device did not publish the next station lifecycle event".into())
    }

    pub fn wait_station_lifecycle_event_optional(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<Option<StationLifecycleEvent>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        Ok(self
            .wait_for_protocol_cursor(cursor, timeout, |message| {
                message.boot_id == boot_id
                    && message.session_id == 0
                    && message.request_id == 0
                    && matches!(message.body, Event::StationLifecycle(_))
            })?
            .map(|message| match message.body {
                Event::StationLifecycle(event) => event,
                _ => unreachable!("lifecycle predicate accepted only lifecycle events"),
            }))
    }

    pub fn latest_boot_id(&self) -> Option<u64> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        latest_boot_id_in(&state.messages)
    }

    pub fn observed_protocol_ipv4(
        &self,
        network_interface: WifiNetworkInterface,
    ) -> Option<Ipv4Addr> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let messages = &state.messages;
        let boot_id = latest_boot_id_in(messages)?;
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

    pub fn beacon_loss_count(&self) -> usize {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        beacon_loss_count_in(&state.messages)
    }

    /// A pause must preserve the existing association, including when a
    /// reconnect happens quickly enough for the throughput floor to pass.
    pub fn require_station_unchanged_since(&self, first_event: usize) -> Result<()> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        station_unchanged_since_in(&state.messages, first_event)
    }

    pub fn require_no_beacon_loss(&self) -> Result<()> {
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
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let messages = &state.messages;
        let Some(boot_id) = latest_boot_id_in(messages) else {
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
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .messages
            .len()
    }

    pub(super) fn check_link(&self) -> Result<()> {
        oer_process::check_cancelled()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check()
    }

    pub(super) fn wait_for_protocol_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Result<Option<Envelope<Event>>> {
        let mut cursor = start;
        self.wait_for_protocol_cursor(&mut cursor, timeout, predicate)
    }

    fn wait_for_protocol_cursor(
        &self,
        cursor: &mut usize,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Result<Option<Envelope<Event>>> {
        let deadline = crate::transport::events::deadline_after(timeout);
        let mut state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            oer_process::check_cancelled()?;
            state.check()?;
            if let Some((relative, message)) = state
                .messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find(|(_, message)| predicate(message))
            {
                *cursor += relative + 1;
                return Ok(Some(message.clone()));
            }
            *cursor = state.messages.len();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let (next, _) = self
                .protocol
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
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
                        reason: oer_hil_protocol::StationDisconnectReason::BeaconLoss,
                        ..
                    })
                )
        })
        .count()
}

pub(super) fn decode_counters_are_clean(counters: DecodeCounters) -> bool {
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

pub(super) fn validate_target_link_health(health: LinkHealth) -> Result<()> {
    if health.rx_cobs_errors != 0
        || health.rx_checksum_errors != 0
        || health.rx_decode_errors != 0
        || health.rx_overflows != 0
        || health.tx_dropped != 0
    {
        return Err(LinkError::protocol(format!(
            "target reported unhealthy serialized console transport: {health:?}"
        ))
        .into());
    }
    Ok(())
}

pub(super) fn station_unchanged_since_in(
    messages: &[Envelope<Event>],
    first_event: usize,
) -> Result<()> {
    let subsequent = messages
        .get(first_event..)
        .ok_or("station lifecycle cursor exceeds captured events")?;
    let boot_id = latest_boot_id_in(&messages[..first_event])
        .filter(|boot_id| *boot_id != 0)
        .ok_or("station lifecycle cursor has no established boot identity")?;
    for message in subsequent {
        if message.boot_id != boot_id {
            return Err("device rebooted during station pause workload".into());
        }
        match &message.body {
            // GetCapabilities replies use Hello too. Only a solicited reply
            // from the established boot can occur inside an unchanged epoch.
            Event::Hello(_) if message.request_id == 0 => {
                return Err("device restarted its greeting during station pause workload".into());
            }
            Event::StationLifecycle(event) => {
                return Err(format!("station changed during pause workload: {event:?}").into());
            }
            _ => {}
        }
    }
    Ok(())
}

/// A drained profile: the target's status and each hart's raw `(pc, ra)`
/// samples.
pub(crate) type DrainedProfile = (oer_hil_protocol::ProfileStatus, [Vec<(u32, u32)>; 2]);
