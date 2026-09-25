//! Shared scenario lifecycle: run directory, linked image, execution
//! submission, failure without publication and source-free preservation.
use crate::harness::{Budget, Input, ProbeCatalog, Result, Runner, args, invalid, seed};
use crate::layout::*;
use blobray_application::QuerySummary;
use blobray_domain::{
    ArtifactId, CallAbi, CallEndpoint, CompanionProposal, EffectContract, EffectContractRef,
    EntrySelection, ErrorCode, ExecutionEvidence, ExecutionRequest, ExecutionTarget,
    FunctionSource, ImageManifest, ImageMapping, KnowledgeOccurrence, LayoutProjection,
    LinkRequest, ObjectId, PreparedImageId, ProjectionRef, ReviewedCallBoundary, Revision,
    RevisionId, SymbolId, SymbolTableKind,
};
use blobray_next_host::wire::RecordDocument;
use evidence_index::LocationKind;
use object::{Object, ObjectSection, ObjectSymbol};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

/// One compared request and its in-memory result.
pub struct Artifact {
    pub label: String,
    /// Digest of the canonical request.
    pub identity: ArtifactId,
    pub request: ExecutionRequest,
    pub records: Vec<ExecutionEvidence>,
    pub verdict: Option<blobray_domain::ComparisonVerdict>,
    pub complete: bool,
    /// Vendor and production entries and the verdict of every compared case.
    pub compared: Vec<ComparedCase>,
}

/// One claimed pair: `symbol` of vendor `source` at `vendor`, compared with
/// the production probe `entry` at `production`.
struct Claimed<'a> {
    source: &'a str,
    symbol: &'a str,
    vendor: u32,
    entry: &'a str,
    production: u32,
}

/// Evidence entries of one scenario and its untriaged uncovered locations.
#[derive(Default)]
pub struct Claims {
    pub entries: Vec<evidence_index::Entry>,
    pub untriaged: std::collections::BTreeSet<evidence_index::Location>,
    /// Production probe instructions any retained execution reached.
    pub reach: std::collections::BTreeSet<u32>,
}

/// One compared case of a retained execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComparedCase {
    pub vendor: u32,
    pub production: u32,
    pub verdict: Option<blobray_domain::ComparisonVerdict>,
}

#[path = "../../../../schema/scenario-evidence.rs"]
pub mod evidence_index;

/// Authenticated inputs, their captured revision and the probe catalog.
pub struct Session {
    pub runner: Runner,
    pub run: PathBuf,
    pub revision: RevisionId,
    pub inventory: Revision,
    pub probes: ProbeCatalog,
    pub artifacts: Vec<Artifact>,
    /// Authenticated bytes of each captured input, by input index.
    inputs: Vec<Vec<u8>>,
    roles: Vec<String>,
    /// Setup results memoized by Blobray, input and request content.
    cache: crate::setup_cache::SetupCache,
    /// Whether this run's Blobray project exists; it is created only when a
    /// setup operation misses the cache.
    project: std::cell::Cell<bool>,
    /// Exported ELF bytes of each prepared image.
    images: std::cell::RefCell<BTreeMap<PreparedImageId, Vec<u8>>>,
    /// Effect contracts and projections the scenario's relations select,
    /// reviewed through git and identified by content.
    effects: Vec<EffectContract>,
    projections: Vec<LayoutProjection>,
    /// Guest instructions executed in process, and the time spent executing.
    pub executed: std::cell::Cell<(u64, f64)>,
}

/// A prepared image and its resolved roots, including the entry.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LinkedImage {
    pub image: PreparedImageId,
    pub manifest: ImageManifest,
    pub mappings: Vec<ImageMapping>,
    pub roots: BTreeMap<String, u32>,
}

pub fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

/// Input index of the compiled production probe ELF.
const PROBE_INPUT: usize = 2;

