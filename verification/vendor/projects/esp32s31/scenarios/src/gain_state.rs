//! Captured calibration storage and RF-test power-policy characterization.
//!
//! No production storage or RF power API is inferred from these vendor
//! observations. All state changes execute captured instructions through native
//! warm phases. Storage needs only the pinned PHY archive and ROM. Without the
//! authenticated RF-test archive the producer is reported as an unmet
//! obligation, never omitted.
use crate::evidence::{Access, Effect, calls, effects, events, has_events, output, stop};
use crate::gain::{Gain, Right, arithmetic, gain_models, publication, signed8};
use crate::harness::{Result, case, filled, known, region, selection, words};
use crate::layout::*;
use blobray_domain::{
    ArtifactId, CallCapture, CallWordCount, ComparisonVerdict, DeviceBehavior, DeviceDeclaration,
    ExecutionGap, ExecutionRequest, ExecutionStop, MemoryAccess, RegionLifetime, RegisterCell,
    SessionReset,
};
use serde::Serialize;

pub const CACHE: u32 = 0x3fff_5000;
pub const INIT: u32 = 0x3fff_6000;
const MAC_POWER: u32 = 0x2010_5500;

/// Independently read policy examples, including truncating remainder and i8 wrap:
/// target power, attenuation, adjustment byte, MAC index and gain publication.
pub const POWER: [(i32, i32, u8, i32, bool); 15] = [
    (80, 0, 0, 20, false),
    (80, 1, 3, 19, false),
    (80, 2, 2, 19, false),
    (80, 3, 1, 19, false),
    (80, 4, 0, 19, false),
    (80, -1, 1, 20, false),
    (80, -2, 2, 20, false),
    (80, -3, 3, 20, false),
    (80, -4, 0, 21, false),
    (80, -5, 1, 21, true),
    (80, -8, 4, 21, true),
    (84, -1, 1, 21, true),
    (80, -48, 0, -32, false),
    (80, 127, 1, -12, false),
    (80, -128, 0, -12, false),
];

/// An obligation that could not execute; its presence fails the scenario.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Unmet {
    pub id: &'static str,
    pub unit: &'static str,
    pub reason: &'static str,
}

pub const RFTEST_OBLIGATION: Unmet = Unmet {
    id: "rftest-power-producer",
    unit: "12.5",
    reason: "authenticated librftest.a was not supplied; producer, rounding/saturation and gain/MAC publication cases did not execute",
};

/// Two read-modify-write updates of the 6-bit index fields at bits 0 and 8.
pub fn mac_power(index: i32, initial: u32) -> Vec<Effect> {
    let index = (index & 63) as u32;
    let first = (initial & !63) | index;
    let second = (first & !(63 << 8)) | (index << 8);
    vec![
        (Access::Read, MAC_POWER, initial),
        (Access::Write, MAC_POWER, first),
        (Access::Read, MAC_POWER, first),
        (Access::Write, MAC_POWER, second),
    ]
}

struct Baseline {
    request: ExecutionRequest,
    identity: ArtifactId,
}

fn state_selection(g: &Gain) -> Vec<blobray_domain::MemorySelection> {
    vec![selection(g.parameter, PHY_PARAM_BYTES)]
}

