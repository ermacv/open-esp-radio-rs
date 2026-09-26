//! Complete RX-gain root over the captured archive and ROM against compiled
//! production.
//!
//! Publication cases take the table/DC guards; calibration cases execute the
//! complete DC estimator path with signed samples, delayed analog I2C and both
//! work-mode settle branches. Both compare ordered effects, the projected
//! per-gain/base/fine DC coefficients and the two bank limits. Production-only
//! cases require a failed channel, a failed minimum search and an exhausted
//! shared budget to leave coefficients and gain memory unpublished. Software
//! comparison under explicit peripheral inputs, never hardware qualification.
use crate::contracts::{
    OutputField, omitted_read_before, output_projection, phy_contract, plumbing,
};
use crate::evidence::{PhyEffect, events, output, phy_effects, steps};
use crate::harness::{Buffer, Result, case, selection, with_stack_fill};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use crate::phy::{PhyImage, PhyOptions, Right, image_layout, phy_sdk_input, select, start_session};
use crate::session::request;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceDeclaration, EffectContractRef, EffectRule,
    ExecutionCase, ExecutionEvidence, Invocation, LinkRequest, ProjectionRef, ReadRun,
    SessionReset,
};
use std::path::Path;

pub type Options = PhyOptions;

/// Vendor root arguments: the 2437 MHz channel frequency and bandwidth 0.
const ROOT_FREQUENCY: u32 = 2437;
const ROOT_BANDWIDTH: u32 = 0;
/// PBus RX path value selected by both sides.
const RX_PATH: u8 = 0xbf;
/// Semantic parameter words: 16 Wi-Fi per-gain DC, 2 Wi-Fi base, 22 shared
/// per-gain DC, 12 RXBB adjustments and one Wi-Fi auxiliary word.
const PARAMETER_WORDS: usize = 53;
/// Production output: the 52 coefficient words, then the Wi-Fi and shared
/// last gain indices.
const OUTPUT_WORDS: usize = 54;
const OUTPUT_BYTES: u32 = (OUTPUT_WORDS * 2) as u32;
/// Untouched production output bytes beyond the seeded coefficients.
const OUTPUT_FILL: u8 = 0xa5;
/// `phy_param` offsets of the semantic RX state.
const WIFI_INDEX_DC: usize = 334;
const WIFI_DC_BASE: usize = 366;
const SHARED_INDEX_DC: usize = 436;
const RXBB_ADJUSTMENTS: usize = 480;
const WIFI_AUXILIARY: usize = 212;
const GUARD_FLAGS: usize = 164;
const TABLE_MODE: usize = 16;
const PARAMETER_RX_PATH: usize = 2;
const SHARED_LAST_INDEX: usize = 288;
const WIFI_LAST_INDEX: usize = 289;
/// Initial last-index bytes and two cleared selector bytes.
const INITIAL_SHARED_LAST: u8 = 75;
const INITIAL_WIFI_LAST: u8 = 71;
const CLEARED: [usize; 2] = [79, 430];
/// Guard flags: DC already calibrated (bit 0) and tables initialized (bit 1).
const DC_CALIBRATED: u8 = 1;
const TABLES_INITIALIZED: u8 = 2;
/// Outer DC control snapshot the vendor reads even when DC is skipped.
const DC_CONTROL: u32 = 0x2010_0434;
/// Registers the vendor reads immediately after that skipped-DC snapshot,
/// before table initialization and with initialized tables. Calibration never
/// reads DC control immediately before either.
const SKIPPED_DC_SUCCESSORS: [u32; 2] = [0x2010_088c, 0x2010_702c];
/// Skipped-DC snapshots per root.
const SKIPPED_DC_SNAPSHOTS: u32 = 1;
/// Production RX entry compared with `phy_set_rx_gain_table`.
const PRODUCTION_ENTRY: &str = "open_phy_calibration_trace_rx_gain";
/// DC estimator readiness (bit 16), signed samples and activity.
const ESTIMATOR_READY: u32 = 0x2010_047c;
const ESTIMATOR_DONE: u32 = 0x10000;
const ESTIMATOR_SAMPLE: u32 = 0x2010_0464;
const ESTIMATOR_NEGATED: u32 = 0x2010_0468;
const ESTIMATOR_MAGNITUDE: u32 = 0x2010_046c;
const ESTIMATOR_ACTIVITY: u32 = 0x2010_08d0;
/// PBus readiness word of the calibration path.
const PBUS_READY: u32 = 0x2010_0894;
const PBUS_READY_VALUE: u32 = 0x0100_0100;
/// Idle PBus status with writable retained clock bits.
const PBUS_IDLE: u32 = 0x1234;
const WORK_MODE_SETTLE: u32 = 2;
/// Retained RX analog selector (block 0x67, register 3).
const RX_ANALOG: u32 = 0x0367;
/// Production may execute at most this multiple of the vendor's steps.
const MAX_PRODUCTION_STEP_MULTIPLIER: u64 = 2;
/// Production outcomes of the typed RX calibration failures.
const FAILED_CALIBRATION: u32 = 4;
const OPERATION_LIMIT: u32 = 5;
/// Estimator readiness samples of the failed and slow minimum profiles.
const NEVER_READY_SAMPLES: u32 = 10_000;
const SLOW_MINIMA: u32 = 20;
const SLOW_MINIMUM_POLLS: u32 = 9_000;
/// Event capacity of the shared-budget case: every slow poll is a recorded read.
const BUDGET_EVENTS: u32 = SLOW_MINIMA * (SLOW_MINIMUM_POLLS + 1) + MAX_EVENTS;
/// Cases of one profile: parameter setup, callback installation and the root.
const PROFILE_CASES: u32 = 3;
/// Case index of the root within its profile.
const ROOT: u32 = 2;

