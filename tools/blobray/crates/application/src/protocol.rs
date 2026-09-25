//! Application/host execution protocol, independent of retained research schemas.
use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportBinding {
    pub role: String,
    pub origin: OriginPath,
    pub expected: Option<ArtifactId>,
}

/// Private worker protocol, independently versioned from revision manifests.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportWork {
    pub schema: u32,
    pub run: RunId,
    pub project: ProjectId,
    pub parent: Option<RevisionId>,
    pub target: Target,
    pub inputs: Vec<ImportBinding>,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReport {
    pub schema: u32,
    pub diagnostics: RunDiagnostics,
    pub state: RunState,
    pub prepared: Option<PreparedReceipt>,
    pub error: Option<Error>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "receipt",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum PreparedReceipt {
    Ir(PreparedIrReceipt),
    Execution(PreparedExecutionReceipt),
    Knowledge(PreparedKnowledgeReceipt),
    Import(PreparedImport),
    Image(PreparedImageReceipt),
    Function(PreparedFunctionReceipt),
    Investigation(PreparedInvestigationReceipt),
}

/// Host-owned process session. Drop must stop and reap the session's processes.
/// Implementations must retain containment until cleanup finishes.
pub trait OperationWorker: Send {
    fn poll(&mut self) -> Result<Option<WorkerReport>>;
    fn cancel(&mut self) -> Result<()>;
    fn progress(&self) -> Result<Option<RunProgress>> {
        Ok(None)
    }
    /// Block for at most `timeout_ms`, returning early once the session exits.
    /// The default sleeps; hosts with an exit notification should use it.
    fn wait(&mut self, timeout_ms: u64) -> Result<()> {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms));
        Ok(())
    }
}

/// Injected platform capability. Libraries do not install signal handlers.
pub trait OperationHost: Send + Sync {
    /// Host chooses and validates a private runtime root. No project mutation.
    fn temporary_root(&self, requested: Option<&Path>) -> Result<PathBuf>;
    fn now_ms(&self) -> u64;
    fn save_progress(&self, stage: &Path, progress: &ProgressRecord) -> Result<()>;
    fn owner(&self) -> Result<OwnerIdentity>;
    fn alive(&self, owner: &OwnerIdentity) -> Result<bool>;
    /// Reclaim empty host containment after store has acquired the stage lease.
    fn reclaim(&self, stage: &Path) -> Result<()>;
    fn launch(
        &self,
        stage: &Path,
        budget: &ResourceBudget,
        deadline_ms: u64,
    ) -> Result<Box<dyn OperationWorker>>;
}

