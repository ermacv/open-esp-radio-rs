//! Captured Wi-Fi/BT gain arithmetic and complete publication comparison.
//!
//! Explicit finite software inputs only; no RF, protocol or whole-TXCAL
//! qualification. Private inputs and retained requests/evidence belong in the
//! selected ignored output.
use crate::evidence::{Access, Effect, calls, effects, events, has_events, outcomes, output, stop};
use crate::harness::{
    Buffer, ExecutionDocument, Input, LimitMode, ProbeCatalog, Result, Runner, args, case,
    data_request, evidence, filled, invalid, invocation, known, manifest, named_object,
    named_section, region, selection, sha256, symbol, words, words_padded,
};
use crate::{I2C_LIBRARY_SHA, ROM_SHA};
use blobray_domain::{
    ArtifactId, CallAbi, CallCapture, ComparisonVerdict, CompiledBinding, DataSelector,
    DeviceBehavior, DeviceDeclaration, EXECUTION_SCHEMA, EntrySelection, ErrorCode, ExecutionCase,
    ExecutionEvent, ExecutionEvidence, ExecutionGap, ExecutionRegion, ExecutionRequest,
    ExecutionStop, ExecutionTarget, FunctionSource, ImageLayout, ImageRegion, Invocation,
    LinkRequest, MemorySelection, ObjectId, ObjectLocation, ObservedCallTarget, RegionLifetime,
    RegisterCell, Revision, RevisionId, SessionReset,
};
use blobray_next_host::wire::RecordDocument;
use object::{Object, ObjectSymbol};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

/// Pinned `librftest.a` from the same PHY source revision as the archive.
pub const RFTEST_SHA: &str = "547786cd684eb9cd8902955176e9a9a7f113d8faa3f415e12108ed261f55a11e";
pub const OBJECT_SHA: &str = "88ee26018604100c9ba7839214024d54b48adf982c482dfb1e90d2a11f29f7d3";
/// Independently extracted with llvm-ar/llvm-objcopy from the pinned archive;
/// the native export must match before these data may support a comparison.
pub const COEFFICIENT_SHA: &str =
    "748936005c8ba31c1b826d0d0d2bb56f8982548c7d9826fed90930100d79f7a1";
pub const INPUT: u32 = 0x3fff_0000;
pub const OUTPUT: u32 = 0x3fff_1000;
pub const ABI: u32 = 0x3fff_4000;
pub const CURVES: [[u8; 6]; 4] = [
    [0; 6],
    [0, 5, 10, 15, 20, 25],
    [127, 128, 255, 1, 2, 3],
    [255, 0, 127, 128, 7, 9],
];
const PARAMETER_BYTES: u32 = 516;

pub fn signed8(value: i32) -> i32 {
    (value & 127) - (value & 128)
}

fn halfwords(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Independent interval oracle: choose each interval without cursor state.
pub fn arithmetic(
    coefficients: &[u8],
    curve: &[u8],
    base: i32,
    correction: i32,
    channel: Option<u32>,
) -> Vec<u8> {
    let low = halfwords(&coefficients[0..36]);
    let mid = halfwords(&coefficients[36..72]);
    let high: Vec<i32> = halfwords(&coefficients[72..108])
        .into_iter()
        .map(|v| i32::from(v as i16))
        .collect();
    let (targets, interpolation): (Vec<i32>, i32) = match channel {
        None => (
            (0..16)
                .map(|i| base - correction + (-96 + 12 * i).clamp(-72, 80))
                .collect(),
            signed8(i32::from(curve[1])),
        ),
        Some(channel) => {
            let channel = channel as i32;
            let interpolation = if channel <= 6 {
                let (a, b) = (signed8(i32::from(curve[0])), signed8(i32::from(curve[1])));
                signed8((b - a) * (channel - 1) / 5 + a)
            } else if channel <= 11 {
                let (a, b) = (signed8(i32::from(curve[1])), signed8(i32::from(curve[2])));
                signed8((b - a) * (channel - 6) / 5 + a)
            } else {
                signed8(signed8(i32::from(curve[2])) + 2)
            };
            (
                (0..32).map(|i| base - correction + 84 - 4 * i).collect(),
                interpolation,
            )
        }
    };
    let indexes: Vec<usize> = targets
        .iter()
        .map(|target| high.iter().position(|h| target >= h).unwrap_or(17))
        .collect();
    let mut residuals: Vec<i32> = targets
        .iter()
        .zip(&indexes)
        .map(|(t, i)| t - high[*i] - interpolation)
        .collect();
    if channel.is_none() {
        for v in &mut residuals {
            *v = (*v).clamp(-60, 24);
        }
    }
    let mut result: Vec<u8> = residuals.iter().map(|v| (v & 255) as u8).collect();
    result.extend(indexes.iter().flat_map(|i| mid[*i].to_le_bytes()));
    result.extend(indexes.iter().flat_map(|i| low[*i].to_le_bytes()));
    result
}

pub fn gain_models(base: u32, fill: u8) -> Vec<DeviceDeclaration> {
    vec![
        DeviceDeclaration {
            id: "gain-base".into(),
            applicability: "explicit gain bank base".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::ConstantRead {
                address: 0x2010_0408,
                width: 4,
                value: (base << 24) | 0x005a_a55a,
            },
        },
        DeviceDeclaration {
            id: "gain-ports".into(),
            applicability: "retained index and three passive data ports; no gain algorithm".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: (0x2010_0844..0x2010_0854)
                    .step_by(4)
                    .map(|address| RegisterCell {
                        address,
                        width: 4,
                        value: if address == 0x2010_0844 {
                            u32::from(fill) * 0x0101_0101
                        } else {
                            0
                        },
                    })
                    .collect(),
            },
        },
    ]
}

