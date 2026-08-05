//! Observable-effect extraction and policy comparison.

use super::*;

pub fn effects_from_observable(events: &[ObservableEvent]) -> Result<Vec<ContractEffect>> {
    let mut effects = Vec::with_capacity(events.len());
    for event in events {
        match event {
            ObservableEvent::Memory {
                access,
                width,
                address,
                register,
                value,
            } => {
                if register == "UNMAPPED" {
                    return Err(format!(
                        "cannot create an effect contract for unmapped MMIO {address:#010x}"
                    )
                    .into());
                }
                let register = RegisterId {
                    address: *address,
                    width: *width,
                    name: register.clone(),
                };
                let effect = match access {
                    MemoryAccess::Read => ContractEffect::MmioRead {
                        register,
                        value: ContractValue::ReadResult {
                            ordinal: effects.len() as u32,
                        },
                    },
                    MemoryAccess::Write => ContractEffect::MmioWrite {
                        register,
                        value: ContractValue::Symbolic(value.clone().ok_or_else(|| {
                            format!("MMIO write at {address:#010x} has no modeled value")
                        })?),
                    },
                };
                effects.push(effect);
            }
            ObservableEvent::Fence { .. } => {
                return Err("memory fence has no Effect Contract v1 classification".into());
            }
        }
    }
    Ok(effects)
}

pub fn compare_effects(
    vendor: &[ContractEffect],
    rust: &[ContractEffect],
    policy: &EffectPolicy,
) -> Result<EffectComparisonVerdict> {
    let mut rust_index = 0_usize;
    let mut used_rules = BTreeSet::new();
    for (vendor_index, vendor_effect) in vendor.iter().enumerate() {
        let selector = vendor_effect.selector();
        let disposition = policy.disposition(&selector).ok_or_else(|| {
            format!(
                "unclassified vendor effect at index {vendor_index}: {}",
                selector.canonical()
            )
        })?;
        used_rules.insert(selector.clone());
        match disposition {
            EffectDisposition::Required | EffectDisposition::PlatformOwned => {
                let Some(rust_effect) = rust.get(rust_index) else {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "required {} is missing from Rust effects",
                        selector.canonical()
                    )));
                };
                if !vendor_effect.equivalent(rust_effect) {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "vendor effect {} does not match Rust effect {} at index {rust_index}",
                        selector.canonical(),
                        rust_effect.selector().canonical()
                    )));
                }
                rust_index += 1;
            }
            EffectDisposition::ReplacedByAsync { condition, timeout } => {
                let Some(ContractEffect::AwaitReady {
                    condition: rust_condition,
                    timeout: rust_timeout,
                }) = rust.get(rust_index)
                else {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "{} requires one Rust await-ready replacement",
                        selector.canonical()
                    )));
                };
                if rust_condition.id() != *condition || rust_timeout != timeout {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "{} requires await-ready {condition} {}, received await-ready {} {}",
                        selector.canonical(),
                        timeout.canonical(),
                        rust_condition.id(),
                        rust_timeout.canonical(),
                    )));
                }
                rust_index += 1;
            }
            EffectDisposition::PlatformProvidedInput { input } => {
                let expected = ContractEffect::PlatformProvidedInput {
                    input: input.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::PlatformProvidedService { service } => {
                let expected = ContractEffect::PlatformProvidedService {
                    service: service.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::PublishedEvent { event } => {
                let expected = ContractEffect::PublishedEvent {
                    event: event.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::InitializationPrerequisite { prerequisite } => {
                let expected = ContractEffect::InitializationPrerequisite {
                    prerequisite: prerequisite.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::AllowedOmission(_) => {
                if rust
                    .get(rust_index)
                    .is_some_and(|rust_effect| vendor_effect.equivalent(rust_effect))
                {
                    rust_index += 1;
                }
            }
            EffectDisposition::Forbidden => {
                return Err(format!(
                    "forbidden vendor effect at index {vendor_index}: {}",
                    selector.canonical()
                )
                .into());
            }
        }
    }
    if let Some(extra) = rust.get(rust_index) {
        return Err(format!(
            "unclassified extra Rust effect at index {rust_index}: {}",
            extra.selector().canonical()
        )
        .into());
    }
    for (selector, disposition) in policy.rules() {
        if disposition != &EffectDisposition::Forbidden && !used_rules.contains(selector) {
            return Err(format!(
                "declared effect rule was not exercised: {}",
                selector.canonical()
            )
            .into());
        }
    }
    Ok(EffectComparisonVerdict::Match)
}

fn consume_replacement(
    vendor_selector: &EffectSelector,
    expected: &ContractEffect,
    rust: &[ContractEffect],
    rust_index: usize,
) -> core::result::Result<usize, String> {
    let Some(actual) = rust.get(rust_index) else {
        return Err(format!(
            "{} requires Rust replacement {}, but it is missing",
            vendor_selector.canonical(),
            expected.selector().canonical(),
        ));
    };
    if !expected.equivalent(actual) {
        return Err(format!(
            "{} requires Rust replacement {}, received {} at index {rust_index}",
            vendor_selector.canonical(),
            expected.selector().canonical(),
            actual.selector().canonical(),
        ));
    }
    Ok(rust_index + 1)
}