/// Signed DC estimator accumulator stream of the calibration path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Estimator {
    /// Every estimate reads this sample, with a matching power accumulator.
    Constant(i32),
    /// Successive estimates read these samples in turn with a zero power
    /// accumulator, so estimates differ as corrections take effect.
    Cycle(&'static [i32]),
}

/// One RX root profile.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Guard flags; zero executes calibration.
    pub flags: u8,
    pub estimator: Estimator,
    pub seed: u16,
    pub fill: u8,
    /// Work-mode settle branch and delayed analog I2C completion.
    pub settle: bool,
}

impl Profile {
    fn calibrates(&self) -> bool {
        self.flags == 0
    }
    fn label(&self) -> String {
        let estimator = match self.estimator {
            Estimator::Constant(sample) => format!("sample{sample}"),
            Estimator::Cycle(samples) => format!(
                "cycle{}",
                samples
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join("_")
            ),
        };
        format!(
            "rx-flags{}-{estimator}-seed{}-fill{:x}-settle{}",
            self.flags,
            self.seed,
            self.fill,
            u8::from(self.settle)
        )
    }
}

fn profiles(flags: &[u8], estimators: &[Estimator]) -> Vec<Profile> {
    let mut result = vec![];
    for &estimator in estimators {
        for &flags in flags {
            for seed in [1, 17] {
                for fill in FILLS {
                    for settle in [false, true] {
                        result.push(Profile {
                            flags,
                            estimator,
                            seed,
                            fill,
                            settle,
                        });
                    }
                }
            }
        }
    }
    result
}

/// Publication through both table/DC guards.
pub fn publication_profiles() -> Vec<Profile> {
    profiles(
        &[DC_CALIBRATED, DC_CALIBRATED | TABLES_INITIALIZED],
        &[Estimator::Constant(0)],
    )
}

/// Complete calibration with zero, small and saturating signed samples, and
/// with a periodic stream whose corrections reach the published codes.
pub fn calibration_profiles() -> Vec<Profile> {
    profiles(
        &[0],
        &[0, 64, -64, 1 << 24, -(1 << 24)]
            .map(Estimator::Constant)
            .into_iter()
            .chain([Estimator::Cycle(CALIBRATION_CYCLE)])
            .collect::<Vec<_>>(),
    )
}

/// Accumulator value of one RX-DC estimate: the calibration requests
/// estimator control `0x800`, and the estimator shifts the accumulator right
/// by six and divides it by the control plus one.
const ESTIMATE_UNIT: i32 = (1 << 6) * (0x800 + 1);

/// Estimator stream whose three-read period is odd against the low/high read
/// pair of a baseband iteration, so successive iterations see different
/// deltas: corrections are applied until a later iteration converges and
/// publishes the corrected code, and a seven-unit delta against a fifty-unit
/// low estimate reaches the low-estimate correction of the shared bank.
const CALIBRATION_CYCLE: &[i32] = &[50 * ESTIMATE_UNIT, 57 * ESTIMATE_UNIT, 50 * ESTIMATE_UNIT];

