//! Fail-closed validation for finite execution profiles.

use std::collections::BTreeSet;

use crate::{NamedScenario, Result};
use open_radio_vendor_semantics::DriverAdapterClaim;

use super::{ArgumentRange, ArgumentValues, MmioDomain, Profile, ProfileContract};

pub(super) fn validate_coverage_domain(
    profile: &str,
    argument_ranges: &[ArgumentRange],
    argument_values: &[ArgumentValues],
    mmio_domains: &[MmioDomain],
    scenarios: &[NamedScenario],
) -> Result<()> {
    const MAX_DOMAIN_CASES: usize = 4_096;

    for pair in mmio_domains.windows(2) {
        if pair[0].address == pair[1].address {
            return Err(crate::Error::invalid(format!(
                "profile {profile} repeats MMIO domain {:#010x}",
                pair[0].address
            )));
        }
    }
    for domain in mmio_domains {
        if domain.values.is_empty() {
            return Err(crate::Error::invalid(format!(
                "profile {profile} MMIO domain {:#010x} has no values",
                domain.address
            )));
        }
        let unique = domain.values.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != domain.values.len() {
            return Err(crate::Error::invalid(format!(
                "profile {profile} MMIO domain {:#010x} repeats a value",
                domain.address
            )));
        }
    }

    let stub = Profile {
        name: String::new(),
        vendor_source: String::new(),
        vendor_symbol: String::new(),
        rust_symbol: String::new(),
        claim: DriverAdapterClaim::WholeFunctionEquivalence,
        precondition: None,
        contract: ProfileContract::Scenario,
        compare_return: false,
        argument_ranges: argument_ranges.to_vec(),
        argument_values: argument_values.to_vec(),
        mmio_domains: mmio_domains.to_vec(),
        scenarios: Vec::new(),
    };
    let expected = stub.coverage_constraints();
    if expected.len() > MAX_DOMAIN_CASES {
        return Err(crate::Error::invalid(format!(
            "profile {profile} combined argument/MMIO domain has {} cases; maximum is {MAX_DOMAIN_CASES}",
            expected.len()
        )));
    }
    for constraint in expected {
        let covered = scenarios.iter().any(|scenario| {
            argument_ranges.iter().all(|range| {
                scenario.scenario.arguments.get(range.index).copied()
                    == constraint.arguments[range.index]
            }) && argument_values.iter().all(|domain| {
                scenario.scenario.arguments.get(domain.index).copied()
                    == constraint.arguments[domain.index]
            }) && constraint
                .stable_words
                .iter()
                .all(|(address, value)| scenario.scenario.mmio_initial.get(address) == Some(value))
        });
        if !covered {
            let arguments = argument_ranges
                .iter()
                .map(|range| range.index)
                .chain(argument_values.iter().map(|domain| domain.index))
                .map(|index| format!("a{index}={:#x}", constraint.arguments[index].unwrap()));
            let words = constraint
                .stable_words
                .iter()
                .map(|(address, value)| format!("mmio[{address:#010x}]={value:#010x}"));
            return Err(crate::Error::invalid(format!(
                "profile {profile} has no case for admissible coverage combination {}",
                arguments.chain(words).collect::<Vec<_>>().join(", ")
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_scenario(scenario: &NamedScenario) -> Result<()> {
    for (side, goal, services) in [
        (
            "vendor",
            &scenario.vendor_goal,
            &scenario.vendor_fifo_services,
        ),
        ("Rust", &scenario.rust_goal, &scenario.rust_fifo_services),
    ] {
        if let crate::execution_model::ExecutionGoal::ObserveFifoDequeue { service_id, .. } = goal
            && !services.iter().any(|service| &service.id == service_id)
        {
            return Err(crate::Error::invalid(format!(
                "scenario {} {side} goal refers to missing FIFO service {service_id}",
                scenario.name
            )));
        }
    }
    for instance in scenario
        .vendor_memory_instances
        .iter()
        .chain(&scenario.rust_memory_instances)
    {
        if instance.id.is_empty() || instance.length == 0 {
            return Err(crate::Error::invalid(
                "memory instances require a non-empty id and non-zero length",
            ));
        }
        instance
            .base_address
            .checked_add(instance.length)
            .ok_or("memory instance range overflows the 32-bit address space")
            .map_err(crate::Error::invalid)?;
    }
    for observation in scenario
        .vendor_observations
        .iter()
        .chain(&scenario.rust_observations)
    {
        if observation.length() == 0 {
            return Err(crate::Error::invalid(
                "memory observation length must be non-zero",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_argument_domain(
    profile: &str,
    ranges: &[ArgumentRange],
    values: &[ArgumentValues],
    scenarios: &[NamedScenario],
) -> Result<()> {
    const MAX_DOMAIN_CASES: u64 = 4_096;

    for pair in ranges.windows(2) {
        if pair[0].index == pair[1].index {
            return Err(crate::Error::invalid(format!(
                "profile {profile} repeats arg-range {}",
                pair[0].index
            )));
        }
    }
    for pair in values.windows(2) {
        if pair[0].index == pair[1].index {
            return Err(crate::Error::invalid(format!(
                "profile {profile} repeats argument-values {}",
                pair[0].index
            )));
        }
    }
    for range in ranges {
        if values
            .binary_search_by_key(&range.index, |domain| domain.index)
            .is_ok()
        {
            return Err(crate::Error::invalid(format!(
                "profile {profile} constrains argument {} with both a range and explicit values",
                range.index
            )));
        }
    }
    let mut domain_size = 1_u64;
    for range in ranges {
        if range.index >= 8 {
            return Err(crate::Error::invalid(format!(
                "profile {profile} arg-range index exceeds RV32 ABI a0..a7"
            )));
        }
        if range.min > range.max {
            return Err(crate::Error::invalid(format!(
                "profile {profile} arg-range {} has minimum above maximum",
                range.index
            )));
        }
        domain_size = domain_size
            .checked_mul(u64::from(range.max) - u64::from(range.min) + 1)
            .ok_or_else(|| format!("profile {profile} argument domain overflows"))
            .map_err(crate::Error::invalid)?;
        if domain_size > MAX_DOMAIN_CASES {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument domain has {domain_size} cases; maximum is {MAX_DOMAIN_CASES}"
            )));
        }
    }

    for domain in values {
        if domain.index >= 8 {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument-values index exceeds RV32 ABI a0..a7"
            )));
        }
        if domain.values.is_empty() {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument-values {} has no values",
                domain.index
            )));
        }
        let unique = domain.values.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != domain.values.len() {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument-values {} repeats a value",
                domain.index
            )));
        }
        domain_size = domain_size
            .checked_mul(domain.values.len() as u64)
            .ok_or_else(|| format!("profile {profile} argument domain overflows"))
            .map_err(crate::Error::invalid)?;
        if domain_size > MAX_DOMAIN_CASES {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument domain has {domain_size} cases; maximum is {MAX_DOMAIN_CASES}"
            )));
        }
    }

    if ranges.is_empty() && values.is_empty() {
        return Ok(());
    }

    let mut covered = BTreeSet::new();
    for scenario in scenarios {
        let mut projection = [None; 8];
        for range in ranges {
            let value = scenario
                .scenario
                .arguments
                .get(range.index)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "profile {profile} case {} does not provide constrained argument {}",
                        scenario.name, range.index
                    )
                })
                .map_err(crate::Error::invalid)?;
            if !(range.min..=range.max).contains(&value) {
                return Err(crate::Error::invalid(format!(
                    "profile {profile} case {} argument {} value {value:#x} is outside {:#x}..={:#x}",
                    scenario.name, range.index, range.min, range.max
                )));
            }
            projection[range.index] = Some(value);
        }
        for domain in values {
            let value = scenario
                .scenario
                .arguments
                .get(domain.index)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "profile {profile} case {} does not provide constrained argument {}",
                        scenario.name, domain.index
                    )
                })
                .map_err(crate::Error::invalid)?;
            if !domain.values.contains(&value) {
                return Err(crate::Error::invalid(format!(
                    "profile {profile} case {} argument {} value {value:#x} is outside the reviewed set {:?}",
                    scenario.name, domain.index, domain.values
                )));
            }
            projection[domain.index] = Some(value);
        }
        covered.insert(projection);
    }

    let profile_stub = Profile {
        name: String::new(),
        vendor_source: String::new(),
        vendor_symbol: String::new(),
        rust_symbol: String::new(),
        claim: DriverAdapterClaim::WholeFunctionEquivalence,
        precondition: None,
        contract: ProfileContract::Scenario,
        compare_return: false,
        argument_ranges: ranges.to_vec(),
        argument_values: values.to_vec(),
        mmio_domains: Vec::new(),
        scenarios: Vec::new(),
    };
    for expected in profile_stub.coverage_argument_constraints() {
        if !covered.contains(&expected) {
            let values = ranges
                .iter()
                .map(|range| range.index)
                .chain(values.iter().map(|domain| domain.index))
                .map(|index| format!("a{index}={:#x}", expected[index].unwrap()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(crate::Error::invalid(format!(
                "profile {profile} has no case for admissible argument combination {values}"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_claim(
    profile: &str,
    claim: DriverAdapterClaim,
    precondition: Option<&str>,
    ranges: &[ArgumentRange],
    values: &[ArgumentValues],
    mmio_domains: &[MmioDomain],
) -> Result<()> {
    match claim {
        DriverAdapterClaim::WholeFunctionEquivalence => {
            if precondition.is_some() {
                return Err(crate::Error::invalid(format!(
                    "whole-function profile {profile} cannot declare a precondition"
                )));
            }
        }
        DriverAdapterClaim::ReviewedDomainEquivalence => {
            let precondition = precondition
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "reviewed-domain profile {profile} requires a non-empty precondition"
                    ))
                })?;
            if !precondition.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            }) {
                return Err(crate::Error::invalid(format!(
                    "reviewed-domain profile {profile} has invalid precondition {precondition:?}"
                )));
            }
            if ranges.is_empty() && values.is_empty() && mmio_domains.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "reviewed-domain profile {profile} must declare a finite argument or MMIO domain"
                )));
            }
        }
        DriverAdapterClaim::ReviewedProjection | DriverAdapterClaim::RustConformance => {
            return Err(crate::Error::invalid(format!(
                "execution profile {profile} cannot use adapter-only claim {}",
                claim.label()
            )));
        }
    }
    Ok(())
}
