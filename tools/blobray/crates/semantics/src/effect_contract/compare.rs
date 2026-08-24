//! Observable-effect extraction and policy comparison.

use super::*;
use crate::{EquivalenceMode, EquivalenceOutcome};

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
            ObservableEvent::Fence {
                fm,
                predecessor,
                successor,
            } => effects.push(ContractEffect::Fence {
                fm: *fm,
                predecessor: *predecessor,
                successor: *successor,
            }),
        }
    }
    Ok(effects)
}

pub fn compare_effects(
    vendor: &[ContractEffect],
    rust: &[ContractEffect],
    policy: &EffectPolicy,
) -> Result<EquivalenceOutcome> {
    let mut rust_index = 0_usize;
    let mut used_rules = BTreeSet::new();
    for (vendor_index, vendor_effect) in vendor.iter().enumerate() {
        rust_index = consume_rust_additions(rust, rust_index, policy, &mut used_rules);
        let selector = vendor_effect.selector();
        let Some(disposition) = policy.disposition(&selector) else {
            return Ok(EquivalenceOutcome::incomplete(
                EquivalenceMode::Semantic,
                format!(
                    "unclassified vendor effect at index {vendor_index}: {}",
                    selector.canonical()
                ),
            ));
        };
        used_rules.insert(selector.clone());
        match disposition {
            EffectDisposition::Required
            | EffectDisposition::RequiredWhenObserved
            | EffectDisposition::PlatformOwned => {
                let Some(rust_effect) = rust.get(rust_index) else {
                    return Ok(EquivalenceOutcome::different(
                        EquivalenceMode::Semantic,
                        format!(
                            "required {} is missing from Rust effects",
                            selector.canonical()
                        ),
                    ));
                };
                if !vendor_effect.equivalent(rust_effect) {
                    return Ok(EquivalenceOutcome::different(
                        EquivalenceMode::Semantic,
                        format!(
                            "vendor effect {} does not match Rust effect {} at index {rust_index}",
                            selector.canonical(),
                            rust_effect.selector().canonical()
                        ),
                    ));
                }
                rust_index += 1;
            }
            EffectDisposition::ReplacedByAsync { condition, timeout } => {
                let Some(ContractEffect::AwaitReady {
                    condition: rust_condition,
                    timeout: rust_timeout,
                }) = rust.get(rust_index)
                else {
                    return Ok(EquivalenceOutcome::different(
                        EquivalenceMode::Semantic,
                        format!(
                            "{} requires one Rust await-ready replacement",
                            selector.canonical()
                        ),
                    ));
                };
                if rust_condition.id() != *condition || rust_timeout != timeout {
                    return Ok(EquivalenceOutcome::different(
                        EquivalenceMode::Semantic,
                        format!(
                            "{} requires await-ready {condition} {}, received await-ready {} {}",
                            selector.canonical(),
                            timeout.canonical(),
                            rust_condition.id(),
                            rust_timeout.canonical(),
                        ),
                    ));
                }
                rust_index += 1;
            }
            EffectDisposition::PlatformProvidedInput { input } => {
                let expected = ContractEffect::PlatformProvidedInput {
                    input: input.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => {
                        return Ok(EquivalenceOutcome::different(
                            EquivalenceMode::Semantic,
                            reason,
                        ));
                    }
                };
            }
            EffectDisposition::PlatformProvidedService { service } => {
                let expected = ContractEffect::PlatformProvidedService {
                    service: service.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => {
                        return Ok(EquivalenceOutcome::different(
                            EquivalenceMode::Semantic,
                            reason,
                        ));
                    }
                };
            }
            EffectDisposition::PublishedEvent { event } => {
                let expected = ContractEffect::PublishedEvent {
                    event: event.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => {
                        return Ok(EquivalenceOutcome::different(
                            EquivalenceMode::Semantic,
                            reason,
                        ));
                    }
                };
            }
            EffectDisposition::InitializationPrerequisite { prerequisite } => {
                let expected = ContractEffect::InitializationPrerequisite {
                    prerequisite: prerequisite.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => {
                        return Ok(EquivalenceOutcome::different(
                            EquivalenceMode::Semantic,
                            reason,
                        ));
                    }
                };
            }
            EffectDisposition::RustAddition(_) => {
                return Ok(EquivalenceOutcome::incomplete(
                    EquivalenceMode::Semantic,
                    format!(
                        "rust-addition rule unexpectedly matched vendor effect at index {vendor_index}: {}",
                        selector.canonical()
                    ),
                ));
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
                return Ok(EquivalenceOutcome::different(
                    EquivalenceMode::Semantic,
                    format!(
                        "forbidden vendor effect at index {vendor_index}: {}",
                        selector.canonical()
                    ),
                ));
            }
        }
    }
    rust_index = consume_rust_additions(rust, rust_index, policy, &mut used_rules);
    if let Some(extra) = rust.get(rust_index) {
        return Ok(EquivalenceOutcome::incomplete(
            EquivalenceMode::Semantic,
            format!(
                "unclassified extra Rust effect at index {rust_index}: {}",
                extra.selector().canonical()
            ),
        ));
    }
    for (selector, disposition) in policy.rules() {
        if !matches!(
            disposition,
            EffectDisposition::Forbidden | EffectDisposition::RequiredWhenObserved
        ) && !used_rules.contains(selector)
        {
            return Ok(EquivalenceOutcome::incomplete(
                EquivalenceMode::Semantic,
                format!(
                    "declared effect rule was not exercised: {}",
                    selector.canonical()
                ),
            ));
        }
    }
    Ok(EquivalenceOutcome::matched(EquivalenceMode::Semantic))
}

fn consume_rust_additions(
    rust: &[ContractEffect],
    mut rust_index: usize,
    policy: &EffectPolicy,
    used_rules: &mut BTreeSet<EffectSelector>,
) -> usize {
    while let Some(effect) = rust.get(rust_index) {
        let selector = effect.selector();
        if !matches!(
            policy.disposition(&selector),
            Some(EffectDisposition::RustAddition(_))
        ) {
            break;
        }
        used_rules.insert(selector);
        rust_index += 1;
    }
    rust_index
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