/// Sample, negated-sample and power accumulator reads of `estimator`.
fn estimator_models(estimator: Estimator) -> Vec<DeviceDeclaration> {
    let streams = |sample: i32, power: u32| {
        [
            ("estimator-sample", ESTIMATOR_SAMPLE, sample as u32),
            (
                "estimator-negated",
                ESTIMATOR_NEGATED,
                sample.wrapping_neg() as u32,
            ),
            ("estimator-magnitude", ESTIMATOR_MAGNITUDE, power),
        ]
    };
    match estimator {
        Estimator::Constant(sample) => streams(sample, sample.unsigned_abs())
            .into_iter()
            .map(|(id, address, value)| constant_read(id, address, value))
            .collect(),
        Estimator::Cycle(samples) => (0..3)
            .map(|stream| {
                let reads = samples.iter().map(|&sample| streams(sample, 0)[stream]);
                let (id, address, _) = streams(0, 0)[stream];
                DeviceDeclaration {
                    id: id.into(),
                    applicability: "periodic synthetic DC estimator stream with zero power".into(),
                    lifetime: blobray_domain::RegionLifetime::Phase,
                    behavior: blobray_domain::DeviceBehavior::CyclicRead {
                        address,
                        width: 4,
                        values: reads.map(|(_, _, value)| value).collect(),
                    },
                }
            })
            .collect(),
    }
}

/// Deterministic 9-bit per-gain DC words, signed RXBB adjustments and the
/// auxiliary word, derived from `seed`.
pub fn parameters(seed: u16) -> [u16; PARAMETER_WORDS] {
    core::array::from_fn(|i| match i {
        0..40 => seed.wrapping_add(i as u16 * 7) & 0x1ff,
        40..52 => (i as i16 - 46).wrapping_mul(seed as i16) as u16,
        _ => 0x125,
    })
}

fn put_words(image: &mut [u8], offset: usize, values: &[u16]) {
    for (i, value) in values.iter().enumerate() {
        image[offset + 2 * i..offset + 2 * i + 2].copy_from_slice(&value.to_le_bytes());
    }
}

/// Captured `phy_param` image of one profile.
pub fn parameter_image(profile: &Profile) -> Vec<u8> {
    let values = parameters(profile.seed);
    let mut image = vec![0u8; PHY_PARAM_BYTES as usize];
    let guards = u32::from(profile.flags & DC_CALIBRATED) * 128
        + u32::from((profile.flags & TABLES_INITIALIZED) >> 1) * 512;
    image[GUARD_FLAGS..GUARD_FLAGS + 4].copy_from_slice(&guards.to_le_bytes());
    // The qualified normal gain table; the alternate vendor table mode is excluded.
    image[TABLE_MODE..TABLE_MODE + 2].copy_from_slice(&0u16.to_le_bytes());
    put_words(&mut image, WIFI_INDEX_DC, &values[..16]);
    put_words(&mut image, WIFI_DC_BASE, &values[16..18]);
    put_words(&mut image, SHARED_INDEX_DC, &values[18..40]);
    put_words(&mut image, RXBB_ADJUSTMENTS, &values[40..52]);
    put_words(&mut image, WIFI_AUXILIARY, &values[52..]);
    image[PARAMETER_RX_PATH] = RX_PATH;
    image[SHARED_LAST_INDEX] = INITIAL_SHARED_LAST;
    image[WIFI_LAST_INDEX] = INITIAL_WIFI_LAST;
    for offset in CLEARED {
        image[offset] = 0;
    }
    image
}

/// Committed RX `phy_param` fields in production output order: the 52
/// coefficient words, then the Wi-Fi and shared last gain indices, each in
/// the low byte of an output word.
pub const COMMITTED: [OutputField; 6] = [
    OutputField {
        name: "wifi-index-dc",
        parameter: WIFI_INDEX_DC as u32,
        output: 0,
        width: 2,
        count: 16,
    },
    OutputField {
        name: "wifi-dc-base",
        parameter: WIFI_DC_BASE as u32,
        output: 32,
        width: 2,
        count: 2,
    },
    OutputField {
        name: "shared-index-dc",
        parameter: SHARED_INDEX_DC as u32,
        output: 36,
        width: 2,
        count: 22,
    },
    OutputField {
        name: "rxbb-adjustments",
        parameter: RXBB_ADJUSTMENTS as u32,
        output: 80,
        width: 2,
        count: 12,
    },
    OutputField {
        name: "wifi-last-index",
        parameter: WIFI_LAST_INDEX as u32,
        output: 104,
        width: 1,
        count: 1,
    },
    OutputField {
        name: "shared-last-index",
        parameter: SHARED_LAST_INDEX as u32,
        output: 106,
        width: 1,
        count: 1,
    },
];