fn storage(g: &mut Gain) -> Result<Baseline> {
    let startup = g.image_data("startup-adjustment", g.parameter + 434, 1)?;
    assert_eq!(startup, [0]);
    let memset = g.sym(1, "memset");
    let memcpy = g.sym(1, "memcpy");
    let mut baseline = None;
    for fill in [0x5a, 0xa5] {
        let adjustments = [0u8, 1, 31, 127, 128, 255];
        for batch in (0..6).step_by(6) {
            let (mut rows, mut states) = (vec![], vec![]);
            for &adjustment in &adjustments[batch..] {
                let mut state: Vec<u8> = (0..PHY_PARAM_BYTES)
                    .map(|i| (i * 37 + u32::from(fill)) as u8)
                    .collect();
                state[434] = adjustment;
                let mut seed = g.setup(&state, false);
                seed.observe_memory = state_selection(g);
                rows.push(case(
                    "seed-complete-state",
                    seed,
                    None,
                    SessionReset::Cold,
                    false,
                ));
                let inverted: Vec<u8> = state.iter().map(|v| v ^ 255).collect();
                let phases = [
                    (
                        "backup",
                        g.enter(
                            g.root("phy_rf_cal_data_backup_new"),
                            &[CACHE],
                            vec![region(
                                CACHE,
                                532,
                                &[],
                                Some(fill),
                                RegionLifetime::Session,
                            )?],
                            vec![selection(CACHE, 532)],
                            vec![],
                        ),
                    ),
                    ("destroy-live-state", g.setup(&inverted, false)),
                    (
                        "recover-saved-state",
                        g.enter(
                            g.root("phy_rf_cal_data_recovery_new"),
                            &[CACHE],
                            vec![],
                            vec![],
                            vec![],
                        ),
                    ),
                    (
                        "register-init-parameters",
                        g.enter(
                            g.root("register_chipv7_phy_init_param"),
                            &[INIT],
                            vec![filled(INIT, 128, fill)?],
                            vec![],
                            vec![],
                        ),
                    ),
                    (
                        "isolate-curve",
                        g.enter(memset, &[g.parameter + 241, 0, 7], vec![], vec![], vec![]),
                    ),
                    (
                        "isolate-base",
                        g.enter(memset, &[g.parameter + 291, 0, 1], vec![], vec![], vec![]),
                    ),
                    ("consume-restored-adjustment", g.wifi_phase(13, fill, false)),
                    (
                        "clear-adjustment",
                        g.enter(memset, &[g.parameter + 434, 0, 1], vec![], vec![], vec![]),
                    ),
                    (
                        "move-adjustment-to-base",
                        g.enter(
                            memcpy,
                            &[g.parameter + 291, INPUT, 1],
                            vec![known(INPUT, 1, &[adjustment])?],
                            vec![],
                            vec![],
                        ),
                    ),
                    ("consume-normalized-base", g.wifi_phase(13, fill, false)),
                ];
                for (name, mut phase) in phases {
                    if phase.observe_memory.is_empty() {
                        phase.observe_memory = state_selection(g);
                    }
                    rows.push(case(name, phase, None, SessionReset::Warm, false));
                }
                states.push(state);
            }
            let executed = g.characterize_vendor(&format!("storage-{fill}-{batch}"), rows, fill)?;
            let records = &executed.records;
            assert!(!has_events(records));
            for (i, state) in states.iter().enumerate() {
                let offset = 11 * i as u32;
                let at = |n: u32| output(records, offset + n, false);
                assert_eq!(at(0), *state);
                let mut backup = vec![fill; 12];
                backup.extend(state);
                backup.extend([fill; 4]);
                assert_eq!(at(1), backup);
                assert_eq!(at(2), state.iter().map(|v| v ^ 255).collect::<Vec<_>>());
                assert_eq!(at(3), *state);
                // Exact byte writes of the captured parameter registration loops.
                let mut initialized = state.clone();
                initialized[78] = fill;
                initialized[80..98].fill(fill);
                initialized[100..152].fill(fill);
                assert_eq!(at(4), initialized);
                initialized[241..248].fill(0);
                assert_eq!(at(5), initialized);
                initialized[291] = 0;
                assert_eq!(at(6), initialized);
                let expected = arithmetic(
                    &g.coefficients[108..],
                    &[0; 6],
                    signed8(i32::from(state[434])),
                    0,
                    Some(13),
                );
                assert_eq!(at(7), expected);
                initialized[434] = 0;
                assert_eq!(at(8), initialized);
                initialized[291] = state[434];
                assert_eq!(at(9), initialized);
                assert_eq!(at(10), expected);
            }
            if batch == 0 && fill == 0x5a {
                baseline = Some(Baseline {
                    request: executed.request,
                    identity: executed.identity,
                });
            }
        }
    }
    Ok(baseline.expect("first storage batch"))
}

