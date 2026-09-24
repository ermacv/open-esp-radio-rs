//! Transport cases over captured ROM callbacks and compiled PHY transactions.
//!
//! Scenario expectations are independent of either execution result. The
//! peripheral assumption supplies responses only; ROM/PAC/PHY code performs
//! every transaction.
use crate::evidence::stop;
use crate::harness::{Result, case, invocation, known, words, words_padded};
use crate::i2c::{I2c, models, returned_low, word_writes};
use blobray_domain::{
    CommandBank, CommandCell, CommandPort, ComparisonVerdict, DeviceBehavior, DeviceDeclaration,
    DeviceIssue, ExecutionCase, ExecutionEvent, ExecutionEvidence, ExecutionRegion, ExecutionStop,
    Invocation, ModelStatus, RegionLifetime, RegisterCell, SessionReset,
};

/// Independent `.iram1` +0x44..+0x60 instruction reading.
const CONFIGURATION: u32 = 0x1237_fa08;

pub fn controls() -> DeviceDeclaration {
    DeviceDeclaration {
        id: "controls".into(),
        applicability: "captured complemented read mask and host-map RMW".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![
                RegisterCell {
                    address: 0x2010_f81c,
                    width: 4,
                    value: 0,
                },
                RegisterCell {
                    address: 0x2010_f820,
                    width: 4,
                    value: 0x1234_5678,
                },
            ],
        },
    }
}

/// Shared analog bytes; samples at issue, writes at ready observation.
pub fn bank(
    selector: Option<u32>,
    sample: u32,
    scripted: bool,
    busy: u32,
    initial: [u32; 2],
) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "analog".into(),
        applicability: "shared analog bytes; samples at issue, writes at ready observation".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::CommandBank(CommandBank {
            selector_mask: 0xffff,
            data_mask: 0x00ff_0000,
            read_command: 0x0400_0000,
            write_command: 0x0500_0000,
            busy_mask: 0x0200_0000,
            reset_command: Some(0x0400_0000),
            ports: (0..2)
                .map(|i| CommandPort {
                    address: 0x2010_f800 + 4 * i,
                    initial: 0,
                    initial_busy_reads: initial[i as usize],
                    busy_reads: busy,
                })
                .collect(),
            cells: [0x026b, 0x0466]
                .into_iter()
                .map(|s| CommandCell {
                    selector: s,
                    initial: if Some(s) == selector { sample } else { 0 },
                    reads: (scripted && Some(s) == selector).then(|| vec![sample]),
                })
                .collect(),
        }),
    }
}

struct Transport {
    entry: u32,
    callbacks: Vec<u32>,
}

impl Transport {
    /// The ROM interface pointer has no PT_LOAD mapping. Its explicit RAM
    /// value selects captured code, not callback response models.
    fn memory(&self, arguments: &[u32]) -> Result<Vec<ExecutionRegion>> {
        Ok(vec![
            known(0x2f07_fc3c, 4, &words(&[0x3fff_3000]))?,
            known(0x3fff_3000, 16, &words(&self.callbacks))?,
            known(0x3fff_4000, 32, &words_padded(arguments, 8, 0)?)?,
        ])
    }

    fn invoke(
        &self,
        target: u32,
        arguments: &[u32],
        models: Vec<DeviceDeclaration>,
    ) -> Result<Invocation> {
        Ok(invocation(
            self.entry,
            vec![Some(target), Some(0x3fff_4000)],
            self.memory(arguments)?,
            models,
            vec![],
        ))
    }

    /// Production reads busy before issuing a read; ROM's org leaf does not.
    /// Compare every write and selected return, retaining all excluded reads.
    #[allow(clippy::too_many_arguments)]
    fn paired(
        &self,
        name: &str,
        left: u32,
        left_words: &[u32],
        right: u32,
        right_words: &[u32],
        models: Vec<DeviceDeclaration>,
        returns: bool,
    ) -> Result<ExecutionCase> {
        let mut row = case(
            name,
            self.invoke(left, left_words, models.clone())?,
            Some(self.invoke(right, right_words, models)?),
            SessionReset::Cold,
            false,
        );
        let relation = row.relation.as_mut().unwrap();
        relation.events.mmio_read = false;
        relation.returns.low = returns;
        Ok(row)
    }
}

