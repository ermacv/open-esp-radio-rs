//! Shared scenario lifecycle: run directory, linked image, execution
//! submission, failure without publication and source-free preservation.
use crate::harness::{
    Budget, ExecutionDocument, Input, ProbeCatalog, Result, Runner, args, invalid, manifest, seed,
};
use crate::layout::*;
use blobray_application::QuerySummary;
use blobray_domain::{
    ArtifactId, CallAbi, ErrorCode, ExecutionRequest, ExecutionTarget, FunctionSource,
    ImageManifest, ImageMapping, LinkRequest, PreparedImageId, Revision, RevisionId,
};
use blobray_next_host::wire::RecordDocument;
use object::{Object, ObjectSymbol};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

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
        let probes = ProbeCatalog::capture(&runner, &revision, &inventory, 2)?;
        Ok(Self {
            runner,
            run,
            revision,
            inventory,
            probes,
            artifacts: vec![],
        })
    }

    /// Plan, prepare, inspect and export one linked image. `entry` names the
    /// request's entry so it resolves like the other roots.
    pub fn link(&self, request: &LinkRequest, linker: &Path, entry: &str) -> Result<LinkedImage> {
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
        let document = self
            .runner
            .execution(&format!("{label}-evidence"), &identity)?;
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
    /// reopens and replays exactly from a restored backup.
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
        let reopened = self.runner.execution("moved-evidence", &check.identity)?;
        assert_eq!(reopened.records, check.document.records);
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
                .execution(&format!("restored-{}", artifact.label), &artifact.identity)?;
            assert_eq!(restored.records, artifact.document.records);
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