fn storage_negative(g: &mut Gain, baseline: &Baseline) -> Result<()> {
    let memcpy = g.sym(1, "memcpy");
    let mut rows: Vec<_> = baseline.request.cases[..8]
        .iter()
        .map(|source| {
            let memory = !source.vendor.observe_memory.is_empty();
            case(
                source.name.clone(),
                source.vendor.clone(),
                Some(source.vendor.clone()),
                source.reset,
                memory,
            )
        })
        .collect();
    let mutate = |value: u8| -> Result<_> {
        Ok(g.enter(
            memcpy,
            &[CACHE + 12 + 434, INPUT, 1],
            vec![known(INPUT, 1, &[value])?],
            vec![selection(CACHE + 12 + 434, 1)],
            vec![],
        ))
    };
    // The baseline adjustment is zero; both sides overwrite it with distinct values.
    let seeded = baseline.request.cases[0]
        .vendor
        .memory
        .iter()
        .find(|m| m.seed.address == INPUT)
        .expect("seed input");
    assert_eq!(seeded.seed.bytes[434], 0);
    rows.insert(
        2,
        case(
            "corrupt-saved-adjustment",
            mutate(1)?,
            Some(mutate(31)?),
            SessionReset::Warm,
            true,
        ),
    );
    let records = g
        .execute(
            "storage-corruption-propagates",
            rows,
            0x5a,
            Right::Vendor,
            ComparisonVerdict::Diff,
            MAX_EVENTS,
        )?
        .records;
    assert_eq!(output(&records, 2, false), [1]);
    assert_eq!(output(&records, 2, true), [31]);
    assert_eq!(output(&records, 4, false)[434], 1);
    assert_eq!(output(&records, 4, true)[434], 31);
    assert_eq!(
        output(&records, 8, false),
        arithmetic(&g.coefficients[108..], &[0; 6], 1, 0, Some(13))
    );
    assert_eq!(
        output(&records, 8, true),
        arithmetic(&g.coefficients[108..], &[0; 6], 31, 0, Some(13))
    );
    assert!(!has_events(&records));
    // This documents that the vendor copy itself does not validate corruption;
    // no production storage or checksum guarantee is introduced.
    let recovery = g.enter(
        g.root("phy_rf_cal_data_recovery_new"),
        &[CACHE],
        vec![region(CACHE, 532, &[], None, RegionLifetime::Phase)?],
        state_selection(g),
        vec![],
    );
    let rows = vec![
        case(
            "seed-live-state",
            g.setup(&[0; PHY_PARAM_BYTES as usize], false),
            None,
            SessionReset::Cold,
            false,
        ),
        case("unknown-cache", recovery, None, SessionReset::Warm, false),
    ];
    let records = g
        .execute(
            "storage-unknown-cache",
            rows,
            0x5a,
            Right::None,
            ComparisonVerdict::Incomplete,
            MAX_EVENTS,
        )?
        .records;
    assert!(!g.last_manifest_complete());
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::Memory { .. },
            ..
        }
    ));
    assert!(!has_events(&records));
    capacity(g, baseline)
}

fn capacity(g: &mut Gain, baseline: &Baseline) -> Result<()> {
    let before = g
        .runner
        .execution("storage-before-failure", &baseline.identity)?;
    let mut limited = baseline.request.clone();
    limited.cases.truncate(2);
    limited.cases[1].vendor.observe_calls = Some(CallCapture {
        include_tail: true,
        argument_words: 3,
        overrides: vec![],
    });
    limited.max_events = 1;
    g.capacity_failure("storage-capacity", &limited)?;
    g.assert_retained("storage-after-failure", &baseline.identity, &before)
}

