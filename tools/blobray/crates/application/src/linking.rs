//! Captured-input linking. The host supplies tools, never selection or publication policy.
use crate::*;
use blobray_store::{TemporaryBudget, TemporaryFile};
use std::{
    io::{BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex},
};

pub const LINK_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MEMBERS: usize = 4096;
#[derive(Clone, Debug)]
pub enum LinkInput {
    Object(String),
    Archive(Vec<String>),
}
pub struct LinkInvocation<'a> {
    pub executable: &'a Path,
    pub identity: &'a LinkerIdentity,
    pub directory: &'a Path,
    pub entry: &'a [u8],
    pub roots: Vec<Vec<u8>>,
    pub forced: Vec<String>,
    pub inputs: Vec<LinkInput>,
    pub script: String,
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
}
pub trait LinkerHost {
    fn identify(&self, executable: &Path, control: &mut dyn RunControl) -> Result<LinkerIdentity>;
    fn link(
        &self,
        request: &LinkInvocation<'_>,
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()>;
}
pub(crate) struct NoLinker;
impl LinkerHost for NoLinker {
    fn identify(&self, _: &Path, _: &mut dyn RunControl) -> Result<LinkerIdentity> {
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
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(storage_io)?;
    if bytes.len() > 65536 {
        return Err(invalid("link plan exceeds 64 KiB"));
    }
    let plan = serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    validate_link_plan(&plan)?;
    Ok(plan)
}
pub fn validate_link_plan(plan: &LinkPlanDescription) -> Result<()> {
    let recipe = &plan.recipe;
    if recipe.schema != 1 || recipe.policy != 4 || recipe.linker.implementation != "lld-elf-22" {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported link recipe",
        ));
    }
    validate_request(&LinkRequest {
        companions: recipe.companions.clone(),
        revision: Some(recipe.revision.clone()),
        inputs: recipe.inputs.clone(),
        entry: recipe.entry.clone(),
        roots: recipe.roots.clone(),
        layout: recipe.layout,
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
    if request.inputs.is_empty() || request.inputs.len() > 512 || request.roots.len() > 15 {
        return Err(invalid(
            "link request needs 1..512 input occurrences and at most 15 additional roots",
        ));
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
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<LinkPlanDescription> {
    let _fixed = memory.reserve(LINK_METADATA_BYTES, control.position())?;
    let tool = host.identify(executable, control)?;
    let project = Project::open(project)?;
    let found = collect(&project, request, memory, control, |_, _, _, _| Ok(()))?;
    let recipe = LinkRecipe {
        companions: request.companions.clone(),
        schema: 1,
        policy: 4,
        project: found.project,
        revision: request.revision.clone().unwrap(),
        inputs: request.inputs.clone(),
        entry: request.entry.clone(),
        roots: request.roots.clone(),
        layout: request.layout,
        linker: tool,
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
    }
}
struct Outputs {
    exit_code: Option<i32>,
    signal: Option<i32>,
    elf: TemporaryFile,
    map: TemporaryFile,
    extraction: TemporaryFile,
    stderr: Vec<u8>,
    truncated: bool,
}
impl LinkOutputSink for Outputs {
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
fn script(layout: ImageLayout) -> String {
    format!(
        "MEMORY {{ CODE (rx) : ORIGIN = {:#x}, LENGTH = {:#x}\n DATA (rw) : ORIGIN = {:#x}, LENGTH = {:#x} }}\nPHDRS {{ code PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); }}\nSECTIONS {{ .text : {{ INPUT_SECTION_FLAGS (SHF_ALLOC & SHF_EXECINSTR) *(*) }} > CODE :code\n .rodata : {{ *(.rodata .rodata.* .srodata .srodata.*) }} > CODE :code\n .eh_frame : {{ *(.eh_frame) }} > CODE :code\n .data : {{ *(.data .data.* .sdata .sdata.*) }} > DATA :data\n PROVIDE(__global_pointer$ = ADDR(.data) + 0x800);\n .bss (NOLOAD) : {{ *(.bss .bss.* .sbss .sbss.*) *(COMMON) }} > DATA :data\n }}\n",
        layout.code.start, layout.code.length, layout.data.start, layout.data.length
    )
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
    let mut temporary_control = blobray_store::TemporaryControl {
        control,
        budget: &disk,
    };
    let result = prepare_image_inner(
        stage,
        work,
        host,
        &memory,
        &disk,
        &mut temporary_control,
        diagnostics,
    );
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
    if host.identify(&executable, control)? != work.plan.recipe.linker {
        return Err(Error::new(
            ErrorCode::SourceChanged,
            "linker identity changed since planning",
        ));
    }
    let project = Project::open(&work.project.to_path()?)?;
    if project.id() != &work.plan.recipe.project {
        return Err(invalid("link plan belongs to another project"));
    }
    let directory = stage.join("staging");
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
    let mut layout_script = script(work.plan.recipe.layout);
    for (name, address) in definitions {
        use std::fmt::Write as _;
        writeln!(&mut layout_script, "{name} = {address:#x};").unwrap();
    }
    let mut script_file = disk.create(&directory.join("layout.ld"))?;
    script_file
        .write_all(layout_script.as_bytes())
        .map_err(storage_io)?;
    script_file.sync_all().map_err(storage_io)?;
    let mut outputs = Outputs {
        exit_code: None,
        signal: None,
        elf: disk.temporary(&directory)?,
        map: disk.temporary(&directory)?,
        extraction: disk.temporary(&directory)?,
        stderr: Vec::new(),
        truncated: false,
    };
    control.phase(RunPhase::Link)?;
    let invocation = LinkInvocation {
        executable: &executable,
        identity: &work.plan.recipe.linker,
        directory: &directory,
        entry: &found.roots[0].name,
        roots: found.roots.iter().skip(1).map(|r| r.name.clone()).collect(),
        forced,
        inputs,
        script: "layout.ld".into(),
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
    let roots = map_roots(&mut outputs.map, &found, disk, &directory, control)?;
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
        schema: 2,
        synthetic: true,
        plan: work.plan.clone(),
        elf,
        map: storage.retain_temporary(outputs.map, control)?,
        extraction: storage.retain_temporary(outputs.extraction, control)?,
        provenance: storage.retain_temporary(provenance, control)?,
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
fn map_roots(
    map: &mut TemporaryFile,
    found: &Collected,
    disk: &TemporaryBudget,
    directory: &Path,
    control: &mut dyn RunControl,
) -> Result<(Vec<ResolvedRoot>, TemporaryFile)> {
    use std::io::Seek;
    map.rewind().map_err(storage_io)?;
    let mut reader = BufReader::with_capacity(WORK_BLOCK, map);
    let mut line = Vec::new();
    let mut proof = disk.temporary(directory)?;
    let mut addresses = vec![None; found.roots.len()];
    loop {
        line.clear();
        loop {
            control.checkpoint(1)?;
            let available = reader.fill_buf().map_err(storage_io)?;
            if available.is_empty() {
                break;
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |i| i + 1)
                .min(WORK_BLOCK);
            if line.len() + count > 65536 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "link map record exceeds 64 KiB",
                ));
            }
            let end = available[count - 1] == b'\n';
            line.extend_from_slice(&available[..count]);
            reader.consume(count);
            if end {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        control.bytes(line.len())?;
        // LLD 22 header ends with one space, followed by exactly eight spaces
        // for input sections. Symbol rows use sixteen and must never be evidence.
        let mut cursor = 0;
        for _ in 0..4 {
            while cursor < line.len() && line[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            while cursor < line.len() && !line[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
        }
        let padding = cursor;
        while cursor < line.len() && line[cursor] == b' ' {
            cursor += 1;
        }
        if cursor - padding != 9 {
            continue;
        }
        let fields: Vec<_> = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .take(5)
            .collect();
        if fields.len() < 5 {
            continue;
        }
        let Some(split) = fields[4].windows(2).position(|b| b == b":(") else {
            continue;
        };
        let alias = &fields[4][..split];
        let Some(member) = found.members.iter().find(|m| m.alias.as_bytes() == alias) else {
            continue;
        };
        let prefix = line
            .windows(fields[4].len())
            .position(|b| b == fields[4])
            .unwrap()
            + split
            + 2;
        let tail = &line[prefix..];
        let Some(end) = tail.iter().rposition(|b| *b == b')') else {
            return Err(Error::new(
                ErrorCode::LinkBlocked,
                "unrecognized linker map section record",
            ));
        };
        let section = &tail[..end];
        let hex = |bytes: &[u8]| {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
                .ok_or_else(|| invalid("invalid linker map address"))
        };
        let address = hex(fields[0])?;
        let size = hex(fields[2])?;
        let mut exact = false;
        for (index, root) in found.roots.iter().enumerate() {
            if root.selection.input == member.input
                && root.selection.symbol.object == member.object
                && root.section == section
            {
                if root.section_size != size || addresses[index].is_some() {
                    return Err(Error::new(
                        ErrorCode::LinkBlocked,
                        "root section placement is transformed or ambiguous",
                    ));
                }
                addresses[index] = Some(
                    address
                        .checked_add(root.offset)
                        .ok_or_else(|| invalid("root address overflow"))?,
                );
                exact = true;
            }
        }
        let mapping = ImageMapping {
            input: member.input,
            object: member.object.clone(),
            payload: member.payload.clone().unwrap(),
            section: section.into(),
            address,
            size,
            exact,
        };
        write_control_message(&mut proof, &mapping)?;
        proof.write_all(b"\n").map_err(storage_io)?;
    }
    let mut result = Vec::new();
    for (root, address) in found.roots.iter().zip(addresses) {
        result.push(ResolvedRoot {
            selection: root.selection.clone(),
            name: root.name.clone(),
            size: root.size,
            address: address.ok_or_else(|| {
                Error::new(
                    ErrorCode::LinkBlocked,
                    "no exact linker-map provenance for a requested root",
                )
            })?,
        });
    }
    Ok((result, proof))
}
