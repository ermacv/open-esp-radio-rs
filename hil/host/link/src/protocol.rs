use oer_hil_protocol::base::RejectReason;
use oer_hil_protocol::base::{
    GetBootStatus, GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints, Hello,
    ImageKeyPage, PostMortemCheckpoints, Rejected,
};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest};
use oer_hil_protocol::{Endpoint, Message};

use super::*;
use oer_hil_image_class::DeviceImageKeys;

#[derive(serde::Serialize)]
pub struct Observation {
    boot_id: u64,
    capabilities: DeviceImageKeys,
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

    /// The capabilities of whatever boot runs, found without knowing it.
    pub fn discover(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        let reply = self.exchange(0, 0, GetHello, timeout)?;
        let Some(hello) = reply.decode::<Hello>().filter(|_| reply.boot_id != 0) else {
            return Err(format!(
                "device answered read-only discovery with {}; firmware must support boot discovery",
                reply.path()
            )
            .into());
        };
        self.image_keys_of(reply.boot_id, hello, timeout)
    }

    /// Ask a boot whose Hello the link lost for it again.
    ///
    /// The USB Serial/JTAG can drop the first bytes of a frame, so the
    /// boot's unsolicited Hello never decodes while the boot runs and
    /// answers. Only a capture that began at a reset, whose console already
    /// shows a boot starting and which has seen no boot yet, may begin that
    /// boot with the answer, and only while the answer is among the boot's
    /// first messages; the capture then records the solicited Hello.
    /// Otherwise the missing Hello stands as the failure.
    fn solicit_lost_hello(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        const MISSING: &str = "device did not publish a HIL protocol hello";
        {
            let booted = console_shows_boot(
                &self
                    .bytes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            let mut state = self
                .protocol
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.health.origin != CaptureOrigin::Boot
                || state.health.boot_id.is_some()
                || !booted
            {
                return Err(MISSING.into());
            }
            state.health.accept_solicited_hello = true;
        }
        let answer = self.exchange(0, 0, GetHello, timeout);
        let solicited = {
            let mut state = self
                .protocol
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.health.accept_solicited_hello = false;
            state.health.solicited_hello
        };
        let answer =
            answer.map_err(|error| format!("{MISSING}, nor answered a hello request: {error}"))?;
        match (answer.decode::<Hello>(), solicited) {
            (Some(hello), Some(solicited)) => {
                eprintln!(
                    "hil: the link lost the boot's hello; its answer to the hello request at \
                     target message {} began the boot",
                    solicited.message_sequence
                );
                self.image_keys_of(answer.boot_id, hello, timeout)
            }
            _ => Err(format!("{MISSING}, and its hello answer began no fresh boot").into()),
        }
    }

    /// The capability set `hello` of boot `boot_id` announces, page by page.
    fn image_keys_of(
        &self,
        boot_id: u64,
        hello: Hello,
        timeout: Duration,
    ) -> Result<DeviceImageKeys> {
        let mut pages = Vec::new();
        let mut first = 0_u16;
        while first < hello.keys {
            let page: ImageKeyPage =
                self.request_to(boot_id, 0, GetImageKeys { first }, timeout)?;
            if page.first != first || page.keys.is_empty() {
                return Err(format!("invalid capability page {first}").into());
            }
            first += page.keys.len() as u16;
            pages.push(page);
        }
        Ok(DeviceImageKeys::assemble(hello, &pages)?)
    }

    pub fn inspect_stack_usage(&self, timeout: Duration) -> Result<Option<StackUsage>> {
        match self.call(0, oer_hil_protocol::system::GetStacks, timeout)? {
            Ok(oer_hil_protocol::system::Stacks(stack)) => Ok(Some(stack)),
            Err(oer_hil_protocol::base::RejectReason::InvalidState) => Ok(None),
            Err(reason) => Err(format!("device rejected stack observation: {reason:?}").into()),
        }
    }

    /// The current image's capabilities, from its boot's Hello.
    pub fn request_image_keys(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        let Some(hello) =
            self.wait_for_message_after(0, timeout, |message| message.is::<Hello>())?
        else {
            return self.solicit_lost_hello(timeout);
        };
        let announced = hello.decode::<Hello>().ok_or("undecodable hello")?;
        self.image_keys_of(hello.boot_id, announced, timeout)
    }

    /// Establishes the typed link and provisions this boot from host-owned
    /// local configuration. The passphrase is never echoed by the target or
    /// appended to the UART capture.
    fn prepare_protocol(
        &self,
        target: Target<'_>,
    ) -> Result<(DeviceImageKeys, Option<StartupArtifactStatus>)> {
        let capabilities = self.request_image_keys(PROTOCOL_READY_TIMEOUT)?;
        let artifact_path = target.dut.startup_artifact();
        if artifact_path.is_some() && !capabilities.has::<oer_hil_protocol::phy::StartupArtifact>()
        {
            return Err("firmware does not support a host-owned startup artifact".into());
        }
        if !capabilities.has::<oer_hil_protocol::network::DataPlanePlacement>() {
            return Err("firmware does not support explicit data-plane placement".into());
        }
        let artifact_event_start = self.protocol_event_count();
        if capabilities.has::<oer_hil_protocol::phy::StartupArtifact>()
            && let Some(path) = artifact_path
            && let Some(bytes) = crate::startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
            target.dut.journal(DutEvent::StartupArtifactUploaded {
                path: path.display().to_string(),
                sha256: oer_hil_durable::sha256_bytes(&bytes),
            });
        }
        if capabilities.has::<oer_hil_protocol::wifi::RuntimeInitialization>() {
            self.initialize(target, PROTOCOL_READY_TIMEOUT)?;
        }
        let startup_artifact_status = if capabilities
            .has::<oer_hil_protocol::phy::StartupArtifact>()
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
            crate::startup_artifact::persist_atomically(path, &bytes)?;
            target.dut.journal(DutEvent::StartupArtifactWritten {
                path: path.display().to_string(),
                sha256: oer_hil_durable::sha256_bytes(&bytes),
                disposition: format!("{:?}", status.disposition),
            });
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
        let (_, oer_hil_protocol::phy::StartupArtifactReady(status)) = self
            .wait_for_after(
                start,
                timeout,
                |_, _: &oer_hil_protocol::phy::StartupArtifactReady| true,
            )?
            .ok_or("device did not report startup artifact initialization status")?;
        Ok(status)
    }

    fn upload_startup_artifact(&self, bytes: &[u8], timeout: Duration) -> Result<()> {
        for chunk in crate::startup_artifact::chunks(bytes)? {
            match self.call(
                0,
                oer_hil_protocol::phy::UploadStartupArtifact(chunk),
                timeout,
            )? {
                Ok(oer_hil_protocol::base::Accepted) => {}
                Err(reason) => {
                    return Err(format!("device rejected HIL startup artifact: {reason:?}").into());
                }
            }
        }
        Ok(())
    }