/// Instruction-derived packed-word oracle, independent of the device model.
pub fn publication(
    seed_words: &[u32],
    config: u32,
    calculated: &[u8],
    count: usize,
    base: u32,
    fill: u8,
    bluetooth: bool,
) -> Vec<Effect> {
    let bb = halfwords(&calculated[count..count * 3]);
    let rf = halfwords(&calculated[count * 3..count * 5]);
    let seed_data = words(seed_words);
    let mut result = vec![(Access::Read, 0x2010_0408, (base << 24) | 0x005a_a55a)];
    let mut control = u32::from(fill) * 0x0101_0101;
    for i in 0..count {
        let index = match bb[i] {
            0 => 0,
            128 => 1,
            256 => 2,
            other => panic!("unexpected baseband gain {other}"),
        };
        let field = |n: usize| {
            u64::from(u16::from_le_bytes([
                seed_data[index * 8 + n * 2],
                seed_data[index * 8 + n * 2 + 1],
            ]))
        };
        let (a, b, c, d) = (field(0), field(1), field(2), field(3));
        let (bb, rf) = (u64::from(bb[i]), u64::from(rf[i]));
        let word0 = ((c << 22) | (b << 31) | (d << 13) | u64::from(config & 8191)) as u32;
        let word1 = ((a << 8)
            | (b >> 1)
            | (((bb >> 6) & 255) << 17)
            | ((rf & 7) << 31)
            | ((bb & 63) << 20)
            | 0x1000_0000) as u32;
        let word2 = (((rf >> 1) & 31) | (u64::from(calculated[i]) << 15) | 0x7f80) as u32;
        let slot = (base + if bluetooth { 32 } else { 0 } + i as u32) & 255;
        // The captured ROM publisher writes data first, then updates its index.
        result.extend([
            (Access::Write, 0x2010_0848, word0),
            (Access::Write, 0x2010_084c, word1),
            (Access::Write, 0x2010_0850, word2),
            (Access::Read, 0x2010_0844, control),
        ]);
        control = (control & 0xfff0_0000) | 0x80000 | (slot << 11);
        result.push((Access::Write, 0x2010_0844, control));
    }
    result
}

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

pub struct Artifact {
    pub label: String,
    pub identity: ArtifactId,
    pub document: ExecutionDocument,
}

pub struct Options {
    pub binary: PathBuf,
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub limit_mode: LimitMode,
    pub rftest: Option<PathBuf>,
}

/// Linked captured gain image, compiled production and retained evidence.
pub struct Gain {
    pub runner: Runner,
    pub run: PathBuf,
    pub revision: RevisionId,
    pub inventory: Revision,
    pub probes: ProbeCatalog,
    pub coefficients: Vec<u8>,
    pub roots: BTreeMap<String, u32>,
    pub parameter: u32,
    pub image_object: ObjectId,
    pub vendor: ExecutionTarget,
    pub production: ExecutionTarget,
    pub artifacts: Vec<Artifact>,
    wifi_request: Option<ExecutionRequest>,
    bluetooth_request: Option<ExecutionRequest>,
    publication_request: Option<ExecutionRequest>,
    coefficient_address: Option<u32>,
}

fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