impl Session {
    /// Create the run's Blobray project from the authenticated inputs, once,
    /// when a setup operation misses the cache.
    fn ensure_project(&self) -> Result<()> {
        if self.project.get() {
            return Ok(());
        }
        let roles: Vec<&str> = self.roles.iter().map(String::as_str).collect();
        let revision = self.runner.import(&roles, &self.inputs)?;
        if revision != self.revision {
            return Err(invalid("imported revision differs from the cached setup"));
        }
        self.project.set(true);
        Ok(())
    }

    /// Export the captured bytes `request` selects into `output`, from the
    /// setup cache when present. Returns the selected data bytes.
    pub fn data(
        &self,
        name: &str,
        request: &blobray_domain::DataRequest,
        output: &Path,
    ) -> Result<Vec<u8>> {
        const FILES: [&str; 2] = ["data.bin", "object.elf"];
        let entry = self.cache.entry("data", request)?;
        if entry.load::<()>()?.is_none() {
            self.ensure_project()?;
            self.runner.data(name, request, output)?;
            let files: Vec<(&str, PathBuf)> = FILES
                .iter()
                .map(|f| (*f, output.join(f)))
                .filter(|(_, p)| p.exists())
                .collect();
            let files: Vec<(&str, &Path)> = files.iter().map(|(n, p)| (*n, p.as_path())).collect();
            entry.store(&(), &files)?;
        } else {
            fs::create_dir_all(output)?;
            for file in FILES {
                let cached = entry.file(file);
                if cached.exists() {
                    fs::copy(cached, output.join(file))?;
                }
            }
        }
        Ok(fs::read(output.join("data.bin"))?)
    }