/// Production output seeded with the input coefficients and untouched indices.
fn seeded_output(profile: &Profile) -> Vec<u8> {
    let mut bytes: Vec<u8> = parameters(profile.seed)[..52]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    bytes.extend([OUTPUT_FILL; 4]);
    bytes
}

/// Explicit peripheral inputs; `ready` selects channel readiness.
pub fn rx_models(profile: &Profile, ready: bool) -> Vec<DeviceDeclaration> {
    // Work mode selects the settle branch and PBus starts idle; every other
    // radio register is retained storage starting with the fill pattern.
    let mut inputs = vec![
        (WORK_MODE, u32::from(profile.settle) * WORK_MODE_SETTLE),
        (PBUS_STATUS, PBUS_IDLE),
    ];
    let mut models = vec![];
    if profile.calibrates() {
        inputs.extend([(I2C_READ_MASK, 0), (I2C_HOST_MAP, 0)]);
        models.push(analog_bank(
            "rx-analog",
            "explicit retained RX analog register",
            u32::from(profile.settle) * 2,
            [0, 0],
            vec![CommandCell {
                selector: RX_ANALOG,
                initial: u32::from(profile.fill),
                reads: None,
            }],
        ));
        for (id, address, value) in [
            ("pbus-ready", PBUS_READY, PBUS_READY_VALUE),
            (
                "channel-ready",
                CHANNEL_STATUS,
                if ready { CHANNEL_READY } else { 0 },
            ),
            ("estimator-ready", ESTIMATOR_READY, ESTIMATOR_DONE),
        ] {
            models.push(constant_read(id, address, value));
        }
        models.extend(estimator_models(profile.estimator));
    }
    models.insert(
        0,
        register_bank(
            "rx-inputs",
            "work-mode settle branch, idle PBus and transport controls",
            inputs,
        ),
    );
    models.push(radio_aperture(profile.fill));
    models
}

/// Reviewed rules of the RX root: transport plumbing, the PBus status polling
/// interval and the vendor's unused skipped-DC snapshot.
pub fn rx_rules() -> Vec<EffectRule> {
    let mut rules = plumbing(&[PBUS_STATUS], MAX_EVENTS);
    for successor in SKIPPED_DC_SUCCESSORS {
        rules.push(omitted_read_before(
            format!("skipped-dc-snapshot-{successor:08x}"),
            DC_CONTROL,
            successor,
            SKIPPED_DC_SNAPSHOTS,
            "the vendor snapshots DC control on the skipped-DC path and never uses the value",
        ));
    }
    rules
}

pub struct RxGain {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
    effects: EffectContractRef,
    committed: ProjectionRef,
}