impl Gain {
    pub fn new(options: &Options) -> Result<Self> {
        fs::create_dir_all(&options.output)?;
        let run = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(std::path::absolute(&options.output)?)?
            .keep();
        fs::write(
            options.output.join("latest"),
            run.as_os_str().as_encoded_bytes(),
        )?;
        let runner = Runner::new(
            &options.binary,
            &run,
            run.join("project"),
            options.limit_mode,
        )?;
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
        if let Some(rftest) = &options.rftest {
            inputs.push(Input {
                role: "rftest",
                path: rftest,
                sha256: Some(RFTEST_SHA),
            });
        }
        let (revision, identities) = runner.capture(&inputs)?;
        runner.doc(
            "identities",
            &serde_json::json!({
                "sha256": identities,
                "scope": "captured Wi-Fi/BT gain, calibration storage and RF-test policy; no RF qualification",
                "rftest": options.rftest.is_some(),
            }),
        )?;
        let inventory = runner.inventory()?;
        let probes = ProbeCatalog::capture(&runner, &revision, &inventory, 2)?;
        let object = named_object(&inventory, 0, "phy_tx_gain.o")?;
        let section = named_section(object, ".rodata")?;
        let request = data_request(
            &revision,
            FunctionSource::Input { input: 0 },
            object,
            DataSelector::Section {
                section: section.index,
                offset: 0,
                length: 216,
            },
        );
        let coefficients = runner.data("coefficients", &request, &run.join("coefficients"))?;
        if sha256(&fs::read(run.join("coefficients/object.elf"))?) != OBJECT_SHA
            || sha256(&coefficients) != COEFFICIENT_SHA
        {
            return Err(invalid("gain object or coefficient identity mismatch"));
        }
        let mut roots: Vec<&str> = vec![
            "phy_wifi_get_tx_tab_new",
            "phy_bt_get_tx_tab_new",
            "phy_set_tx_gain_mem_new",
            "phy_bt_set_tx_gain_new",
            "phy_get_romfunc_addr",
        ];
        let mut companions: Vec<&str> = vec![
            "memcpy",
            "phy_wifi_get_tx_gain",
            "phy_bt_get_tx_gain",
            "phy_txbbgain_to_index",
            "phy_write_gain_mem",
            "phy_param_addr",
            "phy_get_romfuncs",
            "phy_get_data_sat",
        ];
        // Co-located archive sections retain physical references outside the
        // selected gain roots. Bind these names to captured ROM definitions;
        // the run never grants their absent hardware inputs or executes a model.
        companions.extend([
            "phy_i2c_writeReg",
            "memset",
            "phy_get_i2c_mst0_mask",
            "phy_i2c_paral_write_num",
            "ets_delay_us",
            "phy_wait_i2c_sdm_stable",
            "phy_tsens_dac_cal",
            "phy_tsens_temp_read_local",
            "phy_i2c_readReg",
            "phy_i2c_writeReg_Mask",
        ]);
        // Calibration storage roots need no RF-test input and always participate.
        roots.extend([
            "phy_rf_cal_data_backup_new",
            "phy_rf_cal_data_recovery_new",
            "register_chipv7_phy_init_param",
            "phy_wifi_set_tx_gain_new",
        ]);
        companions.extend(["phy_get_target_pwr", "phy_byte_to_word"]);
        let root = |input: usize, name: &str| -> Result<EntrySelection> {
            Ok(EntrySelection {
                input: input as u64,
                symbol: symbol(&inventory, input, name)?.id.clone(),
            })
        };
        let mut link = LinkRequest {
            companions: companions
                .iter()
                .map(|n| root(1, n))
                .collect::<Result<_>>()?,
            revision: Some(revision.clone()),
            inputs: vec![0],
            entry: root(0, roots[0])?,
            roots: roots[1..]
                .iter()
                .map(|n| root(0, n))
                .collect::<Result<_>>()?,
            layout: ImageLayout {
                code: ImageRegion {
                    start: 0x1100_0000,
                    length: 0x100_0000,
                },
                data: ImageRegion {
                    start: 0x2000_0000,
                    length: 0x100_0000,
                },
            },
        };
        if options.rftest.is_some() {
            link.inputs.push(3);
            for name in ["set_rate_power_index", "mac_power_set"] {
                link.roots.push(root(3, name)?);
            }
        }
        let plan = run.join("link-plan.json");
        let linker = path_arg(&std::path::absolute(&options.linker)?);
        let mut command = args(["link-plan", "--request"]);
        command.extend([
            path_arg(&runner.doc("plan", &link)?),
            "--linker".into(),
            linker.clone(),
            "--output".into(),
            path_arg(&plan),
        ]);
        runner.call("plan", &command, 0)?;
        let mut command = args(["prepare-image", "--plan"]);
        command.extend([path_arg(&plan), "--linker".into(), linker]);
        let image = runner
            .run_record("prepare", &command, 0)?
            .image
            .ok_or_else(|| invalid("prepare-image published no image"))?;
        let view: RecordDocument<serde_json::Value> =
            runner.json("image", &args(["image", "--id", image.as_str()]))?;
        let blobray_application::QuerySummary::Image {
            manifest: image_manifest,
            ..
        } = view.summary
        else {
            return Err(invalid("image query returned another summary"));
        };
        let mut root_addresses = BTreeMap::new();
        for r in &image_manifest.roots {
            root_addresses.insert(
                String::from_utf8(r.name.clone())?,
                u32::try_from(r.address)?,
            );
        }
        root_addresses.insert(roots[0].to_owned(), u32::try_from(image_manifest.entry)?);
        let mut command = args(["export-image", "--id", image.as_str(), "--output"]);
        command.push(path_arg(&run.join("image")));
        runner.call("export-image", &command, 0)?;
        let parameter = image_symbol(
            &run.join("image/image.elf"),
            &run.join("image-symbols.txt"),
            "phy_param",
        )?;
        if parameter.1 != u64::from(PARAMETER_BYTES) {
            return Err(invalid(
                "phy_param does not have the expected 516-byte extent",
            ));
        }
        let stack = crate::harness::seed(0x3ffe_0000, 0x8000, &[], None)?;
        Ok(Self {
            vendor: ExecutionTarget {
                revision: revision.clone(),
                source: FunctionSource::Image {
                    image: image.clone(),
                },
                companions: vec![1, 2],
                abi: CallAbi::RiscvInteger,
                stack: stack.clone(),
            },
            production: ExecutionTarget {
                revision: revision.clone(),
                source: FunctionSource::Input { input: 2 },
                companions: vec![1],
                abi: CallAbi::RiscvInteger,
                stack,
            },
            image_object: ObjectId {
                artifact: image_manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            runner,
            run,
            revision,
            inventory,
            probes,
            coefficients,
            roots: root_addresses,
            parameter: parameter.0,
            artifacts: vec![],
            wifi_request: None,
            bluetooth_request: None,
            publication_request: None,
            coefficient_address: None,
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
                ABI,
                64,
                &words_padded(arguments, 16, 0).expect("sixteen words"),
            )
            .unwrap(),
        ];
        regions.extend(memory);
        invocation(
            entry,
            vec![Some(target), Some(ABI)],
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

    /// Copy 516 parameter bytes into the captured `phy_param` or a production buffer.
    pub fn setup(&self, data: &[u8], production: bool) -> Invocation {
        let memcpy = self.sym(1, "memcpy");
        if production {
            self.enter(
                memcpy,
                &[INPUT + 0x800, INPUT, PARAMETER_BYTES],
                vec![
                    known(INPUT, PARAMETER_BYTES, data).unwrap(),
                    region(
                        INPUT + 0x800,
                        PARAMETER_BYTES,
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
                &[self.parameter, INPUT, PARAMETER_BYTES],
                vec![known(INPUT, PARAMETER_BYTES, data).unwrap()],
                vec![],
                vec![],
            )
        }
    }

    pub fn wifi_phase(&self, channel: u32, fill: u8, capture: bool) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_wifi_get_tx_tab_new"),
            &[channel, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
            vec![filled(OUTPUT, 160, fill).unwrap()],
            vec![selection(OUTPUT, 160)],
            vec![],
        );
        if capture {
            phase.observe_calls = Some(CallCapture {
                include_tail: true,
                argument_words: 0,
                overrides: vec![
                    blobray_domain::CallWordCount {
                        target: self.sym(1, "memcpy"),
                        words: 3,
                    },
                    blobray_domain::CallWordCount {
                        target: self.sym(1, "phy_wifi_get_tx_gain"),
                        words: 11,
                    },
                ],
            });
        }
        phase
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
        let mut left = self.vendor.clone();
        left.stack.fill = Some(fill);
        let replacement = match right {
            Right::None => None,
            Right::Production => Some(self.production.clone()),
            Right::Vendor => Some(self.vendor.clone()),
        }
        .map(|mut target| {
            target.stack.fill = Some(fill);
            target
        });
        let count = rows.len() * if replacement.is_some() { 2 } else { 1 };
        let request = ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: left,
            binding: replacement.as_ref().map(|_| CompiledBinding::SharedCore),
            replacement,
            cases: rows,
            max_events: maximum,
        };
        let command = if request.replacement.is_some() {
            "compare"
        } else {
            "execute"
        };
        let mut invocation = args([command, "--request"]);
        invocation.push(path_arg(&self.runner.doc(label, &request)?));
        let identity = self
            .runner
            .run_record(label, &invocation, 0)?
            .execution
            .ok_or_else(|| invalid(format!("{label}: no execution published")))?;
        let document = self
            .runner
            .execution(&format!("{label}-evidence"), &identity)?;
        let summary = manifest(&document);
        let expected = (right != Right::None).then_some(verdict);
        assert_eq!(summary.verdict, expected, "{label}");
        let records = evidence(&document);
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
        self.artifacts.push(Artifact {
            label: label.into(),
            identity: identity.clone(),
            document,
        });
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
            32768,
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
            32768,
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

    pub fn coefficient_boundaries(&mut self) -> Result<()> {
        // The legacy input matrix leaves some table intervals unselected in the
        // production path. Select every authenticated threshold exactly, without
        // reading private production constants or supplying vendor results.
        for bluetooth in [false, true] {
            let coefficients = if bluetooth {
                self.coefficients[..108].to_vec()
            } else {
                self.coefficients[108..].to_vec()
            };
            let high: Vec<i32> = halfwords(&coefficients[72..])
                .into_iter()
                .map(|v| i32::from(v as i16))
                .collect();
            let length = if bluetooth { 80 } else { 160 };
            for fill in [0x5a, 0xa5] {
                for batch in (0..18).step_by(6) {
                    let (mut rows, mut expected) = (vec![], vec![]);
                    for (index, threshold) in high.iter().enumerate().skip(batch).take(6) {
                        let mut data = vec![0u8; 516];
                        let (left, right, correction);
                        if bluetooth {
                            let base = 64;
                            correction = -8 - threshold;
                            data[292] = base as u8;
                            data[254] = (correction & 255) as u8;
                            left = self.enter(
                                self.root("phy_bt_get_tx_tab_new"),
                                &[OUTPUT + 48, OUTPUT + 16, OUTPUT, 0],
                                vec![filled(OUTPUT, 80, fill)?],
                                vec![selection(OUTPUT, 80)],
                                vec![],
                            );
                            let mut packed = vec![0u32; 7];
                            packed.extend([((correction & 255) as u32) << 24, base as u32]);
                            right = self.probes.invoke(
                                "open_phy_bluetooth_trace_calculate_gain",
                                vec![
                                    ("input", Buffer::new(INPUT, words(&packed)).into()),
                                    ("output", Buffer::filled(OUTPUT, fill).into()),
                                ],
                                vec![],
                                vec![selection(OUTPUT, 80)],
                            )?;
                            expected.push(arithmetic(
                                &coefficients,
                                &[0; 3],
                                base,
                                correction,
                                None,
                            ));
                        } else {
                            let base = -64;
                            correction = 20 - threshold;
                            data[291] = (base & 255) as u8;
                            data[247] = (correction & 255) as u8;
                            left = self.wifi_phase(1, fill, false);
                            right = self.probes.invoke(
                                "open_phy_channel_trace_calculate_tx_gain",
                                vec![
                                    ("channel", 1.into()),
                                    ("curve", Buffer::new(INPUT, [0u8; 6]).into()),
                                    ("correction", i64::from(correction).into()),
                                    ("base_and_delta", i64::from(base).into()),
                                    ("output", Buffer::filled(OUTPUT, fill).into()),
                                ],
                                vec![],
                                vec![selection(OUTPUT, 160)],
                            )?;
                            expected.push(arithmetic(
                                &coefficients,
                                &[0; 6],
                                base,
                                correction,
                                Some(1),
                            ));
                        }
                        assert!((-128..=127).contains(&correction));
                        let right = self.enter_probe(right);
                        rows.push(case(
                            "initialize",
                            self.setup(&data, false),
                            Some(self.setup(&data, true)),
                            SessionReset::Cold,
                            false,
                        ));
                        rows.push(case(
                            format!("coefficient-interval-{index}"),
                            left,
                            Some(right),
                            SessionReset::Warm,
                            true,
                        ));
                    }
                    let label = format!(
                        "coefficient-boundary-{}-{fill}-{batch}",
                        python_bool(bluetooth)
                    );
                    let executed = self.compare(&label, rows, fill)?;
                    assert!(!has_events(&executed.records));
                    for (i, value) in expected.iter().enumerate() {
                        let case = 2 * i as u32 + 1;
                        assert_eq!(value.len(), length);
                        assert_eq!(&output(&executed.records, case, false), value);
                        assert_eq!(&output(&executed.records, case, true), value);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn characterize(&mut self) -> Result<()> {
        let memcpy = self.sym(1, "memcpy");
        let kernel_address = self.sym(1, "phy_wifi_get_tx_gain");
        for fill in [0x5a, 0xa5] {
            let mut specs = vec![];
            for curve in [CURVES[0], CURVES[2], CURVES[3]] {
                for channel in [1, 6, 11, 12, 13] {
                    for (base, adjustment, correction) in [
                        (0, 0, 0),
                        (127, 1, 127),
                        (128, 255, -128),
                        (255, 1, -17),
                        (0, 255, 17),
                    ] {
                        specs.push((curve, channel, base, adjustment, correction));
                    }
                }
            }
            for batch in (0..specs.len()).step_by(4) {
                let selected = &specs[batch..(batch + 4).min(specs.len())];
                let data_for =
                    |(curve, _, base, adjustment, correction): &([u8; 6], u32, i32, i32, i32)| {
                        let mut data = vec![0u8; 516];
                        data[241..247].copy_from_slice(curve);
                        data[247] = (correction & 255) as u8;
                        data[291] = *base as u8;
                        data[434] = *adjustment as u8;
                        data
                    };
                let mut rows = vec![];
                for spec in selected {
                    rows.push(case(
                        "initialize",
                        self.setup(&data_for(spec), false),
                        None,
                        SessionReset::Cold,
                        false,
                    ));
                    rows.push(case(
                        "current-kernel-inputs",
                        self.wifi_phase(spec.1, fill, true),
                        None,
                        SessionReset::Warm,
                        false,
                    ));
                }
                let records = self
                    .characterize_vendor(&format!("kernel-inputs-{fill}-{batch}"), rows, fill)?
                    .records;
                let mut direct_rows = vec![];
                for (i, spec) in selected.iter().enumerate() {
                    let (_, channel, base, adjustment, correction) = *spec;
                    let observed = events(&records, 2 * i as u32 + 1, false);
                    assert!(observed.iter().all(|e| matches!(
                        e,
                        ExecutionEvent::CallTransfer { .. }
                            | ExecutionEvent::TransferArgument { .. }
                    )));
                    let kernel = calls(&observed, kernel_address);
                    let effective = signed8((base + adjustment) & 255);
                    assert_eq!(kernel.len(), 1);
                    assert_eq!(
                        kernel[0][..4],
                        [
                            channel,
                            self.parameter + 241,
                            correction as u32,
                            effective as u32
                        ]
                    );
                    let copies = calls(&observed, memcpy);
                    assert!(copies.len() == 3 && copies.iter().all(|c| c[2] == 36));
                    let sources: Vec<u32> = copies.iter().map(|c| c[1]).collect();
                    assert_eq!(
                        sources,
                        (0..3).map(|i| sources[0] + i * 36).collect::<Vec<_>>()
                    );
                    if self.coefficient_address.is_none() {
                        let address = sources[0] - 108;
                        self.coefficient_address = Some(address);
                        assert_eq!(
                            self.image_data("linked-coefficients", address, 216)?,
                            self.coefficients
                        );
                    }
                    assert_eq!(sources[0], self.coefficient_address.unwrap() + 108);
                    assert_eq!(
                        kernel[0][4..7],
                        copies.iter().map(|c| c[0]).collect::<Vec<_>>()[..]
                    );
                    assert_eq!(kernel[0][7..], [OUTPUT, OUTPUT + 32, OUTPUT + 96, 0]);
                    let mut arguments = vec![
                        channel,
                        self.parameter + 241,
                        correction as u32,
                        effective as u32,
                    ];
                    arguments.extend(&sources);
                    arguments.extend([OUTPUT, OUTPUT + 32, OUTPUT + 96, 0]);
                    let direct = self.enter(
                        kernel_address,
                        &arguments,
                        vec![filled(OUTPUT, 160, fill)?],
                        vec![selection(OUTPUT, 160)],
                        vec![],
                    );
                    direct_rows.push(case(
                        "initialize",
                        self.setup(&data_for(spec), false),
                        None,
                        SessionReset::Cold,
                        false,
                    ));
                    direct_rows.push(case(
                        "direct-ROM-kernel",
                        direct,
                        None,
                        SessionReset::Warm,
                        false,
                    ));
                }
                let direct = self
                    .characterize_vendor(
                        &format!("kernel-direct-{fill}-{batch}"),
                        direct_rows,
                        fill,
                    )?
                    .records;
                assert!(!has_events(&direct));
                for (i, (curve, channel, base, adjustment, correction)) in
                    selected.iter().enumerate()
                {
                    let expected = arithmetic(
                        &self.coefficients[108..],
                        curve,
                        signed8((base + adjustment) & 255),
                        *correction,
                        Some(*channel),
                    );
                    let case = 2 * i as u32 + 1;
                    assert_eq!(output(&records, case, false), expected);
                    assert_eq!(output(&direct, case, false), expected);
                }
            }
        }
        Ok(())
    }

    pub fn wifi(&mut self) -> Result<()> {
        for fill in [0x5a, 0xa5] {
            let mut specs = vec![];
            for curve in CURVES {
                for channel in [1, 2, 5, 6, 7, 10, 11, 12, 13] {
                    for (base, correction) in
                        [(0, 0), (-128, 127), (127, -128), (31, -17), (-31, 17)]
                    {
                        specs.push((curve, channel, base, correction));
                    }
                }
            }
            for batch in (0..specs.len()).step_by(6) {
                let mut rows = vec![];
                for &(curve, channel, base, correction) in &specs[batch..batch + 6] {
                    let mut data = vec![0u8; 516];
                    data[241..247].copy_from_slice(&curve);
                    data[247] = (correction & 255) as u8;
                    data[291] = (base & 255) as u8;
                    rows.push(case(
                        "initialize",
                        self.setup(&data, false),
                        Some(self.setup(&data, true)),
                        SessionReset::Cold,
                        false,
                    ));
                    let right = self.probes.invoke(
                        "open_phy_channel_trace_calculate_tx_gain",
                        vec![
                            ("channel", i64::from(channel).into()),
                            ("curve", Buffer::new(INPUT, curve).into()),
                            ("correction", i64::from(correction).into()),
                            ("base_and_delta", i64::from(base).into()),
                            ("output", Buffer::filled(OUTPUT, fill).into()),
                        ],
                        vec![],
                        vec![selection(OUTPUT, 160)],
                    )?;
                    let right = self.enter_probe(right);
                    rows.push(case(
                        format!("wifi-{curve:?}-{channel}-{base}-{correction}"),
                        self.wifi_phase(channel, fill, false),
                        Some(right),
                        SessionReset::Warm,
                        true,
                    ));
                }
                let executed = self.compare(&format!("wifi-{fill}-{batch}"), rows, fill)?;
                if self.wifi_request.is_none() {
                    self.wifi_request = Some(executed.request);
                }
                assert!(!has_events(&executed.records));
                for (i, &(curve, channel, base, correction)) in
                    specs[batch..batch + 6].iter().enumerate()
                {
                    let expected = arithmetic(
                        &self.coefficients[108..],
                        &curve,
                        base,
                        correction,
                        Some(channel),
                    );
                    let case = 2 * i as u32 + 1;
                    assert_eq!(
                        output(&executed.records, case, false),
                        expected,
                        "{batch} {case}"
                    );
                    assert_eq!(
                        output(&executed.records, case, true),
                        expected,
                        "{batch} {case}"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn publish_wifi(&mut self) -> Result<()> {
        for fill in [0x5a, 0xa5] {
            let (mut rows, mut expectations) = (vec![], vec![]);
            for seed_value in [0u32, 0x1357_2468, 0xffff_ffff] {
                for base in [0u32, 32, 224, 255] {
                    let mut image: Vec<u32> = (0..47)
                        .map(|i| seed_value.wrapping_add(i * 0x0102_0305))
                        .collect();
                    for i in 0..32usize {
                        let shift = (i % 2) * 16;
                        let value = [0u32, 128, 256][i % 3];
                        image[14 + i / 2] =
                            (image[14 + i / 2] & !(0xffff << shift)) | (value << shift);
                    }
                    let left = self.enter(
                        self.root("phy_set_tx_gain_mem_new"),
                        &[
                            0,
                            32,
                            INPUT + 120,
                            INPUT + 56,
                            INPUT + 24,
                            INPUT,
                            INPUT + 184,
                        ],
                        vec![known(INPUT, 188, &words(&image))?],
                        vec![],
                        gain_models(base, fill),
                    );
                    let right = self.probes.invoke(
                        "open_phy_channel_trace_publish_tx_gain",
                        vec![("input", Buffer::new(INPUT, words(&image)).into())],
                        gain_models(base, fill),
                        vec![],
                    )?;
                    let right = self.enter_probe(right);
                    rows.push(case(
                        format!("publish-{seed_value}-{base}"),
                        left,
                        Some(right),
                        SessionReset::Cold,
                        false,
                    ));
                    expectations.push(publication(
                        &image[..6],
                        image[46],
                        &words(&image[6..46]),
                        32,
                        base,
                        fill,
                        false,
                    ));
                }
            }
            let executed = self.compare(&format!("publish-wifi-{fill}"), rows, fill)?;
            self.publication_request = Some(executed.request);
            for (i, expected) in expectations.iter().enumerate() {
                assert!(matches!(
                    stop(&executed.records, i as u32, true),
                    ExecutionStop::Returned { low: Some(0), .. }
                ));
                for side in [false, true] {
                    let observed = events(&executed.records, i as u32, side);
                    assert_eq!(observed.len(), 161);
                    assert_eq!(&effects(&observed, false), expected, "{i} {side}");
                }
            }
        }
        Ok(())
    }

    fn install_callbacks(&self, observed_slot: u32) -> Result<Invocation> {
        Ok(self.enter(
            self.root("phy_get_romfunc_addr"),
            &[],
            vec![
                region(
                    0x2f07_fc3c,
                    4,
                    &words(&[0x2f07_f944]),
                    None,
                    RegionLifetime::Session,
                )?,
                region(0x2f07_fc40, 4, &[], None, RegionLifetime::Session)?,
            ],
            vec![selection(observed_slot, 4)],
            vec![],
        ))
    }

    pub fn bluetooth(&mut self) -> Result<()> {
        let tab = self.root("phy_bt_get_tx_tab_new");
        for fill in [0x5a, 0xa5] {
            let mut specs = vec![];
            for curve in [[0u8, 0, 0], [127, 128, 255], [255, 31, 128]] {
                for (base, attenuation, correction) in [
                    (0, 0, 0),
                    (127, 255, 127),
                    (128, 1, -128),
                    (255, 31, -17),
                    (0, 127, 17),
                ] {
                    for bank in [0u32, 32, 224, 255] {
                        specs.push((curve, base, attenuation, correction, bank));
                    }
                }
            }
            for batch in (0..specs.len()).step_by(3) {
                let (mut rows, mut expected) = (vec![], vec![]);
                for &(curve, base, attenuation, correction, bank) in &specs[batch..batch + 3] {
                    let f = u32::from(fill);
                    let mut packed: Vec<u32> = (0..6)
                        .map(|i| (f * 0x0101_0101).wrapping_add(i * 0x0102_0305))
                        .collect();
                    let mut curve_word = curve.to_vec();
                    curve_word.push((correction & 255) as u8);
                    packed.extend([
                        f * 0x0101,
                        u32::from_le_bytes(curve_word.try_into().unwrap()),
                        base as u32 | ((attenuation as u32) << 8),
                    ]);
                    let mut data = vec![0u8; 516];
                    data[260..284].copy_from_slice(&words(&packed[..6]));
                    data[208..210].copy_from_slice(&packed[6].to_le_bytes()[..2]);
                    data[251..255].copy_from_slice(&words(&[packed[7]]));
                    data[292] = base as u8;
                    data[8] = attenuation as u8;
                    data[291] = fill;
                    data[434] = fill;
                    rows.push(case(
                        "initialize",
                        self.setup(&data, false),
                        Some(self.setup(&data, true)),
                        SessionReset::Cold,
                        false,
                    ));
                    let install = self.install_callbacks(0x2f07_f96c)?;
                    let noop = self.enter(
                        self.sym(1, "memcpy"),
                        &[INPUT + 0x800, INPUT + 0x800, 0],
                        vec![],
                        vec![],
                        vec![],
                    );
                    rows.push(case(
                        "install-captured-callbacks",
                        install,
                        Some(noop),
                        SessionReset::Warm,
                        false,
                    ));
                    let left = self.enter(
                        tab,
                        &[OUTPUT + 48, OUTPUT + 16, OUTPUT, 0],
                        vec![region(
                            OUTPUT,
                            80,
                            &[],
                            Some(fill),
                            RegionLifetime::Session,
                        )?],
                        vec![selection(OUTPUT, 80)],
                        vec![],
                    );
                    let right = self.probes.invoke(
                        "open_phy_bluetooth_trace_calculate_gain",
                        vec![
                            ("input", Buffer::new(INPUT, words(&packed)).into()),
                            ("output", Buffer::filled(OUTPUT, fill).session().into()),
                        ],
                        vec![],
                        vec![selection(OUTPUT, 80)],
                    )?;
                    rows.push(case(
                        "bt-calculation",
                        left,
                        Some(self.enter_probe(right)),
                        SessionReset::Warm,
                        true,
                    ));
                    let mut left = self.enter(
                        self.root("phy_bt_set_tx_gain_new"),
                        &[0],
                        vec![],
                        vec![selection(OUTPUT, 80)],
                        gain_models(bank, fill),
                    );
                    left.observe_calls = Some(CallCapture {
                        include_tail: true,
                        argument_words: 0,
                        overrides: vec![],
                    });
                    let right = self.probes.invoke(
                        "open_phy_bluetooth_trace_tx_gain",
                        vec![
                            ("input", Buffer::new(INPUT, words(&packed)).into()),
                            ("output", i64::from(OUTPUT).into()),
                        ],
                        gain_models(bank, fill),
                        vec![selection(OUTPUT, 80)],
                    )?;
                    rows.push(case(
                        "bt-complete-publication",
                        left,
                        Some(self.enter_probe(right)),
                        SessionReset::Warm,
                        true,
                    ));
                    let calculated = arithmetic(
                        &self.coefficients[..108],
                        &curve,
                        signed8((base - attenuation) & 255),
                        correction,
                        None,
                    );
                    let writes =
                        publication(&packed[..6], packed[6], &calculated, 16, bank, fill, true);
                    expected.push((calculated, writes));
                }
                let executed = self.compare(&format!("bluetooth-{fill}-{batch}"), rows, fill)?;
                if self.bluetooth_request.is_none() {
                    self.bluetooth_request = Some(executed.request);
                }
                let records = &executed.records;
                for (i, (calculated, writes)) in expected.iter().enumerate() {
                    let i = i as u32;
                    assert_eq!(output(records, 4 * i + 1, false), tab.to_le_bytes());
                    assert!(events(records, 4 * i + 3, false).iter().any(|e| matches!(e,
                        ExecutionEvent::CallTransfer { target, indirect: true, target_kind: ObservedCallTarget::CapturedCode, .. } if *target == tab)));
                    for side in [false, true] {
                        assert!(events(records, 4 * i + 2, side).is_empty());
                        assert_eq!(&output(records, 4 * i + 2, side), calculated);
                        assert_eq!(&output(records, 4 * i + 3, side), calculated);
                        let observed = effects(&events(records, 4 * i + 3, side), true);
                        assert_eq!(observed.len(), 81);
                        assert_eq!(&observed, writes, "{batch} {i} {side}");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn additive(&mut self) -> Result<()> {
        let mut profiles = vec![(0u8, 0u8, 0u8)];
        profiles.extend([1, 31, 127, 255].map(|a| (0, a, 0)));
        for (base, adjustment) in [(0u8, 1u8), (0, 31), (127, 1), (255, 1), (10, 255)] {
            profiles.push((base, 0, adjustment));
            profiles.push((base.wrapping_add(adjustment), 0, 0));
        }
        let data_for = |base: u8, attenuation: u8, adjustment: u8| {
            let mut data = vec![0u8; 516];
            data[291] = base;
            data[8] = attenuation;
            data[434] = adjustment;
            data
        };
        let mut rows = vec![];
        for &(base, attenuation, adjustment) in &profiles {
            rows.push(case(
                "initialize",
                self.setup(&data_for(base, attenuation, adjustment), false),
                None,
                SessionReset::Cold,
                false,
            ));
            rows.push(case(
                "additive-current",
                self.wifi_phase(13, 0xa5, false),
                None,
                SessionReset::Warm,
                false,
            ));
        }
        let records = self
            .characterize_vendor("additive-current", rows, 0x5a)?
            .records;
        assert!(!has_events(&records));
        let baseline = output(&records, 1, false);
        for i in 1..5 {
            assert_eq!(output(&records, i * 2 + 1, false), baseline);
        }
        for i in (5..15).step_by(2) {
            assert_eq!(
                output(&records, i * 2 + 1, false),
                output(&records, (i + 1) * 2 + 1, false)
            );
        }
        assert_ne!(output(&records, 15, false), baseline); // (base=0, adjustment=31)
        let mut old = vec![];
        for (base, attenuation, adjustment) in [(0, 0, 0), (31, 31, 0), (0, 0, 31)] {
            let phase = self.enter(
                self.sym(1, "phy_wifi_get_tx_tab_"),
                &[13, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
                vec![
                    known(self.sym(1, "phy_param_rom"), 4, &words(&[INPUT + 0x800]))?,
                    filled(OUTPUT, 160, 0xa5)?,
                ],
                vec![selection(OUTPUT, 160)],
                vec![],
            );
            old.push(case(
                "initialize",
                self.setup(&data_for(base, attenuation, adjustment), true),
                None,
                SessionReset::Cold,
                false,
            ));
            old.push(case(
                "subtractive-ROM",
                phase,
                None,
                SessionReset::Warm,
                false,
            ));
        }
        let observed = self
            .characterize_vendor("subtractive-ROM", old, 0x5a)?
            .records;
        assert!(!has_events(&observed));
        let first = output(&observed, 1, false);
        assert_eq!(output(&observed, 3, false), first);
        assert_eq!(output(&observed, 5, false), first);
        assert_ne!(first, baseline);
        Ok(())
    }

    /// Run a request that must fail for capacity and publish nothing.
    pub fn capacity_failure(
        &self,
        label: &str,
        request: &ExecutionRequest,
        command: &str,
    ) -> Result<()> {
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

    pub fn negative(&mut self) -> Result<()> {
        let mut unknown = self.wifi_request.clone().expect("Wi-Fi matrix ran").cases;
        unknown.truncate(2);
        let curve = &mut unknown[1].replacement.as_mut().unwrap().memory[1].seed;
        assert_eq!(curve.address, INPUT);
        curve.bytes.clear();
        curve.fill = None;
        let records = self
            .execute(
                "gain-unknown-curve",
                unknown,
                0x5a,
                Right::Production,
                ComparisonVerdict::Incomplete,
                32768,
            )?
            .records;
        assert!(matches!(
            stop(&records, 1, true),
            ExecutionStop::Incomplete {
                reason: ExecutionGap::Memory { .. },
                ..
            }
        ));
        let mut uninstalled = self
            .bluetooth_request
            .clone()
            .expect("Bluetooth matrix ran")
            .cases;
        uninstalled.truncate(4);
        // Keep parameter setup and the output calculation, but do not execute
        // the installer. The archive's callback pointer remains unknown.
        uninstalled.remove(1);
        uninstalled.last_mut().unwrap().name = "missing-callback-installation".into();
        let records = self
            .execute(
                "gain-missing-callback",
                uninstalled,
                0x5a,
                Right::Production,
                ComparisonVerdict::Incomplete,
                32768,
            )?
            .records;
        assert!(matches!(
            stop(&records, 2, false),
            ExecutionStop::Incomplete { .. }
        ));
        assert!(!events(&records, 2, false).iter().any(|e| matches!(
            e,
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. }
        )));
        let previous = self.artifacts.last().expect("retained execution");
        let (identity, document) = (previous.identity.clone(), previous.document.clone());
        let mut limited = self.publication_request.clone().expect("publication ran");
        limited.cases.truncate(1);
        limited.max_events = 1;
        self.capacity_failure("gain-capacity", &limited, "compare")?;
        self.assert_retained("retained-after-failure", &identity, &document)?;
        // The same ROM kernel receives caller-owned coefficient bytes. This is
        // explicitly a vendor-boundary characterization, not a production match.
        let data = vec![0u8; 516];
        let direct = self.enter(
            self.sym(1, "phy_wifi_get_tx_gain"),
            &[
                13,
                INPUT + 0x800 + 241,
                0,
                0,
                INPUT + 256,
                INPUT + 292,
                INPUT + 328,
                OUTPUT,
                OUTPUT + 32,
                OUTPUT + 96,
                0,
            ],
            vec![
                known(INPUT + 256, 108, &self.coefficients[108..])?,
                filled(OUTPUT, 160, 0x5a)?,
            ],
            vec![selection(OUTPUT, 160)],
            vec![],
        );
        let rows = vec![
            case(
                "initialize",
                self.setup(&data, false),
                Some(self.setup(&data, true)),
                SessionReset::Cold,
                false,
            ),
            case(
                "current-coefficients",
                self.wifi_phase(13, 0x5a, false),
                Some(direct),
                SessionReset::Warm,
                true,
            ),
        ];
        let original = self
            .execute(
                "gain-coefficients-original",
                rows.clone(),
                0x5a,
                Right::Vendor,
                ComparisonVerdict::Match,
                32768,
            )?
            .identity;
        let mut changed = rows;
        changed[1].replacement.as_mut().unwrap().memory[1]
            .seed
            .bytes[72] ^= 1;
        let different = self.execute(
            "gain-coefficients-changed",
            changed,
            0x5a,
            Right::Vendor,
            ComparisonVerdict::Diff,
            32768,
        )?;
        assert_ne!(different.identity, original);
        assert_ne!(
            output(&different.records, 1, false),
            output(&different.records, 1, true)
        );
        Ok(())
    }

    /// Source-free move, backup/restore and exact replay of every retained run.
    pub fn preserve(&mut self) -> Result<()> {
        let backup = self.run.join("backup.blobray");
        let mut command = args(["backup", "--output"]);
        command.push(path_arg(&backup));
        self.runner.call("backup", &command, 0)?;
        let moved = self.run.join("moved");
        fs::rename(&self.runner.project, &moved)?;
        self.runner.project = moved;
        let first = self
            .artifacts
            .first()
            .ok_or_else(|| invalid("no retained executions"))?;
        let reopened = self.runner.execution("moved", &first.identity)?;
        assert_eq!(reopened.records, first.document.records);
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

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// Resolve one uniquely named defined symbol in the exported image and retain
/// the complete defined-symbol listing next to the evidence.
fn image_symbol(elf: &Path, listing: &Path, name: &str) -> Result<(u32, u64)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_oracle_retains_index_control_and_wraps_bank() {
        let wifi = publication(&[0; 6], 0, &[0; 160], 32, 255, 0xa5, false);
        assert_eq!(wifi.len(), 161);
        assert_eq!(
            wifi[1..4],
            [
                (Access::Write, 0x2010_0848, 0),
                (Access::Write, 0x2010_084c, 0x1000_0000),
                (Access::Write, 0x2010_0850, 0x7f80),
            ]
        );
        assert_eq!(wifi[4], (Access::Read, 0x2010_0844, 0xa5a5_a5a5));
        assert_eq!(wifi[5], (Access::Write, 0x2010_0844, 0xa5af_f800));
        assert_eq!(wifi[10], (Access::Write, 0x2010_0844, 0xa5a8_0000));
        // Default is the Wi-Fi bank; explicit BT selects +32 and wraps to zero.
        assert_eq!(
            publication(&[0; 6], 0, &[0; 80], 16, 224, 0, false)[5].2,
            0xf0000
        );
        let bluetooth = publication(&[0; 6], 0, &[0; 80], 16, 224, 0, true);
        assert_eq!(bluetooth.len(), 81);
        assert_eq!(bluetooth[5].2, 0x80000);
    }

    #[test]
    fn arithmetic_selects_intervals_and_narrows_signed_residuals() {
        let mut coefficients = vec![0u8; 108];
        // Descending thresholds 170, 160, ..., 0; mid/low tables echo the index.
        for i in 0..18u16 {
            coefficients[2 * i as usize..][..2].copy_from_slice(&i.to_le_bytes());
            coefficients[36 + 2 * i as usize..][..2].copy_from_slice(&(100 + i).to_le_bytes());
            coefficients[72 + 2 * i as usize..][..2].copy_from_slice(&(170 - 10 * i).to_le_bytes());
        }
        let wifi = arithmetic(&coefficients, &[0; 6], 0, 0, Some(13));
        assert_eq!(wifi.len(), 160);
        // Target 84 selects threshold 80 (index 9): residual 4 - interpolation 2.
        assert_eq!(wifi[0], 2);
        assert_eq!(u16::from_le_bytes([wifi[32], wifi[33]]), 109);
        let bluetooth = arithmetic(&coefficients, &[0; 3], 0, 0, None);
        assert_eq!(bluetooth.len(), 80);
        // Target -72 is below every threshold: index 17 and a clamped residual.
        assert_eq!(bluetooth[0], (-60i32 & 255) as u8);
        assert_eq!(u16::from_le_bytes([bluetooth[16], bluetooth[17]]), 117);
    }
}