    /// Create a fresh `run-*` directory, authenticate the inputs and read their
    /// inventory and the probe catalog of the compiled production input, from
    /// the setup cache when present.
    pub fn start(
        binary: &Path,
        output: &Path,
        budget: Budget,
        inputs: &[Input<'_>],
        scope: &str,
    ) -> Result<Self> {
        let run = start_run(output)?;
        let runner = Runner::new(binary, &run, run.join("project"), budget)?;
        let (identities, contents) = crate::harness::authenticate(inputs)?;
        let roles: Vec<_> = inputs.iter().map(|i| i.role).collect();
        runner.doc(
            "identities",
            &serde_json::json!({"sha256": identities, "roles": roles, "scope": scope}),
        )?;
        let cache = crate::setup_cache::SetupCache::new(output, binary, &roles, &identities)?;
        let entry = cache.entry("session", &serde_json::json!({"probes": PROBE_INPUT}))?;
        let (revision, inventory, probes, project) =
            match entry.load::<(RevisionId, Revision, ProbeCatalog)>()? {
                Some((revision, inventory, probes)) => (revision, inventory, probes, false),
                None => {
                    let revision = runner.import(&roles, &contents)?;
                    let inventory = runner.inventory()?;
                    let probes =
                        ProbeCatalog::capture(&runner, &revision, &inventory, PROBE_INPUT)?;
                    entry.store(&(&revision, &inventory, &probes), &[])?;
                    (revision, inventory, probes, true)
                }
            };
        if let Some(rom) = inputs.iter().position(|i| i.role == "rom") {
            verify_rom_symbols(&inventory, rom)?;
        }
        Ok(Self {
            runner,
            run,
            revision,
            inventory,
            probes,
            artifacts: vec![],
            inputs: contents,
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            cache,
            project: std::cell::Cell::new(project),
            images: Default::default(),
            effects: vec![],
            projections: vec![],
            executed: Default::default(),
        })
    }

    /// Exact code endpoint of the linked image symbol `name` at `address`,
    /// in the image `target` executes.
    pub fn image_endpoint(
        &self,
        target: &ExecutionTarget,
        object: &ObjectId,
        name: &str,
        address: u32,
    ) -> Result<CallEndpoint> {
        let symbol = image_symbol_id(&self.run.join("image/image.elf"), object, name)?;
        Ok(CallEndpoint {
            occurrence: KnowledgeOccurrence {
                revision: self.revision.clone(),
                source: target.source.clone(),
                object: object.clone(),
                symbol: Some(symbol),
            },
            boundary: ReviewedCallBoundary::Code { address },
        })
    }

    /// Exact code endpoint of the captured symbol `name` in `input`.
    pub fn input_endpoint(&self, input: u64, name: &str) -> Result<CallEndpoint> {
        let record = crate::harness::symbol(&self.inventory, input as usize, name)?;
        Ok(CallEndpoint {
            occurrence: KnowledgeOccurrence {
                revision: self.revision.clone(),
                source: FunctionSource::Input { input },
                object: record.id.object.clone(),
                symbol: Some(record.id.clone()),
            },
            boundary: ReviewedCallBoundary::Code {
                address: u32::try_from(record.value)?,
            },
        })
    }

    /// Select `contract` by content for this scenario's relations. The
    /// contract is a typed value reviewed through git; `name`, `subject` and
    /// `reason` document it and are retained with the run.
    pub fn review_effects(
        &mut self,
        name: &str,
        subject: &str,
        contract: EffectContract,
        reason: &str,
    ) -> Result<EffectContractRef> {
        self.runner.doc(
            name,
            &serde_json::json!({"subject": subject, "reason": reason, "contract": &contract}),
        )?;
        let selection = blobray_application::in_process::effect_contract_ref(&contract)?;
        if !self.effects.contains(&contract) {
            self.effects.push(contract);
        }
        Ok(selection)
    }

    /// Select `projection` by content, as `review_effects` does for contracts.
    pub fn review_projection(
        &mut self,
        name: &str,
        subject: &str,
        projection: LayoutProjection,
        reason: &str,
    ) -> Result<ProjectionRef> {
        self.runner.doc(
            name,
            &serde_json::json!({"subject": subject, "reason": reason, "projection": &projection}),
        )?;
        let selection = blobray_application::in_process::projection_ref(&projection)?;
        if !self.projections.contains(&projection) {
            self.projections.push(projection);
        }
        Ok(selection)
    }

    /// ELF bytes of every source of `target`, in source order.
    fn executables(&self, target: &ExecutionTarget) -> Result<Vec<Vec<u8>>> {
        let input = |index: u64| {
            self.inputs
                .get(index as usize)
                .cloned()
                .ok_or_else(|| invalid(format!("input {index} is not captured")))
        };
        let mut executables = vec![match &target.source {
            FunctionSource::Input { input: index } => input(*index)?,
            FunctionSource::Image { image } => self
                .images
                .borrow()
                .get(image)
                .cloned()
                .ok_or_else(|| invalid("image is not exported"))?,
        }];
        for companion in &target.companions {
            executables.push(input(*companion)?);
        }
        Ok(executables)
    }

    /// Execute and compare `request` in this process.
    fn verify(
        &self,
        request: &ExecutionRequest,
    ) -> blobray_domain::Result<blobray_application::in_process::InProcessResult> {
        let failed = |e: crate::harness::Error| {
            blobray_domain::Error::new(ErrorCode::InvalidRequest, e.to_string())
        };
        let vendor = self.executables(&request.vendor).map_err(failed)?;
        let replacement = request
            .replacement
            .as_ref()
            .map(|t| self.executables(t))
            .transpose()
            .map_err(failed)?;
        let vendor: Vec<&[u8]> = vendor.iter().map(Vec::as_slice).collect();
        let replacement: Option<Vec<&[u8]>> = replacement
            .as_ref()
            .map(|r| r.iter().map(Vec::as_slice).collect());
        let budget = self.runner.budget;
        let memory = blobray_domain::WorkingMemory::new(budget.working_memory_mib << 20)?;
        blobray_application::in_process::verify(
            &blobray_application::in_process::InProcessComparison {
                request,
                vendor: &vendor,
                replacement: replacement.as_deref(),
                effects: &self.effects,
                projections: &self.projections,
                vendor_results: None,
            },
            &blobray_backend_riscv::RiscvExecutor,
            &memory,
            &mut crate::harness::InProcessControl::new(&budget),
        )
    }

    /// Every companion the request's closure needs, proposed by a trial link
    /// against `candidates` in priority order. An unresolved name fails.
    pub fn propose(
        &self,
        request: &LinkRequest,
        linker: &Path,
        candidates: &[u64],
    ) -> Result<Vec<EntrySelection>> {
        let runner = &self.runner;
        let mut command = args(["propose-companions", "--request"]);
        command.extend([
            path_arg(&runner.doc("propose", request)?),
            "--linker".into(),
            path_arg(&std::path::absolute(linker)?),
        ]);
        for candidate in candidates {
            command.extend(["--candidate".into(), candidate.to_string().into()]);
        }
        let called = runner.call("propose", &command, 0);
        // The proposal document is retained even when names stay unresolved.
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(self.run.join("propose.json"))?).unwrap_or_default();
        let proposal: Option<CompanionProposal> =
            serde_json::from_value(document["summary"]["proposal"].clone()).ok();
        match (called, proposal) {
            (Ok(_), Some(proposal)) if proposal.unresolved.is_empty() => {
                Ok(proposal.resolved.into_iter().map(|c| c.selection).collect())
            }
            (_, Some(proposal)) => Err(invalid(format!(
                "unresolved link names: {}",
                proposal.unresolved.join(", ")
            ))),
            (Err(error), None) => Err(error),
            (Ok(_), None) => Err(invalid("propose-companions returned no proposal")),
        }
    }

