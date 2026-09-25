//! Captured RFPLL search and frequency maintenance against compiled production.
use crate::calibration_prefix::{delay_calls, delays_of};
use crate::evidence::{events, stop};
use crate::harness::{Result, case, invocation, known, region, words, words_padded};
use crate::i2c::{I2c, all_complete, models, returned_low};
use crate::layout::*;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceBehavior, DeviceDeclaration, ExecutionCase,
    ExecutionEvent, ExecutionGap, ExecutionRegion, ExecutionStop, Invocation, MemoryAccess,
    ModelStatus, ReadRun, RegionLifetime, RegisterCell, SessionReset,
};

/// Independently read candidate domains and signed outputs of the pinned archive.
/// These finite expectations are never an execution model or replacement body:
/// name, initial capacitor, lock statuses, candidates and selected capacitor.
/// Name, initial capacitor, lock statuses, candidates and selected capacitor.
pub type Search = (&'static str, i32, Vec<u32>, Vec<i32>, i32);

pub fn search_cases() -> Vec<Search> {
    let around = |c: i32| {
        (0..10)
            .map(|i| c - i)
            .chain((1..=10).map(|i| c + i))
            .collect::<Vec<_>>()
    };
    vec![
        ("all-accepted", 100, vec![0; 20], around(100), 100),
        ("lowest-initial-cap", 0, vec![0; 20], around(0), 0),
        ("highest-initial-cap", 511, vec![0; 20], around(511), 511),
        ("no-accepted", 100, vec![3; 20], around(100), 100),
        (
            "positive",
            100,
            [vec![1; 2], vec![0; 10]].concat(),
            [vec![100, 99], (101..111).collect()].concat(),
            105,
        ),
        (
            "negative",
            300,
            [vec![0; 10], vec![2; 2]].concat(),
            [(291..=300).rev().collect(), vec![301, 302]].concat(),
            295,
        ),
        (
            "nonconsecutive",
            100,
            vec![1, 0, 1, 2, 3, 2],
            vec![100, 99, 98, 101, 102, 103],
            99,
        ),
        (
            "signed-wrap",
            0,
            vec![1, 0, 1, 2, 2],
            vec![0, -1, -2, 1, 2],
            -1,
        ),
        (
            "opposite-boundaries",
            100,
            [vec![2; 10], vec![1; 10]].concat(),
            around(100),
            100,
        ),
    ]
}

/// Expected command-port writes of one search.
pub fn expected_commands(candidates: &[i32], selected: i32) -> Vec<(u32, u32)> {
    let mut result = vec![0x0400_0562u32, 0x0400_0762, 0x0400_0b62, 0x0555_0b62];
    let mut high = 0x95u32;
    for (i, candidate) in candidates.iter().chain([&selected]).enumerate() {
        let programmed = (*candidate).max(0) as u32;
        high = ((high & !0x40) | ((programmed >> 8) << 6)) & 255;
        result.extend([
            0x0500_0162 | ((programmed & 255) << 16),
            0x0400_0262,
            0x0500_0262 | (high << 16),
        ]);
        if i < candidates.len() {
            result.push(0x0400_0c62);
        }
    }
    result.into_iter().map(|v| (I2C_PORT_1, v)).collect()
}

/// Retained frequency-control words and the ordered transactions over them.
struct Frequency {
    regs: std::collections::BTreeMap<u32, u32>,
    result: Vec<(bool, u32, u32)>,
}

impl Frequency {
    fn get(&self, offset: u32) -> u32 {
        self.regs[&offset]
    }
    fn read(&mut self, offset: u32) {
        let value = self.get(offset);
        self.result.push((false, 0x2010_0000 + offset, value));
    }
    fn write(&mut self, offset: u32, value: u32) {
        self.regs.insert(offset, value);
        self.result.push((true, 0x2010_0000 + offset, value));
    }
    fn update(&mut self, offset: u32, f: impl FnOnce(u32) -> u32) {
        let value = f(self.get(offset));
        self.write(offset, value);
    }
}

/// Instruction-derived retained-word transaction oracle. It does not feed
/// execution: the device receives only finite input words. Entries are
/// (write, address, value).
pub fn frequency_expectation(
    initial: u32,
    contents: Option<&[u32]>,
    delta: i32,
    channel: u32,
) -> Vec<(bool, u32, u32)> {
    let mut f = Frequency {
        regs: std::collections::BTreeMap::from([
            (0x1c, 0x4128_0055),
            (0x20, 0xa5a4_5678),
            (0x28, initial),
            (0x30, 0x1234_5678),
        ]),
        result: vec![],
    };
    f.read(0x28);
    f.write(0x28, (initial & !3) | 2);
    f.result.push((false, 0x2010_d800, 0x9876_5432));
    f.read(0x30);
    if let Some(contents) = contents {
        for (i, word) in contents.iter().enumerate() {
            let address = 0x20 + 7 * i as u32;
            f.read(0x1c);
            f.update(0x1c, |v| (v & !0x7ff00) | (address << 8));
            f.read(0x30);
            f.update(0x30, |v| (v & !3) | 2);
            f.read(0x20);
            f.update(0x20, |v| v | 0x10000);
            f.read(0x20);
            f.update(0x20, |v| v & !0x10000);
            f.result.push((false, 0x2010_0040, *word));
            let corrected = (((word & 255) | ((word >> 6) & 0x100)) as i32 + delta) & 0xffff;
            let signed = i32::from(corrected as u16 as i16);
            let encoded = (corrected as u32 & 255)
                | (word & 0xbf00)
                | (((signed >> 8) as u32) << 14)
                | 0x0300_0000;
            f.read(0x1c);
            f.update(0x1c, |v| (v & !0x7ff00) | (address << 8));
            f.write(0x2c, encoded);
            f.read(0x1c);
            f.update(0x1c, |v| v | 0x10_0000);
            f.read(0x1c);
            f.update(0x1c, |v| v & !0x10_0000);
        }
        let frequency = match channel {
            1 | 2412 => 2412,
            13 => 2472,
            14 | 2484 => 2484,
            other => panic!("unsupported channel {other}"),
        };
        f.read(0x1c);
        f.update(0x1c, |v| (v & !255) | ((frequency - 0x60) & 255));
    }
    f.read(0x28);
    f.update(0x28, |v| v & !3);
    f.result
}

/// Name, lock statuses, signed correction, initial control word, frequency
/// memory contents and channel of one maintenance case.
type Maintenance = (String, Vec<u32>, i32, u32, Option<Vec<u32>>, u32);

struct Rfpll {
    shim: u32,
    parameter: u32,
    memcpy: u32,
    rom_delay: u32,
    production_delay: u32,
    search_entry: u32,
    maintain_entry: u32,
    rom_search: u32,
    rom_track: u32,
    callbacks: Vec<u32>,
}

fn bank(cap: i32, statuses: &[u32], busy: u32) -> Vec<DeviceDeclaration> {
    let cap = cap as u32;
    let mut cells: Vec<CommandCell> = [
        (1u32, cap & 255),
        (2, 0x95),
        (5, cap & 255),
        (7, 0xc2 | ((cap >> 8) << 2)),
        (11, 0x15),
    ]
    .map(|(r, v)| CommandCell {
        selector: (r << 8) | 0x62,
        initial: v,
        reads: None,
    })
    .to_vec();
    cells.push(CommandCell {
        selector: 0x0c62,
        initial: 0,
        reads: Some(statuses.iter().map(|s| 0xa3 | (s << 2)).collect()),
    });
    vec![
        analog_bank(
            "rfpll",
            "explicit capacitor bytes and finite lock-status samples; no search algorithm",
            busy,
            [0, 0],
            cells,
        ),
        DeviceDeclaration {
            id: "transport-controls".into(),
            applicability: "explicit retained host-map/read-mask controls".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: vec![
                    RegisterCell {
                        address: I2C_READ_MASK,
                        width: 4,
                        value: 0,
                    },
                    RegisterCell {
                        address: I2C_HOST_MAP,
                        width: 4,
                        value: 0,
                    },
                ],
            },
        },
    ]
}