/// Expected writes, returned low word and issued command count of one case.
type Expectation = (Vec<(u32, u32)>, Option<u32>, u64);

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let transport = Transport {
        entry: ctx.probe("open_phy_trace_i2c_entry"),
        callbacks: [
            "phy_i2c_enter_critical",
            "phy_i2c_exit_critical",
            "phy_get_i2c_read_mask_new",
            "phy_get_i2c_hostid_new",
        ]
        .map(|n| ctx.root(n))
        .to_vec(),
    };
    let transfer = ctx.probe("open_phy_trace_i2c_transfer");
    let (mut cases, mut expected): (Vec<ExecutionCase>, Vec<Expectation>) = (vec![], vec![]);
    let hosts = [1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0];
    for (block, host) in (0x61..0x6e).zip(hosts) {
        cases.push(transport.paired(
            &format!("host-{block:x}"),
            ctx.root("phy_get_i2c_hostid_new"),
            &[block],
            ctx.probe("open_phy_trace_i2c_host"),
            &[block],
            vec![controls()],
            true,
        )?);
        expected.push((vec![(0x2010_f820, CONFIGURATION)], Some(host), 0));
    }
    let names = [
        "phy_i2c_readReg",
        "phy_i2c_writeReg",
        "phy_i2c_readReg_Mask",
        "phy_i2c_writeReg_Mask",
    ];
    for profile in 0..8u32 {
        let host = profile / 4;
        let (block, register, sample, high, low, maximum, mask, cleared, set_value) = if host == 0 {
            (0x66, 4, 0xa6, 3, 2, 3, 0xffff_ff7f, 0xa2, 0xae)
        } else {
            (0x6b, 2, 0xa5, 7, 4, 15, 0xffff_fff7, 5, 0xf5)
        };
        let selector = (register << 8) | block;
        let mode = profile % 4;
        let target = ctx.captured(1, names[mode as usize]);
        for busy in [0, 2] {
            let values = match mode {
                1 => vec![0, 255],
                3 => vec![0, maximum],
                _ => vec![0],
            };
            for value in values {
                let mut arguments = vec![block, host, register];
                if mode == 1 {
                    arguments.push(value);
                }
                if mode >= 2 {
                    arguments.extend([high, low]);
                }
                if mode == 3 {
                    arguments.push(value);
                }
                let models = vec![
                    controls(),
                    bank(Some(selector), sample, mode != 1, busy, [0, 0]),
                ];
                cases.push(transport.paired(
                    &format!("transfer-{profile}-{busy}-{value}"),
                    target,
                    &arguments,
                    transfer,
                    &[profile, value, 32],
                    models,
                    matches!(mode, 0 | 2),
                )?);
                let mut writes = vec![(0x2010_f820, CONFIGURATION)];
                let port = 0x2010_f800 + 4 * host;
                if mode != 1 {
                    writes.extend([(0x2010_f81c, mask), (port, 0x0400_0000 | selector)]);
                }
                if mode == 3 {
                    writes.push((0x2010_f820, CONFIGURATION));
                }
                if matches!(mode, 1 | 3) {
                    let data = if mode == 1 {
                        value
                    } else if value == 0 {
                        cleared
                    } else {
                        set_value
                    };
                    writes.push((port, 0x0500_0000 | data << 16 | selector));
                }
                let returned = match mode {
                    0 => Some(sample),
                    2 => Some(if host == 0 { 1 } else { 10 }),
                    _ => None,
                };
                expected.push((writes, returned, if mode == 3 { 2 } else { 1 }));
            }
        }
    }
    let reset = ctx.captured(1, "phy_i2c_master_reset");
    let reset_replacement = ctx.probe("open_phy_trace_i2c_reset");
    for (initial, busy) in [([0, 0], 0), ([1, 1], 0), ([1, 1], 2)] {
        cases.push(transport.paired(
            &format!("reset-{}-{busy}", initial[0]),
            reset,
            &[],
            reset_replacement,
            &[],
            vec![bank(None, 0, false, busy, initial)],
            false,
        )?);
        let writes = (0..2)
            .filter(|i| initial[*i as usize] != 0)
            .map(|i| (0x2010_f800 + 4 * i, 0x0400_0000))
            .collect();
        expected.push((
            writes,
            None,
            initial.iter().filter(|v| **v != 0).count() as u64,
        ));
    }
    // Independent cold cases are batched below the native 64-KiB request bound;
    // every declared case and expectation is still executed and retained.
    for (batch, (rows, expectations)) in cases.chunks(10).zip(expected.chunks(10)).enumerate() {
        let records = ctx.compare(
            &format!("transport-{batch}"),
            rows.to_vec(),
            ComparisonVerdict::Match,
            32768,
        )?;
        for (i, (writes, returned, commands)) in expectations.iter().enumerate() {
            let i = i as u32;
            for side in [false, true] {
                let low = returned_low(&records, i, side);
                if let Some(value) = returned {
                    assert_eq!(low, Some(*value), "{batch} {i} {side}");
                }
                assert_eq!(
                    &word_writes(&records, i, side),
                    writes,
                    "{batch} {i} {side}"
                );
                for model in models(&records, i, side) {
                    assert!(
                        model.status == ModelStatus::Complete
                            && model.closed
                            && model.issue.is_none()
                    );
                    if let Some(observed) = model.commands {
                        assert_eq!(observed.pending, 0);
                        assert_eq!(observed.issued, *commands);
                    }
                }
            }
        }
    }
    // No sample fallback: identical exhausted environments are still incomplete.
    let mut exhausted = cases[13].clone();
    exhausted.name = "exhausted-samples".into();
    for side in [Some(&mut exhausted.vendor), exhausted.replacement.as_mut()]
        .into_iter()
        .flatten()
    {
        if let DeviceBehavior::CommandBank(bank) = &mut side.models[1].behavior {
            for cell in &mut bank.cells {
                if cell.reads.is_some() {
                    cell.reads = Some(vec![]);
                }
            }
        }
    }
    let records = ctx.compare(
        "transport-exhausted",
        vec![exhausted],
        ComparisonVerdict::Incomplete,
        32768,
    )?;
    assert!(records.iter().all(|r| match r {
        ExecutionEvidence::Model { observation, .. } if observation.id == "analog" => {
            observation.issue == Some(DeviceIssue::ExhaustedReads)
        }
        _ => true,
    }));
    // The captured masked-write ABI permits out-of-field bits to spill; the
    // production field update clips them. Preserve that known difference.
    let wide = transport.paired(
        "out-of-field-value",
        ctx.captured(1, "phy_i2c_writeReg_Mask"),
        &[0x66, 0, 4, 3, 2, 7],
        transfer,
        &[3, 7, 32],
        vec![controls(), bank(Some(0x0466), 0xa6, true, 0, [0, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-field-domain",
        vec![wide],
        ComparisonVerdict::Diff,
        32768,
    )?;
    let port: Vec<u32> = records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                event:
                    ExecutionEvent::Write {
                        address: 0x2010_f800,
                        value,
                        ..
                    },
                ..
            } => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(port, [0x0400_0466, 0x05be_0466, 0x0400_0466, 0x05ae_0466]);
    // Finite supplied completion edges expire before a delayed read becomes ready.
    let timeout = transport.paired(
        "edge-schedule-exhausted",
        ctx.captured(1, "phy_i2c_readReg"),
        &[0x66, 0, 4],
        transfer,
        &[0, 0, 2],
        vec![controls(), bank(Some(0x0466), 0xa6, true, 4, [0, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-edge-timeout",
        vec![timeout],
        ComparisonVerdict::Incomplete,
        32768,
    )?;
    assert!(matches!(
        stop(&records, 0, true),
        ExecutionStop::Returned {
            low: Some(0x10001),
            ..
        }
    ));
    // The actual shipping reset helper has a finite 10,000-observation bound;
    // ROM keeps polling. Exclude void returns but keep the pending model obligation.
    let timeout = transport.paired(
        "production-reset-timeout",
        reset,
        &[],
        reset_replacement,
        &[],
        vec![bank(None, 0, false, 10001, [1, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-reset-timeout",
        vec![timeout],
        ComparisonVerdict::Incomplete,
        32768,
    )?;
    assert!(matches!(
        stop(&records, 0, true),
        ExecutionStop::Returned {
            low: Some(0x10001),
            ..
        }
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_samples_apply_only_to_the_selected_cell() {
        let DeviceBehavior::CommandBank(bank) = bank(Some(0x0466), 0xa6, true, 2, [1, 0]).behavior
        else {
            panic!("command bank")
        };
        assert_eq!(bank.cells[0].reads, None);
        assert_eq!(bank.cells[1].reads, Some(vec![0xa6]));
        assert_eq!(bank.cells[1].initial, 0xa6);
        assert_eq!(
            (bank.ports[0].initial_busy_reads, bank.ports[1].busy_reads),
            (1, 2)
        );
    }
}