impl std::ops::Deref for RxGain {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for RxGain {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

impl RxGain {
    pub fn new(options: &Options, phy_sdk: &Path) -> Result<Self> {
        let session = start_session(
            options,
            &[phy_sdk_input(phy_sdk)],
            "captured RX gain publication and DC calibration; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            revision: Some(session.revision.clone()),
            inputs: vec![0],
            entry: select(&session, 0, "phy_set_rx_gain_table")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
        };
        // ROM first; the co-located RFPLL diagnostics reference `phy_printf`,
        // bound to the authenticated PHY SDK firmware and never executed.
        let mut image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_set_rx_gain_table",
            &[ROM_INPUT, PHY_SDK_INPUT],
        )?;
        let applicability = "RX gain publication and DC calibration under explicit estimator, PBus and readiness inputs";
        let (vendor, production) = (
            image.vendor_endpoint("phy_set_rx_gain_table")?,
            image.production_endpoint(PRODUCTION_ENTRY)?,
        );
        let projection = output_projection(
            vendor.clone(),
            image.parameter,
            production.clone(),
            OUTPUT_BYTES,
            &COMMITTED,
            applicability,
        );
        let contract = phy_contract(vendor, production, rx_rules(), applicability);
        let effects = image.review_effects(
            "rx-effects",
            "esp32s31.phy.rx-gain.effects",
            contract,
            "every RX register effect compares exactly except transport polling and the unused skipped-DC snapshot",
        )?;
        let committed = image.review_projection(
            "rx-committed",
            "esp32s31.phy.rx-gain.committed",
            projection,
            "production publishes the committed DC coefficients and bank limits",
        )?;
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            image,
            effects,
            committed,
        })
    }

    fn vendor_phase(&self, profile: &Profile) -> Result<Invocation> {
        let mut phase = self.enter(
            self.root("phy_set_rx_gain_table"),
            &[ROOT_FREQUENCY, ROOT_BANDWIDTH],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            rx_models(profile, true),
        );
        phase.calls = delay_calls("rx-delay", self.rom_delay);
        Ok(phase)
    }

    pub fn production_phase(
        &self,
        profile: &Profile,
        models: Vec<DeviceDeclaration>,
    ) -> Result<Invocation> {
        let input: Vec<u8> = parameters(profile.seed)
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let probe = self.probes.invoke(
            PRODUCTION_ENTRY,
            vec![
                ("input", Buffer::new(INPUT, input).into()),
                ("flags", i64::from(profile.flags).into()),
                ("crystal_selector", 0.into()),
                ("pbus_rx_path", i64::from(RX_PATH).into()),
                ("output", Buffer::new(OUTPUT, seeded_output(profile)).into()),
            ],
            models,
            vec![selection(OUTPUT, OUTPUT_BYTES)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.calls = delay_calls("rx-delay", self.production_delay);
        Ok(phase)
    }

    fn rows(&self, profile: &Profile) -> Result<Vec<ExecutionCase>> {
        let image = parameter_image(profile);
        let mut root = case(
            profile.label(),
            self.vendor_phase(profile)?,
            Some(self.production_phase(profile, rx_models(profile, true))?),
            SessionReset::Warm,
            false,
        );
        // Blobray compares every effect and the committed state under the
        // reviewed contract and projection.
        let relation = root.relation.as_mut().unwrap();
        relation.effects = Some(self.effects.clone());
        relation.projection = Some(self.committed.clone());
        Ok(with_stack_fill(
            vec![
                case(
                    "initialize",
                    self.setup(&image, false),
                    Some(self.setup(&image, true)),
                    SessionReset::Cold,
                    false,
                ),
                case(
                    "install-captured-callbacks",
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?,
                    Some(self.noop()),
                    SessionReset::Warm,
                    false,
                ),
                root,
            ],
            profile.fill,
        ))
    }
}

fn check(label: &str, records: &[ExecutionEvidence], root: u32) {
    assert_eq!(returned_low(records, root, true), Some(0), "{label}");
    for side in [false, true] {
        assert!(all_complete(records, root, side), "{label} {side}");
    }
    let (vendor_steps, production_steps) =
        (steps(records, root, false), steps(records, root, true));
    assert!(
        production_steps <= vendor_steps.saturating_mul(MAX_PRODUCTION_STEP_MULTIPLIER),
        "{label}: production {production_steps} steps, vendor {vendor_steps}"
    );
}

/// Each matrix is one request; every profile starts cold with its own stack fill.
pub fn exercise(ctx: &mut RxGain) -> Result<()> {
    for (name, matrix) in [
        ("publication", publication_profiles()),
        ("calibration", calibration_profiles()),
    ] {
        let mut rows = vec![];
        for profile in &matrix {
            rows.extend(ctx.rows(profile)?);
        }
        let executed = ctx.image.execute_without_events(
            &format!("rx-{name}"),
            rows,
            FILLS[0],
            Right::Production,
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )?;
        for (i, profile) in matrix.iter().enumerate() {
            let root = i as u32 * PROFILE_CASES + ROOT;
            check(&profile.label(), &executed.records, root);
        }
    }
    containment(ctx)?;
    negative(ctx)
}

/// One production-only failing root.
struct Failure {
    label: String,
    models: Vec<DeviceDeclaration>,
    /// Expected typed failure.
    outcome: u32,
    /// Successful estimator minima before the failure: exact, or a range.
    minima: std::ops::Range<u32>,
}

/// Unpublished coefficients and gain memory after a failed root.
fn assert_unpublished(label: &str, profile: &Profile, records: &[ExecutionEvidence], case: u32) {
    assert_eq!(
        output(records, case, false),
        seeded_output(profile),
        "{label}: output published"
    );
    assert!(
        !phy_effects(&events(records, case, false))
            .iter()
            .any(|e| matches!(e, PhyEffect::Write(GAIN_INDEX, _))),
        "{label}: gain memory published"
    );
}

/// A channel that never becomes ready, a minimum search whose estimator never
/// completes and slow successful minima that exhaust the shared operation
/// budget all fail without publishing coefficients or gain memory. One
/// production-only request per fill; each root starts cold.
fn containment(ctx: &mut RxGain) -> Result<()> {
    // Both fills are one request; each root sets its own stack fill.
    let (mut rows, mut parts) = (vec![], vec![]);
    for fill in FILLS {
        let profile = Profile {
            flags: 0,
            estimator: Estimator::Constant(0),
            seed: 17,
            fill,
            settle: false,
        };
        let mut failures = vec![Failure {
            label: "channel-never-ready".into(),
            models: rx_models(&profile, false),
            outcome: FAILED_CALIBRATION,
            minima: 0..1,
        }];
        for slow in [false, true] {
            let mut models = rx_models(&profile, true);
            models.retain(|m| m.id != "estimator-ready");
            let runs = if slow {
                (0..SLOW_MINIMA)
                    .flat_map(|_| {
                        [
                            ReadRun {
                                value: 0,
                                count: SLOW_MINIMUM_POLLS,
                            },
                            ReadRun::once(ESTIMATOR_DONE),
                        ]
                    })
                    .collect()
            } else {
                vec![ReadRun {
                    value: 0,
                    count: NEVER_READY_SAMPLES,
                }]
            };
            models.push(sequence_read("estimator-ready", ESTIMATOR_READY, runs));
            // The not-ready branch also samples estimator activity; idle here.
            models.push(constant_read("estimator-activity", ESTIMATOR_ACTIVITY, 0));
            failures.push(if slow {
                Failure {
                    label: "minimum-shared-budget".into(),
                    models,
                    outcome: OPERATION_LIMIT,
                    // Several minima complete, but not the whole input sequence.
                    minima: 2..SLOW_MINIMA,
                }
            } else {
                Failure {
                    label: "minimum-timeout".into(),
                    models,
                    outcome: FAILED_CALIBRATION,
                    minima: 0..1,
                }
            });
        }
        for failure in &failures {
            let mut row = case(
                format!("{}-fill{fill:x}", failure.label),
                ctx.production_phase(&profile, failure.models.clone())?,
                None,
                SessionReset::Cold,
                false,
            );
            row.relation = None;
            row.stack_fill = Some(fill);
            rows.push(row);
        }
        parts.push((profile, failures));
    }
    let production = ctx.production.clone();
    let request = request(&production, None, None, rows, BUDGET_EVENTS);
    let records = ctx
        .submit("rx-containment", &request, None)?
        .records
        .clone();
    let mut case = 0u32;
    for (profile, failures) in &parts {
        for failure in failures {
            let label = format!("rx-containment-fill{:x}-{}", profile.fill, failure.label);
            assert_eq!(
                returned_low(&records, case, false),
                Some(failure.outcome),
                "{label}"
            );
            assert_unpublished(&label, profile, &records, case);
            let minima = phy_effects(&events(&records, case, false))
                .iter()
                .filter(|e| matches!(e, PhyEffect::Read(ESTIMATOR_READY, ESTIMATOR_DONE)))
                .count() as u32;
            assert!(failure.minima.contains(&minima), "{label}: {minima} minima");
            case += 1;
        }
    }
    Ok(())
}

/// A saturating production sample is a DIFF, omitted callback installation leaves
/// the vendor INCOMPLETE, and an undersized event capacity publishes nothing.
fn negative(ctx: &mut RxGain) -> Result<()> {
    let profile = calibration_profiles()[0];
    let mut changed = ctx.rows(&profile)?;
    // A saturating sample drives a different minimum search.
    let other = Profile {
        estimator: Estimator::Constant(1 << 24),
        ..profile
    };
    changed[ROOT as usize].replacement =
        Some(ctx.production_phase(&other, rx_models(&other, true))?);
    ctx.image.execute(
        "rx-negative-changed-sample",
        changed,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let mut uninstalled = ctx.rows(&profile)?;
    uninstalled[1].vendor = ctx.noop();
    ctx.image.execute(
        "rx-negative-uninstalled-callbacks",
        uninstalled,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Incomplete,
        MAX_EVENTS,
    )?;
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    let mut rows = ctx.rows(&profile)?;
    let relation = rows[ROOT as usize].relation.as_mut().unwrap();
    relation.effects = None;
    relation.projection = None;
    let limited = request(
        &ctx.vendor,
        Some(&ctx.production),
        Some(profile.fill),
        rows,
        1,
    );
    ctx.capacity_failure("rx-negative-capacity", &limited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::ExecutionEvent;

    fn read(address: u32, value: u32) -> ExecutionEvent {
        ExecutionEvent::Read {
            address,
            width: 4,
            value,
        }
    }

    #[test]
    fn matrices_cover_guards_samples_seeds_fills_and_settle() {
        let publication = publication_profiles();
        assert_eq!(publication.len(), 2 * 2 * FILLS.len() * 2);
        assert!(publication.iter().all(|p| !p.calibrates()));
        let calibration = calibration_profiles();
        assert_eq!(calibration.len(), 6 * 2 * FILLS.len() * 2);
        let samples: std::collections::BTreeSet<_> =
            calibration.iter().map(|p| p.estimator).collect();
        assert!(
            samples.contains(&Estimator::Constant(1 << 24))
                && samples.contains(&Estimator::Constant(-(1 << 24)))
                && samples.contains(&Estimator::Cycle(CALIBRATION_CYCLE))
        );
    }

    #[test]
    fn parameter_image_places_every_semantic_field() {
        let profile = Profile {
            flags: DC_CALIBRATED | TABLES_INITIALIZED,
            estimator: Estimator::Constant(0),
            seed: 17,
            fill: 0x5a,
            settle: false,
        };
        let image = parameter_image(&profile);
        let values = parameters(17);
        let word = |offset: usize| u16::from_le_bytes([image[offset], image[offset + 1]]);
        assert_eq!(word(WIFI_INDEX_DC), values[0]);
        assert_eq!(word(WIFI_DC_BASE + 2), values[17]);
        assert_eq!(word(SHARED_INDEX_DC + 42), values[39]);
        assert_eq!(word(RXBB_ADJUSTMENTS + 22), values[51]);
        assert_eq!(word(WIFI_AUXILIARY), 0x125);
        assert_eq!(
            u32::from_le_bytes(image[GUARD_FLAGS..GUARD_FLAGS + 4].try_into().unwrap()),
            128 + 512
        );
        assert_eq!(image[PARAMETER_RX_PATH], RX_PATH);
        assert_eq!(
            (image[SHARED_LAST_INDEX], image[WIFI_LAST_INDEX]),
            (INITIAL_SHARED_LAST, INITIAL_WIFI_LAST)
        );
    }

    #[test]
    fn committed_fields_tile_the_output_except_index_padding() {
        let mut covered = vec![false; OUTPUT_BYTES as usize];
        for field in &COMMITTED {
            let length = (u32::from(field.width) * field.count) as usize;
            assert!(field.parameter as usize + length <= PHY_PARAM_BYTES as usize);
            for byte in &mut covered[field.output as usize..][..length] {
                assert!(!*byte, "{}", field.name);
                *byte = true;
            }
        }
        let padding: Vec<_> = (0..covered.len()).filter(|i| !covered[*i]).collect();
        assert_eq!(padding, [105, 107]);
    }

    #[test]
    fn contract_selects_only_the_skipped_dc_snapshot_and_polling_waits() {
        let rules = rx_rules();
        let selected = |event: &ExecutionEvent, next: &ExecutionEvent| {
            rules
                .iter()
                .filter(|r| r.vendor.unwrap().selects(event, Some(next)))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
        };
        let delay = ExecutionEvent::DelayMicros { value: 1 };
        // A wait before an estimator readiness read is compared; one before a
        // PBus status read is polling.
        assert!(selected(&delay, &read(ESTIMATOR_READY, 0)).is_empty());
        assert_eq!(selected(&delay, &read(PBUS_STATUS, 0)).len(), 1);
        // Only the snapshot before a skipped-path successor may be omitted.
        for successor in SKIPPED_DC_SUCCESSORS {
            assert_eq!(selected(&read(DC_CONTROL, 7), &read(successor, 0)).len(), 1);
        }
        assert!(selected(&read(DC_CONTROL, 7), &read(DC_CONTROL, 7)).is_empty());
    }
}
