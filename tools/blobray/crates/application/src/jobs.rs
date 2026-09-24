use crate::*;
use blobray_store::Writer;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

pub(crate) struct State {
    record: RunRecord,
    cancel: bool,
    committing: bool,
    done: bool,
    events: std::collections::VecDeque<RunEvent>,
    sequence: u64,
    event_capacity: usize,
    output: Option<QueryOutput>,
}
pub(crate) struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    _permit: Arc<()>,
}

/// Client view. Dropping it does not detach the application-owned operation.
#[derive(Clone)]
pub struct RunHandle {
    shared: Arc<Shared>,
}
impl RunHandle {
    /// Transfer the query result once after terminal cleanup. Waiting remains repeatable.
    pub fn take_output(&self) -> Result<QueryOutput> {
        self.wait();
        let mut state = self.shared.state.lock().unwrap();
        if let Some(error) = &state.record.error {
            return Err(error.clone());
        }
        state.output.take().ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "query output already taken or operation is not a query",
            )
        })
    }
    pub fn take_link_plan(&self) -> Result<LinkPlan> {
        LinkPlan::from_output(self.take_output()?)
    }
    pub fn take_plan(&self) -> Result<Plan> {
        Plan::from_output(self.take_output()?)
    }
    pub fn report(&self) -> WorkerReport {
        let record = self.status();
        WorkerReport {
            schema: 6,
            state: record.state,
            error: record.error,
            prepared: None,
            diagnostics: record.diagnostics.unwrap_or_default(),
        }
    }
    pub fn status(&self) -> RunRecord {
        self.shared.state.lock().unwrap().record.clone()
    }
    /// True means cancellation was admitted before the commit boundary.
    pub fn cancel(&self) -> bool {
        let mut state = self.shared.state.lock().unwrap();
        if state.committing || state.record.state.terminal() {
            return false;
        }
        state.cancel = true;
        self.shared.changed.notify_all();
        true
    }
    /// Events newer than the caller's cursor; oldest progress can be coalesced.
    pub fn events(&self, after: u64) -> Vec<RunEvent> {
        self.shared
            .state
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.sequence > after)
            .cloned()
            .collect()
    }
    pub fn wait(&self) -> RunRecord {
        let mut state = self.shared.state.lock().unwrap();
        while !state.done {
            state = self.shared.changed.wait(state).unwrap();
        }
        state.record.clone()
    }
}