fn sequence(address: u32, values: &[u32], name: &str) -> DeviceDeclaration {
    DeviceDeclaration {
        id: name.into(),
        applicability: "finite caller-supplied peripheral observations".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::SequenceRead {
            address,
            width: 4,
            runs: values.iter().map(|v| ReadRun::once(*v)).collect(),
        },
    }
}

fn maintenance_models(
    statuses: &[u32],
    initial: u32,
    contents: Option<&[u32]>,
    busy: u32,
) -> Vec<DeviceDeclaration> {
    let mut result = bank(100, statuses, busy);
    let mut cells = vec![(0x2010_0028u32, initial)];
    if let Some(contents) = contents {
        cells.extend([
            (0x2010_001c, 0x4128_0055),
            (0x2010_0020, 0xa5a4_5678),
            (0x2010_002c, 0),
            (0x2010_0030, 0x1234_5678),
        ]);
        result.push(sequence(0x2010_0040, contents, "frequency-memory"));
    } else {
        result.push(sequence(0x2010_0030, &[0x1234_5678], "i2c-number"));
    }
    cells.sort();
    result.push(DeviceDeclaration {
        id: "frequency-control".into(),
        applicability: "explicit retained frequency control words".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: cells
                .into_iter()
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .collect(),
        },
    });
    result.push(sequence(0x2010_d800, &[0x9876_5432], "sdm"));
    result
}