    /// Complete `request` with proposed companions from `candidates`, then plan,
    /// prepare, inspect and export one linked image. `entry` names the
    /// request's entry so it resolves like the other roots. The retained plan
    /// request lists every exact companion.
    pub fn link(
        &self,
        request: &LinkRequest,
        linker: &Path,
        entry: &str,
        candidates: &[u64],
    ) -> Result<LinkedImage> {
        let key = self.cache.entry(
            "link",
            &serde_json::json!({
                "request": request,
                "linker": crate::harness::sha256(&fs::read(linker)?),
                "entry": entry,
                "candidates": candidates,
            }),
        )?;
        let exported = self.run.join("image/image.elf");
        if let Some(linked) = key.load::<LinkedImage>()? {
            fs::create_dir_all(self.run.join("image"))?;
            fs::copy(key.file("image.elf"), &exported)?;
            self.images
                .borrow_mut()
                .insert(linked.image.clone(), fs::read(&exported)?);
            return Ok(linked);
        }
        self.ensure_project()?;
        let linked = self.link_uncached(request, linker, entry, candidates)?;
        key.store(&linked, &[("image.elf", &exported)])?;
        Ok(linked)
    }

    fn link_uncached(
        &self,
        request: &LinkRequest,
        linker: &Path,
        entry: &str,
        candidates: &[u64],
    ) -> Result<LinkedImage> {
        let mut request = request.clone();
        request
            .companions
            .extend(self.propose(&request, linker, candidates)?);
        let request = &request;
        let runner = &self.runner;
        let plan = self.run.join("link-plan.json");
        let linker = path_arg(&std::path::absolute(linker)?);
        let mut command = args(["link-plan", "--request"]);
        command.extend([
            path_arg(&runner.doc("plan", request)?),
            "--linker".into(),
            linker.clone(),
            "--output".into(),
            path_arg(&plan),
        ]);
        runner.call("plan", &command, 0)?;
        let description: blobray_domain::LinkPlanDescription =
            serde_json::from_slice(&fs::read(&plan)?)?;
        if !description.ready() {
            let blockers: Vec<_> = description
                .blockers
                .iter()
                .map(|b| b.message.as_str())
                .collect();
            return Err(invalid(format!(
                "link plan blocked: {}",
                blockers.join("; ")
            )));
        }
        let mut command = args(["prepare-image", "--plan"]);
        command.extend([path_arg(&plan), "--linker".into(), linker]);
        let image = runner
            .run_record("prepare", &command, 0)?
            .image
            .ok_or_else(|| invalid("prepare-image published no image"))?;
        let view: RecordDocument<serde_json::Value> =
            runner.json("image", &args(["image", "--id", image.as_str()]))?;
        let mut mappings = vec![];
        for record in &view.records {
            if record.kind == "mapping" {
                mappings.push(serde_json::from_value(record.value.clone())?);
            }
        }
        let QuerySummary::Image { manifest, .. } = view.summary else {
            return Err(invalid("image query returned another summary"));
        };
        let mut roots = BTreeMap::new();
        for root in &manifest.roots {
            roots.insert(
                String::from_utf8(root.name.clone())?,
                u32::try_from(root.address)?,
            );
        }
        roots.insert(entry.to_owned(), u32::try_from(manifest.entry)?);
        let mut command = args(["export-image", "--id", image.as_str(), "--output"]);
        command.push(path_arg(&self.run.join("image")));
        runner.call("export-image", &command, 0)?;
        self.images
            .borrow_mut()
            .insert(image.clone(), fs::read(self.run.join("image/image.elf"))?);
        Ok(LinkedImage {
            image,
            manifest: *manifest,
            mappings,
            roots,
        })
    }