    fn wait_for_startup_artifact_after(&self, start: usize, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = crate::transport::events::deadline_after(timeout);
        let mut cursor = start;
        let mut assembler = crate::startup_artifact::Assembler::new();
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
            .wait_for_cursor(
                cursor,
                deadline.saturating_duration_since(Instant::now()),
                |_, _: &oer_hil_protocol::phy::StartupArtifactPart| true,
            )?
            .map(|(_, oer_hil_protocol::phy::StartupArtifactPart(chunk))| chunk))
    }

    fn initialize(&self, target: Target<'_>, timeout: Duration) -> Result<()> {
        let first_event = self.protocol_event_count();
        let (reply, outcome) = self.call_current(
            0,
            oer_hil_protocol::wifi::Initialize(
                oer_hil_protocol::wifi::InitializationConfiguration {
                    ap_scheduler: target.settings.ap_scheduler,
                    ipv4: target.station.ipv4(),
                    data_plane: target.settings.data_plane,
                    rx_checksum: target.settings.rx_checksum,
                    tx_udp_checksum: target.settings.tx_udp_checksum,
                    tx_buffer: target.settings.tx_buffer,
                    rx_continuation: target.settings.rx_continuation,
                    l1_cache_counters: target.settings.l1_cache_counters,
                },
            ),
            timeout,
        )?;
        if let Err(reason) = outcome {
            return Err(format!("device rejected HIL initialization: {reason:?}").into());
        }
        let request_id = reply.request_id;
        self.wait_for_after(
            first_event,
            timeout,
            |message, _: &oer_hil_protocol::wifi::Initialized| message.request_id == request_id,
        )?
        .ok_or_else(|| "device did not complete role-neutral initialization".into())
        .map(|_| ())
    }

    /// Establish the typed link and run role-neutral initialization,
    /// exchanging the configured startup artifact, without starting a role.
    pub fn prepare_startup(
        &self,
        target: Target<'_>,
    ) -> Result<(DeviceImageKeys, Option<StartupArtifactStatus>)> {
        self.prepare_protocol(target)
    }

    /// Initialize and submit a real station start without assuming that an AP
    /// exists. The caller must observe its lifecycle and terminal outcome.
    pub fn begin_station_attempt(
        &self,
        target: Target<'_>,
    ) -> Result<(DeviceImageKeys, WifiCommandHandle)> {
        let (capabilities, _) = self.prepare_protocol(target)?;
        let handle = self.request_station_start(target)?;
        Ok((capabilities, handle))
    }

    pub fn prepare_station(
        &self,
        target: Target<'_>,
        timeout: Duration,
    ) -> Result<DeviceImageKeys> {
        self.prepare_station_with_startup_artifact_status(target, timeout)
            .map(|(capabilities, _)| capabilities)
    }

    pub fn prepare_station_with_startup_artifact_status(
        &self,
        target: Target<'_>,
        timeout: Duration,
    ) -> Result<(DeviceImageKeys, Option<StartupArtifactStatus>)> {
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
        request: oer_hil_protocol::phy::PhyRegisterImageRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::phy::PhyRegisterImageWords> {
        match self.call(
            0,
            oer_hil_protocol::phy::ReadRegisterImage(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::phy::RegisterImageWords(words)) => Ok(words),
            Err(reason) => Err(format!("device rejected a register image read: {reason:?}").into()),
        }
    }

    /// Read a window of the target's analog image.
    pub fn read_phy_analog_image(
        &self,
        request: oer_hil_protocol::phy::PhyRegisterImageRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::phy::PhyAnalogImageBytes> {
        match self.call(0, oer_hil_protocol::phy::ReadAnalogImage(request), timeout)? {
            Ok(oer_hil_protocol::phy::AnalogImageBytes(bytes)) => Ok(bytes),
            Err(reason) => Err(format!("device rejected an analog image read: {reason:?}").into()),
        }
    }

    pub fn query_operation_status(&self, timeout: Duration) -> Result<OperationStatus> {
        match self.call(0, oer_hil_protocol::network::GetStatus, timeout)? {
            Ok(oer_hil_protocol::network::Status(status)) => Ok(status),
            Err(reason) => {
                Err(format!("device rejected operation-status query: {reason:?}").into())
            }
        }
    }

    /// Sends `body` to the current boot and returns its response, or the
    /// reason the device refused it.
    pub fn call<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before request")?;
        self.call_to(boot_id, session_id, body, timeout)
    }

    /// Sends `body` to boot `boot_id`; see [`Self::call`].
    fn call_to<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        Ok(self.call_identified(boot_id, session_id, body, timeout)?.1)
    }

    /// [`Self::call`], with the reply's header: its request identifier is
    /// the one the device's later messages about this request carry.
    fn call_current<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<(Received, std::result::Result<E::Response, RejectReason>)> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before request")?;
        self.call_identified(boot_id, session_id, body, timeout)
    }

    /// [`Self::call_to`], with the reply's header.
    fn call_identified<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<(Received, std::result::Result<E::Response, RejectReason>)> {
        let reply = self.exchange(boot_id, session_id, body, timeout)?;
        if let Some(response) = reply.decode::<E::Response>() {
            return Ok((reply, Ok(response)));
        }
        match reply.decode::<Rejected>() {
            Some(Rejected(reason)) => Ok((reply, Err(reason))),
            None => Err(format!("device answered {} with {}", E::PATH, reply.path()).into()),
        }
    }

    /// Sends `body` to the current boot and returns its response; a refusal
    /// is an error.
    pub fn request<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call(session_id, body, timeout)?
            .map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH).into())
    }

    fn request_to<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call_to(boot_id, session_id, body, timeout)?
            .map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH).into())
    }

    /// Sends `body` to boot `boot_id` (0: whichever boot runs) and returns
    /// the device's reply to it.
    fn exchange<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<Received> {
        self.check_link()?;
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
                path: E::PATH,
                host_sent_unix_micros: super::host_unix_micros(),
            });
        // Other messages about this request, such as the evidence a replay
        // publishes before its terminal response, are not its reply.
        self.wait_for_message_after(event_count, timeout, |message| {
            command_response_matches(
                message,
                if boot_id == 0 {
                    message.boot_id
                } else {
                    boot_id
                },
                session_id,
                request_id,
            ) && (message.is::<E::Response>() || message.is::<Rejected>())
        })?
        .ok_or_else(|| format!("device did not answer {}", E::PATH).into())
    }

    fn expect_accepted<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
        &self,
        session_id: u64,
        body: E,
        operation: &str,
    ) -> Result<()> {
        self.call(session_id, body, PROTOCOL_READY_TIMEOUT)
            .map_err(|error| {
                crate::error::context(format!("session {operation} command failed"), error)
            })?
            .map(|_| ())
            .map_err(|reason| format!("device rejected session {operation}: {reason:?}").into())
    }

    pub fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        self.start_session_with_id(session_id, config)
    }

    /// Reserve one exact current-boot/session identity before Configure so
    /// both UDP payload directions can be correlated to this lifecycle stage.
    pub fn start_identified_udp_session(
        &self,
        build: impl FnOnce(oer_hil_protocol::network::UdpSessionPayloadIdentity) -> SessionConfig,
    ) -> Result<(
        SessionHandle,
        oer_hil_protocol::network::UdpSessionPayloadIdentity,
    )> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device omitted the current boot identity")?;
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let identity = oer_hil_protocol::network::UdpSessionPayloadIdentity::new(
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
            oer_hil_protocol::network::Configure(config),
            "configuration",
        )?;
        self.expect_accepted(session_id, oer_hil_protocol::network::Arm, "arm")?;
        self.expect_accepted(session_id, oer_hil_protocol::network::Start, "start")?;
        let expected_directions: &[Direction] = match direction {
            Direction::Rx => &[Direction::Rx],
            Direction::Tx => &[Direction::Tx],
            Direction::Bidirectional => &[Direction::Rx, Direction::Tx],
        };
        for expected in expected_directions {
            self.wait_for_session_event(
                handle,
                PROTOCOL_READY_TIMEOUT,
                |_, reported: &oer_hil_protocol::network::SessionReady| {
                    session_ready_covers(direction, *reported, *expected, link_requirements)
                },
            )?
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

    /// The first `M` of `session` that `accept` takes; the session's failure
    /// ends the wait as an error.
    fn wait_for_session_event<M: Message>(
        &self,
        session: SessionHandle,
        timeout: Duration,
        accept: impl Fn(&Received, &M) -> bool,
    ) -> Result<Option<(Received, M)>> {
        let found = self.wait_for_message_after(session.first_event, timeout, |message| {
            message.session_id == session.session_id
                && (message
                    .decode::<M>()
                    .is_some_and(|body| accept(message, &body))
                    || message.is::<oer_hil_protocol::network::Failed>())
        })?;
        let Some(message) = found else {
            return Ok(None);
        };
        if let Some(oer_hil_protocol::network::Failed(reason)) = message.decode() {
            return Err(format!("target session {} failed: {reason:?}", session.session_id).into());
        }
        let body = message.decode::<M>().expect("the accepted message decodes");
        Ok(Some((message, body)))
    }

    /// The first evidence record of `session` that `pick` takes.
    fn session_evidence<T>(
        &self,
        session: SessionHandle,
        timeout: Duration,
        pick: impl Fn(EvidenceRecord) -> Option<T>,
    ) -> Result<Option<T>> {
        Ok(self
            .wait_for_session_event(
                session,
                timeout,
                |_, oer_hil_protocol::network::Evidence(record)| pick(*record).is_some(),
            )?
            .and_then(|(_, oer_hil_protocol::network::Evidence(record))| pick(record)))
    }

    pub fn wait_for_udp_rx_started(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<u64> {
        let (_, oer_hil_protocol::network::UdpRxStarted { datagrams }) = self
            .wait_for_session_event(
                session,
                timeout,
                |_, oer_hil_protocol::network::UdpRxStarted { datagrams }| *datagrams == 256,
            )?
            .ok_or("device did not confirm UDP delivery before maintenance")?;
        Ok(datagrams)
    }

    pub fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        let deadline = crate::transport::events::deadline_after(timeout);
        let remaining = || deadline.saturating_duration_since(Instant::now());
        let transport = self
            .session_evidence(session, remaining(), |record| match record {
                EvidenceRecord::Transport(transport) => Some(transport),
                _ => None,
            })?
            .ok_or("device did not publish structured session evidence")?;
        let mut flow_transport = [None; SESSION_FLOW_CAPACITY];
        for (index, flow_id) in session.flow_ids.into_iter().enumerate() {
            let Some(flow_id) = flow_id else {
                continue;
            };
            flow_transport[index] = Some(
                self.session_evidence(session, remaining(), |record| match record {
                    EvidenceRecord::FlowTransport(flow) if flow.flow_id == flow_id => Some(flow),
                    _ => None,
                })?
                .ok_or_else(|| {
                    format!("device did not publish transport evidence for flow {flow_id}")
                })?,
            );
        }
        let flow_total = TransportEvidence::from_flows(flow_transport);
        if flow_total != transport {
            return Err(format!(
                "per-flow transport total does not match session aggregate: flows={flow_total:?} aggregate={transport:?}"
            )
            .into());
        }
        let link = self
            .session_evidence(session, remaining(), |record| match record {
                EvidenceRecord::Link(link) => Some(link),
                _ => None,
            })?
            .ok_or("device did not publish structured protocol-link evidence")?;
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
        let stack = self
            .session_evidence(session, remaining(), |record| match record {
                EvidenceRecord::Stack(stack) => Some(stack),
                _ => None,
            })?
            .ok_or("device did not publish structured stack evidence")?;
        validate_stack_usage(stack)?;
        let (_, finished) = self
            .wait_for_session_event(
                session,
                remaining(),
                |_, _: &oer_hil_protocol::network::Finished| true,
            )?
            .ok_or("device did not finish the structured HIL session")?;
        let radio = self.session_evidence(session, Duration::ZERO, |record| match record {
            EvidenceRecord::Radio(radio) => Some(radio),
            _ => None,
        })?;
        let tx_timing = self.session_evidence(session, Duration::ZERO, |record| match record {
            EvidenceRecord::TxAggregateTiming(timing) => Some(timing),
            _ => None,
        })?;
        let rx_delivery =
            self.session_evidence(session, Duration::ZERO, |record| match record {
                EvidenceRecord::RxDelivery(delivery) => Some(delivery),
                _ => None,
            })?;
        let network_scheduler =
            self.session_evidence(session, Duration::ZERO, |record| match record {
                EvidenceRecord::NetworkScheduler(evidence) => Some(evidence),
                _ => None,
            })?;
        let rx_zero_copy =
            self.session_evidence(session, Duration::ZERO, |record| match record {
                EvidenceRecord::RxZeroCopy(evidence) => Some(evidence),
                _ => None,
            })?;
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
        if let Err(reason) = self.call(
            session.session_id,
            oer_hil_protocol::network::ReplayResult,
            PROTOCOL_READY_TIMEOUT,
        )? {
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
            oer_hil_protocol::network::AcknowledgeResult,
            "acknowledgement",
        )
    }

    pub fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
        let first_event = self.protocol_event_count();
        let (reply, outcome) = self.call_current(
            0,
            oer_hil_protocol::wifi::CycleStationEpoch,
            PROTOCOL_READY_TIMEOUT,
        )?;
        outcome.map_err(|reason| format!("device rejected station epoch cycle: {reason:?}"))?;
        Ok(StationEpochHandle {
            request_id: reply.request_id,
            first_event,
        })
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
            .filter(|message| message.request_id == handle.request_id)
            .find_map(|message| {
                message
                    .decode()
                    .map(|oer_hil_protocol::wifi::StationEpochCompleted(evidence)| evidence)
            })
    }

    fn request_wifi_command<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
        &self,
        command: E,
        operation: &str,
    ) -> Result<WifiCommandHandle> {
        let first_event = self.protocol_event_count();
        let (reply, outcome) = self.call_current(0, command, PROTOCOL_READY_TIMEOUT)?;
        outcome.map_err(|reason| format!("device rejected {operation}: {reason:?}"))?;
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let accepted_offset = state.messages[first_event..]
            .iter()
            .position(|message| {
                message.boot_id == reply.boot_id
                    && message.session_id == 0
                    && message.request_id == reply.request_id
                    && message.message_sequence == reply.message_sequence
            })
            .ok_or("accepted Wi-Fi command disappeared from the capture")?;
        Ok(WifiCommandHandle {
            boot_id: reply.boot_id,
            request_id: reply.request_id,
            first_event: first_event + accepted_offset + 1,
        })
    }

    pub fn request_station_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(oer_hil_protocol::wifi::StopStation, "station stop")
    }

    pub fn request_radio_restart(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(oer_hil_protocol::wifi::RestartRadio, "idle radio restart")
    }

    pub fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage> {
        match self.call(0, oer_hil_protocol::system::GetStacks, timeout)? {
            Ok(oer_hil_protocol::system::Stacks(usage)) => {
                validate_stack_usage(usage)?;
                Ok(usage)
            }
            Err(reason) => Err(format!("device rejected stack-usage query: {reason:?}").into()),
        }
    }

    pub fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        self.request(0, GetLinkHealth, timeout)
    }

    pub fn probe_memory_benchmark(
        &self,
        request: oer_hil_protocol::system::MemoryBenchmarkRequest,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::system::MemoryBenchmarkEvidence> {
        match self.call(
            0,
            oer_hil_protocol::system::RunMemoryBenchmark(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::system::MemoryBenchmarkCompleted(evidence)) => Ok(evidence),
            Err(reason) => Err(format!("device rejected memory benchmark: {reason:?}").into()),
        }
    }

    pub fn bluetooth_dtm(
        &self,
        operation: oer_hil_protocol::bluetooth::BluetoothDtmOperation,
    ) -> Result<oer_hil_protocol::bluetooth::BluetoothDtmEvidence> {
        match self.call(
            0,
            oer_hil_protocol::bluetooth::RunDtm(operation),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::bluetooth::DtmResult(evidence))
                if evidence.completed(operation) =>
            {
                Ok(evidence)
            }
            response => Err(format!("Bluetooth {operation:?} failed: {response:?}").into()),
        }
    }

    /// One raw HCI exchange with the Controller of a `bluetooth_hci` image.
    pub fn bluetooth_hci(
        &self,
        request: oer_hil_protocol::bluetooth::BluetoothHciRequest,
    ) -> Result<oer_hil_protocol::bluetooth::BluetoothHciResponse> {
        let wait = match &request {
            oer_hil_protocol::bluetooth::BluetoothHciRequest::NextEvent { wait_ms } => {
                Duration::from_millis(u64::from(*wait_ms))
            }
            oer_hil_protocol::bluetooth::BluetoothHciRequest::Command { .. } => Duration::ZERO,
        };
        match self.call(
            0,
            oer_hil_protocol::bluetooth::ExchangeHci(request),
            Duration::from_secs(5) + wait,
        )? {
            Ok(oer_hil_protocol::bluetooth::HciResponse(response)) => Ok(response),
            response => Err(format!("Bluetooth HCI exchange rejected: {response:?}").into()),
        }
    }

    /// The standalone GATT image's observation, accepted only with its
    /// single-core Bluetooth IRQ stack evidence.
    pub fn bluetooth_gatt(&self) -> Result<oer_hil_protocol::bluetooth::BluetoothGattEvidence> {
        let evidence = self.bluetooth_gatt_observation()?;
        self.require_bluetooth_irq_stack()?;
        Ok(evidence)
    }

    /// The GATT application's observation without an image-specific stack
    /// policy; the joint Wi-Fi/Bluetooth image reports its stacks through the
    /// Wi-Fi evidence instead.
    pub fn bluetooth_gatt_observation(
        &self,
    ) -> Result<oer_hil_protocol::bluetooth::BluetoothGattEvidence> {
        match self.call(
            0,
            oer_hil_protocol::bluetooth::GetGatt,
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::GattState(evidence)) => Ok(evidence),
            response => Err(format!("invalid GATT observation: {response:?}").into()),
        }
    }

    pub fn bluetooth_secure_gatt(
        &self,
    ) -> Result<oer_hil_protocol::bluetooth::BluetoothSecureGattEvidence> {
        let evidence = self.bluetooth_secure_gatt_snapshot()?;
        self.require_bluetooth_irq_stack()?;
        Ok(evidence)
    }

    /// Protocol observation only. IRQ watermark scanning masks interrupts;
    /// callers selecting this path must explicitly own their sampling policy.
    pub fn bluetooth_secure_gatt_snapshot(
        &self,
    ) -> Result<oer_hil_protocol::bluetooth::BluetoothSecureGattEvidence> {
        match self.call(
            0,
            oer_hil_protocol::bluetooth::GetSecureGatt,
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::SecureGattState(evidence)) => Ok(evidence),
            response => Err(format!("invalid secure GATT observation: {response:?}").into()),
        }
    }

    pub fn restart_bluetooth_gatt(&self, boot: u64, epoch: u32) -> Result<()> {
        if boot == 0 {
            return Err("secure GATT restart requires the observed boot identity".into());
        }
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::RestartGatt { epoch },
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
                if e.epoch == epoch && e.restarting =>
            {
                Ok(())
            }
            response => Err(format!("secure GATT restart rejected: {response:?}").into()),
        }
    }

    pub fn fail_next_bluetooth_gatt_bond_load(&self, boot: u64, epoch: u32) -> Result<()> {
        if boot == 0 {
            return Err("bond load fault requires observed boot identity".into());
        }
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::FailNextGattBondLoad { epoch },
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
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
        use oer_hil_protocol::bluetooth::BluetoothGattResetReadGate as Phase;
        if boot == 0 {
            return Err("Reset gate requires observed boot identity".into());
        }
        let expected = if release {
            Phase::Released
        } else {
            Phase::Armed
        };
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::GattResetReadGate { epoch, release },
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
                if e.epoch == epoch && e.reset_read_gate == expected =>
            {
                Ok(())
            }
            response => Err(format!("Reset reader gate rejected: {response:?}").into()),
        }
    }

    pub fn require_bluetooth_gatt_restart_rejected(&self, boot: u64, epoch: u32) -> Result<()> {
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::RestartGatt { epoch },
            Duration::from_secs(2),
        )? {
            Err(oer_hil_protocol::base::RejectReason::InvalidState) => Ok(()),
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
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::FailGattResetRead { epoch },
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
                if e.epoch == epoch
                    && e.reset_read_gate
                        == oer_hil_protocol::bluetooth::BluetoothGattResetReadGate::FailureRequested =>
            {
                Ok(())
            }
            response => Err(format!("Reset read fault rejected: {response:?}").into()),
        }
    }

    pub fn confirm_bluetooth_gatt(
        &self,
        boot: u64,
        decision: oer_hil_protocol::bluetooth::BluetoothNumericDecision,
    ) -> Result<()> {
        if boot == 0 {
            return Err("Numeric Comparison requires the displayed boot identity".into());
        }
        match self.call_to(
            boot,
            0,
            oer_hil_protocol::bluetooth::ConfirmGatt(decision),
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::bluetooth::GattDecisionRecorded(recorded))
                if recorded == decision =>
            {
                Ok(())
            }
            response => Err(format!("Numeric Comparison decision rejected: {response:?}").into()),
        }
    }

    pub fn boot_status(&self) -> Result<oer_hil_protocol::base::BootEvidence> {
        self.request(0, GetBootStatus, Duration::from_secs(5))
    }

    /// The previous boot's post-mortem checkpoints, oldest first: `count`
    /// of them, as its boot evidence reports, fetched page by page.
    pub fn post_mortem_checkpoints(
        &self,
        count: u8,
    ) -> Result<Vec<oer_hil_protocol::base::Checkpoint>> {
        let mut checkpoints = Vec::with_capacity(usize::from(count));
        while checkpoints.len() < usize::from(count) {
            let first = checkpoints.len() as u8;
            let page: PostMortemCheckpoints = self.request(
                0,
                GetPostMortemCheckpoints { first },
                Duration::from_secs(5),
            )?;
            if page.first != first || page.checkpoints.is_empty() {
                return Err(format!("invalid post-mortem page {first}: {page:?}").into());
            }
            checkpoints.extend(page.checkpoints);
        }
        checkpoints.truncate(usize::from(count));
        Ok(checkpoints)
    }

    /// Arm the program-counter profile once the boot's hello arrived; the
    /// capture drains it into `profile.json` when it finishes.
    pub fn profiled(mut self, profile: oer_hil_scenario::ProfileRequest) -> Result<Self> {
        self.wait_for_message_after(0, PROTOCOL_READY_TIMEOUT, |message| message.is::<Hello>())?
            .ok_or("device did not publish a HIL protocol hello before the profile")?;
        match self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(profile.control()),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::telemetry::ProfileState(status)) if status.armed => {}
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
        let status = match self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(
                oer_hil_protocol::telemetry::ProfileControl::Status,
            ),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::telemetry::ProfileState(status)) => status,
            response => return Err(format!("profile status rejected: {response:?}").into()),
        };
        let mut samples = [Vec::new(), Vec::new()];
        if !status.open {
            for (hart, pairs) in samples.iter_mut().enumerate() {
                while (pairs.len() as u32) < status.samples[hart] {
                    let first = pairs.len() as u32;
                    match self.call(
                        0,
                        oer_hil_protocol::telemetry::GetProfileSamples {
                            hart: hart as u8,
                            first,
                        },
                        Duration::from_secs(5),
                    )? {
                        Ok(oer_hil_protocol::telemetry::ProfileSamples(page))
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
        let _ = self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(
                oer_hil_protocol::telemetry::ProfileControl::Disarm,
            ),
            Duration::from_secs(5),
        );
        Ok((status, samples))
    }

    /// Report, start or re-mask the target's event trace.
    pub fn trace_control(
        &self,
        control: oer_hil_protocol::telemetry::TraceControl,
    ) -> Result<oer_hil_protocol::telemetry::TraceStatus> {
        match self.call(
            0,
            oer_hil_protocol::telemetry::ControlTrace(control),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::telemetry::TraceState(status)) => Ok(status),
            response => Err(format!("trace control {control:?} rejected: {response:?}").into()),
        }
    }

    /// Every complete trace entry in storage, in storage order.
    pub fn trace_entries(
        &self,
        slots: u16,
    ) -> Result<Vec<oer_hil_protocol::telemetry::TraceEntry>> {
        let mut entries = Vec::new();
        let mut first = 0;
        while first < slots {
            match self.call(
                0,
                oer_hil_protocol::telemetry::GetTraceEntries { first },
                Duration::from_secs(5),
            )? {
                Ok(oer_hil_protocol::telemetry::TraceEntriesPage(page))
                    if page.first == first && page.next > first =>
                {
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
    ) -> Result<Option<(oer_hil_protocol::telemetry::TraceSnapshotPage, Vec<u32>)>> {
        let mut words = Vec::new();
        let mut header = None;
        loop {
            let offset = u16::try_from(words.len())?;
            match self.call(
                0,
                oer_hil_protocol::telemetry::GetTraceSnapshot { slot, offset },
                Duration::from_secs(5),
            )? {
                Ok(oer_hil_protocol::telemetry::TraceSnapshot(None)) => return Ok(None),
                Ok(oer_hil_protocol::telemetry::TraceSnapshot(Some(page)))
                    if page.offset == offset =>
                {
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
    pub fn inject_hang(&self, target: oer_hil_protocol::system::HangTarget) -> Result<()> {
        match self.call(
            0,
            oer_hil_protocol::system::InjectHang(target),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::system::HangInjected(observed)) if observed == target => Ok(()),
            response => Err(format!("hang injection {target:?} rejected: {response:?}").into()),
        }
    }

    /// Ask the panic-reset image to panic; it acknowledges first.
    pub fn inject_panic(&self) -> Result<()> {
        match self.call(
            0,
            oer_hil_protocol::system::InjectPanic,
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::system::PanicInjected) => Ok(()),
            response => Err(format!("panic injection rejected: {response:?}").into()),
        }
    }

    /// Ask the USB Serial/JTAG-off image to switch its USB off its pads; it
    /// acknowledges first.
    pub fn disable_usb(&self) -> Result<()> {
        match self.call(
            0,
            oer_hil_protocol::system::DisableUsb,
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::system::UsbDisabled) => Ok(()),
            response => Err(format!("USB switch-off rejected: {response:?}").into()),
        }
    }

    /// Console bytes captured so far: a mark for
    /// [`Self::console_shows_boot_since`].
    pub fn console_length(&self) -> usize {
        self.bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether the console bytes after `mark` show a chip starting: the ROM
    /// banner or the bootloader's lines.
    pub fn console_shows_boot_since(&self, mark: usize) -> bool {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        console_shows_boot(bytes.get(mark..).unwrap_or_default())
    }

    pub fn system_watchdog_test(
        &self,
        mode: oer_hil_protocol::system::WatchdogTestMode,
    ) -> Result<()> {
        let WatchdogArmed(armed) = self.request(0, WatchdogTest(mode), Duration::from_secs(5))?;
        if armed != mode {
            return Err(format!("watchdog {mode:?} armed as {armed:?}").into());
        }
        Ok(())
    }

    pub fn phy_fault(
        &self,
        command: oer_hil_protocol::phy::PhyFaultCommand,
    ) -> Result<oer_hil_protocol::phy::PhyFaultEvidence> {
        match self.call(
            0,
            oer_hil_protocol::phy::ControlFault(command),
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::phy::FaultState(evidence)) => Ok(evidence),
            response => Err(format!("PHY fault control {command:?} rejected: {response:?}").into()),
        }
    }

    /// Suspend, resume or report the shared PHY's periodic tracking timer.
    pub fn phy_tracking(
        &self,
        command: oer_hil_protocol::phy::PhyTrackingCommand,
    ) -> Result<oer_hil_protocol::phy::PhyTrackingEvidence> {
        match self.call(
            0,
            oer_hil_protocol::phy::ControlTracking(command),
            Duration::from_secs(2),
        )? {
            Ok(oer_hil_protocol::phy::TrackingState(evidence)) => Ok(evidence),
            response => {
                Err(format!("PHY tracking control {command:?} rejected: {response:?}").into())
            }
        }
    }

    pub fn require_bluetooth_irq_stack(&self) -> Result<()> {
        match self.call(
            0,
            oer_hil_protocol::system::GetInterruptStacks,
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::system::InterruptStacks { cpu0, cpu1 }) => {
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
        match self.call(0, oer_hil_protocol::system::ProbeTimebase(request), timeout)? {
            Ok(oer_hil_protocol::system::TimebaseProbed(evidence)) => Ok(evidence),
            Err(reason) => Err(format!("device rejected timebase probe: {reason:?}").into()),
        }
    }

    pub fn call_across_cores(
        &self,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::system::CoresCalled> {
        match self.call(0, oer_hil_protocol::system::CallAcrossCores, timeout)? {
            Ok(called) => Ok(called),
            Err(reason) => Err(format!("device rejected the cross-core call: {reason:?}").into()),
        }
    }

    pub fn probe_source_gate(
        &self,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::system::SourceGated> {
        match self.call(0, oer_hil_protocol::system::ProbeSourceGate, timeout)? {
            Ok(gated) => Ok(gated),
            Err(reason) => Err(format!("device rejected the source-gate probe: {reason:?}").into()),
        }
    }

    pub fn probe_ieee802154_event_status(
        &self,
        request: Ieee802154EventStatusProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154EventStatusProbeEvidence> {
        // `call` admits only a reply from this boot, session and
        // request ID; this match then admits only the probe's typed event.
        match self.call(
            0,
            oer_hil_protocol::ieee802154::ProbeEventStatus(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::EventStatusProbed(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 EVENT_STATUS probe: {reason:?}").into())
            }
        }
    }

    pub fn probe_ieee802154_ed_event(
        &self,
        request: Ieee802154EdEventProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154EdEventProbeEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::ProbeEdEvent(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::EdEventProbed(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 ED event probe: {reason:?}").into())
            }
        }
    }

    pub fn probe_ieee802154_route(
        &self,
        request: Ieee802154RouteProbeRequest,
        timeout: Duration,
    ) -> Result<Ieee802154RouteProbeEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::ProbeRoute(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::RouteProbed(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected the IEEE 802.15.4 route probe: {reason:?}").into())
            }
        }
    }

    pub fn run_ieee802154_air_check(
        &self,
        request: Ieee802154AirCheckRequest,
        timeout: Duration,
    ) -> Result<Ieee802154AirCheckEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::RunAirCheck(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::AirCheckCompleted(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 air check: {reason:?}").into())
            }
        }
    }

    pub fn start_ieee802154_session(
        &self,
        config: Ieee802154SessionConfig,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::StartSession(config),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionStarted(result)) => Ok(result),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session start: {reason:?}").into())
            }
        }
    }

    pub fn transmit_ieee802154_session(
        &self,
        request: Ieee802154SessionTransmitRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionTransmitEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::TransmitSession(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionTransmitted(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session transmit: {reason:?}").into())
            }
        }
    }

    fn ieee802154_session_accepted<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
        &self,
        command: E,
        what: &str,
    ) -> Result<()> {
        match self.call(0, command, Duration::from_secs(5))? {
            Ok(oer_hil_protocol::base::Accepted) => Ok(()),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session {what}: {reason:?}").into())
            }
        }
    }

    pub fn receive_ieee802154_session(&self) -> Result<()> {
        self.ieee802154_session_accepted(oer_hil_protocol::ieee802154::ReceiveSession, "receive")
    }

    pub fn set_ieee802154_session_pending(
        &self,
        request: Ieee802154SessionPendingRequest,
    ) -> Result<()> {
        self.ieee802154_session_accepted(
            oer_hil_protocol::ieee802154::SetSessionPending(request),
            "pending",
        )
    }

    pub fn collect_ieee802154_session(&self) -> Result<Ieee802154SessionReceiveEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::CollectSession,
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionReceived(evidence)) => Ok(evidence),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session collect: {reason:?}").into())
            }
        }
    }

    pub fn maintain_ieee802154_session_phy(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionPhyMaintenance> {
        match self.call(0, oer_hil_protocol::ieee802154::MaintainSessionPhy, timeout)? {
            Ok(oer_hil_protocol::ieee802154::SessionPhyMaintained(outcome)) => Ok(outcome),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 PHY maintenance: {reason:?}").into())
            }
        }
    }

    /// Start OpenThread over the composed client and join the dataset's
    /// network.
    pub fn start_ieee802154_thread(
        &self,
        request: Ieee802154ThreadStartRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::StartThread(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::ThreadStarted(result)) => Ok(result),
            Err(reason) => {
                Err(format!("device rejected the Thread session start: {reason:?}").into())
            }
        }
    }

    pub fn query_ieee802154_thread(&self, timeout: Duration) -> Result<Ieee802154ThreadState> {
        match self.call(0, oer_hil_protocol::ieee802154::GetThread, timeout)? {
            Ok(oer_hil_protocol::ieee802154::ThreadState(state)) => Ok(state),
            Err(reason) => {
                Err(format!("device rejected the Thread state query: {reason:?}").into())
            }
        }
    }

    pub fn send_ieee802154_thread(
        &self,
        request: Ieee802154ThreadSendRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionResult> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::SendThread(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::ThreadSent(result)) => Ok(result),
            Err(reason) => Err(format!("device rejected the Thread datagram: {reason:?}").into()),
        }
    }

    pub fn collect_ieee802154_thread(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154ThreadReceiveEvidence> {
        match self.call(0, oer_hil_protocol::ieee802154::CollectThread, timeout)? {
            Ok(oer_hil_protocol::ieee802154::ThreadReceived(evidence)) => Ok(evidence),
            Err(reason) => Err(format!("device rejected the Thread collection: {reason:?}").into()),
        }
    }

    pub fn stop_ieee802154_thread(&self, timeout: Duration) -> Result<Ieee802154SessionResult> {
        match self.call(0, oer_hil_protocol::ieee802154::StopThread, timeout)? {
            Ok(oer_hil_protocol::ieee802154::ThreadStopped(result)) => Ok(result),
            Err(reason) => {
                Err(format!("device rejected the Thread session stop: {reason:?}").into())
            }
        }
    }

    /// Stop the running session's client and start it again, then apply the
    /// session configuration and receive.
    pub fn restart_ieee802154_session_radio(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionRestartEvidence> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::RestartSessionRadio,
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionRadioRestarted(evidence)) => Ok(evidence),
            Err(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 session radio restart: {reason:?}"
            )
            .into()),
        }
    }

    /// Read the live RSSI of the most recent baseband reception in the
    /// running session.
    pub fn read_ieee802154_session_recent_rssi(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionRecentRssi> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::ReadSessionRecentRssi,
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionRecentRssi(evidence)) => Ok(evidence),
            Err(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 session RSSI read: {reason:?}"
            )
            .into()),
        }
    }

    /// Scan the energy on one channel and assess it once in the running
    /// session.
    pub fn assess_ieee802154_session_channel(
        &self,
        request: Ieee802154SessionAssessRequest,
        timeout: Duration,
    ) -> Result<Ieee802154SessionAssessment> {
        match self.call(
            0,
            oer_hil_protocol::ieee802154::AssessSessionChannel(request),
            timeout,
        )? {
            Ok(oer_hil_protocol::ieee802154::SessionAssessed(assessment)) => Ok(assessment),
            Err(reason) => Err(format!(
                "device rejected the IEEE 802.15.4 channel assessment: {reason:?}"
            )
            .into()),
        }
    }

    pub fn stop_ieee802154_session(
        &self,
        timeout: Duration,
    ) -> Result<Ieee802154SessionStopEvidence> {
        match self.call(0, oer_hil_protocol::ieee802154::StopSession, timeout)? {
            Ok(oer_hil_protocol::ieee802154::SessionStopped(result)) => Ok(result),
            Err(reason) => {
                Err(format!("device rejected IEEE 802.15.4 session stop: {reason:?}").into())
            }
        }
    }

    pub fn request_station_start(&self, target: Target<'_>) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::StartStation(oer_hil_protocol::wifi::StationStart {
                credentials: target.station.credentials()?,
                power_save: target.settings.station_power_save,
            }),
            "station start",
        )
    }

    pub fn request_wifi_scan(&self, request: WifiScanRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::Scan(request),
            "standalone Wi-Fi scan",
        )
    }

    pub fn request_monitor_start(&self, request: WifiMonitorRequest) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::StartMonitor(request),
            "monitor start",
        )
    }

    pub fn request_monitor_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(oer_hil_protocol::wifi::StopMonitor, "monitor stop")
    }

    pub fn request_access_point_start(
        &self,
        request: oer_hil_protocol::wifi::WifiAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::StartAccessPoint(request),
            "access-point start",
        )
    }

    pub fn request_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(oer_hil_protocol::wifi::StopAccessPoint, "access-point stop")
    }

    pub fn request_station_access_point_start(
        &self,
        request: oer_hil_protocol::wifi::WifiStationAccessPointRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::StartStationAccessPoint(request),
            "station-access-point start",
        )
    }

    pub fn request_station_access_point_stop(&self) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::StopStationAccessPoint,
            "station-access-point stop",
        )
    }

    pub fn request_monitor_capture(
        &self,
        request: WifiMonitorCaptureRequest,
    ) -> Result<WifiCommandHandle> {
        self.request_wifi_command(
            oer_hil_protocol::wifi::CaptureMonitor(request),
            "finite monitor capture",
        )
    }

    /// The `M` that completes the Wi-Fi operation of `handle`; the
    /// operation's failure is an error.
    fn wait_for_wifi_event<M: Message>(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
        missing: &str,
    ) -> Result<M> {
        let message = self
            .wait_for_message_after(handle.first_event, timeout, |message| {
                message.boot_id == handle.boot_id
                    && message.session_id == 0
                    && message.request_id == handle.request_id
                    && (message.is::<M>()
                        || message.is::<oer_hil_protocol::wifi::RoleFailed>()
                        || message.is::<oer_hil_protocol::network::Failed>())
            })?
            .ok_or_else(|| missing.to_owned())?;
        if let Some(oer_hil_protocol::wifi::RoleFailed(reason)) = message.decode() {
            return Err(format!("Wi-Fi operation {} failed: {reason:?}", handle.request_id).into());
        }
        if let Some(oer_hil_protocol::network::Failed(reason)) = message.decode() {
            return Err(
                format!("target operation {} failed: {reason:?}", handle.request_id).into(),
            );
        }
        Ok(message.decode::<M>().expect("the accepted message decodes"))
    }

    pub fn wait_wifi_role_transition(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let oer_hil_protocol::wifi::RoleTransitioned(evidence) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete the Wi-Fi role transition",
        )?;
        Ok(evidence)
    }

    pub fn wait_wifi_radio_restart(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRadioRestartEvidence> {
        let oer_hil_protocol::wifi::RadioRestarted(evidence) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete the idle radio restart",
        )?;
        Ok(evidence)
    }

    pub fn wait_wifi_scan(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiScanEvidence> {
        let oer_hil_protocol::wifi::ScanCompleted(evidence) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete the standalone Wi-Fi scan",
        )?;
        Ok(evidence)
    }

    pub fn wait_monitor_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let oer_hil_protocol::wifi::MonitorStarted(evidence) =
            self.wait_for_wifi_event(handle, timeout, "device did not complete monitor start")?;
        Ok(evidence)
    }

    pub fn wait_access_point_start(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiRoleTransitionEvidence> {
        let oer_hil_protocol::wifi::AccessPointStarted(evidence) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete the access-point start",
        )?;
        Ok(evidence)
    }

    pub fn wait_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::wifi::WifiAccessPointEvidence> {
        let oer_hil_protocol::wifi::AccessPointStopped(evidence) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete the access-point stop",
        )?;
        Ok(evidence)
    }

    pub fn wait_station_access_point_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<oer_hil_protocol::wifi::WifiStationAccessPointStopEvidence> {
        let oer_hil_protocol::wifi::StationAccessPointStopped(evidence) = self
            .wait_for_wifi_event(
                handle,
                timeout,
                "device did not complete the station-access-point stop",
            )?;
        Ok(evidence)
    }

    pub fn wait_monitor_stop(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<WifiMonitorEvidence> {
        let oer_hil_protocol::wifi::MonitorStopped(evidence) =
            self.wait_for_wifi_event(handle, timeout, "device did not complete monitor stop")?;
        Ok(evidence)
    }

    pub fn wait_monitor_capture(
        &self,
        handle: WifiCommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence> {
        let oer_hil_protocol::wifi::MonitorCaptureCompleted(summary) = self.wait_for_wifi_event(
            handle,
            timeout,
            "device did not complete finite monitor capture",
        )?;
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
            .filter(|message| message.request_id == handle.request_id)
            .filter_map(|message| {
                message
                    .decode()
                    .map(|oer_hil_protocol::wifi::MonitorFrame(chunk)| chunk)
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
        let (_, oer_hil_protocol::network::Ready(info)) = self
            .wait_for_after(
                first_event,
                timeout,
                |message, oer_hil_protocol::network::Ready(info)| {
                    message.boot_id == boot_id
                        && message.session_id == 0
                        && message.request_id == 0
                        && info.network_interface == interface
                },
            )?
            .ok_or("new station stage did not publish a fresh network endpoint")?;
        Ok(Ipv4Addr::from(info.address))
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
            .wait_for_cursor(
                cursor,
                timeout,
                |message, _: &oer_hil_protocol::wifi::StationLifecycle| {
                    message.boot_id == boot_id && message.session_id == 0 && message.request_id == 0
                },
            )?
            .map(|(_, oer_hil_protocol::wifi::StationLifecycle(event))| event))
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
            .filter(|message| message.boot_id == boot_id)
            .find_map(|message| match message.decode() {
                Some(oer_hil_protocol::network::Ready(network))
                    if network.network_interface == network_interface =>
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
        messages
            .iter()
            .filter(|message| message.boot_id == boot_id)
            .any(|message| match message.decode() {
                Some(oer_hil_protocol::network::ServiceReady(service)) => {
                    service.network_interface == network_interface
                        && service.transport == transport
                        && service.direction == direction
                        && service.local_port == port
                }
                None => false,
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

    /// The first `M` from message `start` on that `accept` takes, with its
    /// header, waiting up to `timeout`.
    pub(super) fn wait_for_after<M: Message>(
        &self,
        start: usize,
        timeout: Duration,
        accept: impl Fn(&Received, &M) -> bool,
    ) -> Result<Option<(Received, M)>> {
        let mut cursor = start;
        self.wait_for_cursor(&mut cursor, timeout, accept)
    }

    /// [`Self::wait_for_after`] from `cursor`, which it advances past what
    /// it read.
    fn wait_for_cursor<M: Message>(
        &self,
        cursor: &mut usize,
        timeout: Duration,
        accept: impl Fn(&Received, &M) -> bool,
    ) -> Result<Option<(Received, M)>> {
        Ok(self
            .wait_for_message_cursor(cursor, timeout, |message| {
                message
                    .decode::<M>()
                    .is_some_and(|body| accept(message, &body))
            })?
            .map(|message| {
                let body = message.decode::<M>().expect("the accepted message decodes");
                (message, body)
            }))
    }

    /// The first message from message `start` on that `predicate` accepts,
    /// waiting up to `timeout`.
    pub(super) fn wait_for_message_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Received) -> bool,
    ) -> Result<Option<Received>> {
        let mut cursor = start;
        self.wait_for_message_cursor(&mut cursor, timeout, predicate)
    }

    fn wait_for_message_cursor(
        &self,
        cursor: &mut usize,
        timeout: Duration,
        predicate: impl Fn(&Received) -> bool,
    ) -> Result<Option<Received>> {
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

fn latest_boot_id_in(messages: &[Received]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        message
            .is::<oer_hil_protocol::base::Hello>()
            .then_some(message.boot_id)
    })
}

pub(super) fn beacon_loss_count_in(messages: &[Received]) -> usize {
    let Some(boot_id) = latest_boot_id_in(messages) else {
        return 0;
    };
    messages
        .iter()
        .filter(|message| {
            message.boot_id == boot_id
                && matches!(
                    message.decode(),
                    Some(oer_hil_protocol::wifi::StationLifecycle(
                        StationLifecycleEvent::Disconnected {
                            reason: oer_hil_protocol::wifi::StationDisconnectReason::BeaconLoss,
                            ..
                        }
                    ))
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

pub(super) fn station_unchanged_since_in(messages: &[Received], first_event: usize) -> Result<()> {
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
        // A Hello answering a request is a capability reply; only an
        // unsolicited one restarts the greeting.
        if message.is::<oer_hil_protocol::base::Hello>() && message.request_id == 0 {
            return Err("device restarted its greeting during station pause workload".into());
        }
        if let Some(oer_hil_protocol::wifi::StationLifecycle(event)) = message.decode() {
            return Err(format!("station changed during pause workload: {event:?}").into());
        }
    }
    Ok(())
}

/// A drained profile: the target's status and each hart's raw `(pc, ra)`
/// samples.
pub(crate) type DrainedProfile = (
    oer_hil_protocol::telemetry::ProfileStatus,
    [Vec<(u32, u32)>; 2],
);

/// Whether console bytes show a chip starting: the ROM banner or the ESP-IDF
/// bootloader's lines.
fn console_shows_boot(bytes: &[u8]) -> bool {
    [b"ESP-ROM:".as_slice(), b" boot: ".as_slice()]
        .iter()
        .any(|marker| bytes.windows(marker.len()).any(|window| window == *marker))
}