/// Owns admission, workers and shutdown. No process-global state or signal handlers.
struct Jobs {
    closed: bool,
    tasks: Vec<(RunHandle, JoinHandle<()>)>,
    permits: Vec<std::sync::Weak<()>>,
}
/// Bounds application-owned jobs/results independently of worker memory.
#[derive(Clone, Copy, Debug)]
pub struct ApplicationLimits {
    pub max_operations: usize,
    pub event_capacity: usize,
}
impl Default for ApplicationLimits {
    fn default() -> Self {
        Self {
            max_operations: 16,
            event_capacity: 64,
        }
    }
}
pub struct Application {
    host: Arc<dyn OperationHost>,
    temporary: crate::temporary::TemporaryRuntime,
    jobs: Mutex<Jobs>,
    limits: ApplicationLimits,
    shutdown_lock: Mutex<()>,
}
impl Application {
    pub fn new(host: Arc<dyn OperationHost>) -> Self {
        Self::with_limits(host, ApplicationLimits::default()).expect("valid default limits")
    }
    pub fn with_limits(host: Arc<dyn OperationHost>, limits: ApplicationLimits) -> Result<Self> {
        Self::with_temporary_storage(host, limits, TemporaryStoragePolicy::default())
    }
    pub fn temporary_storage_status(&self) -> TemporaryStorageStatus {
        self.temporary.status()
    }
    pub fn with_temporary_storage(
        host: Arc<dyn OperationHost>,
        limits: ApplicationLimits,
        policy: TemporaryStoragePolicy,
    ) -> Result<Self> {
        if limits.max_operations == 0
            || limits.event_capacity == 0
            || limits.max_operations > 1024
            || limits.event_capacity > 4096
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "operation capacity must be 1..1024; events 1..4096",
            ));
        }
        Ok(Self {
            temporary: crate::temporary::TemporaryRuntime::new(policy, limits.max_operations)?,
            host,
            limits,
            shutdown_lock: Mutex::new(()),
            jobs: Mutex::new(Jobs {
                closed: false,
                tasks: Vec::new(),
                permits: Vec::new(),
            }),
        })
    }
    fn admit(&self, jobs: &mut Jobs) -> Result<Arc<()>> {
        if jobs.closed {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "application has shut down",
            ));
        }
        let mut index = 0;
        while index < jobs.tasks.len() {
            if jobs.tasks[index].1.is_finished() {
                let (_, task) = jobs.tasks.swap_remove(index);
                let _ = task.join();
            } else {
                index += 1;
            }
        }
        jobs.permits.retain(|p| p.strong_count() != 0);
        if jobs.permits.len() >= self.limits.max_operations {
            return Err(Error::new(
                ErrorCode::Busy,
                "application operation/result capacity exhausted",
            ));
        }
        let permit = Arc::new(());
        jobs.permits.push(Arc::downgrade(&permit));
        Ok(permit)
    }
    pub fn start_import(
        &self,
        project: &Path,
        inputs: Vec<ImportInput>,
        target: Target,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working memory capacity required for new runs",
            ));
        }
        if inputs.is_empty() || inputs.iter().any(|i| i.role.trim().is_empty()) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "import requires nonempty input roles",
            ));
        }
        // Preflight borrowed inputs before cloning/normalizing paths.
        crate::protocol::check_import_size(&inputs)?;
        let mut binding_bytes = crate::protocol::BoundedWriter {
            writer: std::io::sink(),
            remaining: crate::protocol::MESSAGE_BYTES,
            exceeded: false,
            failure: None,
        };
        let bindings = inputs
            .into_iter()
            .map(|input| {
                let path = std::path::absolute(input.path).map_err(storage_io)?;
                let binding = ImportBinding {
                    role: input.role,
                    origin: OriginPath::from_path(&path),
                    expected: input.expected,
                };
                serde_json::to_writer(&mut binding_bytes, &binding).map_err(|_| {
                    Error::new(ErrorCode::InvalidRequest, "worker request exceeds 64 KiB")
                })?;
                Ok(binding)
            })
            .collect::<Result<Vec<_>>>()?;
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        let (record, stage) =
            writer.register_with_stage_owner(budget.clone(), self.host.owner()?, |stage| {
                reservation.attach(stage)
            })?;
        let work = ImportWork {
            schema: 2,
            run: record.id.clone(),
            project: writer.project().id().clone(),
            parent: record.base.clone(),
            target,
            inputs: bindings,
            started_ms,
            deadline_ms,
            budget,
        };
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Import(work),
                permit,
                reservation,
            },
        )
    }
    fn launch_durable(
        &self,
        jobs: &mut Jobs,
        project: PathBuf,
        admission: DurableAdmission,
    ) -> Result<RunHandle> {
        let DurableAdmission {
            writer,
            record,
            stage,
            work,
            permit,
            reservation,
        } = admission;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                record: record.clone(),
                cancel: false,
                committing: false,
                done: false,
                events: std::collections::VecDeque::from([RunEvent {
                    run: record.id.clone(),
                    sequence: 1,
                    state: RunState::Registered,
                    progress: None,
                }]),
                sequence: 1,
                event_capacity: self.limits.event_capacity,
                output: None,
            }),
            changed: Condvar::new(),
            _permit: permit,
        });
        let handle = RunHandle {
            shared: shared.clone(),
        };
        let host = self.host.clone();
        let registration = record.clone();
        let thread = match thread::Builder::new()
            .name("blobray-import".into())
            .spawn(move || supervise(writer, record, stage, work, host, shared, reservation))
        {
            Ok(thread) => thread,
            Err(error) => {
                let error = Error::new(ErrorCode::Io, error.to_string());
                let mut failed = registration;
                failed.state = RunState::Failed;
                failed.error = Some(error.clone());
                let mut writer = Writer::open(&project)?;
                writer.update_run(&failed)?;
                writer.cleanup_stage(&failed.id)?;
                return Err(error);
            }
        };
        jobs.tasks.push((handle.clone(), thread));
        Ok(handle)
    }
    pub fn start_link_plan(
        &self,
        project: &Path,
        request: LinkRequest,
        linker: &Path,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_query(
            project,
            ReadQuery::LinkPlan {
                request,
                linker: OriginPath::from_path(&std::path::absolute(linker).map_err(storage_io)?),
            },
            budget,
        )
    }
    pub fn link_plan(
        &self,
        project: &Path,
        request: LinkRequest,
        linker: &Path,
        budget: ResourceBudget,
    ) -> Result<LinkPlan> {
        self.start_link_plan(project, request, linker, budget)?
            .take_link_plan()
    }
    pub fn start_prepare_image(
        &self,
        project: &Path,
        plan: &LinkPlanDescription,
        linker: &Path,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        validate_link_plan(plan)?;
        if !plan.ready() {
            return Err(Error::new(ErrorCode::LinkBlocked, "link plan has blockers"));
        }
        let linker = OriginPath::from_path(&std::path::absolute(linker).map_err(storage_io)?);
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "image working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        if writer.project().id() != &plan.recipe.project {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "link plan belongs to another project",
            ));
        }
        let mut work = ImageWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            plan: plan.clone(),
            linker,
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget.clone(),
            self.host.owner()?,
            RunOperation::PrepareImage {
                revision: plan.recipe.revision.clone(),
                plan: plan.id.clone(),
            },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Image(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    pub fn start_build_ir(
        &self,
        project: &Path,
        request: IrBuildRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        request.validate()?;
        write_control_message(&mut std::io::sink(), &request)?;
        let mut work = IrWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            request: request.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::BuildIr { request },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Ir(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    pub fn start_execution(
        &self,
        project: &Path,
        request: ExecutionRequest,
        executor: &dyn Executor,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        request.validate()?;
        write_control_message(&mut std::io::sink(), &request)?;
        let producer = ExecutionProducer {
            executor: executor.identity().into(),
            environment: EXECUTION_ENVIRONMENT.into(),
            verifier: blobray_verification::VERIFIER.into(),
        };
        let mut work = ExecutionWork {
            producer: producer.clone(),
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            request: request.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::Execute { request, producer },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Execution(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    pub fn start_analyze_function(
        &self,
        project: &Path,
        mut request: FunctionRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        if let FunctionSource::Image { image } = &request.source {
            let revision = writer.project().image_revision(image)?;
            if request
                .revision
                .as_ref()
                .is_some_and(|selected| selected != &revision)
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "image belongs to another revision",
                ));
            }
            request.revision = Some(revision);
        }
        if request.revision.is_none() {
            request.revision = Some(
                writer
                    .project()
                    .current()?
                    .ok_or_else(|| Error::new(ErrorCode::NotFound, "project has no revision"))?,
            );
        }
        let mut work = FunctionWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            request: request.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::AnalyzeFunction { request },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Function(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    pub fn start_analyze_project(
        &self,
        project: &Path,
        input: InvestigationInput,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        match input {
            InvestigationInput::Plan { plan } => {
                self.start_investigation_plan(project, &plan, budget)
            }
            InvestigationInput::Automatic { request, producer } => self.start_scenario(
                project,
                ScenarioRequest::Investigate { request, producer },
                budget,
            ),
        }
    }
    pub fn start_research(
        &self,
        project: &Path,
        request: ResearchRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_scenario(project, ScenarioRequest::Research { request }, budget)
    }
    pub fn start_propose_data(
        &self,
        project: &Path,
        request: DataProposalRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_scenario(project, ScenarioRequest::ProposeData { request }, budget)
    }
    pub fn start_propose_constant(
        &self,
        project: &Path,
        request: ConstantProposalRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_scenario(
            project,
            ScenarioRequest::ProposeConstant { request },
            budget,
        )
    }
    pub fn start_propose_register(
        &self,
        project: &Path,
        request: RegisterProposalRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_scenario(
            project,
            ScenarioRequest::ProposeRegister { request },
            budget,
        )
    }
    pub fn start_replay(
        &self,
        project: &Path,
        execution: ArtifactId,
        executor: &dyn Executor,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_scenario(
            project,
            ScenarioRequest::Replay {
                execution,
                producer: ExecutionProducer {
                    executor: executor.identity().into(),
                    environment: EXECUTION_ENVIRONMENT.into(),
                    verifier: blobray_verification::VERIFIER.into(),
                },
            },
            budget,
        )
    }
    fn start_scenario(
        &self,
        project: &Path,
        mut request: ScenarioRequest,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        if let ScenarioRequest::Investigate { request, .. } = &mut request {
            if let Some(image) = &request.image {
                let revision = writer.project().image_revision(image)?;
                if request.revision.as_ref().is_some_and(|id| id != &revision) {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "image belongs to another revision",
                    ));
                }
                request.revision = Some(revision);
            }
            if request.revision.is_none() {
                request.revision =
                    Some(writer.project().current()?.ok_or_else(|| {
                        Error::new(ErrorCode::NotFound, "project has no revision")
                    })?);
            }
        }
        if let ScenarioRequest::ProposeData { request } = &request {
            writer
                .project()
                .check_knowledge_base(&request.expected_base)?;
        }
        if let ScenarioRequest::ProposeConstant { request } = &request {
            writer
                .project()
                .check_knowledge_base(&request.expected_base)?;
        }
        if let ScenarioRequest::ProposeRegister { request } = &request {
            writer
                .project()
                .check_knowledge_base(&request.expected_base)?;
        }
        let mut work = ScenarioWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            request: request.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::Scenario { request },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Scenario(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    fn start_investigation_plan(
        &self,
        project: &Path,
        plan: &InvestigationPlan,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        blobray_store::validate_investigation_plan(plan)?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        if writer.project().id() != &plan.recipe.project {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "plan belongs to another project",
            ));
        }
        let mut work = InvestigationWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            plan: plan.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::Investigate {
                revision: plan.recipe.request.revision.clone().unwrap(),
                plan: plan.id.clone(),
            },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Investigation(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    pub fn start_knowledge(
        &self,
        project: &Path,
        change: &KnowledgeChange,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        write_control_message(std::io::sink(), change)?;
        blobray_knowledge::validate_change(change)?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity missing",
            ));
        }
        let project = std::path::absolute(project).map_err(storage_io)?;
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        self.temporary.root(&*self.host)?;
        let mut reservation = self.temporary.reserve()?;
        let mut writer = Writer::open(&project)?;
        writer
            .project()
            .check_knowledge_base(&change.expected_base)?;
        let mut work = KnowledgeWork {
            schema: 1,
            run: ArtifactId::of_bytes(b"admission").as_str().parse()?,
            project: OriginPath::from_path(&project),
            change: change.clone(),
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        write_control_message(&mut std::io::sink(), &work)?;
        let (record, stage) = writer.register_operation(
            budget,
            self.host.owner()?,
            RunOperation::Knowledge {
                change: change.clone(),
            },
            |stage| reservation.attach(stage),
        )?;
        work.run = record.id.clone();
        self.launch_durable(
            &mut jobs,
            project,
            DurableAdmission {
                writer,
                record,
                stage,
                work: DurableWork::Knowledge(Box::new(work)),
                permit,
                reservation,
            },
        )
    }
    /// Admit a read-only operation in the same owned job set as imports.
    pub fn start_query(
        &self,
        project: &Path,
        query: ReadQuery,
        budget: ResourceBudget,
    ) -> Result<RunHandle> {
        self.start_query_owned(project, query, budget, None)
    }
    pub(crate) fn start_query_owned(
        &self,
        project: &Path,
        mut query: ReadQuery,
        budget: ResourceBudget,
        keep_plan: Option<Plan>,
    ) -> Result<RunHandle> {
        let mut jobs = self.jobs.lock().unwrap();
        let permit = self.admit(&mut jobs)?;
        budget.validate()?;
        if budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working capacity required",
            ));
        }
        let started_ms = self.host.now_ms();
        let deadline_ms = started_ms
            .checked_add(budget.timeout_ms)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "deadline overflow"))?;
        let path = std::path::absolute(project).map_err(storage_io)?;
        if let ReadQuery::Knowledge { revision, .. } = &mut query
            && revision.is_none()
        {
            *revision = Project::open(&path)?.current_knowledge()?;
        }
        if !matches!(
            query,
            ReadQuery::ExecutePlan { .. }
                | ReadQuery::AuditTargets { .. }
                | ReadQuery::Restore { .. }
                | ReadQuery::ImportLegacy { .. }
        ) {
            if let ReadQuery::PlanInvestigation { request, .. } = &mut query
                && let Some(image) = &request.image
            {
                let revision = Project::open(&path)?.image_revision(image)?;
                if request
                    .revision
                    .as_ref()
                    .is_some_and(|selected| selected != &revision)
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "image belongs to another revision",
                    ));
                }
                request.revision = Some(revision);
            }
            let view = ReadView::open(&path)?;
            let revision = match &mut query {
                ReadQuery::Inventory { revision } | ReadQuery::Select { revision, .. } => {
                    Some(revision)
                }
                ReadQuery::Plan { request } => Some(&mut request.revision),
                ReadQuery::LinkPlan { request, .. } => Some(&mut request.revision),
                ReadQuery::NamedLinkPlan { request, .. } => Some(&mut request.revision),
                ReadQuery::PlanInvestigation { request, .. } => Some(&mut request.revision),
                _ => None,
            };
            if let Some(revision) = revision
                && revision.is_none()
            {
                *revision = Some(view.current()?.ok_or_else(|| {
                    Error::new(ErrorCode::NotFound, "project has no imported revision")
                })?);
            }
        }
        let root = self.temporary.root(&*self.host)?;
        let reservation = self.temporary.reserve()?;
        let temporary = self
            .temporary
            .workspace(self.host.clone(), &root, reservation)?;
        let stage = temporary.stage.clone();
        let run = temporary.run.clone();
        crate::temporary::configure(&stage, &run, self.temporary.policy.operation_bytes)?;
        let work = QueryWork {
            schema: 2,
            run: run.clone(),
            project: OriginPath::from_path(&path),
            query,
            budget: budget.clone(),
            started_ms,
            deadline_ms,
        };
        crate::protocol::write_request(
            std::fs::File::create(stage.join("query.json")).map_err(storage_io)?,
            &work,
        )?;
        let record = RunRecord {
            schema: 28,
            operation: blobray_store::RunOperation::Query,
            resolved_operation: None,
            image: None,
            analysis: None,
            publication: None,
            knowledge: None,
            execution: None,
            semantic_ir: None,
            assessment: None,
            id: run,
            owner: self.host.owner()?,
            budget,
            base: None,
            revision: None,
            state: RunState::Registered,
            error: None,
            diagnostics: Some(RunDiagnostics::default()),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                record: record.clone(),
                cancel: false,
                committing: false,
                done: false,
                events: Default::default(),
                sequence: 0,
                event_capacity: self.limits.event_capacity,
                output: None,
            }),
            changed: Condvar::new(),
            _permit: permit.clone(),
        });
        {
            let mut state = shared.state.lock().unwrap();
            event(&mut state, RunState::Registered);
        }
        let handle = RunHandle {
            shared: shared.clone(),
        };
        let host = self.host.clone();
        let task = thread::Builder::new()
            .name("blobray-query".into())
            .spawn(move || {
                let _retained_plan = keep_plan;
                let mut record = record;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    checkpoint(&shared, &*host, deadline_ms)?;
                    let report = execute_worker(
                        &*host,
                        &stage,
                        deadline_ms,
                        &mut record,
                        &shared,
                        |record| {
                            let mut state = shared.state.lock().unwrap();
                            state.record = record.clone();
                            event(&mut state, RunState::Running);
                            shared.changed.notify_all();
                            Ok(())
                        },
                    )?;
                    if report.prepared.is_some() {
                        return Err(Error::new(
                            ErrorCode::WorkerProtocol,
                            "query worker returned a publication receipt",
                        ));
                    }
                    checkpoint(&shared, &*host, deadline_ms)?;
                    QueryOutput::open(temporary, &stage, &work, report, host.clone(), permit)
                }))
                .unwrap_or_else(|_| {
                    Err(Error::new(
                        ErrorCode::WorkerProtocol,
                        "query supervisor or host panicked",
                    ))
                });
                let output = match result {
                    Ok(output) => {
                        // Linearize successful completion against cancellation acceptance.
                        let mut state = shared.state.lock().unwrap();
                        if state.cancel {
                            record.state = RunState::Cancelled;
                            record.error =
                                Some(Error::new(ErrorCode::Cancelled, "query cancelled"));
                            None
                        } else {
                            state.committing = true;
                            record.state = RunState::Completed;
                            record.assessment = Some(output.assessment().clone());
                            Some(output)
                        }
                    }
                    Err(mut error) => {
                        truncate_message(&mut error.message);
                        record.state = failure_state(&error);
                        record.error = Some(error);
                        None
                    }
                };
                finish(&shared, record, output);
            })
            .map_err(storage_io)?;
        jobs.tasks.push((handle.clone(), task));
        Ok(handle)
    }
    pub fn query(
        &self,
        project: &Path,
        query: ReadQuery,
        budget: ResourceBudget,
    ) -> Result<QueryOutput> {
        self.start_query(project, query, budget)?.take_output()
    }
    /// Synchronous adapter over the same owned job path; returns a compact result.
    pub fn import(
        &self,
        project: &Path,
        inputs: Vec<ImportInput>,
        target: Target,
        budget: ResourceBudget,
    ) -> Result<RunRecord> {
        let record = self.start_import(project, inputs, target, budget)?.wait();
        if record.state == RunState::Completed {
            Ok(record)
        } else {
            Err(record.error.clone().unwrap_or_else(|| {
                Error::new(
                    ErrorCode::Storage,
                    format!("import ended as {:?}", record.state),
                )
            }))
        }
    }
    pub fn recover(&self, project: &Path) -> Result<Vec<RunRecord>> {
        Writer::open(project)?.recover(&|owner| self.host.alive(owner), &|stage| {
            self.host.reclaim(stage)
        })
    }
    pub fn shutdown(&self) {
        let _drain = self.shutdown_lock.lock().unwrap();
        let tasks = {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.closed = true;
            for (handle, _) in &jobs.tasks {
                handle.cancel();
            }
            std::mem::take(&mut jobs.tasks)
        };
        for (_, thread) in tasks {
            let _ = thread.join();
        }
    }
}
impl Drop for Application {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn publish_state(writer: &mut Writer, record: &RunRecord, shared: &Shared) -> Result<()> {
    writer.update_run(record)?;
    {
        let mut state = shared.state.lock().unwrap();
        state.record = record.clone();
        event(&mut state, record.state);
    }
    shared.changed.notify_all();
    Ok(())
}

fn checkpoint(shared: &Shared, host: &dyn OperationHost, deadline_ms: u64) -> Result<()> {
    if shared.state.lock().unwrap().cancel {
        return Err(Error::new(ErrorCode::Cancelled, "operation cancelled"));
    }
    if host.now_ms() >= deadline_ms {
        return Err(Error::new(
            ErrorCode::TimedOut,
            "operation time budget exhausted",
        ));
    }
    Ok(())
}

struct DurableAdmission {
    writer: Writer,
    record: RunRecord,
    stage: PathBuf,
    work: DurableWork,
    permit: Arc<()>,
    reservation: crate::temporary::TemporaryReservation,
}
enum DurableWork {
    Ir(Box<IrWork>),
    Scenario(Box<ScenarioWork>),
    Execution(Box<ExecutionWork>),
    Knowledge(Box<KnowledgeWork>),
    Import(ImportWork),
    Image(Box<ImageWork>),
    Function(Box<FunctionWork>),
    Investigation(Box<InvestigationWork>),
}
impl DurableWork {
    fn started_ms(&self) -> u64 {
        match self {
            Self::Ir(w) => w.started_ms,
            Self::Scenario(w) => w.started_ms,
            Self::Execution(w) => w.started_ms,
            Self::Knowledge(w) => w.started_ms,
            Self::Import(w) => w.started_ms,
            Self::Image(w) => w.started_ms,
            Self::Function(w) => w.started_ms,
            Self::Investigation(w) => w.started_ms,
        }
    }
    fn deadline_ms(&self) -> u64 {
        match self {
            Self::Ir(w) => w.deadline_ms,
            Self::Scenario(w) => w.deadline_ms,
            Self::Execution(w) => w.deadline_ms,
            Self::Knowledge(w) => w.deadline_ms,
            Self::Import(w) => w.deadline_ms,
            Self::Image(w) => w.deadline_ms,
            Self::Function(w) => w.deadline_ms,
            Self::Investigation(w) => w.deadline_ms,
        }
    }
}
enum Retained {
    Ir(blobray_store::RetainedIr),
    Execution(blobray_store::RetainedExecution),
    Knowledge(Box<blobray_store::RetainedKnowledge>),
    Import(blobray_store::RetainedImport),
    Image(blobray_store::RetainedImage),
    Function(blobray_store::RetainedFunction),
    Investigation(Box<blobray_store::RetainedInvestigation>),
}
fn supervise(
    mut writer: Writer,
    mut record: RunRecord,
    stage: PathBuf,
    work: DurableWork,
    host: Arc<dyn OperationHost>,
    shared: Arc<Shared>,
    mut reservation: crate::temporary::TemporaryReservation,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        crate::temporary::configure(&stage, &record.id, reservation.capacity())?;
        match &work {
            DurableWork::Ir(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("ir.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Scenario(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("scenario.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Execution(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("execution.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Knowledge(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("knowledge.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Investigation(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("investigation.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Import(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("request.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Function(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("function.json")).map_err(storage_io)?,
                work,
            )?,
            DurableWork::Image(work) => crate::protocol::write_request(
                std::fs::File::create(stage.join("image.json")).map_err(storage_io)?,
                work,
            )?,
        }
        checkpoint(&shared, &*host, work.deadline_ms())?;
        let report = execute_worker(
            &*host,
            &stage,
            work.deadline_ms(),
            &mut record,
            &shared,
            |record| publish_state(&mut writer, record, &shared),
        )?;
        if record
            .diagnostics
            .as_ref()
            .and_then(|d| d.progress)
            .is_none()
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "worker omitted work accounting",
            ));
        }
        let prepared = report
            .prepared
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "worker omitted prepared receipt"))?;
        record.state = RunState::Validating;
        publish_state(&mut writer, &record, &shared)?;
        let coordinator_run = record.id.clone();
        let environment = CoordinatorEnvironment {
            host: &*host,
            shared: &shared,
            stage: &stage,
            run: &coordinator_run,
            last_write: std::cell::Cell::new(None),
        };
        let mut context = crate::RunContext::new(
            &environment,
            work.started_ms(),
            work.deadline_ms(),
            &record.budget,
            record.diagnostics.as_ref().and_then(|d| d.progress),
        )?;
        let publication_memory = WorkingMemory::new(record.budget.working_memory_bytes.unwrap())?;
        let publication_reservation = if matches!(
            work,
            DurableWork::Ir(_)
                | DurableWork::Investigation(_)
                | DurableWork::Knowledge(_)
                | DurableWork::Execution(_)
                | DurableWork::Scenario(_)
        ) {
            Some(publication_memory.reserve(2 * 1024 * 1024, context.position())?)
        } else {
            None
        };
        let result = (|| {
            context.set_position(RunPosition {
                phase: RunPhase::Retain,
                ..Default::default()
            });
            context.checkpoint(0)?;
            let resolution = if let DurableWork::Scenario(work) = &work {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(stage.join("resolved.json"))
                    .map_err(storage_io)?
                    .take(65537)
                    .read_to_end(&mut bytes)
                    .map_err(storage_io)?;
                if bytes.len() > 65536 {
                    return Err(Error::new(
                        ErrorCode::WorkerProtocol,
                        "scenario resolution exceeds bound",
                    ));
                }
                let resolved: crate::scenarios::Resolution = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::new(ErrorCode::WorkerProtocol, e.to_string()))?;
                crate::scenarios::validate_resolution(
                    writer.project(),
                    &work.request,
                    &resolved,
                    &publication_memory,
                    &mut context,
                )?;
                record.resolved_operation = Some(Box::new(resolved.operation.clone()));
                Some(resolved)
            } else {
                None
            };
            let retained = match (&work, prepared) {
                (DurableWork::Scenario(_), PreparedReceipt::Execution(p))
                    if matches!(record.effective_operation(), RunOperation::Execute { .. }) =>
                {
                    Retained::Execution(writer.retain_execution(
                        &record,
                        &p,
                        &publication_memory,
                        &mut context,
                    )?)
                }
                (DurableWork::Scenario(_), PreparedReceipt::Knowledge(p))
                    if matches!(record.effective_operation(), RunOperation::Knowledge { .. }) =>
                {
                    Retained::Knowledge(Box::new(writer.retain_knowledge(
                        &record,
                        &p,
                        &mut context,
                    )?))
                }
                (DurableWork::Scenario(_), PreparedReceipt::Function(p))
                    if matches!(
                        record.effective_operation(),
                        RunOperation::AnalyzeFunction { .. }
                    ) =>
                {
                    Retained::Function(writer.retain_function(&record, &p, &mut context)?)
                }
                (DurableWork::Scenario(_), PreparedReceipt::Investigation(p))
                    if matches!(
                        record.effective_operation(),
                        RunOperation::Investigate { .. }
                    ) =>
                {
                    Retained::Investigation(Box::new(
                        writer.retain_investigation(
                            &record,
                            &p,
                            resolution
                                .as_ref()
                                .and_then(|r| r.plan.as_ref())
                                .ok_or_else(|| {
                                    Error::new(ErrorCode::WorkerProtocol, "scenario omitted plan")
                                })?,
                            &mut context,
                        )?,
                    ))
                }
                (DurableWork::Ir(_), PreparedReceipt::Ir(p)) => Retained::Ir(writer.retain_ir(
                    &record,
                    &p,
                    &publication_memory,
                    &mut context,
                )?),
                (DurableWork::Execution(_), PreparedReceipt::Execution(p)) => Retained::Execution(
                    writer.retain_execution(&record, &p, &publication_memory, &mut context)?,
                ),
                (DurableWork::Knowledge(_), PreparedReceipt::Knowledge(prepared)) => {
                    Retained::Knowledge(Box::new(writer.retain_knowledge(
                        &record,
                        &prepared,
                        &mut context,
                    )?))
                }
                (DurableWork::Investigation(work), PreparedReceipt::Investigation(prepared)) => {
                    Retained::Investigation(Box::new(writer.retain_investigation(
                        &record,
                        &prepared,
                        &work.plan,
                        &mut context,
                    )?))
                }
                (DurableWork::Import(_), PreparedReceipt::Import(prepared)) => {
                    Retained::Import(writer.retain_candidate(&record, &prepared, &mut context)?)
                }
                (DurableWork::Function(_), PreparedReceipt::Function(prepared)) => {
                    Retained::Function(writer.retain_function(&record, &prepared, &mut context)?)
                }
                (DurableWork::Image(_), PreparedReceipt::Image(prepared)) => {
                    Retained::Image(writer.retain_image(&record, &prepared, &mut context)?)
                }
                _ => {
                    return Err(Error::new(
                        ErrorCode::WorkerProtocol,
                        "worker returned a receipt for another operation",
                    ));
                }
            };
            context.set_position(RunPosition {
                phase: RunPhase::Publish,
                ..Default::default()
            });
            context.checkpoint(0)?;
            Ok(retained)
        })();
        if publication_reservation.is_some() {
            context.working_memory(publication_memory.observation());
        }
        record.diagnostics.as_mut().unwrap().progress = Some(context.snapshot());
        let retained = result?;
        {
            let mut state = shared.state.lock().unwrap();
            if state.cancel {
                return Err(Error::new(
                    ErrorCode::Cancelled,
                    "cancelled before publication",
                ));
            }
            if host.now_ms() >= work.deadline_ms() {
                return Err(Error::new(
                    ErrorCode::TimedOut,
                    "time budget exhausted before publication",
                ));
            }
            state.committing = true;
        }
        match retained {
            Retained::Ir(retained) => writer.publish_ir(&mut record, retained),
            Retained::Execution(retained) => writer.publish_execution(&mut record, retained),
            Retained::Knowledge(retained) => {
                writer.publish_knowledge(&mut record, *retained, &mut context)
            }
            Retained::Investigation(retained) => {
                let result = writer.publish_investigation(&mut record, *retained, &mut context);
                record.diagnostics.as_mut().unwrap().progress = Some(context.snapshot());

                result
            }
            Retained::Import(retained) => writer.publish_run(&mut record, retained),
            Retained::Image(retained) => writer.publish_image(&mut record, retained),
            Retained::Function(retained) => writer.publish_function(&mut record, retained),
        }
    }))
    .unwrap_or_else(|_| {
        Err(Error::new(
            ErrorCode::Storage,
            "supervisor or host adapter panicked",
        ))
    });
    if let Err(mut error) = result {
        truncate_message(&mut error.message);
        record.state = match error.code {
            ErrorCode::Cancelled => RunState::Cancelled,
            ErrorCode::TimedOut => RunState::TimedOut,
            ErrorCode::ResourceLimited => RunState::ResourceLimited,
            _ => RunState::Failed,
        };
        record.error = Some(error);
        record.resolved_operation = None;
        if let Ok(committed) = writer.project().run(&record.id)
            && committed.state == RunState::Completed
        {
            // A durable publication wins over a lost/erroring completion delivery.
            record = committed;
        } else if let Err(error) = writer.update_run(&record) {
            record
                .diagnostics
                .get_or_insert_with(RunDiagnostics::default)
                .secondary(error);
        }
    }
    if let Some(progress) = record.diagnostics.as_ref().and_then(|d| d.progress)
        && let Err(error) = host.save_progress(
            &stage,
            &ProgressRecord {
                schema: 1,
                run: record.id.clone(),
                progress,
            },
        )
    {
        record.diagnostics.as_mut().unwrap().secondary(error);
        let _ = writer.update_run(&record);
    }
    // A cleanup failure never rewrites a successfully committed outcome. Recovery
    // can reclaim the stage later; surface its diagnostic in the terminal record.
    if let Err(error) = writer.cleanup_stage(&record.id) {
        record
            .diagnostics
            .get_or_insert_with(RunDiagnostics::default)
            .secondary(error);
        let _ = writer.update_run(&record);
    }
    if !stage.try_exists().unwrap_or(true) {
        reservation.released();
    } else {
        record.diagnostics.as_mut().unwrap().secondary(Error::new(
            ErrorCode::RecoveryRequired,
            format!("temporary residue: {}", stage.display()),
        ));
        let _ = writer.update_run(&record);
    }
    drop(reservation);
    drop(writer);
    finish(&shared, record, None);
}
fn finish(shared: &Shared, record: RunRecord, output: Option<QueryOutput>) {
    let mut state = shared.state.lock().unwrap();
    state.record = record;
    state.output = output;
    let terminal = state.record.state;
    event(&mut state, terminal);
    state.done = true;
    shared.changed.notify_all();
}
fn failure_state(error: &Error) -> RunState {
    match error.code {
        ErrorCode::Cancelled => RunState::Cancelled,
        ErrorCode::TimedOut => RunState::TimedOut,
        ErrorCode::ResourceLimited => RunState::ResourceLimited,
        _ => RunState::Failed,
    }
}

