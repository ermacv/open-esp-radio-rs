//! Owned query result delivery; execution is owned by the common application jobs.
use crate::*;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

pub struct QueryOutput {
    pub report: WorkerReport,
    pub clean: bool,
    summary: QuerySummary,
    records: File,
    bundle: Vec<(&'static str, File)>,
    manifest: Option<File>,
    manifest_path: PathBuf,
    host: Arc<dyn OperationHost>,
    budget: ResourceBudget,
    started_ms: u64,
    deadline_ms: u64,
    attempted: bool,
    _permit: Arc<()>,
    _temporary: crate::temporary::Workspace,
}
impl QueryOutput {
    pub(crate) fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }
    pub fn summary(&self) -> &QuerySummary {
        &self.summary
    }
    pub(crate) fn open(
        mut temporary: crate::temporary::Workspace,
        stage: &Path,
        work: &QueryWork,
        report: WorkerReport,
        host: Arc<dyn OperationHost>,
        permit: Arc<()>,
    ) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(stage.join("query-summary.json"))
            .map_err(io)?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > 65536 {
            return Err(Error::new(
                ErrorCode::WorkerProtocol,
                "query summary exceeds limit",
            ));
        }
        let summary: QuerySummary = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorCode::WorkerProtocol, e.to_string()))?;
        let manifest = match (&work.query, &summary) {
            (
                ReadQuery::ValidateKnowledge { change },
                QuerySummary::KnowledgeValidation { expected_base },
            ) if &change.expected_base == expected_base => None,
            (
                ReadQuery::RetainedPayload { id },
                QuerySummary::RetainedPayload { id: actual, .. },
            ) if id == actual => None,
            (ReadQuery::Legacy, QuerySummary::Legacy { .. }) => None,
            (ReadQuery::ImportLegacy { .. }, QuerySummary::Preservation { restored: true, .. }) => {
                None
            }
            (
                ReadQuery::Backup,
                QuerySummary::Preservation {
                    restored: false, ..
                },
            )
            | (ReadQuery::Restore { .. }, QuerySummary::Preservation { restored: true, .. }) => {
                None
            }
            (ReadQuery::Knowledge { revision, .. }, QuerySummary::Knowledge { status })
                if revision == &status.revision =>
            {
                None
            }
            (
                ReadQuery::PlanInvestigation { request, producer },
                QuerySummary::InvestigationPlan { plan },
            ) if &plan.recipe.request == request && &plan.recipe.producer == producer => {
                blobray_store::validate_investigation_plan(plan)?;
                None
            }
            (ReadQuery::Publications, QuerySummary::Publications { .. }) => None,
            (ReadQuery::Publication { id, .. }, QuerySummary::Publication { id: actual, .. })
                if id == actual =>
            {
                None
            }
            (ReadQuery::InvestigationStatus, QuerySummary::InvestigationStatus { .. }) => None,
            (ReadQuery::Analyses, QuerySummary::Analyses { .. }) => None,
            (ReadQuery::Execution { id }, QuerySummary::Execution { id: actual, .. })
                if id == actual =>
            {
                None
            }
            (ReadQuery::Analysis { id, .. }, QuerySummary::Analysis { id: actual, .. })
                if id == actual =>
            {
                None
            }
            (
                ReadQuery::NamedLinkPlan { request, .. },
                QuerySummary::Selection { revision, .. },
            ) if request.revision.as_ref() == Some(revision) => None,
            (ReadQuery::NamedLinkPlan { request, .. }, QuerySummary::LinkPlan { description })
                if request.revision.as_ref() == Some(&description.recipe.revision)
                    && request.inputs == description.recipe.inputs
                    && request.entry_input == description.recipe.entry.input
                    && request.layout == description.recipe.layout
                    && description.recipe.roots.is_empty() =>
            {
                Some(File::open(stage.join("query-manifest")).map_err(io)?)
            }
            (ReadQuery::LinkPlan { request, .. }, QuerySummary::LinkPlan { description })
                if request.revision.as_ref() == Some(&description.recipe.revision)
                    && request.inputs == description.recipe.inputs
                    && request.entry == description.recipe.entry
                    && request.roots == description.recipe.roots
                    && request.layout == description.recipe.layout =>
            {
                Some(File::open(stage.join("query-manifest")).map_err(io)?)
            }
            (ReadQuery::Images, QuerySummary::Images { .. }) => None,
            (ReadQuery::Image { id, .. }, QuerySummary::Image { id: actual, .. })
                if id == actual =>
            {
                None
            }
            (
                ReadQuery::Inventory {
                    revision: Some(expected),
                },
                QuerySummary::Inventory { revision_id, .. },
            ) if expected == revision_id => {
                Some(File::open(stage.join("query-manifest")).map_err(io)?)
            }
            (ReadQuery::Doctor, QuerySummary::Doctor { .. }) => None,
            (
                ReadQuery::Select {
                    revision: Some(expected),
                    ..
                },
                QuerySummary::Selection { revision, .. },
            ) if expected == revision => None,
            (ReadQuery::Plan { request }, QuerySummary::Plan { description })
                if request.revision.as_ref() == Some(&description.recipe.revision)
                    && request.scope == description.recipe.scope
                    && request.budget == description.recipe.budget =>
            {
                Some(File::open(stage.join("query-manifest")).map_err(io)?)
            }
            (
                ReadQuery::ReopenPlan {
                    description: expected,
                },
                QuerySummary::Plan { description },
            ) if expected == description => {
                Some(File::open(stage.join("query-manifest")).map_err(io)?)
            }
            (ReadQuery::ExecutePlan { description, .. }, QuerySummary::Inspection { plan, .. })
                if &description.id == plan =>
            {
                None
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::WorkerProtocol,
                    "query result differs from admitted selection",
                ));
            }
        };
        let mut bundle = Vec::new();
        if matches!(&work.query, ReadQuery::Image { export: true, .. }) {
            for name in [
                "image.elf",
                "link.map",
                "extraction.tsv",
                "provenance.jsonl",
                "manifest.json",
            ] {
                bundle.push((name, File::open(stage.join(name)).map_err(io)?));
            }
        }
        if matches!(&work.query, ReadQuery::Analysis { export: true, .. }) {
            for name in ["records.jsonl", "manifest.json"] {
                bundle.push((name, File::open(stage.join(name)).map_err(io)?));
            }
        }
        if matches!(&work.query, ReadQuery::RetainedPayload { .. }) {
            bundle.push((
                "payload.bin",
                File::open(stage.join("payload.bin")).map_err(io)?,
            ));
        }
        if matches!(&work.query, ReadQuery::Backup) {
            bundle.push((
                "backup.blobray",
                File::open(stage.join("backup.blobray")).map_err(io)?,
            ));
        }
        if !matches!(
            summary,
            QuerySummary::Preservation { .. } | QuerySummary::RetainedPayload { .. }
        ) {
            temporary.retain()?;
        }
        Ok(Self {
            _temporary: temporary,
            clean: summary.clean(),
            summary,
            bundle,
            records: File::open(stage.join("query-records")).map_err(io)?,
            manifest,
            manifest_path: stage.join("query-manifest"),
            report,
            host,
            budget: work.budget.clone(),
            started_ms: work.started_ms,
            deadline_ms: work.deadline_ms,
            attempted: false,
            _permit: permit,
        })
    }
    /// Stream borrowed typed records once. Consumers own any retained copies.
    pub fn records(
        &mut self,
        cancelled: &dyn Fn() -> bool,
        sink: &mut dyn QuerySink,
    ) -> Result<()> {
        self.deliver(cancelled, |this, memory, control| {
            crate::query_stream::visit(&mut this.records, memory, control, sink)?;
            sink.summary(&this.summary, control)
        })
    }
    /// Publish a complete private backup file without replacing an existing destination.
    pub fn export_backup(
        &mut self,
        destination: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        if !matches!(
            self.summary,
            QuerySummary::Preservation {
                restored: false,
                ..
            }
        ) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query is not a backup",
            ));
        }
        self.export_file(destination, cancelled)
    }
    /// Export a retained legacy/provenance payload by its exact content identity.
    pub fn export_payload(
        &mut self,
        destination: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        if !matches!(self.summary, QuerySummary::RetainedPayload { .. }) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query has no retained payload",
            ));
        }
        self.export_file(destination, cancelled)
    }
    fn export_file(&mut self, destination: &Path, cancelled: &dyn Fn() -> bool) -> Result<()> {
        self.deliver(cancelled, |this, _, control| {
            let parent = destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let disk = blobray_store::TemporaryBudget::open(this.manifest_path.parent().unwrap())?;
            if let Some(usage) = this
                .report
                .diagnostics
                .progress
                .and_then(|p| p.temporary_storage)
            {
                disk.reserve(usage.current_bytes.saturating_sub(TEMPORARY_CONTROL_BYTES))?;
            }
            disk.reserve(this.bundle[0].1.metadata().map_err(io)?.len())?;
            control.temporary_storage(disk.usage());
            let mut output = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
            let source = &mut this.bundle[0].1;
            source.rewind().map_err(io)?;
            let mut buffer = [0; WORK_BLOCK];
            loop {
                control.checkpoint(1)?;
                let n = source.read(&mut buffer).map_err(io)?;
                if n == 0 {
                    break;
                }
                control.bytes(n)?;
                std::io::Write::write_all(&mut output, &buffer[..n]).map_err(io)?;
            }
            output.as_file().sync_all().map_err(io)?;
            control.checkpoint(0)?;
            output
                .persist_noclobber(destination)
                .map_err(|e| io(e.error))?;
            File::open(parent).and_then(|f| f.sync_all()).map_err(io)
        })
    }
    /// Copy a verified staged project beside the destination before exposing its state directory.
    pub fn publish_restore(
        &mut self,
        destination: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        if !matches!(
            self.summary,
            QuerySummary::Preservation { restored: true, .. }
        ) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query is not a restored project",
            ));
        }
        self.deliver(cancelled, |this, _, control| {
            let parent = destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let stage = this.manifest_path.parent().unwrap();
            let disk = blobray_store::TemporaryBudget::open(stage)?;
            if let Some(usage) = this
                .report
                .diagnostics
                .progress
                .and_then(|p| p.temporary_storage)
            {
                disk.reserve(usage.current_bytes.saturating_sub(TEMPORARY_CONTROL_BYTES))?;
            }
            let private = tempfile::Builder::new()
                .prefix(".blobray-restore-")
                .tempdir_in(parent)
                .map_err(io)?;
            let source = stage.join("restored/.blobray-next");
            let state = private.path().join(".blobray-next");
            std::fs::create_dir(&state).map_err(io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))
                    .map_err(io)?;
            }
            std::fs::create_dir(state.join("objects")).map_err(io)?;
            std::fs::create_dir(state.join("staging")).map_err(io)?;
            for directory in ["", "objects"] {
                for entry in std::fs::read_dir(source.join(directory)).map_err(io)? {
                    control.checkpoint(1)?;
                    let entry = entry.map_err(io)?;
                    if entry.file_type().map_err(io)?.is_dir() {
                        continue;
                    }
                    let mut input = File::open(entry.path()).map_err(io)?;
                    let mut output = disk.create(&state.join(directory).join(entry.file_name()))?;
                    let mut buffer = [0; WORK_BLOCK];
                    loop {
                        control.checkpoint(1)?;
                        let n = input.read(&mut buffer).map_err(io)?;
                        if n == 0 {
                            break;
                        }
                        control.bytes(n)?;
                        std::io::Write::write_all(&mut output, &buffer[..n]).map_err(io)?;
                    }
                    output.sync_all().map_err(io)?;
                }
            }
            File::open(state.join("objects"))
                .and_then(|f| f.sync_all())
                .map_err(io)?;
            File::open(&state).and_then(|f| f.sync_all()).map_err(io)?;
            control.checkpoint(0)?;
            std::fs::create_dir(destination).map_err(io)?;
            if let Err(error) = std::fs::rename(&state, destination.join(".blobray-next")) {
                let _ = std::fs::remove_dir(destination);
                return Err(io(error));
            }
            File::open(destination)
                .and_then(|f| f.sync_all())
                .map_err(io)?;
            File::open(parent).and_then(|f| f.sync_all()).map_err(io)
        })
    }
    /// Export an already captured image bundle. The destination must not exist.
    /// Delivery is single-use; a failed export may leave an explicitly incomplete directory.
    pub fn export_image(&mut self, destination: &Path, cancelled: &dyn Fn() -> bool) -> Result<()> {
        if self.bundle.len() != 5 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query has no captured image bundle",
            ));
        }
        self.export_bundle(destination, cancelled)
    }
    /// Export a saved function result into a nonexistent directory; manifest is last.
    pub fn export_analysis(
        &mut self,
        destination: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        if self.bundle.len() != 2 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query has no captured analysis bundle",
            ));
        }
        self.export_bundle(destination, cancelled)
    }
    fn export_bundle(&mut self, destination: &Path, cancelled: &dyn Fn() -> bool) -> Result<()> {
        self.deliver(cancelled, |this, _, control| {
            if this.bundle.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "query has no captured image bundle",
                ));
            }
            std::fs::create_dir(destination).map_err(io)?;
            for (name, source) in &mut this.bundle {
                let mut output = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(destination.join(name))
                    .map_err(io)?;
                source.rewind().map_err(io)?;
                let mut buffer = [0; WORK_BLOCK];
                loop {
                    control.checkpoint(1)?;
                    let count = source.read(&mut buffer).map_err(io)?;
                    if count == 0 {
                        break;
                    }
                    control.bytes(count)?;
                    std::io::Write::write_all(&mut output, &buffer[..count]).map_err(io)?;
                }
                output.sync_all().map_err(io)?;
            }
            Ok(())
        })
    }
    /// Borrow the captured manifest for one delivery. No live project lookup.
    pub fn manifest(
        &mut self,
        cancelled: &dyn Fn() -> bool,
        consume: impl FnOnce(&QuerySummary, &dyn ByteSource, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        self.deliver(cancelled, |this, _, control| {
            let file = this.manifest.as_mut().ok_or_else(|| {
                Error::new(ErrorCode::InvalidRequest, "query has no inventory manifest")
            })?;
            let length = file.metadata().map_err(io)?.len();
            let source = ResultSource {
                file: std::cell::RefCell::new(file),
                length,
            };
            consume(&this.summary, &source, control)
        })
    }
    pub(crate) fn deliver(
        &mut self,
        cancelled: &dyn Fn() -> bool,
        f: impl FnOnce(&mut Self, &WorkingMemory, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        if self.attempted {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "query delivery already attempted",
            ));
        }
        self.attempted = true; // An error may leave a destination prefix: never retry implicitly.
        let host = self.host.clone();
        let environment = DeliveryEnvironment {
            host: &*host,
            cancelled,
        };
        let mut control = RunContext::new(
            &environment,
            self.started_ms,
            self.deadline_ms,
            &self.budget,
            self.report.diagnostics.progress,
        )?;
        let memory = WorkingMemory::new(self.budget.working_memory_bytes.unwrap())?;
        let result = (|| {
            let _fixed = memory.reserve(65536, control.position())?;
            control.phase(RunPhase::Serialize)?;
            f(self, &memory, &mut control)
        })();
        control.working_memory(memory.observation());
        self.report.diagnostics.progress = Some(control.snapshot());
        result
    }
}
struct DeliveryEnvironment<'a> {
    host: &'a dyn OperationHost,
    cancelled: &'a dyn Fn() -> bool,
}
impl RunEnvironment for DeliveryEnvironment<'_> {
    fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }
    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
    fn observe(&self, _: &RunProgress) -> Result<()> {
        Ok(())
    }
}
struct ResultSource<'a> {
    file: std::cell::RefCell<&'a mut File>,
    length: u64,
}
impl ByteSource for ResultSource<'_> {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "query result range invalid",
            ));
        }
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        for chunk in bytes.chunks_mut(WORK_BLOCK) {
            control.bytes(chunk.len())?;
            file.read_exact(chunk).map_err(io)?;
        }
        Ok(())
    }
}
fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
