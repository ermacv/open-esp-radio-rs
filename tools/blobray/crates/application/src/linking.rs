//! Captured-input linking. The host supplies tools, never selection or publication policy.
use crate::*;
use blobray_store::{TemporaryBudget, TemporaryFile};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

pub const LINK_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MEMBERS: usize = 4096;
mod evidence;
mod probe;
mod proposal;
pub(crate) use proposal::propose_companions;

#[derive(Clone, Debug)]
pub enum LinkInput {
    Object(String),
    Archive(Vec<String>),
}

/// Alias chosen by application, never an original untrusted filename.
#[derive(Clone, Debug)]
pub struct LinkMember {
    pub alias: String,
    pub occurrence: LinkObject,
}

/// Treatment of names the selected closure leaves undefined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedSymbols {
    /// Image preparation: any unresolved name fails the link.
    Error,
    /// Companion proposal: unresolved names stay undefined in a trial image.
    Report,
}

/// Fixed semantic policy is identified by contract; adapters own flags and script dialect.
pub struct LinkInvocation<'a> {
    pub executable: &'a Path,
    pub identity: &'a LinkerIdentity,
    pub contract: LinkerContract,
    pub workspace: &'a LinkWorkspace<'a>,
    pub entry: &'a [u8],
    pub roots: Vec<Vec<u8>>,
    pub forced: Vec<String>,
    pub inputs: Vec<LinkInput>,
    pub members: Vec<LinkMember>,
    pub layout: ImageLayout,
    pub definitions: Vec<(String, u32)>,
    pub unresolved: UnresolvedSymbols,
}