fn execute_worker(
    host: &dyn OperationHost,
    stage: &Path,
    deadline_ms: u64,
    record: &mut RunRecord,
    shared: &Shared,
    mut running: impl FnMut(&RunRecord) -> Result<()>,
) -> Result<WorkerReport> {
    // Publish the active attempt before starting a worker that may immediately
    // open the same SQLite database read-only. Otherwise its first schema read
    // races this short write transaction and can fail with SQLITE_BUSY.
    record.state = RunState::Running;
    running(record)?;
    let mut worker = host.launch(stage, &record.budget, deadline_ms)?;
    let mut stop = None;
    let report = loop {
        if stop.is_none()
            && let Err(error) = checkpoint(shared, host, deadline_ms)
        {
            worker.cancel()?;
            stop = Some(error);
        }
        if let Some(progress) = worker.progress()? {
            validate_progress(record, &progress)?;
            record.diagnostics.as_mut().unwrap().progress = Some(progress);
            observe_shared(shared, progress);
        }
        if let Some(report) = worker.poll()? {
            break report;
        }
        thread::sleep(Duration::from_millis(record.budget.poll_ms));
    };
    drop(worker); // Containment and reaping must finish before publication/cleanup.
    if report.schema != 6 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported worker report",
        ));
    }
    if let Some(progress) = report.diagnostics.progress {
        validate_progress(record, &progress)?;
    }
    if report.state == RunState::Completed
        && report.diagnostics.progress.is_none_or(|p| p.stop.is_some())
    {
        return Err(Error::new(
            ErrorCode::Integrity,
            "worker omitted final work accounting",
        ));
    }
    let previous_progress = record.diagnostics.as_ref().and_then(|d| d.progress);
    record.diagnostics = Some(report.diagnostics.clone());
    if record.diagnostics.as_ref().unwrap().progress.is_none() {
        record.diagnostics.as_mut().unwrap().progress = previous_progress;
    }
    if let Some(error) = stop {
        if let Some(worker_error) = report.error.clone() {
            record.diagnostics.as_mut().unwrap().secondary(worker_error);
        }
        return Err(error);
    }
    if report.state != RunState::Completed {
        return Err(report.error.clone().unwrap_or_else(|| {
            Error::new(ErrorCode::Storage, "worker failed without a diagnostic")
        }));
    }
    if report.error.is_some() {
        return Err(Error::new(
            ErrorCode::WorkerProtocol,
            "successful worker returned an error",
        ));
    }
    Ok(report)
}