    /// The linked vendor image with ROM and production companions, and the
    /// compiled production input with the ROM companion. Stack bytes stay
    /// unknown until a request selects a fill.
    pub fn targets(&self, image: &PreparedImageId) -> Result<(ExecutionTarget, ExecutionTarget)> {
        let stack = seed(STACK_ADDRESS, STACK_BYTES, &[], None)?;
        Ok((
            ExecutionTarget {
                revision: self.revision.clone(),
                source: FunctionSource::Image {
                    image: image.clone(),
                },
                companions: vec![1, 2],
                abi: CallAbi::RiscvInteger,
                stack: stack.clone(),
            },
            ExecutionTarget {
                revision: self.revision.clone(),
                source: FunctionSource::Input { input: 2 },
                companions: vec![1],
                abi: CallAbi::RiscvInteger,
                stack,
            },
        ))
    }

    /// Submit a request, read its evidence, check the verdict and retain it.
    pub fn submit(
        &mut self,
        label: &str,
        request: &ExecutionRequest,
        verdict: Option<blobray_domain::ComparisonVerdict>,
    ) -> Result<&Artifact> {
        self.submit_records(label, request, verdict, true)
    }

    /// `submit`, reading guest event records only when `events` is set.
    pub fn submit_records(
        &mut self,
        label: &str,
        request: &ExecutionRequest,
        verdict: Option<blobray_domain::ComparisonVerdict>,
        events: bool,
    ) -> Result<&Artifact> {
        let started = std::time::Instant::now();
        let result = self
            .verify(request)
            .map_err(|e| invalid(format!("{label}: {e:?}")))?;
        let seconds = started.elapsed().as_secs_f64();
        println!("{label} 0 {seconds:.2}");
        let steps: u64 = result
            .records
            .iter()
            .map(|r| match r {
                ExecutionEvidence::Outcome { steps, .. } => *steps,
                _ => 0,
            })
            .sum();
        let (total, time) = self.executed.get();
        self.executed.set((total + steps, time + seconds));
        assert_eq!(result.verdict, verdict, "{label}");
        let identity = ArtifactId::of_bytes(&serde_json::to_vec(request)?);
        let records: Vec<ExecutionEvidence> = if events {
            result.records
        } else {
            result
                .records
                .into_iter()
                .filter(|r| !matches!(r, ExecutionEvidence::Event { .. }))
                .collect()
        };
        let compared = request
            .cases
            .iter()
            .enumerate()
            .filter(|(_, c)| c.relation.is_some())
            .filter_map(|(index, c)| {
                let production = c.replacement.as_ref()?.entry;
                let verdict = records.iter().find_map(|r| match r {
                    ExecutionEvidence::Comparison { case, result } if *case == index as u32 => {
                        Some(result.verdict)
                    }
                    _ => None,
                });
                Some(ComparedCase {
                    vendor: c.vendor.entry,
                    production,
                    verdict,
                })
            })
            .collect();
        self.artifacts.push(Artifact {
            label: label.into(),
            identity,
            request: request.clone(),
            records,
            verdict: result.verdict,
            complete: result.complete,
            compared,
        });
        Ok(self.artifacts.last().unwrap())
    }