/// Application-owned capacity; Linux adapters cannot allocate unaccounted outputs.
pub struct LinkWorkspace<'a> {
    directory: &'a Path,
    disk: &'a TemporaryBudget,
    elf_limit: u64,
}
impl LinkWorkspace<'_> {
    pub(crate) fn for_query<'a>(
        directory: &'a Path,
        disk: &'a TemporaryBudget,
        memory: &WorkingMemory,
    ) -> LinkWorkspace<'a> {
        let capacity = memory.observation();
        LinkWorkspace {
            directory,
            disk,
            elf_limit: capacity
                .limit_bytes
                .saturating_sub(capacity.reserved_bytes + LINK_METADATA_BYTES),
        }
    }
    pub fn directory(&self) -> &Path {
        self.directory
    }
    pub fn temporary(&self) -> Result<TemporaryFile> {
        self.disk.temporary(self.directory)
    }
    pub fn external_elf(&self) -> Result<blobray_store::ExternalOutput> {
        if self.elf_limit == 0 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "no working capacity remains for ELF validation",
            ));
        }
        self.disk
            .external(&self.directory.join("image.elf"), self.elf_limit)
    }
    pub fn materialize(&self, name: &str, bytes: &[u8]) -> Result<TemporaryFile> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(invalid("link workspace name must be a single component"));
        }
        let mut file = self.disk.create(&self.directory.join(name))?;
        file.write_all(bytes).map_err(storage_io)?;
        Ok(file)
    }
    /// Exercises the actual adapter, then independently validates RV32 output/evidence.
    pub fn probe(
        &self,
        host: &dyn LinkerHost,
        executable: &Path,
        identity: &LinkerIdentity,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        probe::check(self, host, executable, identity, control)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutput {
    Elf,
    Map,
    Extraction,
    Stderr,
}
pub trait LinkOutputSink {
    fn exited(&mut self, _code: Option<i32>, _signal: Option<i32>) {}
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()>;
    fn observe(&mut self, observation: LinkObservation, control: &mut dyn RunControl)
    -> Result<()>;
    /// Takes ownership of a reaped, quota-accounted seekable output without copying.
    fn elf_file(&mut self, file: TemporaryFile) -> Result<()>;
}
pub trait LinkerHost {
    fn identify(
        &self,
        executable: &Path,
        workspace: &LinkWorkspace<'_>,
        control: &mut dyn RunControl,
    ) -> Result<LinkerIdentity>;
    fn link(
        &self,
        request: &LinkInvocation<'_>,
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()>;
}
pub(crate) struct NoLinker;
impl LinkerHost for NoLinker {
    fn identify(
        &self,
        _: &Path,
        _: &LinkWorkspace<'_>,
        _: &mut dyn RunControl,
    ) -> Result<LinkerIdentity> {
        Err(Error::new(
            ErrorCode::Unavailable,
            "linker capability was not supplied",
        ))
    }
    fn link(
        &self,
        _: &LinkInvocation<'_>,
        _: &mut dyn LinkOutputSink,
        _: &mut dyn RunControl,
    ) -> Result<()> {
        Err(Error::new(
            ErrorCode::Unavailable,
            "linker capability was not supplied",
        ))
    }
}
#[derive(Clone)]
pub struct LinkPlan {
    inner: Arc<LinkPlanInner>,
}
struct LinkPlanInner {
    description: LinkPlanDescription,
    _output: Mutex<QueryOutput>,
}
impl LinkPlan {
    pub fn from_output(output: QueryOutput) -> Result<Self> {
        let QuerySummary::LinkPlan { description } = output.summary() else {
            return Err(invalid("operation did not produce a link plan"));
        };
        validate_link_plan(description)?;
        Ok(Self {
            inner: Arc::new(LinkPlanInner {
                description: (**description).clone(),
                _output: Mutex::new(output),
            }),
        })
    }
    pub fn description(&self) -> &LinkPlanDescription {
        &self.inner.description
    }
    pub fn write(&self, output: &mut dyn Write, cancelled: &dyn Fn() -> bool) -> Result<()> {
        self.inner
            ._output
            .lock()
            .unwrap()
            .deliver(cancelled, |_, _, control| {
                let mut bytes = Vec::new();
                write_control_message(&mut bytes, self.description())?;
                for part in bytes.chunks(WORK_BLOCK) {
                    control.bytes(part.len())?;
                    output.write_all(part).map_err(storage_io)?;
                }
                output.flush().map_err(storage_io)
            })
    }
}
pub fn read_link_plan(reader: impl Read) -> Result<LinkPlanDescription> {
    let mut bytes = Vec::new();
    reader
        .take(CONTROL_MESSAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(storage_io)?;
    if bytes.len() > CONTROL_MESSAGE_BYTES {
        return Err(invalid("link plan exceeds 64 KiB"));
    }
    let plan = serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    validate_link_plan(&plan)?;
    Ok(plan)
}
pub fn validate_link_plan(plan: &LinkPlanDescription) -> Result<()> {
    let recipe = &plan.recipe;
    recipe.validate_contract()?;
    validate_request(&LinkRequest {
        companions: recipe.companions.clone(),
        revision: Some(recipe.revision.clone()),
        inputs: recipe.inputs.clone(),
        entry: recipe.entry.clone(),
        roots: recipe.roots.clone(),
        layout: recipe.layout,
        absent: recipe.absent.clone(),
    })?;
    let mut bytes = Vec::new();
    write_control_message(&mut bytes, recipe)?;
    if ArtifactId::of_bytes(&bytes).as_str() != plan.id.as_str() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "link plan identity mismatch",
        ));
    }
    let mut encoded = Vec::new();
    write_control_message(&mut encoded, plan)?;
    if encoded.len() > 60 * 1024 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "link plan exceeds 60 KiB metadata capacity",
        ));
    }
    Ok(())
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn validate_request(request: &LinkRequest) -> Result<()> {
    request.layout.validate()?;
    if request.inputs.is_empty()
        || request.inputs.len() > 512
        || request.roots.len() >= MAX_IMAGE_ROOTS
    {
        return Err(invalid(format!(
            "link request needs 1..512 input occurrences and at most {} additional roots",
            MAX_IMAGE_ROOTS - 1
        )));
    }
    for (index, input) in request.inputs.iter().enumerate() {
        if request.inputs[..index].contains(input) {
            return Err(invalid(
                "repeat an imported occurrence through a separate import binding, not a duplicate selector",
            ));
        }
    }
    for root in std::iter::once(&request.entry).chain(&request.roots) {
        if !request.inputs.contains(&root.input) {
            return Err(invalid("root input is absent from ordered link inputs"));
        }
    }
    for (index, root) in request.roots.iter().enumerate() {
        if root == &request.entry || request.roots[..index].contains(root) {
            return Err(invalid("duplicate link root"));
        }
    }
    if request.absent.len() > MAX_ABSENT_SYMBOLS
        || request.absent.iter().enumerate().any(|(index, name)| {
            name.is_empty()
                || name.len() > 512
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'))
                || request.absent[..index].contains(name)
        })
    {
        return Err(invalid("absent names must be distinct linker identifiers"));
    }
    Ok(())
}
#[derive(Clone)]
struct Member {
    input: u64,
    object: ObjectId,
    payload: Option<ArtifactId>,
    source: ArtifactId,
    alias: String,
    elf: bool,
}
struct Inventory<'a> {
    request: &'a LinkRequest,
    input: u64,
    selected: bool,
    source: Option<ArtifactId>,
    members: Vec<Member>,
    seen: Vec<u64>,
    blockers: Vec<LinkBlocker>,
}
impl Inventory<'_> {
    fn block(&mut self, message: impl Into<String>) -> Result<()> {
        if self.blockers.len() == 32 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "link blocker capacity exhausted (32)",
            ));
        }
        let mut message = message.into();
        truncate_message(&mut message);
        self.blockers.push(LinkBlocker {
            input: Some(self.input),
            code: ErrorCode::LinkBlocked,
            message,
        });
        Ok(())
    }
}
impl ElfSink for Inventory<'_> {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, d: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        if self.selected {
            self.block(d.message.clone())?;
        }
        Ok(())
    }
}
impl InventorySink for Inventory<'_> {
    fn input(&mut self, index: u64, input: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = index;
        self.selected = self.request.inputs.contains(&index);
        self.source = input.capture.artifact().cloned();
        if self.selected {
            self.seen.push(index);
            if self.source.is_none() {
                self.block("selected input was not captured")?;
            }
        }
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        if !self.selected {
            return Ok(());
        }
        if self.members.len() == MAX_MEMBERS {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "link member capacity exhausted (4096)",
            ));
        }
        let elf = object.elf.as_ref().is_some_and(|e| {
            e.bits == 32 && e.little_endian && e.machine == 243 && e.object_type == 1
        });
        if !elf || object.content.is_none() {
            self.block("selected member is not a captured relocatable RV32 ELF")?;
        }
        let member = match object.id.location {
            ObjectLocation::Standalone => 0,
            ObjectLocation::ArchiveMember { ordinal } => ordinal,
        };
        self.members.push(Member {
            input: self.input,
            object: object.id.clone(),
            payload: object.content.clone(),
            source: self
                .source
                .clone()
                .ok_or_else(|| invalid("object without capture"))?,
            alias: format!("i{}-m{}.o", self.input, member),
            elf,
        });
        Ok(())
    }
}
struct Collected {
    members: Vec<Member>,
    roots: Vec<blobray_artifacts::LinkRootFacts>,
    blockers: Vec<LinkBlocker>,
    project: ProjectId,
}
fn collect(
    project: &Project,
    request: &LinkRequest,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    mut consume: impl FnMut(
        &Member,
        &dyn ByteSource,
        &[blobray_artifacts::LinkRootFacts],
        &mut dyn RunControl,
    ) -> Result<()>,
) -> Result<Collected> {
    validate_request(request)?;
    crate::companions::resolve(project, request, memory, control)?;
    let revision = request
        .revision
        .as_ref()
        .ok_or_else(|| invalid("link admission must freeze revision"))?;
    let mut inventory = Inventory {
        request,
        input: 0,
        selected: false,
        source: None,
        members: Vec::new(),
        seen: Vec::new(),
        blockers: Vec::new(),
    };
    project.read_inventory(Some(revision), memory, control, &mut inventory)?;
    for input in &request.inputs {
        if !inventory.seen.contains(input) {
            inventory.input = *input;
            inventory.block("input occurrence does not exist")?;
        }
    }
    let selectors: Vec<_> = std::iter::once(&request.entry)
        .chain(&request.roots)
        .cloned()
        .collect();
    let mut root_facts = Vec::new();
    for input in &request.inputs {
        let selected: Vec<_> = inventory
            .members
            .iter()
            .filter(|m| m.input == *input)
            .cloned()
            .collect();
        if selected.is_empty() {
            continue;
        }
        let source = project.open_payload(&selected[0].source, control)?;
        let mut cursor = MemberCursor::new(&source, control)?;
        for member in &selected {
            control.checkpoint(1)?;
            let selected_roots: Vec<_> = selectors
                .iter()
                .filter(|r| r.input == member.input && r.symbol.object == member.object)
                .cloned()
                .collect();
            let external;
            let range;
            let bytes: &dyn ByteSource = match member.object.location {
                ObjectLocation::Standalone => &source,
                ObjectLocation::ArchiveMember { ordinal } => {
                    let current = cursor
                        .next(memory, control)?
                        .ok_or_else(|| invalid("captured membership differs from inventory"))?;
                    if current.ordinal != ordinal {
                        return Err(Error::new(
                            ErrorCode::Integrity,
                            "archive ordinal differs from captured inventory",
                        ));
                    }
                    if let Some((offset, length)) = current.payload {
                        range = SourceRange::new(&source, offset, length)?;
                        &range
                    } else {
                        let Some(payload) = &member.payload else {
                            continue;
                        };
                        external = project.open_payload(payload, control)?;
                        &external
                    }
                }
            };
            if !member.elf {
                continue;
            }
            match blobray_artifacts::inspect_link_input(
                bytes,
                member.payload.as_ref().unwrap(),
                &selected_roots,
                memory,
                control,
            ) {
                Ok(facts) => {
                    consume(member, bytes, &facts, control)?;
                    root_facts.extend(facts);
                }
                Err(e) if e.code == ErrorCode::LinkBlocked => {
                    inventory.input = *input;
                    inventory.block(e.message)?;
                }
                Err(e) => return Err(e),
            }
        }
    }
    for root in &selectors {
        if !root_facts.iter().any(|r| &r.selection == root) {
            inventory.input = root.input;
            inventory
                .block("selected root cannot be resolved to a supported executable occurrence")?;
        }
    }
    for (i, root) in root_facts.iter().enumerate() {
        if root_facts[..i].iter().any(|r| r.name == root.name) {
            inventory.input = root.selection.input;
            inventory.block("distinct roots have the same linker-visible name")?;
        }
    }
    root_facts.sort_by_key(|r| selectors.iter().position(|s| s == &r.selection).unwrap());
    Ok(Collected {
        members: inventory.members,
        roots: root_facts,
        blockers: inventory.blockers,
        project: project.id().clone(),
    })
}
pub(crate) fn make_link_plan(
    project: &Path,
    request: &LinkRequest,
    executable: &Path,
    host: &dyn LinkerHost,
    workspace: &LinkWorkspace<'_>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<LinkPlanDescription> {
    let _fixed = memory.reserve(LINK_METADATA_BYTES, control.position())?;
    let tool = host.identify(executable, workspace, control)?;
    let project = Project::open(project)?;
    let found = collect(&project, request, memory, control, |_, _, _, _| Ok(()))?;
    let recipe = LinkRecipe {
        companions: request.companions.clone(),
        linker_contract: LinkerContract::ElfAnalysisLinkV1,
        schema: 2,
        policy: 5,
        project: found.project,
        revision: request.revision.clone().unwrap(),
        inputs: request.inputs.clone(),
        entry: request.entry.clone(),
        roots: request.roots.clone(),
        layout: request.layout,
        linker: tool,
        absent: request.absent.clone(),
    };
    let mut bytes = Vec::new();
    write_control_message(&mut bytes, &recipe)?;
    let description = LinkPlanDescription {
        id: ArtifactId::of_bytes_controlled(&bytes, control)?
            .as_str()
            .parse()?,
        recipe,
        blockers: found.blockers,
    };
    validate_link_plan(&description)?;
    Ok(description)
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub plan: LinkPlanDescription,
    pub linker: OriginPath,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
fn request_from(plan: &LinkPlanDescription) -> LinkRequest {
    let p = &plan.recipe;
    LinkRequest {
        companions: p.companions.clone(),
        revision: Some(p.revision.clone()),
        inputs: p.inputs.clone(),
        entry: p.entry.clone(),
        roots: p.roots.clone(),
        layout: p.layout,
        absent: p.absent.clone(),
    }
}
struct Outputs {
    exit_code: Option<i32>,
    signal: Option<i32>,
    elf: TemporaryFile,
    map: TemporaryFile,
    extraction: TemporaryFile,
    observations: TemporaryFile,
    placements: TemporaryFile,
    exit_observations: u32,
    stderr: Vec<u8>,
    truncated: bool,
}
impl Outputs {
    fn new(workspace: &LinkWorkspace<'_>) -> Result<Self> {
        Ok(Self {
            exit_code: None,
            signal: None,
            elf: workspace.temporary()?,
            map: workspace.temporary()?,
            extraction: workspace.temporary()?,
            observations: workspace.temporary()?,
            placements: workspace.temporary()?,
            exit_observations: 0,
            stderr: Vec::new(),
            truncated: false,
        })
    }
}
impl LinkOutputSink for Outputs {
    fn elf_file(&mut self, file: TemporaryFile) -> Result<()> {
        self.elf = file;
        Ok(())
    }
    fn observe(
        &mut self,
        observation: LinkObservation,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        control.checkpoint(1)?;
        let destination = match &observation {
            LinkObservation::SectionPlacement { .. } => &mut self.placements,
            LinkObservation::ArchiveExtraction { .. } => &mut self.observations,
            LinkObservation::ToolExit { code, signal } => {
                if *code != self.exit_code || *signal != self.signal || self.exit_observations != 0
                {
                    return Err(invalid("duplicate or inconsistent tool exit"));
                }
                self.exit_observations += 1;
                return Ok(());
            }
        };
        write_control_message(
            &mut *destination,
            &LinkObservationRecord {
                schema: 1,
                observation,
            },
        )?;
        destination.write_all(b"\n").map_err(storage_io)
    }

    fn exited(&mut self, code: Option<i32>, signal: Option<i32>) {
        self.exit_code = code;
        self.signal = signal;
    }
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        control.bytes(bytes.len())?;
        match channel {
            LinkOutput::Elf => self.elf.write_all(bytes).map_err(storage_io),
            LinkOutput::Map => self.map.write_all(bytes).map_err(storage_io),
            LinkOutput::Extraction => self.extraction.write_all(bytes).map_err(storage_io),
            LinkOutput::Stderr => {
                if self.stderr.len() + bytes.len() > 8192 {
                    self.truncated = true;
                    let remove = (self.stderr.len() + bytes.len() - 8192).min(self.stderr.len());
                    self.stderr.drain(..remove);
                }
                self.stderr
                    .extend_from_slice(&bytes[bytes.len().saturating_sub(8192)..]);
                Ok(())
            }
        }
    }
}
pub fn prepare_image_worker(
    stage: &Path,
    work: &ImageWork,
    host: &dyn LinkerHost,
    control: &mut dyn RunControl,
    diagnostics: &mut Option<LinkerDiagnostics>,
) -> Result<blobray_store::PreparedImageReceipt> {
    if work.schema != 1 {
        return Err(invalid("unsupported image worker request"));
    }
    validate_link_plan(&work.plan)?;
    if !work.plan.ready() {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            "link plan has unresolved blockers",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| invalid("image working capacity missing"))?,
    )?;
    let disk = TemporaryBudget::open(stage)?;
    let mut temporary_control = blobray_store::TemporaryControl::new(control, &disk);
    let result = prepare_image_inner(
        stage,
        work,
        host,
        &memory,
        &disk,
        &mut temporary_control,
        diagnostics,
    );
    temporary_control.memory_phases(&memory.phase_observations());
    temporary_control.working_memory(memory.observation());
    result
}
fn prepare_image_inner(
    stage: &Path,
    work: &ImageWork,
    host: &dyn LinkerHost,
    memory: &WorkingMemory,
    disk: &TemporaryBudget,
    control: &mut dyn RunControl,
    diagnostics: &mut Option<LinkerDiagnostics>,
) -> Result<blobray_store::PreparedImageReceipt> {
    let _fixed = memory.reserve(LINK_METADATA_BYTES, control.position())?;
    let executable = work.linker.to_path()?;
    let directory = stage.join("staging");
    let capacity = memory.observation();
    let workspace = LinkWorkspace {
        directory: &directory,
        disk,
        elf_limit: capacity.limit_bytes - capacity.reserved_bytes,
    };
    if host.identify(&executable, &workspace, control)? != work.plan.recipe.linker {
        return Err(Error::new(
            ErrorCode::SourceChanged,
            "linker identity changed since planning",
        ));
    }
    let project = Project::open(&work.project.to_path()?)?;
    if project.id() != &work.plan.recipe.project {
        return Err(invalid("link plan belongs to another project"));
    }
    control.phase(RunPhase::Materialize)?;
    let found = collect(
        &project,
        &request_from(&work.plan),
        memory,
        control,
        |member, source, _, control| {
            let mut file = disk.create(&directory.join(&member.alias))?;
            crate::query_stream::copy_source(source, &mut file, control)?;
            file.sync_all().map_err(storage_io)
        },
    )?;
    if !found.blockers.is_empty() {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            found.blockers[0].message.clone(),
        ));
    }
    let mut forced = Vec::new();
    for root in &found.roots {
        let member = found
            .members
            .iter()
            .find(|m| m.input == root.selection.input && m.object == root.selection.symbol.object)
            .unwrap();
        if !forced.contains(&member.alias) {
            forced.push(member.alias.clone());
        }
    }
    let mut inputs = Vec::new();
    for input in &work.plan.recipe.inputs {
        let members: Vec<_> = found
            .members
            .iter()
            .filter(|m| m.input == *input && !forced.contains(&m.alias))
            .collect();
        if members.is_empty() {
            continue;
        }
        if members[0].object.location == ObjectLocation::Standalone {
            inputs.push(LinkInput::Object(members[0].alias.clone()));
        } else {
            inputs.push(LinkInput::Archive(
                members.iter().map(|m| m.alias.clone()).collect(),
            ));
        }
    }
    let definitions =
        crate::companions::resolve(&project, &request_from(&work.plan), memory, control)?;
    let mut outputs = Outputs::new(&workspace)?;
    control.phase(RunPhase::Link)?;
    let invocation = LinkInvocation {
        executable: &executable,
        identity: &work.plan.recipe.linker,
        workspace: &workspace,
        contract: work.plan.recipe.linker_contract,
        entry: &found.roots[0].name,
        roots: found.roots.iter().skip(1).map(|r| r.name.clone()).collect(),
        forced,
        inputs,
        members: found
            .members
            .iter()
            .map(|m| LinkMember {
                alias: m.alias.clone(),
                occurrence: LinkObject {
                    input: m.input,
                    object: m.object.clone(),
                },
            })
            .collect(),
        layout: work.plan.recipe.layout,
        definitions,
        unresolved: UnresolvedSymbols::Error,
    };
    let linked = host.link(&invocation, &mut outputs, control);
    *diagnostics = Some(LinkerDiagnostics {
        exit_code: outputs.exit_code,
        signal: outputs.signal,
        stderr_tail: outputs.stderr.clone(),
        stderr_truncated: outputs.truncated,
    });
    if let Err(mut error) = linked {
        if error.code == ErrorCode::LinkFailed {
            error.message = format!(
                "{}: {}{}",
                error.message,
                String::from_utf8_lossy(&outputs.stderr),
                if outputs.truncated {
                    " [diagnostics truncated]"
                } else {
                    ""
                }
            );
            truncate_message(&mut error.message);
        }
        return Err(error);
    }
    let roots = evidence::roots(&mut outputs, &found, &invocation, control)?;
    let provenance = roots.1;
    let roots = roots.0;
    let storage = Staging::with_temporary_budget(stage, disk.clone())?;
    let elf = storage.retain_temporary(outputs.elf, control)?;
    let validated = blobray_artifacts::validate_image(
        &storage.open_payload(&elf, control)?,
        &work.plan.recipe.layout,
        &roots,
        memory,
        control,
    )?;
    let manifest = ImageManifest {
        abi: validated.abi,
        linker_diagnostics: LinkerDiagnostics {
            exit_code: outputs.exit_code,
            signal: outputs.signal,
            stderr_tail: outputs.stderr,
            stderr_truncated: outputs.truncated,
        },
        schema: 3,
        synthetic: true,
        plan: work.plan.clone(),
        elf,
        map: storage.retain_temporary(outputs.map, control)?,
        extraction: storage.retain_temporary(outputs.extraction, control)?,
        provenance: storage.retain_temporary(provenance, control)?,
        observations: storage.retain_temporary(outputs.observations, control)?,
        entry: roots[0].address,
        roots,
        segments: validated.segments,
    };
    let mut encoded = Vec::new();
    write_control_message(&mut encoded, &manifest)?;
    if encoded.len() > 56 * 1024 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "image manifest exceeds 56 KiB metadata capacity",
        ));
    }
    let mut file = disk.temporary(&directory)?;
    file.write_all(&encoded).map_err(storage_io)?;
    storage.image_receipt(file, control)
}