/// Read-only worker request. Its temporary output belongs to the caller, never
/// the project. No journal row or writer capability is acquired.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
// One bounded control message, never a resident collection of requests.
#[allow(clippy::large_enum_variant)]
pub enum ReadQuery {
    Trace {
        request: TraceRequest,
    },
    SemanticIr {
        id: ArtifactId,
    },
    Registers {
        request: RegisterQuery,
    },
    EventRoute {
        request: EventRouteQuery,
    },
    MemorySlice {
        request: MemorySliceQuery,
    },
    Flow {
        request: FlowQuery,
    },
    Navigate {
        request: NavigationQuery,
    },
    Interfaces {
        request: InterfaceQuery,
    },
    Coverage {
        id: PublicationId,
    },
    StorageUsage,
    Data {
        request: DataRequest,
    },
    ReviewedData {
        revision: KnowledgeRevisionId,
        assertion: AssertionId,
    },
    AuditTargets {
        artifact: OriginPath,
        ranges: Vec<ForbiddenTargetRange>,
    },
    Execution {
        id: ArtifactId,
        /// Validate every record but return no guest event records.
        #[serde(default)]
        omit_events: bool,
    },
    /// The retained manifest after verifying the request and record payload
    /// digests; records are neither decoded nor returned.
    ExecutionSummary {
        id: ArtifactId,
    },
    ValidateKnowledge {
        change: KnowledgeChange,
    },
    RetainedPayload {
        id: ArtifactId,
    },
    ImportLegacy {
        request: LegacyRequest,
    },
    Legacy,
    Backup,
    Restore {
        bundle: OriginPath,
    },
    Knowledge {
        revision: Option<KnowledgeRevisionId>,
        history: bool,
    },
    PlanInvestigation {
        request: InvestigationRequest,
        producer: FunctionProducer,
    },
    Publications,
    Publication {
        id: PublicationId,
        filter: InvestigationFilter,
    },
    InvestigationStatus,
    Analyses,
    Analysis {
        id: FunctionAnalysisId,
        export: bool,
    },
    LinkPlan {
        request: LinkRequest,
        linker: OriginPath,
    },
    NamedLinkPlan {
        request: NamedLinkRequest,
        linker: OriginPath,
    },
    /// Trial-link `request` and resolve its undefined names in `candidates`.
    ProposeCompanions {
        request: LinkRequest,
        linker: OriginPath,
        candidates: Vec<u64>,
    },
    Images,
    Image {
        id: PreparedImageId,
        export: bool,
    },
    Inventory {
        revision: Option<RevisionId>,
    },
    Doctor,
    Select {
        revision: Option<RevisionId>,
        request: SelectionRequest,
    },
    Plan {
        request: PlanRequest,
    },
    ReopenPlan {
        description: Box<PlanDescription>,
    },
    ExecutePlan {
        description: Box<PlanDescription>,
        manifest: OriginPath,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub query: ReadQuery,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}

pub(crate) use write_control_message as write_request;
pub(crate) const MESSAGE_BYTES: usize = CONTROL_MESSAGE_BYTES;
/// Write one host control message, capped at 64 KiB before any excess write.
/// The fixed temporary control reserve covers at most sixteen such files.
pub fn write_control_message(writer: impl std::io::Write, value: &impl Serialize) -> Result<()> {
    let mut bounded = BoundedWriter {
        writer,
        remaining: MESSAGE_BYTES,
        exceeded: false,
        failure: None,
    };
    let result = serde_json::to_writer(&mut bounded, value);
    if let Some(error) = bounded.failure {
        return Err(error);
    }
    if bounded.exceeded {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "control message exceeds 64 KiB",
        ));
    }
    result.map_err(|e| Error::new(ErrorCode::Io, e.to_string()))
}
pub(crate) struct BoundedWriter<W> {
    pub writer: W,
    pub remaining: usize,
    pub exceeded: bool,
    pub failure: Option<Error>,
}
impl<W: std::io::Write> std::io::Write for BoundedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other("control message exceeds 64 KiB"));
        }
        let written = match self.writer.write(bytes) {
            Ok(written) => written,
            Err(error) => {
                let error = storage_io(error);
                self.failure = Some(error.clone());
                return Err(std::io::Error::other(error));
            }
        };
        self.remaining -= written;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
pub(crate) fn check_import_size(inputs: &[ImportInput]) -> Result<()> {
    // Raw byte count is a lower bound on this protocol's JSON encoding. This
    // check rejects no message that could fit; bounded serialization is final.
    let mut bytes = 0usize;
    for input in inputs {
        bytes = bytes
            .checked_add(input.role.len())
            .and_then(|n| n.checked_add(input.path.as_os_str().as_encoded_bytes().len()))
            .filter(|n| *n <= MESSAGE_BYTES)
            .ok_or_else(|| {
                Error::new(ErrorCode::InvalidRequest, "control message exceeds 64 KiB")
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_bounds_encoded_expansion_before_writing_excess() {
        let mut written = Vec::new();
        let value = "\0".repeat(MESSAGE_BYTES / 2);
        assert!(value.len() < MESSAGE_BYTES);
        let error = write_request(&mut written, &value).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(written.len() <= MESSAGE_BYTES);
        written.clear();
        write_request(&mut written, &"x".repeat(MESSAGE_BYTES - 2)).unwrap();
        assert_eq!(written.len(), MESSAGE_BYTES);
    }
}