fn producer(g: &mut Gain) -> Result<ExecutionRequest> {
    let tab = g.root("phy_wifi_get_tx_tab_new");
    let set_gain = g.root("phy_wifi_set_tx_gain_new");
    let mac = g.root("mac_power_set");
    let mut publishing = None;
    for fill in [0x5a, 0xa5] {
        for batch in (0..POWER.len()).step_by(POWER.len()) {
            let mut rows = vec![];
            for &(target, attenuation, _, _, _) in &POWER[batch..] {
                let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                data[80..98].fill((target & 255) as u8);
                data[6] = 84;
                data[8] = (attenuation & 255) as u8;
                data[284] = 13;
                data[434] = 0x5a;
                rows.push(case(
                    "seed-power-inputs",
                    g.setup(&data, false),
                    None,
                    SessionReset::Cold,
                    false,
                ));
                let install = g.enter(
                    g.root("phy_get_romfunc_addr"),
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
                    vec![selection(0x2f07_f968, 4)],
                    vec![],
                );
                rows.push(case(
                    "install-current-callbacks",
                    install,
                    None,
                    SessionReset::Warm,
                    false,
                ));
                let mut models = gain_models(0, 0);
                if let DeviceBehavior::ConstantRead { value, .. } = &mut models[0].behavior {
                    *value = 0;
                }
                models.push(DeviceDeclaration {
                    id: "test-mac-power".into(),
                    applicability: "explicit retained test MAC word; no power selection algorithm"
                        .into(),
                    lifetime: RegionLifetime::Phase,
                    behavior: DeviceBehavior::RegisterBank {
                        cells: vec![RegisterCell {
                            address: MAC_POWER,
                            width: 4,
                            value: 0xa5a5_a5a5,
                        }],
                    },
                });
                let mut phase = g.enter(
                    g.root("set_rate_power_index"),
                    &[0],
                    vec![],
                    vec![selection(g.parameter + 434, 1)],
                    models,
                );
                phase.observe_calls = Some(CallCapture {
                    include_tail: true,
                    argument_words: 0,
                    overrides: vec![
                        CallWordCount {
                            target: set_gain,
                            words: 2,
                        },
                        CallWordCount {
                            target: mac,
                            words: 1,
                        },
                    ],
                });
                rows.push(case(
                    format!("power-{target}-{attenuation}"),
                    phase,
                    None,
                    SessionReset::Warm,
                    false,
                ));
            }
            let executed = g.characterize_vendor(&format!("power-{fill}-{batch}"), rows, fill)?;
            let records = &executed.records;
            for (i, &(target, attenuation, adjustment, index, publish)) in
                POWER[batch..].iter().enumerate()
            {
                let i = i as u32;
                assert_eq!(output(records, 3 * i + 1, false), tab.to_le_bytes());
                assert!(
                    events(records, 3 * i, false).is_empty()
                        && events(records, 3 * i + 1, false).is_empty()
                );
                assert_eq!(output(records, 3 * i + 2, false), [adjustment]);
                let observed = events(records, 3 * i + 2, false);
                let expected_gain: Vec<Vec<u32>> = if publish { vec![vec![13, 0]] } else { vec![] };
                assert_eq!(calls(&observed, set_gain), expected_gain);
                assert_eq!(calls(&observed, mac), [[index as u32]]);
                let mut expected = vec![];
                if publish {
                    let calculated = arithmetic(
                        &g.coefficients[108..],
                        &[0; 6],
                        i32::from(adjustment),
                        0,
                        Some(13),
                    );
                    expected = publication(&[0; 6], 0, &calculated, 32, 0, 0, false);
                    expected[0] = (Access::Read, 0x2010_0408, 0);
                }
                expected.extend(mac_power(index, 0xa5a5_a5a5));
                assert_eq!(
                    effects(&observed, true),
                    expected,
                    "{target} {attenuation} {fill}"
                );
            }
            if fill == 0x5a {
                publishing = Some(executed.request);
            }
        }
    }
    Ok(publishing.expect("publishing batch"))
}

/// Rows of the first publishing policy case (`POWER[9]`, 80/-5) in the 0x5a request.
const PUBLISHING: usize = 3 * 9;

