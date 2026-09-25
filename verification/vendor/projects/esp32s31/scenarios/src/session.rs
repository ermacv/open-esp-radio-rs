//! Shared scenario lifecycle: run directory, linked image, execution
//! submission, failure without publication and source-free preservation.
use crate::harness::{
    Budget, ExecutionDocument, Input, ProbeCatalog, Result, Runner, args, invalid, manifest, seed,
};
use crate::layout::*;
use blobray_application::QuerySummary;
use blobray_domain::{
    ArtifactId, AssertionId, CallAbi, CompanionProposal, EffectContract, EffectProposalRequest,
    EffectReview, EntrySelection, ErrorCode, ExecutionRequest, ExecutionTarget, FunctionSource,
    ImageManifest, ImageMapping, KnowledgeRevisionId, LayoutProjection, LinkRequest, ObjectId,
    PreparedImageId, ProjectionProposalRequest, ProjectionReview, Revision, RevisionId, SymbolId,
    SymbolTableKind,
};
use blobray_next_host::wire::RecordDocument;
use object::{Object, ObjectSection, ObjectSymbol};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

/// Actor recorded on the knowledge reviews this package submits.
const REVIEW_ACTOR: &str = "oer-esp32s31-vendor-scenarios";

/// One retained execution and the evidence read immediately after it.
pub struct Artifact {
    pub label: String,
    pub identity: ArtifactId,
    pub document: ExecutionDocument,
}

/// Authenticated inputs, their captured revision and the probe catalog.
pub struct Session {
    pub runner: Runner,
    pub run: PathBuf,
    pub revision: RevisionId,
    pub inventory: Revision,
    pub probes: ProbeCatalog,
    pub artifacts: Vec<Artifact>,
    /// Head of the project's knowledge after this session's reviews.
    pub knowledge: Option<KnowledgeRevisionId>,
}

/// A prepared image and its resolved roots, including the entry.
pub struct LinkedImage {
    pub image: PreparedImageId,
    pub manifest: ImageManifest,
    pub mappings: Vec<ImageMapping>,
    pub roots: BTreeMap<String, u32>,
}

pub fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

