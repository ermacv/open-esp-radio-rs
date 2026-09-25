//! A linked captured PHY image with compiled production and the shared
//! stack-entry, parameter-setup and callback-installation phases.
use crate::evidence::outcomes;
use crate::harness::{Budget, Input};
use crate::harness::{
    Result, evidence, invalid, invocation, known, manifest, region, selection, symbol, words,
    words_padded,
};
use crate::layout::*;
use crate::session::{Session, image_symbol, request};
use crate::{I2C_LIBRARY_SHA, ROM_SHA};
use blobray_domain::{
    ArtifactId, ComparisonVerdict, DataSelector, DeviceDeclaration, EntrySelection, ExecutionCase,
    ExecutionEvidence, ExecutionRegion, ExecutionRequest, ExecutionStop, ExecutionTarget,
    ImageLayout, ImageRegion, Invocation, LinkRequest, MemorySelection, ObjectId, ObjectLocation,
    RegionLifetime,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Which implementation, if any, is compared with the captured vendor side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Right {
    /// Vendor characterization; relations and verdict are absent.
    None,
    Production,
    /// Vendor-boundary characterization against a second vendor invocation.
    Vendor,
}

pub struct Executed {
    pub records: Vec<ExecutionEvidence>,
    pub request: ExecutionRequest,
    pub identity: ArtifactId,
}

/// Linked captured PHY image, the production target and their retained runs.
pub struct PhyImage {
    pub session: Session,
    pub roots: BTreeMap<String, u32>,
    pub parameter: u32,
    pub image_object: ObjectId,
    pub vendor: ExecutionTarget,
    pub production: ExecutionTarget,
}

impl std::ops::Deref for PhyImage {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl std::ops::DerefMut for PhyImage {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

/// Private inputs and budget of a scenario over the pinned archive, ROM and
/// compiled production.
pub struct PhyOptions {
    pub binary: PathBuf,
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
}

/// Start a session over the authenticated archive (input 0), ROM (input 1),
/// production (input 2) and `extra` inputs from index 3.
pub fn start_session(options: &PhyOptions, extra: &[Input<'_>], purpose: &str) -> Result<Session> {
    let mut inputs = vec![
        Input {
            role: "phy",
            path: &options.library,
            sha256: Some(I2C_LIBRARY_SHA),
        },
        Input {
            role: "rom",
            path: &options.rom,
            sha256: Some(ROM_SHA),
        },
        Input {
            role: "production",
            path: &options.production,
            sha256: None,
        },
    ];
    inputs.extend_from_slice(extra);
    Session::start(
        &options.binary,
        &options.output,
        options.budget,
        &inputs,
        purpose,
    )
}

/// Requested-delay ABI at `address`: every call returns zero and records its
/// first argument as requested microseconds. The call count is evidence, not
/// a declared budget; scenarios compare the ordered delay values instead.
pub fn delay_calls(id: &str, address: u32) -> Vec<blobray_domain::CallDeclaration> {
    use blobray_domain::{
        CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, CallValue,
    };
    vec![CallDeclaration {
        repetition: CallRepetition::Unbounded,
        id: id.into(),
        applicability: "declared delay ABI; requested microseconds only".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary: CallBoundary::CapturedCode,
            allow_tail: true,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [Some(0), Some(0)],
            outputs: vec![],
            allocation: None,
            delay_micros: Some(CallValue::Argument { word: 0 }),
        }],
    }]
}

/// Code and data placement of linked PHY images.
pub fn image_layout() -> ImageLayout {
    ImageLayout {
        code: ImageRegion {
            start: IMAGE_CODE_START,
            length: IMAGE_REGION_BYTES,
        },
        data: ImageRegion {
            start: IMAGE_DATA_START,
            length: IMAGE_REGION_BYTES,
        },
    }
}

/// Exact captured symbol of one input as a link selection.
pub fn select(session: &Session, input: usize, name: &str) -> Result<EntrySelection> {
    Ok(EntrySelection {
        input: input as u64,
        symbol: symbol(&session.inventory, input, name)?.id.clone(),
    })
}

impl PhyImage {
    /// Link the image, check the captured `phy_param` extent and prepare targets.
    pub fn link(
        session: Session,
        link: &LinkRequest,
        linker: &Path,
        entry: &str,
        candidates: &[u64],
    ) -> Result<Self> {
        let linked = session.link(link, linker, entry, candidates)?;
        let (parameter, size) = image_symbol(
            &session.run.join("image/image.elf"),
            &session.run.join("image-symbols.txt"),
            "phy_param",
        )?;
        if size != u64::from(PHY_PARAM_BYTES) {
            return Err(invalid(
                "phy_param does not have the expected 516-byte extent",
            ));
        }
        let (vendor, production) = session.targets(&linked.image)?;
        Ok(Self {
            image_object: ObjectId {
                artifact: linked.manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            roots: linked.roots,
            parameter,
            vendor,
            production,
            session,
        })
    }

    pub fn sym(&self, input: usize, name: &str) -> u32 {
        let value = symbol(&self.inventory, input, name)
            .unwrap_or_else(|e| panic!("{e}"))
            .value;
        u32::try_from(value).expect("RV32 symbol")
    }

    pub fn root(&self, name: &str) -> u32 {
        *self
            .roots
            .get(name)
            .unwrap_or_else(|| panic!("missing root {name}"))
    }

    /// Enter `target` through the stack-entry adapter with sixteen explicit words.
    pub fn enter(
        &self,
        target: u32,
        arguments: &[u32],
        memory: Vec<ExecutionRegion>,
        observe: Vec<MemorySelection>,
        models: Vec<DeviceDeclaration>,
    ) -> Invocation {
        let entry = self
            .probes
            .entry("open_phy_trace_stack_entry")
            .expect("stack entry probe");
        let mut regions = vec![
            known(
                ABI_WORDS,
                64,
                &words_padded(arguments, 16, 0).expect("sixteen words"),
            )
            .unwrap(),
        ];
        regions.extend(memory);
        invocation(
            entry,
            vec![Some(target), Some(ABI_WORDS)],
            regions,
            models,
            observe,
        )
    }

    /// Enter a prepared probe invocation through the stack-entry adapter.
    pub fn enter_probe(&self, probe: Invocation) -> Invocation {
        let arguments: Vec<u32> = probe
            .arguments
            .iter()
            .map(|a| a.expect("known probe argument"))
            .collect();
        self.enter(
            probe.entry,
            &arguments,
            probe.memory,
            probe.observe_memory,
            probe.models,
        )
    }

    /// A zero-length captured `memcpy`: the production counterpart of a
    /// vendor-only phase such as callback installation.
    pub fn noop(&self) -> Invocation {
        self.enter(
            self.sym(1, "memcpy"),
            &[PARAMETER_COPY, PARAMETER_COPY, 0],
            vec![],
            vec![],
            vec![],
        )
    }

    /// Copy 516 parameter bytes into the captured `phy_param` or a production buffer.
    pub fn setup(&self, data: &[u8], production: bool) -> Invocation {
        let memcpy = self.sym(1, "memcpy");
        if production {
            self.enter(
                memcpy,
                &[PARAMETER_COPY, INPUT, PHY_PARAM_BYTES],
                vec![
                    known(INPUT, PHY_PARAM_BYTES, data).unwrap(),
                    region(
                        PARAMETER_COPY,
                        PHY_PARAM_BYTES,
                        &[],
                        None,
                        RegionLifetime::Session,
                    )
                    .unwrap(),
                ],
                vec![],
                vec![],
            )
        } else {
            self.enter(
                memcpy,
                &[self.parameter, INPUT, PHY_PARAM_BYTES],
                vec![known(INPUT, PHY_PARAM_BYTES, data).unwrap()],
                vec![],
                vec![],
            )
        }
    }

    pub fn execute(
        &mut self,
        label: &str,
        mut rows: Vec<ExecutionCase>,
        fill: u8,
        right: Right,
        verdict: ComparisonVerdict,
        maximum: u32,
    ) -> Result<Executed> {
        if right == Right::None {
            for row in &mut rows {
                row.relation = None;
            }
        }
        let replacement = match right {
            Right::None => None,
            Right::Production => Some(self.production.clone()),
            Right::Vendor => Some(self.vendor.clone()),
        };
        let count = rows.len() * if replacement.is_some() { 2 } else { 1 };
        let request = request(
            &self.vendor,
            replacement.as_ref(),
            Some(fill),
            rows,
            maximum,
        );
        let expected = (right != Right::None).then_some(verdict);
        let artifact = self.session.submit(label, &request, expected)?;
        let summary = manifest(&artifact.document);
        let records = evidence(&artifact.document);
        let stops = outcomes(&records);
        assert_eq!(stops.len(), count, "{label}");
        if matches!(verdict, ComparisonVerdict::Match | ComparisonVerdict::Diff) {
            assert!(summary.complete, "{label}");
            assert!(
                stops
                    .iter()
                    .all(|s| matches!(s, ExecutionStop::Returned { .. })),
                "{label}: {stops:?}"
            );
        }
        let identity = artifact.identity.clone();
        Ok(Executed {
            records,
            request,
            identity,
        })
    }

    /// Run with the default comparison expectation and event capacity.
    pub fn compare(&mut self, label: &str, rows: Vec<ExecutionCase>, fill: u8) -> Result<Executed> {
        self.execute(
            label,
            rows,
            fill,
            Right::Production,
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )
    }

    /// Vendor-only characterization that must complete.
    pub fn characterize_vendor(
        &mut self,
        label: &str,
        rows: Vec<ExecutionCase>,
        fill: u8,
    ) -> Result<Executed> {
        self.execute(
            label,
            rows,
            fill,
            Right::None,
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )
    }

    pub fn last_manifest_complete(&self) -> bool {
        manifest(&self.artifacts.last().expect("retained execution").document).complete
    }

    /// Exact image bytes at a linked address, checked against the source identity.
    pub fn image_data(&self, name: &str, address: u32, length: u64) -> Result<Vec<u8>> {
        let request = blobray_domain::DataRequest {
            occurrence: blobray_domain::KnowledgeOccurrence {
                revision: self.revision.clone(),
                source: self.vendor.source.clone(),
                object: self.image_object.clone(),
                symbol: None,
            },
            ranges: vec![DataSelector::Image {
                address: u64::from(address),
                length,
            }],
            analyses: vec![],
            pointer_table: None,
        };
        self.runner.data(name, &request, &self.run.join(name))
    }

    /// Run the real ROM callback installer with the captured ROM table pointer.
    pub fn install_callbacks(&self, observed_slot: u32) -> Result<Invocation> {
        Ok(self.enter(
            self.root("phy_get_romfunc_addr"),
            &[],
            vec![
                region(
                    ROM_INTERFACE_POINTER,
                    4,
                    &words(&[ROM_CALLBACK_TABLE]),
                    None,
                    RegionLifetime::Session,
                )?,
                region(ROM_PARAMETER_POINTER, 4, &[], None, RegionLifetime::Session)?,
            ],
            vec![selection(observed_slot, 4)],
            vec![],
        ))
    }
}
