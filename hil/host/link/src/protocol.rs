use oer_hil_protocol::base::RejectReason;
use oer_hil_protocol::base::{
    GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints, Hello, ImageKeyPage,
    PostMortemCheckpoints, Rejected,
};
use oer_hil_protocol::{Endpoint, Message};

use super::*;
use oer_hil_protocol::DeviceImageKeys;

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
        let oer_hil_protocol::network::Status(operation) =
            self.request(0, oer_hil_protocol::network::GetStatus, timeout)?;
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
                self.request_boot(boot_id, 0, GetImageKeys { first }, timeout)?;
            if page.first != first || page.keys.is_empty() {
                return Err(format!("invalid capability page {first}").into());
            }
            first += page.keys.len() as u16;
            pages.push(page);
        }
        Ok(DeviceImageKeys::assemble(hello, &pages)?)
    }

    fn inspect_stack_usage(&self, timeout: Duration) -> Result<Option<StackUsage>> {
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
        let artifact_event_start = self.event_cursor();
        if capabilities.has::<oer_hil_protocol::phy::StartupArtifact>()
            && let Some(path) = artifact_path
            && let Some(bytes) = crate::startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
            target.dut.journal(DutEvent::StartupArtifactUploaded {
                path: path.display().to_string(),
                sha256: oer_durable::sha256_bytes(&bytes),
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
            oer_durable::atomic_write(path, &bytes).map_err(|error| {
                format!(
                    "cannot persist startup artifact `{}`: {error}",
                    path.display()
                )
            })?;
            target.dut.journal(DutEvent::StartupArtifactWritten {
                path: path.display().to_string(),
                sha256: oer_durable::sha256_bytes(&bytes),
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
        let first_event = self.event_cursor();
        let (reply, outcome) = self.call_answered(
            0,
            oer_hil_protocol::wifi::Initialize(
                target.settings.initialization(target.station.ipv4()),
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
    ) -> Result<(DeviceImageKeys, CommandHandle)> {
        let (capabilities, _) = self.prepare_protocol(target)?;
        let handle = self.start_station(target)?;
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
        let handle = self.start_station(target)?;
        self.wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(handle, timeout)?;
        self.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
        Ok((capabilities, startup_artifact_status))
    }

    /// Sends `body` to the current boot and returns its response, or the
    /// reason the device refused it. Every request of every family goes
    /// through this exchange (or [`Self::request`]); the link keeps no
    /// per-message wrappers.
    pub fn call<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        Ok(self.call_answered(session_id, body, timeout)?.1)
    }

    /// [`Self::call`] addressed to boot `boot_id`, a boot the caller
    /// observed: a later boot never answers it.
    pub fn call_boot<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        if boot_id == 0 {
            return Err(format!("{} requires an observed boot identity", E::PATH).into());
        }
        Ok(self.call_identified(boot_id, session_id, body, timeout)?.1)
    }

    /// [`Self::call`], with the reply's header: its request identifier is
    /// the one the device's later messages about this request carry.
    pub fn call_answered<E: Endpoint>(
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

    /// The exchange of `body` with boot `boot_id`, with the reply's header.
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
    /// is an error naming the request.
    pub fn request<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call(session_id, body, timeout)?
            .map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH).into())
    }

    /// [`Self::request`] addressed to boot `boot_id`; see [`Self::call_boot`].
    pub fn request_boot<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call_boot(boot_id, session_id, body, timeout)?
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
        let event_count = self.event_cursor();
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

    /// Sends command `body`, which the device accepts at once and completes
    /// later with messages correlated to its request; see
    /// [`Self::wait_command`].
    pub fn command<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
        &self,
        body: E,
        timeout: Duration,
    ) -> Result<CommandHandle> {
        let first_event = self.event_cursor();
        let (reply, outcome) = self.call_answered(0, body, timeout)?;
        outcome.map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH))?;
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
            .ok_or("an accepted command disappeared from the capture")?;
        Ok(CommandHandle {
            path: E::PATH,
            boot_id: reply.boot_id,
            request_id: reply.request_id,
            first_event: first_event + accepted_offset + 1,
        })
    }

    /// The `M` that completes the command of `handle`; the command's typed
    /// failure (`network::Failed`, `wifi::RoleFailed`) is an error.
    pub fn wait_command<M: Message>(&self, handle: CommandHandle, timeout: Duration) -> Result<M> {
        let message = self
            .wait_for_message_after(handle.first_event, timeout, |message| {
                handle.correlates(message)
                    && (message.is::<M>()
                        || message.is::<oer_hil_protocol::wifi::RoleFailed>()
                        || message.is::<oer_hil_protocol::network::Failed>())
            })?
            .ok_or_else(|| format!("device did not complete {} with {}", handle.path, M::PATH))?;
        if let Some(oer_hil_protocol::wifi::RoleFailed(reason)) = message.decode() {
            return Err(format!(
                "{} operation {} failed: {reason:?}",
                handle.path, handle.request_id
            )
            .into());
        }
        if let Some(oer_hil_protocol::network::Failed(reason)) = message.decode() {
            return Err(format!(
                "target operation {} ({}) failed: {reason:?}",
                handle.request_id, handle.path
            )
            .into());
        }
        Ok(message.decode::<M>().expect("the accepted message decodes"))
    }

    /// The messages of this capture from `first` on, with the link's state
    /// checked first: what a family's own evidence analysis reads.
    pub fn messages_since<T>(
        &self,
        first: usize,
        read: impl FnOnce(&[Received]) -> T,
    ) -> Result<T> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.check()?;
        Ok(read(state.messages.get(first..).unwrap_or_default()))
    }

    /// A fresh identity for a session of this capture.
    pub fn next_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        self.request(0, GetLinkHealth, timeout)
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
    pub fn profiled(mut self, profile: Profile) -> Result<Self> {
        self.wait_for_message_after(0, PROTOCOL_READY_TIMEOUT, |message| message.is::<Hello>())?
            .ok_or("device did not publish a HIL protocol hello before the profile")?;
        match self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(profile.control),
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

    /// Submit a real station start; its role transition completes it.
    fn start_station(&self, target: Target<'_>) -> Result<CommandHandle> {
        self.command(
            oer_hil_protocol::wifi::StartStation(oer_hil_protocol::wifi::StationStart {
                credentials: target.station.credentials()?,
                power_save: target.settings.station_power_save,
            }),
            PROTOCOL_READY_TIMEOUT,
        )
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
        self.event_cursor()
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

    /// The cursor of the next message this capture decodes: waits from it
    /// see only what arrives later.
    pub fn event_cursor(&self) -> usize {
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .messages
            .len()
    }

    /// The link's failure, a closed capture or a cancellation, as an error.
    pub fn check_link(&self) -> Result<()> {
        oer_process::check_cancelled()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check()
    }

    /// The first `M` from message `start` on that `accept` takes, with its
    /// header, waiting up to `timeout`.
    pub fn wait_for_after<M: Message>(
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
    pub fn wait_for_cursor<M: Message>(
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
    pub fn wait_for_message_after(
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

/// The boot of the newest Hello among `messages`: the boot they belong to now.
pub fn latest_boot_id_in(messages: &[Received]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        message
            .is::<oer_hil_protocol::base::Hello>()
            .then_some(message.boot_id)
    })
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