impl Session {
    /// Create a fresh `run-*` directory, capture inputs and read the probe
    /// catalog of input 2 (compiled production).
    pub fn start(
        binary: &Path,
        output: &Path,
        budget: Budget,
        inputs: &[Input<'_>],
        scope: &str,
    ) -> Result<Self> {
        let run = start_run(output)?;
        let runner = Runner::new(binary, &run, run.join("project"), budget)?;
        let (revision, identities) = runner.capture(inputs)?;
        let roles: Vec<_> = inputs.iter().map(|i| i.role).collect();
        runner.doc(
            "identities",
            &serde_json::json!({"sha256": identities, "roles": roles, "scope": scope}),
        )?;
        let inventory = runner.inventory()?;
        if let Some(rom) = inputs.iter().position(|i| i.role == "rom") {
            verify_rom_symbols(&inventory, rom)?;
        }
        let probes = ProbeCatalog::capture(&runner, &revision, &inventory, 2)?;
        Ok(Self {
            runner,
            run,
            revision,
            inventory,
            probes,
            artifacts: vec![],
            knowledge: None,
        })
    }

    /// Propose `contract` as `subject`, then accept exactly that proposal as
    /// the scenario's review. Returns the accepted review a relation selects.
    pub fn review_effects(
        &mut self,
        name: &str,
        subject: &str,
        contract: EffectContract,
        reason: &str,
    ) -> Result<EffectReview> {
        let request = EffectProposalRequest {
            subject: subject.to_owned().try_into()?,
            contract,
            expected_base: self.knowledge.clone(),
            actor: REVIEW_ACTOR.into(),
            reason: reason.into(),
        };
        let (knowledge, assertion) =
            self.review(name, "propose-effect-contract", &request, reason)?;
        Ok(EffectReview {
            knowledge,
            assertion,
        })
    }

    /// Propose `projection` as `subject` and accept exactly that proposal.
    pub fn review_projection(
        &mut self,
        name: &str,
        subject: &str,
        projection: LayoutProjection,
        reason: &str,
    ) -> Result<ProjectionReview> {
        let request = ProjectionProposalRequest {
            subject: subject.to_owned().try_into()?,
            projection,
            expected_base: self.knowledge.clone(),
            actor: REVIEW_ACTOR.into(),
            reason: reason.into(),
        };
        let (knowledge, assertion) = self.review(name, "propose-projection", &request, reason)?;
        Ok(ProjectionReview {
            knowledge,
            assertion,
        })
    }

    /// Submit `request` through the knowledge `propose` command, then accept
    /// the one assertion that proposal published. Returns the accepted
    /// knowledge revision and the assertion.
    fn review<T: serde::Serialize>(
        &mut self,
        name: &str,
        propose: &str,
        request: &T,
        reason: &str,
    ) -> Result<(KnowledgeRevisionId, AssertionId)> {
        let mut command = args(["knowledge", propose, "--request"]);
        command.push(path_arg(
            &self.runner.doc(&format!("{name}-proposal"), request)?,
        ));
        let proposed = self
            .runner
            .run_record(&format!("{name}-proposal"), &command, 0)?
            .knowledge
            .ok_or_else(|| invalid("proposal published no knowledge"))?;
        let shown: serde_json::Value = self.runner.json(
            &format!("{name}-proposed"),
            &args(["knowledge", "show", "--revision", proposed.as_str()]),
        )?;
        let assertion: AssertionId = shown["records"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["value"]["proposed_in"].as_str() == Some(proposed.as_str()))
            .and_then(|r| r["value"]["id"].as_str())
            .ok_or_else(|| invalid("proposed assertion missing"))?
            .parse()?;
        let accepted = self
            .runner
            .run_record(
                &format!("{name}-review"),
                &args([
                    "knowledge",
                    "accept",
                    "--assertion",
                    assertion.as_str(),
                    "--base",
                    proposed.as_str(),
                    "--actor",
                    REVIEW_ACTOR,
                    "--reason",
                    reason,
                ]),
                0,
            )?
            .knowledge
            .ok_or_else(|| invalid("review published no knowledge"))?;
        self.knowledge = Some(accepted.clone());
        Ok((accepted, assertion))
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
        let command = if request.replacement.is_some() {
            "compare"
        } else {
            "execute"
        };
        let mut invocation = args([command, "--request"]);
        invocation.push(path_arg(&self.runner.doc(label, request)?));
        let identity = self
            .runner
            .run_record(label, &invocation, 0)?
            .execution
            .ok_or_else(|| invalid(format!("{label}: no execution published")))?;
        let name = format!("{label}-evidence");
        let document = if events {
            self.runner.execution(&name, &identity)?
        } else {
            self.runner.execution_without_events(&name, &identity)?
        };
        assert_eq!(manifest(&document).verdict, verdict, "{label}");
        self.artifacts.push(Artifact {
            label: label.into(),
            identity,
            document,
        });
        Ok(self.artifacts.last().unwrap())
    }

    /// Run a request that must fail for capacity and publish nothing.
    pub fn capacity_failure(&self, label: &str, request: &ExecutionRequest) -> Result<()> {
        let command = if request.replacement.is_some() {
            "compare"
        } else {
            "execute"
        };
        let mut invocation = args([command, "--request"]);
        invocation.push(path_arg(&self.runner.doc(label, request)?));
        let failed = self.runner.run_record(label, &invocation, 1)?;
        assert!(
            failed.execution.is_none()
                && failed.publication.is_none()
                && failed.resolved_operation.is_none()
        );
        assert_eq!(
            failed.error.map(|e| e.code),
            Some(ErrorCode::ResourceLimited)
        );
        Ok(())
    }

    /// A retained execution is unchanged after a later failure.
    pub fn assert_retained(
        &self,
        label: &str,
        identity: &ArtifactId,
        before: &ExecutionDocument,
    ) -> Result<()> {
        let after = self.runner.execution(label, identity)?;
        assert_eq!(after.records, before.records);
        assert_eq!(manifest(&after), manifest(before));
        Ok(())
    }

    /// Source-free preservation. After backup the project moves; the selected
    /// execution reopens and replays there. Every retained execution then
    /// reopens and replays exactly from a restored backup. Reopening compares
    /// manifests, whose record digest Blobray verifies against the payload.
    pub fn preserve(&mut self, moved_check: usize) -> Result<()> {
        let backup = self.run.join("backup.blobray");
        let mut command = args(["backup", "--output"]);
        command.push(path_arg(&backup));
        self.runner.call("backup", &command, 0)?;
        let moved = self.run.join("moved");
        fs::rename(&self.runner.project, &moved)?;
        self.runner.project = moved;
        let check = self
            .artifacts
            .get(moved_check)
            .ok_or_else(|| invalid("no retained executions"))?;
        // Manifests carry the record payload digest, which reopening verifies.
        let reopened = self
            .runner
            .execution_summary("moved-evidence", &check.identity)?;
        assert_eq!(manifest(&reopened), manifest(&check.document));
        let replay = self.runner.run_record(
            "moved-replay",
            &args(["replay", "--id", check.identity.as_str()]),
            0,
        )?;
        assert_eq!(replay.execution.as_ref(), Some(&check.identity));
        self.runner.project = self.run.join("restored");
        let mut command = args(["restore", "--backup"]);
        command.push(path_arg(&backup));
        self.runner.call("restore", &command, 0)?;
        for artifact in &self.artifacts {
            let restored = self
                .runner
                .execution_summary(&format!("restored-{}", artifact.label), &artifact.identity)?;
            assert_eq!(manifest(&restored), manifest(&artifact.document));
            let replay = self.runner.run_record(
                &format!("replay-{}", artifact.label),
                &args(["replay", "--id", artifact.identity.as_str()]),
                0,
            )?;
            assert_eq!(replay.execution.as_ref(), Some(&artifact.identity));
        }
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