impl Rfpll {
    fn enter(
        &self,
        target: u32,
        arguments: &[u32],
        models: Vec<DeviceDeclaration>,
        memory: Vec<ExecutionRegion>,
    ) -> Result<Invocation> {
        let mut regions = vec![
            known(ABI_WORDS, 32, &words_padded(arguments, 8, 0)?)?,
            known(ROM_INTERFACE_POINTER, 4, &words(&[CALLBACK_TABLE]))?,
            known(CALLBACK_TABLE, 16, &words(&self.callbacks))?,
        ];
        regions.extend(memory);
        Ok(invocation(
            self.shim,
            vec![Some(target), Some(ABI_WORDS)],
            regions,
            models,
            vec![],
        ))
    }

    fn delay(&self, side: bool) -> u32 {
        if side {
            self.production_delay
        } else {
            self.rom_delay
        }
    }

    fn search(&self, side: bool, cap: i32, statuses: &[u32], busy: u32) -> Result<Invocation> {
        let mut phase = self.enter(
            if side {
                self.search_entry
            } else {
                self.rom_search
            },
            &[],
            bank(cap, statuses, busy),
            vec![],
        )?;
        phase.calls = delay_calls("requested-delay", self.delay(side), statuses.len());
        Ok(phase)
    }

    fn setup(&self, side: bool) -> Result<Invocation> {
        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
        data[..2].copy_from_slice(&[100, 0]);
        let target = if side {
            PARAMETER_DESTINATION
        } else {
            self.parameter
        };
        let mut memory = vec![known(PARAMETER_SOURCE, PHY_PARAM_BYTES, &data)?];
        if side {
            memory.push(region(
                target,
                PHY_PARAM_BYTES,
                &[],
                None,
                RegionLifetime::Session,
            )?);
        }
        self.enter(
            self.memcpy,
            &[target, PARAMETER_SOURCE, PHY_PARAM_BYTES],
            vec![],
            memory,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn maintain(
        &self,
        side: bool,
        statuses: &[u32],
        initial: u32,
        contents: Option<&[u32]>,
        channel: u32,
        busy: u32,
        diagnostics: u32,
    ) -> Result<Invocation> {
        let mut table = vec![0u8; 284];
        table.extend((channel as u16).to_le_bytes());
        let memory = vec![
            known(ROM_PARAMETER_POINTER, 4, &words(&[0x3fff_0000]))?,
            known(0x3fff_0000, 286, &table)?,
        ];
        let (target, arguments) = if side {
            (self.maintain_entry, [channel])
        } else {
            (self.rom_track, [diagnostics])
        };
        let mut phase = self.enter(
            target,
            &arguments,
            maintenance_models(statuses, initial, contents, busy),
            memory,
        )?;
        phase.calls = delay_calls("requested-delay", self.delay(side), statuses.len() + 1);
        Ok(phase)
    }
}

/// Frequency-control envelope: every read/write/fence/delay except transport
/// port reads and the two transport control writes.
fn envelope(events: &[ExecutionEvent]) -> Vec<ExecutionEvent> {
    events
        .iter()
        .filter(|e| match e {
            ExecutionEvent::Read { address, .. } => !(I2C_PORT_0..0x2010_f824).contains(address),
            ExecutionEvent::Write { address, .. } => {
                !matches!(*address, I2C_READ_MASK | I2C_HOST_MAP)
            }
            ExecutionEvent::Fence { .. } | ExecutionEvent::DelayMicros { .. } => true,
            _ => false,
        })
        .cloned()
        .collect()
}

fn in_frequency_domain(address: u32) -> bool {
    (0x2010_0000..0x2010_0044).contains(&address) || address == 0x2010_d800
}

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let rfpll = Rfpll {
        shim: ctx.probe("open_phy_trace_i2c_entry"),
        parameter: ctx.parameter,
        memcpy: ctx.captured(1, "memcpy"),
        rom_delay: ctx.captured(1, "ets_delay_us"),
        production_delay: ctx.probe("open_phy_trace_delay_event"),
        search_entry: ctx.probe("open_phy_rfpll_trace_search"),
        maintain_entry: ctx.probe("open_phy_rfpll_trace_maintain"),
        rom_search: ctx.root("phy_rfpll_cap_init_cal_new"),
        rom_track: ctx.root("phy_rfpll_cap_track_new"),
        callbacks: [
            "phy_i2c_enter_critical",
            "phy_i2c_exit_critical",
            "phy_get_i2c_read_mask_new",
            "phy_get_i2c_hostid_new",
        ]
        .map(|n| ctx.root(n))
        .to_vec(),
    };
    let (vendor, replacement) = (ctx.vendor.clone(), ctx.replacement.clone());
    for (name, cap, statuses, candidates, selected) in search_cases() {
        let mut baseline = None;
        for fill in [0x5au8, 0xa5] {
            for busy in [0u32, 1] {
                let label = format!("rfpll-search-{name}-{fill}-{busy}");
                let mut row = case(
                    label.clone(),
                    rfpll.search(false, cap, &statuses, busy)?,
                    Some(rfpll.search(true, cap, &statuses, busy)?),
                    SessionReset::Cold,
                    false,
                );
                let relation = row.relation.as_mut().unwrap();
                relation.returns.low = true;
                relation.events.mmio_read = false;
                let records = ctx.submit_with(
                    &label,
                    &vendor,
                    Some(&replacement),
                    Some(fill),
                    vec![row],
                    MAX_EVENTS,
                    Some(ComparisonVerdict::Match),
                )?;
                for side in [false, true] {
                    let low = returned_low(&records, 0, side);
                    assert_eq!(low, Some((selected - cap) as u32), "{label} {side}");
                    let observed = events(&records, 0, side);
                    let commands: Vec<_> = observed
                        .iter()
                        .filter_map(|e| match e {
                            ExecutionEvent::Write {
                                address: address @ (I2C_PORT_0 | I2C_PORT_1),
                                value,
                                ..
                            } => Some((*address, *value)),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        commands,
                        expected_commands(&candidates, selected),
                        "{label} {side}"
                    );
                    let waits = delays_of(&observed);
                    assert_eq!(waits, vec![5; statuses.len()], "{label} {side}");
                    let facts = (low, commands, waits);
                    assert!(
                        baseline.as_ref().is_none_or(|b| *b == facts),
                        "{label} {side}"
                    );
                    baseline = Some(facts);
                    assert!(all_complete(&records, 0, side), "{label} {side}");
                }
            }
        }
    }
    let mut maintenance: Vec<Maintenance> = vec![];
    for status in [0u32, 3] {
        for initial in [0x2582_4e58u32, 0xa5a5_5a5b] {
            maintenance.push((
                format!("zero-{status}-{initial:x}"),
                vec![status; 20],
                0,
                initial,
                None,
                13,
            ));
        }
    }
    for (name, statuses, delta, boundary) in [
        ("positive", [vec![1; 2], vec![0; 10]].concat(), 5, None),
        ("negative", [vec![0; 10], vec![2; 2]].concat(), -5, None),
        (
            "underflow",
            [vec![0; 10], vec![2; 2]].concat(),
            -5,
            Some(0x00aa_bf00u32),
        ),
        (
            "overflow",
            [vec![1; 2], vec![0; 10]].concat(),
            5,
            Some(0x00aa_ffff),
        ),
    ] {
        let contents: Vec<u32> = (0..85u32)
            .map(|i| boundary.unwrap_or(0x0055_0000 | (i << 8) | (100 + i)))
            .collect();
        for channel in [1, 13, 14, 2412, 2484] {
            maintenance.push((
                name.into(),
                statuses.clone(),
                delta,
                0x2582_4e58,
                Some(contents.clone()),
                channel,
            ));
        }
    }
    for (name, statuses, delta, initial, contents, channel) in &maintenance {
        for fill in [0x5au8, 0xa5] {
            let label = format!("rfpll-maintain-{name}-{channel}-{fill}");
            let mut rows = vec![
                case(
                    "initialize-parameters",
                    rfpll.setup(false)?,
                    Some(rfpll.setup(true)?),
                    SessionReset::Cold,
                    false,
                ),
                case(
                    label.clone(),
                    rfpll.maintain(
                        false,
                        statuses,
                        *initial,
                        contents.as_deref(),
                        *channel,
                        0,
                        0,
                    )?,
                    Some(rfpll.maintain(
                        true,
                        statuses,
                        *initial,
                        contents.as_deref(),
                        *channel,
                        0,
                        0,
                    )?),
                    SessionReset::Warm,
                    false,
                ),
            ];
            rows[1].relation.as_mut().unwrap().events.mmio_read = false;
            let records = ctx.submit_with(
                &label,
                &vendor,
                Some(&replacement),
                Some(fill),
                rows,
                MAX_EVENTS,
                Some(ComparisonVerdict::Match),
            )?;
            let mut projected_sides = vec![];
            for side in [false, true] {
                let low = returned_low(&records, 1, side);
                if side {
                    assert_eq!(low, Some(*delta as u32), "{label}");
                }
                let observed = events(&records, 1, side);
                let mut expected_waits = vec![2];
                expected_waits.extend(vec![5; statuses.len()]);
                assert_eq!(delays_of(&observed), expected_waits, "{label}");
                let mut projected = envelope(&observed);
                if !side && contents.is_some() {
                    let indices: Vec<_> = projected
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            matches!(
                                e,
                                ExecutionEvent::Read {
                                    address: 0x2010_0028,
                                    ..
                                }
                            )
                        })
                        .map(|(i, _)| i)
                        .collect();
                    assert_eq!(indices.len(), 3, "{label}");
                    let removed = projected.remove(indices[1]);
                    assert!(
                        matches!(
                            removed,
                            ExecutionEvent::Read {
                                value: 0x2582_4e5a,
                                ..
                            }
                        ),
                        "{label}"
                    );
                }
                let frequency: Vec<_> = projected
                    .iter()
                    .filter_map(|e| match e {
                        ExecutionEvent::Read { address, value, .. }
                            if in_frequency_domain(*address) =>
                        {
                            Some((false, *address, *value))
                        }
                        ExecutionEvent::Write { address, value, .. }
                            if in_frequency_domain(*address) =>
                        {
                            Some((true, *address, *value))
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    frequency,
                    frequency_expectation(*initial, contents.as_deref(), *delta, *channel),
                    "{label} {side}"
                );
                assert!(all_complete(&records, 1, side), "{label} {side}");
                projected_sides.push(projected);
            }
            assert_eq!(
                projected_sides[0], projected_sides[1],
                "{label}: frequency envelope differs"
            );
        }
    }
    let label = "rfpll-maintenance-timeout";
    let mut failed = rfpll.maintain(true, &[], 0x2582_4e58, None, 13, 65535, 0)?;
    // No status sample is expected while the first command remains pending.
    if let DeviceBehavior::CommandBank(bank) = &mut failed.models[0].behavior {
        bank.cells.retain(|c| c.reads.is_none());
    }
    let mut row = case(label, failed, None, SessionReset::Cold, false);
    row.relation = None;
    let records = ctx.submit_with(
        label,
        &replacement,
        None,
        Some(0x5a),
        vec![row],
        MAX_EVENTS,
        None,
    )?;
    assert_eq!(returned_low(&records, 0, false), Some(0x8000_0000));
    let frequency_writes: Vec<_> = events(&records, 0, false)
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::Write { address, value, .. }
                if (0x2010_0000..0x2010_0044).contains(address) =>
            {
                Some((*address, *value))
            }
            _ => None,
        })
        .collect();
    assert_eq!(frequency_writes, [(0x2010_0028, 0x2582_4e5a)]);
    let pending = models(&records, 0, false)
        .into_iter()
        .find(|m| m.id == "rfpll")
        .expect("rfpll model");
    assert!(
        pending.commands.is_some_and(|c| c.pending == 1)
            && pending.status == ModelStatus::Incomplete
    );
    let original: Vec<ExecutionCase> = {
        let mut row = case(
            "rfpll-search",
            rfpll.search(false, 100, &[0; 20], 0)?,
            Some(rfpll.search(true, 100, &[0; 20], 0)?),
            SessionReset::Cold,
            false,
        );
        let relation = row.relation.as_mut().unwrap();
        relation.returns.low = true;
        relation.events.mmio_read = false;
        vec![row]
    };
    let mut changed = original.clone();
    if let DeviceBehavior::CommandBank(bank) =
        &mut changed[0].replacement.as_mut().unwrap().models[0].behavior
    {
        bank.cells
            .iter_mut()
            .find(|c| c.selector == 0x0562)
            .unwrap()
            .initial = 101;
    }
    ctx.submit_with(
        "rfpll-changed-capacitor",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        changed,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    let mut unknown = original.clone();
    unknown[0].vendor.memory[1].seed.bytes.clear();
    let records = ctx.submit_with(
        "rfpll-unknown-callbacks",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        unknown,
        MAX_EVENTS,
        Some(ComparisonVerdict::Incomplete),
    )?;
    assert!(matches!(
        stop(&records, 0, false),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::Memory {
                address: ROM_INTERFACE_POINTER,
                access: MemoryAccess::Read
            },
            ..
        }
    ));
    let mut diagnostics = vec![
        case(
            "initialize-parameters",
            rfpll.setup(false)?,
            Some(rfpll.setup(true)?),
            SessionReset::Cold,
            false,
        ),
        case(
            "diagnostics-unmapped",
            rfpll.maintain(false, &[0; 20], 0x2582_4e58, None, 13, 0, 1)?,
            Some(rfpll.maintain(true, &[0; 20], 0x2582_4e58, None, 13, 0, 0)?),
            SessionReset::Warm,
            false,
        ),
    ];
    diagnostics[1].relation.as_mut().unwrap().events.mmio_read = false;
    let records = ctx.submit_with(
        "rfpll-diagnostics-unmapped",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        diagnostics,
        MAX_EVENTS,
        Some(ComparisonVerdict::Incomplete),
    )?;
    let printf = ctx.captured(4, "phy_printf");
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete { reason: ExecutionGap::Memory { address, access: MemoryAccess::Fetch }, .. } if address == printf
    ));
    let mut raw = original.clone();
    raw[0].relation.as_mut().unwrap().events.mmio_read = true;
    ctx.submit_with(
        "rfpll-raw-polling",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        raw,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    let limited = crate::session::request(&vendor, Some(&replacement), Some(0x5a), original, 1);
    ctx.capacity_failure("rfpll-capacity", &limited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_oracle_programs_every_candidate_then_the_selection() {
        let commands = expected_commands(&[100, 99], 99);
        // Four setup commands, then three per candidate plus a status read, then the selection.
        assert_eq!(commands.len(), 4 + 2 * 4 + 3);
        assert_eq!(commands[4], (I2C_PORT_1, 0x0500_0162 | (100 << 16)));
        // Negative candidates program zero.
        assert_eq!(expected_commands(&[-2], 0)[4], (I2C_PORT_1, 0x0500_0162));
        let cases = search_cases();
        assert_eq!(cases.len(), 9);
        assert_eq!(
            cases[5].3,
            [300, 299, 298, 297, 296, 295, 294, 293, 292, 291, 301, 302]
        );
    }

    #[test]
    fn frequency_oracle_encodes_signed_corrections() {
        let zero = frequency_expectation(0x2582_4e58, None, 0, 13);
        assert_eq!(zero.len(), 6);
        assert_eq!(zero[1], (true, 0x2010_0028, 0x2582_4e5a));
        let underflow = frequency_expectation(0x2582_4e58, Some(&[0x00aa_bf00]), -5, 1);
        let encoded = underflow
            .iter()
            .find(|(write, address, _)| *write && *address == 0x2010_002c)
            .unwrap()
            .2;
        // Word bit 14 would be the ninth capacitor bit; here it is clear, so
        // 0 - 5 wraps to 0xfffb: low byte 0xfb, retained 0xbf00 and the sign field.
        assert_eq!(encoded, 0xffff_fffb);
        let ninth = frequency_expectation(0x2582_4e58, Some(&[0x0000_4000]), -5, 1);
        let encoded = ninth
            .iter()
            .find(|(write, address, _)| *write && *address == 0x2010_002c)
            .unwrap()
            .2;
        // 0x100 - 5 = 0xfb stays positive: no sign field.
        assert_eq!(encoded, 0x0300_00fb);
    }
}