#[cfg(test)]
mod root_limit_tests {
    use super::*;

    fn selection(index: u64) -> EntrySelection {
        EntrySelection {
            input: 0,
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"object"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 0,
                index,
            },
        }
    }

    fn request(roots: u64) -> LinkRequest {
        LinkRequest {
            companions: vec![],
            revision: None,
            inputs: vec![0],
            entry: selection(0),
            roots: (1..=roots).map(selection).collect(),
            layout: ImageLayout {
                code: ImageRegion {
                    start: 0x1000_0000,
                    length: 0x10_0000,
                },
                data: ImageRegion {
                    start: 0x2000_0000,
                    length: 0x10_0000,
                },
            },
            absent: vec![],
        }
    }

    #[test]
    fn absent_names_are_distinct_bounded_linker_identifiers() {
        let mut named = request(0);
        named.absent = vec!["putchar".into(), "puts".into()];
        validate_request(&named).unwrap();
        for absent in [
            vec!["putchar".to_owned(), "putchar".to_owned()],
            vec![String::new()],
            vec!["bad name".to_owned()],
            (0..=MAX_ABSENT_SYMBOLS).map(|i| format!("n{i}")).collect(),
        ] {
            named.absent = absent;
            assert_eq!(
                validate_request(&named).unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn an_image_holds_the_entry_and_at_most_the_remaining_roots() {
        let additional = (MAX_IMAGE_ROOTS - 1) as u64;
        validate_request(&request(additional)).unwrap();
        assert_eq!(
            validate_request(&request(additional + 1)).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
}