    /// The evidence entry of one claim: `symbol` of vendor `source` at
    /// `vendor` compared with production `entry` at `production`. Only
    /// executions whose every case of that pair is MATCH count; a claim
    /// without one such execution fails.
    fn claim(
        &self,
        suite: &str,
        claimed: Claimed<'_>,
        decisions: &[crate::coverage::Decision],
        observed: &mut crate::coverage::Observed,
    ) -> Result<evidence_index::Entry> {
        let Claimed {
            source,
            symbol,
            vendor,
            entry,
            production,
        } = claimed;
        let mut cases = 0;
        let mut executions = vec![];
        let mut reviews = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            let selected: Vec<_> = artifact
                .compared
                .iter()
                .filter(|c| c.vendor == vendor && c.production == production)
                .collect();
            if selected.is_empty()
                || selected
                    .iter()
                    .any(|c| c.verdict != Some(blobray_domain::ComparisonVerdict::Match))
            {
                continue;
            }
            cases += selected.len() as u64;
            executions.push(artifact.identity.as_str().to_owned());
            // Contracts and projections the pair's comparisons selected.
            for case in &artifact.request.cases {
                let Some(relation) = &case.relation else {
                    continue;
                };
                if case.vendor.entry != vendor
                    || case.replacement.as_ref().map(|r| r.entry) != Some(production)
                {
                    continue;
                }
                if let Some(EffectContractRef::Content { contract }) = &relation.effects {
                    reviews.insert(contract.as_str().to_owned());
                }
                if let Some(ProjectionRef::Content { projection }) = &relation.projection {
                    reviews.insert(projection.as_str().to_owned());
                }
            }
        }
        if executions.is_empty() {
            return Err(invalid(format!(
                "{suite}: no MATCH execution compares {symbol} with {entry}"
            )));
        }
        let selected: Vec<&Artifact> = self
            .artifacts
            .iter()
            .filter(|a| executions.contains(&a.identity.as_str().to_owned()))
            .collect();
        let pairs: Vec<(ArtifactId, &ExecutionRequest, &[ExecutionEvidence])> = selected
            .iter()
            .map(|a| (a.identity.clone(), &a.request, a.records.as_slice()))
            .collect();
        let executables = self.executables(&selected[0].request.vendor)?;
        let executables: Vec<&[u8]> = executables.iter().map(Vec::as_slice).collect();
        let memory =
            blobray_domain::WorkingMemory::new(self.runner.budget.working_memory_mib << 20)?;
        let report = blobray_application::in_process::coverage(
            &pairs,
            &executables,
            &blobray_backend_riscv::RiscvDecoder,
            &memory,
            &mut crate::harness::InProcessControl::new(&self.runner.budget),
        )
        .map_err(|e| invalid(format!("{suite} {symbol} coverage: {e:?}")))?;
        let root = report
            .roots
            .iter()
            .find(|r| r.entry == vendor)
            .ok_or_else(|| invalid(format!("{suite}: {symbol} is not a coverage root")))?;
        // Uncovered locations by absolute address and kind: a block shared by
        // several closure functions is named once, by the lowest entry.
        let mut located = BTreeMap::new();
        for function in report
            .functions
            .iter()
            .filter(|f| root.functions.contains(&f.entry))
        {
            let name = function
                .name
                .clone()
                .unwrap_or_else(|| format!("{:#x}", function.entry));
            observed.functions.insert(name.clone());
            let mut add = |address: u32, kind: LocationKind| {
                located.entry((address, kind)).or_insert_with(|| {
                    crate::coverage::location(&name, function.entry, address, kind)
                });
            };
            for block in &function.uncovered_blocks {
                add(*block, LocationKind::Block);
            }
            for direction in &function.uncovered_directions {
                add(
                    direction.site,
                    if direction.taken {
                        LocationKind::Taken
                    } else {
                        LocationKind::Fallthrough
                    },
                );
            }
        }
        let locations: std::collections::BTreeSet<_> = located.into_values().collect();
        let (excluded, untriaged) = crate::coverage::Observed::classify(decisions, &locations);
        let count = |c: &blobray_domain::CoverageCount| evidence_index::Count {
            reached: c.reached,
            total: c.total,
        };
        let coverage = evidence_index::Coverage {
            blocks: count(&root.blocks),
            directions: count(&root.directions),
            excluded: excluded.len() as u64,
            untriaged: untriaged.len() as u64,
        };
        observed.uncovered.extend(locations);
        Ok(evidence_index::Entry {
            suite: suite.into(),
            source: source.into(),
            symbol: symbol.into(),
            production: entry.into(),
            verdict: evidence_index::MATCH.into(),
            cases,
            reviews: reviews.into_iter().collect(),
            coverage,
        })
    }

    /// Evidence entries of `list` (vendor source, root symbol, production
    /// probe): archive roots resolve in `roots`, ROM roots in the ROM input.
    /// Coverage of every claimed root closure is classified under
    /// `decisions`, each of which must still exclude something.
    pub fn claims(
        &self,
        suite: &str,
        roots: &BTreeMap<String, u32>,
        list: &[(&str, &str, &str)],
        decisions: &[crate::coverage::Decision],
    ) -> Result<Claims> {
        let mut observed = crate::coverage::Observed::default();
        let entries = list
            .iter()
            .map(|(source, symbol, production)| {
                let vendor = match *source {
                    "archive" => *roots
                        .get(*symbol)
                        .ok_or_else(|| invalid(format!("{symbol} is not a linked root")))?,
                    "rom" => u32::try_from(
                        crate::harness::symbol(&self.inventory, ROM_INPUT as usize, symbol)?.value,
                    )?,
                    other => return Err(invalid(format!("unknown vendor source {other}"))),
                };
                let address = self.probes.entry(production)?;
                self.claim(
                    suite,
                    Claimed {
                        source,
                        symbol,
                        vendor,
                        entry: production,
                        production: address,
                    },
                    decisions,
                    &mut observed,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        observed.check(suite, decisions)?;
        let (steps, seconds) = self.executed.get();
        println!(
            "{suite} interpreter {steps} guest instructions in {seconds:.2} s ({:.1} M/s)",
            steps as f64 / seconds.max(f64::EPSILON) / 1e6
        );
        let (_, untriaged) = crate::coverage::Observed::classify(decisions, &observed.uncovered);
        let reach = self
            .artifacts
            .iter()
            .flat_map(|artifact| &artifact.records)
            .filter_map(|record| match record {
                blobray_domain::ExecutionEvidence::Coverage {
                    replacement: true,
                    coverage,
                } => Some(coverage.instructions.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect();
        Ok(Claims {
            entries,
            untriaged,
            reach,
        })
    }

    /// Run a request that must fail for capacity.
    pub fn capacity_failure(&self, label: &str, request: &ExecutionRequest) -> Result<()> {
        let failed = self
            .verify(request)
            .err()
            .ok_or_else(|| invalid(format!("{label}: capacity was not exhausted")))?;
        assert_eq!(
            failed.code,
            ErrorCode::ResourceLimited,
            "{label}: {failed:?}"
        );
        Ok(())
    }
}

/// The named ROM storage constants are the pinned ROM's own symbols.
pub fn verify_rom_symbols(inventory: &Revision, rom: usize) -> Result<()> {
    for (name, address, size) in ROM_SYMBOLS {
        let symbol = crate::harness::symbol(inventory, rom, name)?;
        if symbol.value != u64::from(address) || symbol.size != size {
            return Err(invalid(format!(
                "ROM symbol {name} differs from its named address"
            )));
        }
    }
    Ok(())
}

/// Create a fresh `run-*` directory below `output` and record it as `latest`.
pub fn start_run(output: &Path) -> Result<PathBuf> {
    fs::create_dir_all(output)?;
    let run = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(std::path::absolute(output)?)?
        .keep();
    fs::write(output.join("latest"), run.as_os_str().as_encoded_bytes())?;
    Ok(run)
}

/// Resolve one uniquely named defined symbol in an exported image and retain
/// the complete defined-symbol listing next to the evidence.
pub fn image_symbol(elf: &Path, listing: &Path, name: &str) -> Result<(u32, u64)> {
    let bytes = fs::read(elf)?;
    let file = object::File::parse(&*bytes)?;
    let mut lines = String::new();
    let mut matches = vec![];
    for symbol in file.symbols().filter(|s| !s.is_undefined()) {
        let Ok(symbol_name) = symbol.name() else {
            continue;
        };
        if symbol_name.is_empty() {
            continue;
        }
        lines.push_str(&format!(
            "{symbol_name} {:x} {:x}\n",
            symbol.address(),
            symbol.size()
        ));
        if symbol_name == name {
            matches.push((u32::try_from(symbol.address())?, symbol.size()));
        }
    }
    fs::write(listing, lines)?;
    match matches[..] {
        [one] => Ok(one),
        _ => Err(invalid(format!(
            "{name}: {} image definitions",
            matches.len()
        ))),
    }
}

/// Static-table identity of the unique defined `name` in an exported image
/// ELF whose payload is `object`.
pub fn image_symbol_id(elf: &Path, object: &ObjectId, name: &str) -> Result<SymbolId> {
    let bytes = fs::read(elf)?;
    let file = object::File::parse(&*bytes)?;
    let table = file
        .sections()
        .find(|s| s.name() == Ok(".symtab"))
        .ok_or_else(|| invalid("image has no static symbol table"))?;
    let matches: Vec<_> = file
        .symbols()
        .filter(|s| !s.is_undefined() && s.name() == Ok(name))
        .collect();
    match matches[..] {
        [ref one] => Ok(SymbolId {
            object: object.clone(),
            table: SymbolTableKind::Static,
            table_section: u32::try_from(table.index().0)?,
            index: one.index().0 as u64,
        }),
        _ => Err(invalid(format!(
            "{name}: {} image definitions",
            matches.len()
        ))),
    }
}

/// Request over explicit targets; `fill` initializes both stacks when given.
pub fn request(
    vendor: &ExecutionTarget,
    replacement: Option<&ExecutionTarget>,
    fill: Option<u8>,
    cases: Vec<blobray_domain::ExecutionCase>,
    max_events: u32,
) -> ExecutionRequest {
    let with_fill = |target: &ExecutionTarget| {
        let mut target = target.clone();
        if fill.is_some() {
            target.stack.fill = fill;
        }
        target
    };
    ExecutionRequest {
        schema: blobray_domain::EXECUTION_SCHEMA,
        vendor: with_fill(vendor),
        binding: replacement.map(|_| blobray_domain::CompiledBinding::SharedCore),
        replacement: replacement.map(with_fill),
        cases,
        max_events,
    }
}
