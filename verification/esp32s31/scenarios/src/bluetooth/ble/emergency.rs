//! Confirmed scheduler-stop paths of the compiled emergency transaction.
//! These cases compare register effects, not DMA completion or coex release.

use super::*;
use blobray_domain::{EffectDisposition, EffectPattern, EffectRule, EffectSelector, EffectValue};

const STATES: &[u32] = &[0, 1, 2, 3, 4];
const COMMAND_ZERO: u32 = 0x2010_1254;
const COMMAND_ONE: u32 = 0x2010_1258;

pub(super) const LEAF: Leaf = in_archive(
    ruled(
        quiet(
            stated(
                objects(
                    leaf(
                        "r_sym_bt_74l62ZLsZuXg67pPHSd7",
                        "open_ble_emergency_stop",
                        &[],
                        false,
                    ),
                    inputs,
                ),
                STATES,
            ),
            &["wr_btdm_log_internal_x0"],
        ),
        rules,
    ),
    BTDM_COMMON_INPUT,
);

fn inputs(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let [state] = words else {
        return Err("emergency stop requires one declared state".into());
    };
    let pairs: &[u32] = match state {
        0 => &[IDLE],
        1 | 3 => &[BUSY, BUSY, IDLE],
        2 => &[BUSY, BUSY, BUSY, BUSY, IDLE],
        4 => &[BUSY, IDLE, IDLE],
        _ => return Err("undeclared emergency-stop state".into()),
    };
    let mut samples = Vec::new();
    if *state == 3 {
        samples.extend([ReadRun::once(IDLE), ReadRun::once(BUSY)]);
    }
    samples.extend(pairs.iter().map(|value| ReadRun {
        value: *value,
        count: PAIR,
    }));
    let mut objects = Objects {
        registers: vec![(COMMAND_ZERO, 1 << 26), (COMMAND_ONE, 1 << 18)],
        sequences: vec![(DIAGNOSTIC_VALUE, samples)],
        ..Objects::default()
    };
    if *state == 2 {
        objects
            .sequences
            .push((COMMAND_ZERO, vec![ReadRun::once(0), ReadRun::once(1 << 26)]));
        objects
            .registers
            .retain(|(address, _)| *address != COMMAND_ZERO);
    }
    Ok(objects)
}

fn rules() -> Vec<EffectRule> {
    let mut rules = busy_reselection_rules();
    // This vendor root selects again on every command/BUSY iteration, just
    // like production. Only the initial unequal diagnostic pair adds a
    // selection: the second selector write, immediately between reads and
    // before the preamble. Do not classify the shared loop selections as
    // additions; those must compare exactly.
    rules[0].name = "initial-unstable-diagnostic-reselection".into();
    if let Some(pattern) = rules[0].replacement.as_mut() {
        pattern.occurrence = Some(2);
    }
    rules[0].max_occurrences = 1;
    rules[0].reason = "the initial unequal pair retries the HAL's selected diagnostic transaction; the vendor retries only its two reads; every outer-loop selection remains required".into();
    rules.push(EffectRule {
        name: "emergency-device-ordering".into(),
        vendor: None,
        replacement: Some(EffectPattern {
            selector: EffectSelector::Fence { predecessor: FULL_FENCE, successor: FULL_FENCE },
            value: EffectValue::Any,
            preceded_by: None, occurrence: None, followed_by: None,
        }),
        disposition: EffectDisposition::Added,
        min_occurrences: 1,
        max_occurrences: 3,
        reason: "production fences the preamble, lifecycle request and confirmed stop; idle only fences its final sample".into(),
    });
    rules
}