fn event(state: &mut State, phase: RunState) {
    state.sequence += 1;
    state.events.push_back(RunEvent {
        run: state.record.id.clone(),
        sequence: state.sequence,
        state: phase,
        progress: state.record.diagnostics.as_ref().and_then(|d| d.progress),
    });
    if state.events.len() > state.event_capacity {
        state.events.pop_front();
    }
}

fn validate_progress(record: &RunRecord, progress: &RunProgress) -> Result<()> {
    let previous = record.diagnostics.as_ref().and_then(|d| d.progress);
    if progress.work_used > record.budget.max_work_units.unwrap_or(u64::MAX)
        || previous.is_some_and(|old| {
            progress.sequence < old.sequence
                || progress.work_used < old.work_used
                || progress.elapsed_ms < old.elapsed_ms
        })
    {
        return Err(Error::new(
            ErrorCode::Integrity,
            "worker progress regressed or exceeded budget",
        ));
    }
    Ok(())
}
fn observe_shared(shared: &Shared, progress: RunProgress) {
    let mut state = shared.state.lock().unwrap();
    let diagnostics = state
        .record
        .diagnostics
        .get_or_insert_with(RunDiagnostics::default);
    if diagnostics.progress == Some(progress) {
        return;
    }
    diagnostics.progress = Some(progress);
    let phase = state.record.state;
    event(&mut state, phase);
    shared.changed.notify_all();
}
struct CoordinatorEnvironment<'a> {
    host: &'a dyn OperationHost,
    shared: &'a Shared,
    stage: &'a Path,
    run: &'a RunId,
    last_write: std::cell::Cell<Option<u64>>,
}
impl RunEnvironment for CoordinatorEnvironment<'_> {
    fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }
    fn cancelled(&self) -> bool {
        self.shared.state.lock().unwrap().cancel
    }
    fn observe(&self, progress: &RunProgress) -> Result<()> {
        let now = self.now_ms();
        if self
            .last_write
            .get()
            .is_none_or(|previous| now.saturating_sub(previous) >= 100)
        {
            self.host.save_progress(
                self.stage,
                &ProgressRecord {
                    schema: 1,
                    run: self.run.clone(),
                    progress: *progress,
                },
            )?;
            observe_shared(self.shared, *progress);
            self.last_write.set(Some(now));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    struct Host {
        release: Arc<AtomicBool>,
        reaped: Arc<AtomicBool>,
        omit_accounting: bool,
    }
    struct Worker {
        release: Arc<AtomicBool>,
        reaped: Arc<AtomicBool>,
        report: Option<WorkerReport>,
        observed: RunProgress,
        cancel: bool,
    }
    impl OperationWorker for Worker {
        fn progress(&self) -> Result<Option<RunProgress>> {
            Ok(Some(self.observed))
        }
        fn poll(&mut self) -> Result<Option<WorkerReport>> {
            if self.cancel {
                return Ok(Some(WorkerReport {
                    schema: 6,
                    diagnostics: RunDiagnostics::default(),
                    state: RunState::Cancelled,
                    prepared: None,
                    error: Some(Error::new(ErrorCode::Cancelled, "fixture cancelled")),
                }));
            }
            if self.release.load(Ordering::SeqCst) {
                Ok(self.report.take())
            } else {
                Ok(None)
            }
        }
        fn cancel(&mut self) -> Result<()> {
            self.cancel = true;
            Ok(())
        }
    }
    impl Drop for Worker {
        fn drop(&mut self) {
            self.reaped.store(true, Ordering::SeqCst);
        }
    }
    impl RunEnvironment for Host {
        fn now_ms(&self) -> u64 {
            OperationHost::now_ms(self)
        }
        fn cancelled(&self) -> bool {
            false
        }
        fn observe(&self, _: &RunProgress) -> Result<()> {
            Ok(())
        }
    }
    impl OperationHost for Host {
        fn temporary_root(&self, requested: Option<&Path>) -> Result<PathBuf> {
            let root = requested.map(Path::to_owned).unwrap_or_else(|| {
                std::env::temp_dir().join(format!("blobray-test-runtime-{}", std::process::id()))
            });
            std::fs::create_dir_all(&root).map_err(storage_io)?;
            Ok(root)
        }
        fn now_ms(&self) -> u64 {
            static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            START.get_or_init(Instant::now).elapsed().as_millis() as u64
        }
        fn save_progress(&self, _: &Path, _: &ProgressRecord) -> Result<()> {
            Ok(())
        }
        fn owner(&self) -> Result<OwnerIdentity> {
            Ok(OwnerIdentity {
                pid: 123,
                start_ticks: 456,
                boot_id: "fixture".into(),
            })
        }
        fn alive(&self, _: &OwnerIdentity) -> Result<bool> {
            Ok(false)
        }
        fn reclaim(&self, _: &Path) -> Result<()> {
            Ok(())
        }
        fn launch(
            &self,
            stage: &Path,
            _: &ResourceBudget,
            _: u64,
        ) -> Result<Box<dyn OperationWorker>> {
            let work: ImportWork =
                serde_json::from_reader(std::fs::File::open(stage.join("request.json")).unwrap())
                    .unwrap();
            let project = Project::open(stage.ancestors().nth(3).unwrap())?;
            assert_eq!(
                project.run(&work.run)?.state,
                RunState::Running,
                "worker must not race the coordinator's startup transaction"
            );
            let mut context = crate::RunContext::new(
                self,
                work.started_ms,
                work.deadline_ms,
                &work.budget,
                None,
            )?;
            let prepared = crate::prepare_import(stage, work, &mut context)?;
            Ok(Box::new(Worker {
                observed: context.snapshot(),
                release: self.release.clone(),
                reaped: self.reaped.clone(),
                report: Some(WorkerReport {
                    schema: 6,
                    diagnostics: RunDiagnostics {
                        progress: (!self.omit_accounting).then(|| context.snapshot()),
                        ..Default::default()
                    },
                    state: RunState::Completed,
                    prepared: Some(PreparedReceipt::Import(prepared)),
                    error: None,
                }),
                cancel: false,
            }))
        }
    }
    fn fixture() -> (
        tempfile::TempDir,
        Application,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        create_project(temp.path()).unwrap();
        std::fs::write(temp.path().join("source"), b"captured fixture").unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let reaped = Arc::new(AtomicBool::new(false));
        let app = Application::new(Arc::new(Host {
            omit_accounting: false,
            release: release.clone(),
            reaped: reaped.clone(),
        }));
        (temp, app, release, reaped)
    }
    fn start(app: &Application, path: &Path, timeout: u64) -> Result<RunHandle> {
        app.start_import(
            path,
            vec![ImportInput {
                role: "vendor".into(),
                path: path.join("source"),
                expected: None,
            }],
            Target::Riscv32Ilp32,
            ResourceBudget {
                mode: LimitMode::Watchdog,
                poll_ms: 1,
                timeout_ms: timeout,
                ..ResourceBudget::default()
            },
        )
    }
    fn running(handle: &RunHandle) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while handle.status().state == RunState::Registered {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(handle.status().state, RunState::Running);
    }
    #[test]
    fn cancellation_reaps_before_wait_and_prevents_publication() {
        let (temp, app, _, reaped) = fixture();
        let handle = start(&app, temp.path(), 5000).unwrap();
        running(&handle);
        assert!(handle.cancel());
        let result = handle.wait();
        assert_eq!(result.state, RunState::Cancelled);
        assert!(reaped.load(Ordering::SeqCst));
        assert!(revisions(temp.path()).unwrap().is_empty());
        assert_eq!(runs(temp.path()).unwrap()[0], result);
        assert!(!handle.cancel());
        assert_eq!(handle.events(0).last().unwrap().state, RunState::Cancelled);
    }
    #[test]
    fn completed_commit_is_not_relabelled_by_late_cancel() {
        let (temp, app, release, _) = fixture();
        let handle = start(&app, temp.path(), 5000).unwrap();
        release.store(true, Ordering::SeqCst);
        let result = handle.wait();
        assert_eq!(result.state, RunState::Completed);
        assert!(!handle.cancel());
        assert_eq!(
            inventory(temp.path(), None).unwrap().revision_id,
            result.revision.unwrap()
        );
        let events = handle.events(0);
        assert!(events.iter().all(|event| event.run == result.id));
        assert_eq!(
            events.last().unwrap().progress,
            result.diagnostics.as_ref().unwrap().progress
        );
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        let mut states = events.iter().map(|e| e.state).collect::<Vec<_>>();
        states.dedup();
        assert_eq!(
            states,
            vec![
                RunState::Registered,
                RunState::Running,
                RunState::Validating,
                RunState::Completed
            ]
        );
    }
    #[test]
    fn client_drop_keeps_application_ownership_and_shutdown_cancels() {
        let (temp, app, _, reaped) = fixture();
        let handle = start(&app, temp.path(), 5000).unwrap();
        running(&handle);
        drop(handle);
        assert!(!reaped.load(Ordering::SeqCst));
        drop(app);
        assert!(reaped.load(Ordering::SeqCst));
        assert_eq!(runs(temp.path()).unwrap()[0].state, RunState::Cancelled);
    }
    #[test]
    fn deadline_and_competing_writer_have_distinct_results() {
        let (temp, app, _, _) = fixture();
        let handle = start(&app, temp.path(), 100).unwrap();
        running(&handle);
        assert!(matches!(
            start(&app, temp.path(), 5000),
            Err(Error {
                code: ErrorCode::Busy,
                ..
            })
        ));
        assert_eq!(handle.wait().state, RunState::TimedOut);
        assert!(revisions(temp.path()).unwrap().is_empty());
    }
    #[test]
    fn shutdown_closes_admission() {
        let (temp, app, _, _) = fixture();
        app.shutdown();
        assert!(matches!(
            start(&app, temp.path(), 5000),
            Err(Error {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert!(runs(temp.path()).unwrap().is_empty());
    }
    #[test]
    fn cleanup_failure_preserves_primary_cause_and_committed_success() {
        for success in [false, true] {
            let (temp, app, release, _) = fixture();
            let handle = start(&app, temp.path(), 5000).unwrap();
            running(&handle);
            let stage = temp
                .path()
                .join(".blobray-next/staging")
                .join(handle.status().id.as_str());
            let lease = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(stage.join("lease.lock"))
                .unwrap();
            lease.lock_shared().unwrap();
            if success {
                release.store(true, Ordering::SeqCst);
            } else {
                assert!(handle.cancel());
            }
            let record = handle.wait();
            assert_eq!(
                record.state,
                if success {
                    RunState::Completed
                } else {
                    RunState::Cancelled
                }
            );
            if success {
                assert!(record.error.is_none());
            } else {
                assert_eq!(record.error.as_ref().unwrap().code, ErrorCode::Cancelled);
            }
            assert!(
                record
                    .diagnostics
                    .as_ref()
                    .unwrap()
                    .secondary
                    .iter()
                    .any(|e| e.code == ErrorCode::Busy)
            );
            assert_eq!(runs(temp.path()).unwrap()[0], record);
            drop(lease);
            app.recover(temp.path()).unwrap();
            assert!(!stage.exists());
        }
    }
    #[test]
    fn last_checkpoint_cannot_replace_missing_final_worker_accounting() {
        let (temp, mut app, release, reaped) = fixture();
        app.host = Arc::new(Host {
            release: release.clone(),
            reaped,
            omit_accounting: true,
        });
        release.store(true, Ordering::SeqCst);
        let record = start(&app, temp.path(), 5000).unwrap().wait();
        assert_eq!(record.state, RunState::Failed);
        assert_eq!(record.error.unwrap().code, ErrorCode::Integrity);
        assert!(record.diagnostics.unwrap().progress.is_some());
        assert!(
            Writer::open(temp.path())
                .unwrap()
                .project()
                .current()
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod operation_tests;