fn power_negative(g: &mut Gain, request: &ExecutionRequest) -> Result<()> {
    let set_gain = g.root("phy_wifi_set_tx_gain_new");
    let mac = g.root("mac_power_set");
    let mut rows = request.cases[PUBLISHING..PUBLISHING + 3].to_vec();
    rows.remove(1);
    // Supply only the parameter pointer. The absent callback installation must
    // stop gain regeneration even though the parent already wrote adjustment.
    rows[1].vendor.memory.push(region(
        ROM_PARAMETER_POINTER,
        4,
        &words(&[g.parameter]),
        None,
        RegionLifetime::Phase,
    )?);
    let records = g
        .execute(
            "power-missing-callback",
            rows,
            0x5a,
            Right::None,
            ComparisonVerdict::Incomplete,
            MAX_EVENTS,
        )?
        .records;
    assert!(!g.last_manifest_complete());
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete { .. }
    ));
    assert_eq!(output(&records, 1, false), [1]);
    let observed = events(&records, 1, false);
    assert_eq!(calls(&observed, set_gain), [[13, 0]]);
    assert!(calls(&observed, mac).is_empty());
    assert!(effects(&observed, true).is_empty());

    // Leave the attenuation byte and later policy inputs unknown. Loading an
    // unknown byte stops the seed phase, so the policy phase cannot select a
    // gain or MAC index from partially known parameters.
    let mut rows = request.cases[PUBLISHING..PUBLISHING + 3].to_vec();
    let source = rows[0]
        .vendor
        .memory
        .iter_mut()
        .find(|m| m.seed.address == INPUT)
        .expect("seed input");
    source.seed.bytes.truncate(8);
    source.seed.fill = None;
    let records = g
        .execute(
            "power-unknown-attenuation",
            rows,
            0x5a,
            Right::None,
            ComparisonVerdict::Incomplete,
            MAX_EVENTS,
        )?
        .records;
    assert!(!g.last_manifest_complete());
    assert!(matches!(
        stop(&records, 0, false),
        ExecutionStop::Incomplete { reason: ExecutionGap::Memory { address, access: MemoryAccess::Read }, .. } if address == INPUT + 8
    ));
    assert_eq!(stop(&records, 1, false), ExecutionStop::BlockedByPriorPhase);
    assert_eq!(stop(&records, 2, false), ExecutionStop::BlockedByPriorPhase);
    assert!(events(&records, 1, false).is_empty() && events(&records, 2, false).is_empty());
    Ok(())
}

/// Callbacks executed by [`exercise`]; host tests substitute them.
pub trait Steps {
    fn storage(&mut self) -> Result<()>;
    fn producer(&mut self) -> Result<()>;
}

impl Steps for Gain {
    fn storage(&mut self) -> Result<()> {
        let baseline = storage(self)?;
        storage_negative(self, &baseline)
    }
    fn producer(&mut self) -> Result<()> {
        let request = producer(self)?;
        power_negative(self, &request)
    }
}

/// Run every available 12.5 case; return obligations that could not execute.
pub fn exercise(steps: &mut impl Steps, rftest: bool) -> Result<Vec<Unmet>> {
    steps.storage()?;
    if !rftest {
        return Ok(vec![RFTEST_OBLIGATION]);
    }
    steps.producer()?;
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_power_updates_both_index_fields_and_preserves_other_bits() {
        assert_eq!(
            mac_power(21, 0xa5a5_a5a5),
            [
                (Access::Read, MAC_POWER, 0xa5a5_a5a5),
                (Access::Write, MAC_POWER, 0xa5a5_a595),
                (Access::Read, MAC_POWER, 0xa5a5_a595),
                (Access::Write, MAC_POWER, 0xa5a5_9595),
            ]
        );
        // Signed policy results are truncated to the six-bit fields.
        assert_eq!(mac_power(-12, 0)[3], (Access::Write, MAC_POWER, 0x3434));
    }

    struct Recorder(Vec<&'static str>);
    impl Steps for Recorder {
        fn storage(&mut self) -> Result<()> {
            self.0.push("storage");
            Ok(())
        }
        fn producer(&mut self) -> Result<()> {
            self.0.push("producer");
            Ok(())
        }
    }

    #[test]
    fn missing_rftest_is_an_unmet_obligation_after_storage() {
        let mut steps = Recorder(vec![]);
        assert_eq!(exercise(&mut steps, false).unwrap(), [RFTEST_OBLIGATION]);
        assert_eq!(steps.0, ["storage"]);
        let mut steps = Recorder(vec![]);
        assert!(exercise(&mut steps, true).unwrap().is_empty());
        assert_eq!(steps.0, ["storage", "producer"]);
    }

    #[test]
    fn power_policy_table_covers_every_publication_branch() {
        assert!(POWER.iter().any(|p| p.4) && POWER.iter().any(|p| !p.4));
        assert!(POWER.iter().any(|p| p.3 < 0), "signed MAC index wrap");
    }
}
